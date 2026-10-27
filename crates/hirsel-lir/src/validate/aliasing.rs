//! OWN-3 at call sites: a callee reaches a `mut` argument's place only through
//! that argument (`CallAlias`). It is the evidence the native backends' `noalias`
//! on `mut` place addresses rests on (`docs/spec/implementation.md`, Aliasing facts).
//!
//! A callee reaches caller storage through its arguments and its callee value. A
//! `mut` argument names a slot place; every other argument, and the callee, is a
//! value that reaches the places its loans view: its own loans (`borrowed_holders`,
//! views carried through records with borrowed fields included) and, for a closure,
//! the loans of its borrowed captures, wherever the closure value has flowed. A
//! loan through a value root views the place that value reaches, with the loan's
//! path below it. No two `mut` arguments may overlap, and no value the callee
//! receives may hold a loan on a place overlapping a `mut` argument. This is a
//! property of what the call hands over, not of loan activity: a loan the caller
//! still has open but passes nowhere is not the callee's.
use super::structure::{Facts, Root, Step, construct_values, overlap, propagate};
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::{AddressId, ArgumentValue, BlockId, CaptureUse, Inst, LoanId, Terminator, ValueId};
use crate::stage::Stage;
use hirsel_core_contract::Passing;
use std::collections::BTreeSet;

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, facts: &Facts, report: &mut Report) {
    let unit = cx.unit;
    if unit.tables.calls.is_empty() {
        return;
    }
    let captured = captured_loans(cx, facts);
    for (b, terminator) in unit.terminators.iter().enumerate() {
        let Terminator::Call { call, .. } = *terminator else {
            continue;
        };
        let site = &unit.tables.calls[call.index()];
        let arguments = site
            .arguments
            .get(&unit.pools.call_arguments)
            .unwrap_or(&[]);
        let mut places: Vec<(AddressId, Root, Vec<Step>)> = Vec::new();
        let mut reached = Vec::new();
        for argument in arguments {
            match (argument.passing, argument.value) {
                (Passing::Mut, ArgumentValue::Address { address, .. }) => {
                    if let Some(a) = &facts.addresses[address.index()] {
                        places.push((address, a.root, a.path.clone()));
                    }
                }
                (_, ArgumentValue::Value(value)) => reached.push(value),
                _ => {}
            }
        }
        if let Some(receipts) = &site.receipts {
            reached.extend(receipts.inputs());
            for (address, _) in receipts.writebacks() {
                if let Some(a) = &facts.addresses[address.index()] {
                    places.push((address, a.root, a.path.clone()));
                }
            }
        }
        if places.is_empty() {
            continue;
        }
        reached.extend(site.callee.value());
        let viewed = viewed_places(cx, facts, &captured, &reached);
        let at = Site::block(BlockId(b as u32));
        for (i, (address, root, path)) in places.iter().enumerate() {
            let twice = places[..i]
                .iter()
                .any(|(_, r, p)| r == root && overlap(p, path));
            let viewed = viewed.iter().any(|(r, p)| r == root && overlap(p, path));
            if twice || viewed {
                report.add(at, K::CallAlias(*address));
            }
        }
    }
}

/// Per value, the loans of the borrowed captures of every closure it may be: seeded
/// at each `Closure`, then carried wherever a value can be that closure (a read or
/// copy of the slot holding it, a store, a replace, a recast, a weakening, a copy,
/// a record or variant field, a block parameter) until nothing changes.
fn captured_loans<S: Stage>(cx: &Cx<'_, S>, facts: &Facts) -> Vec<BTreeSet<LoanId>> {
    let unit = cx.unit;
    let mut captured = vec![BTreeSet::new(); unit.values.len()];
    if unit.tables.closures.is_empty() {
        return captured;
    }
    // (from, to): `to` may be the value `from` is.
    let mut flows: Vec<(ValueId, ValueId)> = Vec::new();
    for block in &unit.blocks {
        for inst in block.instructions.get(&unit.instructions).unwrap_or(&[]) {
            match *inst {
                Inst::Closure {
                    result, closure, ..
                } => {
                    let site = &unit.tables.closures[closure.index()];
                    for c in site.captures.get(&unit.pools.captures).unwrap_or(&[]) {
                        if c.usage == CaptureUse::Borrow || c.usage == CaptureUse::Mut {
                            let loans = facts.borrowed_holders[c.value.index()].clone();
                            captured[result.index()].extend(loans);
                        }
                        flows.push((c.value, result));
                    }
                }
                Inst::Read {
                    result, reaching, ..
                }
                | Inst::Copy {
                    result, reaching, ..
                } => flows.push((reaching, result)),
                Inst::Store { value, version, .. } | Inst::Replace { value, version, .. } => {
                    flows.push((value, version))
                }
                Inst::Recast { result, value }
                | Inst::Weaken { result, value }
                | Inst::UniqueCopy { result, value, .. } => flows.push((value, result)),
                Inst::Construct {
                    result, construct, ..
                }
                | Inst::Reuse {
                    result, construct, ..
                } => flows.extend(construct_values(cx, construct).map(|v| (v, result))),
                _ => {}
            }
        }
    }
    loop {
        let mut changed = false;
        for &(from, to) in &flows {
            if from == to || captured[from.index()].is_empty() {
                continue;
            }
            let add: Vec<_> = captured[from.index()]
                .difference(&captured[to.index()])
                .copied()
                .collect();
            if !add.is_empty() {
                captured[to.index()].extend(add);
                changed = true;
            }
        }
        let before = captured.clone();
        propagate(cx, &facts.cfg, &mut captured, |a, b| {
            a.union(b).copied().collect()
        });
        changed |= captured != before;
        if !changed {
            return captured;
        }
    }
}

/// The slot places the values a callee receives may view: each loan's place, a
/// loan through a value root standing for what that value views with the loan's
/// path below it.
fn viewed_places<S: Stage>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    captured: &[BTreeSet<LoanId>],
    values: &[ValueId],
) -> Vec<(Root, Vec<Step>)> {
    let loans_of = |v: ValueId| -> Vec<LoanId> {
        facts.borrowed_holders[v.index()]
            .iter()
            .chain(&captured[v.index()])
            .copied()
            .collect()
    };
    let mut out = Vec::new();
    // (value, path below what it views): a value root's view prefixes its loan's path.
    let mut pending: Vec<(ValueId, Vec<Step>)> = values.iter().map(|&v| (v, Vec::new())).collect();
    let mut seen = BTreeSet::new();
    while let Some((value, below)) = pending.pop() {
        if !seen.insert((value, below.clone())) {
            continue;
        }
        for loan in loans_of(value) {
            let address = cx.unit.tables.loans[loan.index()].address;
            let Some(a) = facts.addresses[address.index()].as_ref() else {
                continue;
            };
            let mut path = a.path.clone();
            path.extend(below.iter().copied());
            match a.root {
                Root::Slot(_) => out.push((a.root, path)),
                Root::Value(base) => pending.push((base, path)),
            }
        }
    }
    out
}
