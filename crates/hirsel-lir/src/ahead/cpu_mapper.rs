//! Allocating CPU mapper proposals. These are not executable certificates:
//! emission must discharge every handoff and prove the final machine frames.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuMapperGraph {
    pub growth: BTreeMap<(ContentHash, usize), CpuGrowth>,
    pub regional: BTreeMap<(ContentHash, usize), CpuRegional>,
    pub graph: ScalarGraph,
    pub helpers: ContentHash,
    pub spines: BTreeMap<ContentHash, Spine>,
    /// Every heap construction must use the ordinal-owned allocation handoff.
    pub allocations: BTreeMap<(ContentHash, usize), ConstructId>,
    /// Ownership operations require canonical cleanup until summary retirement
    /// and withdrawal can retain their exact saved frames.
    pub cleanup: BTreeSet<(ContentHash, usize)>,
}

/// A saved flat-sequence reservation. Strict C proves the mutable place and
/// live old owner; the checker reconstructs the element arithmetic separately.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuGrowth {
    pub address: AddressId,
    pub capacity: ValueId,
    pub layout: LayoutId,
    pub element: LayoutId,
    pub size: u64,
    pub align: u32,
}

/// Headerless storage in the exact compiler-local region, with its ordinary
/// descriptor, cleanup policy and alignment independently reconstructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuRegional {
    pub construct: ConstructId,
    pub region: RegionInstanceId,
    pub layout: LayoutId,
    pub align: u32,
}

/// Produce the bounded, nonrecursive Ready graph. The separate checker below
/// validates proposals against actual bodies without invoking this producer.
pub fn cpu_mapper_graph(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    target: &TargetSpec,
    root: ContentHash,
    fuel: bool,
    helpers: ContentHash,
) -> Result<CpuMapperGraph, Refusal> {
    if !supported(target) {
        return Err(refused(root, "unsupported CPU mapper target"));
    }
    let by_id: BTreeMap<_, _> = units
        .iter()
        .map(|u| (u.unit().header.instance, u.unit()))
        .collect();
    if by_id.len() != units.len() {
        return Err(refused(root, "duplicate CPU mapper Ready instance"));
    }
    let mut proposal = CpuMapperGraph {
        graph: ScalarGraph {
            root,
            bodies: BTreeMap::new(),
            identity: ContentHash::default(),
        },
        helpers,
        spines: BTreeMap::new(),
        growth: BTreeMap::new(),
        regional: BTreeMap::new(),
        allocations: BTreeMap::new(),
        cleanup: BTreeSet::new(),
    };
    fn visit(
        id: ContentHash,
        units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
        table: &LayoutTable,
        active: &mut BTreeSet<ContentHash>,
        instructions: &mut usize,
        out: &mut CpuMapperGraph,
    ) -> Result<(), Refusal> {
        if out.graph.bodies.contains_key(&id) {
            return Ok(());
        }
        let no = |s| refused(id, s);
        if !active.insert(id) {
            return Err(no("recursive CPU mapper graph"));
        }
        if active.len() + out.graph.bodies.len() > MAX_UNITS {
            return Err(no("CPU mapper graph exceeds 64 units"));
        }
        let u = *units
            .get(&id)
            .ok_or_else(|| no("missing CPU mapper Ready body"))?;
        *instructions = instructions
            .saturating_add(u.instructions.len())
            .saturating_add(u.terminators.len());
        if *instructions > MAX_INSTRUCTIONS {
            return Err(no("CPU mapper graph exceeds 4096 instructions"));
        }
        if !body_custody(u, table) {
            return Err(no("unproved CPU mapper data custody"));
        }
        for (index, inst) in u.instructions.iter().enumerate() {
            match instruction(u, table, inst).ok_or_else(|| {
                refused(
                    id,
                    format!("CPU mapper needs a handoff for {:?}", inst.opcode()),
                )
            })? {
                Action::Private => {}
                Action::Allocate(construct) => {
                    out.allocations.insert((id, index), construct);
                }
                Action::Grow(recipe) => {
                    out.growth.insert((id, index), recipe);
                }
                Action::Regional(recipe) => {
                    out.regional.insert((id, index), recipe);
                }
                Action::Cleanup => {
                    out.cleanup.insert((id, index));
                }
            }
        }
        let mut calls = BTreeSet::new();
        for term in &u.terminators {
            if let Terminator::Call { call, .. } = term {
                let callee = direct_call(u, table, units, *call)
                    .ok_or_else(|| no("unproved CPU mapper call or destination"))?;
                calls.insert(callee);
            } else if !matches!(
                term,
                Terminator::Jump(_)
                    | Terminator::Branch { .. }
                    | Terminator::Switch { .. }
                    | Terminator::Return(_)
                    | Terminator::Raise(_)
                    | Terminator::Propagate
                    | Terminator::Unreachable
            ) {
                return Err(no("unclassified CPU mapper terminator"));
            }
        }
        for callee in &calls {
            visit(*callee, units, table, active, instructions, out)?;
        }
        active.remove(&id);
        out.spines.insert(id, spine(u, &out.cleanup));
        out.graph.bodies.insert(
            id,
            ScalarBody {
                body: u.digest(),
                calls,
                fuel_points: crate::fuel::fuel_points(u),
            },
        );
        Ok(())
    }
    visit(
        root,
        &by_id,
        table,
        &mut BTreeSet::new(),
        &mut 0,
        &mut proposal,
    )?;
    proposal.graph.identity = identity(&proposal, table, target, fuel);
    Ok(proposal)
}

/// Check graph closure, all allocation/cleanup sites, custody and the complete
/// identity. A producer cannot omit a body, allocation, loan or fuel point and
/// obtain a certificate. This never calls `cpu_mapper_graph`.
pub fn validate_cpu_mapper_graph(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    target: &TargetSpec,
    proposal: &CpuMapperGraph,
    fuel: bool,
    helpers: ContentHash,
) -> bool {
    let graph = &proposal.graph;
    if !supported(target)
        || graph.bodies.is_empty()
        || graph.bodies.len() > MAX_UNITS
        || !graph.bodies.contains_key(&graph.root)
        || proposal.helpers != helpers
        || identity(proposal, table, target, fuel) != graph.identity
    {
        return false;
    }
    let by_id: BTreeMap<_, _> = units
        .iter()
        .map(|u| (u.unit().header.instance, u.unit()))
        .collect();
    if by_id.len() != units.len() {
        return false;
    }
    let mut growth = BTreeMap::new();
    let mut regional = BTreeMap::new();
    let mut allocations = BTreeMap::new();
    let mut cleanup = BTreeSet::new();
    let mut instructions = 0usize;
    for (id, body) in &graph.bodies {
        let Some(u) = by_id.get(id).copied() else {
            return false;
        };
        instructions = instructions
            .saturating_add(u.instructions.len())
            .saturating_add(u.terminators.len());
        if instructions > MAX_INSTRUCTIONS
            || body.body != u.digest()
            || body.fuel_points != crate::fuel::fuel_points(u)
            || !body_custody(u, table)
        {
            return false;
        }
        for (index, inst) in u.instructions.iter().enumerate() {
            match instruction(u, table, inst) {
                Some(Action::Private) => {}
                Some(Action::Allocate(construct)) => {
                    allocations.insert((*id, index), construct);
                }
                Some(Action::Grow(recipe)) => {
                    growth.insert((*id, index), recipe);
                }
                Some(Action::Regional(recipe)) => {
                    regional.insert((*id, index), recipe);
                }
                Some(Action::Cleanup) => {
                    cleanup.insert((*id, index));
                }
                None => return false,
            }
        }
        let mut actual_calls = BTreeSet::new();
        for term in &u.terminators {
            match term {
                Terminator::Call { call, .. } => {
                    let Some(callee) = direct_call(u, table, &by_id, *call) else {
                        return false;
                    };
                    if !graph.bodies.contains_key(&callee) {
                        return false;
                    }
                    actual_calls.insert(callee);
                }
                Terminator::Jump(_)
                | Terminator::Branch { .. }
                | Terminator::Switch { .. }
                | Terminator::Return(_)
                | Terminator::Raise(_)
                | Terminator::Propagate
                | Terminator::Unreachable => {}
                _ => return false,
            }
        }
        if actual_calls != body.calls || proposal.spines.get(id) != Some(&spine(u, &cleanup)) {
            return false;
        }
    }
    // A topological elimination checks cycles independently of producer DFS.
    let mut remaining: BTreeMap<_, _> = graph
        .bodies
        .iter()
        .map(|(id, body)| (*id, body.calls.clone()))
        .collect();
    let mut reached = BTreeSet::new();
    let mut pending = vec![graph.root];
    while let Some(id) = pending.pop() {
        if reached.insert(id) {
            pending.extend(&graph.bodies[&id].calls);
        }
    }
    if reached.len() != graph.bodies.len() {
        return false;
    }
    while !remaining.is_empty() {
        let leaves: BTreeSet<_> = remaining
            .iter()
            .filter_map(|(id, calls)| calls.is_empty().then_some(*id))
            .collect();
        if leaves.is_empty() {
            return false;
        }
        remaining.retain(|id, calls| {
            calls.retain(|id| !leaves.contains(id));
            !leaves.contains(id)
        });
    }
    regional == proposal.regional
        && growth == proposal.growth
        && allocations == proposal.allocations
        && cleanup == proposal.cleanup
        && proposal.spines.len() == graph.bodies.len()
}

fn supported(target: &TargetSpec) -> bool {
    target.triple == "x86_64-unknown-linux-gnu"
        && target.c_convention == crate::layout_types::CConvention::SysV64
        && target.pointer_bytes == 8
        && target.cpu == "x86-64"
        && target.features.is_empty()
}

fn identity(
    proposal: &CpuMapperGraph,
    table: &LayoutTable,
    target: &TargetSpec,
    fuel: bool,
) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.ahead.cpu-mapper.v1",
        [
            table.digest().0.as_slice(),
            target.digest().0.as_slice(),
            proposal.helpers.0.as_slice(),
            &[u8::from(fuel)],
            format!(
                "{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
                proposal.graph.root,
                proposal.graph.bodies,
                proposal.allocations,
                proposal.growth,
                proposal.cleanup,
                proposal.spines,
                proposal.regional
            )
            .as_bytes(),
        ],
    )
}

fn mutable_capture(u: &LirUnit<Ready>, address: AddressId) -> bool {
    let mut at = address;
    let mut seen = BTreeSet::new();
    while seen.insert(at) {
        match u.addresses[at.index()].base {
            AddressBase::Slot(slot) => {
                return matches!(u.slots[slot.index()].kind, SlotKind::Capture(_));
            }
            AddressBase::Address(parent) => at = parent,
            // C permits value roots only through a live borrowed value. It
            // does not establish that such a borrow is ordinal-local here.
            AddressBase::Value(_) => return true,
        }
    }
    true
}

fn body_custody(u: &LirUnit<Ready>, table: &LayoutTable) -> bool {
    let data = |layout| {
        cpu_owned_layout(table, layout)
            && table
                .layout(layout)
                .is_some_and(|layout| layout.align <= 16)
            && matches!(
        table.layout(layout).map(|l| &l.size), Some(SizeSpec::Fixed(n)) if *n <= MAX_IMAGE_BYTES)
    };
    if u.slots.iter().any(|s| {
        matches!(s.kind, SlotKind::LoopCarried(_))
            || matches!(s.kind, SlotKind::Capture(_))
                && !scalar_image(table, u.types[s.ty.index()].layout)
    }) {
        return false;
    }
    let local_callable = |ty: &LirType| {
        ty.repr == ValueRepr::Callable
            && matches!(
                table.layout(ty.layout).map(|layout| &layout.kind),
                Some(LayoutKind::Callable { .. })
            )
    };
    let owned = |ty: &LirType| {
        matches!(
            ty.repr,
            ValueRepr::Unit
                | ValueRepr::Scalar(_)
                | ValueRepr::Aggregate
                | ValueRepr::Block
                | ValueRepr::InitMask { .. }
        ) && data(ty.layout)
    };
    if u.values.iter().enumerate().any(|(index, value)| {
        let ty = &u.types[value.ty.index()];
        if ty.repr == ValueRepr::RegionMark {
            u.instructions
                .iter()
                .filter(|inst| {
                    matches!(inst, Inst::RegionEnter { result, region }
                    if result.index() == index
                        && u.tables.regions.get(region.index()).is_some_and(|r| !r.seal_demand))
                })
                .count()
                != 1
        } else if ty.repr == ValueRepr::Uncounted {
            !cpu_owned_layout(table, ty.layout)
                || !table.layout(ty.layout).is_some_and(|layout| {
                    layout.scan.is_empty() && layout.drop.is_empty() && layout.finalizer.is_none()
                })
        } else if local_callable(ty) {
            super::plan::known_code(u, ValueId(index as u32), &[], false).is_none()
        } else {
            !owned(ty)
        }
    }) || u.slots.iter().any(|slot| {
        let ty = &u.types[slot.ty.index()];
        !owned(ty)
            && !local_callable(ty)
            && !(ty.repr == ValueRepr::Uncounted && cpu_owned_layout(table, ty.layout))
    }) {
        return false;
    }
    let abi = &table.fn_abis[u.header.abi.0 as usize];
    if abi.parameters.iter().any(|p| !data(p.layout))
        || abi.result.as_ref().is_some_and(|r| !data(r.layout))
    {
        return false;
    }
    for inst in &u.instructions {
        let write = match inst {
            Inst::Store { address, .. }
            | Inst::Replace { address, .. }
            | Inst::SeqReserve { address, .. }
            | Inst::SeqSetLen { address, .. }
            | Inst::BeginLoan {
                address,
                mutable: true,
                ..
            } => Some(*address),
            _ => None,
        };
        if write.is_some_and(|a| mutable_capture(u, a)) {
            return false;
        }
    }
    !u.tables.calls.iter().any(|call| call.arguments.get(&u.pools.call_arguments)
        .unwrap_or(&[]).iter().any(|a| a.passing == hirsel_core_contract::Passing::Mut
            && !matches!(a.value, ArgumentValue::Address { address, .. } if !mutable_capture(u, address))))
}

fn direct_call(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
    call: CallId,
) -> Option<ContentHash> {
    let call = &u.tables.calls[call.index()];
    if call
        .destination
        .as_ref()
        .is_some_and(|d| !matches!(d, DestinationClass::Heap))
    {
        return None;
    }
    let (id, definition) = match call.callee {
        Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => {
            let symbol = &u.tables.symbols[symbol.index()];
            (symbol.instance?, Some(&symbol.definition))
        }
        Callee::Indirect(value) => (super::plan::known_code(u, value, &[], false)?, None),
        _ => return None,
    };
    if let Callee::Bound { closure, .. } = call.callee
        && super::plan::known_code(u, closure, &[], false) != Some(id)
    {
        return None;
    }
    let reached = units.get(&id)?;
    let expected = table.fn_abis.get(call.abi.0 as usize)?;
    let actual = table.fn_abis.get(reached.header.abi.0 as usize)?;
    (definition.is_none_or(|definition| *definition == reached.header.definition)
        && expected == actual)
        .then_some(id)
}

enum Action {
    Private,
    Allocate(ConstructId),
    Grow(CpuGrowth),
    Regional(CpuRegional),
    Cleanup,
}

fn instruction(u: &LirUnit<Ready>, table: &LayoutTable, inst: &Inst) -> Option<Action> {
    Some(match inst {
        Inst::Const { constant, .. }
            if matches!(
                u.tables.constants[constant.index()].value,
                ConstantValue::Unit | ConstantValue::Bool(_) | ConstantValue::Integer(_)
            ) =>
        {
            Action::Private
        }
        Inst::FrameEnter { .. }
        | Inst::FrameLeave
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
        | Inst::EndOrigin { .. }
        | Inst::SeqLen { .. }
        | Inst::SeqCapacity { .. }
        | Inst::IsUnique { .. }
        | Inst::IsWritable { .. } => Action::Private,
        Inst::Convert {
            mode: ConvertMode::Widen | ConvertMode::Truncate | ConvertMode::TryNarrow,
            ..
        } => Action::Private,
        Inst::SeqReserve {
            address, capacity, ..
        } => {
            if mutable_capture(u, *address) {
                return None;
            }
            let layout = u.types[u.addresses.get(address.index())?.ty.index()].layout;
            let LayoutKind::Block { payload } = table.layout(layout)?.kind else {
                return None;
            };
            let LayoutKind::Sequence { element } = table.layout(payload)?.kind else {
                return None;
            };
            let record = table.layout(element)?;
            let SizeSpec::Fixed(size) = record.size else {
                return None;
            };
            if !cpu_owned_layout(table, element) {
                return None;
            }
            Action::Grow(CpuGrowth {
                address: *address,
                capacity: *capacity,
                layout,
                element,
                size,
                align: record.align,
            })
        }
        Inst::SeqSetLen { address, .. } if !mutable_capture(u, *address) => Action::Private,
        Inst::Dup { .. }
        | Inst::Release { .. }
        | Inst::Share { .. }
        | Inst::RegionEnter { .. }
        | Inst::RegionExit { .. } => Action::Cleanup,
        Inst::Construct {
            result, construct, ..
        } => {
            let ty = &u.types[u.values[result.index()].ty.index()];
            let site = &u.tables.constructs[construct.index()];
            if let Some(Placement::Fixed(StorageOwner::LocalRegion(region))) = site.placement {
                let layout = table.layout(site.layout)?;
                if ty.repr != ValueRepr::Uncounted
                    || !cpu_owned_layout(table, site.layout)
                    || !layout.scan.is_empty()
                    || !layout.drop.is_empty()
                    || layout.finalizer.is_some()
                    || layout.kind == LayoutKind::Flat
                    || u.tables.regions.get(region.index())?.seal_demand
                {
                    return None;
                }
                Action::Regional(CpuRegional {
                    construct: *construct,
                    region,
                    layout: site.layout,
                    align: layout.align,
                })
            } else if matches!(ty.repr, ValueRepr::Block) {
                if !matches!(
                    site.placement,
                    Some(Placement::Fixed(StorageOwner::Heap(_)))
                ) || !matches!(
                    site.destination,
                    DestinationClass::Heap | DestinationClass::Inferred
                ) || site.arena_no_drop
                {
                    return None;
                }
                Action::Allocate(*construct)
            } else if cpu_owned_layout(table, site.layout) {
                Action::Private
            } else {
                return None;
            }
        }
        Inst::Closure { closure, .. } => {
            let site = u.tables.closures.get(closure.index())?;
            // An empty environment uses the existing immortal image, allocates
            // no storage and carries no borrowed or counted capture across save.
            if site.code.is_none()
                || site.receipts.is_some()
                || !site.captures.get(&u.pools.captures)?.is_empty()
                || table.layout(site.environment)?.size != SizeSpec::Fixed(0)
                || !scalar_image(table, site.environment)
            {
                return None;
            }
            Action::Private
        }
        // Growth, regional placement, reuse and implicit deep copies need
        // their own handoff recipes. Never treat them as ordinary allocation.
        _ => return None,
    })
}

fn spine(u: &LirUnit<Ready>, cleanup: &BTreeSet<(ContentHash, usize)>) -> Spine {
    let barriers: BTreeSet<_> = cleanup
        .iter()
        .filter_map(|(id, i)| (*id == u.header.instance).then_some(*i))
        .collect();
    Spine {
        private_instructions: (0..u.instructions.len())
            .map(|i| !barriers.contains(&i))
            .collect(),
        scalar_calls: u
            .terminators
            .iter()
            .filter_map(|term| match term {
                Terminator::Call { call, .. } => Some(*call),
                _ => None,
            })
            .collect(),
        reached_calls: BTreeSet::new(),
        suspended: BTreeMap::new(),
        barriers,
        deferred_releases: BTreeMap::new(),
    }
}
