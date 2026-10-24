use super::*;
use crate::context::{
    CONFIG_DIAGNOSTIC, Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_ctx_stats,
};
use crate::release::{LAYOUT_FROZEN, LAYOUT_MAGIC, LAYOUT_VERSION, Layout};
use crate::sealed_region::Charge;
use crate::{alloc, count, release, roots};
use std::ptr;

fn with_context(test: impl FnOnce(*mut TaskCtx)) {
    let plain = Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: ptr::null(),
        step_count: 0,
        field_count: 0,
        logical: 0,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: u64::MAX,
        flags: 0,
    };
    let shared = Layout {
        flags: LAYOUT_FROZEN,
        ..plain
    };
    let descriptors = [ptr::from_ref(&plain), ptr::from_ref(&shared)];
    let config = Config {
        flags: CONFIG_DIAGNOSTIC,
        layouts: descriptors.as_ptr(),
        layout_count: descriptors.len() as u32,
        ..Config::new()
    };
    let mut ctx = ptr::null_mut();
    assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
    test(ctx);
    let mut stats = Stats::default();
    unsafe { hirsel_rt_v2_ctx_stats(ctx, &mut stats) };
    assert_eq!(stats.live_blocks, 0);
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}
fn node(
    run: MetadataRun,
    origin: semantic::Origin,
    logical: u64,
    children: &[Receipt<Record>],
) -> Receipt<Record> {
    semantic::snapshot(run, origin, Charge::Bytes(logical), &[], 1, children).unwrap()
}
fn accept<T>(result: Result<T, Rejected>) -> T {
    match result {
        Ok(value) => value,
        Err((error, _)) => panic!("unexpected rejected source event: {error:?}"),
    }
}
unsafe fn heap(ctx: *mut TaskCtx, descriptor: u32) -> *mut Header {
    let block = unsafe { alloc::hirsel_rt_v2_alloc(ctx, descriptor, 8, 8, crate::DOMAIN_HEAP, 0) };
    unsafe { alloc::payload(block).cast::<u64>().write(7) };
    block
}
unsafe fn retire(ctx: *mut TaskCtx, block: *mut Header) {
    assert_eq!(
        unsafe { release::release_block(&mut *ctx, block, ptr::null(), 0, 0) },
        crate::STATUS_OK
    );
}

#[test]
fn source_credit_transfer_and_metadata_clones_do_not_increment_counts() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, source);
        count::hirsel_rt_v2_dup(ctx, source);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let root = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[leaf.clone(), leaf],
        );
        let id = accept(journal.allocate(Update {
            value: root.clone(),
            additions: vec![source, source],
        }));
        assert_eq!(journal.holders(id).unwrap(), &[source, source]);
        assert_eq!((*source).count, 3);
        let snapshot = root.clone();
        drop(root);
        assert_eq!((*source).count, 3);
        drop(snapshot);
        assert_eq!((*source).count, 3);
        // The dead source binding leaves two retained credits in the journal.
        drop(journal);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn unique_updates_move_kept_credits_and_retire_only_removed_ones() {
    with_context(|ctx| unsafe {
        let first = heap(ctx, 2);
        let second = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, first);
        count::hirsel_rt_v2_dup(ctx, second);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let one = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            8,
            std::slice::from_ref(&leaf),
        );
        let id = accept(journal.allocate(Update {
            value: one.clone(),
            additions: vec![first],
        }));
        let two = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[leaf.clone(), leaf.clone()],
        );
        let old = accept(journal.replace(
            id,
            &[0],
            Update {
                value: two.clone(),
                additions: vec![second],
            },
        ));
        assert_eq!(old.generation(), one.generation());
        assert_eq!(journal.holders(id).unwrap(), &[first, second]);
        assert_eq!(((*first).count, (*second).count), (2, 2));
        assert_eq!(journal.inventory().seal_charge(&two), Ok(Charge::Bytes(6)));
        let last = node(run, semantic::Origin::Arena(journal.grant()), 8, &[leaf]);
        accept(journal.replace(
            id,
            &[1],
            Update {
                value: last.clone(),
                additions: vec![],
            },
        ));
        assert_eq!(((*first).count, (*second).count), (1, 2));
        assert_eq!(journal.holders(id).unwrap(), &[second]);
        assert_eq!(journal.inventory().seal_charge(&last), Ok(Charge::Bytes(3)));
        assert_eq!(old.record().outgoing_heap_charge(), Charge::Bytes(3));
        drop(journal);
        assert_eq!((*second).count, 1);
        retire(ctx, first);
        retire(ctx, second);
    });
}

#[test]
fn semantic_cow_keeps_old_entry_and_its_actual_holder_credit() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        for _ in 0..3 {
            count::hirsel_rt_v2_dup(ctx, source);
        }
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let one = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            8,
            std::slice::from_ref(&leaf),
        );
        let old_id = accept(journal.allocate(Update {
            value: one,
            additions: vec![source],
        }));
        let two = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[leaf.clone(), leaf],
        );
        let new_id = accept(journal.allocate(Update {
            value: two.clone(),
            additions: vec![source, source],
        }));
        assert_ne!(old_id, new_id);
        assert_eq!(journal.holders(old_id).unwrap().len(), 1);
        assert_eq!(journal.holders(new_id).unwrap().len(), 2);
        assert_eq!(journal.inventory().seal_charge(&two), Ok(Charge::Bytes(9)));
        assert_eq!((*source).count, 4);
        drop(journal);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn shared_credits_preserve_multiplicity_through_update_and_scope_cleanup() {
    with_context(|ctx| unsafe {
        let shared = heap(ctx, 3);
        roots::hirsel_rt_v2_root_dup(ctx, shared);
        roots::hirsel_rt_v2_root_dup(ctx, shared);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Shared, 0, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let root = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[leaf.clone(), leaf.clone()],
        );
        let id = accept(journal.allocate(Update {
            value: root.clone(),
            additions: vec![shared, shared],
        }));
        assert_eq!(roots::count(journal.ctx, shared), 3);
        let clone = root.clone();
        drop(root);
        drop(clone);
        assert_eq!(roots::count(journal.ctx, shared), 3);
        let next = node(run, semantic::Origin::Arena(journal.grant()), 8, &[leaf]);
        accept(journal.replace(
            id,
            &[1],
            Update {
                value: next.clone(),
                additions: vec![],
            },
        ));
        assert_eq!(roots::count(journal.ctx, shared), 2);
        assert_eq!(journal.inventory().seal_charge(&next), Ok(Charge::Bytes(0)));
        drop(journal);
        assert_eq!(roots::count(&*ctx, shared), 1);
        roots::hirsel_rt_v2_root_release(ctx, shared);
    });
}

#[test]
fn rejected_events_return_input_and_leave_both_metadata_and_credits_unchanged() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, source);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let rejected = journal.allocate(Update {
            value: leaf.clone(),
            additions: vec![source],
        });
        let Err((Error::Semantic(semantic::Error::WrongAllocationOrigin), input)) = rejected else {
            panic!("expected wrong allocation origin")
        };
        assert_eq!(input.additions, vec![source]);
        assert_eq!(input.value.generation(), leaf.generation());
        assert_eq!((*source).count, 2);
        assert_eq!(journal.inventory().entries().len(), 0);
        let good = node(run, semantic::Origin::Arena(journal.grant()), 8, &[leaf]);
        let id = accept(journal.allocate(Update {
            value: good.clone(),
            additions: input.additions,
        }));
        count::hirsel_rt_v2_dup(journal.ctx, source);
        let input = Update {
            value: good.clone(),
            additions: vec![source],
        };
        let Err((Error::InvalidRetention, input)) = journal.replace(id, &[0, 0], input) else {
            panic!("expected duplicate retention rejection")
        };
        assert_eq!(input.value.generation(), good.generation());
        assert_eq!((*source).count, 3);
        assert_eq!(journal.holders(id).unwrap(), &[source]);
        let mut other = Inventory::new(run);
        let other_value = node(run, semantic::Origin::Arena(other.grant()), 0, &[]);
        let other_id = other.allocate(other_value).unwrap();
        let Err((Error::Semantic(semantic::Error::UnknownAllocation), input)) =
            journal.replace(other_id, &[], input)
        else {
            panic!("expected foreign identity rejection")
        };
        assert_eq!((*source).count, 3);
        // Still caller-owned after both rejections; explicitly retire it once.
        retire(journal.ctx, input.additions[0]);
        assert_eq!((*source).count, 2);
        assert_eq!(
            journal.inventory().entries().next().unwrap().1.generation(),
            good.generation()
        );
        drop(journal);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn failed_seal_charge_preflight_keeps_source_custody_until_journal_unwind() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, source);
        count::hirsel_rt_v2_dup(ctx, source);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let retained = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[leaf.clone(), leaf],
        );
        accept(journal.allocate(Update {
            value: retained,
            additions: vec![source, source],
        }));
        let scalar = node(run, semantic::Origin::Inline, 0, &[]);
        let charge = journal.inventory().seal_charge(&scalar).unwrap();
        assert_eq!(charge, Charge::Bytes(6));
        assert!(!matches!(charge, Charge::Bytes(bytes) if bytes <= 21 - 16));
        assert_eq!((*source).count, 3);
        drop(journal);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn invalid_holders_and_foreign_or_wrong_origin_replacements_cannot_retire_old_credits() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, source);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let good = node(run, semantic::Origin::Arena(journal.grant()), 0, &[]);
        let Err((Error::InvalidHolder, null_input)) = journal.allocate(Update {
            value: good.clone(),
            additions: vec![ptr::null_mut()],
        }) else {
            panic!("expected checked null rejection")
        };
        assert!(null_input.additions[0].is_null());
        assert_eq!(journal.inventory().entries().len(), 0);
        let id = accept(journal.allocate(Update {
            value: good.clone(),
            additions: vec![source],
        }));
        for bad_keep in [vec![1], vec![1, 0]] {
            let Err((Error::InvalidRetention, _)) = journal.replace(
                id,
                &bad_keep,
                Update {
                    value: good.clone(),
                    additions: vec![],
                },
            ) else {
                panic!("expected invalid retention")
            };
            assert_eq!((*source).count, 2);
        }
        let wrong_origin = node(run, semantic::Origin::Inline, 0, &[]);
        let Err((Error::Semantic(semantic::Error::WrongAllocationOrigin), _)) = journal.replace(
            id,
            &[],
            Update {
                value: wrong_origin,
                additions: vec![],
            },
        ) else {
            panic!("expected wrong replacement origin")
        };
        let foreign = node(
            MetadataRun::new(),
            semantic::Origin::Arena(journal.grant()),
            0,
            &[],
        );
        let Err((
            Error::Semantic(semantic::Error::Metadata(super::super::Error::DifferentRun)),
            returned,
        )) = journal.replace(
            id,
            &[],
            Update {
                value: foreign,
                additions: vec![],
            },
        )
        else {
            panic!("expected foreign metadata run")
        };
        assert_ne!(returned.value.run(), run);
        assert_eq!((*source).count, 2);
        assert_eq!(journal.holders(id).unwrap(), &[source]);
        assert_eq!(
            journal.inventory().entries().next().unwrap().1.generation(),
            good.generation()
        );
        drop(journal);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}
