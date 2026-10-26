use super::*;
use crate::context::{
    CONFIG_DIAGNOSTIC, Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_ctx_stats,
};
use crate::release::{LAYOUT_MAGIC, LAYOUT_VERSION, Layout};
use crate::sealed_region::receipt::semantic;
use crate::sealed_region::{
    drop_owner, hirsel_rt_v2_sealed_release, hirsel_rt_v2_sealed_root, into_native_owner, root,
};

fn layout(size: u64, align: u64) -> Layout {
    Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align,
        finalizer: None,
        steps: ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: 0,
    }
}

fn context(layouts: &[Layout], diagnostic: bool, test: impl FnOnce(*mut TaskCtx)) {
    let descriptors: Vec<_> = layouts.iter().map(ptr::from_ref).collect();
    let config = Config {
        flags: if diagnostic { CONFIG_DIAGNOSTIC } else { 0 },
        layouts: descriptors.as_ptr(),
        layout_count: descriptors.len() as u32,
        ..Config::new()
    };
    let mut ctx = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) },
        STATUS_OK
    );
    test(ctx);
    let mut stats = Stats::default();
    unsafe { hirsel_rt_v2_ctx_stats(ctx, &mut stats) };
    assert_eq!((stats.live_blocks, stats.active_regions), (0, 0));
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}

fn image(run: MetadataRun, bytes: &[u8], align: usize) -> Receipt<Record> {
    semantic::snapshot(run, Origin::Inline, Charge::Bytes(0), bytes, align, &[]).unwrap()
}

#[test]
fn sealed_bits_plan_33_builds_unit_scalar_bool_and_aligned_aggregate_without_supplied_plan() {
    let layouts = [layout(0, 1), layout(8, 8), layout(1, 1), layout(16, 16)];
    for diagnostic in [false, true] {
        for (index, bytes) in [
            (2, vec![]),
            (3, 73u64.to_ne_bytes().to_vec()),
            (4, vec![1]),
            (5, 97u128.to_ne_bytes().to_vec()),
        ] {
            context(&layouts, diagnostic, |ctx| unsafe {
                let run = MetadataRun::new();
                let mut guard = DemandedGuard::from_raw(ctx, run);
                guard.bound(0).unwrap();
                let align = layouts[index as usize - 2].align as usize;
                let metadata = image(run, &bytes, align);
                let mut source = 0u128;
                let value = if bytes.is_empty() {
                    ptr::null_mut()
                } else {
                    ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        ptr::from_mut(&mut source).cast(),
                        bytes.len(),
                    );
                    ptr::from_mut(&mut source).cast()
                };
                let bound = (*guard.base()).bound;
                let owner = guard.seal_bits(index, value, &metadata).unwrap();
                assert!(guard.was_sealed());
                assert_eq!((*guard.base()).remaining, 0);
                assert_eq!((*guard.base()).bound, bound);
                assert_eq!(guard.grant(), Err(Error::Consumed));
                assert_eq!(guard.close(0, 0), STATUS_OK);
                assert!((*ctx).region.is_null());
                // The original stack payload can disappear after transfer.
                source = 0;
                std::hint::black_box(source);
                let handle = into_native_owner(ctx, owner);
                for _ in 0..2 {
                    let borrowed = hirsel_rt_v2_sealed_root(ctx, handle).cast::<u8>();
                    assert_eq!(borrowed.addr() % align, 0);
                    assert_eq!(std::slice::from_raw_parts(borrowed, bytes.len()), bytes);
                }
                hirsel_rt_v2_sealed_release(ctx, handle);
            });
        }
    }
}

#[test]
fn sealed_bits_plan_33_rejects_retained_zero_cost_events_and_unlisted_physical_storage() {
    for retained in [false, true] {
        context(&[layout(8, 8)], true, |ctx| unsafe {
            let run = MetadataRun::new();
            let mut guard = DemandedGuard::from_raw(ctx, run);
            guard.bound(0).unwrap();
            let grant = guard.grant().unwrap();
            if retained {
                let value =
                    semantic::snapshot(run, Origin::Arena(grant), Charge::Bytes(0), &[], 1, &[])
                        .unwrap();
                assert!(
                    guard
                        .allocate(Update {
                            value,
                            additions: vec![]
                        })
                        .is_ok()
                );
            } else {
                crate::region::hirsel_rt_v2_arena_alloc(ctx, guard.base(), 2, 8, 8, 0);
            }
            let mut value = 7u64;
            let metadata = image(run, &value.to_ne_bytes(), 8);
            assert_eq!(
                guard.seal_bits(2, ptr::from_mut(&mut value).cast(), &metadata),
                Err(Error::InventoryRequired)
            );
            assert!(!guard.was_sealed());
            assert_eq!(guard.grant(), Ok(grant));
            assert_eq!((*guard.base()).remaining, 0);
            assert_eq!(guard.close(0, 0), STATUS_OK);
        });
    }
}

#[test]
fn sealed_bits_plan_33_refuses_mismatched_images_and_uncertified_layout_before_transfer() {
    for mutation in 0..8 {
        let mut rejected = layout(8, 8);
        if mutation == 7 {
            rejected.scan_words = u64::MAX;
        }
        context(&[layout(8, 8), rejected], true, |ctx| unsafe {
            let run = MetadataRun::new();
            let mut guard = DemandedGuard::from_raw(ctx, run);
            guard.bound(0).unwrap();
            let grant = guard.grant().unwrap();
            let mut value = 7u64;
            let good = image(run, &value.to_ne_bytes(), 8);
            let bad = match mutation {
                0 => run.node(*good.record(), &[]).unwrap(),
                1 => image(run, &[], 8),
                2 => image(run, &8u64.to_ne_bytes(), 8),
                3 => image(run, &value.to_ne_bytes(), 1),
                4 => semantic::snapshot(
                    run,
                    Origin::Heap,
                    Charge::Bytes(0),
                    &value.to_ne_bytes(),
                    8,
                    &[],
                )
                .unwrap(),
                5 => semantic::snapshot(
                    run,
                    Origin::Inline,
                    Charge::Bytes(0),
                    &value.to_ne_bytes(),
                    8,
                    std::slice::from_ref(&good),
                )
                .unwrap(),
                6 => image(MetadataRun::new(), &value.to_ne_bytes(), 8),
                7 => good.clone(),
                _ => unreachable!(),
            };
            let refusal = guard.seal_bits(3, ptr::from_mut(&mut value).cast(), &bad);
            if mutation == 6 {
                assert!(matches!(
                    refusal,
                    Err(Error::Seal(journal::SealError::Semantic(_)))
                ));
            } else {
                assert_eq!(
                    refusal,
                    Err(if mutation == 7 {
                        Error::RootLayout
                    } else {
                        Error::RootImage
                    })
                );
            }
            assert_eq!(guard.grant(), Ok(grant));
            assert!(!guard.was_sealed());
            assert_eq!((*guard.base()).remaining, 0);
            let owner = guard
                .seal_bits(2, ptr::from_mut(&mut value).cast(), &good)
                .unwrap();
            assert_eq!(root(owner).cast::<u64>().read(), 7);
            drop_owner(ctx, owner);
            assert_eq!(guard.close(0, 0), STATUS_OK);
        });
    }
}
