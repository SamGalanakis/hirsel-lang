//! H's own control-flow audit against the independently supplied invocation
//! reference. Candidate units supply events, never the expected event inventory.
use crate::violation::{Checked, ViolationKind as V, err};
use hirsel_lir::{invocations::ReferenceInvocation, ir::*, stage::Stage};
use std::collections::{BTreeMap, BTreeSet};

mod cleanup;
mod flow;

type Identity = (u32, u32, u32);

pub(crate) fn audit<S: Stage>(
    unit: &LirUnit<S>,
    references: &[ReferenceInvocation],
    body: u32,
    bodies: &[hirsel_lir::invocations::ReferenceBody],
    layouts: &hirsel_lir::LayoutTable,
) -> Checked<()> {
    let malformed = |message: &str| err(V::FrameImbalance, message);
    if unit.tables.source_anchors.iter().any(|a| a.body != body) {
        return Err(malformed(
            "source anchor differs from independently selected body",
        ));
    }
    let mut references = references.to_vec();
    let cleanups = cleanup::targets(unit, bodies, layouts, body)?;
    let model = flow::Model::new(unit, &cleanups, layouts)?;
    references.extend(cleanups.iter().map(|target| target.reference.clone()));
    let expected: BTreeMap<_, _> = references
        .iter()
        .filter(|r| r.body == body && !r.restart)
        .map(|r| ((r.body, r.block, r.statement), r))
        .collect();
    let admitted: BTreeSet<_> = expected
        .iter()
        .filter(|(_, r)| r.admitted())
        .map(|(id, _)| *id)
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
                || !admitted.contains(&id)
                || !found.insert(id)
            {
                return Err(malformed(
                    "invocation event disagrees with its independent reference",
                ));
            }
        }
    }
    if found != admitted {
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
    let mut states = vec![None::<flow::State>; unit.blocks.len()];
    let entry = states
        .get_mut(unit.entry.index())
        .ok_or_else(|| malformed("invocation entry is outside CFG"))?;
    *entry = Some(flow::State::default());
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
            model.update(&mut active, inst)?;
            match *inst {
                Inst::FrameEnter { failure } => model.enter(
                    &mut active,
                    identity(unit.tables.failures[failure.index()].anchor)?,
                )?,
                Inst::FrameLeave => {
                    active.leave(false)?;
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
                let scope = if reference.admitted() {
                    active.current() == Some(&model.key(id))
                } else {
                    active.empty()
                };
                if !scope {
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
        ) && !active.empty()
        {
            return Err(malformed("invocation remains live at source unit exit"));
        }
        if let Some(paths) = model.split(unit, block, &active)? {
            for (edge, state) in paths {
                successors.push((target(edge)?, state));
            }
        } else {
            for edge in term.normal_edges(&unit.pools) {
                successors.push((target(edge)?, active.clone()));
            }
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
                Some(previous) => {
                    if previous.merge(&state)? {
                        pending.push(block);
                    }
                }
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
    layouts: &hirsel_lir::LayoutTable,
) -> Checked<()> {
    let malformed = |text: &str| err(V::FrameImbalance, text);
    let cleanup = cleanup::targets(unit, bodies, layouts, 0)?;
    let model = flow::Model::new(unit, &cleanup, layouts)?;
    let expected: BTreeSet<_> = cleanup
        .iter()
        .map(|target| {
            let reference = &target.reference;
            (reference.body, reference.block, reference.statement)
        })
        .collect();
    let mut found = BTreeSet::new();
    for instruction in &unit.instructions {
        if let Inst::FrameEnter { failure } = instruction {
            let origin = unit
                .tables
                .failures
                .get(failure.index())
                .and_then(|failure| unit.tables.source_anchors.get(failure.anchor.index()))
                .ok_or_else(|| malformed("cleanup admission has no source identity"))?;
            if origin.block == u32::MAX
                && !found.insert((origin.body, origin.block, origin.statement))
            {
                return Err(malformed("duplicate cleanup invocation admission"));
            }
        }
    }
    if found != expected {
        return Err(malformed(
            "cleanup invocation coverage differs from typed finalizers",
        ));
    }
    // H reconstructs the static callback recipe from canonical library identity.
    let path = &unit.header.definition;
    let forwarded = if path.is_core("iter.next") {
        Some(iterator_forward(unit, layouts).map_err(|error| malformed(&error))?)
    } else {
        None
    };
    let callbacks = if forwarded.is_some() {
        Some(0)
    } else if path.is_core("map.upsert_with") {
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
            != expected + cleanup.len()
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
        .ok_or_else(|| malformed("callback entry outside CFG"))? = Some(flow::State::default());
    let mut pending = vec![unit.entry];
    while let Some(block) = pending.pop() {
        let mut active = states[block.index()].clone().unwrap();
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
                exits.push((target(failure.cleanup)?, active.clone()));
            }
            model.update(&mut active, inst)?;
            match *inst {
                Inst::FrameEnter { failure } => {
                    if unit.tables.failures[failure.index()].rule.as_deref() != Some("LANG-10") {
                        return Err(malformed("invalid generated callback entry"));
                    }
                    let origin = &unit.tables.source_anchors
                        [unit.tables.failures[failure.index()].anchor.index()];
                    model.enter(&mut active, (origin.body, origin.block, origin.statement))?;
                }
                Inst::FrameLeave => {
                    active.leave(row.kind != BlockKind::FailureCleanup)?;
                }
                _ => (),
            }
        }
        let term = unit
            .terminators
            .get(block.index())
            .ok_or_else(|| malformed("callback terminator"))?;
        if let Terminator::Call { call: id, .. } = term {
            let call = unit
                .tables
                .calls
                .get(id.index())
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
            if Some(*id) != forwarded
                && (source_direct
                    || matches!(call.callee, Callee::Indirect(_) | Callee::Bound { .. })
                    || !active.empty() && !call.forward_caller)
            {
                active.dispatch()?;
            }
        }
        if matches!(
            term,
            Terminator::Return(_) | Terminator::Propagate | Terminator::RejectTransfer
        ) && !active.empty()
        {
            return Err(malformed("generated callback remains live at exit"));
        }
        if let Some(paths) = model.split(unit, block, &active)? {
            for (edge, state) in paths {
                exits.push((target(edge)?, state));
            }
        } else {
            for edge in term.normal_edges(&unit.pools) {
                exits.push((target(edge)?, active.clone()));
            }
        }
        for failure in term.failure_edges(&unit.tables) {
            exits.push((
                target(unit.tables.failures[failure.index()].cleanup)?,
                active.clone(),
            ));
        }
        for (target, state) in exits {
            let slot = states
                .get_mut(target.index())
                .ok_or_else(|| malformed("callback target"))?;
            match slot {
                Some(previous) => {
                    if previous.merge(&state)? {
                        pending.push(target);
                    }
                }
                None => {
                    *slot = Some(state);
                    pending.push(target);
                }
            }
        }
    }
    Ok(())
}

/// Recognize only the canonical forwarding recipe, never an arbitrary helper.
/// It borrows the declared iterator parameter and invokes its typed compiler
/// thunk exactly once before any successful return, without adding a source
/// activation of its own.
fn iterator_forward<S: Stage>(
    unit: &LirUnit<S>,
    layouts: &hirsel_lir::LayoutTable,
) -> Result<CallId, String> {
    let refuse = || "iterator forwarding differs from its canonical recipe".to_owned();
    if unit.tables.calls.len() != 1 {
        return Err(refuse());
    }
    let call = &unit.tables.calls[0];
    let own = layouts
        .fn_abis
        .get(unit.header.abi.0 as usize)
        .ok_or_else(refuse)?;
    let abi = layouts
        .fn_abis
        .get(call.abi.0 as usize)
        .ok_or_else(refuse)?;
    if own.parameters.len() != 1
        || own.parameters[0].source != 0
        || own.parameters[0].convention != hirsel_core_contract::Passing::Mut
        || abi.shape != hirsel_lir::abi_types::CallableShape::IterNext
        || !abi.parameters.is_empty()
        || abi.result != own.result
        || !call.forward_caller
        || call.pretransfer_failure.is_some()
        || !call.operation_arguments.is_empty()
        || call.arguments.len != 0
    {
        return Err(refuse());
    }
    let value = match call.callee {
        Callee::Indirect(value) | Callee::Bound { closure: value, .. } => value,
        _ => return Err(refuse()),
    };
    let address = unit
        .instructions
        .iter()
        .find_map(|instruction| match instruction {
            Inst::Read {
                result,
                address,
                mode: ReadMode::Borrow,
                ..
            } if *result == value => Some(*address),
            _ => None,
        })
        .ok_or_else(refuse)?;
    let info = unit.addresses.get(address.index()).ok_or_else(refuse)?;
    let AddressBase::Slot(slot) = info.base else {
        return Err(refuse());
    };
    if info.projection.len != 0
        || unit
            .slots
            .get(slot.index())
            .is_none_or(|slot| slot.kind != SlotKind::Parameter(0))
    {
        return Err(refuse());
    }
    let Holding::Borrowed(loan) = unit.values.get(value.index()).ok_or_else(refuse)?.holding else {
        return Err(refuse());
    };
    if unit
        .tables
        .loans
        .get(loan.index())
        .is_none_or(|loan| !loan.mutable || loan.address != address)
    {
        return Err(refuse());
    }
    let mut pending = vec![unit.entry];
    let mut seen = BTreeSet::new();
    let mut dispatched = false;
    while let Some(block) = pending.pop() {
        if !seen.insert(block) {
            continue;
        }
        let term = unit.terminators.get(block.index()).ok_or_else(refuse)?;
        if let Terminator::Call { call, .. } = term {
            if *call != CallId(0) {
                return Err(refuse());
            }
            dispatched = true;
            continue;
        }
        if matches!(term, Terminator::Return(_)) {
            return Err(refuse());
        }
        for edge in term.normal_edges(&unit.pools) {
            pending.push(
                unit.tables
                    .edges
                    .get(edge.index())
                    .ok_or_else(refuse)?
                    .target,
            );
        }
    }
    if !dispatched {
        return Err(refuse());
    }
    Ok(CallId(0))
}
