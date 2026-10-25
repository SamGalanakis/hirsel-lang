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
        if site(unit, observation.point) != Some(marker.site) {
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
        let form = drop_form(before, old.point);
        let replacement = drop_form(after, new.point);
        if form != replacement && !(form == Some(0) && matches!(replacement, Some(1 | 2))) {
            return Err(Error::InvalidTransport(id));
        }
        let old_anchor = drop_anchor(before, old.point);
        let new_anchor = drop_anchor(after, new.point);
        if old_anchor.is_some() || new_anchor.is_some() {
            // This typed expansion is not an arbitrary relocation permit. Its
            // source must be the same semantic cleanup action and address.
            if old_anchor.is_none() || old_anchor != new_anchor {
                return Err(Error::InvalidTransport(id));
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

fn binding<S: Stage>(unit: &LirUnit<S>, observation: &Observation) -> Result<bool, Error> {
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
                    Inst::Drop {
                        address,
                        if_live: false,
                        ..
                    },
                    Moment::Before,
                )
                | (
                    Inst::Drop {
                        address,
                        if_live: true,
                        ..
                    },
                    Moment::IfLive,
                ) => same(Drop, Address(address)),
                (Inst::FinalizeEnd { address, .. }, Moment::After) => same(Drop, Address(address)),
                // Conditional Drop, Replace, generic runtime operations and other
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
        Point::FinalizerDrop { block } => {
            let Some((address, _)) = super::finalizer::begin(unit, block) else {
                return Err(Error::InvalidPoint(m.id));
            };
            same(Drop, Address(address)) && super::finalizer::exits(unit, block, address).is_some()
        }
        Point::Edge(id) => {
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
        Point::FinalizerDrop { block } => cleanup(super::finalizer::begin(unit, block)?.1),
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
        Point::Return(block) => block_site(block),
    }
}

// Exact cleanup identity is stricter than equal source ranges: two actions of
// one source statement may have the same span and still name different drops.
fn drop_anchor<S: Stage>(unit: &LirUnit<S>, point: Point) -> Option<&CleanupAnchor> {
    if let Point::FinalizerDrop { block } = point {
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
