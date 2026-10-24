//! Sorted maps: shared comparison-before-write search and owned entry cursors.
use super::builder::{Body, Cx};
use super::{CallShape, InlineLowering, LibraryBody, R, native_path};
use crate::PassError;
use crate::import::ImportCx;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::Value;
use hirsel_types::{Primitive, SignatureKind, Substitution, TypeKind, UseKind};

pub static EMPTY: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("sorted_map.empty"),
    lower: empty,
};
pub static LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("$intrinsic.SortedMapOrder_len"),
    lower: len,
};
pub static PUBLIC_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("sorted_map.len"),
    lower: public_len,
};
pub static GET: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("sorted_map.get"),
    build: get,
};
pub static REMOVE: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("sorted_map.remove"),
    build: remove,
};
pub static INSERT: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("sorted_map.insert"),
    build: insert,
};
pub static OPEN: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.SortedMapOrder_open"),
    build: open,
};
pub static NEXT: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("$intrinsic.SortedMapOrder_next"),
    build: next,
};
pub static CURSOR_DROP: LibraryBody = LibraryBody {
    path: || native_path("owned_sorted_map_cursor_drop"),
    build: cursor_drop,
};
macro_rules! orders {
    ($($row:ident : $name:literal),* $(,)?) => { $(pub static $row: LibraryBody = LibraryBody {
        path: || hirsel_syntax::DefPath::core(concat!("$intrinsic.OrderCompare_", $name)), build: order,
    };)* };
}
orders! { U8:"u8", U16:"u16", U32:"u32", U64:"u64", U128:"u128",
I8:"i8", I16:"i16", I32:"i32", I64:"i64", I128:"i128" }

fn empty(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    if call.arguments.len() != 1 {
        return Ok(false);
    }
    let by = cx.operand(&call.arguments[0].value)?;
    cx.consumed(by);
    let layout = cx.place_layout(call.destination)?;
    let payload = match cx.record(layout).kind {
        hirsel_lir::layout_types::LayoutKind::Block { payload }
        | hirsel_lir::layout_types::LayoutKind::Box { payload } => payload,
        _ => return Ok(false),
    };
    let entries_layout = cx.record(payload).fields[0].layout;
    let entries_payload = match cx.record(entries_layout).kind {
        hirsel_lir::layout_types::LayoutKind::Block { payload }
        | hirsel_lir::layout_types::LayoutKind::Box { payload } => payload,
        _ => unreachable!(),
    };
    let entries = cx.construct(
        entries_layout,
        entries_payload,
        ConstructorKind::Sequence,
        &[],
    )?;
    let symbol = cx.symbol(&Symbol {
        kind: SymbolKind::Type,
        definition: hirsel_syntax::DefPath::core("SortedMap"),
        instance: None,
    });
    let result = cx.construct(layout, payload, ConstructorKind::Record(symbol), &[entries])?;
    cx.finish(call, result)?;
    Ok(true)
}
fn len(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    if call.arguments.len() != 1 || call.arguments[0].value.usage != UseKind::Borrow {
        return Ok(false);
    }
    cx.container_len(call)?;
    Ok(true)
}
fn ret_unit(b: &mut Body<'_, '_>) {
    let result =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(result);
}
fn field(
    b: &mut Body<'_, '_>,
    root: AddressId,
    layout: LayoutId,
    ordinal: u32,
) -> Result<AddressId, PassError> {
    let payload = b.cx.payload(layout).unwrap_or(layout);
    let field = b
        .record(payload)
        .fields
        .iter()
        .find(|f| f.ordinal == ordinal)
        .ok_or_else(|| PassError::Malformed("sorted map field".into()))?
        .layout;
    b.place(
        AddressBase::Address(root),
        &[ProjectionStep::Field(ordinal)],
        field,
        None,
    )
}
fn field_layout(b: &Body<'_, '_>, layout: LayoutId, ordinal: usize) -> LayoutId {
    let payload = b.cx.payload(layout).unwrap_or(layout);
    b.record(payload).fields[ordinal].layout
}
fn construct(
    b: &mut Body<'_, '_>,
    layout: LayoutId,
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
    b.ed.unit.tables.constructs.push(ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor,
        layout: b.cx.payload(layout).unwrap_or(layout),
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
fn record_symbol(b: &mut Body<'_, '_>, name: &str) -> SymbolId {
    b.symbol(Symbol {
        kind: SymbolKind::Type,
        definition: hirsel_syntax::DefPath::core(name),
        instance: None,
    })
}
fn order(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let [a, c] = b.parameters[..] else {
        unreachable!()
    };
    let a = b.read_bits(a.address)?;
    let c = b.read_bits(c.address)?;
    let less = b.compare(BinaryOp::Lt, a, c)?;
    let greater = b.compare(BinaryOp::Gt, a, c)?;
    let negative = b.block(BlockKind::Body, &[]);
    let positive_test = b.block(BlockKind::Body, &[]);
    let positive = b.block(BlockKind::Body, &[]);
    let equal = b.block(BlockKind::Body, &[]);
    b.branch(less, negative, positive_test);
    let layout = b.cx.fn_abi().result.unwrap().layout;
    b.at(negative);
    let minus_one = b.constant(layout, ConstantValue::Integer(u32::MAX as u128));
    b.ret(Some(minus_one));
    b.at(positive_test);
    b.branch(greater, positive, equal);
    b.at(positive);
    let one = b.constant(layout, ConstantValue::Integer(1));
    b.ret(Some(one));
    b.at(equal);
    let zero = b.constant(layout, ConstantValue::Integer(0));
    b.ret(Some(zero));
    Ok(b.finish())
}
fn comparator_abi(cx: &mut Cx<'_>) -> Result<hirsel_lir::abi_types::FnAbiId, PassError> {
    let signature = cx
        .program
        .signatures
        .get(&hirsel_syntax::DefPath::core("Order"))
        .unwrap();
    let SignatureKind::Type(order) = &signature.kind else {
        unreachable!()
    };
    let key = cx.type_actual(0)?;
    let shape = hirsel_types::substitute(
        cx.types,
        &cx.program.memo,
        order.representation.unwrap(),
        &Substitution {
            binder: Some(hirsel_types::GenericBinder::Bound(0)),
            roles: &signature.metadata.roles,
            arguments: &[hirsel_types::GenericArgument::Type(key)],
            authorities: &[hirsel_types::AuthoritySummary::inert()],
        },
    )
    .map_err(|e| PassError::Malformed(format!("Order representation: {e:?}")))?;
    let TypeKind::Record(fields) = &cx.types.get(shape).unwrap().kind else {
        unreachable!()
    };
    let callable = fields[0].ty;
    cx.layouts
        .fn_abi(cx.types, callable, CallableShape::Closure)
        .map_err(|e| PassError::Layout(cx.path.clone(), e))
}
fn callback(
    b: &mut Body<'_, '_>,
    callee: ValueId,
    abi: hirsel_lir::abi_types::FnAbiId,
    values: &[ValueId],
    loans: &[LoanId],
) -> ValueId {
    let arguments = values
        .iter()
        .enumerate()
        .map(|(i, &value)| CallArgument {
            parameter: i as u32,
            passing: Passing::Borrow,
            value: ArgumentValue::Value(value),
        })
        .collect();
    b.call(Callee::Indirect(callee), Vec::new(), abi, arguments, loans)
        .expect("comparator result")
}

fn arithmetic(b: &mut Body<'_, '_>, op: BinaryOp, left: ValueId, right: ValueId) -> ValueId {
    let result = b.value(b.layout_of(left), Holding::Plain);
    let failure = b.failure(Some("SEM-1"));
    b.push(Inst::Binary {
        result,
        op,
        left,
        right,
        failure,
    });
    result
}
fn replace_from(
    b: &mut Body<'_, '_>,
    address: AddressId,
    incoming: AddressId,
) -> Result<(), PassError> {
    let unit = &b.ed.unit;
    let layout = unit.types[unit.addresses[incoming.index()].ty.index()].layout;
    let root = b.root(address).unwrap();
    let displaced = b.slot(layout, SlotKind::Temporary);
    b.push(Inst::StorageLive { slot: displaced });
    let old = b.place(AddressBase::Slot(displaced), &[], layout, None)?;
    // Finish every address preparation before extracting the taken value.
    let value = b.take(incoming)?;
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
    b.set_reaching(displaced, displaced_version);
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
    let failure = b.failure(None);
    b.push(Inst::Drop {
        address: old,
        if_live: false,
        anchor,
        failure,
    });
    b.push(Inst::StorageDead {
        slot: displaced,
        if_live: false,
    });
    Ok(())
}
fn insert(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let abi = comparator_abi(cx)?;
    let mut b = Body::new(cx)?;
    let [_by, map, key, value] = b.parameters[..] else {
        unreachable!()
    };
    let one = b.u64(1)?;
    let u64_layout = b.u64_layout()?;
    let Search {
        entries,
        pair_layout,
        len,
        equal,
        equal_position,
        equal_version,
        vacant,
        position,
        vacant_version,
    } = search(&mut b, abi)?;
    b.at(equal);
    b.set_reaching(map.slot, equal_version);
    let pair = b.slot_place(entries, equal_position)?;
    let target = field(&mut b, pair, pair_layout, 1)?;
    replace_from(&mut b, target, value.address)?;
    // The taken key equals the stored one, which stays.
    super::map::drop_place(&mut b, key.address);
    ret_unit(&mut b);
    b.at(vacant);
    b.set_reaching(map.slot, vacant_version);
    b.hold(key.slot, key.address);
    b.hold(value.slot, value.address);
    b.arena_push(entries)?;
    let local = b.slot(pair_layout, SlotKind::Temporary);
    b.push(Inst::StorageLive { slot: local });
    let local_address = b.place(AddressBase::Slot(local), &[], pair_layout, None)?;
    let key = b.take(key.address)?;
    let moved = b.take(value.address)?;
    let pair = construct(&mut b, pair_layout, ConstructorKind::Tuple, &[key, moved]);
    b.store(local_address, pair)?;
    b.hold(local, local_address);
    let count = b.add(ArithMode::Wrapping, len, one);
    b.seq_reserve(entries, count)?;
    let initial_version = b.reaching(map.slot)?;
    let tail = b.value(u64_layout, Holding::Plain);
    let version = b.version(map.slot);
    let shift = b.block(BlockKind::Body, &[tail, version]);
    let step = b.block(BlockKind::Body, &[]);
    let install = b.block(BlockKind::Body, &[]);
    b.jump(shift, &[len, initial_version]);
    b.at(shift);
    b.set_reaching(map.slot, version);
    let more = b.compare(BinaryOp::Gt, tail, position)?;
    b.branch(more, step, install);
    b.at(step);
    let minus_one = b.u64(u64::MAX)?;
    let last = b.add(ArithMode::Wrapping, tail, minus_one);
    let from = b.slot_place(entries, last)?;
    let to = b.slot_place(entries, tail)?;
    let moved = b.take(from)?;
    b.store(to, moved)?;
    let after = b.reaching(map.slot)?;
    b.jump(shift, &[last, after]);
    b.at(install);
    b.set_reaching(map.slot, version);
    let target = b.slot_place(entries, position)?;
    let pair = b.take(local_address)?;
    b.store(target, pair)?;
    b.push(Inst::StorageDead {
        slot: local,
        if_live: false,
    });
    // The shift's detaches may have moved the entries, so their place is
    // located afresh.
    let entries = field(&mut b, map.address, map.layout, 0)?;
    b.seq_set_len(entries, count)?;
    ret_unit(&mut b);
    Ok(b.finish())
}

fn open(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let source = b.parameters[0];
    let flag = b.bool_layout()?;
    let (view, loan) = b.borrow(source.address, false)?;
    let root_unique = b.value(flag, Holding::Plain);
    b.push(Inst::IsUnique {
        result: root_unique,
        value: view,
    });
    b.end_loan(loan);
    let entries = field(&mut b, source.address, source.layout, 0)?;
    let (entries, loan) = b.borrow(entries, false)?;
    let entries_unique = b.value(flag, Holding::Plain);
    b.push(Inst::IsUnique {
        result: entries_unique,
        value: entries,
    });
    b.end_loan(loan);
    let unique = b.value(flag, Holding::Plain);
    let failure = b.failure(None);
    b.push(Inst::Binary {
        result: unique,
        op: BinaryOp::BitAnd,
        left: root_unique,
        right: entries_unique,
        failure,
    });
    let value = b.take(source.address)?;
    let zero = b.u64(0)?;
    let layout = b.cx.fn_abi().result.unwrap().layout;
    let symbol = record_symbol(&mut b, "$intrinsic.OwnedSortedMapCursor");
    let cursor = construct(
        &mut b,
        layout,
        ConstructorKind::Record(symbol),
        &[value, zero, unique],
    );
    b.ret(Some(cursor));
    Ok(b.finish())
}
fn next(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let cursor = b.parameters[0];
    let source_layout = field_layout(&b, cursor.layout, 0);
    let source = field(&mut b, cursor.address, cursor.layout, 0)?;
    let source = field(&mut b, source, source_layout, 0)?;
    let position = field(&mut b, cursor.address, cursor.layout, 1)?;
    let unique = field(&mut b, cursor.address, cursor.layout, 2)?;
    let index = b.read_bits(position)?;
    let (view, loan) = b.borrow(source, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    let done = b.compare(BinaryOp::Eq, index, len)?;
    let exhausted = b.block(BlockKind::Body, &[]);
    let item = b.block(BlockKind::Body, &[]);
    b.branch(done, exhausted, item);
    let option_layout = b.cx.fn_abi().result.unwrap().layout;
    let option_symbol = record_symbol(&mut b, "Option");
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
    let array_layout = field_layout(&b, source_layout, 0);
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

fn cursor_drop(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let cursor = b.parameters[0];
    let source_layout = field_layout(&b, cursor.layout, 0);
    let source = field(&mut b, cursor.address, cursor.layout, 0)?;
    let source = field(&mut b, source, source_layout, 0)?;
    let position = field(&mut b, cursor.address, cursor.layout, 1)?;
    let unique = field(&mut b, cursor.address, cursor.layout, 2)?;
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
    let element = b.cx.element(field_layout(&b, source_layout, 0))?;
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
) -> Result<(), PassError> {
    let root = b
        .root(address)
        .ok_or_else(|| PassError::Malformed("cursor position has no root".into()))?;
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

struct Search {
    entries: AddressId,
    pair_layout: LayoutId,
    len: ValueId,
    equal: BlockId,
    equal_position: ValueId,
    equal_version: ValueId,
    vacant: BlockId,
    position: ValueId,
    vacant_version: ValueId,
}
// All keyed operations use the same comparison-before-write probes (SEM-5).
fn search(b: &mut Body<'_, '_>, abi: hirsel_lir::abi_types::FnAbiId) -> R<Search> {
    let [by, map, key, ..] = b.parameters[..] else {
        unreachable!()
    };
    let entries_layout = field_layout(b, map.layout, 0);
    let pair_layout = b.cx.element(entries_layout)?;
    let entries = field(b, map.address, map.layout, 0)?;
    let compare = field(b, by.address, by.layout, 0)?;
    let (view, loan) = b.borrow(entries, false)?;
    let len = b.seq_len(view)?;
    b.end_loan(loan);
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let two = b.u64(2)?;
    let u64_layout = b.u64_layout()?;
    // Spec #94: all engines use these same observable binary-search probes.
    let lo = b.value(u64_layout, Holding::Plain);
    let hi = b.value(u64_layout, Holding::Plain);
    let search = b.block(BlockKind::Body, &[lo, hi]);
    let probe = b.block(BlockKind::Body, &[]);
    let position = b.value(u64_layout, Holding::Plain);
    let vacant_version = b.version(map.slot);
    let vacant = b.block(BlockKind::Body, &[position, vacant_version]);
    let equal_position = b.value(u64_layout, Holding::Plain);
    let equal_version = b.version(map.slot);
    let equal = b.block(BlockKind::Body, &[equal_position, equal_version]);
    b.jump(search, &[zero, len]);
    b.at(search);
    let more = b.compare(BinaryOp::Lt, lo, hi)?;
    let yes = b.edge(probe, &[]);
    let no = b.edge(vacant, &[lo, map.value]);
    b.terminate(Terminator::Branch {
        condition: more,
        yes,
        no,
    });
    b.at(probe);
    let width = arithmetic(b, BinaryOp::Sub, hi, lo);
    let half = arithmetic(b, BinaryOp::Div, width, two);
    let mid = b.add(ArithMode::Wrapping, lo, half);
    let pair = b.slot_place(entries, mid)?;
    let stored_key = field(b, pair, pair_layout, 0)?;
    let (callee, callee_loan) = b.borrow(compare, false)?;
    let (incoming, key_loan) = b.borrow(key.address, false)?;
    let (stored, stored_loan) = b.borrow(stored_key, false)?;
    let order = callback(
        b,
        callee,
        abi,
        &[incoming, stored],
        &[callee_loan, key_loan, stored_loan],
    );
    let i32_layout = b.cx.primitive(Primitive::I32)?;
    let zero_order = b.constant(i32_layout, ConstantValue::Integer(0));
    let is_equal = b.compare(BinaryOp::Eq, order, zero_order)?;
    let unequal = b.block(BlockKind::Body, &[]);
    let yes = b.edge(equal, &[mid, map.value]);
    let no = b.edge(unequal, &[]);
    b.terminate(Terminator::Branch {
        condition: is_equal,
        yes,
        no,
    });
    b.at(unequal);
    let less = b.compare(BinaryOp::Lt, order, zero_order)?;
    let above = b.add(ArithMode::Wrapping, mid, one);
    let yes = b.edge(search, &[lo, mid]);
    let no = b.edge(search, &[above, hi]);
    b.terminate(Terminator::Branch {
        condition: less,
        yes,
        no,
    });
    Ok(Search {
        entries,
        pair_layout,
        len,
        equal,
        equal_position,
        equal_version,
        vacant,
        position,
        vacant_version,
    })
}

fn get(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    access(cx, false)
}
fn remove(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    access(cx, true)
}
fn access(cx: &mut Cx<'_>, remove: bool) -> R<LirUnit<Value>> {
    let abi = comparator_abi(cx)?;
    let mut b = Body::new(cx)?;
    let map = b.parameters[1];
    let Search {
        entries,
        pair_layout,
        equal,
        equal_position,
        equal_version,
        vacant,
        vacant_version,
        ..
    } = search(&mut b, abi)?;
    b.at(vacant);
    b.set_reaching(map.slot, vacant_version);
    let none = super::map::option(&mut b, None)?;
    b.ret(Some(none));
    b.at(equal);
    b.set_reaching(map.slot, equal_version);
    let value = if remove {
        let (slot, owner) = super::map::remove_entry(&mut b, entries, equal_position, true)?
            .expect("pair removal owns its value");
        let value = b.take(owner)?;
        b.push(Inst::StorageDead {
            slot,
            if_live: false,
        });
        value
    } else {
        let entry = b.slot_place(entries, equal_position)?;
        let value = field(&mut b, entry, pair_layout, 1)?;
        let failure = b.failure(None);
        b.copy(value, failure)?
    };
    let some = super::map::option(&mut b, Some(value))?;
    b.ret(Some(some));
    Ok(b.finish())
}
fn public_len(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> R<bool> {
    let [by, map] = call.arguments else {
        return Ok(false);
    };
    let by = cx.operand(&by.value)?;
    cx.consumed(by);
    let map = cx.operand(&map.value)?;
    let entries = cx.read_field(map, 0)?;
    let layout = cx.place_layout(call.destination)?;
    let result = cx.value(layout, Holding::Plain);
    cx.push(Inst::SeqLen {
        result,
        value: entries,
    });
    cx.consumed(map);
    cx.finish(call, result)?;
    Ok(true)
}
