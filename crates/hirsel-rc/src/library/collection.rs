//! Ownership-moving List primitives. Every bounds check precedes mutation.
use super::LibraryBody;
use super::builder::{Body, Cx};
use crate::PassError;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::runtime_abi::EXTENT_MAX;
use hirsel_lir::stage::Value;

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
// Capacity is unobservable except through allocation, so a reservation whose
// storage lies beyond any host's address space (`EXTENT_MAX`) reserves nothing,
// as the interpreter's does; one within it that the host cannot supply is
// exhaustion, which is environmental.
fn reserve(cx: &mut Cx<'_>) -> Result<LirUnit<Value>, PassError> {
    let mut b = Body::new(cx)?;
    let list = b.parameters[0];
    let capacity = b.read_bits(b.parameters[1].address)?;
    let least = match b
        .cx
        .payload(b.address_layout(list.address))
        .map(|p| &b.cx.record(p).kind)
    {
        Some(
            LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element }
            | LayoutKind::SoaColumns { element },
        ) => hirsel_lir::layout::least_size(&b.cx.record(*element).size),
        _ => {
            return b
                .cx
                .unsupported("list.reserve of a place that holds no sequence block");
        }
    };
    if least == 0 {
        b.seq_reserve(list.address, capacity)?;
        ret_unit(&mut b);
        return Ok(b.finish());
    }
    // The count and used words and the first element's alignment come before
    // the elements: 64 bytes is more than either sequence form takes.
    let most = b.u64((EXTENT_MAX - 64) / least)?;
    let fits = b.compare(BinaryOp::Le, capacity, most)?;
    let grow = b.block(BlockKind::Body, &[]);
    let done = b.block(BlockKind::Body, &[]);
    b.branch(fits, grow, done);
    b.at(grow);
    b.seq_reserve(list.address, capacity)?;
    ret_unit(&mut b);
    b.at(done);
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
