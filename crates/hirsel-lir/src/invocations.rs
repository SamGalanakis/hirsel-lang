//! Independent source instance identity and invocation reference. Storage and
//! invocation evidence share ReferenceBody; candidate anchors cannot select it.
use crate::ir::*;
use crate::stage::Stage;
use hirsel_core_contract::OperationId;
use hirsel_syntax::DefPath;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A source call's identity within its independently supplied definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceInvocation {
    pub body: u32,
    pub block: u32,
    pub statement: u32,
    pub restart: bool,
    pub primitive: Option<OperationId>,
    pub callee: Option<DefPath>,
}

/// Body selected by the independent source instance plan. Candidate anchors
/// cannot select which source invocation inventory they are checked against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceBody {
    pub definition: DefPath,
    pub instance: hirsel_syntax::ContentHash,
    pub body: u32,
}

hirsel_codec::snapshot_struct!(ReferenceBody {
    definition,
    instance,
    body
});

/// Check static event coverage as well as path balance. Deleting both entry and
/// retirement cannot satisfy the independently reconstructed source inventory.
pub fn check<S: Stage>(
    unit: &LirUnit<S>,
    reference: &[ReferenceInvocation],
    body: u32,
) -> Result<(), String> {
    if unit
        .tables
        .source_anchors
        .iter()
        .any(|anchor| anchor.body != body)
    {
        return Err("source anchor differs from independently selected body".into());
    }
    let expected: BTreeSet<_> = reference
        .iter()
        .filter(|site| site.body == body && !site.restart)
        .map(|site| (site.body, site.block, site.statement))
        .collect();
    let mut found = BTreeSet::new();
    for instruction in &unit.instructions {
        if let Inst::FrameEnter { failure } = *instruction {
            let failure = unit
                .tables
                .failures
                .get(failure.index())
                .ok_or("invocation admission failure outside table")?;
            if failure.rule.as_deref() != Some("LANG-10") {
                return Err("invocation admission must fail LANG-10".into());
            }
            let anchor = unit
                .tables
                .source_anchors
                .get(failure.anchor.index())
                .ok_or("invocation admission has no source identity")?;
            if !found.insert((anchor.body, anchor.block, anchor.statement)) {
                return Err("duplicate semantic invocation admission".into());
            }
        }
    }
    if expected != found {
        return Err(format!(
            "semantic invocation coverage differs: expected {expected:?}, found {found:?}"
        ));
    }
    let references: BTreeMap<_, _> = reference
        .iter()
        .map(|site| ((site.body, site.block, site.statement), site))
        .collect();
    let identity = |anchor: SourceAnchorId| {
        unit.tables
            .source_anchors
            .get(anchor.index())
            .map(|a| (a.body, a.block, a.statement))
            .ok_or("invocation source anchor outside table")
    };
    let mut incoming = BTreeMap::from([(unit.entry, None)]);
    let mut pending = VecDeque::from([unit.entry]);
    let edge = |id: EdgeId| {
        unit.tables
            .edges
            .get(id.index())
            .map(|edge| edge.target)
            .ok_or("invocation edge outside table")
    };
    while let Some(block) = pending.pop_front() {
        let mut active = incoming[&block];
        let mut outgoing = Vec::new();
        let instructions = unit.blocks[block.index()]
            .instructions
            .get(&unit.instructions)
            .ok_or("invocation instruction range outside table")?;
        for instruction in instructions {
            // An unsuccessful admission has not added its activation.
            if let Some(failure) = instruction.failure() {
                let cleanup = unit
                    .tables
                    .failures
                    .get(failure.index())
                    .ok_or("invocation failure outside table")?
                    .cleanup;
                outgoing.push((edge(cleanup)?, active));
            }
            match instruction {
                Inst::FrameEnter { failure } => {
                    if active.is_some() {
                        return Err("nested source invocation admission in one unit".into());
                    }
                    active = Some(identity(unit.tables.failures[failure.index()].anchor)?);
                }
                Inst::FrameLeave => {
                    if active.take().is_none() {
                        return Err("invocation retirement without admission".into());
                    }
                }
                _ => (),
            }
        }
        let terminator = &unit.terminators[block.index()];
        if matches!(
            terminator,
            Terminator::Return(_) | Terminator::Propagate | Terminator::RejectTransfer
        ) && active.is_some()
        {
            return Err("semantic invocation remains live at unit exit".into());
        }
        if let Terminator::Call { call, .. } = terminator {
            let call = unit
                .tables
                .calls
                .get(call.index())
                .ok_or("invocation call outside table")?;
            let id = identity(call.anchor)?;
            if let Some(site) = references.get(&id).filter(|site| !site.restart) {
                if active != Some(id) {
                    return Err("language call occurs outside its admitted invocation".into());
                }
                if let Some(expected) = &site.callee {
                    let actual = match call.callee {
                        Callee::Direct(symbol) => unit
                            .tables
                            .symbols
                            .get(symbol.index())
                            .map(|s| &s.definition),
                        Callee::Operation(operation) => {
                            let operation = unit
                                .tables
                                .operations
                                .get(operation.index())
                                .ok_or("invocation operation outside table")?;
                            if site.primitive.as_ref() != Some(operation) {
                                return Err(
                                    "invocation primitive declaration identity differs".into()
                                );
                            }
                            Some(&operation.definition)
                        }
                        _ => None,
                    };
                    // FACT-3's canonical spare-capacity implementation is the
                    // same source primitive. It retains this admitted scope;
                    // no arbitrary helper or same-signature target is equivalent.
                    let spare_push = expected.is_core("list.push")
                        && actual.is_some_and(|path| path.is_core("$native.list_push_spare"))
                        && site
                            .primitive
                            .as_ref()
                            .is_some_and(|op| op.definition == *expected);
                    if actual != Some(expected) && !spare_push {
                        return Err(
                            "invocation callee identity differs from source declaration".into()
                        );
                    }
                }
            }
        }
        outgoing.extend(
            terminator
                .normal_edges(&unit.pools)
                .into_iter()
                .map(|e| edge(e).map(|b| (b, active)))
                .collect::<Result<Vec<_>, _>>()?,
        );
        for failure in terminator.failure_edges(&unit.tables) {
            outgoing.push((edge(unit.tables.failures[failure.index()].cleanup)?, active));
        }
        for (target, state) in outgoing {
            match incoming.get(&target) {
                Some(previous) if *previous != state => {
                    return Err("invocation identity differs at control-flow join".into());
                }
                Some(_) => (),
                None => {
                    incoming.insert(target, state);
                    pending.push_back(target);
                }
            }
        }
    }
    Ok(())
}

hirsel_codec::snapshot_struct!(ReferenceInvocation {
    body,
    block,
    statement,
    restart,
    primitive,
    callee
});

/// Compiler library bodies have no source TIR body. Reconstruct each callback
/// boundary from its typed indirect/bound Call, rather than producer counters.
/// Removing both events while retaining a callback is therefore rejected.
pub fn check_library<S: Stage>(unit: &LirUnit<S>) -> Result<(), String> {
    check_library_with_declarations(unit, &())
}

pub fn check_library_with_declarations<S: Stage>(
    unit: &LirUnit<S>,
    declarations: &dyn crate::operation::OperationDeclarations,
) -> Result<(), String> {
    check_library_context(unit, declarations, None)
}

/// Validate typed compiler forwarding with the independently supplied table.
pub fn check_library_with_layouts<S: Stage>(
    unit: &LirUnit<S>,
    declarations: &dyn crate::operation::OperationDeclarations,
    layouts: &crate::LayoutTable,
) -> Result<(), String> {
    check_library_context(unit, declarations, Some(layouts))
}

fn check_library_context<S: Stage>(
    unit: &LirUnit<S>,
    declarations: &dyn crate::operation::OperationDeclarations,
    layouts: Option<&crate::LayoutTable>,
) -> Result<(), String> {
    let forwarded = if unit.header.definition.is_core("iter.next") {
        Some(iterator_forward(
            unit,
            layouts.ok_or("iterator forwarding lacks its ABI table")?,
        )?)
    } else {
        None
    };
    // Canonical library recipes are independent of candidate calls/events.
    // Even an empty callback must preserve its invocation and dispatch witness.
    let expected = if forwarded.is_some() {
        Some(0)
    } else if unit.header.definition.is_core("map.upsert_with") {
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
    .any(|name| unit.header.definition.is_core(name))
    {
        Some(1)
    } else {
        None
    };
    if let Some(expected) = expected {
        let entries = unit
            .instructions
            .iter()
            .filter(|i| matches!(i, Inst::FrameEnter { .. }))
            .count();
        if entries != expected {
            return Err("generated callback recipe invocation coverage differs".into());
        }
    }
    let edge = |id: EdgeId| {
        unit.tables
            .edges
            .get(id.index())
            .map(|e| e.target)
            .ok_or_else(|| "library invocation edge outside table".to_owned())
    };
    let mut states = BTreeMap::from([(unit.entry, (false, false))]);
    let mut pending = VecDeque::from([unit.entry]);
    while let Some(block) = pending.pop_front() {
        let (mut active, mut dispatched) = states[&block];
        let mut outgoing = Vec::new();
        let row = unit
            .blocks
            .get(block.index())
            .ok_or("library invocation block")?;
        for inst in row
            .instructions
            .get(&unit.instructions)
            .ok_or("library invocation instructions")?
        {
            if let Some(failure) = inst.failure() {
                let failure = unit
                    .tables
                    .failures
                    .get(failure.index())
                    .ok_or("library invocation failure")?;
                outgoing.push((edge(failure.cleanup)?, (active, dispatched)));
            }
            match *inst {
                Inst::FrameEnter { failure } => {
                    if active
                        || unit.tables.failures[failure.index()].rule.as_deref() != Some("LANG-10")
                    {
                        return Err("invalid library callback admission".into());
                    }
                    active = true;
                    dispatched = false;
                }
                Inst::FrameLeave => {
                    if !active || (!dispatched && row.kind != BlockKind::FailureCleanup) {
                        return Err("library retirement has no callback invocation".into());
                    }
                    active = false;
                    dispatched = false;
                }
                _ => (),
            }
        }
        let term = unit
            .terminators
            .get(block.index())
            .ok_or("library invocation terminator")?;
        if let Terminator::Call { call: id, .. } = term {
            let call = unit
                .tables
                .calls
                .get(id.index())
                .ok_or("library invocation call")?;
            let source_direct = match call.callee {
                Callee::Direct(id) => unit.tables.symbols.get(id.index()).is_some_and(|symbol| {
                    symbol.instance.is_some_and(|instance| {
                        declarations
                            .invocation_body(&symbol.definition, instance)
                            .is_some()
                    })
                }),
                _ => false,
            };
            if Some(*id) != forwarded
                && (source_direct
                    || matches!(call.callee, Callee::Indirect(_) | Callee::Bound { .. })
                    || active && !call.forward_caller)
            {
                if !active || dispatched {
                    return Err("library callback lacks its own admission".into());
                }
                dispatched = true;
            }
        }
        if matches!(
            term,
            Terminator::Return(_) | Terminator::Propagate | Terminator::RejectTransfer
        ) && active
        {
            return Err("library callback remains active at exit".into());
        }
        for id in term.normal_edges(&unit.pools) {
            outgoing.push((edge(id)?, (active, dispatched)));
        }
        for failure in term.failure_edges(&unit.tables) {
            outgoing.push((
                edge(unit.tables.failures[failure.index()].cleanup)?,
                (active, dispatched),
            ));
        }
        for (target, state) in outgoing {
            match states.get(&target) {
                Some(previous) if *previous != state => {
                    return Err("library callback scope differs at join".into());
                }
                Some(_) => (),
                None => {
                    states.insert(target, state);
                    pending.push_back(target);
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
    layouts: &crate::LayoutTable,
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
        || abi.shape != crate::abi_types::CallableShape::IterNext
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
