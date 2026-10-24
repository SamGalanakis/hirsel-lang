use super::*;
use crate::region::{hirsel_rt_v2_region_reserve, hirsel_rt_v2_region_schedule};
use crate::release::{NO_OFFSET, STEP_RELEASE_ROOT, Step, WHOLE_VALUE};
use crate::sealed_region::transaction::Event;
use crate::sealed_region::{Plan, Root, SealError as TransactionError, drop_owner};
use seal_tests::{close, enter};

fn with_order_context(test: impl FnOnce(*mut TaskCtx)) {
    let root_step = Step {
        kind: STEP_RELEASE_ROOT,
        field: WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset: 0,
        stride: 0,
        count: 0,
        count_offset: NO_OFFSET,
        layout: ptr::null(),
    };
    let plain = Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: ptr::null(),
        step_count: 0,
        field_count: 1,
        logical: 0,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: u64::MAX,
        flags: 0,
    };
    let layouts = [
        plain,
        Layout {
            flags: LAYOUT_FROZEN,
            ..plain
        },
        Layout {
            steps: ptr::from_ref(&root_step),
            step_count: 1,
            ..plain
        },
    ];
    let descriptors: Vec<_> = layouts.iter().map(ptr::from_ref).collect();
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

fn update(run: MetadataRun, grant: GrantId, words: &[*mut Header]) -> Update {
    let shared = node(run, semantic::Origin::Shared, 0, &[]);
    Update {
        value: node(
            run,
            semantic::Origin::Arena(grant),
            words.len() as u64 * 8,
            &vec![shared; words.len()],
        ),
        additions: words.to_vec(),
    }
}

/// Allocate distinct roots with one external and `copies` source credits each.
unsafe fn sources<const N: usize>(ctx: *mut TaskCtx, copies: usize) -> [*mut Header; N] {
    std::array::from_fn(|_| unsafe {
        let root = heap(ctx, 3);
        for _ in 0..copies {
            roots::hirsel_rt_v2_root_dup(ctx, root);
        }
        root
    })
}

unsafe fn release_external(ctx: *mut TaskCtx, words: &[*mut Header]) {
    for &word in words {
        assert_eq!(unsafe { roots::count(&*ctx, word) }, 1);
        unsafe { roots::hirsel_rt_v2_root_release(ctx, word) };
    }
}

fn plan(scalar: &mut u64, shared: &[*mut Header]) -> Plan {
    Plan {
        root: Root {
            descriptor: 2,
            source: ptr::from_mut(scalar).cast(),
            extent: 8,
            align: 8,
            patches: vec![],
        },
        blocks: vec![],
        shared: shared.to_vec(),
        charge: Charge::Bytes(0),
    }
}

#[test]
fn interleaved_allocations_and_growth_match_actual_typed_region_cleanup_order() {
    with_order_context(|ctx| unsafe {
        let [a, b, c] = sources(ctx, 2);
        let mut arena = enter(ctx);
        (*ctx).roots.speculative = true;
        // Use actual inline Shared cleanup, whose STEP_RELEASE_ROOT respects
        // root deferral. region_hold is the ordinary block-credit operation.
        for source in [a, b, c] {
            let mut cell = ptr::null_mut();
            assert_eq!(
                hirsel_rt_v2_region_reserve(ctx, &mut *arena, 8, 8, &mut cell),
                0
            );
            cell.cast::<*mut Header>().write(source);
            assert_eq!(hirsel_rt_v2_region_schedule(ctx, &mut *arena, 4, cell), 0);
        }
        close(ctx, &mut arena);
        let actual_physical = (*ctx).roots.deferred_for_test().to_vec();
        assert_eq!(actual_physical, [c, b, a]);
        for source in [a, b, c] {
            assert_eq!(roots::count(&*ctx, source), 3);
        }
        roots::retire(&mut *ctx);
        for source in [a, b, c] {
            assert_eq!(roots::count(&*ctx, source), 2);
        }

        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let grant = journal.grant();
        let first = accept(journal.allocate(update(run, grant, &[a])));
        accept(journal.allocate(update(run, grant, &[b])));
        let mut grown = update(run, grant, &[a, c]);
        grown.additions = vec![c];
        accept(journal.replace(first, &[0], grown));
        journal.ctx.as_mut().roots.speculative = true;
        drop(journal);
        assert_eq!((*ctx).roots.deferred_for_test(), actual_physical);
        for source in [a, b, c] {
            assert_eq!(roots::count(&*ctx, source), 2);
        }
        roots::retire(&mut *ctx);
        release_external(ctx, &[a, b, c]);
    });
}

#[test]
fn transfer_out_erases_enrollment_and_omitted_credits_retire_newest_first() {
    with_context(|ctx| unsafe {
        let [a, b, c, d] = sources(ctx, 1);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let grant = journal.grant();
        let id = accept(journal.allocate(update(run, grant, &[a, b, c, d])));
        journal.ctx.as_mut().roots.speculative = true;
        let mut next = update(run, grant, &[a]);
        next.additions.clear();
        let result = accept(journal.replace_transfer(id, &[0], &[1], next));
        assert_eq!(result.transferred, [b]);
        assert_eq!(journal.ctx.as_ref().roots.deferred_for_test(), [d, c]);
        drop(journal);
        assert_eq!((*ctx).roots.deferred_for_test(), [d, c, a]);
        roots::retire(&mut *ctx);
        assert_eq!(roots::count(&*ctx, b), 2);
        roots::hirsel_rt_v2_root_release(ctx, result.transferred[0]);
        release_external(ctx, &[a, b, c, d]);
    });
}

#[test]
fn cow_enrolls_distinct_credits_even_when_source_words_repeat() {
    with_context(|ctx| unsafe {
        let [a, b, c] = sources(ctx, 1);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let grant = journal.grant();
        let id = accept(journal.allocate(update(run, grant, &[a])));
        accept(journal.allocate(update(run, grant, &[b])));
        let mut next = update(run, grant, &[a, c]);
        next.additions = vec![c];
        accept(journal.replace(id, &[0], next));
        roots::hirsel_rt_v2_root_dup(journal.ctx.as_ptr(), a);
        roots::hirsel_rt_v2_root_dup(journal.ctx.as_ptr(), c);
        let cow = accept(journal.allocate(update(run, grant, &[a, c])));
        assert_ne!(id, cow);
        journal.ctx.as_mut().roots.speculative = true;
        drop(journal);
        assert_eq!((*ctx).roots.deferred_for_test(), [c, a, c, b, a]);
        roots::retire(&mut *ctx);
        release_external(ctx, &[a, b, c]);
    });
}

#[test]
fn metadata_rejection_keeps_enrollment_and_returns_unenrolled_additions() {
    with_context(|ctx| unsafe {
        let [a, b, c] = sources(ctx, 1);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let grant = journal.grant();
        let old = update(run, grant, &[a]);
        let old_snapshot = old.value.clone();
        let id = accept(journal.allocate(old));
        accept(journal.allocate(update(run, grant, &[b])));
        let Err((Error::Semantic(semantic::Error::WrongAllocationOrigin), returned)) = journal
            .replace_transfer(
                id,
                &[],
                &[0],
                Update {
                    value: node(run, semantic::Origin::Inline, 0, &[]),
                    additions: vec![c],
                },
            )
        else {
            panic!("expected wrong-origin refusal")
        };
        assert_eq!(returned.additions, [c]);
        assert_eq!(
            journal.inventory().entries().next().unwrap().1.generation(),
            old_snapshot.generation()
        );
        assert!(!journal.ctx.as_ref().roots.has_pending());
        accept(journal.allocate(update(run, grant, &returned.additions)));
        journal.ctx.as_mut().roots.speculative = true;
        drop(journal);
        assert_eq!((*ctx).roots.deferred_for_test(), [c, b, a]);
        roots::retire(&mut *ctx);
        release_external(ctx, &[a, b, c]);
    });
}

#[test]
fn credit_identity_exhaustion_refuses_without_wrap_or_lost_evidence() {
    with_context(|ctx| unsafe {
        let [a, b] = sources(ctx, 1);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let grant = journal.grant();
        custody::exhaust_for_test(&mut journal.custody, Some(u64::MAX));
        let Err((Error::CreditIdExhausted, returned)) =
            journal.allocate(update(run, grant, &[a, b]))
        else {
            panic!("expected checked identity refusal")
        };
        assert_eq!(returned.additions, [a, b]);
        assert_eq!(journal.inventory().entries().len(), 0);
        let id = accept(journal.allocate(update(run, grant, &[a])));
        let Err((Error::CreditIdExhausted, returned)) =
            journal.replace(id, &[0], update(run, grant, &[b]))
        else {
            panic!("expected exhausted identity refusal")
        };
        assert_eq!(returned.additions, [b]);
        assert_eq!(journal.holders(id).unwrap(), [a]);
        // A keep/empty event needs no new identity and remains valid.
        let mut keep = update(run, grant, &[a]);
        keep.additions.clear();
        accept(journal.replace(id, &[0], keep));
        accept(journal.allocate(update(run, grant, &[])));
        journal.ctx.as_mut().roots.speculative = true;
        drop(journal);
        assert_eq!((*ctx).roots.deferred_for_test(), [a]);
        roots::retire(&mut *ctx);
        roots::hirsel_rt_v2_root_release(ctx, b); // rejected caller credit
        release_external(ctx, &[a, b]);
    });
}

#[test]
fn publication_retires_journal_in_enrollment_order_and_disarms_all_source_cleanup() {
    with_context(|ctx| unsafe {
        let [a, b, c] = sources(ctx, 1);
        let mut arena = enter(ctx);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let grant = journal.grant();
        let id = accept(journal.allocate(update(run, grant, &[a])));
        accept(journal.allocate(update(run, grant, &[b])));
        let mut grown = update(run, grant, &[a, c]);
        grown.additions = vec![c];
        accept(journal.replace(id, &[0], grown));
        let root = node(run, semantic::Origin::Inline, 0, &[]);
        let mut scalar = 7;
        let plan = plan(&mut scalar, &[a, b, c]);
        journal.ctx.as_mut().roots.speculative = true;
        let mut ready = false;
        let owner = match journal.seal_with(&mut *arena, &root, &plan, &mut |event| {
            if event == Event::Ready {
                ready = true;
                for source in [a, b, c] {
                    assert_eq!((*source).count, 3);
                }
            }
            Ok(())
        }) {
            Ok(owner) => owner,
            Err((error, _)) => panic!("unexpected {error:?}"),
        };
        assert!(ready);
        assert_eq!((*ctx).roots.deferred_for_test(), [c, b, a]);
        close(ctx, &mut arena);
        assert_eq!((*ctx).roots.deferred_for_test(), [c, b, a]);
        roots::retire(&mut *ctx);
        for source in [a, b, c] {
            assert_eq!(roots::count(&*ctx, source), 2);
        }
        drop_owner(ctx, owner);
        release_external(ctx, &[a, b, c]);
    });
}

#[test]
fn ready_abort_preserves_journal_order_after_separate_destination_rollback() {
    with_context(|ctx| unsafe {
        let [a, b, c] = sources(ctx, 1);
        let mut arena = enter(ctx);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let grant = journal.grant();
        let id = accept(journal.allocate(update(run, grant, &[a])));
        accept(journal.allocate(update(run, grant, &[b])));
        let mut grown = update(run, grant, &[a, c]);
        grown.additions = vec![c];
        accept(journal.replace(id, &[0], grown));
        let root = node(run, semantic::Origin::Inline, 0, &[]);
        let mut scalar = 7;
        let plan = plan(&mut scalar, &[a, b, c]);
        journal.ctx.as_mut().roots.speculative = true;
        let Err((SealError::Transaction(TransactionError::Interrupted), journal)) = journal
            .seal_with(&mut *arena, &root, &plan, &mut |event| {
                if event == Event::Ready {
                    Err(TransactionError::Interrupted)
                } else {
                    Ok(())
                }
            })
        else {
            panic!("expected ready-stage interruption")
        };
        // Only the three private destination acquisitions rolled back.
        assert_eq!(journal.ctx.as_ref().roots.deferred_for_test(), [c, b, a]);
        assert_eq!(journal.holders(id).unwrap(), [a, c]);
        roots::retire(&mut *journal.ctx.as_ptr());
        for source in [a, b, c] {
            assert_eq!(roots::count(journal.ctx.as_ref(), source), 2);
        }
        let mut journal = journal;
        journal.ctx.as_mut().roots.speculative = true;
        drop(journal);
        assert_eq!((*ctx).roots.deferred_for_test(), [c, b, a]);
        roots::retire(&mut *ctx);
        close(ctx, &mut arena);
        release_external(ctx, &[a, b, c]);
    });
}
