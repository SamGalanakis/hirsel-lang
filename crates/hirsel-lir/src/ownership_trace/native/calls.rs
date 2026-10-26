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
    fn join(&self, other: &Self) -> Self {
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
