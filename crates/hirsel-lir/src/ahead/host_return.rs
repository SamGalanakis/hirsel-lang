//! Post-read proposals are separate from scalar spawn proposals. The prefix
//! runs canonically; only an explicitly named selected-read continuation can
//! enter a private segment. Final objects must still prove every active frame.
use super::*;

/// A root's complete instrumented caller spine. Prefix calls not in `bodies`
/// stay ordinary and cannot arm a private return. Every such call, every
/// unclassified instruction and every spine return is a canonical barrier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostReturnGraph {
    pub graph: ScalarGraph,
    pub spines: BTreeMap<ContentHash, Spine>,
    pub reads: BTreeMap<(ContentHash, CallId), ReadContinuation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spine {
    /// One bit per instruction, in the original Ready instruction arena.
    pub private_instructions: Vec<bool>,
    /// These calls are separately proved whole scalar DAGs.
    pub scalar_calls: BTreeSet<CallId>,
    pub reached_calls: BTreeSet<CallId>,
    /// Saved caller custody at read-reaching calls and ordinary prefix
    /// adapters that can reject before transfer. Prefix adapters remain
    /// behind barriers and are not edges of the private machine-frame DAG.
    pub suspended: BTreeMap<CallId, LiveOwners>,
    /// First unproved instruction reached by a segment, before executing it.
    pub barriers: BTreeSet<usize>,
    /// Inert byte owner consumed at its old position, physically freed at admission.
    pub deferred_releases: BTreeMap<usize, u32>,
}

/// The live-owner image is deliberately descriptive, never a rollback recipe.
/// Private instructions cannot move, release or mutate these owners. Their
/// original saved stack and TaskCtx retain custody on withdrawal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadContinuation {
    pub block: BlockId,
    pub normal: EdgeId,
    pub buffer: SlotId,
    pub buffer_layout: LayoutId,
    pub descriptor: LayoutId,
    pub maximum: u32,
    pub slots: Vec<(SlotId, LayoutId)>,
    pub values: Vec<(ValueId, LayoutId)>,
    pub loans: Vec<LoanId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveOwners {
    pub at: BlockId,
    pub slots: Vec<(SlotId, LayoutId)>,
    pub values: Vec<(ValueId, LayoutId, Holding)>,
    pub loans: Vec<LoanId>,
}

fn callee(unit: &LirUnit<Ready>, call: &CallSite) -> Option<ContentHash> {
    let symbol = match call.callee {
        Callee::Direct(s) | Callee::Bound { code: s, .. } => s,
        _ => return None,
    };
    unit.tables.symbols[symbol.index()].instance
}

/// Find the enclosing private local, refusing heap-root or foreign addresses.
fn slot(unit: &LirUnit<Ready>, address: AddressId) -> Option<SlotId> {
    match unit.addresses[address.index()].base {
        AddressBase::Slot(s) => Some(s),
        AddressBase::Address(a) => slot(unit, a),
        _ => None,
    }
}

/// A caller spine is not itself a scalar kernel. Its resource-owning prefix
/// and suffix are retained, with barriers enclosing every unproved action.
pub fn read_metadata_name(unit: ContentHash, call: CallId, plan: ContentHash) -> String {
    format!("{}_read_{}", super::metadata_name(unit, plan), call.0)
}

fn byte_buffer(table: &LayoutTable, layout: LayoutId) -> Option<LayoutId> {
    let LayoutKind::Block { payload } = table.layout(layout)?.kind else {
        return None;
    };
    let p = table.layout(payload)?;
    let LayoutKind::Sequence { element } = p.kind else {
        return None;
    };
    (matches!(
        table.layout(element)?.kind,
        LayoutKind::Scalar(ScalarKind::U8)
    ) && p.finalizer.is_none()
        && p.drop.is_empty())
    .then_some(payload)
}

pub(super) fn propose(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    target: &TargetSpec,
    source: ContentHash,
    fuel: bool,
    helpers: ContentHash,
) -> Result<HostReturnGraph, Refusal> {
    if target.triple != "x86_64-unknown-linux-gnu"
        || target.cpu != "x86-64"
        || !target.features.is_empty()
        || target.pointer_bytes != 8
        || target.c_convention != crate::layout_types::CConvention::SysV64
    {
        return Err(refused(source, "unsupported host-return target"));
    }
    let by_id: BTreeMap<_, _> = units
        .iter()
        .map(|u| (u.unit().header.instance, u.unit()))
        .collect();
    let mut reaches_read = BTreeSet::new();
    let mut pending = BTreeSet::new();
    fn reaches(
        id: ContentHash,
        units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
        active: &mut BTreeSet<ContentHash>,
        memo: &mut BTreeSet<ContentHash>,
        done: &mut BTreeSet<ContentHash>,
    ) -> Result<bool, Refusal> {
        if done.contains(&id) {
            return Ok(memo.contains(&id));
        }
        if !active.insert(id) {
            return Err(refused(id, "recursive host-return caller spine"));
        }
        if active.len() + done.len() > MAX_UNITS {
            return Err(refused(id, "host-return discovery exceeds 64 units"));
        }
        let unit = units
            .get(&id)
            .ok_or_else(|| refused(id, "missing host-return Ready caller"))?;
        let mut found = false;
        for call in &unit.tables.calls {
            match call.callee {
                Callee::Operation(op) => {
                    found |= unit.tables.operations[op.index()]
                        .definition
                        .is_core("net.read");
                }
                Callee::Direct(_) | Callee::Bound { .. } => {
                    if let Some(next) = callee(unit, call)
                        && units.contains_key(&next)
                    {
                        found |= reaches(next, units, active, memo, done)?;
                    }
                }
                Callee::Indirect(_) => {}
            }
        }
        active.remove(&id);
        done.insert(id);
        if found {
            memo.insert(id);
        }
        Ok(found)
    }
    if !reaches(
        source,
        &by_id,
        &mut BTreeSet::new(),
        &mut reaches_read,
        &mut pending,
    )? {
        return Err(refused(source, "no selected-read continuation"));
    }
    let mut bodies = BTreeMap::new();
    let mut spines = BTreeMap::new();
    let mut reads: BTreeMap<(ContentHash, CallId), ReadContinuation> = BTreeMap::new();
    let mut total = 0usize;
    for id in &reaches_read {
        let unit = by_id[id];
        total += unit.instructions.len() + unit.terminators.len();
        if total > MAX_INSTRUCTIONS {
            return Err(refused(*id, "host-return spine exceeds 4096 instructions"));
        }
        for ty in unit
            .values
            .iter()
            .map(|v| v.ty)
            .chain(unit.slots.iter().map(|s| s.ty))
        {
            let layout = table
                .layout(unit.types[ty.index()].layout)
                .ok_or_else(|| refused(*id, "missing host image"))?;
            if layout.size.storage().is_none_or(|n| n > MAX_IMAGE_BYTES)
                || layout.finalizer.is_some()
            {
                return Err(refused(
                    *id,
                    "unbounded host frame image or user destructor",
                ));
            }
        }
        let mut scalar_calls = BTreeSet::new();
        let mut calls = BTreeSet::new();
        for (i, call) in unit.tables.calls.iter().enumerate() {
            if let Some(next) = callee(unit, call) {
                if reaches_read.contains(&next) {
                    calls.insert(next);
                } else if let Ok(graph) = scalar_graph(units, table, target, next, fuel, helpers) {
                    calls.insert(next);
                    scalar_calls.insert(CallId(i as u32));
                    bodies.extend(graph.bodies);
                }
            }
        }
        let suspended_calls: BTreeSet<_> = unit
            .tables
            .calls
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                (c.pretransfer_failure.is_some()
                    || callee(unit, c).is_some_and(|id| reaches_read.contains(&id)))
                .then_some(CallId(i as u32))
            })
            .collect();
        let suspended = enclosing(unit, &suspended_calls);
        let mut reached_calls = BTreeSet::new();
        let mut private_instructions = vec![false; unit.instructions.len()];
        let mut barriers = BTreeSet::new();
        let mut deferred_releases = BTreeMap::new();
        for (b, term) in unit.terminators.iter().enumerate() {
            let Terminator::Call {
                call,
                normal: Some(normal),
                ..
            } = term
            else {
                continue;
            };
            let site = &unit.tables.calls[call.index()];
            if !matches!(site.callee, Callee::Operation(op) if unit.tables.operations[op.index()].definition.is_core("net.read"))
            {
                continue;
            }
            if reads.keys().any(|(other, _)| other == id) {
                return Err(refused(
                    *id,
                    "multiple selected-read continuations in one frame",
                ));
            }
            let args = site
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap_or(&[]);
            let Some(ArgumentValue::Address { address, version }) =
                args.iter().find(|a| a.parameter == 2).map(|a| a.value)
            else {
                return Err(refused(
                    *id,
                    "read buffer is not an exclusive local address",
                ));
            };
            let buffer =
                slot(unit, address).ok_or_else(|| refused(*id, "read buffer is not local"))?;
            if unit.addresses[address.index()].projection.len != 0
                || matches!(
                    unit.slots[buffer.index()].kind,
                    SlotKind::Capture(_) | SlotKind::Parameter(_)
                )
            {
                return Err(refused(*id, "read buffer escapes its frame"));
            }
            let buffer_layout = unit.types[unit.slots[buffer.index()].ty.index()].layout;
            let descriptor = byte_buffer(table, buffer_layout)
                .ok_or_else(|| refused(*id, "read buffer is not an inert byte list"))?;
            let Some(ArgumentValue::Value(max)) =
                args.iter().find(|a| a.parameter == 3).map(|a| a.value)
            else {
                return Err(refused(*id, "missing read maximum"));
            };
            // The selected bytes and pending custody fit a fixed run-owned image.
            // A nonconstant maximum uses the ordinary return, without a guess.
            let mut constants = BTreeMap::new();
            let mut changed = true;
            while changed {
                changed = false;
                for inst in &unit.instructions {
                    let fact = match *inst {
                        Inst::Const { result, constant } => {
                            match unit.tables.constants[constant.index()].value {
                                ConstantValue::Integer(n) => Some((result, n)),
                                _ => None,
                            }
                        }
                        Inst::Read {
                            result, reaching, ..
                        } => constants.get(&reaching).map(|n| (result, *n)),
                        Inst::Store { value, version, .. } => {
                            constants.get(&value).map(|n| (version, *n))
                        }
                        _ => None,
                    };
                    if let Some((v, n)) = fact
                        && constants.insert(v, n) != Some(n)
                    {
                        changed = true;
                    }
                }
            }
            let maximum = constants
                .get(&max)
                .and_then(|n| u32::try_from(*n).ok())
                .filter(|n| (1..=256).contains(n))
                .ok_or_else(|| refused(*id, "unbounded selected read image"))?;
            let mut owners = BTreeSet::from([version]);
            let mut changed = true;
            while changed {
                changed = false;
                for inst in &unit.instructions {
                    if let Inst::Read {
                        result, address, ..
                    } = *inst
                        && slot(unit, address) == Some(buffer)
                        && unit.addresses[address.index()].projection.len == 0
                    {
                        changed |= owners.insert(result);
                    }
                }
                for edge in &unit.tables.edges {
                    for (arg, param) in edge
                        .arguments
                        .get(&unit.pools.values)
                        .unwrap_or(&[])
                        .iter()
                        .zip(
                            unit.blocks[edge.target.index()]
                                .parameters
                                .get(&unit.pools.values)
                                .unwrap_or(&[]),
                        )
                    {
                        if owners.contains(arg) {
                            changed |= owners.insert(*param);
                        }
                    }
                }
            }
            let live = live_owners(unit, BlockId(b as u32));
            let values = live.values.iter().map(|(v, l, _)| (*v, *l)).collect();
            let slots = live.slots;
            let loans = live.loans;
            reads.insert(
                (*id, *call),
                ReadContinuation {
                    block: BlockId(b as u32),
                    normal: *normal,
                    buffer,
                    buffer_layout,
                    descriptor,
                    maximum,
                    slots,
                    values,
                    loans,
                },
            );
            let mut pending = vec![unit.tables.edges[normal.index()].target];
            let mut seen = BTreeSet::new();
            while let Some(block) = pending.pop() {
                if !seen.insert(block) {
                    continue;
                }
                let range = unit.blocks[block.index()].instructions;
                let mut ended = false;
                for i in range.start as usize..(range.start + range.len) as usize {
                    let inst = &unit.instructions[i];
                    if let Inst::Release {
                        value,
                        layout,
                        live: MaskRef::All,
                        ..
                    } = *inst
                        && layout == descriptor
                        && owners.contains(&value)
                    {
                        let index = deferred_releases.len() as u32;
                        if index >= 8 {
                            return Err(refused(*id, "too many deferred inert cleanups"));
                        }
                        deferred_releases.entry(i).or_insert(index);
                        private_instructions[i] = true;
                    } else if safe_instruction(unit, table, inst, buffer, &owners) {
                        private_instructions[i] = true;
                    } else {
                        barriers.insert(i);
                        ended = true;
                        break;
                    }
                }
                if ended {
                    continue;
                }
                let term = &unit.terminators[block.index()];
                match term {
                    Terminator::Call { call, .. } if scalar_calls.contains(call) => {
                        reached_calls.insert(*call);
                    }
                    Terminator::Jump(_) | Terminator::Branch { .. } | Terminator::Switch { .. } => {
                    }
                    _ => continue, // Call/return/failure lowering inserts its barrier.
                }
                pending.extend(
                    term.normal_edges(&unit.pools)
                        .into_iter()
                        .map(|e| unit.tables.edges[e.index()].target),
                );
            }
            // A fixed pending slot cannot be consumed twice before a barrier.
            // Refuse caller-spine CFG cycles; scalar callee CFG loops remain allowed.
            let succ: Vec<Vec<usize>> = unit
                .terminators
                .iter()
                .map(|t| {
                    t.normal_edges(&unit.pools)
                        .iter()
                        .map(|e| unit.tables.edges[e.index()].target.index())
                        .collect()
                })
                .collect();
            if !hirsel_graph::feedback_edges(&succ, unit.entry.index()).is_empty() {
                return Err(refused(*id, "cyclic selected-read caller frame"));
            }
        }
        bodies.insert(
            *id,
            ScalarBody {
                body: unit.digest(),
                calls,
                fuel_points: crate::fuel::fuel_points(unit),
            },
        );
        spines.insert(
            *id,
            Spine {
                private_instructions,
                scalar_calls,
                reached_calls,
                suspended,
                barriers,
                deferred_releases,
            },
        );
    }
    if reads.is_empty() {
        return Err(refused(source, "missing selected read entry"));
    }
    if bodies.len() > MAX_UNITS {
        return Err(refused(source, "oversize host-return DAG"));
    }
    let identity = ContentHash::of_parts(
        "hirsel.ahead.host-return.v1",
        [
            format!("{bodies:?}{spines:?}{reads:?}").as_bytes(),
            table.digest().0.as_slice(),
            target.digest().0.as_slice(),
            helpers.0.as_slice(),
            &[u8::from(fuel)],
        ],
    );
    Ok(HostReturnGraph {
        graph: ScalarGraph {
            root: source,
            bodies,
            identity,
        },
        spines,
        reads,
    })
}

fn safe_instruction(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    inst: &Inst,
    buffer: SlotId,
    owners: &BTreeSet<ValueId>,
) -> bool {
    let scalar = |v: ValueId| {
        let ty = &u.types[u.values[v.index()].ty.index()];
        matches!(
            ty.repr,
            ValueRepr::Unit
                | ValueRepr::Scalar(_)
                | ValueRepr::Aggregate
                | ValueRepr::InitMask { .. }
        ) && scalar_image(table, ty.layout)
    };
    let scalar_address = |a: AddressId| {
        let info = &u.addresses[a.index()];
        let root = match slot(u, a) {
            Some(s) => scalar_image(table, u.types[u.slots[s.index()].ty.index()].layout),
            None => matches!(info.base, AddressBase::Value(v) if scalar(v)),
        };
        root && info
            .projection
            .get(&u.pools.projections)
            .unwrap_or(&[])
            .iter()
            .all(|p| {
                matches!(
                    p,
                    ProjectionStep::Field(_) | ProjectionStep::VariantField { .. }
                )
            })
    };
    let byte_address = |a: AddressId| {
        slot(u, a) == Some(buffer)
            && scalar_image(table, u.types[u.addresses[a.index()].ty.index()].layout)
            && u.addresses[a.index()]
                .projection
                .get(&u.pools.projections)
                .unwrap_or(&[])
                .iter()
                .all(|p| matches!(p, ProjectionStep::Payload | ProjectionStep::Index(_)))
    };
    match *inst {
        Inst::Const { constant, .. } => matches!(
            u.tables.constants[constant.index()].value,
            ConstantValue::Unit | ConstantValue::Bool(_) | ConstantValue::Integer(_)
        ),
        Inst::Read {
            result,
            address,
            mode,
            ..
        } => {
            (scalar(result)
                && (scalar_address(address) || (mode == ReadMode::Bits && byte_address(address))))
                || (owners.contains(&result) && slot(u, address) == Some(buffer))
        }
        Inst::Address { address, .. } => scalar_address(address) || byte_address(address),
        Inst::Store { address, value, .. } => {
            scalar(value)
                && scalar_address(address)
                && slot(u, address).is_some_and(|s| {
                    !matches!(
                        u.slots[s.index()].kind,
                        SlotKind::Capture(_) | SlotKind::Parameter(_)
                    )
                })
        }
        Inst::Unary {
            operand, result, ..
        }
        | Inst::Convert {
            operand, result, ..
        } => scalar(operand) && scalar(result),
        Inst::Binary {
            left,
            right,
            result,
            ..
        }
        | Inst::IntArith {
            left,
            right,
            result,
            ..
        } => scalar(left) && scalar(right) && scalar(result),
        Inst::SeqLen { value, .. } => owners.contains(&value),
        Inst::Construct { construct, .. } => {
            scalar_image(table, u.tables.constructs[construct.index()].layout)
        }
        Inst::Release { layout, .. } => scalar_image(table, layout),
        Inst::Check { condition, .. } => scalar(condition),
        // These exact task-local leaves have the published eight-byte call
        // extent in the helper/frame proof and publish no failure themselves.
        Inst::FrameEnter { .. }
        | Inst::FrameLeave
        | Inst::StorageLive { .. }
        | Inst::StorageDead { .. }
        | Inst::EndScope { .. }
        | Inst::EndLoan { .. }
        | Inst::BindOrigin { .. }
        | Inst::EndOrigin { .. } => true,
        Inst::BeginLoan { mutable: false, .. } => true,
        _ => false,
    }
}

/// At a caller suspension/selected return, union normal and cleanup liveness.
/// Storage and loan sets are forward may-live facts; Ready's existing masks
/// select initialized owners. Descriptive supersets never authorize an access.
fn live_owners(u: &LirUnit<Ready>, at: BlockId) -> LiveOwners {
    let n = u.blocks.len();
    let mut storage: Vec<BTreeSet<SlotId>> = vec![BTreeSet::new(); n];
    let mut loans: Vec<BTreeSet<LoanId>> = vec![BTreeSet::new(); n];
    storage[u.entry.index()].extend(u.slots.iter().enumerate().filter_map(|(i, s)| {
        matches!(
            s.kind,
            SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReturnSlot
        )
        .then_some(SlotId(i as u32))
    }));
    let mut reached = BTreeSet::from([u.entry]);
    let mut pending = vec![u.entry];
    let mut at_slots = BTreeSet::new();
    let mut at_loans = BTreeSet::new();
    while let Some(b) = pending.pop() {
        let mut slots = storage[b.index()].clone();
        let mut active = loans[b.index()].clone();
        let mut outgoing = Vec::new();
        for inst in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
            if let Some(f) = inst.failure() {
                outgoing.push((
                    u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target,
                    slots.clone(),
                    active.clone(),
                ));
            }
            match *inst {
                Inst::StorageLive { slot } => {
                    slots.insert(slot);
                }
                Inst::StorageDead { slot, .. } => {
                    slots.remove(&slot);
                }
                Inst::BeginLoan { loan, .. } => {
                    active.insert(loan);
                }
                Inst::EndLoan { loan, .. } => {
                    active.remove(&loan);
                }
                _ => {}
            }
        }
        if b == at {
            at_slots.extend(slots.iter().copied());
            at_loans.extend(active.iter().copied());
        }
        let term = &u.terminators[b.index()];
        let mut edges = term.normal_edges(&u.pools);
        for f in term.failure_edges(&u.tables) {
            edges.push(u.tables.failures[f.index()].cleanup);
        }
        outgoing.extend(edges.into_iter().map(|e| {
            (
                u.tables.edges[e.index()].target,
                slots.clone(),
                active.clone(),
            )
        }));
        for (next, slots, active) in outgoing {
            let before = (storage[next.index()].len(), loans[next.index()].len());
            storage[next.index()].extend(slots);
            loans[next.index()].extend(active);
            if reached.insert(next)
                || before != (storage[next.index()].len(), loans[next.index()].len())
            {
                pending.push(next);
            }
        }
    }
    let live = crate::liveness::liveness(u);
    LiveOwners {
        at,
        slots: at_slots
            .into_iter()
            .map(|s| (s, u.types[u.slots[s.index()].ty.index()].layout))
            .collect(),
        values: live
            .successor_needs(u, at)
            .iter()
            .map(|v| {
                (
                    v,
                    u.types[u.values[v.index()].ty.index()].layout,
                    u.values[v.index()].holding,
                )
            })
            .collect(),
        loans: at_loans.into_iter().collect(),
    }
}

pub(super) fn enclosing(
    u: &LirUnit<Ready>,
    calls: &BTreeSet<CallId>,
) -> BTreeMap<CallId, LiveOwners> {
    u.terminators
        .iter()
        .enumerate()
        .filter_map(|(i, t)| match t {
            Terminator::Call { call, .. } if calls.contains(call) => {
                Some((*call, live_owners(u, BlockId(i as u32))))
            }
            _ => None,
        })
        .collect()
}

pub(super) fn wrapper_spine(u: &LirUnit<Ready>) -> Spine {
    Spine {
        private_instructions: vec![false; u.instructions.len()],
        scalar_calls: BTreeSet::new(),
        reached_calls: BTreeSet::new(),
        suspended: enclosing(u, &BTreeSet::from([CallId(0)])),
        barriers: BTreeSet::new(),
        deferred_releases: BTreeMap::new(),
    }
}
