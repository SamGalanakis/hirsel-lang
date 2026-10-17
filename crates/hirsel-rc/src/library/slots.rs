//! Generation-checked Slots over an ordinary List of (generation, Option[T]).
//! Generation zero is reserved for retirement after the maximum wraps.
use super::builder::{Body, Cx};
use super::{CallShape, InlineLowering, LibraryBody};
use crate::PassError;
use crate::import::ImportCx;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind};
use hirsel_lir::stage::Value;
type R<T> = Result<T, PassError>;
pub static EMPTY: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("slots.empty"),
    lower: empty,
};
pub static INSERT: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("slots.insert"),
    build: insert,
};
pub static GET: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("slots.get"),
    build: get,
};
pub static REMOVE: LibraryBody = LibraryBody {
    path: || hirsel_syntax::DefPath::core("slots.remove"),
    build: remove,
};
fn payload(cx: &ImportCx<'_>, layout: LayoutId) -> LayoutId {
    match cx.record(layout).kind {
        LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
        _ => layout,
    }
}
fn empty(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> super::R<bool> {
    if !call.arguments.is_empty() {
        return Ok(false);
    }
    let layout = cx.place_layout(call.destination)?;
    let shape = payload(cx, layout);
    let fields: Vec<_> = cx.record(shape).fields.iter().map(|f| f.layout).collect();
    let mut values = Vec::new();
    for field in fields {
        values.push(cx.construct(field, payload(cx, field), ConstructorKind::Sequence, &[])?);
    }
    let symbol = cx.symbol(&Symbol {
        kind: SymbolKind::Type,
        definition: call.path.clone(),
        instance: None,
    });
    let result = cx.construct(layout, shape, ConstructorKind::Record(symbol), &values)?;
    cx.finish(call, result)?;
    Ok(true)
}
fn field(b: &mut Body<'_, '_>, address: AddressId, ordinal: u32) -> R<AddressId> {
    let layout = b.ed.unit.types[b.ed.unit.addresses[address.index()].ty.index()].layout;
    let shape = b.cx.payload(layout).unwrap_or(layout);
    let layout = b
        .record(shape)
        .fields
        .iter()
        .find(|f| f.ordinal == ordinal)
        .ok_or_else(|| PassError::Malformed(format!("field {ordinal} of {shape:?}")))?
        .layout;
    b.place(
        AddressBase::Address(address),
        &[ProjectionStep::Field(ordinal)],
        layout,
        None,
    )
}
fn len(b: &mut Body<'_, '_>, address: AddressId) -> R<ValueId> {
    let (value, loan) = b.borrow(address, false)?;
    let result = b.seq_len(value)?;
    b.end_loan(loan);
    Ok(result)
}
fn construct(
    b: &mut Body<'_, '_>,
    layout: LayoutId,
    constructor: ConstructorKind,
    values: &[ValueId],
) -> R<ValueId> {
    let shape = b.cx.payload(layout).unwrap_or(layout);
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
        constructor,
        layout: shape,
        fields,
        destination: DestinationClass::Inferred,
        placement: None,
        anchor: b.anchor,
    });
    let result = b.fresh(layout);
    let active: Vec<_> = b.ed.code[b.current().index()]
        .iter()
        .fold(Vec::new(), |mut ls, i| {
            match *i {
                Inst::BeginLoan { loan, .. } => ls.push(loan),
                Inst::EndLoan { loan, .. } => ls.retain(|&l| l != loan),
                _ => {}
            };
            ls
        });
    let failure = loan_failure(b, &active, None);
    b.push(Inst::Construct {
        result,
        construct,
        failure,
    });
    Ok(result)
}
fn nominal(b: &mut Body<'_, '_>, name: &str) -> SymbolId {
    b.symbol(Symbol {
        kind: SymbolKind::Type,
        definition: hirsel_syntax::DefPath::core(name),
        instance: None,
    })
}
fn loan_failure(b: &mut Body<'_, '_>, loans: &[LoanId], rule: Option<&str>) -> FailureId {
    let next = b.failure(rule);
    let cleanup = b.ed.unit.tables.failures[next.index()].cleanup;
    let block = b.block(BlockKind::FailureCleanup, &[]);
    for &loan in loans.iter().rev() {
        b.ed.code[block.index()].push(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    b.ed.unit.terminators[block.index()] = Terminator::Jump(cleanup);
    b.failure_to(block, &[], rule)
}

fn option(b: &mut Body<'_, '_>, layout: LayoutId, value: Option<ValueId>) -> R<ValueId> {
    let definition = nominal(b, "Option");
    construct(
        b,
        layout,
        ConstructorKind::Variant {
            definition,
            variant: u32::from(value.is_some()),
        },
        &value.into_iter().collect::<Vec<_>>(),
    )
}
fn id(b: &mut Body<'_, '_>, index: ValueId, generation: ValueId) -> R<ValueId> {
    let layout = b.cx.fn_abi().result.unwrap().layout;
    construct(b, layout, ConstructorKind::Tuple, &[index, generation])
}
// Exchange a live place and retain the displaced value in ordinary cleanup storage.
fn displaced(b: &mut Body<'_, '_>, address: AddressId) -> R<(SlotId, AddressId)> {
    let layout = b.address_layout(address);
    let slot = b.slot(layout, SlotKind::Displaced);
    b.push(Inst::StorageLive { slot });
    let old = b.place(AddressBase::Slot(slot), &[], layout, None)?;
    Ok((slot, old))
}
fn replace(
    b: &mut Body<'_, '_>,
    address: AddressId,
    value: ValueId,
    slot: SlotId,
    old: AddressId,
) -> R<()> {
    let root = b.root(address).expect("slot container");
    let version = b.version(root);
    b.set_reaching(root, version);
    let displaced_version = b.version(slot);
    b.set_reaching(slot, displaced_version);
    let failure = b.failure(None);
    b.push(Inst::Replace {
        address,
        value,
        displaced: slot,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    b.hold(slot, old);
    Ok(())
}
fn drop_local(b: &mut Body<'_, '_>, slot: SlotId, address: AddressId) {
    b.forget_held(address);
    let anchor = CleanupAnchorId(b.ed.unit.tables.cleanup_anchors.len() as u32);
    b.ed.unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site: SiteRange { start: 0, end: 0 },
    });
    let failure = b.failure(None);
    b.push(Inst::Drop {
        address,
        if_live: false,
        anchor,
        failure,
    });
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
}
fn insert(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let container = b.parameters[0];
    let entries = field(&mut b, container.address, 0)?;
    let count = len(&mut b, entries)?;
    let word = b.u64_layout()?;
    let i = b.fresh(word);
    let scan = b.block(BlockKind::Body, &[i]);
    let inspect = b.block(BlockKind::Body, &[]);
    let append = b.block(BlockKind::Body, &[]);
    let advance = b.block(BlockKind::Body, &[]);
    let available = b.block(BlockKind::Body, &[]);
    let reuse = b.block(BlockKind::Body, &[]);
    let zero = b.u64(0)?;
    b.jump(scan, &[zero]);
    b.at(scan);
    let more = b.compare(BinaryOp::Lt, i, count)?;
    b.branch(more, inspect, append);
    b.at(inspect);
    let entry = b.slot_place(entries, i)?;
    let generation_address = field(&mut b, entry, 0)?;
    let generation = b.read_bits(generation_address)?;
    let retired_generation = b.u64(0)?;
    let retired = b.compare(BinaryOp::Eq, generation, retired_generation)?;
    b.branch(retired, advance, available);
    b.at(available);
    let value_address = field(&mut b, entry, 1)?;
    let (tag, loan) = b.borrow(value_address, false)?;
    let occupied = b.block(BlockKind::Body, &[]);
    let vacant = b.edge(reuse, &[]);
    let occupied_edge = b.edge(occupied, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm {
            value: 1,
            edge: occupied_edge,
        }],
    );
    b.terminate(Terminator::Switch {
        value: tag,
        arms,
        otherwise: vacant,
    });
    b.at(occupied);
    b.end_loan(loan);
    b.jump(advance, &[]);
    b.at(advance);
    let one = b.u64(1)?;
    let next = b.add(ArithMode::Wrapping, i, one);
    b.jump(scan, &[next]);
    let held = b.held_state();
    let before = b.reaching(container.slot)?;
    b.at(reuse);
    b.end_loan(loan);
    // The scan's addresses are read-only. Prepare a separate write path only
    // after selecting the vacancy, so COW cannot change a loop-carried root.
    let entries = field(&mut b, container.address, 0)?;
    let entry = b.slot_place(entries, i)?;
    let value_address = field(&mut b, entry, 1)?;
    let option_layout = b.address_layout(value_address);
    let (old_slot, old) = displaced(&mut b, value_address)?;
    let moved = b.take(b.parameters[1].address)?;
    let value = option(&mut b, option_layout, Some(moved))?;
    replace(&mut b, value_address, value, old_slot, old)?;
    drop_local(&mut b, old_slot, old);
    let result = id(&mut b, i, generation)?;
    b.ret(Some(result));
    b.at(append);
    b.restore_held(&held);
    b.set_reaching(container.slot, before);
    let entries = field(&mut b, container.address, 0)?;
    let one = b.u64(1)?;
    let next = b.add(ArithMode::Wrapping, count, one);
    b.seq_reserve(entries, next)?;
    let entries = field(&mut b, container.address, 0)?;
    let element = b.cx.element(b.address_layout(entries))?;
    let option_layout = b.record(element).fields[1].layout;
    let at = b.slot_place(entries, count)?;
    let moved = b.take(b.parameters[1].address)?;
    let value = option(&mut b, option_layout, Some(moved))?;
    let generation = b.u64(1)?;
    let pair = construct(
        &mut b,
        element,
        ConstructorKind::Tuple,
        &[generation, value],
    )?;
    b.store(at, pair)?;
    let entries = field(&mut b, container.address, 0)?;
    b.seq_set_len(entries, next)?;
    let result = id(&mut b, count, generation)?;
    b.ret(Some(result));
    Ok(b.finish())
}
fn get(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    lookup(cx, false)
}
fn remove(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    lookup(cx, true)
}
fn lookup(cx: &mut Cx<'_>, remove: bool) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let container = b.parameters[0];
    let handle = b.parameters[1];
    let entries = field(&mut b, container.address, 0)?;
    let index_address = field(&mut b, handle.address, 0)?;
    let index = b.read_bits(index_address)?;
    let expected_address = field(&mut b, handle.address, 1)?;
    let expected = b.read_bits(expected_address)?;
    let count = len(&mut b, entries)?;
    let inside = b.compare(BinaryOp::Lt, index, count)?;
    let check = b.block(BlockKind::Body, &[]);
    let no = b.block(BlockKind::Body, &[]);
    let yes = b.block(BlockKind::Body, &[]);
    b.branch(inside, check, no);
    b.at(check);
    let entry = b.slot_place(entries, index)?;
    let generation_address = field(&mut b, entry, 0)?;
    let generation = b.read_bits(generation_address)?;
    let same = b.compare(BinaryOp::Eq, generation, expected)?;
    b.branch(same, yes, no);
    b.at(no);
    let layout = b.cx.fn_abi().result.unwrap().layout;
    let result = option(&mut b, layout, None)?;
    b.ret(Some(result));
    b.at(yes);
    let value_address = field(&mut b, entry, 1)?;
    let result = if remove {
        // Construct the replacement before extracting; no failing operation may
        // observe a hole in the published container.
        let (removed_slot, removed) = displaced(&mut b, value_address)?;
        let empty = option(&mut b, layout, None)?;
        replace(&mut b, value_address, empty, removed_slot, removed)?;
        let one = b.u64(1)?;
        let next = b.add(ArithMode::Wrapping, generation, one);
        let entries = field(&mut b, container.address, 0)?;
        let entry = b.slot_place(entries, index)?;
        let generation_address = field(&mut b, entry, 0)?;
        let (old_slot, old) = displaced(&mut b, generation_address)?;
        replace(&mut b, generation_address, next, old_slot, old)?;
        drop_local(&mut b, old_slot, old);
        let value = b.take(removed)?;
        b.push(Inst::StorageDead {
            slot: removed_slot,
            if_live: false,
        });
        value
    } else {
        let failure = b.failure(None);
        b.copy(value_address, failure)?
    };
    b.ret(Some(result));
    Ok(b.finish())
}
