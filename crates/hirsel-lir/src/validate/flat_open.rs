//! Independent replay of builders' open elements (LIR schema revision 24).
//! A version of a slot is open at a field when the `FlatField` opening that
//! field defines it, when a reservation or push of the field defines it
//! from a version open at it, or when it joins versions all open at that
//! field. A reservation, a push and a `FlatClose` need a version open at
//! their field, and an opening one open at none. Any other write defines a version that is not open, so a
//! close completes only bytes its own element's writes put in place: a
//! write between them that could have moved or overwritten those bytes
//! leaves the close without its open element. Open writes reach only
//! storage past the used bytes, so they never change the builder's value.
//! No producer analysis is consumed.
use super::structure::{Def, Facts};
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::*;
use crate::stage::Stage;
use std::collections::BTreeMap;

/// What a version says about its slot's open element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Open {
    /// No definition seen yet: the solver's bottom.
    Unknown,
    /// No open element.
    Closed,
    /// The element open at this field.
    At(u32),
    /// Open on some paths, or at different fields.
    Mixed,
}

impl Open {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Open::Unknown, x) | (x, Open::Unknown) => x,
            (a, b) if a == b => a,
            _ => Open::Mixed,
        }
    }
}

/// `reaching` holds the slot version reaching each `FlatField` and
/// `FlatClose` site, when exactly one does.
pub(super) fn check<S: Stage>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    reaching: &BTreeMap<Site, Option<ValueId>>,
    report: &mut Report,
) {
    if reaching.is_empty() {
        return;
    }
    let u = cx.unit;
    let inst = |b: BlockId, i: u32| {
        u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .and_then(|insts| insts.get(i as usize))
            .copied()
    };
    // Versions only grow towards Mixed, so this reaches a fixpoint.
    let mut state = vec![Open::Unknown; u.values.len()];
    let at = |state: &[Open], site: Site| match reaching.get(&site).copied().flatten() {
        Some(v) => state[v.index()],
        None => Open::Mixed,
    };
    let mut changed = true;
    while changed {
        changed = false;
        for v in 0..u.values.len() {
            if facts.version[v].is_none() {
                continue;
            }
            let next = match facts.defs[v] {
                Def::Inst(b, i) => match inst(b, i) {
                    Some(Inst::FlatField {
                        field,
                        op: FlatFieldOp::Open,
                        ..
                    }) => Open::At(field),
                    Some(Inst::FlatField { field, .. }) => match at(&state, Site::at(b, i)) {
                        Open::At(f) if f == field => Open::At(field),
                        Open::Unknown => Open::Unknown,
                        _ => Open::Mixed,
                    },
                    _ => Open::Closed,
                },
                Def::Param(b, ordinal) => facts.cfg.preds[b.index()]
                    .iter()
                    .filter(|(p, _)| facts.cfg.reachable(*p))
                    .map(|(_, e)| {
                        u.tables.edges[e.index()]
                            .arguments
                            .get(&u.pools.values)
                            .and_then(|a| a.get(ordinal as usize))
                            .map_or(Open::Mixed, |a| state[a.index()])
                    })
                    .fold(Open::Unknown, Open::join),
                _ => Open::Closed,
            };
            if next != state[v] {
                state[v] = next;
                changed = true;
            }
        }
    }
    for (&site, _) in reaching {
        let (Some(b), Some(i)) = (site.block, site.index) else {
            continue;
        };
        let open = at(&state, site);
        let ok = match inst(b, i) {
            Some(Inst::FlatField {
                op: FlatFieldOp::Open,
                ..
            }) => matches!(open, Open::Closed | Open::Unknown),
            Some(Inst::FlatField { field, .. } | Inst::FlatClose { field, .. }) => {
                open == Open::At(field)
            }
            _ => true,
        };
        if !ok && matches!(inst(b, i), Some(Inst::FlatField { op: FlatFieldOp::Open, .. })) {
            report.add(site, K::Type("flat field opened twice"));
        } else if !ok {
            report.add(site, K::Type("flat field of no open element"));
        }
    }
}
