//! Portable local arena predicates. Extract once from a checked Value; solving
//! a component never needs a Value or a program-local layout table.
//!
//! The predicate and evaluation order are those of arena_credit::Discovery.
//! Component identities follow Unison's recursive component boundary; the two
//! deletion strata are Hirsel's existing assume-then-verify semantics.
use super::*;
use hirsel_lir::stage::ValidatedUnit;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[cfg(test)]
mod tests;

pub type Function = (hirsel_syntax::DefPath, hirsel_syntax::ContentHash);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Equation {
    Constant(bool),
    All(Vec<u32>),
    Call {
        target: Option<Function>,
        fallback: Option<u32>,
        arguments: Vec<(u32, u32)>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Candidate {
    parameter: u32,
    equations: Vec<Equation>,
    requests: Vec<u32>,
    returns_void: bool,
}

/// Private typed archive of root, credit and token facts, with direct-call
/// substitutions. It contains no Value, layout index or producer proof token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalArenaEquations {
    owner: Function,
    candidates: Vec<Candidate>,
    callees: Vec<Function>,
}

impl LocalArenaEquations {
    pub fn owner(&self) -> &Function {
        &self.owner
    }
    pub fn callees(&self) -> &[Function] {
        &self.callees
    }
    pub fn candidates(&self) -> Vec<u32> {
        self.candidates.iter().map(|c| c.parameter).collect()
    }

    /// Refuse malformed archive indices/order before evaluating any predicate.
    pub fn validate(&self) -> Result<(), String> {
        if self
            .candidates
            .windows(2)
            .any(|w| w[0].parameter >= w[1].parameter)
            || self.callees.windows(2).any(|w| w[0] >= w[1])
        {
            return Err("noncanonical arena equations".into());
        }
        for c in &self.candidates {
            let inside = |n: &u32| (*n as usize) < c.equations.len();
            if !c.requests.iter().all(inside) {
                return Err("arena request outside equations".into());
            }
            for e in &c.equations {
                match e {
                    Equation::All(xs) if !xs.iter().all(inside) => {
                        return Err("arena flow outside equations".into());
                    }
                    Equation::Call {
                        target, arguments, ..
                    } => {
                        if !arguments.iter().all(|(_, n)| inside(n))
                            || target
                                .as_ref()
                                .is_some_and(|t| self.callees.binary_search(t).is_err())
                        {
                            return Err("arena call outside equations".into());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// Value acceptance remains the independent native validator's responsibility.
pub fn extract_local_arena(
    value: &ValidatedUnit<Value>,
    layouts: &LayoutTable,
) -> LocalArenaEquations {
    extract(value.unit(), layouts)
}

pub(super) fn extract(u: &LirUnit<Value>, table: &LayoutTable) -> LocalArenaEquations {
    let d = Discovery::new(u, table);
    let abi = &table.fn_abis[u.header.abi.0 as usize];
    let block_result = abi.result.as_ref().is_some_and(|r| {
        table
            .layout(r.layout)
            .is_some_and(|r| matches!(r.kind, LayoutKind::Block { .. } | LayoutKind::Box { .. }))
    });
    let target = |call: &CallSite| match call.callee {
        Callee::Direct(s) => {
            let s = &u.tables.symbols[s.index()];
            s.instance.map(|h| (s.definition.clone(), h))
        }
        _ => None,
    };
    let callees: BTreeSet<_> = u.tables.calls.iter().filter_map(target).collect();
    let mut candidates = Vec::new();
    for (i, s) in u.slots.iter().enumerate() {
        let SlotKind::Parameter(parameter) = s.kind else {
            continue;
        };
        if !block_result
            || !d.stable[i]
            || !table
                .layout(u.types[s.ty.index()].layout)
                .is_some_and(|r| r.kind == LayoutKind::Arena)
            || !abi.parameters.iter().any(|a| {
                a.source == parameter && a.convention == hirsel_core_contract::Passing::Borrow
            })
        {
            continue;
        }
        let slot = SlotId(i as u32);
        let mut equations = Vec::with_capacity(u.values.len() * 3);
        for (v, value) in u.values.iter().enumerate() {
            let ty = &u.types[value.ty.index()];
            for p in 0..3 {
                let e = if p == 1 && ty.repr == ValueRepr::Block {
                    Equation::All(vec![(v * 3) as u32])
                } else if p == 1
                    && table
                        .layout(ty.layout)
                        .is_some_and(|r| r.drop.is_empty() && r.finalizer.is_none())
                {
                    Equation::Constant(true)
                } else if p == 2
                    && (d.reads[v] == Some(slot)
                        || d.parameters
                            .get(&ValueId(v as u32))
                            .is_some_and(|&p| s.kind == SlotKind::Parameter(p)))
                {
                    Equation::Constant(true)
                } else {
                    match &d.source[v] {
                        Source::Flow(xs) if !xs.is_empty() => {
                            Equation::All(xs.iter().map(|x| (x.index() * 3 + p) as u32).collect())
                        }
                        Source::Made(k) => {
                            let site = &u.tables.constructs[k.index()];
                            if p == 0 {
                                Equation::Constant(
                                    ty.repr == ValueRepr::Block && d.arena(site) == Some(slot),
                                )
                            } else if p == 1
                                && ty.repr == ValueRepr::Aggregate
                                && table
                                    .layout(site.layout)
                                    .is_some_and(|r| r.finalizer.is_none())
                            {
                                Equation::All(
                                    site.fields
                                        .get(&u.pools.fields)
                                        .unwrap_or(&[])
                                        .iter()
                                        .map(|f| (f.value.index() * 3 + 1) as u32)
                                        .collect(),
                                )
                            } else {
                                Equation::Constant(false)
                            }
                        }
                        Source::Call(i) if p == 0 => {
                            let call = &u.tables.calls[*i];
                            if matches!(call.callee, Callee::Direct(_)) {
                                let mut seen = BTreeSet::new();
                                Equation::Call {
                                    target: target(call),
                                    fallback: table.fn_abis[call.abi.0 as usize].result_arena,
                                    // The old predicate selects the first argument
                                    // of each parameter, then tests only value args.
                                    arguments: call
                                        .arguments
                                        .get(&u.pools.call_arguments)
                                        .unwrap_or(&[])
                                        .iter()
                                        .filter(|a| seen.insert(a.parameter))
                                        .filter_map(|a| match a.value {
                                            ArgumentValue::Value(v) => {
                                                Some((a.parameter, (v.index() * 3 + 2) as u32))
                                            }
                                            ArgumentValue::Address { .. } => None,
                                        })
                                        .collect(),
                                }
                            } else {
                                Equation::Constant(false)
                            }
                        }
                        _ => Equation::Constant(false),
                    }
                };
                equations.push(e);
            }
        }
        candidates.push(Candidate {
            parameter,
            equations,
            requests: u
                .terminators
                .iter()
                .filter_map(|t| match t {
                    Terminator::Return(Some(v)) => Some((v.index() * 3) as u32),
                    _ => None,
                })
                .collect(),
            returns_void: u
                .terminators
                .iter()
                .any(|t| matches!(t, Terminator::Return(None))),
        });
    }
    candidates.sort_by_key(|c| c.parameter);
    LocalArenaEquations {
        owner: (u.header.definition.clone(), u.header.instance),
        candidates,
        callees: callees.into_iter().collect(),
    }
}

impl Candidate {
    fn proves(&self, claims: &BTreeMap<Function, BTreeSet<u32>>) -> bool {
        struct Query<'a> {
            candidate: &'a Candidate,
            claims: &'a BTreeMap<Function, BTreeSet<u32>>,
            memo: Vec<Option<bool>>,
            active: Vec<bool>,
        }
        impl Query<'_> {
            fn test(&mut self, n: u32) -> bool {
                let i = n as usize;
                if let Some(b) = self.memo[i] {
                    return b;
                }
                if std::mem::replace(&mut self.active[i], true) {
                    return false;
                }
                let answer = match &self.candidate.equations[i] {
                    Equation::Constant(b) => *b,
                    Equation::All(xs) => xs.iter().all(|&n| self.test(n)),
                    Equation::Call {
                        target,
                        fallback,
                        arguments,
                    } => {
                        let params: Vec<_> = target
                            .as_ref()
                            .and_then(|t| self.claims.get(t))
                            .map(|xs| xs.iter().copied().collect())
                            .unwrap_or_else(|| fallback.iter().copied().collect());
                        params.iter().any(|p| {
                            arguments
                                .iter()
                                .find(|(a, _)| a == p)
                                .is_some_and(|(_, n)| self.test(*n))
                        })
                    }
                };
                self.active[i] = false;
                self.memo[i] = Some(answer);
                answer
            }
        }
        let mut q = Query {
            candidate: self,
            claims,
            memo: vec![None; self.equations.len()],
            active: vec![false; self.equations.len()],
        };
        // Populate every requested predicate in terminator order, as Discovery
        // does before examining any result. Active local cycles remain false.
        for &n in &self.requests {
            q.test(n);
        }
        !self.returns_void
            && self
                .requests
                .iter()
                .all(|&n| q.memo[n as usize] == Some(true))
    }
}

/// Solve exactly one stratum. Candidates starts from every borrowed candidate;
/// Chosen starts from the minimum of each member's *own settled Candidates*,
/// then only removes. Missing outgoing rows refuse, never imply empty.
pub fn solve_component(
    members: &[&LocalArenaEquations],
    own_candidates: Option<&[(Function, Vec<u32>)]>,
    outgoing: &[(Function, Vec<u32>)],
) -> Result<Vec<(Function, Vec<u32>)>, String> {
    let canonical = |rows: &[(Function, Vec<u32>)]| {
        rows.windows(2).all(|w| w[0].0 < w[1].0)
            && rows
                .iter()
                .all(|(_, xs)| xs.windows(2).all(|w| w[0] < w[1]))
    };
    if members.windows(2).any(|w| w[0].owner >= w[1].owner) || !canonical(outgoing) {
        return Err("noncanonical arena component".into());
    }
    if let Some(own) = own_candidates {
        if !canonical(own)
            || own.len() != members.len()
            || own.iter().zip(members).any(|((f, xs), m)| {
                f != &m.owner
                    || xs.iter().any(|p| {
                        m.candidates
                            .binary_search_by_key(p, |c| c.parameter)
                            .is_err()
                    })
            })
        {
            return Err("chosen stratum lacks its exact Candidates component".into());
        }
    }
    let mut claims: BTreeMap<_, BTreeSet<_>> = outgoing
        .iter()
        .map(|(f, xs)| (f.clone(), xs.iter().copied().collect()))
        .collect();
    for (i, m) in members.iter().enumerate() {
        m.validate()?;
        let initial = own_candidates.map_or_else(
            || m.candidates(),
            |own| own[i].1.first().copied().into_iter().collect(),
        );
        if claims
            .insert(m.owner.clone(), initial.into_iter().collect())
            .is_some()
        {
            return Err("member also supplied as outgoing arena component".into());
        }
    }
    for m in members {
        if m.callees.iter().any(|f| !claims.contains_key(f)) {
            return Err("missing outgoing arena summary".into());
        }
    }
    let mut callers = vec![Vec::new(); members.len()];
    for (i, m) in members.iter().enumerate() {
        for f in &m.callees {
            if let Ok(callee) = members.binary_search_by(|m| m.owner.cmp(f)) {
                callers[callee].push(i);
            }
        }
    }
    let mut queue: VecDeque<_> = (0..members.len()).collect();
    let mut queued = vec![true; members.len()];
    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        let m = members[i];
        let invalid: Vec<_> = m
            .candidates
            .iter()
            .filter(|c| claims[&m.owner].contains(&c.parameter) && !c.proves(&claims))
            .map(|c| c.parameter)
            .collect();
        if !invalid.is_empty() {
            let own = claims.get_mut(&m.owner).expect("component member");
            for p in invalid {
                own.remove(&p);
            }
            for &caller in &callers[i] {
                if !queued[caller] {
                    queued[caller] = true;
                    queue.push_back(caller);
                }
            }
        }
    }
    Ok(members
        .iter()
        .map(|m| (m.owner.clone(), claims[&m.owner].iter().copied().collect()))
        .collect())
}

/// Condense call topology first, then settle Candidates and Chosen as distinct
/// dependency strata. Each component consumes only its immediate outgoing rows.
/// This orchestration works on compact equations, never a Value collection.
pub fn settle(
    equations: &[LocalArenaEquations],
    external: &[(Function, Vec<u32>)],
) -> Result<BTreeMap<Function, Vec<u32>>, String> {
    let mut sorted: Vec<_> = equations.iter().collect();
    sorted.sort_by(|a, b| a.owner.cmp(&b.owner));
    if sorted.windows(2).any(|w| w[0].owner == w[1].owner) {
        return Err("duplicate arena owner".into());
    }
    let indices: BTreeMap<_, _> = sorted
        .iter()
        .enumerate()
        .map(|(i, m)| (m.owner.clone(), i))
        .collect();
    let graph: Vec<Vec<_>> = sorted
        .iter()
        .map(|m| {
            m.callees
                .iter()
                .filter_map(|f| indices.get(f).copied())
                .collect()
        })
        .collect();
    let components = hirsel_graph::components(&graph);
    let mut candidates: BTreeMap<Function, Vec<u32>> = external.iter().cloned().collect();
    if candidates.len() != external.len()
        || external.windows(2).any(|w| w[0].0 >= w[1].0)
        || external
            .iter()
            .any(|(f, xs)| indices.contains_key(f) || xs.windows(2).any(|w| w[0] >= w[1]))
    {
        return Err("noncanonical external arena summaries".into());
    }
    let mut chosen = candidates.clone();
    for stratum in [false, true] {
        for component in &components {
            let members: Vec<_> = component.iter().map(|&i| sorted[i]).collect();
            let outgoing: BTreeSet<_> = members
                .iter()
                .flat_map(|m| m.callees.iter())
                .filter(|f| !members.iter().any(|m| &m.owner == *f))
                .cloned()
                .collect();
            let own: Vec<_> = if stratum {
                members
                    .iter()
                    .map(|m| (m.owner.clone(), candidates[&m.owner].clone()))
                    .collect()
            } else {
                Vec::new()
            };
            let state = if stratum {
                &mut chosen
            } else {
                &mut candidates
            };
            let outgoing = outgoing
                .into_iter()
                .map(|f| {
                    state
                        .get(&f)
                        .cloned()
                        .map(|xs| (f, xs))
                        .ok_or_else(|| "unsettled outgoing arena component".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let result = solve_component(&members, stratum.then_some(own.as_slice()), &outgoing)?;
            state.extend(result);
        }
    }
    chosen.retain(|f, _| indices.contains_key(f));
    Ok(chosen)
}
