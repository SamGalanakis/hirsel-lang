//! The actual descriptor walker consumes native sealed custody, independently
//! of compiler construction and H. Skipped fields retain their original owner.
use super::*;
use crate::release::*;

fn step(offset: u64) -> Step {
    Step {
        kind: STEP_RELEASE_SEALED,
        field: WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset,
        stride: 0,
        count: 0,
        count_offset: NO_OFFSET,
        layout: ptr::null(),
    }
}
fn with_program(steps: &[Step], size: u64, fields: u32, test: impl FnOnce(*mut TaskCtx)) {
    let frozen = crate::Layout {
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
    let program = crate::Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align: 8,
        finalizer: None,
        steps: steps.as_ptr(),
        step_count: steps.len() as u64,
        field_count: fields,
        logical: LOGICAL_UNSUPPORTED,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: u64::MAX,
        flags: if steps.iter().any(|s| s.kind == STEP_LOOP) {
            LAYOUT_WIDE_CURSOR
        } else {
            0
        },
    };
    let layouts = [&raw const frozen, &raw const program];
    let config = Config {
        flags: CONFIG_DIAGNOSTIC,
        layouts: layouts.as_ptr(),
        layout_count: 2,
        ..Config::new()
    };
    let mut ctx = ptr::null_mut();
    // SAFETY: all descriptor storage remains live until the run is destroyed.
    assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
    test(ctx);
    // SAFETY: the test retires every live owner and region before destruction.
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}
unsafe fn owner(ctx: *mut TaskCtx, credit: Option<*mut Header>) -> *mut std::ffi::c_void {
    // SAFETY: the test has exclusive access to this run and the created region.
    unsafe {
        let mut arena = enter(ctx);
        let mut data = ptr::null_mut();
        assert_eq!(
            hirsel_rt_v2_region_reserve(ctx, &mut *arena, 32, 8, &mut data),
            0
        );
        data.cast::<u64>().write(83);
        let mut preparation = PreparedSeal::new(&mut *ctx, data, 8, 8);
        if let Some(credit) = credit {
            preparation.retain_shared(credit);
        }
        let raw = preparation.commit(&mut *arena);
        let raw = into_native_owner(ctx, raw);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        raw
    }
}
unsafe fn finalize(ctx: *mut TaskCtx, fields: &mut [*mut std::ffi::c_void], mask: *const u64) {
    let mut continuation = MaybeUninit::<crate::Cleanup>::uninit();
    // SAFETY: fields contain exactly the ownership selected by the program/mask.
    assert_eq!(
        unsafe {
            crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                3,
                fields.as_mut_ptr().cast(),
                mask,
                continuation.as_mut_ptr(),
                0,
                0,
            )
        },
        0
    );
}
#[test]
fn sealed_runtime_drop_33_mask_consumes_only_selected_owner_after_source_exit() {
    let steps = [
        Step {
            field: 0,
            ..step(0)
        },
        Step {
            field: 1,
            ..step(8)
        },
    ];
    with_program(&steps, 16, 2, |ctx| unsafe {
        let mut fields = [owner(ctx, None), owner(ctx, None)];
        let mask = 1;
        finalize(ctx, &mut fields, &mask);
        assert_eq!(
            hirsel_rt_v2_sealed_root(ctx, fields[1])
                .cast::<u64>()
                .read(),
            83
        );
        hirsel_rt_v2_sealed_release(ctx, fields[1]);
        assert_eq!(stats(ctx).live_blocks, 0);
        assert_eq!(stats(ctx).active_regions, 0);
    });
}
#[test]
fn sealed_runtime_drop_33_loop_releases_storage_and_repeated_shared_credits() {
    let steps = [
        Step {
            kind: STEP_LOOP,
            span: 1,
            stride: 8,
            count: 2,
            ..step(0)
        },
        step(0),
    ];
    with_program(&steps, 16, 0, |ctx| unsafe {
        let shared = shared(ctx);
        let mut fields = [owner(ctx, Some(shared)), owner(ctx, Some(shared))];
        roots::hirsel_rt_v2_root_release(ctx, shared);
        assert_eq!(roots::count(&*ctx, shared), 2);
        finalize(ctx, &mut fields, ptr::null());
        assert_eq!(stats(ctx).live_blocks, 0);
        assert_eq!(stats(ctx).active_regions, 0);
    });
}
#[test]
fn sealed_runtime_drop_33_duplicate_forged_and_control_overlap_refuse() {
    const CASE: &str = "HIRSEL_SEALED_DROP_BAD_CASE";
    if let Ok(case) = std::env::var(CASE) {
        let steps = if case == "control-overlap" {
            vec![
                Step {
                    kind: STEP_LIVE_BIT,
                    span: 1,
                    variant: 0,
                    ..step(0)
                },
                step(0),
            ]
        } else {
            vec![step(0), step(8)]
        };
        with_program(&steps, 16, 0, |ctx| unsafe {
            let first = owner(ctx, None);
            let mut fields = [
                first,
                if case == "forged" {
                    std::ptr::without_provenance_mut(usize::MAX)
                } else {
                    first
                },
            ];
            finalize(ctx, &mut fields, ptr::null());
        });
        panic!("invalid sealed drop unexpectedly returned");
    }
    for case in ["duplicate", "forged", "control-overlap"] {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sealed_region::tests::drop_tests::sealed_runtime_drop_33_duplicate_forged_and_control_overlap_refuse", "--nocapture"])
            .env(CASE, case).output().unwrap();
        assert!(!child.status.success(), "{case} was accepted");
        let stderr = String::from_utf8_lossy(&child.stderr);
        let reason = if case == "control-overlap" {
            "drop control word aliases a consumed reference slot"
        } else {
            "foreign or dead sealed owner"
        };
        assert!(stderr.contains(reason), "{case}: {stderr}");
    }
}
