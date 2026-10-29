//! Checked custody for normal retirement and failure cleanup introduced by RC.
use super::*;
use crate::stage::StageKind;
use std::collections::HashSet;

fn address<S: Stage>(
    unit: &LirUnit<S>,
    location: Location,
) -> Option<(SlotId, Vec<ProjectionStep>)> {
    let Location::Address(mut id) = location else {
        return None;
    };
    let mut parts = Vec::new();
    for _ in 0..unit.addresses.len() {
        let info = unit.addresses.get(id.index())?;
        parts.push(info.projection.get(&unit.pools.projections)?.to_vec());
        match info.base {
            AddressBase::Slot(slot) => {
                return Some((slot, parts.into_iter().rev().flatten().collect()));
            }
            AddressBase::Address(parent) if parent.0 < id.0 => id = parent,
            _ => return None,
        }
    }
    None
}

fn trigger<S: Stage>(
    unit: &LirUnit<S>,
    point: Point,
) -> Option<(Option<ValueId>, AddressId, CleanupAnchorId, BlockId)> {
    let (block, index, moment) = match point {
        Point::Instruction {
            block,
            index,
            moment,
        } => (block, index, moment),
        Point::ReleaseDrop { block, index } => (block, index, Moment::Before),
        _ => return None,
    };
    let code = unit
        .blocks
        .get(block.index())?
        .instructions
        .get(&unit.instructions)?;
    let (value, address, anchor, failure) = match *code.get(index as usize)? {
        Inst::Drop {
            address,
            anchor,
            failure,
            if_live,
        } if moment
            == if if_live {
                Moment::IfLive
            } else {
                Moment::Before
            } =>
        {
            (None, address, anchor, failure)
        }
        Inst::Release {
            value,
            anchor,
            live: MaskRef::All,
            failure,
            ..
        } if moment == Moment::Before => {
            let Inst::Read {
                result,
                address,
                mode: ReadMode::Take,
                ..
            } = *code.get(index.checked_sub(1)? as usize)?
            else {
                return None;
            };
            if value != result {
                return None;
            }
            (Some(value), address, anchor, failure)
        }
        _ => return None,
    };
    let cleanup = unit.tables.failures.get(failure.index())?.cleanup;
    Some((
        value,
        address,
        anchor,
        unit.tables.edges.get(cleanup.index())?.target,
    ))
}

fn block(point: Point) -> Option<BlockId> {
    match point {
        Point::Instruction { block, .. }
        | Point::ReleaseDrop { block, .. }
        | Point::FinalizerDrop { block }
        | Point::CompoundDrop { block, .. }
        | Point::EmptyDrop { block, .. } => Some(block),
        _ => None,
    }
}

fn reaches<S: Stage>(unit: &LirUnit<S>, entry: BlockId, target: BlockId) -> bool {
    let mut pending = vec![entry];
    let mut seen = HashSet::new();
    while let Some(at) = pending.pop() {
        if !seen.insert(at) {
            continue;
        }
        let Some(info) = unit.blocks.get(at.index()) else {
            return false;
        };
        if info.kind != BlockKind::FailureCleanup {
            return false;
        }
        if at == target {
            return true;
        }
        // The generation boundary is RC: synthesized cleanup consists of
        // cleanup instructions and jumps, never a body call or a normal arm.
        if let Some(Terminator::Jump(edge)) = unit.terminators.get(at.index()) {
            let Some(edge) = unit.tables.edges.get(edge.index()) else {
                return false;
            };
            pending.push(edge.target);
        }
    }
    false
}

pub(super) fn check<S: Stage>(unit: &LirUnit<S>, plan: &Plan) -> Result<(), Error> {
    if plan.cleanups.is_empty() {
        return Ok(());
    }
    if matches!(S::KIND, StageKind::Value | StageKind::Placed) && !plan.cleanups.is_empty() {
        return Err(Error::InvalidCleanup(plan.cleanups[0].marker));
    }
    let sources: BTreeMap<_, _> = plan
        .observations
        .iter()
        .map(|row| (row.marker.id, row))
        .collect();
    let mut placements: HashSet<_> = plan
        .observations
        .iter()
        .filter(|row| row.marker.kind == Kind::Drop)
        .map(|row| row.point)
        .collect();
    for extra in &plan.cleanups {
        let fail = || Error::InvalidCleanup(extra.marker);
        let source = sources.get(&extra.marker).copied().ok_or_else(fail)?;
        if source.marker.kind != Kind::Drop
            || source.marker.token != source.marker.owner
            || address(unit, source.marker.token).is_none()
            || address(unit, source.marker.token) != address(unit, extra.token)
            || (extra.trigger.unit == plan.unit && trigger(unit, extra.trigger.point).is_none())
            || !placements.insert(extra.point)
        {
            return Err(fail());
        }
        let actual = Observation {
            point: extra.point,
            marker: ObserveOwnership {
                id: source.marker.id,
                kind: Kind::Drop,
                token: extra.token,
                owner: extra.token,
                site: extra.site,
            },
        };
        if !super::check::binding(unit, &actual)?
            || super::check::site(unit, extra.point) != Some(extra.site)
        {
            return Err(fail());
        }
    }
    Ok(())
}

pub(super) fn transform<A: Stage, B: Stage>(
    before: &LirUnit<A>,
    input: &Plan,
    after: &LirUnit<B>,
    output: &Plan,
    recipe: &Recipe,
) -> Result<(), Error> {
    if input.cleanups.is_empty() && output.cleanups.is_empty() && recipe.cleanups.is_empty() {
        return Ok(());
    }
    if output.cleanups.len() != recipe.cleanups.len() {
        return Err(Error::InvalidCleanup(
            output
                .cleanups
                .first()
                .or(input.cleanups.first())
                .map_or(MarkerId(0), |row| row.marker),
        ));
    }
    let source_markers: BTreeMap<_, _> = input
        .observations
        .iter()
        .map(|row| (row.marker.id, row))
        .collect();
    let old_cleanups: std::collections::HashMap<_, _> = input
        .cleanups
        .iter()
        .enumerate()
        .map(|(index, row)| (row, index))
        .collect();
    let old_anchors: HashSet<_> = before.tables.cleanup_anchors.iter().collect();
    let old_data_drops: HashSet<_> = before
        .instructions
        .iter()
        .filter_map(|inst| match *inst {
            Inst::Drop {
                address: at,
                anchor,
                ..
            } => Some((
                before.tables.cleanup_anchors.get(anchor.index())?.clone(),
                address(before, Location::Address(at))?,
            )),
            _ => None,
        })
        .collect();
    let mut consumed = HashSet::new();
    for (extra, relocation) in output.cleanups.iter().zip(&recipe.cleanups) {
        let fail = || Error::InvalidCleanup(extra.marker);
        if extra != &relocation.after {
            return Err(fail());
        }
        match &relocation.before {
            Some(old) => {
                let index = *old_cleanups.get(old).ok_or_else(fail)?;
                if !consumed.insert(index)
                    || old.marker != extra.marker
                    || old.token != extra.token
                    || old.site != extra.site
                {
                    return Err(fail());
                }
                if old.trigger != extra.trigger {
                    return Err(fail());
                }
                let old_anchor = super::check::drop_anchor(before, old.point);
                if old_anchor.is_none()
                    || old_anchor != super::check::drop_anchor(after, extra.point)
                {
                    return Err(fail());
                }
                let form = super::check::drop_form(before, old.point);
                let replacement = super::check::drop_form(after, extra.point);
                if form != replacement
                    && !(form == Some(0) && matches!(replacement, Some(1 | 2 | 3)))
                {
                    return Err(fail());
                }
            }
            None => {
                if extra.trigger.unit != output.unit {
                    return Err(fail());
                }
                let current = trigger(after, extra.trigger.point).ok_or_else(fail)?;
                if extra.point == extra.trigger.point {
                    let source = source_markers
                        .get(&extra.marker)
                        .copied()
                        .ok_or_else(fail)?;
                    let data = address(after, Location::Address(current.1)).ok_or_else(fail)?;
                    let action = after
                        .tables
                        .cleanup_anchors
                        .get(current.2.index())
                        .ok_or_else(fail)?;
                    let Point::ReleaseDrop { block, .. } = extra.point else {
                        return Err(fail());
                    };
                    if !matches!(A::KIND, StageKind::Placed | StageKind::Rc)
                        || B::KIND != StageKind::Rc
                        || !current
                            .0
                            .is_some_and(|value| value.index() >= before.values.len())
                        || after.blocks[block.index()].kind != BlockKind::Body
                        || super::check::drop_form(after, extra.point) != Some(1)
                        || source.marker.kind != Kind::Drop
                        || super::check::drop_form(before, source.point) != Some(0)
                        || super::check::drop_anchor(before, source.point) != Some(action.clone())
                        || address(before, source.marker.token).as_ref() != Some(&data)
                        || address(after, extra.token).as_ref() != Some(&data)
                        || extra.site != source.marker.site
                        || !old_data_drops.contains(&(action.clone(), data))
                    {
                        return Err(fail());
                    }
                    continue;
                }
                if !matches!(A::KIND, StageKind::Placed | StageKind::Rc)
                    || B::KIND != StageKind::Rc
                    || current
                        .0
                        .is_some_and(|value| value.index() < before.values.len())
                    || super::check::drop_form(after, extra.point) != Some(0)
                    || !block(extra.point).is_some_and(|target| reaches(after, current.3, target))
                {
                    return Err(fail());
                }
                if current.0.is_none() {
                    let data_root =
                        address(after, Location::Address(current.1)).ok_or_else(fail)?;
                    let action = after
                        .tables
                        .cleanup_anchors
                        .get(current.2.index())
                        .ok_or_else(fail)?;
                    let original = old_data_drops.contains(&(action.clone(), data_root));
                    if !original {
                        return Err(fail());
                    }
                }
                let source = source_markers
                    .get(&extra.marker)
                    .copied()
                    .ok_or_else(fail)?;
                let resource = address(before, source.marker.token).ok_or_else(fail)?;
                let data = address(after, Location::Address(current.1)).ok_or_else(fail)?;
                if source.marker.kind != Kind::Drop
                    || resource.0 == data.0
                    || address(after, extra.token).as_ref() != Some(&resource)
                {
                    return Err(fail());
                }
                let Point::Instruction { block, index, .. } = extra.point else {
                    return Err(fail());
                };
                let code = after.blocks[block.index()]
                    .instructions
                    .get(&after.instructions)
                    .ok_or_else(fail)?;
                let Inst::Drop { anchor, .. } = code[index as usize] else {
                    return Err(fail());
                };
                let action = after
                    .tables
                    .cleanup_anchors
                    .get(anchor.index())
                    .ok_or_else(fail)?;
                let Point::Instruction { block: trigger, .. } = extra.trigger.point else {
                    return Err(fail());
                };
                let at = after
                    .tables
                    .source_anchors
                    .get(after.blocks[trigger.index()].anchor.index())
                    .ok_or_else(fail)?;
                if old_anchors.contains(action)
                    || action.cleanup != STATE_RELEASE
                    || action.action != resource.0.0
                    || action.body != at.body
                    || action.site != at.site
                {
                    return Err(fail());
                }
            }
        }
    }
    if consumed.len() != input.cleanups.len() {
        return Err(Error::InvalidCleanup(
            input
                .cleanups
                .iter()
                .enumerate()
                .find(|(index, _)| !consumed.contains(index))
                .map_or(MarkerId(0), |(_, row)| row.marker),
        ));
    }
    Ok(())
}
