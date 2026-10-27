use super::*;
use hirsel_core_contract::Passing;

/// Check the plan's exact unit binding, unique inventory and local operation
/// correspondence. Unsupported placements fail closed; no fallback observation
/// is inferred from counters, equal payload bits or a nearby source operation.
pub fn check<'a, S: Stage>(unit: &'a LirUnit<S>, plan: &'a Plan) -> Result<CheckedPlan<'a>, Error> {
    if plan.version != VERSION {
        return Err(Error::Version(plan.version));
    }
    if plan.unit != unit.digest() {
        return Err(Error::StalePlan);
    }
    let mut markers = BTreeMap::new();
    let mut placements = std::collections::HashSet::new();
    for observation in &plan.observations {
        let marker = &observation.marker;
        if markers.insert(marker.id, observation).is_some() {
            return Err(Error::DuplicateMarker(marker.id));
        }
        if !binding(unit, observation)? {
            return Err(Error::InvalidBinding(marker.id));
        }
        let source = if marker.kind == Kind::Move
            && let Point::FinalizerDrop { block } = observation.point
        {
            let sites = match marker.owner {
                Location::Value(parameter) => {
                    super::finalizer::failure_edges(unit, block, parameter)
                }
                _ => None,
            }
            .and_then(|edges| {
                let mut sites = edges.into_iter().map(|edge| site(unit, Point::Edge(edge)));
                let first = sites.next()??;
                sites.all(|next| next == Some(first)).then_some(first)
            });
            sites
        } else {
            site(unit, observation.point)
        };
        if source != Some(marker.site) {
            return Err(Error::InvalidSite(marker.id));
        }
        if !placements.insert((observation.point, marker.kind, marker.token, marker.owner)) {
            return Err(Error::DuplicateObservation(marker.id));
        }
    }
    Ok(CheckedPlan { plan, markers })
}

/// Check every rewriting boundary, including a no-op pass. The recipe is
/// mandatory and consumes each input id exactly once. There is no implicit
/// identity recipe, silent deletion, synthesized replacement marker or permit
/// to change the immutable source/kind. Pass-specific semantic checks still
/// judge whether the rewrite itself is valid.
pub fn check_transform<'a, A: Stage, B: Stage>(
    before: &LirUnit<A>,
    input: &Plan,
    after: &'a LirUnit<B>,
    output: &'a Plan,
    recipe: Option<&Recipe>,
) -> Result<CheckedPlan<'a>, Error> {
    let input = check(before, input)?;
    if before.header.definition != after.header.definition
        || before.header.target != after.header.target
        || before.header.abi != after.header.abi
        || before.header.instance != after.header.instance
        || before.header.implementation_revision != after.header.implementation_revision
    {
        return Err(Error::ChangedUnit);
    }
    let recipe = recipe.ok_or(Error::MissingRecipe)?;
    if recipe.before != input.plan.unit || recipe.after != output.unit {
        return Err(Error::StaleRecipe);
    }
    let output = check(after, output)?;
    let mut relocations = BTreeMap::new();
    for relocation in &recipe.relocations {
        if relocations.insert(relocation.id, relocation).is_some() {
            return Err(Error::DuplicateRelocation(relocation.id));
        }
        if !input.markers.contains_key(&relocation.id) {
            return Err(Error::ForgedRelocation(relocation.id));
        }
    }
    for (&id, old) in &input.markers {
        let new = output.markers.get(&id).ok_or(Error::MissingMarker(id))?;
        let relocation = relocations.get(&id).ok_or(Error::MissingRelocation(id))?;
        if relocation.token != old.marker.token || relocation.owner != old.marker.owner {
            return Err(Error::IdentityRewrite(id));
        }
        for location in [old.marker.token, old.marker.owner] {
            if let Location::Address(address) = location
                && before.addresses.get(address.index()) != after.addresses.get(address.index())
            {
                return Err(Error::IdentityRewrite(id));
            }
        }
        let form = (old.marker.kind == Kind::Drop)
            .then(|| drop_form(before, old.point))
            .flatten();
        let replacement = (new.marker.kind == Kind::Drop)
            .then(|| drop_form(after, new.point))
            .flatten();
        if form != replacement && !(form == Some(0) && matches!(replacement, Some(1 | 2 | 3))) {
            return Err(Error::InvalidTransport(id));
        }
        if old.marker.kind == Kind::Move && !edge_transport(before, old.point, after, new.point) {
            return Err(Error::InvalidTransport(id));
        }
        let old_anchor = (old.marker.kind == Kind::Drop)
            .then(|| drop_anchor(before, old.point))
            .flatten();
        let new_anchor = (new.marker.kind == Kind::Drop)
            .then(|| drop_anchor(after, new.point))
            .flatten();
        if old_anchor.is_some() || new_anchor.is_some() {
            // This typed expansion is not an arbitrary relocation permit. Its
            // source must be the same semantic cleanup action and address.
            if old_anchor.is_none() || old_anchor != new_anchor {
                return Err(Error::InvalidTransport(id));
            }
        }
        if let Point::StagedReturn { address, .. } = new.point {
            match old.point {
                Point::Return(_) => {
                    if after
                        .addresses
                        .get(address.index())
                        .is_none_or(|info| match info.base {
                            AddressBase::Slot(slot) => slot.index() < before.slots.len(),
                            _ => true,
                        })
                    {
                        return Err(Error::InvalidTransport(id));
                    }
                }
                Point::StagedReturn { address: old, .. } if old == address => (),
                _ => return Err(Error::InvalidTransport(id)),
            }
        }
        if old.marker.kind != new.marker.kind
            || old.marker.site != new.marker.site
            || new.point != relocation.point
            || new.marker.token != relocation.token
            || new.marker.owner != relocation.owner
        {
            return Err(Error::ChangedMarker(id));
        }
    }
    for &id in output.markers.keys() {
        if !input.markers.contains_key(&id) {
            return Err(Error::ForgedMarker(id));
        }
    }
    if output
        .plan
        .observations
        .iter()
        .map(|o| o.marker.id)
        .ne(recipe.relocations.iter().map(|r| r.id))
    {
        return Err(Error::ChangedOrder);
    }
    Ok(output)
}

/// Source parameters may enter directly into a parameter slot. Their Plain
/// SSA entry value is its version name, not another language-owned credit.
pub(super) fn parameter_address<S>(
    unit: &LirUnit<S>,
    parameter: u32,
    mut address: AddressId,
) -> Option<SlotId> {
    let value = *unit.parameters.get(parameter as usize)?;
    if unit.values.get(value.index())?.holding != Holding::Plain {
        return None;
    }
    for _ in 0..unit.addresses.len() {
        let info = unit.addresses.get(address.index())?;
        if info.projection.len != 0 {
            return None;
        }
        match info.base {
            AddressBase::Slot(slot) => {
                let slot_info = unit.slots.get(slot.index())?;
                return (slot_info.kind == SlotKind::Parameter(parameter)
                    && slot_info.ty == unit.values[value.index()].ty)
                    .then_some(slot);
            }
            AddressBase::Address(parent) => address = parent,
            _ => return None,
        }
    }
    None
}

/// Pooled edge rows can outlive their last CFG reference. They are metadata,
/// not executed transfers, and cannot be used as ownership observations.
pub(super) fn used_edges<S>(unit: &LirUnit<S>) -> std::collections::BTreeSet<EdgeId> {
    let mut edges = std::collections::BTreeSet::new();
    for term in &unit.terminators {
        edges.extend(term.normal_edges(&unit.pools));
        for failure in term.failure_edges(&unit.tables) {
            if let Some(failure) = unit.tables.failures.get(failure.index()) {
                edges.insert(failure.cleanup);
            }
        }
    }
    for block in &unit.blocks {
        for inst in block.instructions.get(&unit.instructions).unwrap_or(&[]) {
            if let Some(failure) = inst.failure()
                && let Some(failure) = unit.tables.failures.get(failure.index())
            {
                edges.insert(failure.cleanup);
            }
        }
    }
    edges
}

/// Accept a callable stored and borrowed along one checked predecessor chain.
/// C independently checks the ordinary unit; this
/// checker replays the named slot's live/initialized version and loan rather
/// than trusting a producer's alias target or an equal callable layout.
pub(super) fn call_alias<S: Stage>(unit: &LirUnit<S>, observation: &Observation) -> bool {
    fn whole<S>(unit: &LirUnit<S>, mut address: AddressId) -> Option<SlotId> {
        for _ in 0..unit.addresses.len() {
            let info = unit.addresses.get(address.index())?;
            if info.projection.len != 0 {
                return None;
            }
            match info.base {
                AddressBase::Slot(slot) => return Some(slot),
                AddressBase::Address(parent) => address = parent,
                _ => return None,
            }
        }
        None
    }
    let Point::CallEntered(block) = observation.point else {
        return false;
    };
    let (Location::Value(view), Location::Address(owner)) =
        (observation.marker.token, observation.marker.owner)
    else {
        return false;
    };
    let Some(info) = unit.values.get(view.index()) else {
        return false;
    };
    let Holding::Borrowed(loan) = info.holding else {
        return false;
    };
    let Some(loan_info) = unit.tables.loans.get(loan.index()) else {
        return false;
    };
    let Some(slot) = whole(unit, owner) else {
        return false;
    };
    if loan_info.mutable
        || whole(unit, loan_info.address) != Some(slot)
        || unit
            .slots
            .get(slot.index())
            .is_none_or(|slot| slot.ty != info.ty)
        || unit
            .types
            .get(info.ty.index())
            .is_none_or(|ty| ty.repr != ValueRepr::Callable)
    {
        return false;
    }
    let Some(Terminator::Call { call, .. }) = unit.terminators.get(block.index()) else {
        return false;
    };
    let Some(call) = unit.tables.calls.get(call.index()) else {
        return false;
    };
    if call.callee.value() != Some(view) || call.pretransfer_failure.is_some() {
        return false;
    }
    // Arena controls split an ordinary store/loan/call sequence into blocks.
    // Follow only a unique normal predecessor back to entry. A join, failure
    // entry or cycle needs a stronger proof and remains refused.
    let mut predecessors = vec![Vec::new(); unit.blocks.len()];
    for (index, term) in unit.terminators.iter().enumerate() {
        for edge in term.normal_edges(&unit.pools) {
            let Some(info) = unit.tables.edges.get(edge.index()) else {
                return false;
            };
            let Some(rows) = predecessors.get_mut(info.target.index()) else {
                return false;
            };
            rows.push((BlockId(index as u32), edge));
        }
    }
    let mut failure_targets = std::collections::BTreeSet::new();
    for (index, term) in unit.terminators.iter().enumerate() {
        let failures = term.failure_edges(&unit.tables).into_iter().chain(
            unit.blocks[index]
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
                .iter()
                .filter_map(Inst::failure),
        );
        for failure in failures {
            let Some(failure) = unit.tables.failures.get(failure.index()) else {
                return false;
            };
            let Some(edge) = unit.tables.edges.get(failure.cleanup.index()) else {
                return false;
            };
            failure_targets.insert(edge.target);
        }
    }
    let mut chain = vec![block];
    let mut current = block;
    while current != unit.entry {
        if failure_targets.contains(&current) {
            return false;
        }
        let [(parent, _)] = predecessors[current.index()].as_slice() else {
            return false;
        };
        if chain.contains(parent) {
            return false;
        }
        chain.push(*parent);
        current = *parent;
    }
    chain.reverse();
    // This proves local custody only. Native emission independently binds
    // incoming code identities over the complete checked program.
    let entered = match unit.slots[slot.index()].kind {
        SlotKind::Parameter(parameter)
            if parameter_address(unit, parameter, owner) == Some(slot) =>
        {
            unit.parameters.get(parameter as usize).copied()
        }
        _ => None,
    };
    let (mut live, mut version, mut active, mut borrowed) =
        (entered.is_some(), entered, false, None);
    for (position, &block) in chain.iter().enumerate() {
        let Some(code) = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
        else {
            return false;
        };
        for instruction in code {
            match *instruction {
                Inst::StorageLive { slot: current } if current == slot => {
                    live = true;
                    version = None;
                }
                Inst::StorageDead { slot: current, .. } if current == slot => {
                    live = false;
                    version = None;
                }
                Inst::EndScope { scope } => {
                    let mut current = Some(unit.slots[slot.index()].scope);
                    for _ in 0..unit.scopes.len() {
                        let Some(at) = current else { break };
                        if at == scope {
                            live = false;
                            version = None;
                            break;
                        }
                        current = unit.scopes.get(at.index()).and_then(|info| info.parent);
                    }
                }
                Inst::Store {
                    address,
                    value,
                    version: next,
                } if whole(unit, address) == Some(slot) => {
                    version = unit
                        .values
                        .get(value.index())
                        .filter(|value| value.holding == Holding::Owned && value.ty == info.ty)
                        .map(|_| next);
                }
                Inst::Read {
                    address,
                    mode: ReadMode::Take,
                    ..
                }
                | Inst::Drop { address, .. }
                | Inst::FinalizeEnd { address, .. }
                    if whole(unit, address) == Some(slot) =>
                {
                    version = None
                }
                Inst::Replace { address, .. } if whole(unit, address) == Some(slot) => {
                    version = None
                }
                Inst::BeginLoan { loan: current, .. } if current == loan => {
                    active = true;
                    borrowed = None;
                }
                Inst::EndLoan { loan: current, .. } if current == loan => {
                    active = false;
                    borrowed = None;
                }
                Inst::Read {
                    result,
                    address,
                    reaching,
                    mode: ReadMode::Borrow,
                } if result == view && address == owner && active => borrowed = Some(reaching),
                _ => (),
            }
        }
        if let Some(&next) = chain.get(position + 1) {
            if let Terminator::Call { call, .. } = unit.terminators[block.index()] {
                let Some(call) = unit.tables.calls.get(call.index()) else {
                    return false;
                };
                for argument in call
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap_or(&[])
                {
                    if let ArgumentValue::Address { address, .. } = argument.value
                        && whole(unit, address) == Some(slot)
                    {
                        return false;
                    }
                }
            }
            let [(_, edge)] = predecessors[next.index()].as_slice() else {
                return false;
            };
            let edge = &unit.tables.edges[edge.index()];
            for (&parameter, &argument) in unit.blocks[next.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .zip(edge.arguments.get(&unit.pools.values).unwrap_or(&[]))
            {
                if version == Some(argument) {
                    version = Some(parameter);
                }
                if borrowed == Some(argument) {
                    borrowed = Some(parameter);
                }
            }
        }
    }
    live && active && version.is_some() && borrowed == version
}

fn binding<S: Stage>(unit: &LirUnit<S>, observation: &Observation) -> Result<bool, Error> {
    if observation.marker.kind == Kind::Alias {
        return Ok(call_alias(unit, observation));
    }
    use Kind::{Create, Drop, Move};
    use Location::{Address, Capture, EnvironmentCapture, Field, Parameter, Return, Value};
    let m = &observation.marker;
    let tuple = (m.kind, m.token, m.owner);
    let same = |kind, location| tuple == (kind, location, location);
    let moved = |from, to| tuple == (Move, from, to);
    let block = |b: BlockId| unit.blocks.get(b.index()).ok_or(Error::InvalidPoint(m.id));
    let term = |b: BlockId| {
        unit.terminators
            .get(b.index())
            .ok_or(Error::InvalidPoint(m.id))
    };
    Ok(match observation.point {
        Point::Entry => {
            unit.parameters
                .iter()
                .enumerate()
                .any(|(i, &v)| {
                    moved(Parameter(i as u32), Value(v)) || matches!(tuple,
                        (Move, Parameter(parameter), Address(address))
                            if parameter == i as u32 && parameter_address(unit, parameter, address).is_some())
                })
                || unit
                    .captures
                    .iter()
                    .enumerate()
                    .any(|(i, &v)| moved(EnvironmentCapture(i as u32), Value(v)))
        }
        Point::Instruction {
            block: b,
            index,
            moment,
        } => {
            let instructions = block(b)?
                .instructions
                .get(&unit.instructions)
                .ok_or(Error::InvalidPoint(m.id))?;
            let inst = instructions
                .get(index as usize)
                .ok_or(Error::InvalidPoint(m.id))?;
            match (*inst, moment) {
                (
                    Inst::Construct {
                        result, construct, ..
                    }
                    | Inst::Reuse {
                        result, construct, ..
                    },
                    Moment::After,
                ) => {
                    let site = unit
                        .tables
                        .constructs
                        .get(construct.index())
                        .ok_or(Error::InvalidPoint(m.id))?;
                    same(Create, Value(result))
                        || site
                            .fields
                            .get(&unit.pools.fields)
                            .unwrap_or(&[])
                            .iter()
                            .any(|f| {
                                moved(
                                    Value(f.value),
                                    Field {
                                        aggregate: result,
                                        field: f.field,
                                    },
                                )
                            })
                }
                (
                    Inst::Closure {
                        result, closure, ..
                    },
                    Moment::After,
                ) => {
                    let site = unit
                        .tables
                        .closures
                        .get(closure.index())
                        .ok_or(Error::InvalidPoint(m.id))?;
                    same(Create, Value(result))
                        || site
                            .captures
                            .get(&unit.pools.captures)
                            .unwrap_or(&[])
                            .iter()
                            .enumerate()
                            .any(|(i, c)| {
                                c.usage == CaptureUse::Take
                                    && moved(
                                        Value(c.value),
                                        Capture {
                                            closure: result,
                                            capture: i as u32,
                                        },
                                    )
                            })
                }
                (
                    Inst::Read {
                        result,
                        address,
                        mode: ReadMode::Take,
                        ..
                    },
                    Moment::After,
                ) => moved(Address(address), Value(result)),
                (Inst::Store { address, value, .. }, Moment::After) => {
                    moved(Value(value), Address(address))
                }
                (
                    Inst::Recast { result, value } | Inst::Weaken { result, value },
                    Moment::After,
                ) => moved(Value(value), Value(result)),
                (
                    Inst::Copy { result, .. }
                    | Inst::CopyValue { result, .. }
                    | Inst::FlatCopy { result, .. },
                    Moment::After,
                ) => same(Create, Value(result)),
                (Inst::Release { value, .. } | Inst::Reset { value, .. }, Moment::Before) => {
                    same(Drop, Value(value))
                }
                (
                    Inst::Drop { address, if_live: false, .. },
                    Moment::Before,
                ) | (
                    Inst::Drop { address, if_live: true, .. },
                    Moment::IfLive,
                ) => same(Drop, Address(address)),
                (Inst::FinalizeEnd { address, .. }, Moment::After) => same(Drop, Address(address)),
                // Replace, generic runtime operations and other
                // compound transitions need their own explicit commit recipe.
                // Never accept them as "close enough" to a supported transition.
                _ => false,
            }
        }
        Point::ReleaseDrop { block: b, index } => {
            let instructions = block(b)?
                .instructions
                .get(&unit.instructions)
                .ok_or(Error::InvalidPoint(m.id))?;
            let pair = index
                .checked_sub(1)
                .and_then(|start| instructions.get(start as usize..=index as usize));
            matches!(pair, Some([
                Inst::Read { result, address, mode: ReadMode::Take, .. },
                Inst::Release { value, live: MaskRef::All, .. },
            ]) if result == value && same(Drop, Address(*address)))
        }
        Point::CompoundDrop { .. } => {
            matches!(tuple, (Drop, Address(address), Address(owner)) if address == owner
                && super::compound::parts(unit, observation.point, address).is_some())
        }
        Point::FinalizerDrop { block } if m.kind == Move => {
            let Location::Value(parameter) = m.owner else { return Ok(false) };
            let Some(edges) = super::finalizer::failure_edges(unit, block, parameter) else { return Ok(false) };
            edges.into_iter().all(|id| {
                let edge = &unit.tables.edges[id.index()];
                let Some(parameters) = unit.blocks[edge.target.index()].parameters.get(&unit.pools.values) else { return false };
                let Some(arguments) = edge.arguments.get(&unit.pools.values) else { return false };
                parameters.iter().zip(arguments).any(|(&to, &from)| moved(Value(from), Value(to)))
            })
        }
        Point::FinalizerDrop { block } => {
            let Some((address, _)) = super::finalizer::begin(unit, block) else {
                return Err(Error::InvalidPoint(m.id));
            };
            same(Drop, Address(address)) && super::finalizer::exits(unit, block, address).is_some()
        }
        Point::Edge(id) => {
            if !used_edges(unit).contains(&id) {
                return Err(Error::InvalidPoint(m.id));
            }
            let edge = unit
                .tables
                .edges
                .get(id.index())
                .ok_or(Error::InvalidPoint(m.id))?;
            let parameters = block(edge.target)?
                .parameters
                .get(&unit.pools.values)
                .ok_or(Error::InvalidPoint(m.id))?;
            let arguments = edge
                .arguments
                .get(&unit.pools.values)
                .ok_or(Error::InvalidPoint(m.id))?;
            parameters
                .iter()
                .zip(arguments)
                .any(|(&to, &from)| moved(Value(from), Value(to)))
        }
        Point::CallEntered(b) | Point::CallReturned(b) => {
            let Terminator::Call { call, normal, .. } = *term(b)? else {
                return Err(Error::InvalidPoint(m.id));
            };
            let call = unit
                .tables
                .calls
                .get(call.index())
                .ok_or(Error::InvalidPoint(m.id))?;
            if matches!(observation.point, Point::CallEntered(_)) {
                call.arguments.get(&unit.pools.call_arguments).unwrap_or(&[]).iter().any(|a| {
                    matches!(a.value, ArgumentValue::Value(v) if a.passing == Passing::Take && moved(Value(v), Parameter(a.parameter)))
                })
            } else {
                normal.is_some() && call.result.is_some_and(|v| moved(Return, Value(v)))
            }
        }
        Point::StagedReturn { .. } => super::staged::binding(unit, observation).is_some(),
        Point::Return(b) => {
            matches!(term(b)?, Terminator::Return(Some(v)) if moved(Value(*v), Return))
        }
    })
}

// An observation has the same source as the operation it describes. A join
// observation uses its recipient block's anchor. It never takes an arbitrary
// source range merely because that range occurs somewhere in the unit.
pub(super) fn site<S: Stage>(unit: &LirUnit<S>, point: Point) -> Option<SiteRange> {
    let source = |id: SourceAnchorId| unit.tables.source_anchors.get(id.index()).map(|a| a.site);
    let cleanup = |id: CleanupAnchorId| unit.tables.cleanup_anchors.get(id.index()).map(|a| a.site);
    let block_site = |b: BlockId| source(unit.blocks.get(b.index())?.anchor);
    match point {
        Point::Entry => block_site(unit.entry),
        Point::CompoundDrop { block, .. } => cleanup(super::finalizer::begin(unit, block)?.1),
        Point::FinalizerDrop { block } => {
            // Drop markers use the cleanup action. Failure-join Move markers
            // retain the recipient block's source, as their original edge did.
            cleanup(super::finalizer::begin(unit, block)?.1)
        }
        Point::Instruction { block, index, .. } | Point::ReleaseDrop { block, index } => {
            let inst = unit
                .blocks
                .get(block.index())?
                .instructions
                .get(&unit.instructions)?
                .get(index as usize)?;
            match *inst {
                Inst::Construct { construct, .. } | Inst::Reuse { construct, .. } => {
                    source(unit.tables.constructs.get(construct.index())?.anchor)
                }
                Inst::Closure { closure, .. } => {
                    source(unit.tables.closures.get(closure.index())?.anchor)
                }
                Inst::Drop { anchor, .. }
                | Inst::Release { anchor, .. }
                | Inst::Reset { anchor, .. }
                | Inst::FinalizeEnd { anchor, .. } => cleanup(anchor),
                _ => block_site(block),
            }
        }
        Point::Edge(edge) => block_site(unit.tables.edges.get(edge.index())?.target),
        Point::CallEntered(block) | Point::CallReturned(block) => {
            let Terminator::Call { call, .. } = unit.terminators.get(block.index())? else {
                return None;
            };
            source(unit.tables.calls.get(call.index())?.anchor)
        }
        Point::Return(block) | Point::StagedReturn { block, .. } => block_site(block),
    }
}

// Exact cleanup identity is stricter than equal source ranges: two actions of
// one source statement may have the same span and still name different drops.
fn drop_anchor<S: Stage>(unit: &LirUnit<S>, point: Point) -> Option<&CleanupAnchor> {
    if let Point::FinalizerDrop { block } | Point::CompoundDrop { block, .. } = point {
        let (_, anchor) = super::finalizer::begin(unit, block)?;
        return unit.tables.cleanup_anchors.get(anchor.index());
    }
    let (block, index, moment) = match point {
        Point::Instruction {
            block,
            index,
            moment,
        } => (block, index, moment),
        Point::ReleaseDrop { block, index } => (block, index, Moment::Before),
        _ => return None,
    };
    let instruction = unit
        .blocks
        .get(block.index())?
        .instructions
        .get(&unit.instructions)?
        .get(index as usize)?;
    let anchor = match (instruction, moment) {
        (
            Inst::Drop {
                anchor,
                if_live: false,
                ..
            },
            Moment::Before,
        )
        | (
            Inst::Drop {
                anchor,
                if_live: true,
                ..
            },
            Moment::IfLive,
        )
        | (
            Inst::Release {
                anchor,
                live: MaskRef::All,
                ..
            },
            Moment::Before,
        )
        | (Inst::Reset { anchor, .. }, Moment::Before)
        | (Inst::FinalizeEnd { anchor, .. }, Moment::After) => *anchor,
        _ => return None,
    };
    unit.tables.cleanup_anchors.get(anchor.index())
}

// A bundled logical drop cannot be reduced to one of its physical exits.
// Only a semantic Drop may expand to a checked release/finalizer transport.
fn drop_form<S: Stage>(unit: &LirUnit<S>, point: Point) -> Option<u8> {
    match point {
        Point::ReleaseDrop { .. } => Some(1),
        Point::FinalizerDrop { .. } => Some(2),
        Point::CompoundDrop { .. } => Some(3),
        Point::Instruction {
            block,
            index,
            moment: Moment::Before | Moment::IfLive,
        } if matches!(
            unit.blocks
                .get(block.index())?
                .instructions
                .get(&unit.instructions)?
                .get(index as usize)?,
            Inst::Drop { .. }
        ) =>
        {
            Some(0)
        }
        _ => None,
    }
}

// Only the exact semantic Drop that named this failure edge can bundle it
// with a finalizer's independently checked body-failure/rejection exits.
fn edge_transport<A: Stage, B: Stage>(
    before: &LirUnit<A>,
    from: Point,
    after: &LirUnit<B>,
    to: Point,
) -> bool {
    match (from, to) {
        (Point::Edge(edge), Point::FinalizerDrop { block }) => {
            let Some((address, anchor)) = super::finalizer::begin(after, block) else {
                return false;
            };
            before.instructions.iter().any(|inst| matches!(*inst,
                Inst::Drop { address: old, anchor: action, failure, .. }
                if old == address && before.tables.cleanup_anchors.get(action.index()) == after.tables.cleanup_anchors.get(anchor.index())
                    && before.tables.failures.get(failure.index()).is_some_and(|f| f.cleanup == edge)))
        }
        (Point::FinalizerDrop { block: old }, Point::FinalizerDrop { block: new }) => {
            match (
                super::finalizer::begin(before, old),
                super::finalizer::begin(after, new),
            ) {
                (Some((a, x)), Some((b, y))) => {
                    a == b
                        && before.tables.cleanup_anchors.get(x.index())
                            == after.tables.cleanup_anchors.get(y.index())
                }
                _ => false,
            }
        }
        (Point::FinalizerDrop { .. }, _) => false,
        _ => true,
    }
}
