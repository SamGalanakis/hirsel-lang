//! Compiler-owned proposals for private scalar execution. A proposal is not a
//! machine certificate and must never by itself authorize a runtime entry.
//!
//! The graph is built before object partitioning. In particular, the older
//! op-free spawn hint (which admits recursive SCCs) is not used here.
use std::collections::{BTreeMap, BTreeSet};

use hirsel_syntax::ContentHash;

use crate::ir::*;
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind, SizeSpec, TargetSpec};
use crate::stage::{Ready, ValidatedUnit};

mod cpu_worker;
mod host_return;
mod plan;
pub use cpu_worker::{
    CpuMapperBinding, CpuWorkerGraph, CpuWorkerProtocol, cpu_mapper_candidates,
    validate_cpu_workers,
};
pub use host_return::{HostReturnGraph, ReadContinuation, Spine, read_metadata_name};
pub use plan::{
    AheadPlan, CaptureRecipe, RootPlan, alternate_name, alternate_resume_name, environments,
    metadata_name, prospective,
};

pub const MAX_UNITS: usize = 64;
pub const MAX_INSTRUCTIONS: usize = 4096;
pub const MAX_IMAGE_BYTES: u64 = 256;
pub const MAX_STACK_BYTES: u64 = 65536;

/// Identity includes the full dependency body, not merely its instance name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScalarBody {
    pub body: ContentHash,
    pub calls: BTreeSet<ContentHash>,
    pub fuel_points: Vec<bool>,
}

/// The complete scalar call closure, still awaiting final machine evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScalarGraph {
    pub root: ContentHash,
    pub bodies: BTreeMap<ContentHash, ScalarBody>,
    pub identity: ContentHash,
}

/// Refusals are optimization outcomes. They do not reject ordinary execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    pub instance: ContentHash,
    pub reason: String,
}

fn refused(instance: ContentHash, reason: impl Into<String>) -> Refusal {
    Refusal {
        instance,
        reason: reason.into(),
    }
}

/// Whether a bounded image contains only inert integer/boolean bits. Walk
/// inline layouts only; a cycle is invalid evidence, never a recursive proof.
pub fn scalar_image(table: &LayoutTable, id: LayoutId) -> bool {
    fn visit(table: &LayoutTable, id: LayoutId, active: &mut BTreeSet<LayoutId>) -> bool {
        if !active.insert(id) {
            return false;
        }
        let admitted = table.layout(id).is_some_and(|layout| {
            matches!(layout.size, SizeSpec::Fixed(n) if n <= MAX_IMAGE_BYTES)
                && layout.align <= 16
                && layout.drop.is_empty()
                && layout.scan.is_empty()
                && layout.finalizer.is_none()
                && match &layout.kind {
                    LayoutKind::Unit => true,
                    LayoutKind::Scalar(k) => matches!(
                        k,
                        ScalarKind::Bool
                            | ScalarKind::U8
                            | ScalarKind::U16
                            | ScalarKind::U32
                            | ScalarKind::U64
                            | ScalarKind::I8
                            | ScalarKind::I16
                            | ScalarKind::I32
                            | ScalarKind::I64
                    ),
                    LayoutKind::Aggregate
                    | LayoutKind::Variant
                    | LayoutKind::Environment { .. } => {
                        layout.fields.iter().all(|f| visit(table, f.layout, active))
                            && layout
                                .tag
                                .iter()
                                .flat_map(|t| &t.variants)
                                .flat_map(|v| &v.fields)
                                .all(|f| visit(table, f.layout, active))
                    }
                    LayoutKind::Inline { element, count } => {
                        *count <= MAX_IMAGE_BYTES && visit(table, *element, active)
                    }
                    _ => false,
                }
        });
        active.remove(&id);
        admitted
    }
    visit(table, id, &mut BTreeSet::new())
}

/// Analyze a scalar source body and all its reachable direct/Bound bodies.
/// Missing Ready content is refused, even when a symbol has an op-free hint.
/// `helper_identity` binds the exact runtime assembly/record contract; callers
/// must also bind emission options and final object/frame evidence later.
pub fn scalar_graph(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    target: &TargetSpec,
    root: ContentHash,
    logical_fuel: bool,
    helper_identity: ContentHash,
) -> Result<ScalarGraph, Refusal> {
    if target.triple != "x86_64-unknown-linux-gnu"
        || target.c_convention != crate::layout_types::CConvention::SysV64
        || target.pointer_bytes != 8
        || target.cpu != "x86-64"
        || !target.features.is_empty()
    {
        return Err(refused(root, "unsupported ahead target"));
    }
    let mut graph = Walker {
        units: units
            .iter()
            .map(|u| (u.unit().header.instance, u.unit()))
            .collect(),
        table,
        active: BTreeSet::new(),
        bodies: BTreeMap::new(),
        instructions: 0,
    };
    if graph.units.len() != units.len() {
        return Err(refused(root, "duplicate Ready instance"));
    }
    graph.visit(root)?;
    // These encodings have fixed widths or an explicit count, so concatenation
    // cannot alias a different graph. Unit digests bind instructions, layouts,
    // call ABI and exact source/failure anchors.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&root.0);
    bytes.extend_from_slice(&target.digest().0);
    bytes.extend_from_slice(&table.digest().0);
    bytes.extend_from_slice(&helper_identity.0);
    bytes.push(u8::from(logical_fuel));
    bytes.extend_from_slice(&(graph.bodies.len() as u32).to_le_bytes());
    for (id, body) in &graph.bodies {
        bytes.extend_from_slice(&id.0);
        bytes.extend_from_slice(&body.body.0);
        bytes.extend_from_slice(&(body.fuel_points.len() as u32).to_le_bytes());
        bytes.extend(body.fuel_points.iter().map(|v| u8::from(*v)));
    }
    Ok(ScalarGraph {
        root,
        bodies: graph.bodies,
        identity: ContentHash::of_parts("hirsel.ahead.scalar-graph.v1", [bytes.as_slice()]),
    })
}

struct Walker<'a> {
    units: BTreeMap<ContentHash, &'a LirUnit<Ready>>,
    table: &'a LayoutTable,
    active: BTreeSet<ContentHash>,
    bodies: BTreeMap<ContentHash, ScalarBody>,
    instructions: usize,
}

impl Walker<'_> {
    fn visit(&mut self, id: ContentHash) -> Result<(), Refusal> {
        if self.bodies.contains_key(&id) {
            return Ok(());
        }
        if !self.active.insert(id) {
            return Err(refused(id, "recursive call SCC"));
        }
        if self.active.len() + self.bodies.len() > MAX_UNITS {
            return Err(refused(id, "ahead graph exceeds 64 units"));
        }
        let unit = *self
            .units
            .get(&id)
            .ok_or_else(|| refused(id, "missing Ready body"))?;
        self.instructions = self
            .instructions
            .checked_add(
                unit.instructions
                    .len()
                    .checked_add(unit.terminators.len())
                    .ok_or_else(|| refused(id, "instruction count overflow"))?,
            )
            .ok_or_else(|| refused(id, "instruction count overflow"))?;
        if self.instructions > MAX_INSTRUCTIONS {
            return Err(refused(id, "ahead graph exceeds 4096 instructions"));
        }
        for slot in &unit.slots {
            if matches!(slot.kind, SlotKind::LoopCarried(_)) {
                return Err(refused(id, "normalized recursive source body"));
            }
        }
        // Ready can retain unused type-table rows from earlier passes (e.g.
        // reuse's Token type). Only actual values/slots constitute live images.
        for ty in unit
            .values
            .iter()
            .map(|v| v.ty)
            .chain(unit.slots.iter().map(|s| s.ty))
        {
            let ty = &unit.types[ty.index()];
            let layout = ty.layout;
            if !matches!(
                ty.repr,
                ValueRepr::Unit
                    | ValueRepr::Scalar(_)
                    | ValueRepr::Aggregate
                    | ValueRepr::InitMask { .. }
            ) || !scalar_image(self.table, layout)
            {
                return Err(refused(id, format!("non-scalar layout {}", layout.0)));
            }
        }
        let abi = &self.table.fn_abis[unit.header.abi.0 as usize];
        if abi
            .parameters
            .iter()
            .any(|p| !scalar_image(self.table, p.layout))
            || abi
                .result
                .as_ref()
                .is_some_and(|r| !scalar_image(self.table, r.layout))
        {
            return Err(refused(id, "non-scalar entry ABI"));
        }
        fn root_slot(u: &LirUnit<Ready>, a: AddressId) -> Option<SlotId> {
            match u.addresses[a.index()].base {
                AddressBase::Slot(s) => Some(s),
                AddressBase::Address(a) => root_slot(u, a),
                _ => None,
            }
        }
        for inst in &unit.instructions {
            if let Inst::Store { address, .. }
            | Inst::Replace { address, .. }
            | Inst::BeginLoan {
                address,
                mutable: true,
                ..
            } = inst
                && root_slot(unit, *address)
                    .is_none_or(|s| matches!(unit.slots[s.index()].kind, SlotKind::Capture(_)))
            {
                return Err(refused(id, "mutable scalar captures"));
            }
            let safe = match inst {
                Inst::Const { constant, .. } => matches!(
                    unit.tables.constants[constant.index()].value,
                    ConstantValue::Unit | ConstantValue::Bool(_) | ConstantValue::Integer(_)
                ),
                // Exact native depth leaves are bound by the machine helper
                // identity and extent; they never publish a failure themselves.
                Inst::FrameEnter { .. }
                | Inst::FrameLeave { .. }
                | Inst::Unary { .. }
                | Inst::Binary { .. }
                | Inst::IntArith { .. }
                | Inst::Check { .. }
                | Inst::StorageLive { .. }
                | Inst::StorageDead { .. }
                | Inst::EndScope { .. }
                | Inst::Address { .. }
                | Inst::Read { .. }
                | Inst::CopyValue { .. }
                | Inst::Store { .. }
                | Inst::Replace { .. }
                | Inst::UpdateMask { .. }
                | Inst::BeginLoan { .. }
                | Inst::EndLoan { .. }
                | Inst::BindOrigin { .. }
                | Inst::BindFields { .. }
                | Inst::EndOrigin { .. } => true,
                Inst::Convert { mode, .. } => matches!(
                    mode,
                    ConvertMode::Widen | ConvertMode::Truncate | ConvertMode::TryNarrow
                ),
                Inst::Construct { construct, .. } => {
                    scalar_image(self.table, unit.tables.constructs[construct.index()].layout)
                }
                _ => false,
            };
            if !safe {
                return Err(refused(
                    id,
                    format!("unclassified scalar instruction {:?}", inst.opcode()),
                ));
            }
        }
        // Address roots are either private scalar slots or scalar SSA spills.
        // No heap/foreign/callable/handle address is admitted by the type proof.
        for address in &unit.addresses {
            if address
                .projection
                .get(&unit.pools.projections)
                .unwrap_or(&[])
                .iter()
                .any(|p| {
                    !matches!(
                        p,
                        ProjectionStep::Field(_) | ProjectionStep::VariantField { .. }
                    )
                })
            {
                return Err(refused(id, "non-inline scalar address projection"));
            }
        }
        let mut calls = BTreeSet::new();
        for term in &unit.terminators {
            match term {
                Terminator::Call { call, .. } => {
                    let call = &unit.tables.calls[call.index()];
                    if call
                        .arguments
                        .get(&unit.pools.call_arguments)
                        .unwrap_or(&[])
                        .iter()
                        .any(|a| {
                            a.passing == hirsel_core_contract::Passing::Mut
                                && match a.value {
                                    ArgumentValue::Address { address, .. } => {
                                        root_slot(unit, address).is_none_or(|s| {
                                            matches!(
                                                unit.slots[s.index()].kind,
                                                SlotKind::Capture(_)
                                            )
                                        })
                                    }
                                    _ => true,
                                }
                        })
                    {
                        return Err(refused(id, "nonlocal/mutable scalar capture argument"));
                    }

                    let symbol = match call.callee {
                        Callee::Direct(s) | Callee::Bound { code: s, .. } => {
                            &unit.tables.symbols[s.index()]
                        }
                        Callee::Indirect(_) => return Err(refused(id, "indirect scalar call")),
                        Callee::Operation(_) => return Err(refused(id, "opaque scalar operation")),
                    };
                    let callee = symbol
                        .instance
                        .ok_or_else(|| refused(id, "unknown callee instance"))?;
                    let reached = self
                        .units
                        .get(&callee)
                        .ok_or_else(|| refused(callee, "missing Ready callee"))?;
                    if reached.header.definition != symbol.definition
                        || reached.header.abi != call.abi
                    {
                        return Err(refused(id, "callee code/ABI mismatch"));
                    }
                    calls.insert(callee);
                }
                Terminator::Jump(_)
                | Terminator::Branch { .. }
                | Terminator::Switch { .. }
                | Terminator::Return(_)
                | Terminator::Raise(_)
                | Terminator::Propagate
                | Terminator::Unreachable => {}
                _ => return Err(refused(id, "unclassified scalar terminator")),
            }
        }
        for callee in &calls {
            self.visit(*callee)?;
        }
        self.active.remove(&id);
        self.bodies.insert(
            id,
            ScalarBody {
                body: unit.digest(),
                calls,
                fuel_points: crate::fuel::fuel_points(unit),
            },
        );
        Ok(())
    }
}

/// Compose exact final frame extents along the already proven DAG. Include
/// startup separately and the larger of the two fixed helper call extents.
/// Absence, overflow and oversize are all refusal, never a guessed reserve.
pub fn stack_bound(
    graph: &ScalarGraph,
    frames: &BTreeMap<ContentHash, u64>,
    startup: u64,
    getter: u64,
    handoff: u64,
) -> Option<u64> {
    stack_bound_with_depth(graph, frames, startup, getter, handoff, 0)
}

/// Compose the same final frame proof with the exact certified depth-leaf
/// footprint. Callers bind the helper implementation into the plan identity.
pub fn stack_bound_with_depth(
    graph: &ScalarGraph,
    frames: &BTreeMap<ContentHash, u64>,
    startup: u64,
    getter: u64,
    handoff: u64,
    depth: u64,
) -> Option<u64> {
    fn visit(
        id: ContentHash,
        graph: &ScalarGraph,
        frames: &BTreeMap<ContentHash, u64>,
        helper: u64,
        active: &mut BTreeSet<ContentHash>,
        memo: &mut BTreeMap<ContentHash, u64>,
    ) -> Option<u64> {
        if let Some(bound) = memo.get(&id) {
            return Some(*bound);
        }
        if !active.insert(id) {
            return None;
        }
        let body = graph.bodies.get(&id)?;
        let mut child = helper;
        for callee in &body.calls {
            child = child.max(visit(*callee, graph, frames, helper, active, memo)?);
        }
        let bound = frames.get(&id)?.checked_add(child)?;
        active.remove(&id);
        memo.insert(id, bound);
        Some(bound)
    }
    let bound = startup.checked_add(visit(
        graph.root,
        graph,
        frames,
        getter.max(handoff).max(depth),
        &mut BTreeSet::new(),
        &mut BTreeMap::new(),
    )?)?;
    (bound <= MAX_STACK_BYTES).then_some(bound)
}

/// Final-object sidecar. The plan binds complete bodies, layouts, target,
/// original fuel points and helper implementation; object binds exact code.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct MachineEvidence {
    pub plan: ContentHash,
    pub object: ContentHash,
    pub options: ContentHash,
    pub frames: Vec<(ContentHash, u64)>,
    pub bounds: Vec<(ContentHash, u64)>,
}
hirsel_codec::snapshot_struct!(MachineEvidence {
    plan,
    object,
    options,
    frames,
    bounds
});
