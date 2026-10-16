//! Independent replay of FlatAt offsets. Only zero at index zero, FlatNext
//! of the preceding element of the same reaching content, and correlated
//! block-parameter joins are evidence. No producer analysis is consumed.
use super::structure::{Def, Facts};
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::*;
use crate::stage::Stage;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Sequence {
    base: AddressBase,
    prefix: Vec<ProjectionStep>,
    version: Option<ValueId>,
}

pub(super) fn check<S: Stage>(
    cx: &Cx<'_, S>,
    facts: &Facts,
    reaching: &BTreeMap<Site, Option<ValueId>>,
    report: &mut Report,
) {
    let replay = Replay {
        cx,
        facts,
        reaching,
    };
    for (b, block) in cx.unit.blocks.iter().enumerate() {
        for (i, inst) in block
            .instructions
            .get(&cx.unit.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let Inst::Address { address, .. } = *inst else {
                continue;
            };
            let site = Site::at(BlockId(b as u32), i as u32);
            let Some((base, path)) = replay.path(address) else {
                continue;
            };
            for (p, s) in path.iter().enumerate() {
                if let ProjectionStep::FlatAt { index, offset } = *s {
                    let sequence = Sequence {
                        base,
                        prefix: path[..p].to_vec(),
                        version: reaching.get(&site).copied().flatten(),
                    };
                    if (matches!(base, AddressBase::Slot(_)) && sequence.version.is_none())
                        || !replay.prove(index, offset, &sequence)
                    {
                        report.add(site, K::Type("flat walk offset replay"));
                    }
                }
            }
        }
    }
}
struct Replay<'a, 'b, S> {
    cx: &'a Cx<'b, S>,
    facts: &'a Facts,
    reaching: &'a BTreeMap<Site, Option<ValueId>>,
}
impl<S: Stage> Replay<'_, '_, S> {
    fn inst(&self, v: ValueId) -> Option<(Site, Inst)> {
        let Def::Inst(b, i) = self.facts.defs[v.index()] else {
            return None;
        };
        Some((
            Site::at(b, i),
            *self.cx.unit.blocks[b.index()]
                .instructions
                .get(&self.cx.unit.instructions)?
                .get(i as usize)?,
        ))
    }
    fn path(&self, mut a: AddressId) -> Option<(AddressBase, Vec<ProjectionStep>)> {
        let u = self.cx.unit;
        let mut parts = vec![];
        let base = loop {
            let info = &u.addresses[a.index()];
            parts.push(info.projection.get(&u.pools.projections)?.to_vec());
            match info.base {
                AddressBase::Address(p) => a = p,
                b => break b,
            }
        };
        Some((base, parts.into_iter().rev().flatten().collect()))
    }
    fn source(&self, mut v: ValueId) -> ValueId {
        let mut seen = HashSet::new();
        while seen.insert(v) {
            if let Def::Param(b, ordinal) = self.facts.defs[v.index()] {
                let args: Vec<_> = self.facts.cfg.preds[b.index()]
                    .iter()
                    .filter(|(p, _)| self.facts.cfg.reachable(*p))
                    .filter_map(|(_, e)| {
                        self.cx.unit.tables.edges[e.index()]
                            .arguments
                            .get(&self.cx.unit.pools.values)?
                            .get(ordinal as usize)
                            .copied()
                    })
                    .collect();
                if let Some(&a) = args.first()
                    && args.iter().all(|&x| x == a)
                {
                    v = a;
                    continue;
                }
            }
            match self.inst(v).map(|x| x.1) {
                Some(Inst::Read {
                    address, reaching, ..
                }) if self.path(address).is_some_and(|(_, p)| p.is_empty()) => v = reaching,
                Some(Inst::Store { value, version, .. } | Inst::Replace { value, version, .. })
                    if version == v =>
                {
                    v = value
                }
                _ => break,
            }
        }
        v
    }
    fn constant(&self, v: ValueId) -> Option<u128> {
        let (_, Inst::Const { constant, .. }) = self.inst(self.source(v))? else {
            return None;
        };
        match self.cx.unit.tables.constants[constant.index()].value {
            ConstantValue::Integer(n) => Some(n),
            _ => None,
        }
    }
    fn previous(&self, i: ValueId, previous: ValueId) -> bool {
        if let (Some(i), Some(p)) = (self.constant(i), self.constant(previous)) {
            return p.checked_add(1) == Some(i);
        }
        let Some((
            _,
            Inst::Binary {
                op: BinaryOp::Add,
                left,
                right,
                ..
            },
        )) = self.inst(i)
        else {
            return false;
        };
        (self.source(left) == self.source(previous) && self.constant(right) == Some(1))
            || (self.source(right) == self.source(previous) && self.constant(left) == Some(1))
    }
    fn prove(&self, i: ValueId, o: ValueId, sequence: &Sequence) -> bool {
        let mut work = vec![(i, o, sequence.clone())];
        let mut seen = HashSet::new();
        let mut grounded = false;
        while let Some((i, o, sequence)) = work.pop() {
            let (i, o) = (self.source(i), self.source(o));
            if !seen.insert((i, o, sequence.clone())) {
                continue;
            }
            if self.constant(i) == Some(0) && self.constant(o) == Some(0) {
                grounded = true;
                continue;
            }
            if let Some((site, Inst::FlatNext { address, .. })) = self.inst(o) {
                let Some((base, path)) = self.path(address) else {
                    return false;
                };
                let Some((last, prefix)) = path.split_last() else {
                    return false;
                };
                let previous = match *last {
                    ProjectionStep::Index(v) | ProjectionStep::FlatAt { index: v, .. } => v,
                    _ => return false,
                };
                if base != sequence.base
                    || prefix != sequence.prefix
                    // Joins can rename unchanged content without changing the
                    // cursor. Only identical incoming versions are aliases;
                    // differing writes remain distinct and fail this replay.
                    || self.reaching.get(&site).copied().flatten().map(|v| self.source(v))
                        != sequence.version.map(|v| self.source(v))
                    || !self.previous(i, previous)
                {
                    return false;
                }
                // A Next from an ordinary Index is independently checked at its
                // address. A Next from FlatAt also needs that prior offset proved.
                if let ProjectionStep::FlatAt { offset, .. } = *last {
                    work.push((previous, offset, sequence.clone()));
                } else {
                    grounded = true;
                }
                continue;
            }
            let (Def::Param(b, ip), Def::Param(ob, op)) =
                (self.facts.defs[i.index()], self.facts.defs[o.index()])
            else {
                return false;
            };
            if b != ob {
                return false;
            }
            let mut incoming = false;
            for &(pred, e) in &self.facts.cfg.preds[b.index()] {
                if !self.facts.cfg.reachable(pred) {
                    continue;
                }
                let Some(args) = self.cx.unit.tables.edges[e.index()]
                    .arguments
                    .get(&self.cx.unit.pools.values)
                else {
                    return false;
                };
                // Transport the sequence as well as the cursor across the
                // edge. Reusing a header SSA name after a write is not evidence
                // that this iteration contains the previous iteration's bytes.
                let substitute = |v: ValueId| match self.facts.defs[v.index()] {
                    Def::Param(owner, ordinal) if owner == b => args[ordinal as usize],
                    _ => v,
                };
                let mut incoming_sequence = sequence.clone();
                incoming_sequence.version = incoming_sequence.version.map(substitute);
                if let AddressBase::Value(v) = incoming_sequence.base {
                    incoming_sequence.base = AddressBase::Value(substitute(v));
                }
                for step in &mut incoming_sequence.prefix {
                    match step {
                        ProjectionStep::Index(v) | ProjectionStep::Slot(v) => *v = substitute(*v),
                        ProjectionStep::FlatAt { index, offset } => {
                            *index = substitute(*index);
                            *offset = substitute(*offset);
                        }
                        _ => {}
                    }
                }
                work.push((args[ip as usize], args[op as usize], incoming_sequence));
                incoming = true;
            }
            if !incoming {
                return false;
            }
        }
        grounded
    }
}
