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
    pub cleanup: SynchronousCleanupPlan,
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
                .synchronous_cleanup(wrapper, inputs.table)
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
    /// Recheck all actual incoming SSA values, every cleanup operand and the
    /// complete wrapper CFG. Neither candidate classification nor a frame hash
    /// substitutes for this independent source authority.
    pub fn check(
        &self,
        units: &[&ValidatedUnit<Ready>],
        table: &LayoutTable,
    ) -> Result<(), String> {
        let wrapper = self.protocol.checked_sources(units, table)?;
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
            for inst in &u.instructions {
                let layout = match *inst {
                    Inst::FinalizeBegin { .. } => {
                        return Err("uncertified callback finalizer".into());
                    }
                    Inst::Drop { address, .. } => {
                        Some(u.types[u.addresses[address.index()].ty.index()].layout)
                    }
                    Inst::Release { layout, .. } | Inst::Reset { layout, .. } => Some(layout),
                    _ => None,
                };
                if let Some(layout) = layout {
                    if table
                        .layout(layout)
                        .is_none_or(|l| matches!(l.kind, LayoutKind::Callable { .. }))
                        || crate::validate::Layouts(table).destructible(layout)
                    {
                        return Err("unproved callback instruction destruction".into());
                    }
                }
            }
            for call in &u.tables.calls {
                match call.callee {
                    Callee::Operation(_) => {}
                    Callee::Indirect(_) => return Err("unknown callback callee".into()),
                    Callee::Direct(_) | Callee::Bound { .. } => {
                        work.push(
                            callee_symbol(u, call)
                                .and_then(|s| s.instance)
                                .ok_or("unknown callback body identity")?,
                        );
                    }
                }
            }
        }
        if self
            .callback_bodies
            .iter()
            .map(|r| r.body)
            .collect::<Vec<_>>()
            != reached.into_iter().collect::<Vec<_>>()
        {
            return Err("callback graph coverage".into());
        }
        for caller in units {
            if self
                .protocol
                .validate(caller, wrapper, callback, table)
                .is_ok()
            {
                return if self.protocol.check_synchronous_cleanup(
                    caller,
                    wrapper,
                    callback,
                    table,
                    &self.cleanup,
                ) {
                    Ok(())
                } else {
                    Err("isolate mechanical cleanup coverage".into())
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
            let layout = match *inst {
                Inst::FinalizeBegin { .. } => {
                    return Err("callback finalizer needs entered cleanup".into());
                }
                Inst::Release { layout, .. } | Inst::Reset { layout, .. } => Some(layout),
                Inst::Drop { address, .. } => {
                    Some(u.types[u.addresses[address.index()].ty.index()].layout)
                }
                _ => None,
            };
            if let Some(layout) = layout {
                if table
                    .layout(layout)
                    .is_none_or(|l| matches!(l.kind, LayoutKind::Callable { .. }))
                    || crate::validate::Layouts(table).destructible(layout)
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
        for call in &u.tables.calls {
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
