use super::super::receipt::{MetadataRun, journal::Update, semantic};
use super::super::{Charge, drop_owner, root, transaction};
use super::*;
use crate::context::{
    CONFIG_DIAGNOSTIC, Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_ctx_stats,
};
use crate::region::hirsel_rt_v2_arena_alloc;
use hirsel_core_contract::handle_scan::*;
use std::ptr;
mod compiler;

fn op(code: u64, operands: &[u64]) -> Vec<u64> {
    let mut words = vec![0; 8];
    words[0] = code;
    words[1..1 + operands.len()].copy_from_slice(operands);
    words
}
fn words(ops: &[Vec<u64>]) -> Vec<u64> {
    ops.concat()
}
fn release(offset: u64, slice: bool, shared: bool) -> crate::Step {
    crate::Step {
        kind: if shared {
            crate::STEP_RELEASE_ROOT
        } else {
            crate::STEP_RELEASE
        },
        field: crate::WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset,
        stride: if slice { 24 } else { 0 },
        count: 0,
        count_offset: crate::NO_OFFSET,
        layout: ptr::null(),
    }
}
pub(in crate::sealed_region) fn fixture(test: impl FnOnce(*mut TaskCtx)) {
    fixture_with(|_| {}, test);
}
fn fixture_with(change: impl FnOnce(&mut Vec<Vec<u64>>), test: impl FnOnce(*mut TaskCtx)) {
    let mut scans = vec![
        op(VALUE, &[VALUE_BITS, 8]), //2
        op(VALUE, &[VALUE_BITS, 1]), //3
        words(&[
            op(SEQUENCE, &[16, 0, 1, 3]),
            op(VALUE, &[VALUE_SEQUENCE, 16, 1, 16, 3]),
        ]), //4 byte owner
        words(&[
            op(BLOCK, &[]),
            op(TYPE_EDGE, &[0, 3]),
            op(VALUE, &[VALUE_SLICE, 24, 3]),
        ]), //5 slice
        words(&[
            op(INLINE, &[0, 5]),
            op(INLINE, &[24, 5]),
            op(VALUE, &[VALUE_FIELDS, 48]),
            op(VALUE_FIELD, &[0, 5, u64::MAX]),
            op(VALUE_FIELD, &[24, 5, u64::MAX]),
        ]), //6 pair
        words(&[
            op(BLOCK, &[]),
            op(TYPE_EDGE, &[0, 6]),
            op(VALUE, &[VALUE_INDIRECT, 8, 6]),
        ]), //7 pair holder
        op(VALUE, &[VALUE_BITS, 8]), //8 shared payload
        words(&[
            op(ROOT, &[]),
            op(TYPE_EDGE, &[0, 8]),
            op(VALUE, &[VALUE_INDIRECT, 8, 8]),
        ]), //9 shared holder
        words(&[
            op(INLINE, &[0, 9]),
            op(INLINE, &[8, 9]),
            op(VALUE, &[VALUE_FIELDS, 16]),
            op(VALUE_FIELD, &[0, 9, u64::MAX]),
            op(VALUE_FIELD, &[8, 9, u64::MAX]),
        ]), //10 shared pair
        words(&[op(BLOCK, &[]), op(VALUE, &[VALUE_INDIRECT, 8, 10])]), //11 shared pair holder
        words(&[
            op(INLINE, &[0, 11]),
            op(INLINE, &[8, 11]),
            op(VALUE, &[VALUE_FIELDS, 16]),
            op(VALUE_FIELD, &[0, 11, u64::MAX]),
            op(VALUE_FIELD, &[8, 11, u64::MAX]),
        ]), //12 diamond
    ];
    change(&mut scans);
    let steps = [
        vec![],
        vec![],
        vec![],
        vec![release(0, true, false)],
        vec![release(0, true, false), release(24, true, false)],
        vec![release(0, false, false)],
        vec![],
        vec![release(0, false, true)],
        vec![release(0, false, true), release(8, false, true)],
        vec![release(0, false, false)],
        vec![release(0, false, false), release(8, false, false)],
    ];
    let sizes = [8, 1, 16, 24, 48, 8, 8, 8, 16, 8, 16];
    let layouts: Vec<_> = sizes
        .iter()
        .enumerate()
        .map(|(i, &size)| Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size,
            align: if i == 1 { 1 } else { 8 },
            finalizer: None,
            steps: steps[i].as_ptr(),
            step_count: steps[i].len() as u64,
            field_count: if i == 4 || i == 8 || i == 10 { 2 } else { 0 },
            flags: if i == 2 {
                crate::LAYOUT_DYNAMIC | crate::LAYOUT_SEQUENCE
            } else if i == 6 {
                crate::LAYOUT_FROZEN
            } else {
                0
            },
            logical: 0,
            logical_count: ptr::null(),
            scan: scans[i].as_ptr(),
            scan_words: scans[i].len() as u64,
        })
        .collect();
    context(&layouts, test);
}
fn context(layouts: &[Layout], test: impl FnOnce(*mut TaskCtx)) {
    let pointers: Vec<_> = layouts.iter().map(ptr::from_ref).collect();
    let config = Config {
        flags: CONFIG_DIAGNOSTIC,
        layouts: pointers.as_ptr(),
        layout_count: pointers.len() as u32,
        ..Config::new()
    };
    let mut ctx = ptr::null_mut();
    assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
    test(ctx);
    let mut stats = Stats::default();
    unsafe { hirsel_rt_v2_ctx_stats(ctx, &mut stats) };
    assert_eq!((stats.live_blocks, stats.active_regions), (0, 0));
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}
unsafe fn heap(ctx: *mut TaskCtx, index: u32, extent: usize) -> *mut Header {
    let block = unsafe {
        crate::alloc::hirsel_rt_v2_alloc(ctx, index, extent as u64, 8, crate::DOMAIN_HEAP, 0)
    };
    unsafe { ptr::write_bytes(payload(block), 0, extent) };
    block
}
unsafe fn image(
    run: MetadataRun,
    origin: Origin,
    logical: u64,
    at: *const u8,
    size: usize,
    children: &[Receipt<Record>],
) -> Receipt<Record> {
    semantic::snapshot(
        run,
        origin,
        Charge::Bytes(logical),
        unsafe { std::slice::from_raw_parts(at, size) },
        8,
        children,
    )
    .unwrap()
}
unsafe fn bytes(ctx: *mut TaskCtx, run: MetadataRun) -> (*mut Header, Receipt<Record>) {
    let block = unsafe { heap(ctx, 4, 21) };
    unsafe {
        payload(block).cast::<[u64; 2]>().write([5, 5]);
        ptr::copy_nonoverlapping(b"abcde".as_ptr(), payload(block).add(16), 5)
    };
    let receipt = unsafe { image(run, Origin::Heap, 3, payload(block), 21, &[]) };
    (block, receipt)
}
fn slices(block: *mut Header) -> [crate::views::Slice; 2] {
    [
        crate::views::Slice {
            owner: block,
            start: 8,
            length: 3,
        },
        crate::views::Slice {
            owner: block,
            start: 9,
            length: 2,
        },
    ]
}

#[test]
fn heap_graph_views_external_alias_and_every_physical_checkpoint() {
    fixture(|ctx| unsafe {
        let run = MetadataRun::new();
        let mut guard = DemandedGuard::from_raw(ctx, run);
        let (leaf, leaf_receipt) = bytes(ctx, run);
        crate::count::hirsel_rt_v2_dup(ctx, leaf);
        let parent = heap(ctx, 6, 48);
        payload(parent)
            .cast::<[crate::views::Slice; 2]>()
            .write(slices(leaf));
        crate::count::hirsel_rt_v2_dup(ctx, parent); // external alias
        let second_receipt = image(run, Origin::Heap, 2, payload(leaf), 21, &[]);
        let parent_receipt = image(
            run,
            Origin::Heap,
            16,
            payload(parent),
            48,
            &[leaf_receipt, second_receipt],
        );
        let mut source = parent;
        let metadata = image(
            run,
            Origin::Inline,
            0,
            ptr::from_ref(&source).cast(),
            8,
            &[parent_receipt],
        );
        let plan = plan(&guard, 7, ptr::from_mut(&mut source).cast(), &metadata, &[]).unwrap();
        assert_eq!(plan.blocks.len(), 2);
        assert_eq!(plan.charge, Charge::Bytes(21));
        assert_eq!(
            plan.blocks[0].patches[0].target,
            plan.blocks[0].patches[1].target
        );
        let expected = [
            transaction::Event::Owner,
            transaction::Event::Snapshot,
            transaction::Event::Extension,
            transaction::Event::Block(0),
            transaction::Event::Block(1),
            transaction::Event::Ready,
        ];
        for &failure in &expected {
            let result = transaction::run_with_journal(
                ctx,
                guard.base(),
                &plan,
                &mut |event| {
                    if event == failure {
                        Err(super::super::SealError::Interrupted)
                    } else {
                        Ok(())
                    }
                },
                None,
            );
            assert_eq!(result, Err(super::super::SealError::Interrupted));
            assert_eq!(((*parent).count, (*leaf).count), (2, 2));
            assert_eq!(source, parent);
            assert!(!guard.was_sealed());
            assert_eq!(
                payload(parent).cast::<[crate::views::Slice; 2]>().read()[1].start,
                9
            );
        }
        let owner = guard.seal(&metadata, &plan).unwrap();
        assert_eq!((*parent).count, 1);
        assert_eq!(guard.close(0, 0), 0);
        let moved = root(owner).cast::<*mut Header>().read();
        assert_ne!(moved, parent);
        let views = payload(moved).cast::<[crate::views::Slice; 2]>().read();
        assert_eq!(views[0].owner, views[1].owner);
        assert_ne!(views[0].owner, leaf);
        assert_eq!((views[0].text(), views[1].text()), ("abc", "bc"));
        payload(leaf).add(16).write(b'z');
        assert_eq!(views[0].text(), "abc");
        crate::release::release_block(&mut *ctx, parent, ptr::null(), 0, 0);
        drop_owner(ctx, owner);
    });
}

#[test]
fn retained_dead_live_and_elided_inventory_charge_and_rollback() {
    for live in [false, true] {
        for elided in [false, true] {
            if live && elided {
                continue;
            }
            fixture(|ctx| unsafe {
                let run = MetadataRun::new();
                let mut guard = DemandedGuard::from_raw(ctx, run);
                let (leaf, leaf_receipt) = bytes(ctx, run);
                crate::count::hirsel_rt_v2_dup(ctx, leaf);
                let holders = [slices(leaf)[0]; 2];
                let parent = if elided {
                    None
                } else {
                    Some(hirsel_rt_v2_arena_alloc(
                        ctx,
                        guard.base(),
                        6,
                        48,
                        8,
                        crate::region::ALLOC_TRACKED,
                    ))
                };
                if let Some(parent) = parent {
                    payload(parent)
                        .cast::<[crate::views::Slice; 2]>()
                        .write(holders);
                }
                let at =
                    parent.map_or(holders.as_ptr().cast(), |block| payload(block).cast_const());
                let retained = image(
                    run,
                    Origin::Arena(guard.grant().unwrap()),
                    16,
                    at,
                    48,
                    &[leaf_receipt.clone(), leaf_receipt],
                );
                let allocation = guard
                    .allocate(Update {
                        value: retained.clone(),
                        additions: vec![leaf, leaf],
                    })
                    .ok()
                    .unwrap();
                let bindings = [Retained {
                    allocation,
                    descriptor: 6,
                    block: parent,
                    extent: 48,
                }];
                let mut source = if live {
                    parent.unwrap().addr() as u64
                } else {
                    7
                };
                let metadata = image(
                    run,
                    Origin::Inline,
                    0,
                    ptr::from_ref(&source).cast(),
                    8,
                    if live {
                        std::slice::from_ref(&retained)
                    } else {
                        &[]
                    },
                );
                let plan = plan(
                    &guard,
                    if live { 7 } else { 2 },
                    ptr::from_mut(&mut source).cast(),
                    &metadata,
                    &bindings,
                )
                .unwrap();
                assert_eq!(plan.charge, Charge::Bytes(6));
                assert_eq!(plan.blocks.len(), if elided { 1 } else { 2 });
                guard.bound(5).unwrap();
                assert!(guard.seal(&metadata, &plan).is_err());
                assert!(!guard.was_sealed());
                assert_eq!((*leaf).count, 2);
                guard.bound(6).unwrap();
                let owner = guard.seal(&metadata, &plan).unwrap();
                assert_eq!(guard.close(0, 0), 0);
                if let Some(parent) = parent {
                    if live {
                        assert_eq!(root(owner).cast::<*mut Header>().read(), parent);
                    }
                    let view = payload(parent).cast::<crate::views::Slice>().read();
                    assert_eq!(view.text(), "abc");
                } else {
                    assert_eq!(root(owner).cast::<u64>().read(), 7);
                }
                drop_owner(ctx, owner);
            });
        }
    }
}

#[test]
fn shared_occurrences_survive_physical_parent_dedup_and_final_drop() {
    fixture(|ctx| unsafe {
        let run = MetadataRun::new();
        let mut guard = DemandedGuard::from_raw(ctx, run);
        let shared = heap(ctx, 8, 8);
        crate::roots::hirsel_rt_v2_root_dup(ctx, shared);
        crate::roots::hirsel_rt_v2_root_dup(ctx, shared);
        let shared_receipt = image(
            run,
            Origin::Shared,
            0,
            ptr::from_ref(&shared).cast(),
            8,
            &[],
        );
        let parent = heap(ctx, 10, 16);
        payload(parent)
            .cast::<[*mut Header; 2]>()
            .write([shared, shared]);
        crate::count::hirsel_rt_v2_dup(ctx, parent);
        let parent_receipt = image(
            run,
            Origin::Heap,
            16,
            payload(parent),
            16,
            &[shared_receipt.clone(), shared_receipt],
        );
        let mut source = [parent, parent];
        let metadata = image(
            run,
            Origin::Inline,
            0,
            ptr::from_ref(&source).cast(),
            16,
            &[parent_receipt.clone(), parent_receipt],
        );
        let plan = plan(
            &guard,
            12,
            ptr::from_mut(&mut source).cast(),
            &metadata,
            &[],
        )
        .unwrap();
        assert_eq!(plan.shared, [shared, shared, shared, shared]);
        assert_eq!(plan.blocks.len(), 1);
        for failure in [
            transaction::Event::Shared(0),
            transaction::Event::Shared(1),
            transaction::Event::Shared(2),
            transaction::Event::Shared(3),
            transaction::Event::Ready,
        ] {
            assert_eq!(
                transaction::run_with_journal(
                    ctx,
                    guard.base(),
                    &plan,
                    &mut |event| if event == failure {
                        Err(super::super::SealError::Interrupted)
                    } else {
                        Ok(())
                    },
                    None
                ),
                Err(super::super::SealError::Interrupted)
            );
            assert_eq!(crate::roots::count(&*ctx, shared), 3);
        }
        let owner = guard.seal(&metadata, &plan).unwrap();
        assert_eq!(guard.close(0, 0), 0);
        assert_eq!(crate::roots::count(&*ctx, shared), 5);
        drop_owner(ctx, owner);
        assert_eq!(crate::roots::count(&*ctx, shared), 1);
        crate::roots::hirsel_rt_v2_root_release(ctx, shared);
    });
}

#[test]
fn omitted_edges_wrong_descriptors_images_and_foreign_grants_refuse_without_custody_change() {
    for mutation in 0..7 {
        fixture(|ctx| unsafe {
            let run = MetadataRun::new();
            let mut guard = DemandedGuard::from_raw(ctx, run);
            let (leaf, leaf_receipt) = bytes(ctx, run);
            let mut source = crate::views::Slice {
                owner: leaf,
                start: 8,
                length: 3,
            };
            let child = if mutation == 5 {
                image(run, Origin::Shared, 0, ptr::from_ref(&leaf).cast(), 8, &[])
            } else {
                leaf_receipt
            };
            let metadata = image(
                run,
                Origin::Inline,
                0,
                ptr::from_ref(&source).cast(),
                24,
                if mutation == 0 {
                    &[]
                } else {
                    std::slice::from_ref(&child)
                },
            );
            if mutation == 1 {
                source.start = 99;
            }
            if mutation == 2 {
                source.length = u64::MAX;
            }
            let foreign = MetadataRun::new();
            let metadata = if mutation == 3 {
                image(
                    foreign,
                    Origin::Inline,
                    0,
                    ptr::from_ref(&source).cast(),
                    24,
                    std::slice::from_ref(
                        &semantic::snapshot(foreign, Origin::Heap, Charge::Bytes(3), &[], 8, &[])
                            .unwrap(),
                    ),
                )
            } else {
                metadata
            };
            let index = if mutation == 4 {
                2
            } else if mutation == 6 {
                u32::MAX
            } else {
                5
            };
            assert!(
                plan(
                    &guard,
                    index,
                    ptr::from_mut(&mut source).cast(),
                    &metadata,
                    &[]
                )
                .is_err()
            );
            assert_eq!((*leaf).count, 1);
            assert!(!guard.was_sealed());
            crate::release::release_block(&mut *ctx, leaf, ptr::null(), 0, 0);
            assert_eq!(guard.close(0, 0), 0);
        });
    }
}

#[test]
fn corrupt_descriptor_coverage_refuses_a_valid_original_receipt() {
    for mutation in 0..6 {
        fixture_with(
            |scans| match mutation {
                0 => {
                    scans[4].drain(..8);
                } // omit first holder scan
                1 => {
                    let remaining = scans[4].len() - 8;
                    scans[4].truncate(remaining);
                } // omit typed child
                2 => {
                    scans[4][3 * 8 + 2] = u64::MAX;
                } // wrong linked field
                3 => {
                    scans[4][3 * 8 + 1] = u64::MAX;
                } // field outside image
                4 => {
                    scans[4][0] = UNKNOWN;
                }
                5 => {
                    scans[4][2 * 8 + 1] = VALUE_UNKNOWN;
                }
                _ => unreachable!(),
            },
            |ctx| unsafe {
                let run = MetadataRun::new();
                let guard = DemandedGuard::from_raw(ctx, run);
                let (leaf, leaf_receipt) = bytes(ctx, run);
                crate::count::hirsel_rt_v2_dup(ctx, leaf);
                let mut source = slices(leaf);
                let metadata = image(
                    run,
                    Origin::Inline,
                    0,
                    ptr::from_ref(&source).cast(),
                    48,
                    &[leaf_receipt.clone(), leaf_receipt],
                );
                assert!(
                    plan(&guard, 6, ptr::from_mut(&mut source).cast(), &metadata, &[]).is_err()
                );
                assert_eq!((*leaf).count, 2);
                assert!(!guard.was_sealed());
                crate::release::release_block(&mut *ctx, leaf, ptr::null(), 0, 0);
                crate::release::release_block(&mut *ctx, leaf, ptr::null(), 0, 0);
                assert_eq!(guard.close(0, 0), 0);
            },
        );
    }
}

#[test]
fn dead_elided_shared_inventory_requires_exact_binding_and_keeps_each_credit() {
    fixture(|ctx| unsafe {
        let run = MetadataRun::new();
        let mut guard = DemandedGuard::from_raw(ctx, run);
        let shared = heap(ctx, 8, 8);
        crate::roots::hirsel_rt_v2_root_dup(ctx, shared);
        crate::roots::hirsel_rt_v2_root_dup(ctx, shared);
        let word = shared;
        let leaf = image(run, Origin::Shared, 0, ptr::from_ref(&word).cast(), 8, &[]);
        let holders = [shared, shared];
        let record = image(
            run,
            Origin::Arena(guard.grant().unwrap()),
            16,
            holders.as_ptr().cast(),
            16,
            &[leaf.clone(), leaf],
        );
        let allocation = guard
            .allocate(Update {
                value: record,
                additions: vec![shared, shared],
            })
            .ok()
            .unwrap();
        let mut scalar = 7u64;
        let metadata = image(
            run,
            Origin::Inline,
            0,
            ptr::from_ref(&scalar).cast(),
            8,
            &[],
        );
        assert!(matches!(
            plan(&guard, 2, ptr::from_mut(&mut scalar).cast(), &metadata, &[]),
            Err(Error::Coverage)
        ));
        let bindings = [Retained {
            allocation,
            descriptor: 10,
            block: None,
            extent: 16,
        }];
        let plan = plan(
            &guard,
            2,
            ptr::from_mut(&mut scalar).cast(),
            &metadata,
            &bindings,
        )
        .unwrap();
        assert!(plan.blocks.is_empty());
        assert_eq!(plan.shared, [shared, shared]);
        assert_eq!(plan.charge, Charge::Bytes(0));
        let owner = guard.seal(&metadata, &plan).unwrap();
        assert_eq!(guard.close(0, 0), 0);
        assert_eq!(crate::roots::count(&*ctx, shared), 3);
        assert_eq!(root(owner).cast::<u64>().read(), 7);
        drop_owner(ctx, owner);
        assert_eq!(crate::roots::count(&*ctx, shared), 1);
        crate::roots::hirsel_rt_v2_root_release(ctx, shared);
    });
}
