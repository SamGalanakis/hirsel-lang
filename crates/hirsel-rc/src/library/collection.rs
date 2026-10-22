//! Ownership-moving List primitives. Every bounds check precedes mutation.
use super::LibraryBody;
use super::builder::{Body, Cx};
use crate::PassError;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::runtime_abi::EXTENT_MAX;
use hirsel_lir::stage::Value;

pub static TRUNCATE: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("list.truncate"),
    build: super::list::truncate,
};
pub static POP: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("list.pop"),
    build: pop,
};
pub static REMOVE: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("list.remove"),
    build: remove,
};
pub static SWAP: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("list.swap"),
    build: swap,
};
pub static RESERVE: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("list.reserve"),
    build: reserve,
};

fn length(b: &mut Body<'_, '_>, address: AddressId) -> Result<ValueId, PassError> {
    let (view, loan) = b.borrow(address, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    Ok(len)
}
fn check(b: &mut Body<'_, '_>, index: ValueId, len: ValueId) -> Result<(), PassError> {
    let condition = b.compare(BinaryOp::Lt, index, len)?;
    let failure = b.failure(Some("MEM-14"));
    b.push(Inst::Check { condition, failure });
    Ok(())
}
fn ret_unit(b: &mut Body<'_, '_>) {
    let result =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(result);
}
// Both addresses are prepared before either move. No failing instruction may
// run while an element is outside the sequence; intermediate states have no holes.
fn exchange(
    b: &mut Body<'_, '_>,
    list: AddressId,
    a: ValueId,
    c: ValueId,
) -> Result<(), PassError> {
    let left = b.slot_place(list, a)?;
    let right = b.slot_place(list, c)?;
    let x = b.take(left)?;
    let y = b.take(right)?;
    b.store(left, y)?;
    b.store(right, x)?;
    Ok(())
}
// Rust alloc/src/raw_vec/mod.rs:24-30, 935-941: capacity
// overflow is checked before calling the allocator. Hirsel reports SEM-1.
fn reserve(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let capacity = b.read_bits(b.parameters[1].address)?;
    let element = match b
        .cx
        .payload(b.address_layout(list.address))
        .map(|p| &b.cx.record(p).kind)
    {
        Some(
            LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element }
            | LayoutKind::SoaColumns { element },
        ) => *element,
        _ => {
            return b
                .cx
                .unsupported("list.reserve of a place that holds no sequence block");
        }
    };
    // The logical count limit is shared with the interpreter. Also bound the
    // physical minimum stride: padding and flat encodings can make it larger.
    // 64 bytes covers sequence metadata and its first element's alignment.
    let ty = b.cx.type_actual(0)?;
    let resolve = |definition| {
        use hirsel_layout::layout::OwnerRepresentation;
        use hirsel_type_analysis::logical::LogicalNominal;
        b.cx.program
            .owners
            .nominals
            .get(&definition)
            .map(|owner| match owner {
                OwnerRepresentation::Defined(record) => LogicalNominal::Representation {
                    shape: record.private_shape,
                },
                _ => LogicalNominal::Opaque,
            })
    };
    let logical = hirsel_type_analysis::logical::logical_size(b.cx.types, ty, &resolve)
        .map_err(|e| PassError::Malformed(format!("list.reserve logical element size: {e:?}")))?;
    let physical = hirsel_lir::layout::least_size(&b.cx.record(element).size);
    let first = hirsel_lir::layout::sequence_elements_offset(b.cx.record(element))
        .map_err(|e| PassError::Layout(b.cx.path.clone(), e))?;
    let logical_max = if logical == 0 {
        u64::MAX
    } else {
        (EXTENT_MAX - 64) / logical
    };
    let physical_max = if physical == 0 {
        u64::MAX
    } else {
        (EXTENT_MAX - first.max(64)) / physical
    };
    let maximum = logical_max.min(physical_max);
    if maximum != u64::MAX {
        let most = b.u64(maximum)?;
        let condition = b.compare(BinaryOp::Le, capacity, most)?;
        let failure = b.failure(Some("SEM-1"));
        b.push(Inst::Check { condition, failure });
    }
    b.seq_reserve(list.address, capacity)?;
    ret_unit(&mut b);
    Ok(b.finish())
}
fn swap(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let a = b.read_bits(b.parameters[1].address)?;
    let c = b.read_bits(b.parameters[2].address)?;
    let len = length(&mut b, list.address)?;
    check(&mut b, a, len)?;
    check(&mut b, c, len)?;
    let equal = b.compare(BinaryOp::Eq, a, c)?;
    let done = b.block(BlockKind::Body, &[]);
    let move_elements = b.block(BlockKind::Body, &[]);
    b.branch(equal, done, move_elements);
    b.at(move_elements);
    exchange(&mut b, list.address, a, c)?;
    ret_unit(&mut b);
    b.at(done);
    ret_unit(&mut b);
    Ok(b.finish())
}
fn option(b: &mut Body<'_, '_>, value: Option<ValueId>) -> ValueId {
    let layout = b.cx.fn_abi().result.unwrap().layout;
    let result = b.fresh(layout);
    let definition = b.symbol(Symbol {
        kind: SymbolKind::Type,
        definition: hirsel_syntax::DefPath::core("Option"),
        instance: None,
    });
    let fields = PoolRange::push(
        &mut b.ed.unit.pools.fields,
        value.map(|value| FieldValue { field: 0, value }),
    );
    let construct = ConstructId(b.ed.unit.tables.constructs.len() as u32);
    b.ed.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor: ConstructorKind::Variant {
            definition,
            variant: u32::from(value.is_some()),
        },
        layout,
        fields,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: b.anchor,
    });
    let failure = b.failure(None);
    b.push(Inst::Construct {
        result,
        construct,
        failure,
    });
    result
}
fn pop(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let len = length(&mut b, list.address)?;
    let zero = b.u64(0)?;
    let empty = b.compare(BinaryOp::Eq, len, zero)?;
    let none = b.block(BlockKind::Body, &[]);
    let some = b.block(BlockKind::Body, &[]);
    b.branch(empty, none, some);
    b.at(none);
    let result = option(&mut b, None);
    b.ret(Some(result));
    b.at(some);
    let minus_one = b.u64(u64::MAX)?;
    let last = b.add(ArithMode::Wrapping, len, minus_one);
    let address = b.slot_place(list.address, last)?;
    let value = b.take(address)?;
    b.seq_set_len(list.address, last)?;
    let result = option(&mut b, Some(value));
    b.ret(Some(result));
    Ok(b.finish())
}
fn remove(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let index = b.read_bits(b.parameters[1].address)?;
    let len = length(&mut b, list.address)?;
    check(&mut b, index, len)?;
    let minus_one = b.u64(u64::MAX)?;
    let last = b.add(ArithMode::Wrapping, len, minus_one);
    let layout = b.u64_layout()?;
    let position = b.value(layout, Holding::Plain);
    let version = b.version(list.slot);
    let head = b.block(BlockKind::Body, &[position, version]);
    let step = b.block(BlockKind::Body, &[]);
    let end = b.block(BlockKind::Body, &[]);
    let incoming = b.reaching(list.slot)?;
    b.jump(head, &[index, incoming]);
    b.at(head);
    b.set_reaching(list.slot, version);
    let more = b.compare(BinaryOp::Lt, position, last)?;
    b.branch(more, step, end);
    b.at(step);
    let one = b.u64(1)?;
    let next = b.add(ArithMode::Wrapping, position, one);
    exchange(&mut b, list.address, position, next)?;
    let updated = b.reaching(list.slot)?;
    b.jump(head, &[next, updated]);
    b.at(end);
    b.set_reaching(list.slot, version);
    let address = b.slot_place(list.address, last)?;
    let value = b.take(address)?;
    b.seq_set_len(list.address, last)?;
    b.ret(Some(value));
    Ok(b.finish())
}

// Vec::truncate's tail walk. As in list::cursor_drop, explicit cleanup
// continues the walk after a failed finalizer before propagating to the
// caller. The caller still owns the retained prefix and the allocation.
pub(super) fn truncate(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let first = b.read_bits(b.parameters[1].address)?;
    let len = length(&mut b, list.address)?;
    let shorter = b.compare(BinaryOp::Lt, first, len)?;
    let drain = b.block(BlockKind::Body, &[]);
    let noop = b.block(BlockKind::Body, &[]);
    b.branch(shorter, drain, noop);
    b.at(noop);
    ret_unit(&mut b);
    b.at(drain);
    // A distinct preparation delays copy-on-write until after the no-op
    // branch; the entry address is read-only, as in list.push.
    let writable = b.place(AddressBase::Slot(list.slot), &[], list.layout, None)?;
    if b.cx
        .payload(list.layout)
        .is_some_and(|payload| matches!(b.cx.record(payload).kind, LayoutKind::Sequential { .. }))
    {
        return b.cx.unsupported("list.truncate requires slot-addressable elements; sequential Packed lists are walk-only (MEM-13)");
    }
    let element = b.cx.element(list.layout)?;
    let layout = b.u64_layout()?;
    let index = b.value(layout, Holding::Plain);
    let normal_version = b.version(list.slot);
    let normal = b.block(BlockKind::Body, &[index, normal_version]);
    let failure_index = b.value(layout, Holding::Plain);
    let failure_version = b.version(list.slot);
    let failed = b.block(BlockKind::FailureCleanup, &[failure_index, failure_version]);
    let before = b.reaching(list.slot)?;
    b.jump(normal, &[first, before]);
    let local = b.slot(element, SlotKind::Temporary);
    let local_address = b.place(AddressBase::Slot(local), &[], element, None)?;
    let anchor = CleanupAnchorId(b.ed.unit.tables.cleanup_anchors.len() as u32);
    b.ed.unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site: SiteRange {
            source: 0,
            start: 0,
            end: 0,
        },
    });
    for (head, index, version, failing) in [
        (normal, index, normal_version, false),
        (failed, failure_index, failure_version, true),
    ] {
        b.at(head);
        b.set_reaching(list.slot, version);
        let more = b.compare(BinaryOp::Lt, index, len)?;
        let kind = if failing {
            BlockKind::FailureCleanup
        } else {
            BlockKind::Body
        };
        let step = b.block(kind, &[]);
        let end = b.block(kind, &[]);
        b.branch(more, step, end);
        b.at(step);
        let one = b.u64(1)?;
        let next = b.add(ArithMode::Wrapping, index, one);
        let at = b.slot_place(writable, index)?;
        let value = b.take(at)?;
        b.push(Inst::StorageLive { slot: local });
        b.store(local_address, value)?;
        let remaining = b.value(layout, Holding::Plain);
        let retired_version = b.version(list.slot);
        let retire = b.block(BlockKind::FailureCleanup, &[remaining, retired_version]);
        let updated = b.reaching(list.slot)?;
        let failure = b.failure_to(retire, &[next, updated], None);
        b.push(Inst::Drop {
            address: local_address,
            if_live: false,
            anchor,
            failure,
        });
        b.push(Inst::StorageDead {
            slot: local,
            if_live: false,
        });
        b.jump(head, &[next, updated]);
        b.at(retire);
        b.set_reaching(list.slot, retired_version);
        b.push(Inst::StorageDead {
            slot: local,
            if_live: false,
        });
        b.jump(failed, &[remaining, retired_version]);
        b.at(end);
        b.set_reaching(list.slot, version);
        b.seq_set_len(writable, first)?;
        if failing {
            b.terminate(Terminator::Propagate);
        } else {
            ret_unit(&mut b);
        }
    }
    Ok(b.finish())
}
