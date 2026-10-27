//! Source authority for the specialized isolate exception. Candidate names
//! select an analysis, never permission to save a native caller.
use super::*;
use crate::continuation::{CalleeClass, PlanInputs, callee_symbol};
use crate::stage::ValidatedUnit;
use hirsel_syntax::{ContentHash, DefPath};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallbackBody {
    pub body: ContentHash,
    pub digest: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct IsolateEvidence {
    pub protocol: IsolateProtocol,
    pub cleanup: SourceCleanupPlan,
    /// Complete reachable source bodies, including failure-only cleanup.
    pub callback_bodies: Vec<CallbackBody>,
}

pub(crate) fn candidate(unit: &LirUnit<Ready>) -> bool {
    unit.header.origin == UnitOrigin::Library
        && unit.header.definition.is_core("task.isolate")
        && unit
            .tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Bound { .. }))
}

pub(crate) fn intrinsic(definition: &DefPath) -> Option<CalleeClass> {
    if definition.is_core("$intrinsic.IsolateBegin") {
        return Some(CalleeClass::Boundary);
    }
    [
        "$intrinsic.IsolateEnd",
        "$intrinsic.IsolateCount",
        "$intrinsic.IsolateKind",
        "$intrinsic.IsolateRule",
        "$intrinsic.IsolateDefinition",
        "$intrinsic.IsolateLine",
        "$intrinsic.IsolateColumn",
        "$intrinsic.IsolateSpawn",
    ]
    .iter()
    .any(|path| definition.is_core(path))
    .then_some(CalleeClass::Local)
}

pub(crate) fn evidence(
    units: &[&ValidatedUnit<Ready>],
    inputs: &PlanInputs<'_>,
    instance: ContentHash,
) -> Result<IsolateEvidence, String> {
    let wrapper = units
        .iter()
        .copied()
        .find(|u| u.unit().header.instance == instance)
        .ok_or("missing isolate wrapper")?;
    let target = wrapper
        .unit()
        .tables
        .calls
        .iter()
        .find_map(|c| match c.callee {
            Callee::Bound { code, .. } => wrapper.unit().tables.symbols[code.index()].instance,
            _ => None,
        })
        .ok_or("unknown isolate callback")?;
    let callback = units
        .iter()
        .copied()
        .find(|u| u.unit().header.instance == target)
        .ok_or("missing isolate callback")?;
    for caller in units {
        for (id, call) in caller.unit().tables.calls.iter().enumerate() {
            if !callee_symbol(caller.unit(), call).is_some_and(|s| s.instance == Some(instance)) {
                continue;
            }
            let protocol = IsolateProtocol::derive(
                caller,
                CallId(id as u32),
                wrapper,
                callback,
                inputs.environments.get(&target).copied().flatten(),
                inputs.table,
            )
            .ok_or("unproved isolate source protocol")?;
            protocol.checked_callers(units, inputs)?;
            let cleanup = protocol
                .source_cleanup(wrapper, inputs.table)
                .ok_or("isolate cleanup needs a certified entered protocol")?;
            let callback_bodies = callback_graph(units, inputs.table, target)?;
            let evidence = IsolateEvidence {
                protocol,
                cleanup,
                callback_bodies,
            };
            evidence.check(units, inputs.table)?;
            return Ok(evidence);
        }
    }
    Err("no source isolate caller".into())
}

impl IsolateEvidence {
    /// With no ordinary callee plan, only a fully source-covered graph with
    /// no operation calls can be proved direct by this bounded slice.
    pub(crate) fn direct_callback(
        &self,
        units: &std::collections::BTreeMap<ContentHash, &LirUnit<Ready>>,
        table: &LayoutTable,
    ) -> bool {
        self.callback_bodies.iter().all(|body| {
            units.get(&body.body).is_some_and(|u| {
                !u.instructions.iter().any(|inst| match *inst {
                    Inst::Release { layout, .. } | Inst::Reset { layout, .. } => {
                        crate::validate::Layouts(table).destructible(layout)
                    }
                    Inst::Drop { .. } => true,
                    _ => false,
                }) && u
                    .tables
                    .calls
                    .iter()
                    .all(|call| !matches!(call.callee, Callee::Operation(_)))
            })
        })
    }

    /// Recheck all actual incoming SSA values, every cleanup operand and the
    /// complete wrapper CFG. Neither candidate classification nor a frame hash
    /// substitutes for this independent source authority.
    pub fn check(
        &self,
        units: &[&ValidatedUnit<Ready>],
        table: &LayoutTable,
    ) -> Result<(), String> {
        let wrapper = self.protocol.checked_sources(units, table)?;
        for &point in &self.cleanup.finalizers {
            explicit_finalizer(wrapper.unit(), units, table, point)?;
        }
        let callback = units
            .iter()
            .copied()
            .find(|u| Some(u.unit().header.instance) == self.protocol.identity.target.instance)
            .ok_or("missing isolate callback")?;
        let index: std::collections::BTreeMap<_, _> = units
            .iter()
            .map(|u| (u.unit().header.instance, u.unit()))
            .collect();
        let mut reached = BTreeSet::new();
        let mut edges = Vec::new();
        let mut work = vec![callback.unit().header.instance];
        while let Some(body) = work.pop() {
            if !reached.insert(body) {
                continue;
            }
            let u = index.get(&body).ok_or("missing reachable callback body")?;
            let row = self
                .callback_bodies
                .iter()
                .find(|r| r.body == body)
                .ok_or("missing callback destruction evidence")?;
            if row.digest != u.digest() {
                return Err("stale callback body".into());
            }
            for (b, block) in u.blocks.iter().enumerate() {
                for (i, inst) in block
                    .instructions
                    .get(&u.instructions)
                    .ok_or("callback instruction range")?
                    .iter()
                    .enumerate()
                {
                    if matches!(inst, Inst::FinalizeBegin { .. }) {
                        explicit_finalizer(
                            u,
                            units,
                            table,
                            InstructionPoint {
                                block: BlockId(b as u32),
                                instruction: i as u32,
                            },
                        )?;
                    }
                }
            }
            for inst in &u.instructions {
                if super::cleanup::mechanical_callable_release(u, &index, table, inst) {
                    continue;
                }
                let layout = match *inst {
                    Inst::FinalizeBegin { .. } => None,
                    Inst::Drop { address, .. } => {
                        Some(u.types[u.addresses[address.index()].ty.index()].layout)
                    }
                    Inst::Release { layout, .. } | Inst::Reset { layout, .. } => Some(layout),
                    _ => None,
                };
                if let Some(layout) = layout {
                    if !candidate(u) && !known_cleanup(table, layout) {
                        return Err("erased callback destruction graph".into());
                    }
                    if table.layout(layout).is_none_or(|l| {
                        matches!(l.kind, LayoutKind::Callable { .. }) && !candidate(u)
                    }) || (crate::validate::Layouts(table).destructible(layout)
                        && !matches!(
                            inst,
                            Inst::Release {
                                live: MaskRef::All,
                                ..
                            }
                        ))
                    {
                        return Err("unproved callback instruction destruction".into());
                    }
                }
            }
            for term in &u.terminators {
                let Terminator::Call { call, .. } = *term else {
                    continue;
                };
                let call = &u.tables.calls[call.index()];
                match call.callee {
                    Callee::Operation(_) => {}
                    Callee::Indirect(_) => return Err("unknown callback callee".into()),
                    Callee::Direct(_) | Callee::Bound { .. } => {
                        let target = callee_symbol(u, call)
                            .and_then(|s| s.instance)
                            .ok_or("unknown callback body identity")?;
                        edges.push((body, target));
                        work.push(target);
                    }
                }
            }
        }
        if self
            .callback_bodies
            .iter()
            .map(|r| r.body)
            .collect::<Vec<_>>()
            != reached.iter().copied().collect::<Vec<_>>()
        {
            return Err("callback graph coverage".into());
        }
        // Independently reject recursive source graphs, even when a forged
        // callee table claims that every member has a compact frame.
        let mut degree: std::collections::BTreeMap<_, usize> =
            reached.iter().map(|id| (*id, 0)).collect();
        for (_, to) in &edges {
            *degree.get_mut(to).ok_or("unknown callback edge")? += 1;
        }
        let mut ready: Vec<_> = degree
            .iter()
            .filter_map(|(id, n)| (*n == 0).then_some(*id))
            .collect();
        let mut retired = 0;
        while let Some(id) = ready.pop() {
            retired += 1;
            for (_, to) in edges.iter().filter(|(from, _)| *from == id) {
                let n = degree.get_mut(to).unwrap();
                *n -= 1;
                if *n == 0 {
                    ready.push(*to);
                }
            }
        }
        if retired != reached.len() {
            return Err("recursive callback source graph".into());
        }
        for caller in units {
            if self
                .protocol
                .validate(caller, wrapper, callback, table)
                .is_ok()
            {
                return if self.protocol.check_source_cleanup(
                    caller,
                    wrapper,
                    callback,
                    table,
                    &self.cleanup,
                ) {
                    Ok(())
                } else {
                    Err("isolate source cleanup coverage".into())
                };
            }
        }
        Err("isolate source custody".into())
    }
}

/// Producer walk; the import checker above instead validates supplied rows
/// while traversing the actual graph and instructions independently.
fn callback_graph(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    root: ContentHash,
) -> Result<Vec<CallbackBody>, String> {
    let index: std::collections::BTreeMap<_, _> = units
        .iter()
        .map(|u| (u.unit().header.instance, u.unit()))
        .collect();
    let mut done = std::collections::BTreeMap::new();
    let mut pending = vec![root];
    while let Some(body) = pending.pop() {
        if done.contains_key(&body) {
            continue;
        }
        let u = index.get(&body).ok_or("unavailable callback source")?;
        for inst in &u.instructions {
            if super::cleanup::mechanical_callable_release(u, &index, table, inst) {
                continue;
            }
            let layout = match *inst {
                Inst::FinalizeBegin { .. } => None,
                Inst::Release { layout, .. } | Inst::Reset { layout, .. } => Some(layout),
                Inst::Drop { address, .. } => {
                    Some(u.types[u.addresses[address.index()].ty.index()].layout)
                }
                _ => None,
            };
            if let Some(layout) = layout {
                if !candidate(u) && !known_cleanup(table, layout) {
                    return Err(format!(
                        "unproved callback cleanup descriptor {} {layout:?}",
                        u.header.definition
                    ));
                }
                if table
                    .layout(layout)
                    .is_none_or(|l| matches!(l.kind, LayoutKind::Callable { .. }) && !candidate(u))
                    || (crate::validate::Layouts(table).destructible(layout)
                        && !matches!(
                            inst,
                            Inst::Release {
                                live: MaskRef::All,
                                ..
                            }
                        ))
                {
                    return Err("callback cleanup needs physical custody proof".into());
                }
            }
        }
        done.insert(
            body,
            CallbackBody {
                body,
                digest: u.digest(),
            },
        );
        for term in &u.terminators {
            let Terminator::Call { call, .. } = *term else {
                continue;
            };
            let call = &u.tables.calls[call.index()];
            if matches!(call.callee, Callee::Operation(_)) {
                continue;
            }
            pending.push(
                callee_symbol(u, call)
                    .and_then(|s| s.instance)
                    .ok_or("unknown transitive callback call")?,
            );
        }
    }
    Ok(done.into_values().collect())
}

impl IsolateEvidence {
    pub(crate) fn check_callback_cleanup(
        &self,
        units: &std::collections::BTreeMap<ContentHash, &LirUnit<Ready>>,
        table: &LayoutTable,
        plans: &std::collections::BTreeMap<ContentHash, super::super::ContinuationPlan>,
    ) -> Result<(), String> {
        for body in &self.callback_bodies {
            let u = units
                .get(&body.body)
                .ok_or("missing callback cleanup body")?;
            if candidate(u) {
                if !plans.get(&body.body).is_some_and(|p| {
                    p.representation == super::super::Representation::Resumable
                        && p.isolate.is_some()
                }) {
                    return Err("unproved nested isolate cleanup".into());
                }
                continue;
            }
            for (b, block) in u.blocks.iter().enumerate() {
                for (i, inst) in block
                    .instructions
                    .get(&u.instructions)
                    .ok_or("callback instruction range")?
                    .iter()
                    .enumerate()
                {
                    if let Inst::Release { layout, .. } = *inst
                        && !super::cleanup::mechanical_callable_release(u, units, table, inst)
                        && crate::validate::Layouts(table).destructible(layout)
                        && !plans.get(&body.body).is_some_and(|p| {
                            p.representation == super::super::Representation::Resumable
                                && p.cleanups.iter().any(|c| {
                                    c.block == BlockId(b as u32) && c.instruction == i as u32
                                })
                        })
                    {
                        return Err("callback destruction lacks retained cursor custody".into());
                    }
                }
            }
        }
        Ok(())
    }
}
