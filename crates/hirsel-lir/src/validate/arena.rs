//! Independent result/child placement proof over the SSA graph. Each value
//! widens at most three times; no per-arena dense flow state is retained.
use super::{
    Cx, Report, Site, ViolationKind as K,
    structure::{Facts, Root},
};
use crate::{
    ir::*,
    layout_types::CopyRecipe,
    stage::{Stage, StageKind},
};
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Domain {
    Pending,
    /// No outgoing cleanup (including a fieldless variant).
    Empty,
    Arena(SlotId),
    Unknown,
}
impl Domain {
    fn meet(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Pending, x) | (x, Self::Pending) | (Self::Empty, x) | (x, Self::Empty) => x,
            (a, b) if a == b => a,
            _ => Self::Unknown,
        }
    }
    fn in_arena(self, slot: SlotId) -> bool {
        self == Self::Empty || self == Self::Arena(slot)
    }
}

fn arena_slot<S>(cx: &Cx<'_, S>, site: &ConstructSite) -> Option<SlotId> {
    if cx.stage == StageKind::Value {
        if let DestinationClass::Arena(slot) = site.destination {
            return Some(slot);
        }
    } else if let Some(Placement::Fixed(StorageOwner::Arena(a))) = site.placement {
        return cx.unit.tables.arenas.get(a.index()).map(|a| a.slot);
    }
    None
}

pub(super) fn check<S: Stage>(cx: &Cx<'_, S>, facts: &Facts, report: &mut Report) {
    let u = cx.unit;
    let result_arena = cx.layouts.0.fn_abis[u.header.abi.0 as usize].result_arena;
    if result_arena.is_none() && !u.tables.constructs.iter().any(|site| site.arena_no_drop) {
        return;
    }
    let mut mutable = BTreeSet::new();
    let root = |a: AddressId| {
        facts.addresses[a.index()]
            .as_ref()
            .and_then(|a| match a.root {
                Root::Slot(s) => Some(s),
                _ => None,
            })
    };
    for loan in &u.tables.loans {
        if loan.mutable
            && let Some(s) = root(loan.address)
        {
            mutable.insert(s);
        }
    }
    for arg in &u.pools.call_arguments {
        if let ArgumentValue::Address { address, .. } = arg.value
            && let Some(s) = root(address)
        {
            mutable.insert(s);
        }
    }
    let whole = |a: AddressId| {
        facts.addresses[a.index()]
            .as_ref()
            .and_then(|a| match a.root {
                Root::Slot(s)
                    if a.path.is_empty() && a.crossings.is_empty() && !mutable.contains(&s) =>
                {
                    Some(s)
                }
                _ => None,
            })
    };
    let mut writes: Vec<_> = u
        .slots
        .iter()
        .map(|s| usize::from(matches!(s.kind, SlotKind::Parameter(_))))
        .collect();
    for inst in &u.instructions {
        if let Inst::Store { address, .. } | Inst::Replace { address, .. } = *inst
            && let Some(s) = root(address)
        {
            writes[s.index()] += 1;
        }
    }
    // A syntactically single store can run repeatedly on loop feedback.
    // Only an initialization in a non-reentered entry block is once per frame.
    let mut entry_initialized = BTreeSet::new();
    if facts.cfg.preds[u.entry.index()].is_empty() {
        for inst in u.blocks[u.entry.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
            if let Inst::Store { address, .. } = *inst
                && let Some(slot) = whole(address)
            {
                entry_initialized.insert(slot);
            }
        }
    }
    let stable = |s: SlotId| {
        writes[s.index()] == 1
            && !mutable.contains(&s)
            && (matches!(u.slots[s.index()].kind, SlotKind::Parameter(_))
                || entry_initialized.contains(&s))
    };
    let mut input: Vec<Option<Vec<ValueId>>> = vec![None; u.values.len()];
    let mut constant = vec![Domain::Unknown; u.values.len()];
    for edge in &u.tables.edges {
        for (&p, &a) in u.blocks[edge.target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            input[p.index()].get_or_insert_with(Vec::new).push(a);
        }
    }
    for inst in &u.instructions {
        match *inst {
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
            } if whole(address).is_some() => {
                let ty = &u.types[u.values[result.index()].ty.index()];
                if ty.repr == ValueRepr::Arena {
                    let slot = whole(address).unwrap();
                    if stable(slot) {
                        constant[result.index()] = Domain::Arena(slot);
                    }
                } else if cx
                    .layouts
                    .get(ty.layout)
                    .is_some_and(|r| r.copy != CopyRecipe::FlatBytes)
                {
                    input[result.index()] = Some(vec![reaching]);
                }
            }
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
            } if whole(address).is_some() => input[version.index()] = Some(vec![value]),
            Inst::Recast { result, value } => input[result.index()] = Some(vec![value]),
            Inst::Construct {
                result, construct, ..
            } => {
                let site = &u.tables.constructs[construct.index()];
                let ty = &u.types[u.values[result.index()].ty.index()];
                if ty.repr == ValueRepr::Block {
                    if let Some(s) = arena_slot(cx, site).filter(|&s| stable(s)) {
                        constant[result.index()] = Domain::Arena(s);
                    }
                } else if ty.repr == ValueRepr::Aggregate
                    && cx
                        .layouts
                        .get(site.layout)
                        .is_some_and(|r| r.finalizer.is_none())
                {
                    input[result.index()] = Some(
                        site.fields
                            .get(&u.pools.fields)
                            .unwrap_or(&[])
                            .iter()
                            .map(|f| f.value)
                            .collect(),
                    );
                }
            }
            _ => {}
        }
    }
    for (i, slot) in u.slots.iter().enumerate() {
        if let SlotKind::Parameter(p) = slot.kind
            && stable(SlotId(i as u32))
            && let Some(&v) = u.parameters.get(p as usize)
            && u.types[u.values[v.index()].ty.index()].repr == ValueRepr::Arena
        {
            constant[v.index()] = Domain::Arena(SlotId(i as u32));
        }
    }
    for call in &u.tables.calls {
        if matches!(call.callee, Callee::Direct(_))
            && let (Some(v), Some(p)) = (
                call.result,
                cx.layouts.0.fn_abis[call.abi.0 as usize].result_arena,
            )
            && let Some(arg) = call
                .arguments
                .get(&u.pools.call_arguments)
                .unwrap_or(&[])
                .iter()
                .find(|a| a.parameter == p)
            && let ArgumentValue::Value(value) = arg.value
        {
            input[v.index()] = Some(vec![value]);
        }
    }
    for (i, v) in u.values.iter().enumerate() {
        let ty = &u.types[v.ty.index()];
        if !matches!(ty.repr, ValueRepr::Block | ValueRepr::Arena)
            && cx
                .layouts
                .get(ty.layout)
                .is_some_and(|r| r.drop.is_empty() && r.finalizer.is_none())
        {
            input[i] = None;
            constant[i] = Domain::Empty;
        }
    }
    let mut users = vec![Vec::new(); u.values.len()];
    let mut domain = constant.clone();
    for (i, deps) in input.iter().enumerate() {
        if let Some(deps) = deps {
            domain[i] = Domain::Pending;
            for v in deps {
                users[v.index()].push(i);
            }
        }
    }
    let mut queue: VecDeque<_> = (0..u.values.len()).collect();
    let mut queued = vec![true; u.values.len()];
    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        let next = input[i].as_ref().map_or(constant[i], |xs| {
            if xs.is_empty() {
                Domain::Empty
            } else {
                xs.iter()
                    .fold(Domain::Pending, |d, v| d.meet(domain[v.index()]))
            }
        });
        if domain[i] != next {
            domain[i] = next;
            for &user in &users[i] {
                if !queued[user] {
                    queued[user] = true;
                    queue.push_back(user);
                }
            }
        }
    }
    if let Some(p) = result_arena {
        let slot = u
            .slots
            .iter()
            .position(|s| s.kind == SlotKind::Parameter(p))
            .map(|i| SlotId(i as u32));
        let valid = slot.filter(|&s| stable(s)).is_some_and(|s| {
            u.terminators.iter().all(|t| match t {
                Terminator::Return(Some(v)) => domain[v.index()] == Domain::Arena(s),
                Terminator::Return(None) => false,
                _ => true,
            })
        });
        if !valid {
            report.add(Site::UNIT, K::Placement("unproved result arena"));
        }
    }
    let mut ordinary = vec![true; u.tables.constructs.len()];
    for inst in &u.instructions {
        match *inst {
            Inst::Construct {
                result, construct, ..
            } => {
                ordinary[construct.index()] &=
                    u.types[u.values[result.index()].ty.index()].repr == ValueRepr::Block
            }
            Inst::Reuse { construct, .. } | Inst::UniqueCopy { construct, .. } => {
                ordinary[construct.index()] = false
            }
            _ => {}
        }
    }
    for (i, site) in u.tables.constructs.iter().enumerate().filter(|(_, s)| {
        s.arena_no_drop
            && !matches!(
                s.placement,
                Some(Placement::Fixed(StorageOwner::LocalRegion(_)))
            )
    }) {
        let valid = cx.stage >= StageKind::Placed
            && ordinary[i]
            && arena_slot(cx, site)
                .filter(|&s| stable(s))
                .is_some_and(|s| {
                    cx.layouts
                        .get(site.layout)
                        .is_some_and(|r| r.finalizer.is_none())
                        && site
                            .fields
                            .get(&u.pools.fields)
                            .unwrap_or(&[])
                            .iter()
                            .all(|f| domain[f.value.index()].in_arena(s))
                });
        if !valid {
            report.add(
                Site::UNIT,
                K::Placement("unproved arena child-credit elision"),
            );
        }
    }
}
/// Link the body proof to every direct-call claim. Run over the complete link
/// set before using any summaries; per-unit C checks then verify all returns
/// under these mutually consistent assumptions, including recursive groups.
pub fn check_calls<S>(
    units: &[LirUnit<S>],
    layouts: &crate::layout_types::LayoutTable,
) -> Result<(), String> {
    let targets: std::collections::BTreeMap<_, _> = units
        .iter()
        .map(|u| ((&u.header.definition, u.header.instance), u))
        .collect();
    for u in units {
        for call in &u.tables.calls {
            let abi = layouts
                .fn_abis
                .get(call.abi.0 as usize)
                .ok_or("call ABI outside the layout table")?;
            if abi.result_arena.is_some() {
                let Callee::Direct(symbol) = call.callee else {
                    return Err("an indirect call claims a result arena".into());
                };
                let symbol = u
                    .tables
                    .symbols
                    .get(symbol.index())
                    .ok_or("call symbol outside the symbol table")?;
                let callee = symbol
                    .instance
                    .and_then(|i| targets.get(&(&symbol.definition, i)))
                    .ok_or_else(|| {
                        format!("result-arena callee {} is absent", symbol.definition.0.name)
                    })?;
                if layouts.fn_abis.get(callee.header.abi.0 as usize) != Some(abi) {
                    return Err(format!(
                        "call to {} forges its result arena",
                        symbol.definition.0.name
                    ));
                }
            }
        }
    }
    Ok(())
}
