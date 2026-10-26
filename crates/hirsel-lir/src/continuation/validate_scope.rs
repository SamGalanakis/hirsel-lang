//! Independent scope provenance and entered-result validation. This walk does
//! not invoke the producer's wrapper, flow, callable or ScopeOpPlan builders.
use super::*;

struct Facts {
    enter: CallId,
    callback: CallId,
    scope: ValueId,
    operations: BTreeMap<CallId, (RuntimeOpKind, &'static str)>,
}

fn arg(u: &LirUnit<Ready>, call: CallId, parameter: u32) -> Option<ValueId> {
    let args = u
        .tables
        .calls
        .get(call.index())?
        .arguments
        .get(&u.pools.call_arguments)?;
    let mut args = args.iter().filter(|arg| arg.parameter == parameter);
    let (Some(arg), None) = (args.next(), args.next()) else {
        return None;
    };
    if let ArgumentValue::Value(value) = arg.value {
        Some(value)
    } else {
        None
    }
}

fn whole(u: &LirUnit<Ready>, mut address: AddressId) -> Option<SlotId> {
    let mut seen = BTreeSet::new();
    while seen.insert(address) {
        let row = u.addresses.get(address.index())?;
        if row.projection.len != 0 {
            return None;
        }
        match row.base {
            AddressBase::Slot(slot) => return Some(slot),
            AddressBase::Address(next) => address = next,
            _ => return None,
        }
    }
    None
}

#[derive(Clone, PartialEq, Eq)]
enum Code {
    Unresolved,
    Known(Symbol),
    Unknown,
}

/// Widen SSA equations to a fixed point, independently of the producer's
/// recursive origin walk. Every incoming caller must supply the same code.
fn code_at(u: &LirUnit<Ready>, at: ValueId) -> Option<Symbol> {
    let mut sources = vec![Vec::new(); u.values.len()];
    let mut constants = vec![Code::Unknown; u.values.len()];
    for inst in &u.instructions {
        let alias = match *inst {
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
            } => {
                let Some(slot) = whole(u, address) else {
                    continue;
                };
                if u.tables
                    .loans
                    .iter()
                    .any(|l| l.mutable && whole(u, l.address) == Some(slot))
                {
                    None
                } else {
                    Some((result, reaching))
                }
            }
            Inst::Store {
                version,
                value,
                address,
            }
            | Inst::Replace {
                version,
                value,
                address,
                ..
            } if whole(u, address).is_some() => Some((version, value)),
            Inst::Weaken { result, value } => Some((result, value)),
            _ => None,
        };
        if let Some((to, from)) = alias {
            constants[to.index()] = Code::Unresolved;
            sources[to.index()].push(from);
        } else {
            match *inst {
                Inst::Closure {
                    result, closure, ..
                } => {
                    if let Some(code) = u.tables.closures.get(closure.index())?.code {
                        constants[result.index()] =
                            Code::Known(u.tables.symbols.get(code.symbol.index())?.clone());
                    }
                }
                Inst::FunctionRef { result, symbol } => {
                    constants[result.index()] =
                        Code::Known(u.tables.symbols.get(symbol.index())?.clone())
                }
                _ => {}
            }
        }
    }
    for edge in &u.tables.edges {
        for (&to, &from) in params(u, edge.target)
            .iter()
            .zip(edge.arguments.get(&u.pools.values)?)
        {
            constants[to.index()] = Code::Unresolved;
            sources[to.index()].push(from);
        }
    }
    let mut values = constants.clone();
    loop {
        let mut changed = false;
        for i in 0..values.len() {
            let mut value = constants[i].clone();
            for &source in &sources[i] {
                value = match (&value, &values[source.index()]) {
                    (Code::Unknown, _) | (_, Code::Unknown) => Code::Unknown,
                    (Code::Unresolved, other) | (other, Code::Unresolved) => other.clone(),
                    (Code::Known(a), Code::Known(b)) if a == b => Code::Known(a.clone()),
                    _ => Code::Unknown,
                };
            }
            if value != values[i] {
                values[i] = value;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    match &values.get(at.index())? {
        Code::Known(code) => Some(code.clone()),
        _ => None,
    }
}

fn facts(u: &LirUnit<Ready>, units: &BTreeMap<ContentHash, &LirUnit<Ready>>) -> Option<Facts> {
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
    let mut enter = None;
    let mut callback = None;
    let mut exits = Vec::new();
    let mut operations = BTreeMap::new();
    for (i, call) in u.tables.calls.iter().enumerate() {
        let id = CallId(i as u32);
        match call.callee {
            Callee::Operation(op) => {
                let definition = &u.tables.operations.get(op.index())?.definition;
                if definition.is_core(if until {
                    "$intrinsic.TaskScopeEnterUntil"
                } else {
                    "$intrinsic.TaskScopeEnter"
                }) {
                    if enter.replace(id).is_some() {
                        return None;
                    }
                    operations.insert(
                        id,
                        if until {
                            (
                                RuntimeOpKind::ScopeEnterUntil,
                                "hirsel_rt_v2_task_scope_enter_until",
                            )
                        } else {
                            (RuntimeOpKind::ScopeEnter, "hirsel_rt_v2_task_scope_enter")
                        },
                    );
                } else if definition.is_core(if until {
                    "$intrinsic.TaskScopeExitUntil"
                } else {
                    "$intrinsic.TaskScopeExit"
                }) {
                    exits.push(id);
                    operations.insert(
                        id,
                        if until {
                            (
                                RuntimeOpKind::ScopeExitUntil,
                                "hirsel_rt_v2_task_scope_exit_until",
                            )
                        } else {
                            (RuntimeOpKind::ScopeExit, "hirsel_rt_v2_task_scope_exit")
                        },
                    );
                } else {
                    return None;
                }
            }
            Callee::Direct(_) | Callee::Bound { .. } if callback.is_none() => callback = Some(id),
            _ => return None,
        }
    }
    let (enter, callback) = (enter?, callback?);
    if exits.len() != 2 {
        return None;
    }
    let scope = u.tables.calls[enter.index()].result?;
    if exits.iter().any(|&id| arg(u, id, 0) != Some(scope)) || arg(u, callback, 0) != Some(scope) {
        return None;
    }
    let call = &u.tables.calls[callback.index()];
    let target = callee_symbol(u, call)?;
    let body = units.get(&target.instance?)?;
    if target.kind != SymbolKind::Function
        || target.definition != body.header.definition
        || call.abi != body.header.abi
    {
        return None;
    }
    let ordinal = if until { 2 } else { 1 };
    if let Callee::Bound { closure, .. } = call.callee {
        let Holding::Borrowed(loan) = u.values.get(closure.index())?.holding else {
            return None;
        };
        let loan = u.tables.loans.get(loan.index())?;
        let slot = whole(u, loan.address)?;
        if loan.mutable || u.slots.get(slot.index())?.kind != SlotKind::Parameter(ordinal) {
            return None;
        }
        if !u.instructions.iter().any(|i| matches!(*i, Inst::Read {result,address,..} if result==closure && address==loan.address)) { return None; }
    }
    let mut uses = 0;
    for caller in units.values() {
        for (i, call) in caller.tables.calls.iter().enumerate() {
            if callee_symbol(caller, call).is_some_and(|s| s.instance == Some(u.header.instance)) {
                uses += 1;
                if code_at(caller, arg(caller, CallId(i as u32), ordinal)?)? != target {
                    return None;
                }
            }
        }
    }
    if uses == 0 {
        return None;
    }
    // Propagate lexical membership along all instruction, admission and call
    // failure edges. Exit consumes membership on either completed outcome.
    let mut pending = vec![(u.entry, false)];
    let mut visited = BTreeSet::new();
    while let Some((block, mut live)) = pending.pop() {
        if !visited.insert((block, live)) {
            continue;
        }
        for inst in u
            .blocks
            .get(block.index())?
            .instructions
            .get(&u.instructions)?
        {
            if let Some(failure) = inst.failure() {
                pending.push((
                    u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target,
                    live,
                ));
            }
        }
        let term = &u.terminators[block.index()];
        if let Terminator::Call {
            call,
            normal,
            failure,
        } = *term
        {
            let failed = if call == enter {
                if live {
                    return None;
                }
                live = true;
                false
            } else if exits.contains(&call) {
                if !live {
                    return None;
                }
                live = false;
                false
            } else if call == callback {
                if !live {
                    return None;
                }
                true
            } else {
                return None;
            };
            if let Some(normal) = normal {
                pending.push((u.tables.edges[normal.index()].target, live));
            }
            pending.push((
                u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target,
                failed,
            ));
        } else {
            if live && matches!(term, Terminator::Return(_) | Terminator::Propagate) {
                return None;
            }
            pending.extend(
                term.normal_edges(&u.pools)
                    .into_iter()
                    .map(|e| (u.tables.edges[e.index()].target, live)),
            );
            pending.extend(term.failure_edges(&u.tables).into_iter().map(|f| {
                (
                    u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target,
                    live,
                )
            }));
        }
    }
    Some(Facts {
        enter,
        callback,
        scope,
        operations,
    })
}

/// Exact hidden output cell layout, reconstructed from the original intrinsic
/// ABI and result. Unsupported kinds and borrowed/transient result images refuse.
pub(super) fn check(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
    state: &ResumeState,
    mut end: u64,
) -> Result<u64, ContinuationError> {
    let error = ContinuationError::RuntimeOperation(u.header.instance, state.target.state);
    if !u.header.definition.is_core("task.scope")
        && !u.header.definition.is_core("task.scope_until")
    {
        return if state.runtime.is_some() {
            Err(error)
        } else {
            Ok(end)
        };
    }
    let facts = facts(u, units).ok_or(error.clone())?;
    let Some(&(kind, helper)) = facts.operations.get(&state.boundary.call) else {
        return if state.runtime.is_none() {
            Ok(end)
        } else {
            Err(error)
        };
    };
    let runtime = state.runtime.as_ref().ok_or(error.clone())?;
    if runtime.kind != kind
        || runtime.helper != helper
        || runtime.scope
            != Some(ScopeOpPlan {
                enter: facts.enter,
                scope: facts.scope,
                callback: facts.callback,
            })
        || state.callee.is_some()
    {
        return Err(error);
    }
    let call = &u.tables.calls[state.boundary.call.index()];
    match call.result {
        None if runtime.results.is_empty() => {}
        Some(value) => {
            let [slot] = runtime.results.as_slice() else {
                return Err(error);
            };
            let ty = &u.types[u.values[value.index()].ty.index()];
            let (bytes, align) = fixed(table, ty.layout).ok_or(error.clone())?;
            let abi = table
                .fn_abis
                .get(call.abi.0 as usize)
                .ok_or(error.clone())?;
            if abi.result.as_ref().map(|r| r.layout) != Some(ty.layout)
                || slot.source != RuntimeResultSource::CallResult(value)
                || slot.layout != ty.layout
                || slot.bytes != bytes
                || slot.align != align
                || slot.offset != end.next_multiple_of(align)
                || u.values[value.index()].holding != Holding::Plain
            {
                return Err(error);
            }
            end = (slot.offset + bytes).next_multiple_of(8);
        }
        _ => return Err(error),
    }
    Ok(end)
}
