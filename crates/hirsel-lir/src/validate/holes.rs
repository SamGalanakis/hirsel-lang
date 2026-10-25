//! Constructor-tail destinations (LANG-10, LIR schema revision 59), checked
//! from the IR alone. A hole is linear: the ownership check already holds it
//! to one consumption on every path. This check adds what credits cannot say.
//!
//! - A hole only moves: into `HoleFill` or `HoleAbandon`, into or out of its
//!   own whole slot, or along an edge. Anything else lets it escape the loop.
//! - A value is *open* when it is a `HoleOpen` block or built from one: a
//!   construction with an open field, a slot version storing one, a read of
//!   such a version, an edge argument joining one. While any hole may be live,
//!   an open value only moves into a construction, a fill or its slot, or
//!   ends in a `HoleAbandon`; every other use (a read of a part, a borrow, a
//!   copy, a comparison, a hash, a call, a return, a drop by its static
//!   layout) observes an unfinished value, and so does addressing storage
//!   that may hold one other than to move it whole. After the last fill no
//!   hole is live, and the value is ordinary.
//!
//! A hole is never live across a suspension point: the continuation planner
//! refuses to save one (`crate::continuation`).
use super::structure::{Def, Facts, Root, Usage, inst_operands, terminator_operands};
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::*;
use crate::stage::Stage;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Carrier {
    /// A hole value.
    Value(ValueId),
    /// A slot holding a hole.
    Slot(SlotId),
    /// A slot whose content may be open.
    Open(SlotId),
    /// A hole was abandoned: what stays open stays so.
    Abandoned,
}

/// Whether a hole may be live.
fn holes(live: &BTreeSet<Carrier>) -> bool {
    live.iter()
        .any(|c| matches!(c, Carrier::Value(_) | Carrier::Slot(_)))
}

/// Whether an open value may still be unfinished.
fn guarded(live: &BTreeSet<Carrier>) -> bool {
    holes(live) || live.contains(&Carrier::Abandoned)
}

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, facts: &Facts, report: &mut Report) {
    let u = cx.unit;
    let insts = |b: usize| u.blocks[b].instructions.get(&u.instructions).unwrap_or(&[]);
    if !(0..u.blocks.len()).any(|b| insts(b).iter().any(|i| matches!(i, Inst::HoleOpen { .. }))) {
        // Without an opening no hole exists; any hole operand is malformed.
        for b in 0..u.blocks.len() {
            for (i, inst) in insts(b).iter().enumerate() {
                if matches!(inst, Inst::HoleFill { .. } | Inst::HoleAbandon { .. }) {
                    report.add(
                        Site::at(BlockId(b as u32), i as u32),
                        K::Type("hole never opened"),
                    );
                }
            }
        }
        return;
    }
    let hole = |v: ValueId| cx.repr(v) == ValueRepr::Hole;
    let whole_slot = |a: AddressId| match facts.addresses[a.index()].as_ref() {
        Some(addr) if addr.path.is_empty() => match addr.root {
            Root::Slot(s) => Some(s),
            Root::Value(_) => None,
        },
        _ => None,
    };
    let edge_args = |e: EdgeId| {
        u.tables.edges[e.index()]
            .arguments
            .get(&u.pools.values)
            .unwrap_or(&[])
    };
    let params = |b: BlockId| {
        u.blocks[b.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
    };

    // Open values: a monotone fixpoint over definitions.
    let mut open = vec![false; u.values.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for v in 0..u.values.len() {
            if open[v] {
                continue;
            }
            let next = match facts.defs[v] {
                Def::Inst(b, i) => match insts(b.index()).get(i as usize) {
                    Some(Inst::HoleOpen { result, .. }) => result.index() == v,
                    Some(Inst::Construct { construct, .. }) => {
                        super::structure::construct_values(cx, *construct).any(|f| open[f.index()])
                    }
                    Some(Inst::Store { value, .. }) => open[value.index()],
                    Some(Inst::Read { reaching, .. }) => open[reaching.index()],
                    _ => false,
                },
                Def::Param(b, ordinal) => facts.cfg.preds[b.index()].iter().any(|(_, e)| {
                    edge_args(*e)
                        .get(ordinal as usize)
                        .is_some_and(|a| open[a.index()])
                }),
                _ => false,
            };
            if next {
                open[v] = true;
                changed = true;
            }
        }
    }

    // Live hole carriers at each block entry: a forward may-analysis.
    let n = u.blocks.len();
    let mut entry: Vec<Option<BTreeSet<Carrier>>> = vec![None; n];
    entry[facts.cfg.entry.index()] = Some(BTreeSet::new());
    let mut work: Vec<BlockId> = vec![facts.cfg.entry];
    let mut errors: BTreeSet<(Site, &'static str)> = BTreeSet::new();
    while let Some(b) = work.pop() {
        let Some(mut live) = entry[b.index()].clone() else {
            continue;
        };
        // A failing instruction's edge leaves with the state before it.
        let mut before: Vec<BTreeSet<Carrier>> = Vec::new();
        for (i, inst) in insts(b.index()).iter().enumerate() {
            let site = Site::at(b, i as u32);
            before.push(live.clone());
            observe(
                cx,
                facts,
                inst,
                &live,
                &open,
                site,
                &mut errors,
                &whole_slot,
                &hole,
            );
            match *inst {
                Inst::HoleOpen { hole, .. } => {
                    live.insert(Carrier::Value(hole));
                }
                Inst::HoleFill { hole, .. } => {
                    live.remove(&Carrier::Value(hole));
                }
                Inst::HoleAbandon { hole, .. } => {
                    live.remove(&Carrier::Value(hole));
                    live.insert(Carrier::Abandoned);
                }
                Inst::Store { address, value, .. } if live.contains(&Carrier::Value(value)) => {
                    live.remove(&Carrier::Value(value));
                    if let Some(s) = whole_slot(address) {
                        live.insert(Carrier::Slot(s));
                    }
                }
                Inst::Store { address, value, .. } => {
                    if let Some(s) = whole_slot(address) {
                        if open[value.index()] {
                            live.insert(Carrier::Open(s));
                        } else {
                            live.remove(&Carrier::Open(s));
                        }
                    }
                }
                Inst::Read {
                    result,
                    address,
                    mode: ReadMode::Take,
                    ..
                } => {
                    if let Some(s) = whole_slot(address) {
                        live.remove(&Carrier::Slot(s));
                        live.remove(&Carrier::Open(s));
                    }
                    if hole(result) {
                        live.insert(Carrier::Value(result));
                    }
                }
                Inst::StorageDead { slot, .. } => {
                    live.remove(&Carrier::Slot(slot));
                    live.remove(&Carrier::Open(slot));
                }
                _ => {}
            }
        }
        let term = &u.terminators[b.index()];
        let site = Site::at(b, insts(b.index()).len() as u32);
        for (v, usage) in terminator_operands(cx, term) {
            if usage == Usage::Argument {
                continue;
            }
            if hole(v) {
                errors.insert((site, "hole escapes its loop"));
            } else if open[v.index()] && guarded(&live) {
                errors.insert((site, "open constructor-tail value observed"));
            }
        }
        if holes(&live) && matches!(term, Terminator::Return(_) | Terminator::Propagate) {
            errors.insert((site, "hole live at exit"));
        }
        for out in &facts.cfg.succ[b.index()] {
            let mut state = match out.origin {
                super::flow::Origin::Instruction(i) => before[i as usize].clone(),
                super::flow::Origin::Terminator => live.clone(),
            };
            // Edge arguments rename carried holes to the target's parameters.
            for (arg, param) in edge_args(out.edge).iter().zip(params(out.target)) {
                if state.remove(&Carrier::Value(*arg)) {
                    state.insert(Carrier::Value(*param));
                }
            }
            let slot = &mut entry[out.target.index()];
            let grown = match slot {
                Some(old) => {
                    let len = old.len();
                    old.extend(state);
                    old.len() != len
                }
                None => {
                    *slot = Some(state);
                    true
                }
            };
            if grown {
                work.push(out.target);
            }
        }
    }
    for (site, what) in errors {
        report.add(site, K::Type(what));
    }
}

#[allow(clippy::too_many_arguments)]
fn observe<S>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    inst: &Inst,
    live: &BTreeSet<Carrier>,
    open: &[bool],
    site: Site,
    errors: &mut BTreeSet<(Site, &'static str)>,
    whole_slot: &impl Fn(AddressId) -> Option<SlotId>,
    hole: &impl Fn(ValueId) -> bool,
) {
    // The moves a hole or an open value may make.
    let allowed = |v: ValueId| match *inst {
        Inst::HoleFill { hole: h, value } => v == h || v == value,
        Inst::HoleAbandon { hole: h, root } => v == h || root == Some(v),
        Inst::Construct { .. } => !hole(v),
        Inst::Store { address, value, .. } => v == value && whole_slot(address).is_some(),
        Inst::Read {
            address,
            mode: ReadMode::Take,
            ..
        } => whole_slot(address).is_some(),
        _ => false,
    };
    for (v, _) in inst_operands(cx, inst) {
        if hole(v) && !allowed(v) {
            errors.insert((site, "hole escapes its loop"));
        } else if open[v.index()] && guarded(live) && !allowed(v) {
            errors.insert((site, "open constructor-tail value observed"));
        }
    }
    if !guarded(live) {
        return;
    }
    // Storage holding open content, or a place inside an open block, is
    // addressed only to move it whole.
    let moves = |a: AddressId| match *inst {
        Inst::Store { address, .. }
        | Inst::Read {
            address,
            mode: ReadMode::Take,
            ..
        } => address == a && whole_slot(a).is_some(),
        // Origin markers name storage without reading it.
        Inst::BindOrigin { .. } | Inst::BindFields { .. } | Inst::EndOrigin { .. } => true,
        _ => false,
    };
    let mut observed = false;
    inst.visit(&mut |r| {
        if let Ref::Address(a) = r
            && !moves(a)
            && let Some(addr) = facts.addresses[a.index()].as_ref()
        {
            observed |= match addr.root {
                Root::Slot(s) => live.contains(&Carrier::Open(s)),
                Root::Value(v) => open[v.index()],
            };
        }
    });
    if observed {
        errors.insert((site, "open constructor-tail value observed"));
    }
}
