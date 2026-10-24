use super::super::receipt::semantic::{self, Origin};
use super::super::{Block, Charge, Placement, Root, drop_owner, root};
use super::*;
use crate::context::{
    CONFIG_DIAGNOSTIC, Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_ctx_stats,
};
use crate::release::{LAYOUT_FROZEN, LAYOUT_MAGIC, LAYOUT_VERSION, Layout};
use crate::{alloc, count, release, roots};

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
    assert_eq!(stats.active_regions, 0);
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}
fn node(
    run: MetadataRun,
    origin: Origin,
    logical: u64,
    children: &[Receipt<Record>],
) -> Receipt<Record> {
    semantic::snapshot(run, origin, Charge::Bytes(logical), &[], 1, children).unwrap()
}
fn accept<T>(result: Result<T, (Error, Update)>) -> T {
    match result {
        Ok(value) => value,
        Err((error, _)) => panic!("unexpected journal refusal {error:?}"),
    }
}
unsafe fn heap(ctx: *mut TaskCtx, index: u32) -> *mut crate::Header {
    let block = unsafe { alloc::hirsel_rt_v2_alloc(ctx, index, 8, 8, crate::DOMAIN_HEAP, 0) };
    unsafe { alloc::payload(block).cast::<u64>().write(7) };
    block
}
unsafe fn retire(ctx: *mut TaskCtx, block: *mut crate::Header) {
    assert_eq!(
        unsafe { release::release_block(&mut *ctx, block, ptr::null(), 0, 0) },
        STATUS_OK
    )
}
fn plan(scalar: &mut u64, source: Option<*mut crate::Header>, charge: u64) -> Plan {
    Plan {
        root: Root {
            descriptor: 2,
            source: ptr::from_mut(scalar).cast(),
            extent: 8,
            align: 8,
            patches: vec![],
        },
        blocks: source
            .into_iter()
            .map(|source| Block {
                source,
                descriptor: 2,
                extent: 8,
                align: 8,
                placement: Placement::Evacuate,
                patches: vec![],
            })
            .collect(),
        shared: vec![],
        charge: Charge::Bytes(charge),
    }
}
unsafe fn retained<'ctx>(
    ctx: &'ctx mut TaskCtx,
    run: MetadataRun,
) -> (DemandedGuard<'ctx>, Receipt<Record>, *mut crate::Header) {
    let mut guard = DemandedGuard::new(ctx, run);
    let source = unsafe { heap(guard.context(), 2) };
    unsafe {
        count::hirsel_rt_v2_dup(guard.context(), source);
        count::hirsel_rt_v2_dup(guard.context(), source)
    };
    let leaf = node(run, Origin::Heap, 3, &[]);
    let value = node(
        run,
        Origin::Arena(guard.grant().unwrap()),
        16,
        &[leaf.clone(), leaf],
    );
    accept(unsafe {
        guard.allocate(Update {
            value,
            additions: vec![source, source],
        })
    });
    (guard, node(run, Origin::Inline, 0, &[]), source)
}

#[test]
fn demand_extension_keeps_ordinary_prefix_and_pinned_address_during_wrapper_move() {
    assert_eq!(size_of::<Region>(), 64);
    assert_eq!(size_of::<ReceiptArenaGuard<'_>>(), 80);
    assert_eq!(std::mem::offset_of!(Region, cursor), 8);
    assert_eq!(std::mem::offset_of!(Region, end), 16);
    with_context(|ctx| unsafe {
        let guard = DemandedGuard::new(&mut *ctx, MetadataRun::new());
        let base = guard.base();
        let moved = guard;
        assert_eq!(moved.base(), base);
        assert_eq!((*moved.context()).region, base);
        assert!(!moved.was_sealed());
        assert_eq!(moved.inventory().unwrap().entries().len(), 0);
        assert_eq!(moved.close(0, 0), STATUS_OK);
        assert!((*ctx).region.is_null());
    });
}

#[test]
fn source_runtime_entries_interleave_with_raw_context_journal_and_scope_cleanup() {
    with_context(|ctx| unsafe {
        let run = MetadataRun::new();
        let (guard, metadata, source) = retained(&mut *ctx, run);
        let transient = heap(guard.context(), 2);
        retire(guard.context(), transient);
        let block = region::hirsel_rt_v2_arena_alloc(guard.context(), guard.base(), 2, 8, 8, 0);
        alloc::payload(block).cast::<u64>().write(9);
        assert_eq!(
            guard.inventory().unwrap().seal_charge(&metadata),
            Ok(Charge::Bytes(6))
        );
        assert_eq!((*source).count, 3);
        drop(guard);
        assert_eq!((*source).count, 1);
        assert!((*ctx).region.is_null());
        retire(ctx, source);
    });
}

#[test]
fn guard_growth_and_cow_transfer_keep_and_remove_exact_actual_credit_batches() {
    for cow in [false, true] {
        with_context(|ctx| unsafe {
            let run = MetadataRun::new();
            let mut guard = DemandedGuard::from_raw(ctx, run);
            let source = heap(guard.context(), 2);
            for _ in 0..if cow { 3 } else { 2 } {
                count::hirsel_rt_v2_dup(guard.context(), source);
            }
            let leaf = node(run, Origin::Heap, 3, &[]);
            let grant = guard.grant().unwrap();
            let one = node(run, Origin::Arena(grant), 8, std::slice::from_ref(&leaf));
            let old = accept(guard.allocate(Update {
                value: one,
                additions: vec![source],
            }));
            let two = node(run, Origin::Arena(grant), 16, &[leaf.clone(), leaf]);
            let active = if cow {
                accept(guard.allocate(Update {
                    value: two.clone(),
                    additions: vec![source, source],
                }))
            } else {
                accept(guard.replace(
                    old,
                    &[0],
                    Update {
                        value: two.clone(),
                        additions: vec![source],
                    },
                ));
                old
            };
            assert_eq!((*source).count, if cow { 4 } else { 3 });
            assert_eq!(
                guard.inventory().unwrap().seal_charge(&two),
                Ok(Charge::Bytes(if cow { 9 } else { 6 }))
            );
            let empty = node(run, Origin::Arena(grant), 0, &[]);
            accept(guard.replace(
                active,
                &[],
                Update {
                    value: empty.clone(),
                    additions: vec![],
                },
            ));
            assert_eq!((*source).count, if cow { 2 } else { 1 });
            assert_eq!(
                guard.inventory().unwrap().seal_charge(&empty),
                Ok(Charge::Bytes(if cow { 3 } else { 0 }))
            );
            assert_eq!(guard.close(0, 0), STATUS_OK);
            assert_eq!((*source).count, 1);
            retire(ctx, source);
        });
    }
}

#[test]
fn sealed_guard_refuses_new_events_and_never_dereferences_dropped_owner_marker() {
    with_context(|ctx| unsafe {
        let run = MetadataRun::new();
        let (mut guard, metadata, source) = retained(&mut *ctx, run);
        let mut scalar = 7;
        let owner = guard
            .seal(&metadata, &plan(&mut scalar, Some(source), 6))
            .unwrap();
        assert!(guard.was_sealed());
        assert_eq!((*source).count, 1);
        assert_eq!(guard.grant(), Err(Error::Consumed));
        assert_eq!(guard.bound(0), Err(Error::Consumed));
        let value = node(run, Origin::Inline, 0, &[]);
        let generation = value.generation();
        count::hirsel_rt_v2_dup(guard.context(), source);
        let Err((Error::Consumed, input)) = guard.allocate(Update {
            value,
            additions: vec![source],
        }) else {
            panic!("expected returned rejected custody")
        };
        assert_eq!(input.value.generation(), generation);
        assert_eq!((*source).count, 2);
        retire(guard.context(), input.additions[0]);
        assert_eq!(
            guard.seal(&metadata, &plan(&mut scalar, Some(source), 6)),
            Err(Error::Consumed)
        );
        assert_eq!(root(owner).cast::<u64>().read(), 7);
        drop_owner(guard.context(), owner);
        assert!(guard.was_sealed());
        assert_eq!(guard.grant(), Err(Error::Consumed));
        assert_eq!(guard.close(0, 0), STATUS_OK);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn failed_seal_restores_same_journal_slot_and_original_bounded_failure_identity() {
    with_context(|ctx| unsafe {
        let run = MetadataRun::new();
        let (mut guard, metadata, source) = retained(&mut *ctx, run);
        guard.bound(21).unwrap();
        (*guard.base()).remaining -= 16;
        let slot = (*guard.raw()).inventory;
        let grant = guard.grant().unwrap();
        let bound = (*guard.base()).bound;
        let mut scalar = 7;
        assert_eq!(
            guard.seal(&metadata, &plan(&mut scalar, Some(source), 6)),
            Err(Error::Seal(journal::SealError::Transaction(
                super::super::SealError::Exhausted
            )))
        );
        assert_eq!((*guard.raw()).inventory, slot);
        assert_eq!(guard.grant(), Ok(grant));
        assert!(!guard.was_sealed());
        assert_eq!((*guard.base()).bound, bound);
        assert_eq!((*guard.base()).remaining, 5);
        assert_eq!((*source).count, 3);
        assert_eq!(
            region::hirsel_rt_v2_arena_exhausted(guard.context(), guard.base(), 1, 1, 17, 0),
            crate::STATUS_FAILURE
        );
        assert!(guard.recover());
        assert_eq!((*guard.context()).failure.count(), 0);
        assert_eq!(guard.close(0, 0), STATUS_OK);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn outer_seal_preserves_active_inner_guard_then_owner_survives_both_scope_exits() {
    with_context(|ctx| unsafe {
        let run = MetadataRun::new();
        let (mut outer, metadata, source) = retained(&mut *ctx, run);
        let mut inner = DemandedGuard::from_raw(outer.context(), run);
        let block = region::hirsel_rt_v2_arena_alloc(inner.context(), inner.base(), 2, 8, 8, 0);
        alloc::payload(block).cast::<u64>().write(9);
        let event = node(run, Origin::Arena(inner.grant().unwrap()), 8, &[]);
        accept(inner.allocate(Update {
            value: event,
            additions: vec![],
        }));
        let mut scalar = 7;
        let owner = outer
            .seal(&metadata, &plan(&mut scalar, Some(source), 6))
            .unwrap();
        assert_eq!((*inner.context()).region, inner.base());
        assert_eq!(alloc::payload(block).cast::<u64>().read(), 9);
        assert_eq!(inner.close(0, 0), STATUS_OK);
        assert_eq!((*outer.context()).region, outer.base());
        assert_eq!(outer.close(0, 0), STATUS_OK);
        assert!((*ctx).region.is_null());
        assert_eq!(root(owner).cast::<u64>().read(), 7);
        drop_owner(ctx, owner);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn consumed_outer_guard_cannot_recover_an_inner_arenas_failure() {
    with_context(|ctx| unsafe {
        let run = MetadataRun::new();
        let mut outer = DemandedGuard::from_raw(ctx, run);
        outer.bound(0).unwrap();
        let bound = (*outer.base()).bound;
        let metadata = node(run, Origin::Inline, 0, &[]);
        let mut scalar = 7;
        let owner = outer.seal(&metadata, &plan(&mut scalar, None, 0)).unwrap();
        drop_owner(outer.context(), owner);
        let mut inner = DemandedGuard::from_raw(outer.context(), run);
        inner.bound(0).unwrap();
        region::hirsel_rt_v2_arena_exhausted(inner.context(), inner.base(), 1, 1, 19, 0);
        assert!(!outer.recover());
        assert_eq!((*outer.base()).bound, bound);
        assert_eq!((*outer.context()).failure.count(), 1);
        assert!(inner.recover());
        assert_eq!((*outer.context()).failure.count(), 0);
        assert_eq!(inner.close(0, 0), STATUS_OK);
        assert_eq!(outer.close(0, 0), STATUS_OK);
    });
}

#[test]
fn shared_source_batches_transfer_once_and_outlive_demanded_guard_cleanup() {
    with_context(|ctx| unsafe {
        let run = MetadataRun::new();
        let mut guard = DemandedGuard::from_raw(ctx, run);
        let source = heap(guard.context(), 3);
        roots::hirsel_rt_v2_root_dup(guard.context(), source);
        roots::hirsel_rt_v2_root_dup(guard.context(), source);
        let shared = node(run, Origin::Shared, 0, &[]);
        let value = node(
            run,
            Origin::Arena(guard.grant().unwrap()),
            16,
            &[shared.clone(), shared],
        );
        accept(guard.allocate(Update {
            value,
            additions: vec![source, source],
        }));
        let metadata = node(run, Origin::Inline, 0, &[]);
        let mut scalar = 7;
        let mut typed = plan(&mut scalar, None, 0);
        typed.shared = vec![source, source];
        let owner = guard.seal(&metadata, &typed).unwrap();
        assert_eq!(roots::count(&*guard.context(), source), 3);
        assert_eq!(guard.close(0, 0), STATUS_OK);
        assert_eq!(roots::count(&*ctx, source), 3);
        drop_owner(ctx, owner);
        assert_eq!(roots::count(&*ctx, source), 1);
        roots::hirsel_rt_v2_root_release(ctx, source);
    });
}

#[test]
fn nested_guards_reject_foreign_semantic_grants_without_taking_caller_credits() {
    with_context(|ctx| unsafe {
        let run = MetadataRun::new();
        let mut outer = DemandedGuard::from_raw(ctx, run);
        let mut inner = DemandedGuard::from_raw(outer.context(), run);
        assert_ne!(outer.grant().unwrap(), inner.grant().unwrap());
        let source = heap(inner.context(), 2);
        count::hirsel_rt_v2_dup(inner.context(), source);
        let leaf = node(run, Origin::Heap, 3, &[]);
        let value = node(run, Origin::Arena(outer.grant().unwrap()), 8, &[leaf]);
        let generation = value.generation();
        let Err((
            Error::Journal(journal::Error::Semantic(semantic::Error::WrongAllocationOrigin)),
            input,
        )) = inner.allocate(Update {
            value,
            additions: vec![source],
        })
        else {
            panic!("expected foreign grant refusal")
        };
        assert_eq!(input.value.generation(), generation);
        assert_eq!((*source).count, 2);
        assert_eq!(inner.inventory().unwrap().entries().len(), 0);
        assert_eq!(inner.close(0, 0), STATUS_OK);
        accept(outer.allocate(input));
        assert_eq!(outer.close(0, 0), STATUS_OK);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}
