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

#[test]
fn sealed_bits_plan_33_accepts_real_compiler_value_only_descriptors() {
    use hirsel_lir::layout_types::*;
    for (kind, size, align) in [
        (LayoutKind::Unit, 0, 1),
        (LayoutKind::Scalar(ScalarKind::U64), 8, 8),
        (LayoutKind::Scalar(ScalarKind::Bool), 1, 1),
        (LayoutKind::Scalar(ScalarKind::U128), 16, 16),
    ] {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut table = LayoutTable::new(&target);
        table.layouts.push(LayoutRecord {
            key: LayoutIdentity {
                owner_revision: None,
                instance: None,
                target: target.digest(),
                layout_version: hirsel_lir::LAYOUT_VERSION,
            },
            kind,
            size: SizeSpec::Fixed(size),
            align,
            fields: vec![],
            tag: None,
            initialized: InitializedBytes::Fields,
            scan: vec![],
            drop: vec![],
            copy: CopyRecipe::Bits,
            finalizer: None,
            storage: vec![StorageKind::Heap],
            abi: AbiInputs {
                scalars: vec![],
                unaligned: false,
            },
        });
        let descriptor = hirsel_lir::rt_program::descriptors(&table, &[LayoutId(0)])
            .unwrap()
            .remove(0);
        let words = descriptor.scan.linked(|_| None).unwrap();
        assert_eq!(words.len(), 8);
        assert_eq!(descriptor.flags, crate::LAYOUT_NO_EXTENT);
        assert!(descriptor.steps.is_empty() && descriptor.logical_count.is_none());
        let physical = Layout {
            flags: descriptor.flags,
            logical: descriptor.logical,
            scan: words.as_ptr(),
            scan_words: words.len() as u64,
            ..layout(size, u64::from(align))
        };
        context(&[physical], true, |ctx| unsafe {
            let run = MetadataRun::new();
            let mut guard = DemandedGuard::from_raw(ctx, run);
            guard.bound(0).unwrap();
            let value = 1u128;
            let bytes = &value.to_ne_bytes()[..size as usize];
            let metadata = image(run, bytes, align as usize);
            let owner = guard
                .seal_bits(2, ptr::from_ref(&value).cast_mut().cast(), &metadata)
                .expect("the real compiler's value metadata has no owned graph edge");
            assert_eq!(guard.close(0, 0), STATUS_OK);
            drop_owner(ctx, owner);
        });
    }
}

#[test]
fn sealed_bits_plan_33_value_metadata_cannot_hide_holders_or_unknown_structure() {
    use hirsel_core_contract::handle_scan::*;
    let good = [VALUE, VALUE_BITS, 8, 0, 0, 0, 0, 0];
    let descriptor = Layout {
        flags: crate::LAYOUT_NO_EXTENT,
        scan: good.as_ptr(),
        scan_words: 8,
        ..layout(8, 8)
    };
    assert!(unsafe { pointer_free_descriptor(&descriptor) });
    for (operand, value) in [
        (0, HANDLE),
        (0, TYPE_EDGE),
        (0, UNKNOWN),
        (1, VALUE_UNKNOWN),
        (1, VALUE_HANDLE),
        (1, VALUE_FIELDS),
        (1, VALUE_BOOL),
        (2, 16),
        (3, 1),
        (4, 1),
        (5, 1),
        (6, 1),
        (7, 1),
    ] {
        let mut words = good;
        words[operand] = value;
        assert!(
            !unsafe {
                pointer_free_descriptor(&Layout {
                    scan: words.as_ptr(),
                    ..descriptor
                })
            },
            "{operand}:{value}"
        );
    }
    for count in [1, 7, 9, u64::MAX] {
        assert!(!unsafe {
            pointer_free_descriptor(&Layout {
                scan_words: count,
                ..descriptor
            })
        });
    }
    for bit in [
        1,
        2,
        4,
        16,
        32,
        64,
        128,
        256,
        512,
        1024,
        2048,
        4096,
        u32::MAX,
    ] {
        assert!(
            !unsafe {
                pointer_free_descriptor(&Layout {
                    flags: bit,
                    ..descriptor
                })
            },
            "flags{bit}"
        );
    }
    assert!(!unsafe {
        pointer_free_descriptor(&Layout {
            scan: ptr::null(),
            ..descriptor
        })
    });
    assert!(!unsafe {
        pointer_free_descriptor(&Layout {
            step_count: 1,
            ..descriptor
        })
    });
    assert!(!unsafe {
        pointer_free_descriptor(&Layout {
            logical_count: good.as_ptr(),
            ..descriptor
        })
    });
    assert!(
        unsafe { pointer_free_descriptor(&layout(16, 16)) },
        "legacy certified images stay valid"
    );
}
