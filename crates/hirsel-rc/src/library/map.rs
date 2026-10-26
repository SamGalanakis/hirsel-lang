//! Native insertion-ordered maps and sets. Both retain optional entries, a
//! tagged index, cached full hashes and a live count. Keyed reads never mutate.
//! Indexmap 41a87088 src/inner.rs:21-38 supplies the entries/index split;
//! CPython 59942e24 Objects/dictobject.c:35-88,2934-2973 supplies
//! dummy deletion. Set uses unit values, like Rust HashSet. Tombstones preserve insertion order
//! without shifting the suffix; insertion compacts when dead >= live.
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
    Type, TypeId, TypeKind, Uses, Variant,
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
body!(CONTAINS_MAP, "map.contains", contains);
body!(REMOVE_MAP, "map.remove", remove_map);
body!(REMOVE_SET, "set.remove", remove_set);
body!(CONTAINS, "set.contains", contains);
body!(INSERT_MAP, "map.insert", insert_map);
body!(UPSERT_MAP, "map.upsert_with", upsert_with);
body!(UPDATE_MAP, "map.update_with", update_with);
body!(UPDATE_VALUE_MAP, "map.update_value_with", update_value_with);
mod edit;
use edit::{update as update_with, update_value as update_value_with, upsert as upsert_with};
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
pub static PUBLIC_MAP_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("map.len"),
    lower: length,
};
pub static PUBLIC_SET_LEN: InlineLowering = InlineLowering {
    path: || hirsel_syntax::DefPath::core("set.len"),
    lower: length,
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
        values.push(if matches!(cx.record(field).kind, LayoutKind::Scalar(_)) {
            cx.constant(field, ConstantValue::Integer(0))
        } else {
            cx.construct(field, payload(cx, field), ConstructorKind::Sequence, &[])?
        });
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
    if ["map.len", "set.len"]
        .iter()
        .any(|name| call.path == &hirsel_syntax::DefPath::core(name))
    {
        let result = cx.read_field(value, 3)?;
        cx.consumed(value);
        cx.finish(call, result)?;
        return Ok(true);
    }
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
        tracked: false,
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
pub(super) fn option(b: &mut Body<'_, '_>, value: Option<ValueId>) -> R<ValueId> {
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
fn add_type(types: &hirsel_types::TypeInterner, kind: TypeKind) -> TypeId {
    types.intern(Type::with_kind(kind))
}
fn builtin(b: &mut Body<'_, '_>, kind: BuiltinType, types: &[TypeId]) -> TypeId {
    add_type(
        b.cx.types,
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
    // A list operation has a registry row: its callable is the declared
    // signature under `actuals`, as every other requirement of it states.
    if name.starts_with("list.") {
        let path = hirsel_syntax::DefPath::core(name);
        let body = super::body_index(&path).expect("registered list operation");
        let request = Request::operation(b.cx.program, body, &path, actuals)?;
        return b.cx.require(request);
    }
    let callable = add_type(
        b.cx.types,
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
    let path = native_path(name);
    let body = super::body_index(&path).expect("registered helper");
    let request = Request::native(body, &path, actuals, callable);
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
        receipts: None,
        pretransfer_failure: None,
        forward_caller: false,
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
        b.structural_access(b.cx.type_actual(0)?, layout)?;
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
    b.structural_access(b.cx.type_actual(0)?, layout)?;
    b.push(Inst::Hash {
        result,
        value,
        layout,
        failure,
    });
    b.end_loan(loan);
    Ok(result)
}
struct Probe {
    ordinal: ValueId,
    hash: ValueId,
    bucket: ValueId,
}
fn probe_call(b: &mut Body<'_, '_>) -> R<Probe> {
    probe_body(b)
}

fn probe(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    probe_map(cx)
}

/// A lookup also returns the already computed hash and bucket, so insertion,
/// update and removal never search or hash the key a second time.
fn probe_map(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let result = probe_body(&mut b)?;
    probe_return(&mut b, result.ordinal, result.hash, result.bucket)?;
    Ok(b.finish())
}

/// Emit the probe into its caller. Returning three plain block arguments
/// avoids a tuple ABI round trip and lets the backend share the entry count
/// and address calculations with the selected get/edit/remove branch.
fn probe_body(b: &mut Body<'_, '_>) -> R<Probe> {
    let container = b.parameters[0].address;
    let key = b.parameters[1].address;
    let query_hash = hash(b, key)?;
    let entries = field(b, container, 0)?;
    let index = field(b, container, 1)?;
    let capacity = len(b, index)?;
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let word = b.u64_layout()?;
    let result = Probe {
        ordinal: b.fresh(word),
        hash: b.fresh(word),
        bucket: b.fresh(word),
    };
    let done = b.block(
        BlockKind::Body,
        &[result.ordinal, result.hash, result.bucket],
    );
    let miss = b.block(BlockKind::Body, &[]);
    let indexed = b.block(BlockKind::Body, &[]);
    let linear = b.block(BlockKind::Body, &[]);
    let has_index = b.compare(BinaryOp::Ne, capacity, zero)?;
    b.branch(has_index, indexed, linear);
    b.at(indexed);
    let mask = sub(b, capacity, one);
    let tag_mask = b.u64(0xffff_ffff_0000_0000)?;
    let ordinal_mask = b.u64(u64::from(u32::MAX))?;
    let query_tag = binary(b, BinaryOp::BitAnd, query_hash, tag_mask);
    let start = binary(b, BinaryOp::BitAnd, query_hash, mask);
    let position = b.fresh(word);
    let buckets = b.block(BlockKind::Body, &[position]);
    let check_empty = b.block(BlockKind::Body, &[]);
    let compare_key = b.block(BlockKind::Body, &[]);
    let advance = b.block(BlockKind::Body, &[]);
    let found = b.block(BlockKind::Body, &[]);
    let vacant = b.block(BlockKind::Body, &[]);
    b.jump(buckets, &[start]);
    b.at(buckets);
    let at = b.slot_place(index, position)?;
    let stored = b.read_bits(at)?;
    // Like hashbrown's tag-first lookup (1876e4f src/raw/mod.rs:2049),
    // keep the successful candidate path ahead of the empty termination test.
    // Unlike Swiss control bytes, our scalar word also holds an ordinal;
    // ordinal zero excludes both empty buckets and tombstones from candidates.
    let encoded = binary(b, BinaryOp::BitAnd, stored, ordinal_mask);
    let live = b.compare(BinaryOp::Ne, encoded, zero)?;
    let stored_tag = binary(b, BinaryOp::BitAnd, stored, tag_mask);
    let matches = b.compare(BinaryOp::Eq, stored_tag, query_tag)?;
    let candidate = binary(b, BinaryOp::BitAnd, live, matches);
    b.branch(candidate, compare_key, check_empty);
    b.at(check_empty);
    let empty = b.compare(BinaryOp::Eq, stored, zero)?;
    b.branch(empty, vacant, advance);
    b.at(compare_key);
    let ordinal = sub(b, encoded, one);
    let entry = b.slot_place(entries, ordinal)?;
    let pair = live_pair(b, entry)?;
    let candidate = field(b, pair, 0)?;
    let equal = eq(b, candidate, key)?;
    b.branch(equal, found, advance);
    b.at(found);
    b.jump(done, &[ordinal, query_hash, position]);
    b.at(advance);
    let next = b.add(ArithMode::Wrapping, position, one);
    let next = binary(b, BinaryOp::BitAnd, next, mask);
    b.jump(buckets, &[next]);
    b.at(vacant);
    let count = len(b, entries)?;
    b.jump(done, &[count, query_hash, position]);

    // An immutable static map has not acquired a run's hash seed yet. Reads
    // hash those keys without mutation; its first write caches them once.
    b.at(linear);
    let count = len(b, entries)?;
    let hashes = field(b, container, 2)?;
    let hashes_count = len(b, hashes)?;
    let cached = b.compare(BinaryOp::Eq, hashes_count, count)?;
    let ordinal = b.fresh(word);
    let scan = b.block(BlockKind::Body, &[ordinal]);
    let inspect = b.block(BlockKind::Body, &[]);
    let live = b.block(BlockKind::Body, &[]);
    let absent = b.block(BlockKind::Body, &[]);
    let next = b.block(BlockKind::Body, &[]);
    b.jump(scan, &[zero]);
    b.at(scan);
    let more = b.compare(BinaryOp::Lt, ordinal, count)?;
    b.branch(more, inspect, miss);
    b.at(inspect);
    let entry = b.slot_place(entries, ordinal)?;
    let (view, loan) = b.borrow(entry, false)?;
    let edge = b.edge(live, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm { value: 1, edge }],
    );
    let otherwise = b.edge(absent, &[]);
    b.terminate(Terminator::Switch {
        value: view,
        arms,
        otherwise,
    });
    b.at(absent);
    b.end_loan(loan);
    b.jump(next, &[]);
    b.at(live);
    b.end_loan(loan);
    let pair = live_pair(b, entry)?;
    let candidate = field(b, pair, 0)?;
    let saved = b.block(BlockKind::Body, &[]);
    let fresh = b.block(BlockKind::Body, &[]);
    let candidate_hash = b.fresh(word);
    let hashed = b.block(BlockKind::Body, &[candidate_hash]);
    b.branch(cached, saved, fresh);
    b.at(saved);
    let at = b.slot_place(hashes, ordinal)?;
    let value = b.read_bits(at)?;
    b.jump(hashed, &[value]);
    b.at(fresh);
    let value = hash(b, candidate)?;
    b.jump(hashed, &[value]);
    b.at(hashed);
    let same_hash = b.compare(BinaryOp::Eq, candidate_hash, query_hash)?;
    let equal_key = b.block(BlockKind::Body, &[]);
    let matched = b.block(BlockKind::Body, &[]);
    b.branch(same_hash, equal_key, next);
    b.at(equal_key);
    let equal = eq(b, candidate, key)?;
    b.branch(equal, matched, next);
    b.at(matched);
    b.jump(done, &[ordinal, query_hash, zero]);
    b.at(next);
    let increment = b.add(ArithMode::Wrapping, ordinal, one);
    b.jump(scan, &[increment]);
    b.at(miss);
    b.jump(done, &[count, query_hash, zero]);
    b.at(done);
    Ok(result)
}

fn live_pair(b: &mut Body<'_, '_>, entry: AddressId) -> R<AddressId> {
    let layout = b.address_layout(entry);
    let pair = b
        .record(layout)
        .tag
        .as_ref()
        .unwrap()
        .variants
        .iter()
        .find(|v| v.ordinal == 1)
        .unwrap()
        .fields[0]
        .layout;
    b.place(
        AddressBase::Address(entry),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        pair,
        None,
    )
}
fn probe_return(b: &mut Body<'_, '_>, ordinal: ValueId, hash: ValueId, bucket: ValueId) -> R<()> {
    let layout = b.cx.fn_abi().result.unwrap().layout;
    let result = construct(b, layout, ConstructorKind::Tuple, &[ordinal, hash, bucket])?;
    b.ret(Some(result));
    Ok(())
}

fn get(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let probe = probe_call(&mut b)?;
    let i = probe.ordinal;
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
    let entry = live_pair(&mut b, entry)?;
    let value = field(&mut b, entry, 1)?;
    let failure = b.failure(None);
    let copied = b.copy(value, failure)?;
    let some = option(&mut b, Some(copied))?;
    b.ret(Some(some));
    Ok(b.finish())
}
fn contains(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let probe = probe_call(&mut b)?;
    let i = probe.ordinal;
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
        site: SiteRange {
            source: 0,
            start: 0,
            end: 0,
        },
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
    let probe = probe_call(&mut b)?;
    let i = probe.ordinal;
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
        let entry = live_pair(&mut b, entry)?;
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
    if is_map {
        let key = b.parameters[1].address;
        let value = b.parameters[2].address;
        append_map(&mut b, probe.hash, key, value)?;
    } else {
        table::append_set(&mut b, probe.hash, incoming.address)?;
    }
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
    let first = b.read_bits(position)?;
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
    let i = table::advance(&mut b, entries, first)?;
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
    let pair = live_pair(&mut b, entry)?;
    let key = field(&mut b, pair, 0)?;
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
    let value = b.copy(key, failure)?;
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
    let first = b.read_bits(position)?;
    let i = table::advance(&mut b, entries, first)?;
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
    let stored_entry = b.slot_place(entries, i)?;
    let entry = live_pair(&mut b, stored_entry)?;
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
    let before = b.reaching(cursor.slot)?;
    b.branch(unique, moved, shared);
    b.at(moved);
    let stored_layout = b.address_layout(stored_entry);
    let (entry_slot, moved_entry) = temporary(&mut b, stored_layout)?;
    let empty = table::entry_value(&mut b, stored_layout, None)?;
    // Move a whole counted element before projecting its payload. The cursor
    // owns a finalizer, so a partial take must happen in separate storage.
    let whole = b.take(stored_entry)?;
    b.store(moved_entry, whole)?;
    b.hold(entry_slot, moved_entry);
    b.store(stored_entry, empty)?;
    let pair = live_pair(&mut b, moved_entry)?;
    let value = b.take(pair)?;
    b.forget_held(moved_entry);
    b.push(Inst::StorageDead {
        slot: entry_slot,
        if_live: false,
    });
    let some = option(&mut b, Some(value))?;
    b.ret(Some(some));
    b.at(shared);
    b.set_reaching(cursor.slot, before);
    if b.record(entry_layout).copy == CopyRecipe::NotCopyable {
        b.terminate(Terminator::InvariantFault {
            reason: "a noncopyable map has a shared consuming source".into(),
        });
    } else {
        let failure = b.failure(None);
        let value = b.copy(entry, failure)?;
        let some = option(&mut b, Some(value))?;
        b.ret(Some(some));
    }
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
    let unique = field(&mut b, cursor.address, 2)?;
    let unique = b.read_bits(unique)?;
    let drain = b.block(BlockKind::Body, &[]);
    let shared = b.block(BlockKind::Body, &[]);
    b.branch(unique, drain, shared);
    b.at(shared);
    unit(&mut b);
    b.at(drain);
    // Consumed entries and skipped removal tombstones are initialized None
    // cells. Retire them too before publishing an empty sequence.
    let first = b.u64(0)?;
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
            unit(&mut b);
        }
    }
    Ok(b.finish())
}

pub static MAP_INDEX: LibraryBody = LibraryBody {
    path: || native_path("map_index"),
    build: map_index_update,
};
pub static MAP_WARM: LibraryBody = LibraryBody {
    path: || native_path("map_warm"),
    build: table::map_prepare,
};
pub static MAP_PREPARE: LibraryBody = LibraryBody {
    path: || native_path("map_prepare"),
    build: table::map_prepare,
};
body!(
    MAP_ADVANCE,
    "$intrinsic.MapInsertionOrder_advance",
    map_advance
);
use table::map_advance;
pub static SET_INDEX: LibraryBody = LibraryBody {
    path: || native_path("set_index"),
    build: map_index_update,
};
pub static SET_WARM: LibraryBody = LibraryBody {
    path: || native_path("set_warm"),
    build: table::map_prepare,
};
pub static SET_PREPARE: LibraryBody = LibraryBody {
    path: || native_path("set_prepare"),
    build: table::map_prepare,
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

fn temporary(b: &mut Body<'_, '_>, layout: LayoutId) -> R<(SlotId, AddressId)> {
    let slot = b.slot(layout, SlotKind::Temporary);
    b.push(Inst::StorageLive { slot });
    let address = b.place(AddressBase::Slot(slot), &[], layout, None)?;
    Ok((slot, address))
}
// Dense-entry removal for branded SortedMap. The caller
// keeps the returned value owner live through any private metadata repair.
pub(super) fn remove_entry(
    b: &mut Body<'_, '_>,
    entries: AddressId,
    ordinal: ValueId,
    is_map: bool,
) -> R<Option<(SlotId, AddressId)>> {
    let k = b.cx.type_actual(0)?;
    let element_ty = if is_map {
        let v = b.cx.type_actual(1)?;
        add_type(b.cx.types, TypeKind::Tuple(vec![k, v].into()))
    } else {
        k
    };
    let list_ty = builtin(b, BuiltinType::List, &[element_ty]);
    let index_ty = add_type(b.cx.types, TypeKind::Primitive(Primitive::U64));
    let direct = helper(
        b,
        "list.remove",
        &[(PassingMode::Mut, list_ty), (PassingMode::Borrow, index_ty)],
        element_ty,
        Allocation::NoAlloc,
        vec![GenericArgument::Type(element_ty)],
    )?;
    // Preparing an address may fail. Do it before the call produces an owner,
    // then immediately store that owner so every subsequent failure can drop it.
    let layout = b.cx.layout(element_ty)?;
    let (entry_slot, entry) = temporary(b, layout)?;
    let list = mut_argument(b, 0, entries)?;
    let removed = call(b, direct, &[list, argument(1, Passing::Borrow, ordinal)])?.unwrap();
    b.store(entry, removed)?;
    b.hold(entry_slot, entry);
    let moved_value = if is_map {
        // Prepare every address while the tuple is complete. Moving both fields
        // and storing their separate owners cannot fail or leave a partial drop.
        let key = field(b, entry, 0)?;
        let value = field(b, entry, 1)?;
        let key_layout = b.address_layout(key);
        let value_layout = b.address_layout(value);
        let (key_slot, key_owner) = temporary(b, key_layout)?;
        let (value_slot, value_owner) = temporary(b, value_layout)?;
        let value = b.take(value)?;
        let key = b.take(key)?;
        b.forget_held(entry);
        b.push(Inst::StorageDead {
            slot: entry_slot,
            if_live: false,
        });
        b.store(value_owner, value)?;
        b.hold(value_slot, value_owner);
        b.store(key_owner, key)?;
        b.hold(key_slot, key_owner);
        drop_place(b, key_owner);
        b.push(Inst::StorageDead {
            slot: key_slot,
            if_live: false,
        });
        Some((value_slot, value_owner))
    } else {
        drop_place(b, entry);
        b.push(Inst::StorageDead {
            slot: entry_slot,
            if_live: false,
        });
        None
    };
    Ok(moved_value)
}

mod table;
use table::{append_map, map_index_update, remove_map, remove_set};
