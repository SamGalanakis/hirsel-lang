//! Native entry coverage with compiler descriptors and recoverable interruption
//! of the same prepare path. Expected sums are fixed independently of the walk.
use super::*;
use crate::alloc::{Header, payload};
use crate::sealed_region::receipt::journal::Update;
use crate::sealed_region::transaction::Event;
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::layout_types::{LayoutKind, LayoutTable, TargetSpec};
use hirsel_types::{Primitive, testing::Types};

type Receipt = crate::sealed_region::receipt::Receipt<semantic::Record>;

unsafe fn snapshot(
    run: MetadataRun,
    origin: Origin,
    bytes: u64,
    source: *const u8,
    extent: usize,
    children: &[Receipt],
) -> Receipt {
    semantic::snapshot(
        run,
        origin,
        Charge::Bytes(bytes),
        unsafe { std::slice::from_raw_parts(source, extent) },
        8,
        children,
    )
    .unwrap()
}

fn descriptors(kind: u8) -> (LayoutTable, hirsel_lir::LayoutId) {
    let target = TargetSpec::executed().into_iter().next().unwrap();
    let mut types = Types::default();
    let integer = types.p(Primitive::U64);
    let list = types.list(integer);
    let pair = types.record(&[list, list]);
    let record = types.record(&[integer, pair]);
    let root = match kind {
        0 | 1 => list,
        2 => record,
        _ => types.boxed(record),
    };
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let root = builder.layout(&types.store, root).unwrap();
    // The seed gives test metadata the native entry's exact run identity.
    builder.layout(&types.store, integer).unwrap();
    let table = builder.finish();
    hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
    (table, root)
}

unsafe fn leaf(
    ctx: *mut TaskCtx,
    run: MetadataRun,
    index: u32,
    values: &[u64],
) -> (*mut Header, Receipt) {
    let size = 16 + 8 * values.len();
    let block = unsafe {
        crate::alloc::hirsel_rt_v2_alloc(ctx, index, size as u64, 8, crate::DOMAIN_HEAP, 0)
    };
    unsafe {
        payload(block).cast::<u64>().write(values.len() as u64);
        payload(block)
            .add(8)
            .cast::<u64>()
            .write(values.len() as u64);
        ptr::copy_nonoverlapping(values.as_ptr(), payload(block).add(16).cast(), values.len());
        // The independent alias makes premature source retirement observable.
        crate::count::hirsel_rt_v2_dup(ctx, block);
    }
    let metadata = unsafe {
        snapshot(
            run,
            Origin::Heap,
            (values.len() * 8) as u64,
            payload(block),
            size,
            &[],
        )
    };
    (block, metadata)
}

#[test]
fn native_entry_91_list_nested_record_and_adopted_root_rollback_each_physical_index() {
    let mut indices = 0;
    for kind in 0..=3 {
        let (table, root_id) = descriptors(kind);
        crate::sealed_region::planner::tests::compiler::native(&table, |ctx| unsafe {
            let scalar = table
                .layouts
                .iter()
                .position(|r| {
                    r.kind == LayoutKind::Scalar(hirsel_lir::layout_types::ScalarKind::U64)
                })
                .unwrap() as u32
                + 2;
            let seed = 0u64;
            let raw_seed = hirsel_rt_v2_receipt_bits(ctx, scalar, ptr::from_ref(&seed).cast());
            let seed_receipt = Receipt::from_raw(raw_seed);
            let run = seed_receipt.run();
            let mut guard = MaybeUninit::<ReceiptArenaGuard<'static>>::uninit();
            let raw = guard.as_mut_ptr().cast();
            hirsel_rt_v2_receipt_region_enter(ctx, raw);
            let base = ptr::addr_of_mut!((*guard.as_mut_ptr()).base);
            let journal = &mut *(*guard.as_mut_ptr()).inventory;
            let list_payload = table
                .layouts
                .iter()
                .position(|r| matches!(r.kind, LayoutKind::Sequence { .. }))
                .unwrap() as u32
                + 2;
            let (first, first_metadata) = leaf(
                ctx,
                run,
                list_payload,
                if kind == 0 { &[] } else { &[11, 12] },
            );
            let second = if kind >= 2 {
                Some(leaf(ctx, run, list_payload, &[3, 4]))
            } else {
                None
            };
            let mut source = [0u64; 3];
            let mut adopted = ptr::null_mut();
            let root_metadata;
            if kind <= 1 {
                source.as_mut_ptr().cast::<*mut Header>().write(first);
                root_metadata = snapshot(
                    run,
                    Origin::Inline,
                    0,
                    source.as_ptr().cast(),
                    8,
                    &[first_metadata],
                );
            } else {
                let record_id = if kind == 3 {
                    let LayoutKind::Box { payload } = table.layouts[root_id.0 as usize].kind else {
                        panic!("box")
                    };
                    payload
                } else {
                    root_id
                };
                let record = &table.layouts[record_id.0 as usize];
                let fields = &table.layouts[record.fields[1].layout.0 as usize].fields;
                let (other, other_metadata) = second.as_ref().unwrap();
                let mut record_bytes = [0u64; 3];
                let at = record_bytes.as_mut_ptr().cast::<u8>();
                at.add(record.fields[0].offset as usize)
                    .cast::<u64>()
                    .write(9);
                let pair = at.add(record.fields[1].offset as usize);
                pair.add(fields[0].offset as usize)
                    .cast::<*mut Header>()
                    .write(first);
                pair.add(fields[1].offset as usize)
                    .cast::<*mut Header>()
                    .write(*other);
                if kind == 3 {
                    let extent = record.size.storage().unwrap() as usize;
                    adopted = region::hirsel_rt_v2_arena_alloc(
                        ctx,
                        base,
                        record_id.0 + 2,
                        extent as u64,
                        8,
                        0,
                    );
                    ptr::copy_nonoverlapping(at, payload(adopted), extent);
                    let retained = snapshot(
                        run,
                        Origin::Arena(journal.grant()),
                        24,
                        payload(adopted),
                        extent,
                        &[first_metadata, other_metadata.clone()],
                    );
                    journal
                        .allocate_typed(
                            Update {
                                value: retained.clone(),
                                additions: vec![first, *other],
                            },
                            record_id.0 + 2,
                            Some(adopted),
                            extent,
                        )
                        .ok()
                        .unwrap();
                    source.as_mut_ptr().cast::<*mut Header>().write(adopted);
                    root_metadata = snapshot(
                        run,
                        Origin::Inline,
                        0,
                        source.as_ptr().cast(),
                        8,
                        &[retained],
                    );
                } else {
                    source = record_bytes;
                    root_metadata = snapshot(
                        run,
                        Origin::Inline,
                        0,
                        source.as_ptr().cast(),
                        24,
                        &[first_metadata, other_metadata.clone()],
                    );
                }
            }
            let remaining = (*base).remaining;
            let inventory = (*guard.as_mut_ptr()).inventory;
            let original_source = source;
            let original_record = if adopted.is_null() {
                None
            } else {
                Some(payload(adopted).cast::<[u64; 3]>().read())
            };
            // Enumerate the events actually reached by this exact native plan.
            let mut events = vec![];
            assert_eq!(
                prepare(
                    ctx,
                    guard.as_mut_ptr(),
                    source.as_ptr().cast(),
                    root_id.0 + 2,
                    root_metadata.as_raw(),
                    &mut |event| {
                        events.push(event);
                        if event == Event::Ready {
                            Err(SealError::Interrupted)
                        } else {
                            Ok(())
                        }
                    }
                ),
                Err(Error::Seal(journal::SealError::Transaction(
                    SealError::Interrupted
                )))
            );
            assert!(events.len() >= 5);
            for index in 0..events.len() {
                let mut reached = 0;
                assert_eq!(
                    prepare(
                        ctx,
                        guard.as_mut_ptr(),
                        source.as_ptr().cast(),
                        root_id.0 + 2,
                        root_metadata.as_raw(),
                        &mut |_| {
                            let fail = reached == index;
                            reached += 1;
                            if fail {
                                Err(SealError::Interrupted)
                            } else {
                                Ok(())
                            }
                        }
                    ),
                    Err(Error::Seal(journal::SealError::Transaction(
                        SealError::Interrupted
                    )))
                );
                assert_eq!(reached, index + 1);
                assert_eq!((*guard.as_mut_ptr()).inventory, inventory);
                assert!((*guard.as_mut_ptr()).sealed_owner.is_null());
                assert_eq!((*base).remaining, remaining);
                assert_eq!(source, original_source);
                assert_eq!((*first).count, 2);
                if let Some((other, _)) = &second {
                    assert_eq!((**other).count, 2);
                }
                if let Some(original) = original_record {
                    assert_eq!(payload(adopted).cast::<[u64; 3]>().read(), original);
                }
                indices += 1;
            }
            let mut owner = ptr::null_mut();
            assert_eq!(
                hirsel_rt_v2_seal_prepare_commit(
                    ctx,
                    raw,
                    source.as_ptr().cast(),
                    root_id.0 + 2,
                    root_metadata.as_raw(),
                    91,
                    &mut owner
                ),
                STATUS_OK
            );
            assert_eq!((*first).count, 1);
            assert_eq!(hirsel_rt_v2_receipt_region_exit(ctx, raw, 0, 0), STATUS_OK);
            // Erase stack inputs before borrowing the published root twice.
            source.fill(0);
            std::hint::black_box(&source);
            for _ in 0..2 {
                let mut at = hirsel_rt_v2_sealed_root(ctx, owner);
                if kind == 3 {
                    assert_eq!(at.cast::<*mut Header>().read(), adopted);
                    at = payload(adopted);
                }
                let sum_list = |block: *mut Header| -> u64 {
                    let len = payload(block).cast::<u64>().read() as usize;
                    std::slice::from_raw_parts(payload(block).add(16).cast::<u64>(), len)
                        .iter()
                        .sum()
                };
                let sum = if kind <= 1 {
                    let moved = at.cast::<*mut Header>().read();
                    assert_ne!(moved, first);
                    sum_list(moved)
                } else {
                    // The compiler lays this fixed graph out as 9, first, second.
                    at.cast::<u64>().read()
                        + sum_list(at.add(8).cast::<*mut Header>().read())
                        + sum_list(at.add(16).cast::<*mut Header>().read())
                };
                assert_eq!(
                    sum,
                    match kind {
                        0 => 0,
                        1 => 23,
                        _ => 39,
                    }
                );
            }
            hirsel_rt_v2_sealed_release(ctx, owner);
            assert_eq!(
                crate::release::release_block(&mut *ctx, first, ptr::null(), 0, 0),
                STATUS_OK
            );
            if let Some((other, _)) = second {
                assert_eq!(
                    crate::release::release_block(&mut *ctx, other, ptr::null(), 0, 0),
                    STATUS_OK
                );
            }
        });
    }
    assert_eq!(indices, 22);
}
