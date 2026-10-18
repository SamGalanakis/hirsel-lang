//! Native insertion-ordered maps and sets. Field 0 is dense entries; field 1
//! is an optional open-addressing index. Keyed reads never mutate either.
use super::builder::{Body, Cx, Direct};
use super::{CallShape, InlineLowering, LibraryBody, Request, native_path};
use crate::PassError;
use crate::import::ImportCx;
use hirsel_core_contract::Passing;
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{CopyRecipe, LayoutId, LayoutKind};
use hirsel_lir::stage::Value;
use hirsel_syntax::PassingMode;
use hirsel_types::{
    Allocation, AuthoritySummary, BuiltinType, FunctionType, GenericArgument, Parameter, Primitive,
    Type, TypeId, TypeKind, Uses,
};

type R<T> = Result<T, PassError>;
macro_rules! body {
    ($row:ident, $name:literal, $build:ident) => {
        pub static $row: LibraryBody = LibraryBody {
            path: || hirsel_syntax::DefPath::core($name),
            build: $build,
        };
    };
}
body!(GET, "map.get", get);
body!(CONTAINS, "set.contains", contains);
body!(INSERT_MAP, "map.insert", insert_map);
body!(INSERT_SET, "set.insert", insert_set);
pub static MAP_PROBE: LibraryBody = LibraryBody {
    path: || native_path("map_probe"),
    build: probe,
};
pub static SET_PROBE: LibraryBody = LibraryBody {
    path: || native_path("set_probe"),
    build: probe,
};

pub static EMPTY_MAP: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("map.empty"),
    lower: empty,
};
pub static EMPTY_SET: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("set.empty"),
    lower: empty,
};
pub static MAP_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("$intrinsic.MapInsertionOrder_len"),
    lower: length,
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
fn length(cx: &mut ImportCx<'_>, call: &CallShape<'_>) -> super::R<bool> {
    let [argument] = call.arguments else {
        return Ok(false);
    };
    let value = cx.operand(&argument.value)?;
    let entries = cx.read_field(value, 0)?;
    let layout = cx.place_layout(call.destination)?;
    let result = cx.value(layout, Holding::Plain);
    cx.push(Inst::SeqLen {
        result,
        value: entries,
    });
    cx.consumed(value);
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
fn option(b: &mut Body<'_, '_>, value: Option<ValueId>) -> R<ValueId> {
    let layout = b.cx.fn_abi().result.expect("Option result").layout;
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
fn unit(b: &mut Body<'_, '_>) {
    let result =
        b.cx.fn_abi()
            .result
            .map(|r| b.constant(r.layout, ConstantValue::Unit));
    b.ret(result);
}
fn add_type(store: &mut hirsel_types::TypeStore, kind: TypeKind) -> TypeId {
    store.push(Type::with_kind(kind))
}
fn builtin(b: &mut Body<'_, '_>, kind: BuiltinType, types: &[TypeId]) -> TypeId {
    add_type(
        &mut b.cx.store,
        TypeKind::Builtin {
            kind,
            arguments: types
                .iter()
                .copied()
                .map(GenericArgument::Type)
                .collect::<Vec<_>>()
                .into(),
        },
    )
}
fn helper(
    b: &mut Body<'_, '_>,
    name: &str,
    parameters: &[(PassingMode, TypeId)],
    result: TypeId,
    allocation: Allocation,
    actuals: Vec<GenericArgument>,
) -> R<Direct> {
    let callable = add_type(
        &mut b.cx.store,
        TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: parameters
                .iter()
                .map(|&(passing, ty)| Parameter {
                    name: None,
                    label: None,
                    passing,
                    ty,
                })
                .collect::<Vec<_>>()
                .into(),
            result,
            error: None,
            allocation,
            authority: AuthoritySummary::Unknown,
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }),
    );
    let path = if name == "list.push" {
        hirsel_syntax::DefPath::core(name)
    } else {
        native_path(name)
    };
    let body = super::body_index(&path).expect("registered helper");
    let request = Request::native(body, &path, b.cx.store.clone(), actuals, callable);
    b.cx.require(request)
}
fn call(b: &mut Body<'_, '_>, direct: Direct, arguments: &[CallArgument]) -> R<Option<ValueId>> {
    let abi = b.cx.layouts.table().fn_abis[direct.abi.0 as usize].clone();
    let result = abi.result.map(|r| b.fresh(r.layout));
    let symbol = b.symbol(direct.symbol);
    let args = PoolRange::push(
        &mut b.ed.unit.pools.call_arguments,
        arguments.iter().copied(),
    );
    let id = CallId(b.ed.unit.tables.calls.len() as u32);
    b.ed.unit.tables.calls.push(CallSite {
        operation_arguments: Vec::new(),
        callee: Callee::Direct(symbol),
        abi: direct.abi,
        arguments: args,
        result,
        destination: abi.destination.then_some(DestinationClass::Inferred),
        declared: Effects::ALL,
        anchor: b.anchor,
    });
    let next = b.block(BlockKind::Body, &[]);
    let normal = b.edge(next, &[]);
    let active: Vec<_> = b.ed.code[b.current().index()]
        .iter()
        .fold(Vec::new(), |mut active, i| {
            match *i {
                Inst::BeginLoan { loan, .. } => active.push(loan),
                Inst::EndLoan { loan, .. } => active.retain(|&l| l != loan),
                _ => {}
            };
            active
        });
    let failure = loan_failure(b, &active, None);
    b.terminate(Terminator::Call {
        call: id,
        normal: Some(normal),
        failure,
    });
    b.at(next);
    Ok(result)
}
fn argument(parameter: u32, passing: Passing, value: ValueId) -> CallArgument {
    CallArgument {
        parameter,
        passing,
        value: ArgumentValue::Value(value),
    }
}
fn mut_argument(b: &mut Body<'_, '_>, parameter: u32, address: AddressId) -> R<CallArgument> {
    let slot = b.root(address).expect("mut place");
    let version = b.version(slot);
    b.set_reaching(slot, version);
    Ok(CallArgument {
        parameter,
        passing: Passing::Mut,
        value: ArgumentValue::Address { address, version },
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
fn eq(b: &mut Body<'_, '_>, a: AddressId, c: AddressId) -> R<ValueId> {
    let (left, l) = b.borrow(a, false)?;
    let (right, r) = b.borrow(c, false)?;
    let layout = b.layout_of(left);
    let boolean = b.bool_layout()?;
    let result = b.fresh(boolean);
    let failure = loan_failure(b, &[l, r], Some("MEM-3"));
    if matches!(b.record(layout).kind, LayoutKind::Scalar(_)) {
        b.push(Inst::Binary {
            result,
            op: BinaryOp::Eq,
            left,
            right,
            failure,
        });
    } else {
        b.push(Inst::Compare {
            result,
            op: CompareOp::Eq,
            left,
            right,
            layout,
            failure,
        });
    }
    b.end_loan(r);
    b.end_loan(l);
    Ok(result)
}
fn hash(b: &mut Body<'_, '_>, a: AddressId) -> R<ValueId> {
    let (value, loan) = b.borrow(a, false)?;
    let layout = b.layout_of(value);
    let result_layout = b.u64_layout()?;
    let result = b.fresh(result_layout);
    let failure = loan_failure(b, &[loan], Some("MEM-3"));
    b.push(Inst::Hash {
        result,
        value,
        layout,
        failure,
    });
    b.end_loan(loan);
    Ok(result)
}
fn probe_call(b: &mut Body<'_, '_>) -> R<ValueId> {
    let k = b.cx.type_actual(0)?;
    let types = if b.parameters.len() == 3 || b.cx.path == hirsel_syntax::DefPath::core("map.get") {
        vec![k, b.cx.type_actual(1)?]
    } else {
        vec![k]
    };
    let is_map = types.len() == 2;
    let container = builtin(
        b,
        if is_map {
            BuiltinType::Map
        } else {
            BuiltinType::Set
        },
        &types,
    );
    let u64_ty = add_type(&mut b.cx.store, TypeKind::Primitive(Primitive::U64));
    let actuals = b.cx.arguments.clone();
    let direct = helper(
        b,
        if is_map { "map_probe" } else { "set_probe" },
        &[(PassingMode::Borrow, container), (PassingMode::Borrow, k)],
        u64_ty,
        Allocation::NoAlloc,
        actuals,
    )?;
    let (m, ml) = b.borrow(b.parameters[0].address, false)?;
    let (key, kl) = b.borrow(b.parameters[1].address, false)?;
    let index = call(
        b,
        direct,
        &[
            argument(0, Passing::Borrow, m),
            argument(1, Passing::Borrow, key),
        ],
    )?
    .unwrap();
    b.end_loan(kl);
    b.end_loan(ml);
    Ok(index)
}
fn probe(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let is_map = b.cx.path == native_path("map_probe");
    let container = b.parameters[0];
    let key = b.parameters[1];
    // Hash even an empty/unindexed collection. The complete key's depth is checked here.
    let key_hash = hash(&mut b, key.address)?;
    let entries = field(&mut b, container.address, 0)?;
    let count = len(&mut b, entries)?;
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let u64_layout = b.u64_layout()?;
    let i = b.fresh(u64_layout);
    let loop_ = b.block(BlockKind::Body, &[i]);
    let check = b.block(BlockKind::Body, &[]);
    let miss = b.block(BlockKind::Body, &[]);
    let hit = b.block(BlockKind::Body, &[]);
    let next = b.block(BlockKind::Body, &[]);
    let index = field(&mut b, container.address, 1)?;
    let slots = len(&mut b, index)?;
    let indexed = b.compare(BinaryOp::Ne, slots, zero)?;
    let indexed_block = b.block(BlockKind::Body, &[]);
    let linear = b.block(BlockKind::Body, &[]);
    b.branch(indexed, indexed_block, linear);
    b.at(indexed_block);
    let mask = sub(&mut b, slots, one);
    let start = binary(&mut b, BinaryOp::BitAnd, key_hash, mask);
    let position = b.fresh(u64_layout);
    let buckets = b.block(BlockKind::Body, &[position]);
    let occupied = b.block(BlockKind::Body, &[]);
    let advance = b.block(BlockKind::Body, &[]);
    let found = b.block(BlockKind::Body, &[]);
    b.jump(buckets, &[start]);
    b.at(buckets);
    let slot = b.slot_place(index, position)?;
    let stored = b.read_bits(slot)?;
    let stored = convert(&mut b, stored, Primitive::U64)?;
    let empty = b.compare(BinaryOp::Eq, stored, zero)?;
    b.branch(empty, miss, occupied);
    b.at(occupied);
    let ordinal = sub(&mut b, stored, one);
    let entry = b.slot_place(entries, ordinal)?;
    let candidate = if is_map {
        field(&mut b, entry, 0)?
    } else {
        entry
    };
    let equal = eq(&mut b, candidate, key.address)?;
    b.branch(equal, found, advance);
    b.at(found);
    b.ret(Some(ordinal));
    b.at(advance);
    let inc = b.add(ArithMode::Wrapping, position, one);
    let inc = binary(&mut b, BinaryOp::BitAnd, inc, mask);
    b.jump(buckets, &[inc]);
    b.at(linear);
    b.jump(loop_, &[zero]);
    b.at(loop_);
    let more = b.compare(BinaryOp::Lt, i, count)?;
    b.branch(more, check, miss);
    b.at(check);
    let entry = b.slot_place(entries, i)?;
    let candidate = if is_map {
        field(&mut b, entry, 0)?
    } else {
        entry
    };
    let equal = eq(&mut b, candidate, key.address)?;
    b.branch(equal, hit, next);
    b.at(next);
    let inc = b.add(ArithMode::Wrapping, i, one);
    b.jump(loop_, &[inc]);
    b.at(hit);
    b.ret(Some(i));
    b.at(miss);
    b.ret(Some(count));
    Ok(b.finish())
}
fn get(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let i = probe_call(&mut b)?;
    let address = b.parameters[0].address;
    let entries = field(&mut b, address, 0)?;
    let count = len(&mut b, entries)?;
    let hit = b.compare(BinaryOp::Lt, i, count)?;
    let yes = b.block(BlockKind::Body, &[]);
    let no = b.block(BlockKind::Body, &[]);
    b.branch(hit, yes, no);
    b.at(no);
    let none = option(&mut b, None)?;
    b.ret(Some(none));
    b.at(yes);
    let entry = b.slot_place(entries, i)?;
    let value = field(&mut b, entry, 1)?;
    let failure = b.failure(None);
    let copied = b.copy(value, failure)?;
    let some = option(&mut b, Some(copied))?;
    b.ret(Some(some));
    Ok(b.finish())
}
fn contains(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let i = probe_call(&mut b)?;
    let address = b.parameters[0].address;
    let entries = field(&mut b, address, 0)?;
    let count = len(&mut b, entries)?;
    let hit = b.compare(BinaryOp::Lt, i, count)?;
    b.ret(Some(hit));
    Ok(b.finish())
}
fn cleanup_anchor(b: &mut Body<'_, '_>) -> CleanupAnchorId {
    let id = CleanupAnchorId(b.ed.unit.tables.cleanup_anchors.len() as u32);
    b.ed.unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site: SiteRange { start: 0, end: 0 },
    });
    id
}
pub(super) fn drop_place(b: &mut Body<'_, '_>, address: AddressId) {
    b.forget_held(address);
    let anchor = cleanup_anchor(b);
    let failure = b.failure(None);
    b.push(Inst::Drop {
        address,
        if_live: false,
        anchor,
        failure,
    });
}
fn insert_map(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    insert(cx, true)
}
fn insert_set(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    insert(cx, false)
}
fn insert(cx: &mut Cx<'_>, is_map: bool) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let i = probe_call(&mut b)?;
    let container = b.parameters[0];
    let incoming = b.parameters[if is_map { 2 } else { 1 }];
    let entries = field(&mut b, container.address, 0)?;
    let count = len(&mut b, entries)?;
    let hit = b.compare(BinaryOp::Lt, i, count)?;
    let held = b.held_state();
    let replace_v = b.version(container.slot);
    let append_v = b.version(container.slot);
    let replace = b.block(BlockKind::Body, &[replace_v]);
    let append = b.block(BlockKind::Body, &[append_v]);
    let before = b.reaching(container.slot)?;
    let yes = b.edge(replace, &[before]);
    let no = b.edge(append, &[before]);
    b.terminate(Terminator::Branch {
        condition: hit,
        yes,
        no,
    });
    b.at(replace);
    b.set_reaching(container.slot, replace_v);
    if is_map {
        let entry = b.slot_place(entries, i)?;
        let value = field(&mut b, entry, 1)?;
        let displaced = b.slot(incoming.layout, SlotKind::Displaced);
        b.push(Inst::StorageLive { slot: displaced });
        let old = b.place(AddressBase::Slot(displaced), &[], incoming.layout, None)?;
        let moved = b.take(incoming.address)?;
        let version = b.version(container.slot);
        b.set_reaching(container.slot, version);
        let failure = b.failure(None);
        let displaced_version = b.version(displaced);
        b.set_reaching(displaced, displaced_version);
        b.push(Inst::Replace {
            address: value,
            value: moved,
            displaced,
            version,
            displaced_version,
            if_live: false,
            failure,
        });
        drop_place(&mut b, old);
        b.push(Inst::StorageDead {
            slot: displaced,
            if_live: false,
        });
        // The taken key equals the stored one, which stays.
        let key = b.parameters[1].address;
        drop_place(&mut b, key);
    } else {
        drop_place(&mut b, incoming.address);
    }
    unit(&mut b);
    b.at(append);
    b.set_reaching(container.slot, append_v);
    b.restore_held(&held);
    let element =
        b.cx.element(b.ed.unit.types[b.ed.unit.addresses[entries.index()].ty.index()].layout)?;
    let key = if is_map {
        Some(b.take(b.parameters[1].address)?)
    } else {
        None
    };
    let moved = b.take(incoming.address)?;
    let value = if let Some(key) = key {
        construct(&mut b, element, ConstructorKind::Tuple, &[key, moved])?
    } else {
        moved
    };
    let k = b.cx.type_actual(0)?;
    let element_ty = if is_map {
        let v = b.cx.type_actual(1)?;
        add_type(&mut b.cx.store, TypeKind::Tuple(vec![k, v].into()))
    } else {
        k
    };
    let list_ty = builtin(&mut b, BuiltinType::List, &[element_ty]);
    let unit_ty = add_type(&mut b.cx.store, TypeKind::Tuple(vec![].into()));
    let push = helper(
        &mut b,
        "list.push",
        &[(PassingMode::Mut, list_ty), (PassingMode::Take, element_ty)],
        unit_ty,
        Allocation::MayAllocate,
        vec![GenericArgument::Type(element_ty)],
    )?;
    let list = mut_argument(&mut b, 0, entries)?;
    call(&mut b, push, &[list, argument(1, Passing::Take, value)])?;
    index_call(&mut b, is_map)?;
    unit(&mut b);
    Ok(b.finish())
}
body!(SET_OPEN, "$intrinsic.SetElements_open", set_open);
body!(SET_NEXT, "$intrinsic.SetElements_next", set_next);
body!(MAP_OPEN, "$intrinsic.MapInsertionOrder_open", map_open);
body!(MAP_NEXT, "$intrinsic.MapInsertionOrder_next", map_next);
pub static MAP_DROP: LibraryBody = LibraryBody {
    path: || native_path("owned_map_cursor_drop"),
    build: map_drop,
};

fn set_open(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let source = b.parameters[0];
    let (value, loan) = b.borrow(source.address, false)?;
    let layout = b.cx.layouts.cursor_source();
    let opaque = b.value(layout, Holding::Borrowed(loan));
    b.push(Inst::Recast {
        result: opaque,
        value,
    });
    let zero = b.u64(0)?;
    let layout = b.cx.fn_abi().result.unwrap().layout;
    let symbol = nominal(&mut b, "$intrinsic.Cursor");
    let cursor = construct(
        &mut b,
        layout,
        ConstructorKind::Record(symbol),
        &[opaque, zero, zero],
    )?;
    b.ret(Some(cursor));
    Ok(b.finish())
}
fn set_next(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let cursor = b.parameters[0];
    let source = field(&mut b, cursor.address, 0)?;
    let position = field(&mut b, cursor.address, 1)?;
    let i = b.read_bits(position)?;
    let (opaque, loan) = b.borrow(source, false)?;
    b.ed.unit.values[opaque.index()].holding = Holding::Borrowed(loan);
    let t = b.cx.type_actual(0)?;
    let set = builtin(&mut b, BuiltinType::Set, &[t]);
    let layout = b.cx.layout(set)?;
    let set = b.value(layout, Holding::Borrowed(loan));
    b.push(Inst::Recast {
        result: set,
        value: opaque,
    });
    let entries_layout = b.record(b.cx.payload(layout).unwrap()).fields[0].layout;
    let first_failure = b.ed.unit.tables.failures.len();
    let entries = b.place(
        AddressBase::Value(set),
        &[ProjectionStep::Field(0)],
        entries_layout,
        None,
    )?;
    let (entries_view, entries_loan) = b.borrow(entries, false)?;
    let count = b.fresh(b.layout_of(i));
    b.push(Inst::SeqLen {
        result: count,
        value: entries_view,
    });
    b.end_loan(entries_loan);
    let more = b.compare(BinaryOp::Lt, i, count)?;
    protect(&mut b, first_failure, loan);
    let yes = b.block(BlockKind::Body, &[]);
    let no = b.block(BlockKind::Body, &[]);
    b.branch(more, yes, no);
    b.at(no);
    b.end_loan(loan);
    let none = option(&mut b, None)?;
    b.ret(Some(none));
    b.at(yes);
    let first_failure = b.ed.unit.tables.failures.len();
    let entry = b.slot_place(entries, i)?;
    protect(&mut b, first_failure, loan);
    let one = b.constant(b.layout_of(i), ConstantValue::Integer(1));
    let next = b.add(ArithMode::Wrapping, i, one);
    // Position is plain storage, so update it without a displaced holder.
    let displaced = b.slot(b.layout_of(next), SlotKind::Displaced);
    b.push(Inst::StorageLive { slot: displaced });
    let version = b.version(cursor.slot);
    b.set_reaching(cursor.slot, version);
    let displaced_version = b.version(displaced);
    b.set_reaching(displaced, displaced_version);
    let failure = loan_failure(&mut b, &[loan], None);
    b.push(Inst::Replace {
        address: position,
        value: next,
        displaced,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    b.push(Inst::StorageDead {
        slot: displaced,
        if_live: false,
    });
    // Complete every fallible state write before producing an owned Copy.
    // The source loan covers field 0 and permits the disjoint position write.
    let failure = loan_failure(&mut b, &[loan], None);
    let value = b.copy(entry, failure)?;
    b.end_loan(loan);
    let some = option(&mut b, Some(value))?;
    b.ret(Some(some));
    Ok(b.finish())
}
fn map_open(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let source = b.parameters[0].address;
    let flag = b.bool_layout()?;
    let (view, loan) = b.borrow(source, false)?;
    let root_unique = b.value(flag, Holding::Plain);
    b.push(Inst::IsUnique {
        result: root_unique,
        value: view,
    });
    b.end_loan(loan);
    let entries = field(&mut b, source, 0)?;
    let (entries, loan) = b.borrow(entries, false)?;
    let entries_unique = b.value(flag, Holding::Plain);
    b.push(Inst::IsUnique {
        result: entries_unique,
        value: entries,
    });
    b.end_loan(loan);
    let unique = binary(&mut b, BinaryOp::BitAnd, root_unique, entries_unique);
    let value = b.take(source)?;
    let zero = b.u64(0)?;
    let layout = b.cx.fn_abi().result.unwrap().layout;
    let symbol = nominal(&mut b, "$intrinsic.OwnedMapCursor");
    let cursor = construct(
        &mut b,
        layout,
        ConstructorKind::Record(symbol),
        &[value, zero, unique],
    )?;
    b.ret(Some(cursor));
    Ok(b.finish())
}
fn map_next(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let cursor = b.parameters[0];
    let source = field(&mut b, cursor.address, 0)?;
    let entries = field(&mut b, source, 0)?;
    let position = field(&mut b, cursor.address, 1)?;
    let i = b.read_bits(position)?;
    let count = len(&mut b, entries)?;
    let more = b.compare(BinaryOp::Lt, i, count)?;
    let yes_v = b.version(cursor.slot);
    let no_v = b.version(cursor.slot);
    let yes = b.block(BlockKind::Body, &[yes_v]);
    let no = b.block(BlockKind::Body, &[no_v]);
    let before = b.reaching(cursor.slot)?;
    let yes_edge = b.edge(yes, &[before]);
    let no_edge = b.edge(no, &[before]);
    b.terminate(Terminator::Branch {
        condition: more,
        yes: yes_edge,
        no: no_edge,
    });
    b.at(no);
    b.set_reaching(cursor.slot, no_v);
    let none = option(&mut b, None)?;
    b.ret(Some(none));
    b.at(yes);
    b.set_reaching(cursor.slot, yes_v);
    let entry = b.slot_place(entries, i)?;
    let one = b.u64(1)?;
    let next = b.add(ArithMode::Wrapping, i, one);
    let displaced = b.slot(b.layout_of(next), SlotKind::Displaced);
    b.push(Inst::StorageLive { slot: displaced });
    let version = b.version(cursor.slot);
    b.set_reaching(cursor.slot, version);
    let displaced_version = b.version(displaced);
    b.set_reaching(displaced, displaced_version);
    let failure = b.failure(None);
    b.push(Inst::Replace {
        address: position,
        value: next,
        displaced,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    b.push(Inst::StorageDead {
        slot: displaced,
        if_live: false,
    });
    let unique = field(&mut b, cursor.address, 2)?;
    let unique = b.read_bits(unique)?;
    let entry_layout = b.ed.unit.types[b.ed.unit.addresses[entry.index()].ty.index()].layout;
    let moved = b.block(BlockKind::Body, &[]);
    let shared = b.block(BlockKind::Body, &[]);
    let picked = b.fresh(entry_layout);
    let joined = b.block(BlockKind::Body, &[picked]);
    b.branch(unique, moved, shared);
    b.at(moved);
    let value = b.take(entry)?;
    b.jump(joined, &[value]);
    b.at(shared);
    if b.record(entry_layout).copy == CopyRecipe::NotCopyable {
        b.terminate(Terminator::InvariantFault {
            reason: "a noncopyable map has a shared consuming source".into(),
        });
    } else {
        let failure = b.failure(None);
        let value = b.copy(entry, failure)?;
        b.jump(joined, &[value]);
    }
    b.at(joined);
    let value = picked;
    let some = option(&mut b, Some(value))?;
    b.ret(Some(some));
    Ok(b.finish())
}
fn sub(b: &mut Body<'_, '_>, a: ValueId, c: ValueId) -> ValueId {
    let result = b.fresh(b.layout_of(a));
    let failure = b.failure(None);
    b.push(Inst::Binary {
        result,
        op: BinaryOp::Sub,
        left: a,
        right: c,
        failure,
    });
    result
}
fn map_drop(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let cursor = b.parameters[0];
    let source = field(&mut b, cursor.address, 0)?;
    let source = field(&mut b, source, 0)?;
    let position = field(&mut b, cursor.address, 1)?;
    let unique = field(&mut b, cursor.address, 2)?;
    let unique = b.read_bits(unique)?;
    let drain = b.block(BlockKind::Body, &[]);
    let shared = b.block(BlockKind::Body, &[]);
    b.branch(unique, drain, shared);
    b.at(shared);
    unit(&mut b);
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
    let element =
        b.cx.element(b.ed.unit.types[b.ed.unit.addresses[source.index()].ty.index()].layout)?;
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
            unit(&mut b);
        }
    }
    Ok(b.finish())
}

pub static MAP_INDEX: LibraryBody = LibraryBody {
    path: || native_path("map_index"),
    build: index_update,
};
pub static SET_INDEX: LibraryBody = LibraryBody {
    path: || native_path("set_index"),
    build: index_update,
};

fn binary(b: &mut Body<'_, '_>, op: BinaryOp, a: ValueId, c: ValueId) -> ValueId {
    let result = b.fresh(b.layout_of(a));
    let failure = b.failure(None);
    b.push(Inst::Binary {
        result,
        op,
        left: a,
        right: c,
        failure,
    });
    result
}
fn convert(b: &mut Body<'_, '_>, value: ValueId, primitive: Primitive) -> R<ValueId> {
    let layout = b.cx.primitive(primitive)?;
    let result = b.fresh(layout);
    b.push(Inst::Convert {
        result,
        flag: None,
        operand: value,
        mode: if primitive == Primitive::U64 {
            ConvertMode::Widen
        } else {
            ConvertMode::Truncate
        },
    });
    Ok(result)
}
fn overwrite(b: &mut Body<'_, '_>, address: AddressId, value: ValueId) -> R<()> {
    let displaced = b.slot(b.layout_of(value), SlotKind::Displaced);
    b.push(Inst::StorageLive { slot: displaced });
    let root = b.root(address).unwrap();
    let version = b.version(root);
    b.set_reaching(root, version);
    let displaced_version = b.version(displaced);
    b.set_reaching(displaced, displaced_version);
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
    // Only plain index/position words reach here.
    b.push(Inst::StorageDead {
        slot: displaced,
        if_live: false,
    });
    Ok(())
}
fn index_call(b: &mut Body<'_, '_>, is_map: bool) -> R<()> {
    let k = b.cx.type_actual(0)?;
    let element = if is_map {
        let v = b.cx.type_actual(1)?;
        add_type(&mut b.cx.store, TypeKind::Tuple(vec![k, v].into()))
    } else {
        k
    };
    let entries_type = builtin(b, BuiltinType::List, &[element]);
    let u32_type = add_type(&mut b.cx.store, TypeKind::Primitive(Primitive::U32));
    let index_type = builtin(b, BuiltinType::List, &[u32_type]);
    let result = b.cx.store.push(Type::unit());
    let actuals = b.cx.arguments.clone();
    let direct = helper(
        b,
        if is_map { "map_index" } else { "set_index" },
        &[
            (PassingMode::Borrow, entries_type),
            (PassingMode::Mut, index_type),
        ],
        result,
        Allocation::MayAllocate,
        actuals,
    )?;
    let address = b.parameters[0].address;
    let entries = field(b, address, 0)?;
    let index = field(b, address, 1)?;
    let (entries, loan) = b.borrow(entries, false)?;
    let index = mut_argument(b, 1, index)?;
    call(b, direct, &[argument(0, Passing::Borrow, entries), index])?;
    b.end_loan(loan);
    Ok(())
}
/// Keep the index maintenance and growth off the insertion fast path. Rebuild
/// only when crossing the small-table threshold or reaching 7/8 occupancy.
fn index_update(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let is_map = b.cx.path == native_path("map_index");
    let container = b.parameters[1];
    let entries = b.parameters[0].address;
    let index = container.address;
    let n = len(&mut b, entries)?;
    let capacity = len(&mut b, index)?;
    let eight = b.u64(8)?;
    let one = b.u64(1)?;
    let zero = b.u64(0)?;
    let small = b.compare(BinaryOp::Le, n, eight)?;
    let done = b.block(BlockKind::Body, &[]);
    let check = b.block(BlockKind::Body, &[]);
    b.branch(small, done, check);
    b.at(done);
    unit(&mut b);
    b.at(check);
    let maximum = b.u64(u64::from(u32::MAX))?;
    let too_many = b.compare(BinaryOp::Gt, n, maximum)?;
    let trap = b.block(BlockKind::Body, &[]);
    let admitted = b.block(BlockKind::Body, &[]);
    b.branch(too_many, trap, admitted);
    b.at(trap);
    b.terminate(Terminator::InvariantFault {
        reason: "Map/Set entry count exceeds u32::MAX".into(),
    });
    b.at(admitted);
    let seven = b.u64(7)?;
    let used = binary(&mut b, BinaryOp::Mul, n, eight);
    let limit = binary(&mut b, BinaryOp::Mul, capacity, seven);
    let rebuild = b.compare(BinaryOp::Ge, used, limit)?;
    let rebuild_v = b.version(container.slot);
    let fast_v = b.version(container.slot);
    let rebuild_block = b.block(BlockKind::Body, &[rebuild_v]);
    let fast_block = b.block(BlockKind::Body, &[fast_v]);
    let before = b.reaching(container.slot)?;
    let yes = b.edge(rebuild_block, &[before]);
    let no = b.edge(fast_block, &[before]);
    b.terminate(Terminator::Branch {
        condition: rebuild,
        yes,
        no,
    });
    // An already sized table needs only the newest ordinal.
    b.at(fast_block);
    b.set_reaching(container.slot, fast_v);
    let ordinal = sub(&mut b, n, one);
    index_put(&mut b, entries, index, ordinal, capacity, is_map)?;
    unit(&mut b);
    b.at(rebuild_block);
    b.set_reaching(container.slot, rebuild_v);
    let twice = b.add(ArithMode::Wrapping, capacity, capacity);
    let empty = b.compare(BinaryOp::Eq, capacity, zero)?;
    let word = b.u64_layout()?;
    let wanted = b.fresh(word);
    let capacity_join_v = b.version(container.slot);
    let capacity_join = b.block(BlockKind::Body, &[wanted, capacity_join_v]);
    let fresh = b.block(BlockKind::Body, &[]);
    let doubled = b.block(BlockKind::Body, &[]);
    b.branch(empty, fresh, doubled);
    b.at(fresh);
    let sixteen = b.u64(16)?;
    b.jump(capacity_join, &[sixteen, rebuild_v]);
    b.at(doubled);
    b.jump(capacity_join, &[twice, rebuild_v]);
    b.at(capacity_join);
    b.set_reaching(container.slot, capacity_join_v);
    // Drop the plain old slots; building never touches entries.
    let layout = b.ed.unit.types[b.ed.unit.addresses[index.index()].ty.index()].layout;
    let displaced = b.slot(layout, SlotKind::Displaced);
    b.push(Inst::StorageLive { slot: displaced });
    let old = b.place(AddressBase::Slot(displaced), &[], layout, None)?;
    let sequence = construct(&mut b, layout, ConstructorKind::Sequence, &[])?;
    let version = b.version(container.slot);
    b.set_reaching(container.slot, version);
    let displaced_version = b.version(displaced);
    b.set_reaching(displaced, displaced_version);
    let failure = b.failure(None);
    b.push(Inst::Replace {
        address: index,
        value: sequence,
        displaced,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    drop_place(&mut b, old);
    b.push(Inst::StorageDead {
        slot: displaced,
        if_live: false,
    });
    b.seq_reserve(index, wanted)?;
    let initial_v = b.reaching(container.slot)?;
    let i = b.fresh(word);
    let loop_v = b.version(container.slot);
    let zeros = b.block(BlockKind::Body, &[i, loop_v]);
    let store = b.block(BlockKind::Body, &[]);
    let filled = b.block(BlockKind::Body, &[]);
    b.jump(zeros, &[zero, initial_v]);
    b.at(zeros);
    b.set_reaching(container.slot, loop_v);
    let more = b.compare(BinaryOp::Lt, i, wanted)?;
    b.branch(more, store, filled);
    b.at(store);
    let slot = b.slot_place(index, i)?;
    let u32_layout = b.cx.primitive(Primitive::U32)?;
    let null = b.constant(u32_layout, ConstantValue::Integer(0));
    b.store(slot, null)?;
    let inc = b.add(ArithMode::Wrapping, i, one);
    let after = b.reaching(container.slot)?;
    b.jump(zeros, &[inc, after]);
    b.at(filled);
    b.set_reaching(container.slot, loop_v);
    b.seq_set_len(index, wanted)?;
    let initial_v = b.reaching(container.slot)?;
    let ordinal = b.fresh(word);
    let all_v = b.version(container.slot);
    let all = b.block(BlockKind::Body, &[ordinal, all_v]);
    let put = b.block(BlockKind::Body, &[]);
    let complete = b.block(BlockKind::Body, &[]);
    b.jump(all, &[zero, initial_v]);
    b.at(all);
    b.set_reaching(container.slot, all_v);
    let more = b.compare(BinaryOp::Lt, ordinal, n)?;
    b.branch(more, put, complete);
    b.at(put);
    index_put(&mut b, entries, index, ordinal, wanted, is_map)?;
    let inc = b.add(ArithMode::Wrapping, ordinal, one);
    let after = b.reaching(container.slot)?;
    b.jump(all, &[inc, after]);
    b.at(complete);
    unit(&mut b);
    Ok(b.finish())
}
/// Linear probing visits at most slots.len() buckets: a table is never full.
fn index_put(
    b: &mut Body<'_, '_>,
    entries: AddressId,
    index: AddressId,
    ordinal: ValueId,
    capacity: ValueId,
    is_map: bool,
) -> R<()> {
    let entry = b.slot_place(entries, ordinal)?;
    let key = if is_map { field(b, entry, 0)? } else { entry };
    let hash = hash(b, key)?;
    let one = b.u64(1)?;
    let zero = b.u64(0)?;
    let mask = sub(b, capacity, one);
    let start = binary(b, BinaryOp::BitAnd, hash, mask);
    let word = b.u64_layout()?;
    let position = b.fresh(word);
    let root = b.root(index).unwrap();
    let loop_v = b.version(root);
    let before = b.reaching(root)?;
    let loop_ = b.block(BlockKind::Body, &[position, loop_v]);
    let taken = b.block(BlockKind::Body, &[]);
    let vacant = b.block(BlockKind::Body, &[]);
    b.jump(loop_, &[start, before]);
    b.at(loop_);
    b.set_reaching(root, loop_v);
    let slot = b.slot_place(index, position)?;
    let stored = b.read_bits(slot)?;
    let stored = convert(b, stored, Primitive::U64)?;
    let empty = b.compare(BinaryOp::Eq, stored, zero)?;
    b.branch(empty, vacant, taken);
    b.at(taken);
    let inc = b.add(ArithMode::Wrapping, position, one);
    let inc = binary(b, BinaryOp::BitAnd, inc, mask);
    b.jump(loop_, &[inc, loop_v]);
    b.at(vacant);
    let encoded = b.add(ArithMode::Wrapping, ordinal, one);
    let encoded = convert(b, encoded, Primitive::U32)?;
    overwrite(b, slot, encoded)?;
    Ok(())
}

fn protect(b: &mut Body<'_, '_>, first: usize, loan: LoanId) {
    let end = b.ed.unit.tables.failures.len();
    for i in first..end {
        let old = b.ed.unit.tables.failures[i].cleanup;
        let cleanup = b.block(BlockKind::FailureCleanup, &[]);
        b.ed.code[cleanup.index()].push(Inst::EndLoan {
            loan,
            if_active: false,
        });
        b.ed.unit.terminators[cleanup.index()] = Terminator::Jump(old);
        b.ed.unit.tables.failures[i].cleanup = b.edge(cleanup, &[]);
    }
}
