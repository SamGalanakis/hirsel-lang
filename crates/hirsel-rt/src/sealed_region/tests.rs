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

#[test]
fn native_sealed_access_33_reads_stable_bit_roots_and_releases_after_scope_exit() {
    with_context(|ctx| unsafe {
        for size in [0, 8] {
            let mut arena = enter(ctx);
            crate::region::hirsel_rt_v2_arena_bound(ctx, &mut *arena, 0);
            let mut value = 73u64;
            let source = if size == 0 {
                ptr::null()
            } else {
                (&raw const value).cast()
            };
            let owner = PreparedSeal::new(&mut *ctx, source, size, 64).commit(&mut *arena);
            let owner = into_native_owner(ctx, owner);
            assert_eq!(
                arena.remaining, 0,
                "physical metadata adds no logical charge"
            );
            value = 99;
            assert_eq!(value, 99);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
            drop(arena);
            let first = hirsel_rt_v2_sealed_root(ctx, owner);
            let second = hirsel_rt_v2_sealed_root(ctx, owner);
            assert_eq!(first, second);
            assert_eq!(first.addr() % 64, 0);
            if size != 0 {
                assert_eq!(first.cast::<u64>().read(), 73);
            }
            hirsel_rt_v2_sealed_release(ctx, owner);
        }
        assert_eq!(stats(ctx).active_regions, 0);
        assert_eq!(stats(ctx).live_blocks, 0);
    });
}

#[test]
fn native_sealed_access_33_releases_repeated_shared_credits_and_adopted_storage() {
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
        let mut preparation = PreparedSeal::new(&mut *ctx, source.as_ptr().cast(), 24, 8);
        preparation.retain_shared(shared);
        preparation.retain_shared(shared);
        let owner = preparation.commit(&mut *arena);
        let owner = into_native_owner(ctx, owner);
        roots::hirsel_rt_v2_root_release(ctx, shared);
        assert_eq!(roots::count(&*ctx, shared), 2);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        drop(arena);
        let root = hirsel_rt_v2_sealed_root(ctx, owner).cast::<*mut Header>();
        assert_eq!(root.read(), shared);
        assert_eq!(root.add(1).read(), shared);
        assert_eq!(root.add(2).read().cast::<u64>().read(), 83);
        hirsel_rt_v2_sealed_release(ctx, owner);
        assert_eq!(stats(ctx).live_blocks, 0);
        assert_eq!(stats(ctx).active_regions, 0);
    });
}

#[test]
fn native_sealed_access_33_bad_foreign_dead_and_double_release_refuse_before_dereference() {
    const CASE: &str = "HIRSEL_SEALED_ACCESS_BAD_CASE";
    if let Ok(case) = std::env::var(CASE) {
        with_context(|ctx| unsafe {
            let mut arena = enter(ctx);
            let owner = PreparedSeal::new(&mut *ctx, ptr::null(), 0, 1).commit(&mut *arena);
            let owner = into_native_owner(ctx, owner);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
            match case.as_str() {
                "dead-read" => {
                    hirsel_rt_v2_sealed_release(ctx, owner);
                    hirsel_rt_v2_sealed_root(ctx, owner);
                }
                "stale-after-new-read" | "stale-after-new-release" => {
                    hirsel_rt_v2_sealed_release(ctx, owner);
                    let mut next_arena = enter(ctx);
                    let replacement =
                        PreparedSeal::new(&mut *ctx, ptr::null(), 0, 1).commit(&mut *next_arena);
                    let replacement = into_native_owner(ctx, replacement);
                    assert_ne!(owner, replacement);
                    hirsel_rt_v2_sealed_root(ctx, replacement);
                    if case == "stale-after-new-read" {
                        hirsel_rt_v2_sealed_root(ctx, owner);
                    } else {
                        hirsel_rt_v2_sealed_release(ctx, owner);
                    }
                }
                "double-release" => {
                    hirsel_rt_v2_sealed_release(ctx, owner);
                    hirsel_rt_v2_sealed_release(ctx, owner);
                }
                "foreign-read" => with_context(|foreign| {
                    hirsel_rt_v2_sealed_root(foreign, owner);
                }),
                "foreign-release" => {
                    with_context(|foreign| hirsel_rt_v2_sealed_release(foreign, owner))
                }
                "forged-read" => {
                    hirsel_rt_v2_sealed_root(
                        ctx,
                        std::ptr::without_provenance::<std::ffi::c_void>(usize::MAX),
                    );
                }
                _ => panic!("unknown bad case"),
            }
        });
        panic!("invalid sealed access unexpectedly returned");
    }
    for case in [
        "dead-read",
        "double-release",
        "foreign-read",
        "foreign-release",
        "forged-read",
        "stale-after-new-read",
        "stale-after-new-release",
    ] {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sealed_region::tests::native_sealed_access_33_bad_foreign_dead_and_double_release_refuse_before_dereference", "--nocapture"])
            .env(CASE,case).output().unwrap();
        assert!(!child.status.success(), "{case} was accepted");
        let stderr = String::from_utf8_lossy(&child.stderr);
        assert!(
            stderr.contains("foreign or dead sealed owner"),
            "{case}: {stderr}"
        );
    }
}

/// Deterministically reuse one control allocation, instead of relying on the
/// host allocator to return a recently freed address. The old empty owner's
/// registry identity and storage are retired; its Box allocation remains held
/// solely as test scratch for the next independently prepared owner.
unsafe fn replace_control_at_same_address(
    ctx: *mut TaskCtx,
) -> (*mut std::ffi::c_void, *mut std::ffi::c_void) {
    unsafe {
        let mut arena = enter(ctx);
        let first = PreparedSeal::new(&mut *ctx, ptr::null(), 0, 1).commit(&mut *arena);
        let old = into_native_owner(ctx, first);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        let (address, generation) = (*ctx)
            .run_ref()
            .sealed_owners
            .lock()
            .unwrap()
            .take_native(old.addr());
        assert_eq!(address, first.addr());
        let mut scratch = Box::from_raw(first);
        assert_eq!(scratch.generation, generation);
        assert!(scratch.shared.is_empty() && scratch.extensions.is_none());
        scratch.storage.take().unwrap().release(ctx);
        scratch.state = State::Dead;
        let mut next_arena = enter(ctx);
        let replacement = PreparedSeal::new(&mut *ctx, ptr::null(), 0, 1).commit(&mut *next_arena);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *next_arena, 0, 0), 0);
        let next_generation = (*ctx)
            .run_ref()
            .sealed_owners
            .lock()
            .unwrap()
            .take_control(replacement.addr());
        // Moving out of this Box frees only the second allocation; assigning
        // drops the dead first root image and reuses exactly its held storage.
        *scratch = *Box::from_raw(replacement);
        let reused = Box::into_raw(scratch);
        assert_eq!(
            reused, first,
            "the test reuses the exact physical control address"
        );
        (*ctx)
            .run_ref()
            .sealed_owners
            .lock()
            .unwrap()
            .register(reused.expose_provenance(), next_generation);
        let fresh = into_native_owner(ctx, reused);
        assert_ne!(old, fresh);
        (old, fresh)
    }
}

#[test]
fn native_sealed_access_33_real_c_same_address_stale_read_and_release_bad() {
    const CASE: &str = "HIRSEL_SEALED_SAME_ADDRESS_BAD_CASE";
    if let Ok(case) = std::env::var(CASE) {
        with_context(|ctx| unsafe {
            let (old, fresh) = replace_control_at_same_address(ctx);
            assert!(
                !hirsel_rt_v2_sealed_root(ctx, fresh).is_null(),
                "replacement is live"
            );
            match case.as_str() {
                "read" => {
                    hirsel_rt_v2_sealed_root(ctx, old);
                }
                "release" => hirsel_rt_v2_sealed_release(ctx, old),
                _ => unreachable!(),
            }
        });
        panic!("stale identity reached the replacement control");
    }
    for case in ["read", "release"] {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","sealed_region::tests::native_sealed_access_33_real_c_same_address_stale_read_and_release_bad","--nocapture"])
            .env(CASE,case).output().unwrap();
        assert!(!child.status.success());
        let stderr = String::from_utf8_lossy(&child.stderr);
        assert!(
            stderr.contains("foreign or dead sealed owner"),
            "{case}: {stderr}"
        );
    }
    with_context(|ctx| unsafe {
        let (_, fresh) = replace_control_at_same_address(ctx);
        hirsel_rt_v2_sealed_root(ctx, fresh);
        hirsel_rt_v2_sealed_release(ctx, fresh);
        assert_eq!(stats(ctx).active_regions, 0);
        assert_eq!(stats(ctx).live_blocks, 0);
    });
}
