use super::*;
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::LayoutId;
use hirsel_lir::layout_types::{LayoutKind, LayoutTable, TargetSpec};
use hirsel_types::{Primitive, testing::Types};

pub(in crate::sealed_region) fn native(table: &LayoutTable, test: impl FnOnce(*mut TaskCtx)) {
    let desc: Vec<_> = (0..table.layouts.len())
        .map(|i| hirsel_lir::rt_program::descriptor(table, LayoutId(i as u32)).unwrap())
        .collect();
    let scans: Vec<_> = desc
        .iter()
        .map(|d| d.scan.linked(|id| Some(id.0 + 2)).unwrap())
        .collect();
    let steps: Vec<Vec<_>> = desc
        .iter()
        .map(|d| {
            d.steps
                .iter()
                .map(|s| crate::Step {
                    kind: s.kind.code(),
                    field: s.field,
                    variant: s.variant,
                    span: s.span,
                    offset: s.offset,
                    stride: s.stride,
                    count: s.count,
                    count_offset: s.count_offset,
                    layout: ptr::null(),
                })
                .collect()
        })
        .collect();
    let layouts: Vec<_> = desc
        .iter()
        .enumerate()
        .map(|(i, d)| Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: d.size,
            align: d.align,
            finalizer: None,
            steps: steps[i].as_ptr(),
            step_count: steps[i].len() as u64,
            field_count: d.field_count,
            flags: d.flags,
            logical: d.logical,
            logical_count: ptr::null(),
            scan: scans[i].as_ptr(),
            scan_words: scans[i].len() as u64,
        })
        .collect();
    context(&layouts, test);
}

#[test]
fn compiler_record_variant_list_box_descriptors_plan_real_mixed_graphs() {
    for active in [false, true] {
        let target = TargetSpec::executed().into_iter().next().unwrap();
        let mut types = Types::default();
        let number = types.p(Primitive::U64);
        let list = types.list(number);
        let variant = types.variant(&[&[], &[list]]);
        let boxed = types.boxed(variant);
        let record = types.record(&[number, list, boxed]);
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let root_id = builder.layout(&types.store, record).unwrap();
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        let fields = &table.layouts[root_id.0 as usize].fields;
        let LayoutKind::Block {
            payload: list_payload,
        } = table.layouts[fields[1].layout.0 as usize].kind
        else {
            panic!("list holder")
        };
        let LayoutKind::Box {
            payload: box_payload,
        } = table.layouts[fields[2].layout.0 as usize].kind
        else {
            panic!("box holder")
        };
        let tag = table.layouts[box_payload.0 as usize].tag.as_ref().unwrap();
        native(&table, |ctx| unsafe {
            let run = MetadataRun::new();
            let mut guard = DemandedGuard::from_raw(ctx, run);
            let leaf = heap(ctx, list_payload.0 + 2, 24);
            payload(leaf).cast::<[u64; 3]>().write([1, 1, 7]);
            let leaf_receipt = image(run, Origin::Heap, 8, payload(leaf), 24, &[]);
            let box_desc = &*(*ctx).descriptor(box_payload.0 + 2);
            let block = hirsel_rt_v2_arena_alloc(
                ctx,
                guard.base(),
                box_payload.0 + 2,
                box_desc.size,
                box_desc.align,
                0,
            );
            ptr::write_bytes(payload(block), 0, box_desc.size as usize);
            if active {
                crate::count::hirsel_rt_v2_dup(ctx, leaf);
                let field = &tag.variants[1].fields[0];
                payload(block)
                    .add(field.offset as usize)
                    .cast::<*mut Header>()
                    .write_unaligned(leaf);
                if matches!(tag.form, hirsel_lir::layout_types::TagForm::Ordinal) {
                    payload(block)
                        .add(tag.offset as usize)
                        .cast::<u32>()
                        .write_unaligned(1);
                }
            }
            let box_receipt = image(
                run,
                Origin::Arena(guard.grant().unwrap()),
                8,
                payload(block),
                box_desc.size as usize,
                if active {
                    std::slice::from_ref(&leaf_receipt)
                } else {
                    &[]
                },
            );
            let allocation = guard
                .allocate(Update {
                    value: box_receipt.clone(),
                    additions: if active { vec![leaf] } else { vec![] },
                })
                .ok()
                .unwrap();
            let root_desc = &*(*ctx).descriptor(root_id.0 + 2);
            let mut source = vec![0u64; (root_desc.size as usize).div_ceil(8)];
            let at = source.as_mut_ptr().cast::<u8>();
            at.add(fields[0].offset as usize).cast::<u64>().write(9);
            at.add(fields[1].offset as usize)
                .cast::<*mut Header>()
                .write(leaf);
            at.add(fields[2].offset as usize)
                .cast::<*mut Header>()
                .write(block);
            let metadata = image(
                run,
                Origin::Inline,
                0,
                at,
                root_desc.size as usize,
                &[leaf_receipt, box_receipt],
            );
            let bindings = [Retained {
                allocation,
                descriptor: box_payload.0 + 2,
                block: Some(block),
                extent: box_desc.size as usize,
            }];
            let plan = plan(&guard, root_id.0 + 2, at, &metadata, &bindings).unwrap();
            assert_eq!(plan.charge, Charge::Bytes(if active { 16 } else { 8 }));
            assert_eq!(plan.blocks.len(), 2);
            assert_eq!(plan.root.patches.len(), 2);
            let owner = guard.seal(&metadata, &plan).unwrap();
            assert_eq!(guard.close(0, 0), 0);
            drop(source);
            let moved = root(owner)
                .add(fields[1].offset as usize)
                .cast::<*mut Header>()
                .read();
            assert_ne!(moved, leaf);
            assert_eq!(payload(moved).add(16).cast::<u64>().read(), 7);
            if active {
                let field = &tag.variants[1].fields[0];
                assert_eq!(
                    payload(block)
                        .add(field.offset as usize)
                        .cast::<*mut Header>()
                        .read_unaligned(),
                    moved
                );
            }
            drop_owner(ctx, owner);
        });
    }
}

#[test]
fn compiler_unit_bits_and_empty_lists_still_build_and_publish() {
    let target = TargetSpec::executed().into_iter().next().unwrap();
    let mut types = Types::default();
    let number = types.p(Primitive::U64);
    let unit = types.tuple(&[]);
    let list = types.list(number);
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let bit_id = builder.layout(&types.store, number).unwrap();
    let unit_id = builder.layout(&types.store, unit).unwrap();
    let list_id = builder.layout(&types.store, list).unwrap();
    let table = builder.finish();
    native(&table, |ctx| unsafe {
        for (id, value, size) in [(bit_id, 7u64, 8usize), (unit_id, 0, 0)] {
            let run = MetadataRun::new();
            let mut guard = DemandedGuard::from_raw(ctx, run);
            let mut value = value;
            let metadata = image(
                run,
                Origin::Inline,
                0,
                ptr::from_ref(&value).cast(),
                size,
                &[],
            );
            let plan = plan(
                &guard,
                id.0 + 2,
                ptr::from_mut(&mut value).cast(),
                &metadata,
                &[],
            )
            .unwrap();
            let owner = guard.seal(&metadata, &plan).unwrap();
            assert_eq!(guard.close(0, 0), 0);
            drop_owner(ctx, owner);
        }
        let LayoutKind::Block {
            payload: payload_id,
        } = table.layouts[list_id.0 as usize].kind
        else {
            panic!("holder")
        };
        let run = MetadataRun::new();
        let mut guard = DemandedGuard::from_raw(ctx, run);
        let mut block = heap(ctx, payload_id.0 + 2, 16);
        let child = image(run, Origin::Heap, 0, payload(block), 16, &[]);
        let metadata = image(
            run,
            Origin::Inline,
            0,
            ptr::from_ref(&block).cast(),
            8,
            &[child],
        );
        let plan = plan(
            &guard,
            list_id.0 + 2,
            ptr::from_mut(&mut block).cast(),
            &metadata,
            &[],
        )
        .unwrap();
        assert_eq!(plan.charge, Charge::Bytes(0));
        let owner = guard.seal(&metadata, &plan).unwrap();
        assert_eq!(guard.close(0, 0), 0);
        drop_owner(ctx, owner);
    });
}

#[test]
fn compiler_retained_b3_b7_b8_inventory_survives_growth_cow_and_equal_storage() {
    let target = TargetSpec::executed().into_iter().next().unwrap();
    let mut types = Types::default();
    let bytes_type = types.p(Primitive::Bytes);
    let list = types.list(bytes_type);
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let root_id = builder.layout(&types.store, list).unwrap();
    let scalar = types.p(Primitive::U64);
    let scalar_id = builder.layout(&types.store, scalar).unwrap();
    let table = builder.finish();
    let LayoutKind::Block {
        payload: payload_id,
    } = table.layouts[root_id.0 as usize].kind
    else {
        panic!("list holder")
    };
    for cow in [false, true] {
        for distinct in [false, true] {
            for live in [false, true] {
                native(&table, |ctx| unsafe {
                    let run = MetadataRun::new();
                    let mut guard = DemandedGuard::from_raw(ctx, run);
                    // One source alias remains outside the region; each journal
                    // occurrence owns its own independent source credit.
                    let leaf = heap(ctx, 0, 19);
                    payload(leaf).cast::<[u64; 2]>().write([3, 3]);
                    ptr::copy_nonoverlapping(b"abc".as_ptr(), payload(leaf).add(16), 3);
                    let leaf_receipt = image(run, Origin::Heap, 3, payload(leaf), 19, &[]);
                    let other = if distinct {
                        let other = heap(ctx, 0, 19);
                        ptr::copy_nonoverlapping(payload(leaf), payload(other), 19);
                        other
                    } else {
                        leaf
                    };
                    let other_receipt = image(run, Origin::Heap, 3, payload(other), 19, &[]);
                    crate::count::hirsel_rt_v2_dup(ctx, leaf);
                    let first =
                        hirsel_rt_v2_arena_alloc(ctx, guard.base(), payload_id.0 + 2, 40, 8, 0);
                    ptr::write_bytes(payload(first), 0, 40);
                    payload(first).cast::<[u64; 2]>().write([1, 1]);
                    payload(first).add(16).cast::<crate::views::Slice>().write(
                        crate::views::Slice {
                            owner: leaf,
                            start: 8,
                            length: 3,
                        },
                    );
                    let old = image(
                        run,
                        Origin::Arena(guard.grant().unwrap()),
                        8,
                        payload(first),
                        40,
                        std::slice::from_ref(&leaf_receipt),
                    );
                    let old_id = guard
                        .allocate(Update {
                            value: old,
                            additions: vec![leaf],
                        })
                        .ok()
                        .unwrap();
                    let mut bindings = Vec::new();
                    if cow {
                        bindings.push(Retained {
                            allocation: old_id,
                            descriptor: payload_id.0 + 2,
                            block: Some(first),
                            extent: 40,
                        });
                        crate::count::hirsel_rt_v2_dup(ctx, leaf);
                    }
                    crate::count::hirsel_rt_v2_dup(ctx, other);
                    let second =
                        hirsel_rt_v2_arena_alloc(ctx, guard.base(), payload_id.0 + 2, 64, 8, 0);
                    ptr::write_bytes(payload(second), 0, 64);
                    payload(second).cast::<[u64; 2]>().write([2, 2]);
                    payload(second)
                        .add(16)
                        .cast::<[crate::views::Slice; 2]>()
                        .write([
                            crate::views::Slice {
                                owner: leaf,
                                start: 8,
                                length: 3,
                            },
                            crate::views::Slice {
                                owner: other,
                                start: 8,
                                length: 3,
                            },
                        ]);
                    let current = image(
                        run,
                        Origin::Arena(guard.grant().unwrap()),
                        16,
                        payload(second),
                        64,
                        &[leaf_receipt, other_receipt],
                    );
                    let id = if cow {
                        guard
                            .allocate(Update {
                                value: current.clone(),
                                additions: vec![leaf, other],
                            })
                            .ok()
                            .unwrap()
                    } else {
                        // Growth transfers payload custody, leaving abandoned
                        // physical backing rather than a second semantic row.
                        ptr::write_bytes(payload(first), 0, 40);
                        guard
                            .replace(
                                old_id,
                                &[0],
                                Update {
                                    value: current.clone(),
                                    additions: vec![other],
                                },
                            )
                            .ok()
                            .unwrap();
                        old_id
                    };
                    bindings.push(Retained {
                        allocation: id,
                        descriptor: payload_id.0 + 2,
                        block: Some(second),
                        extent: 64,
                    });
                    let mut source = if live { second.addr() as u64 } else { 7 };
                    let metadata = image(
                        run,
                        Origin::Inline,
                        0,
                        ptr::from_ref(&source).cast(),
                        8,
                        if live {
                            std::slice::from_ref(&current)
                        } else {
                            &[]
                        },
                    );
                    let root_index = if live { root_id.0 + 2 } else { scalar_id.0 + 2 };
                    let plan = plan(
                        &guard,
                        root_index,
                        ptr::from_mut(&mut source).cast(),
                        &metadata,
                        &bindings,
                    )
                    .unwrap();
                    assert_eq!(plan.charge, Charge::Bytes(if cow { 9 } else { 6 }));
                    let construction = if cow { 24 } else { 16 };
                    guard.bound(construction + if cow { 8 } else { 5 }).unwrap();
                    (*guard.base()).remaining -= construction;
                    assert!(guard.seal(&metadata, &plan).is_err());
                    assert!(!guard.was_sealed());
                    guard.bound(construction + if cow { 9 } else { 6 }).unwrap();
                    (*guard.base()).remaining -= construction;
                    let owner = guard.seal(&metadata, &plan).unwrap();
                    assert_eq!(guard.close(0, 0), 0);
                    assert_eq!((*leaf).count, 1);
                    if distinct {
                        assert_eq!((*other).count, 1);
                    }
                    if live {
                        assert_eq!(root(owner).cast::<*mut Header>().read(), second);
                    } else {
                        assert_eq!(root(owner).cast::<u64>().read(), 7);
                    }
                    let views = payload(second)
                        .add(16)
                        .cast::<[crate::views::Slice; 2]>()
                        .read();
                    assert_eq!((views[0].text(), views[1].text()), ("abc", "abc"));
                    assert_eq!(views[0].owner == views[1].owner, !distinct);
                    drop_owner(ctx, owner);
                    crate::release::release_block(&mut *ctx, leaf, ptr::null(), 0, 0);
                    if distinct {
                        crate::release::release_block(&mut *ctx, other, ptr::null(), 0, 0);
                    }
                });
            }
        }
    }
}

#[test]
fn compiler_unit_sequence_needs_no_work_per_zero_sized_element() {
    let target = TargetSpec::executed().into_iter().next().unwrap();
    let mut types = Types::default();
    let unit = types.tuple(&[]);
    let list = types.list(unit);
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let id = builder.layout(&types.store, list).unwrap();
    let table = builder.finish();
    let LayoutKind::Block {
        payload: payload_id,
    } = table.layouts[id.0 as usize].kind
    else {
        panic!("holder")
    };
    native(&table, |ctx| unsafe {
        let run = MetadataRun::new();
        let mut guard = DemandedGuard::from_raw(ctx, run);
        let mut block = heap(ctx, payload_id.0 + 2, 16);
        payload(block)
            .cast::<[u64; 2]>()
            .write([u64::MAX, u64::MAX]);
        let child = image(run, Origin::Heap, 0, payload(block), 16, &[]);
        let metadata = image(
            run,
            Origin::Inline,
            0,
            ptr::from_ref(&block).cast(),
            8,
            &[child],
        );
        let plan = plan(
            &guard,
            id.0 + 2,
            ptr::from_mut(&mut block).cast(),
            &metadata,
            &[],
        )
        .unwrap();
        assert_eq!(plan.blocks.len(), 1);
        assert!(plan.blocks[0].patches.is_empty());
        let owner = guard.seal(&metadata, &plan).unwrap();
        assert_eq!(guard.close(0, 0), 0);
        let moved = root(owner).cast::<*mut Header>().read();
        assert_eq!(payload(moved).cast::<u64>().read(), u64::MAX);
        drop_owner(ctx, owner);
    });
}
