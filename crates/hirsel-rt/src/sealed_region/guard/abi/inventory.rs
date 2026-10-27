//! Publish typed original arena allocations whose payload owns no child holder.
//! Heap-bearing payloads need their explicit paired operand receipts first.
use super::*;
use crate::alloc::{Header, payload};
use crate::sealed_region::planner::descriptors;

/// Retain a successful original arena allocation, including an otherwise dead
/// allocation. Its source recipe establishes arena origin. Descriptor coverage
/// must independently establish the absence of outgoing ordinary/Shared credits.
///
/// # Safety
/// `ctx` is current and `raw` its exact open demanded guard. The initialized
/// `value` is the root holder at descriptor `index` for this one successful
/// original arena construction. C proves its source allocation correspondence,
/// that the block is fresh semantic storage, and no heap-bearing payload or
/// resource is hidden by optimization. The source block stays guard-owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_receipt_arena_inventory(
    ctx: *mut TaskCtx,
    raw: *mut c_void,
    index: u32,
    value: *const c_void,
) {
    if ctx.is_null() || value.is_null() {
        crate::fault("null arena inventory construction");
    }
    let record = guard(raw);
    unsafe { &*ctx }
        .run_ref()
        .sealed_owners
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .check_guard(raw.addr(), ctx.addr());
    let root = unsafe { descriptors::layout(ctx, index) }
        .unwrap_or_else(|_| crate::fault("invalid arena inventory root descriptor"));
    if root.size != 8 || root.flags & crate::LAYOUT_DYNAMIC != 0 {
        crate::fault("arena inventory requires one original block");
    }
    let edges = unsafe { descriptors::edges(ctx, index, value.cast_mut().cast(), 8) }
        .unwrap_or_else(|_| crate::fault("invalid arena inventory root coverage"));
    let [edge] = edges.as_slice() else {
        crate::fault("arena inventory requires one original block")
    };
    let block = unsafe { value.cast::<*mut Header>().read_unaligned() };
    if block.is_null()
        || edge.offset != 0
        || edge.shared
        || edge.slice
        || edge.payload != Some(unsafe { (*block).index() })
    {
        crate::fault("arena inventory differs from its typed construction");
    }
    let extent = unsafe { super::construct::extent(ctx, block) };
    let base = unsafe { ptr::addr_of_mut!((*record).base) };
    if !unsafe { region::contains_storage(base, block, extent) } {
        crate::fault("arena inventory block belongs to another destination");
    }
    let index = unsafe { (*block).index() };
    let outgoing = unsafe { descriptors::edges(ctx, index, payload(block), extent) }
        .unwrap_or_else(|_| crate::fault("invalid arena inventory payload coverage"));
    if !outgoing.is_empty() {
        crate::fault("arena inventory payload needs paired original receipts");
    }
    let journal = unsafe { (*record).inventory.as_mut() }
        .unwrap_or_else(|| crate::fault("consumed arena inventory"));
    let receipt = unsafe {
        super::construct::snapshot(
            ctx,
            index,
            payload(block),
            extent,
            Origin::Arena(journal.grant()),
            &[],
        )
    };
    unsafe {
        journal.allocate_typed(
            journal::Update {
                value: receipt,
                additions: vec![],
            },
            index,
            Some(block),
            extent,
        )
    }
    .ok()
    .unwrap_or_else(|| crate::fault("invalid arena inventory publication"));
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests {
    use super::*;
    use hirsel_layout::layout::{LayoutBuilder, Owners};
    use hirsel_lir::layout_types::{LayoutKind, TargetSpec};
    use hirsel_types::{Primitive, testing::Types};

    #[test]
    fn native_inventory_96_retains_dead_scalar_list_through_seal_and_scope_exit() {
        let target = TargetSpec::executed().into_iter().next().unwrap();
        let mut types = Types::default();
        let integer = types.p(Primitive::U64);
        let list = types.list(integer);
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let list_id = builder.layout(&types.store, list).unwrap();
        let scalar_id = builder.layout(&types.store, integer).unwrap();
        let table = builder.finish();
        let LayoutKind::Block { payload: sequence } = table.layouts[list_id.0 as usize].kind else {
            panic!("list")
        };
        crate::sealed_region::planner::tests::compiler::native(&table, |ctx| unsafe {
            let mut guard = MaybeUninit::<ReceiptArenaGuard<'static>>::uninit();
            let raw = guard.as_mut_ptr().cast();
            hirsel_rt_v2_receipt_region_enter(ctx, raw);
            let base = ptr::addr_of_mut!((*guard.as_mut_ptr()).base);
            let block = region::hirsel_rt_v2_arena_alloc(ctx, base, sequence.0 + 2, 32, 8, 0);
            payload(block).cast::<[u64; 4]>().write([2, 2, 11, 12]);
            hirsel_rt_v2_receipt_arena_inventory(
                ctx,
                raw,
                list_id.0 + 2,
                ptr::from_ref(&block).cast(),
            );
            let journal = &*(*guard.as_mut_ptr()).inventory;
            assert_eq!(journal.inventory().entries().len(), 1);
            let (id, retained) = journal.inventory().entries().next().unwrap();
            assert_eq!(retained.record().origin(), Origin::Arena(journal.grant()));
            assert_eq!(retained.record().outgoing_heap_charge(), Charge::Bytes(0));
            assert!(journal.holders(id).unwrap().is_empty());
            let root = 23u64;
            let receipt = Receipt::<Record>::from_raw(hirsel_rt_v2_receipt_bits(
                ctx,
                scalar_id.0 + 2,
                ptr::from_ref(&root).cast(),
            ));
            let mut owner = ptr::null_mut();
            assert_eq!(
                hirsel_rt_v2_seal_prepare_commit(
                    ctx,
                    raw,
                    ptr::from_ref(&root).cast(),
                    scalar_id.0 + 2,
                    receipt.as_raw(),
                    0,
                    &mut owner
                ),
                crate::STATUS_OK
            );
            assert_eq!(
                hirsel_rt_v2_receipt_region_exit(ctx, raw, 0, 0),
                crate::STATUS_OK
            );
            assert_eq!(*payload(block).cast::<[u64; 4]>(), [2, 2, 11, 12]);
            assert_eq!(
                crate::sealed_region::hirsel_rt_v2_sealed_root(ctx, owner)
                    .cast::<u64>()
                    .read(),
                23
            );
            crate::sealed_region::hirsel_rt_v2_sealed_release(ctx, owner);
        });
    }
}
