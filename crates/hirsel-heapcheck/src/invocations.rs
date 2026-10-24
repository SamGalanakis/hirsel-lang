//! H's own control-flow audit against the independently supplied invocation
//! reference. Candidate units supply events, never the expected event inventory.
use crate::violation::{Checked, ViolationKind as V, err};
use hirsel_lir::{invocations::ReferenceInvocation, ir::*, stage::Stage};
use std::collections::{BTreeMap, BTreeSet};

type Identity = (u32, u32, u32);

pub(crate) fn audit<S: Stage>(
    unit: &LirUnit<S>,
    references: &[ReferenceInvocation],
    body: u32,
) -> Checked<()> {
    let malformed = |message: &str| err(V::FrameImbalance, message);
    if unit.tables.source_anchors.iter().any(|a| a.body != body) {
        return Err(malformed(
            "source anchor differs from independently selected body",
        ));
    }
    let expected: BTreeMap<_, _> = references
        .iter()
        .filter(|r| r.body == body && !r.restart)
        .map(|r| ((r.body, r.block, r.statement), r))
        .collect();
    let identity = |id: SourceAnchorId| -> Checked<Identity> {
        let a = unit
            .tables
            .source_anchors
            .get(id.index())
            .ok_or_else(|| malformed("invocation has no source identity"))?;
        Ok((a.body, a.block, a.statement))
    };
    let mut found = BTreeSet::new();
    for inst in &unit.instructions {
        if let Inst::FrameEnter { failure } = inst {
            let failure = unit
                .tables
                .failures
                .get(failure.index())
                .ok_or_else(|| malformed("admission failure is outside its table"))?;
            let id = identity(failure.anchor)?;
            if failure.rule.as_deref() != Some("LANG-10")
                || !expected.contains_key(&id)
                || !found.insert(id)
            {
                return Err(malformed(
                    "invocation event disagrees with its independent reference",
                ));
            }
        }
    }
    if found != expected.keys().copied().collect() {
        return Err(malformed(
            "invocation events omit an independent source invocation",
        ));
    }
    let target = |edge: EdgeId| -> Checked<BlockId> {
        unit.tables
            .edges
            .get(edge.index())
            .map(|e| e.target)
            .ok_or_else(|| malformed("invocation edge is outside its table"))
    };
    let mut states = vec![None::<Vec<Identity>>; unit.blocks.len()];
    let entry = states
        .get_mut(unit.entry.index())
        .ok_or_else(|| malformed("invocation entry is outside CFG"))?;
    *entry = Some(vec![]);
    let mut pending = vec![unit.entry];
    while let Some(block) = pending.pop() {
        let mut active = states[block.index()].clone().unwrap();
        let mut successors = vec![];
        let instructions = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .ok_or_else(|| malformed("invocation instruction range"))?;
        for inst in instructions {
            if let Some(failure) = inst.failure() {
                let failure = unit
                    .tables
                    .failures
                    .get(failure.index())
                    .ok_or_else(|| malformed("invocation instruction failure"))?;
                successors.push((target(failure.cleanup)?, active.clone()));
            }
            match *inst {
                Inst::FrameEnter { failure } => {
                    active.push(identity(unit.tables.failures[failure.index()].anchor)?)
                }
                Inst::FrameLeave => {
                    active
                        .pop()
                        .ok_or_else(|| malformed("retirement has no invocation"))?;
                }
                _ => (),
            }
        }
        let term = unit
            .terminators
            .get(block.index())
            .ok_or_else(|| malformed("invocation terminator"))?;
        if let Terminator::Call { call, .. } = *term {
            let call = unit
                .tables
                .calls
                .get(call.index())
                .ok_or_else(|| malformed("invocation call"))?;
            let id = identity(call.anchor)?;
            if let Some(reference) = expected.get(&id) {
                if active.last() != Some(&id) {
                    return Err(malformed(
                        "call is outside its independent invocation scope",
                    ));
                }
                if let Some(callee) = &reference.callee {
                    let actual = match call.callee {
                        Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => unit
                            .tables
                            .symbols
                            .get(symbol.index())
                            .map(|s| &s.definition),
                        Callee::Operation(operation) => {
                            let operation = unit
                                .tables
                                .operations
                                .get(operation.index())
                                .ok_or_else(|| malformed("invocation primitive"))?;
                            if reference.primitive.as_ref() != Some(operation) {
                                return Err(malformed("primitive identity differs from reference"));
                            }
                            Some(&operation.definition)
                        }
                        Callee::Indirect(_) => None,
                    };
                    // H independently recognizes the fixed FACT-3 library
                    // replacement; source identity still requires list.push's
                    // independent primitive declaration and active invocation.
                    let spare = callee.is_core("list.push")
                        && actual.is_some_and(|path| path.is_core("$native.list_push_spare"))
                        && reference
                            .primitive
                            .as_ref()
                            .is_some_and(|op| op.definition == *callee);
                    if actual != Some(callee) && !spare {
                        return Err(malformed(
                            "callee identity differs from independent invocation",
                        ));
                    }
                }
            }
        }
        if matches!(
            term,
            Terminator::Return(_) | Terminator::Propagate | Terminator::RejectTransfer
        ) && !active.is_empty()
        {
            return Err(malformed("invocation remains live at source unit exit"));
        }
        for edge in term.normal_edges(&unit.pools) {
            successors.push((target(edge)?, active.clone()));
        }
        for failure in term.failure_edges(&unit.tables) {
            let failure = unit
                .tables
                .failures
                .get(failure.index())
                .ok_or_else(|| malformed("invocation terminator failure"))?;
            successors.push((target(failure.cleanup)?, active.clone()));
        }
        for (block, state) in successors {
            let previous = states
                .get_mut(block.index())
                .ok_or_else(|| malformed("invocation successor outside CFG"))?;
            match previous {
                Some(previous) if *previous != state => {
                    return Err(malformed("invocation scopes disagree at control-flow join"));
                }
                Some(_) => (),
                None => {
                    *previous = Some(state);
                    pending.push(block);
                }
            }
        }
    }
    Ok(())
}

/// H reconstructs generated callback admission from typed Call boundaries.
/// This audit has no producer event totals and never calls C's scope checker.
pub(crate) fn audit_library<S: Stage>(
    unit: &LirUnit<S>,
    bodies: &[hirsel_lir::invocations::ReferenceBody],
) -> Checked<()> {
    let malformed = |text: &str| err(V::FrameImbalance, text);
    // H reconstructs the static callback recipe from canonical library identity.
    let path = &unit.header.definition;
    let callbacks = if path.is_core("map.upsert_with") {
        Some(2)
    } else if [
        "arena.with",
        "arena.bounded",
        "mapped.with",
        "mutex.with",
        "task.scope",
        "task.scope_until",
        "task.isolate",
        "$native.task_main",
        "$native.runner_main",
        "$native.iter_from_step",
        "map.update_with",
        "map.update_value_with",
        "sorted_map.get",
        "sorted_map.remove",
        "sorted_map.insert",
    ]
    .iter()
    .any(|name| path.is_core(name))
    {
        Some(1)
    } else {
        None
    };
    if callbacks.is_some_and(|expected| {
        unit.instructions
            .iter()
            .filter(|i| matches!(i, Inst::FrameEnter { .. }))
            .count()
            != expected
    }) {
        return Err(malformed(
            "H generated callback recipe invocation coverage differs",
        ));
    }
    let target = |edge: EdgeId| {
        unit.tables
            .edges
            .get(edge.index())
            .map(|e| e.target)
            .ok_or_else(|| malformed("callback edge outside table"))
    };
    let mut states = vec![None; unit.blocks.len()];
    *states
        .get_mut(unit.entry.index())
        .ok_or_else(|| malformed("callback entry outside CFG"))? = Some((false, false));
    let mut pending = vec![unit.entry];
    while let Some(block) = pending.pop() {
        let (mut live, mut called) = states[block.index()].unwrap();
        let row = &unit.blocks[block.index()];
        let mut exits = vec![];
        for inst in row
            .instructions
            .get(&unit.instructions)
            .ok_or_else(|| malformed("callback instructions"))?
        {
            if let Some(failure) = inst.failure() {
                let failure = unit
                    .tables
                    .failures
                    .get(failure.index())
                    .ok_or_else(|| malformed("callback failure"))?;
                exits.push((target(failure.cleanup)?, (live, called)));
            }
            match *inst {
                Inst::FrameEnter { failure } => {
                    if live
                        || unit.tables.failures[failure.index()].rule.as_deref() != Some("LANG-10")
                    {
                        return Err(malformed("invalid generated callback entry"));
                    }
                    live = true;
                    called = false;
                }
                Inst::FrameLeave => {
                    if !live || (!called && row.kind != BlockKind::FailureCleanup) {
                        return Err(malformed(
                            "generated callback retirement lacks an invocation",
                        ));
                    }
                    live = false;
                    called = false;
                }
                _ => (),
            }
        }
        let term = unit
            .terminators
            .get(block.index())
            .ok_or_else(|| malformed("callback terminator"))?;
        if let Terminator::Call { call, .. } = term {
            let call = unit
                .tables
                .calls
                .get(call.index())
                .ok_or_else(|| malformed("callback call"))?;
            let source_direct = match call.callee {
                Callee::Direct(id) => unit.tables.symbols.get(id.index()).is_some_and(|symbol| {
                    bodies.iter().any(|body| {
                        body.definition == symbol.definition
                            && Some(body.instance) == symbol.instance
                    })
                }),
                _ => false,
            };
            if source_direct
                || matches!(call.callee, Callee::Indirect(_) | Callee::Bound { .. })
                || live && !call.forward_caller
            {
                if !live || called {
                    return Err(malformed("generated callback has no distinct admission"));
                }
                called = true;
            }
        }
        if matches!(
            term,
            Terminator::Return(_) | Terminator::Propagate | Terminator::RejectTransfer
        ) && live
        {
            return Err(malformed("generated callback remains live at exit"));
        }
        for edge in term.normal_edges(&unit.pools) {
            exits.push((target(edge)?, (live, called)));
        }
        for failure in term.failure_edges(&unit.tables) {
            exits.push((
                target(unit.tables.failures[failure.index()].cleanup)?,
                (live, called),
            ));
        }
        for (target, state) in exits {
            let slot = states
                .get_mut(target.index())
                .ok_or_else(|| malformed("callback target"))?;
            match slot {
                Some(previous) if *previous != state => {
                    return Err(malformed("generated callback states disagree"));
                }
                Some(_) => (),
                None => {
                    *slot = Some(state);
                    pending.push(target);
                }
            }
        }
    }
    Ok(())
}
