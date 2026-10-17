//! Growing lists and consuming sequences, built as ordinary monomorphic LIR.
use super::builder::{Body, Cx};
use super::{CallShape, InlineLowering, LibraryBody, LibraryError, R, Request, native_path};
use crate::import::ImportCx;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_types::{TypeKind, UseKind};

pub static PUSH: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("list.push"),
    build: push,
};
/// `list.push` where `spare_push` proved the count below the capacity.
pub static PUSH_SPARE: LibraryBody = LibraryBody {
    path: crate::spare_push::spare_path,
    build: push_spare,
};
pub static GROW: LibraryBody = LibraryBody {
    path: || native_path("list_grow"),
    build: grow,
};
pub static FORGET: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("forget"),
    lower: forget,
};

fn forget(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let [argument] = call.arguments else {
        return Ok(false);
    };
    if argument.value.usage != UseKind::Take {
        return Ok(false);
    }
    let value = cx.operand(&argument.value)?;
    cx.push(Inst::Forget { value });
    cx.consumed(value);
    let layout = cx.place_layout(call.destination)?;
    let result = cx.constant(layout, ConstantValue::Unit);
    cx.finish(call, result)?;
    Ok(true)
}

fn ret_unit(b: &mut Body<'_, '_>) {
    let result =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(result);
}

fn push(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    if sequential(cx) {
        return push_sequential(cx);
    }
    let mut callable = cx.store.get(cx.callable).unwrap().clone();
    let TypeKind::Function(f) = &mut callable.kind else {
        unreachable!()
    };
    f.parameters = f.parameters[..1].to_vec().into();
    let callable = cx.store.push(callable);
    let path = native_path("list_grow");
    let grow = cx.require(Request::native(
        super::body_index(&path).unwrap(),
        &path,
        cx.store.clone(),
        cx.arguments.clone(),
        callable,
    ))?;
    let mut b = Body::new(cx)?;
    let [list, value] = b.parameters[..] else {
        return Err(LibraryError::Malformed("list.push parameters".into()));
    };
    let (view, loan) = b.borrow(list.address, false)?;
    let len = b.seq_len(view)?;
    let capacity = b.seq_capacity(view)?;
    b.end_loan(loan);
    let full = b.compare(BinaryOp::Ge, len, capacity)?;
    let slow = b.block(BlockKind::Body, &[]);
    let joined = b.version(list.slot);
    let append = b.block(BlockKind::Body, &[joined]);
    let before = b.reaching(list.slot)?;
    let yes = b.edge(slow, &[]);
    let no = b.edge(append, &[before]);
    b.terminate(Terminator::Branch {
        condition: full,
        yes,
        no,
    });
    b.at(slow);
    let symbol = b.symbol(grow.symbol);
    let version = b.version(list.slot);
    let arguments = PoolRange::push(
        &mut b.ed.unit.pools.call_arguments,
        [CallArgument {
            parameter: 0,
            passing: Passing::Mut,
            value: ArgumentValue::Address {
                address: list.address,
                version,
            },
        }],
    );
    let call = CallId(b.ed.unit.tables.calls.len() as u32);
    b.ed.unit.tables.calls.push(CallSite {
        operation_arguments: Vec::new(),
        callee: Callee::Direct(symbol),
        abi: grow.abi,
        arguments,
        result: None,
        destination: None,
        declared: crate::import::CALL_EFFECTS,
        anchor: b.anchor,
    });
    let normal = b.edge(append, &[version]);
    let failure = b.failure(None);
    b.terminate(Terminator::Call {
        call,
        normal: Some(normal),
        failure,
    });
    b.at(append);
    b.set_reaching(list.slot, joined);
    // Prepare the append's write address after growth. P4 makes this place
    // unique here; preparing it at entry would detach the static empty block
    // before list_grow can allocate its first capacity in one step.
    let writable = b.place(AddressBase::Slot(list.slot), &[], list.layout, None)?;
    let slot = b.slot_place(writable, len)?;
    let moved = b.take(value.address)?;
    b.store(slot, moved)?;
    let one = b.u64(1)?;
    let count = b.add(ArithMode::Wrapping, len, one);
    b.seq_set_len(writable, count)?;
    ret_unit(&mut b);
    Ok(b.finish())
}

/// The append of `list.push` without its capacity read and growth branch:
/// `spare_push` proved the count below the capacity at every call (FACT-3).
/// P4 still detaches a shared list where the write address is prepared, and
/// a detach keeps the capacity.
fn push_spare(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let mut b = Body::new(cx)?;
    let [list, value] = b.parameters[..] else {
        return Err(LibraryError::Malformed("list.push parameters".into()));
    };
    let (view, loan) = b.borrow(list.address, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    let writable = b.place(AddressBase::Slot(list.slot), &[], list.layout, None)?;
    let slot = b.slot_place(writable, len)?;
    let moved = b.take(value.address)?;
    b.store(slot, moved)?;
    let one = b.u64(1)?;
    let count = b.add(ArithMode::Wrapping, len, one);
    b.seq_set_len(writable, count)?;
    ret_unit(&mut b);
    Ok(b.finish())
}

/// Whether this list instance holds a sequential Packed sequence (MEM-13).
fn sequential(cx: &Cx<'_>) -> bool {
    let list = cx.fn_abi().parameters[0].layout;
    cx.payload(list).is_some_and(|p| {
        matches!(
            cx.record(p).kind,
            hirsel_lir::layout_types::LayoutKind::Sequential { .. }
        )
    })
}

/// `list.push` of a sequential Packed sequence: the element's extent past the
/// used bytes, the storage growing as needed (LIR schema revision 19).
fn push_sequential(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let mut b = Body::new(cx)?;
    let [list, value] = b.parameters[..] else {
        return Err(LibraryError::Malformed("list.push parameters".into()));
    };
    let moved = b.take(value.address)?;
    b.seq_append(list.address, moved)?;
    ret_unit(&mut b);
    Ok(b.finish())
}

fn grow(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let (view, loan) = b.borrow(list.address, false)?;
    let capacity = b.seq_capacity(view)?;
    b.end_loan(loan);
    let twice = b.add(ArithMode::Saturating, capacity, capacity);
    let four = b.u64(4)?;
    let small = b.compare(BinaryOp::Lt, twice, four)?;
    let minimum = b.block(BlockKind::Body, &[]);
    let doubled = b.block(BlockKind::Body, &[]);
    let layout = b.u64_layout()?;
    let wanted = b.value(layout, Holding::Plain);
    let reserve = b.block(BlockKind::Body, &[wanted]);
    b.branch(small, minimum, doubled);
    b.at(minimum);
    b.jump(reserve, &[four]);
    b.at(doubled);
    b.jump(reserve, &[twice]);
    b.at(reserve);
    b.seq_reserve(list.address, wanted)?;
    ret_unit(&mut b);
    Ok(b.finish())
}
pub static LIST_OPEN: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.ListConsume_open"),
    build: open,
};
pub static ARRAY_OPEN: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.ArrayConsume_open"),
    build: open,
};
pub static INLINE_OPEN: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.InlineConsume_open"),
    build: open,
};
pub static LIST_NEXT: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.ListConsume_next"),
    build: next,
};
pub static ARRAY_NEXT: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.ArrayConsume_next"),
    build: next,
};
pub static INLINE_NEXT: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.InlineConsume_next"),
    build: next,
};
pub static CURSOR_DROP: LibraryBody = LibraryBody {
    path: || native_path("owned_cursor_drop"),
    build: cursor_drop,
};

const UNIQUE: u32 = 2;

fn field(
    b: &mut Body<'_, '_>,
    root: AddressId,
    layout: hirsel_lir::layout_types::LayoutId,
    ordinal: u32,
) -> Result<AddressId, LibraryError> {
    let field = b
        .record(layout)
        .fields
        .iter()
        .find(|f| f.ordinal == ordinal)
        .ok_or_else(|| LibraryError::Malformed("cursor field".into()))?
        .layout;
    b.place(
        AddressBase::Address(root),
        &[ProjectionStep::Field(ordinal)],
        field,
        None,
    )
}

fn construct(
    b: &mut Body<'_, '_>,
    layout: hirsel_lir::layout_types::LayoutId,
    constructor: ConstructorKind,
    values: &[ValueId],
) -> ValueId {
    let result = b.fresh(layout);
    let fields = PoolRange::push(
        &mut b.ed.unit.pools.fields,
        values.iter().enumerate().map(|(i, &value)| FieldValue {
            field: i as u32,
            value,
        }),
    );
    let construct = ConstructId(b.ed.unit.tables.constructs.len() as u32);
    let payload = b.cx.payload(layout).unwrap_or(layout);
    b.ed.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        constructor,
        layout: payload,
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

fn record_symbol(b: &mut Body<'_, '_>, path: hirsel_syntax::DefPath) -> SymbolId {
    b.symbol(Symbol {
        kind: SymbolKind::Type,
        definition: path,
        instance: None,
    })
}

fn open(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let mut b = Body::new(cx)?;
    let source = b.parameters[0];
    let cursor_layout = b.cx.fn_abi().result.unwrap().layout;
    let array_layout = b.record(cursor_layout).fields[0].layout;
    let bool_layout = b.bool_layout()?;
    let inline = matches!(
        b.record(source.layout).kind,
        hirsel_lir::layout_types::LayoutKind::Inline { .. }
    );
    let (array, unique) = if inline {
        let hirsel_lir::layout_types::LayoutKind::Inline { element, count } =
            b.record(source.layout).kind
        else {
            unreachable!()
        };
        let mut addresses = Vec::new();
        for i in 0..count {
            let index = b.u64(i)?;
            let at = b.place(
                AddressBase::Address(source.address),
                &[ProjectionStep::Index(index)],
                element,
                Some("MEM-14"),
            )?;
            addresses.push(at);
        }
        let elements = addresses
            .into_iter()
            .map(|at| b.take(at))
            .collect::<Result<Vec<_>, _>>()?;
        b.emptied(source.slot);
        (
            construct(&mut b, array_layout, ConstructorKind::Sequence, &elements),
            b.constant(bool_layout, ConstantValue::Bool(true)),
        )
    } else {
        let value = b.take(source.address)?;
        let unique = b.value(bool_layout, Holding::Plain);
        b.push(Inst::IsUnique {
            result: unique,
            value,
        });
        let array = if source.layout == array_layout {
            value
        } else {
            let array = b.fresh(array_layout);
            b.push(Inst::Recast {
                result: array,
                value,
            });
            array
        };
        (array, unique)
    };
    let zero = b.u64(0)?;
    let symbol = record_symbol(
        &mut b,
        hirsel_syntax::DefPath::core("$intrinsic.OwnedCursor"),
    );
    let cursor = construct(
        &mut b,
        cursor_layout,
        ConstructorKind::Record(symbol),
        &[array, zero, unique],
    );
    b.ret(Some(cursor));
    Ok(b.finish())
}

fn next(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let mut b = Body::new(cx)?;
    let cursor = b.parameters[0];
    let source = field(&mut b, cursor.address, cursor.layout, 0)?;
    let position = field(&mut b, cursor.address, cursor.layout, 1)?;
    let unique = field(&mut b, cursor.address, cursor.layout, UNIQUE)?;
    let index = b.read_bits(position)?;
    let (view, loan) = b.borrow(source, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    let done = b.compare(BinaryOp::Eq, index, len)?;
    let exhausted = b.block(BlockKind::Body, &[]);
    let item = b.block(BlockKind::Body, &[]);
    b.branch(done, exhausted, item);
    let option_layout = b.cx.fn_abi().result.unwrap().layout;
    let option_symbol = record_symbol(&mut b, hirsel_syntax::DefPath::core("Option"));
    b.at(exhausted);
    let none = construct(
        &mut b,
        option_layout,
        ConstructorKind::Variant {
            definition: option_symbol,
            variant: 0,
        },
        &[],
    );
    b.ret(Some(none));
    b.at(item);
    let at = b.slot_place(source, index)?;
    let array_layout = b.record(cursor.layout).fields[0].layout;
    let element = b.cx.element(array_layout)?;
    let copy = b.record(element).copy;
    // Prepare and update all state before the infallible element move. A
    // failed state write must never leave a taken element outside cleanup.
    let one = b.u64(1)?;
    let advanced = b.add(ArithMode::Wrapping, index, one);
    replace_position(&mut b, position, advanced)?;
    let picked = if copy == hirsel_lir::layout_types::CopyRecipe::NotCopyable {
        // A noncopyable collection cannot have a shared holder or static data.
        b.take(at)?
    } else {
        let is_unique = b.read_bits(unique)?;
        let moved = b.block(BlockKind::Body, &[]);
        let shared = b.block(BlockKind::Body, &[]);
        let picked = b.fresh(element);
        let join = b.block(BlockKind::Body, &[picked]);
        b.branch(is_unique, moved, shared);
        b.at(moved);
        let value = b.take(at)?;
        b.jump(join, &[value]);
        b.at(shared);
        let failure = b.failure(None);
        let value = b.copy(at, failure)?;
        b.jump(join, &[value]);
        b.at(join);
        picked
    };
    let some = construct(
        &mut b,
        option_layout,
        ConstructorKind::Variant {
            definition: option_symbol,
            variant: 1,
        },
        &[picked],
    );
    b.ret(Some(some));
    Ok(b.finish())
}

fn cursor_drop(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, LibraryError> {
    let mut b = Body::new(cx)?;
    let cursor = b.parameters[0];
    let source = field(&mut b, cursor.address, cursor.layout, 0)?;
    let position = field(&mut b, cursor.address, cursor.layout, 1)?;
    let unique = field(&mut b, cursor.address, cursor.layout, UNIQUE)?;
    let unique = b.read_bits(unique)?;
    let drain = b.block(BlockKind::Body, &[]);
    let shared = b.block(BlockKind::Body, &[]);
    b.branch(unique, drain, shared);
    b.at(shared);
    ret_unit(&mut b);
    b.at(drain);
    let first = b.read_bits(position)?;
    let (view, loan) = b.borrow(source, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    let layout = b.u64_layout()?;
    let tail = b.value(layout, Holding::Plain);
    let normal = b.block(BlockKind::Body, &[tail]);
    let failure_tail = b.value(layout, Holding::Plain);
    let failed = b.block(BlockKind::FailureCleanup, &[failure_tail]);
    let before = b.reaching(cursor.slot)?;
    b.jump(normal, &[len]);
    let element = b.cx.element(b.record(cursor.layout).fields[0].layout)?;
    let local = b.slot(element, SlotKind::Temporary);
    let local_address = b.place(AddressBase::Slot(local), &[], element, None)?;
    let anchor = CleanupAnchorId(b.ed.unit.tables.cleanup_anchors.len() as u32);
    b.ed.unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site: SiteRange { start: 0, end: 0 },
    });
    for (head, tail, failing) in [(normal, tail, false), (failed, failure_tail, true)] {
        b.at(head);
        b.set_reaching(cursor.slot, before);
        let more = b.compare(BinaryOp::Gt, tail, first)?;
        let step = b.block(
            if failing {
                BlockKind::FailureCleanup
            } else {
                BlockKind::Body
            },
            &[],
        );
        let end = b.block(
            if failing {
                BlockKind::FailureCleanup
            } else {
                BlockKind::Body
            },
            &[],
        );
        b.branch(more, step, end);
        b.at(step);
        let minus_one = b.u64(u64::MAX)?;
        let last = b.add(ArithMode::Wrapping, tail, minus_one);
        let at = b.slot_place(source, last)?;
        let value = b.take(at)?;
        b.push(Inst::StorageLive { slot: local });
        b.store(local_address, value)?;
        let remaining = b.value(layout, Holding::Plain);
        let retire = b.block(BlockKind::FailureCleanup, &[remaining]);
        let failure = b.failure_to(retire, &[last], None);
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
        b.jump(head, &[last]);
        b.at(retire);
        b.push(Inst::StorageDead {
            slot: local,
            if_live: false,
        });
        b.jump(failed, &[remaining]);
        b.at(end);
        let zero = b.u64(0)?;
        b.seq_set_len(source, zero)?;
        if failing {
            b.terminate(Terminator::Propagate);
        } else {
            ret_unit(&mut b);
        }
    }
    Ok(b.finish())
}

fn replace_position(
    b: &mut Body<'_, '_>,
    address: AddressId,
    value: ValueId,
) -> Result<(), LibraryError> {
    let root = b
        .root(address)
        .ok_or_else(|| LibraryError::Malformed("cursor position has no root".into()))?;
    let layout = b.layout_of(value);
    let displaced = b.slot(layout, SlotKind::Temporary);
    b.push(Inst::StorageLive { slot: displaced });
    let version = b.version(root);
    let displaced_version = b.version(displaced);
    let failure = b.failure(None);
    b.push(Inst::Replace {
        address,
        value,
        displaced,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    b.set_reaching(root, version);
    b.push(Inst::StorageDead {
        slot: displaced,
        if_live: false,
    });
    Ok(())
}
