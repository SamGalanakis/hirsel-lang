//! Read-only callable identity equations, adapted from
//! hirsel-rc/src/closure_calls.rs::discover. Diagnostic discovery never rewrites
//! a call or authorizes an ABI; ordinary C remains mandatory. Unknown joins,
//! mutable storage, projections and incoming dynamic callables stay refused.
use super::*;
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Target {
    pub symbol: Symbol,
    pub abi: Option<crate::abi_types::FnAbiId>,
    pub environment: Option<LayoutId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Code {
    Pending,
    Known(Target),
    Dynamic,
}
impl Code {
    pub(super) fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Pending, x) | (x, Self::Pending) => x.clone(),
            (Self::Known(a), Self::Known(b)) if a == b => self.clone(),
            _ => Self::Dynamic,
        }
    }
}
fn whole<S>(u: &LirUnit<S>, mut address: AddressId) -> Option<SlotId> {
    for _ in 0..u.addresses.len() {
        let a = u.addresses.get(address.index())?;
        if a.projection.len != 0 {
            return None;
        }
        match a.base {
            AddressBase::Slot(slot) => return Some(slot),
            AddressBase::Address(parent) => address = parent,
            _ => return None,
        }
    }
    None
}
/// Each identity widens at most twice, pending -> known -> dynamic. The
/// worklist converges on cycles without an iteration or specialization cap.
pub(super) fn discover<S>(u: &LirUnit<S>) -> Vec<Code> {
    discover_seeded(u, &[])
}
fn discover_seeded<S>(u: &LirUnit<S>, seeds: &[Code]) -> Vec<Code> {
    let mut inputs = vec![Vec::new(); u.values.len()];
    let mut constants = vec![Code::Dynamic; u.values.len()];
    let mutable: BTreeSet<_> = u
        .tables
        .loans
        .iter()
        .filter(|loan| loan.mutable)
        .filter_map(|loan| whole(u, loan.address))
        .collect();
    let safe = |address| whole(u, address).is_some_and(|slot| !mutable.contains(&slot));
    for id in super::super::check::used_edges(u) {
        let edge = &u.tables.edges[id.index()];
        for (&p, &a) in u.blocks[edge.target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            constants[p.index()] = Code::Pending;
            inputs[p.index()].push(a);
        }
    }
    for inst in &u.instructions {
        let transfer = match *inst {
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
            } if safe(address) => Some((result, reaching)),
            Inst::Store {
                address,
                value,
                version,
            }
            | Inst::Replace {
                address,
                value,
                version,
                ..
            } if safe(address) => Some((version, value)),
            Inst::Weaken { result, value } | Inst::Recast { result, value } => {
                Some((result, value))
            }
            _ => None,
        };
        if let Some((to, from)) = transfer {
            constants[to.index()] = Code::Pending;
            inputs[to.index()].push(from);
        } else {
            for v in inst.defs() {
                constants[v.index()] = Code::Dynamic;
            }
            match *inst {
                Inst::Closure {
                    result, closure, ..
                } => {
                    if let Some(code) = u.tables.closures[closure.index()].code {
                        constants[result.index()] = Code::Known(Target {
                            symbol: u.tables.symbols[code.symbol.index()].clone(),
                            abi: Some(code.abi),
                            environment: Some(u.tables.closures[closure.index()].environment),
                        });
                    }
                }
                Inst::FunctionRef { result, symbol } => {
                    constants[result.index()] = Code::Known(Target {
                        symbol: u.tables.symbols[symbol.index()].clone(),
                        abi: None,
                        environment: None,
                    });
                }
                _ => (),
            }
        }
    }
    for &value in u.parameters.iter().chain(&u.captures).chain(
        u.tables
            .calls
            .iter()
            .filter_map(|call| call.result)
            .collect::<Vec<_>>()
            .iter(),
    ) {
        if let Some(seed) = seeds.get(value.index()) {
            constants[value.index()] = seed.clone();
        }
    }
    let mut users = vec![Vec::new(); u.values.len()];
    for (to, sources) in inputs.iter().enumerate() {
        for source in sources {
            users[source.index()].push(to);
        }
    }
    let mut codes = constants.clone();
    let mut queue: VecDeque<_> = (0..codes.len()).collect();
    let mut queued = vec![true; codes.len()];
    while let Some(at) = queue.pop_front() {
        queued[at] = false;
        let code = inputs[at]
            .iter()
            .fold(constants[at].clone(), |code, source| {
                code.join(&codes[source.index()])
            });
        if codes[at] != code {
            codes[at] = code;
            for &user in &users[at] {
                if !std::mem::replace(&mut queued[user], true) {
                    queue.push_back(user);
                }
            }
        }
    }
    codes
}

/// Parameter, capture and return equations over the complete checked diagnostic
/// closure. Like local discovery, every seed widens at most twice. No source
/// spelling, callable layout equality or runtime row selects a code identity.
pub(super) fn closed(units: &[&ValidatedUnit<Ready>]) -> Result<Vec<Vec<Code>>, String> {
    solve(&units.iter().map(|unit| unit.unit()).collect::<Vec<_>>())
}
fn solve(units: &[&LirUnit<Ready>]) -> Result<Vec<Vec<Code>>, String> {
    let mut symbols = BTreeMap::new();
    for (index, unit) in units.iter().enumerate() {
        let body = *unit;
        if symbols
            .insert((&body.header.definition, body.header.instance), index)
            .is_some()
        {
            return Err("native ownership tracing: duplicate diagnostic call target".into());
        }
    }
    let resolve = |symbol: &Symbol| -> Result<usize, String> {
        let instance = symbol
            .instance
            .ok_or("native ownership tracing: unspecialized diagnostic call target")?;
        symbols
            .get(&(&symbol.definition, instance))
            .copied()
            .ok_or_else(|| "native ownership tracing: missing diagnostic call target".into())
    };
    let mut seeds = units
        .iter()
        .map(|unit| vec![Code::Pending; unit.values.len()])
        .collect::<Vec<_>>();
    loop {
        let codes = units
            .iter()
            .zip(&seeds)
            .map(|(unit, seeds)| discover_seeded(unit, seeds))
            .collect::<Vec<_>>();
        let mut changed = false;
        let mut merge = |unit: usize, value: ValueId, code: &Code| -> Result<(), String> {
            let seed = seeds
                .get_mut(unit)
                .and_then(|values| values.get_mut(value.index()))
                .ok_or("native ownership tracing: callable binding is absent")?;
            let next = seed.join(code);
            if *seed != next {
                *seed = next;
                changed = true;
            }
            Ok(())
        };
        for (index, unit) in units.iter().enumerate() {
            let body = *unit;
            for instruction in &body.instructions {
                if let Inst::Closure { closure, .. } = *instruction {
                    let closure = &body.tables.closures[closure.index()];
                    let code = closure
                        .code
                        .ok_or("native ownership tracing: closure has no diagnostic code")?;
                    let target = resolve(&body.tables.symbols[code.symbol.index()])?;
                    let captures = closure
                        .captures
                        .get(&body.pools.captures)
                        .ok_or("native ownership tracing: missing diagnostic captures")?;
                    let callee = units[target];
                    let formals = &callee.captures;
                    // C's capture-mask protocol appends one mechanical live
                    // word after the source fields. It carries no callable
                    // identity and must match this exact environment/slot.
                    let persistent_mask = captures.len().checked_add(1) == Some(formals.len())
                        && callee.tables.environment() == Some(closure.environment)
                        && callee
                            .tables
                            .init_flags
                            .iter()
                            .filter(|flag| flag.environment.is_some())
                            .count()
                            == 1
                        && callee.tables.init_flags.iter().any(|flag| {
                            flag.environment == Some(closure.environment)
                                && flag.value == formals[captures.len()]
                                && flag.path.len == 0
                                && callee.slots.get(flag.slot.index()).is_some_and(|slot| {
                                    slot.kind == SlotKind::Capture(captures.len() as u32)
                                })
                        });
                    if captures.len() != formals.len() && !persistent_mask {
                        return Err(
                            "native ownership tracing: diagnostic capture arity differs".into()
                        );
                    }
                    for (capture, &formal) in captures.iter().zip(formals) {
                        merge(target, formal, &codes[index][capture.value.index()])?;
                    }
                }
            }
            for terminator in &body.terminators {
                let Terminator::Call { call, .. } = *terminator else {
                    continue;
                };
                let call = &body.tables.calls[call.index()];
                let target = match call.callee {
                    Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => {
                        resolve(&body.tables.symbols[symbol.index()])?
                    }
                    Callee::Indirect(value) => match &codes[index][value.index()] {
                        Code::Known(target) => resolve(&target.symbol)?,
                        Code::Pending | Code::Dynamic => continue,
                    },
                    Callee::Operation(_) => continue,
                };
                let callee = units[target];
                for argument in call
                    .arguments
                    .get(&body.pools.call_arguments)
                    .ok_or("native ownership tracing: missing diagnostic arguments")?
                {
                    let formal = *callee
                        .parameters
                        .get(argument.parameter as usize)
                        .ok_or("native ownership tracing: diagnostic parameter arity differs")?;
                    let code = match argument.value {
                        ArgumentValue::Value(value) => &codes[index][value.index()],
                        // Mutable writeback needs a separate interprocedural
                        // storage proof. It never inherits a guessed identity.
                        ArgumentValue::Address { .. } => &Code::Dynamic,
                    };
                    merge(target, formal, code)?;
                }
                if let Some(result) = call.result {
                    let result_code = callee
                        .terminators
                        .iter()
                        .filter_map(|term| match *term {
                            Terminator::Return(Some(value)) => Some(&codes[target][value.index()]),
                            _ => None,
                        })
                        .fold(Code::Pending, |joined, code| joined.join(code));
                    merge(index, result, &result_code)?;
                }
            }
        }
        if !changed {
            // An unbound incoming value is unknown, not evidence that another
            // caller's known code applies. Close unresolved roots and return
            // values before accepting any solution that depended on bottom.
            for (unit, seeds) in units.iter().zip(&mut seeds) {
                let body = *unit;
                for value in body
                    .parameters
                    .iter()
                    .copied()
                    .chain(body.captures.iter().copied())
                    .chain(body.tables.calls.iter().filter_map(|call| call.result))
                {
                    if seeds[value.index()] == Code::Pending {
                        seeds[value.index()] = Code::Dynamic;
                        changed = true;
                    }
                }
            }
            if !changed {
                return Ok(codes);
            }
        }
    }
}

#[cfg(test)]
mod tests;
