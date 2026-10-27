//! Original construction snapshots use supplied provenance and ordered child
//! metadata. Descriptor traversal supplies coverage, never allocation origin.
use super::*;
use crate::alloc::{Header, payload};
use crate::sealed_region::planner::descriptors;
use hirsel_core_contract::handle_scan::{VALUE, VALUE_SEQUENCE, WORDS};

type Snapshot = Receipt<Record>;

unsafe fn image<'a>(source: *const u8, size: usize) -> &'a [u8] {
    if size == 0 {
        &[]
    } else {
        // SAFETY: the checked construction supplies initialized live bytes.
        unsafe { std::slice::from_raw_parts(source, size) }
    }
}

pub(super) unsafe fn extent(ctx: *mut TaskCtx, block: *mut Header) -> usize {
    let layout = unsafe { &*(*ctx).descriptor((*block).index()) };
    let (capacity, _) = unsafe { (*ctx).heap_ref().usable(block) };
    let size = if layout.flags & crate::LAYOUT_DYNAMIC == 0 {
        layout.size
    } else {
        let count = unsafe { payload(block).cast::<u64>().read_unaligned() };
        if unsafe { (*block).index() } == 0 {
            count.checked_add(16)
        } else {
            let words =
                unsafe { std::slice::from_raw_parts(layout.scan, layout.scan_words as usize) };
            let value = words
                .chunks_exact(WORDS)
                .find(|op| op[0] == VALUE)
                .unwrap_or_else(|| crate::fault("missing construction value descriptor"));
            if value[1] != VALUE_SEQUENCE {
                crate::fault("unsupported construction extent");
            }
            count
                .checked_mul(value[3])
                .and_then(|n| n.checked_add(value[4]))
        }
        .unwrap_or_else(|| crate::fault("construction extent overflow"))
    };
    if size > capacity || size < layout.size {
        crate::fault("invalid construction extent");
    }
    usize::try_from(size).unwrap_or_else(|_| crate::fault("construction extent overflow"))
}

pub(super) unsafe fn snapshot(
    ctx: *mut TaskCtx,
    index: u32,
    source: *const u8,
    size: usize,
    origin: Origin,
    children: &[Snapshot],
) -> Snapshot {
    let layout = unsafe { descriptors::layout(ctx, index) }
        .unwrap_or_else(|_| crate::fault("invalid construction descriptor"));
    let edges = unsafe { descriptors::edges(ctx, index, source.cast_mut(), size) }
        .unwrap_or_else(|_| crate::fault("invalid construction coverage"));
    if edges.len() != children.len() {
        crate::fault("construction receipt omits an original occurrence");
    }
    for (edge, child) in edges.iter().zip(children) {
        let block = unsafe {
            source
                .add(edge.offset)
                .cast::<*mut Header>()
                .read_unaligned()
        };
        if block.is_null()
            || edge.shared != (child.record().origin() == Origin::Shared)
            || edge
                .payload
                .is_some_and(|id| id != unsafe { (*block).index() })
        {
            crate::fault("construction receipt child differs from its typed holder");
        }
        let actual = if edge.shared {
            block.addr().to_ne_bytes().to_vec()
        } else {
            unsafe { image(payload(block), extent(ctx, block)) }.to_vec()
        };
        if child.bytes() != Some(actual.as_slice()) {
            crate::fault("construction receipt child describes another value");
        }
    }
    let logical = if origin == Origin::Inline || origin == Origin::Shared {
        Charge::Bytes(0)
    } else {
        if layout.logical == crate::LOGICAL_UNSUPPORTED || !layout.logical_count.is_null() {
            crate::fault("unsupported original construction logical size");
        }
        if layout.flags & crate::LAYOUT_LOGICAL_PER_ELEMENT == 0 {
            Charge::Bytes(layout.logical)
        } else {
            let count = unsafe { source.cast::<u64>().read_unaligned() };
            layout
                .logical
                .checked_mul(count)
                .map_or(Charge::Overflow, Charge::Bytes)
        }
    };
    semantic::snapshot(
        unsafe { metadata_run(ctx) },
        origin,
        logical,
        unsafe { image(source, size) },
        layout.align as usize,
        children,
    )
    .unwrap_or_else(|_| crate::fault("invalid construction receipt"))
}

/// Snapshot one independently checked original heap/inline construction or byte
/// literal. The caller retains every input holder and owns one output holder.
/// No ordinary or Shared storage credit is acquired or retired.
///
/// # Safety
/// `ctx`, `index` and initialized `value` describe this successful original
/// construction. `kind` is proven from its source recipe: 0 inline, 1 heap,
/// 2 static byte literal. Each of the `input_count` nonnull handles in `inputs`
/// is a live, same-run, paired inline snapshot in original field order. Plain
/// scalar inputs may be omitted. All source storage stays live for this call.
/// Descriptor extent, logical size and occurrence correspondence have passed C.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_receipt_construct(
    ctx: *mut TaskCtx,
    index: u32,
    value: *const c_void,
    kind: u8,
    inputs: *mut *mut c_void,
    input_count: u64,
) -> *mut c_void {
    if ctx.is_null() || (input_count != 0 && inputs.is_null()) || kind > 2 {
        crate::fault("invalid construction receipt arguments");
    }
    let run = unsafe { metadata_run(ctx) };
    let count = usize::try_from(input_count)
        .ok()
        .filter(|n| *n <= isize::MAX as usize / size_of::<*mut c_void>())
        .unwrap_or_else(|| crate::fault("construction input extent overflow"));
    let handles = if count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(inputs, count) }
    };
    let mut children = Vec::new();
    for &handle in handles {
        let child = ManuallyDrop::new(unsafe { Snapshot::from_raw(handle) });
        if child.run() != run || child.record().origin() != Origin::Inline {
            crate::fault("construction input is not a paired inline receipt");
        }
        children.extend(child.children().map(|view| view.retain()));
    }
    let layout = unsafe { descriptors::layout(ctx, index) }
        .unwrap_or_else(|_| crate::fault("invalid construction root descriptor"));
    if layout.flags & crate::LAYOUT_DYNAMIC != 0 || (layout.size != 0 && value.is_null()) {
        crate::fault("invalid construction root image");
    }
    if kind != 0 {
        let edges = unsafe {
            descriptors::edges(ctx, index, value.cast_mut().cast(), layout.size as usize)
        }
        .unwrap_or_else(|_| crate::fault("invalid construction root coverage"));
        let [edge] = edges.as_slice() else {
            crate::fault("construction requires one original block")
        };
        if edge.offset != 0
            || edge.shared
            || (kind == 1 && edge.slice)
            || (kind == 2 && !children.is_empty())
        {
            crate::fault("invalid original construction boundary");
        }
        let block = unsafe { value.cast::<*mut Header>().read_unaligned() };
        if block.is_null() {
            crate::fault("null construction block");
        }
        let origin = if kind == 1 {
            Origin::Heap
        } else {
            Origin::Static
        };
        let node = unsafe {
            snapshot(
                ctx,
                (*block).index(),
                payload(block),
                extent(ctx, block),
                origin,
                &children,
            )
        };
        children = vec![node];
    }
    unsafe {
        snapshot(
            ctx,
            index,
            value.cast(),
            layout.size as usize,
            Origin::Inline,
            &children,
        )
    }
    .into_raw()
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests {
    use super::*;
    use hirsel_layout::layout::{LayoutBuilder, Owners};
    use hirsel_lir::layout_types::{LayoutKind, TargetSpec};
    use hirsel_types::{Primitive, testing::Types};

    #[test]
    fn native_construct_96_preserves_repeated_occurrences_and_heap_aliases_through_seal() {
        let target = TargetSpec::executed().into_iter().next().unwrap();
        let mut types = Types::default();
        let integer = types.p(Primitive::U64);
        let list = types.list(integer);
        let pair = types.record(&[list, list]);
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let list_id = builder.layout(&types.store, list).unwrap();
        let pair_id = builder.layout(&types.store, pair).unwrap();
        let table = builder.finish();
        let LayoutKind::Block { payload: sequence } = table.layouts[list_id.0 as usize].kind else {
            panic!("list")
        };
        crate::sealed_region::planner::tests::compiler::native(&table, |ctx| unsafe {
            for values in [&[][..], &[11u64, 12][..]] {
                let mut guard = MaybeUninit::<ReceiptArenaGuard<'static>>::uninit();
                let raw = guard.as_mut_ptr().cast();
                hirsel_rt_v2_receipt_region_enter(ctx, raw);
                let block = crate::alloc::hirsel_rt_v2_alloc(
                    ctx,
                    sequence.0 + 2,
                    16 + values.len() as u64 * 8,
                    8,
                    crate::DOMAIN_HEAP,
                    0,
                );
                payload(block).cast::<u64>().write(values.len() as u64);
                payload(block)
                    .add(8)
                    .cast::<u64>()
                    .write(values.len() as u64);
                ptr::copy_nonoverlapping(
                    values.as_ptr(),
                    payload(block).add(16).cast(),
                    values.len(),
                );
                let leaf = Snapshot::from_raw(hirsel_rt_v2_receipt_construct(
                    ctx,
                    list_id.0 + 2,
                    ptr::from_ref(&block).cast(),
                    1,
                    ptr::null_mut(),
                    0,
                ));
                // The two original fields own separate source credits. Metadata
                // construction must not add a third language holder.
                crate::count::hirsel_rt_v2_dup(ctx, block);
                let mut inputs = [leaf.as_raw(), leaf.as_raw()];
                let source = [block, block];
                let root = Snapshot::from_raw(hirsel_rt_v2_receipt_construct(
                    ctx,
                    pair_id.0 + 2,
                    source.as_ptr().cast(),
                    0,
                    inputs.as_mut_ptr(),
                    2,
                ));
                assert_eq!(
                    root.record().heap_charge(),
                    Charge::Bytes(values.len() as u64 * 16)
                );
                assert_eq!(root.children().len(), 2);
                let leaf_nodes: Vec<_> = root.children().collect();
                assert_eq!(leaf_nodes[0].generation(), leaf_nodes[1].generation());
                // One external alias must survive the transaction and release.
                crate::count::hirsel_rt_v2_dup(ctx, block);
                let mut owner = ptr::null_mut();
                assert_eq!(
                    hirsel_rt_v2_seal_prepare_commit(
                        ctx,
                        raw,
                        source.as_ptr().cast(),
                        pair_id.0 + 2,
                        root.as_raw(),
                        0,
                        &mut owner
                    ),
                    crate::STATUS_OK
                );
                assert!(!owner.is_null());
                assert_eq!(
                    hirsel_rt_v2_receipt_region_exit(ctx, raw, 0, 0),
                    crate::STATUS_OK
                );
                for _ in 0..2 {
                    let sealed = crate::sealed_region::hirsel_rt_v2_sealed_root(ctx, owner)
                        .cast::<*mut Header>();
                    let first = sealed.read();
                    assert_eq!(first, sealed.add(1).read());
                    assert_ne!(first, block);
                    let sum: u64 = std::slice::from_raw_parts(
                        payload(first).add(16).cast::<u64>(),
                        values.len(),
                    )
                    .iter()
                    .sum();
                    assert_eq!(sum, if values.is_empty() { 0 } else { 23 });
                }
                crate::sealed_region::hirsel_rt_v2_sealed_release(ctx, owner);
                assert_eq!(
                    std::slice::from_raw_parts(payload(block).add(16).cast::<u64>(), values.len()),
                    values
                );
                crate::release::hirsel_rt_v2_release(ctx, block, sequence.0 + 2, ptr::null(), 0, 0);
            }
        });
    }
}
