//! Tests of prepared owner custody only. They supply already closed graphs;
//! no public seal, native relocation, inventory preflight or H claim is made.
use super::*;
use crate::context::{
    CONFIG_DIAGNOSTIC, Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_ctx_stats, hirsel_rt_v2_worker_collect, hirsel_rt_v2_worker_create,
    hirsel_rt_v2_worker_retire,
};
use crate::region::{
    hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit, hirsel_rt_v2_region_reserve,
};
use crate::{LAYOUT_FROZEN, LAYOUT_MAGIC, LAYOUT_VERSION};
use std::mem::MaybeUninit;

fn with_context(test: impl FnOnce(*mut TaskCtx)) {
    let layout = crate::Layout {
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
        flags: LAYOUT_FROZEN,
    };
    let layouts = [&layout as *const crate::Layout];
    let config = Config {
        flags: CONFIG_DIAGNOSTIC,
        layouts: layouts.as_ptr(),
        layout_count: 1,
        ..Config::new()
    };
    let mut ctx = ptr::null_mut();
    assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
    test(ctx);
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}
unsafe fn stats(ctx: *mut TaskCtx) -> Stats {
    let mut stats = Stats::default();
    unsafe { hirsel_rt_v2_ctx_stats(ctx, &mut stats) };
    stats
}
unsafe fn enter(ctx: *mut TaskCtx) -> Box<Region> {
    let mut record = Box::new(MaybeUninit::<Region>::uninit());
    unsafe {
        hirsel_rt_v2_region_enter(ctx, record.as_mut_ptr());
        Box::from_raw(Box::into_raw(record).cast())
    }
}
unsafe fn shared(ctx: *mut TaskCtx) -> *mut Header {
    unsafe {
        let block = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::DOMAIN_HEAP, 0);
        crate::alloc::payload(block).cast::<u64>().write(0);
        block
    }
}

#[test]
fn prepared_owner_33_zero_root_and_aligned_snapshot_survive_source_guard() {
    with_context(|ctx| unsafe {
        for size in [0, 8] {
            let mut arena = enter(ctx);
            let mut value = 71u64;
            let source = if size == 0 {
                ptr::null()
            } else {
                (&raw const value).cast()
            };
            let prepared = PreparedSeal::new(&mut *ctx, source, size, 64);
            let owner = prepared.commit(&mut *arena);
            assert_eq!((*owner).state, State::Published);
            assert_ne!((*owner).generation, 0);
            value = 99;
            assert_eq!(value, 99);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
            drop(arena);
            assert_eq!(root(owner).addr() % 64, 0);
            if size != 0 {
                assert_eq!(root(owner).cast::<u64>().read(), 71);
            }
            drop_owner(ctx, owner);
        }
        assert_eq!(stats(ctx).active_regions, 0);
    });
}

#[test]
fn prepared_owner_33_rollback_releases_only_staged_shared_acquisitions() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let root = shared(ctx);
        {
            let mut prepared = PreparedSeal::new(&mut *ctx, (&raw const root).cast(), 8, 8);
            prepared.retain_shared(root);
            prepared.retain_shared(root);
            assert_eq!(roots::count(prepared.ctx, root), 3);
        }
        assert_eq!(roots::count(&*ctx, root), 1);
        assert_eq!(stats(ctx).active_regions, 1);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        roots::hirsel_rt_v2_root_release(ctx, root);
        assert_eq!(stats(ctx).live_blocks, 0);
    });
}

#[test]
fn prepared_owner_33_commit_preserves_repeated_shared_credits_and_segment_custody() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let mut data = ptr::null_mut();
        assert_eq!(
            hirsel_rt_v2_region_reserve(ctx, &mut *arena, 32, 8, &mut data),
            0
        );
        data.cast::<u64>().write(83);
        let shared = shared(ctx);
        let source = [shared, shared, data.cast::<Header>()];
        let mut prepared = PreparedSeal::new(&mut *ctx, source.as_ptr().cast(), 24, 8);
        prepared.retain_shared(shared);
        prepared.retain_shared(shared);
        let owner = prepared.commit(&mut *arena);
        roots::hirsel_rt_v2_root_release(ctx, shared);
        assert_eq!(roots::count(&*ctx, shared), 2);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        drop(arena);
        let bytes = root(owner).cast::<*mut Header>();
        assert_eq!(bytes.read(), shared);
        assert_eq!(bytes.add(1).read(), shared);
        assert_eq!(bytes.add(2).read().cast::<u64>().read(), 83);
        drop_owner(ctx, owner);
        assert_eq!(stats(ctx).live_blocks, 0);
        assert_eq!(stats(ctx).region_active_bytes, 0);
    });
}

#[test]
fn prepared_owner_33_receiving_worker_drops_after_source_worker_retirement() {
    with_context(|run| unsafe {
        let source = hirsel_rt_v2_worker_create(run);
        let receiver = hirsel_rt_v2_worker_create(run);
        let mut arena = enter(source);
        let shared = shared(source);
        let mut prepared = PreparedSeal::new(&mut *source, (&raw const shared).cast(), 8, 8);
        prepared.retain_shared(shared);
        let owner = prepared.commit(&mut *arena);
        roots::hirsel_rt_v2_root_release(source, shared);
        assert_eq!(hirsel_rt_v2_region_exit(source, &mut *arena, 0, 0), 0);
        drop(arena);
        hirsel_rt_v2_worker_retire(source);
        let owner = owner.addr();
        let receiver = receiver.addr();
        std::thread::spawn(move || {
            let owner = owner as *mut SealedRegion;
            let receiver = receiver as *mut TaskCtx;
            let shared = root(owner).cast::<*mut Header>().read();
            assert_eq!(roots::count(&*receiver, shared), 1);
            drop_owner(receiver, owner);
            hirsel_rt_v2_worker_retire(receiver);
        })
        .join()
        .unwrap();
        hirsel_rt_v2_worker_collect(run);
        assert!(
            (*run)
                .run_ref()
                .heaps
                .heaps()
                .all(|heap| (*heap).live().0 == 0)
        );
    });
}
