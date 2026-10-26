//! Scope-specific provenance carried by the shared entered-operation proof.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ScopeOpPlan {
    /// The unique installation whose scope SSA value reaches both exits.
    pub enter: CallId,
    pub scope: ValueId,
    /// The proved known callable invocation between installation and exit.
    pub callback: CallId,
}

pub(super) struct Wrapper {
    pub scope: ValueId,
    pub enter: CallId,
    pub callback: CallId,
    pub exits: [CallId; 2],
    pub until: bool,
}

fn argument(u: &LirUnit<Ready>, call: CallId, parameter: u32) -> Option<ValueId> {
    let arg = u
        .tables
        .calls
        .get(call.index())?
        .arguments
        .get(&u.pools.call_arguments)?
        .iter()
        .find(|a| a.parameter == parameter)?;
    match arg.value {
        ArgumentValue::Value(v) => Some(v),
        _ => None,
    }
}

fn operation<'a>(u: &'a LirUnit<Ready>, call: &CallSite) -> Option<&'a DefPath> {
    match call.callee {
        Callee::Operation(op) => Some(&u.tables.operations.get(op.index())?.definition),
        _ => None,
    }
}

fn whole(u: &LirUnit<Ready>, a: AddressId) -> Option<SlotId> {
    let a = u.addresses.get(a.index())?;
    if a.projection.len != 0 {
        return None;
    }
    match a.base {
        AddressBase::Slot(s) => Some(s),
        AddressBase::Address(a) => whole(u, a),
        _ => None,
    }
}

/// A conservative recursive callable provenance walk, independently of RC's
/// widening analysis. A cycle or unsupported carrier refuses admission.
fn code(u: &LirUnit<Ready>, value: ValueId, active: &mut BTreeSet<ValueId>) -> Option<Symbol> {
    if !active.insert(value) {
        return None;
    }
    let mut sources = Vec::new();
    let mut found = None;
    for inst in &u.instructions {
        match *inst {
            Inst::Closure {
                result, closure, ..
            } if result == value => {
                let code = u.tables.closures.get(closure.index())?.code?;
                found = Some(u.tables.symbols.get(code.symbol.index())?.clone());
            }
            Inst::FunctionRef { result, symbol } if result == value => {
                found = Some(u.tables.symbols.get(symbol.index())?.clone());
            }
            Inst::Read {
                result,
                address,
                reaching,
                ..
            }
            | Inst::Copy {
                result,
                address,
                reaching,
                ..
            } if result == value => {
                let slot = whole(u, address)?;
                if u.tables
                    .loans
                    .iter()
                    .any(|loan| loan.mutable && whole(u, loan.address) == Some(slot))
                {
                    return None;
                }
                sources.push(reaching);
            }
            Inst::Store {
                version,
                value: from,
                address,
            }
            | Inst::Replace {
                version,
                value: from,
                address,
                ..
            } if version == value => {
                whole(u, address)?;
                sources.push(from);
            }
            Inst::Weaken {
                result,
                value: from,
            } if result == value => sources.push(from),
            _ => {}
        }
    }
    for edge in &u.tables.edges {
        let params = u
            .blocks
            .get(edge.target.index())?
            .parameters
            .get(&u.pools.values)?;
        if let Some(index) = params.iter().position(|&p| p == value) {
            sources.push(*edge.arguments.get(&u.pools.values)?.get(index)?);
        }
    }
    for source in sources {
        let target = code(u, source, active)?;
        if found.as_ref().is_some_and(|previous| previous != &target) {
            return None;
        }
        found = Some(target);
    }
    active.remove(&value);
    found
}

/// Reconstruct the specialized wrapper from concrete calls and every caller's
/// callable actual. No header name or imported hash is sufficient by itself.
pub(super) fn wrapper(
    u: &LirUnit<Ready>,
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
) -> Option<Wrapper> {
    let until = if u.header.definition.is_core("task.scope") {
        false
    } else if u.header.definition.is_core("task.scope_until") {
        true
    } else {
        return None;
    };
    if u.header.origin != UnitOrigin::Library || u.tables.calls.len() != 4 {
        return None;
    }
    let (enter_name, exit_name) = if until {
        (
            "$intrinsic.TaskScopeEnterUntil",
            "$intrinsic.TaskScopeExitUntil",
        )
    } else {
        ("$intrinsic.TaskScopeEnter", "$intrinsic.TaskScopeExit")
    };
    let mut enter = None;
    let mut exits = Vec::new();
    let mut callback = None;
    for (index, call) in u.tables.calls.iter().enumerate() {
        let id = CallId(index as u32);
        match operation(u, call) {
            Some(definition) if definition.is_core(enter_name) && enter.is_none() => {
                enter = Some(id)
            }
            Some(definition) if definition.is_core(exit_name) => exits.push(id),
            None if callback.is_none()
                && matches!(call.callee, Callee::Bound { .. } | Callee::Direct(_)) =>
            {
                callback = Some(id)
            }
            _ => return None,
        }
    }
    let (enter, callback) = (enter?, callback?);
    let exits: [CallId; 2] = exits.try_into().ok()?;
    let scope = u.tables.calls[enter.index()].result?;
    if exits
        .iter()
        .any(|&exit| argument(u, exit, 0) != Some(scope))
        || argument(u, callback, 0) != Some(scope)
    {
        return None;
    }
    let cb = &u.tables.calls[callback.index()];
    let target = callee_symbol(u, cb)?;
    let body = units.get(&target.instance?)?;
    if body.header.definition != target.definition
        || body.header.abi != cb.abi
        || target.kind != SymbolKind::Function
    {
        return None;
    }
    let source = if until { 2 } else { 1 };
    if let Callee::Bound { closure, .. } = cb.callee {
        let address = u.instructions.iter().find_map(|inst| match *inst {
            Inst::Read {
                result, address, ..
            } if result == closure => Some(address),
            _ => None,
        })?;
        if u.slots[whole(u, address)?.index()].kind != SlotKind::Parameter(source) {
            return None;
        }
        if !u
            .tables
            .loans
            .iter()
            .any(|loan| !loan.mutable && loan.address == address)
        {
            return None;
        }
    }
    let mut callers = 0;
    for caller in units.values() {
        for (index, call) in caller.tables.calls.iter().enumerate() {
            if callee_symbol(caller, call).is_some_and(|s| s.instance == Some(u.header.instance)) {
                callers += 1;
                let value = argument(caller, CallId(index as u32), source)?;
                if code(caller, value, &mut BTreeSet::new())? != target {
                    return None;
                }
            }
        }
    }
    if callers == 0 {
        return None;
    }
    let proof = Wrapper {
        scope,
        enter,
        callback,
        exits,
        until,
    };
    flow(u, &proof).then_some(proof)
}

/// Scope membership through every normal, admission and failure edge. A pop
/// completes only the one matching exit; no return may retain an installed scope.
fn flow(u: &LirUnit<Ready>, wrapper: &Wrapper) -> bool {
    let mut work = vec![(u.entry, false)];
    let mut seen = BTreeSet::new();
    while let Some((block, entered)) = work.pop() {
        if !seen.insert((block, entered)) {
            continue;
        }
        for inst in u.blocks[block.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
            if let Some(failure) = inst.failure() {
                let edge = u.tables.failures[failure.index()].cleanup;
                work.push((u.tables.edges[edge.index()].target, entered));
            }
        }
        let term = &u.terminators[block.index()];
        match *term {
            Terminator::Call {
                call,
                normal,
                failure,
            } => {
                let installed = if call == wrapper.enter {
                    if entered {
                        return false;
                    }
                    true
                } else if wrapper.exits.contains(&call) {
                    if !entered {
                        return false;
                    }
                    false
                } else if call == wrapper.callback {
                    if !entered {
                        return false;
                    }
                    true
                } else {
                    return false;
                };
                if let Some(normal) = normal {
                    work.push((u.tables.edges[normal.index()].target, installed));
                }
                let failed = if call == wrapper.enter {
                    false
                } else {
                    installed
                };
                work.push((
                    u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target,
                    failed,
                ));
            }
            Terminator::Return(_) | Terminator::Propagate if entered => return false,
            _ => {
                work.extend(
                    term.normal_edges(&u.pools)
                        .into_iter()
                        .map(|edge| (u.tables.edges[edge.index()].target, entered)),
                );
                work.extend(term.failure_edges(&u.tables).into_iter().map(|failure| {
                    (
                        u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target,
                        entered,
                    )
                }));
            }
        }
    }
    true
}

/// Only a concrete known-callback library body may override the registry's
/// opaque scope classification. Full provenance is checked before planning.
pub(super) fn candidate(u: &LirUnit<Ready>) -> bool {
    (u.header.definition.is_core("task.scope") || u.header.definition.is_core("task.scope_until"))
        && u.header.origin == UnitOrigin::Library
        && u.tables.calls.len() == 4
        && !u
            .tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Indirect(_)))
}

impl Wrapper {
    pub(super) fn operation(&self, call: CallId) -> Option<(RuntimeOpKind, &'static str)> {
        match (call == self.enter, self.exits.contains(&call), self.until) {
            (true, false, false) => {
                Some((RuntimeOpKind::ScopeEnter, "hirsel_rt_v2_task_scope_enter"))
            }
            (true, false, true) => Some((
                RuntimeOpKind::ScopeEnterUntil,
                "hirsel_rt_v2_task_scope_enter_until",
            )),
            (false, true, false) => {
                Some((RuntimeOpKind::ScopeExit, "hirsel_rt_v2_task_scope_exit"))
            }
            (false, true, true) => Some((
                RuntimeOpKind::ScopeExitUntil,
                "hirsel_rt_v2_task_scope_exit_until",
            )),
            _ => None,
        }
    }

    pub(super) fn plan(
        &self,
        u: &LirUnit<Ready>,
        table: &LayoutTable,
        state: &mut ResumeState,
    ) -> Result<(), String> {
        let Some((kind, helper)) = self.operation(state.boundary.call) else {
            return Ok(());
        };
        if state.callee.is_some() {
            return Err("a runtime scope site with a nested call frame".into());
        }
        let mut results = Vec::new();
        if let Some(value) = u.tables.calls[state.boundary.call.index()].result {
            let layout = u.types[u.values[value.index()].ty.index()].layout;
            let (bytes, align) = fixed(table, layout).ok_or("dynamic scope intrinsic result")?;
            let offset = state.bytes.next_multiple_of(align);
            state.bytes = (offset + bytes).next_multiple_of(8);
            results.push(RuntimeResultSlot {
                source: RuntimeResultSource::CallResult(value),
                layout,
                offset,
                bytes,
                align,
            });
        }
        state.runtime = Some(RuntimeOpPlan {
            kind,
            helper: helper.into(),
            results,
            scope: Some(ScopeOpPlan {
                enter: self.enter,
                scope: self.scope,
                callback: self.callback,
            }),
        });
        state.action = ResumeAction::ResumeRuntime(state.boundary.call);
        Ok(())
    }
}
