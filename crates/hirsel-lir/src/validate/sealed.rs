//! Sealed transitions are checked against a separately supplied source event
//! inventory. Matching only events present in the candidate would let deleting
//! an allocation and its journal entry silently change MEM-6.
use super::{Cx, Report, Site, ViolationKind as K};
use crate::ir::*;
use crate::sealed::{SourceEvent, SourceEventKind};
use crate::stage::Stage;
use std::collections::BTreeSet;

pub(super) fn check<S: Stage>(
    cx: &Cx<'_, S>,
    facts: &super::structure::Facts,
    report: &mut Report,
) {
    let unit = cx.unit;
    let expected = cx
        .operations
        .seal_events(&unit.header.definition, unit.header.instance);
    if unit.tables.seal_descriptors.is_empty()
        && !unit.instructions.iter().any(|i| {
            matches!(
                i,
                Inst::SealArena { .. }
                    | Inst::ArenaInventory { .. }
                    | Inst::BorrowSealedRoot { .. }
            )
        })
    {
        if expected.as_ref().is_some_and(|events| !events.is_empty()) {
            report.add(
                Site::UNIT,
                K::Sealed("sealed source event correspondence differs or is unavailable".into()),
            );
        }
        return;
    }
    for descriptor in &unit.tables.seal_descriptors {
        if let Err(error) = descriptor.validate_at(
            cx.operations,
            &unit.header.definition,
            unit.header.instance,
            cx.layouts.0,
            cx.target,
        ) {
            report.add(Site::UNIT, K::Sealed(error));
        }
    }
    let mut observed = Vec::new();
    for (b, block) in unit.blocks.iter().enumerate() {
        for (i, inst) in block
            .instructions
            .get(&unit.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let site = Site::at(BlockId(b as u32), i as u32);
            let (shape, anchor, kind) = match *inst {
                Inst::SealArena { shape, origin, .. } => (shape, origin, SourceEventKind::Seal),
                Inst::ArenaInventory {
                    shape,
                    allocation,
                    previous,
                    ..
                } => (
                    shape,
                    allocation,
                    if previous.is_some() {
                        SourceEventKind::Replace
                    } else {
                        SourceEventKind::Allocate
                    },
                ),
                Inst::BorrowSealedRoot {
                    shape,
                    owner,
                    loan,
                    result,
                } => {
                    let info = &unit.tables.loans[loan.index()];
                    let base = &unit.addresses[info.address.index()];
                    let owner_from_loan = match base.base {
                        AddressBase::Value(value) => value == owner,
                        AddressBase::Slot(_) => unit.instructions.iter().any(|inst| matches!(inst,
                            Inst::Read { result: read, address, .. } if *read == owner && *address == info.address)),
                        AddressBase::Address(_) => false,
                    };
                    if !owner_from_loan
                        || !info.mutable
                        || unit.values[result.index()].holding
                            != if cx.owns(result) {
                                Holding::Borrowed(loan)
                            } else {
                                Holding::Plain
                            }
                    {
                        report.add(
                            site,
                            K::Sealed("sealed root is not tied to its exclusive owner loan".into()),
                        );
                    }
                    (shape, block.anchor, SourceEventKind::BorrowRoot)
                }
                _ => continue,
            };
            if S::KIND == crate::stage::StageKind::Ready
                && matches!(inst, Inst::ArenaInventory { .. })
                && matches!(
                    inst,
                    Inst::ArenaInventory {
                        receipt: crate::ir::OptionalValueId::NONE,
                        ..
                    }
                )
                && unit.tables.seal_descriptors[shape.index()]
                    .bits_root(cx.layouts.0)
                    .is_err()
            {
                report.add(
                    site,
                    K::Sealed(
                        "Ready sealed transition requires an original pointer-free root".into(),
                    ),
                );
            }
            observed.push(SourceEvent {
                anchor: unit.tables.source_anchors[anchor.index()].clone(),
                kind,
                root_type: unit.tables.seal_descriptors[shape.index()]
                    .root_type
                    .clone(),
            });
        }
    }
    grant_flow(cx, facts, report);
    match expected {
        Some(expected) if expected == observed => {}
        None if observed.is_empty() && unit.tables.seal_descriptors.is_empty() => {}
        _ => report.add(
            Site::UNIT,
            K::Sealed("sealed source event correspondence differs or is unavailable".into()),
        ),
    }
}

fn grant_flow<S: Stage>(cx: &Cx<'_, S>, facts: &super::structure::Facts, report: &mut Report) {
    let unit = cx.unit;
    let mut origins = vec![BTreeSet::<ValueId>::new(); unit.values.len()];
    for value in &unit.parameters {
        if cx.repr(*value) == ValueRepr::Arena {
            origins[value.index()].insert(*value);
        }
    }
    for inst in &unit.instructions {
        match *inst {
            Inst::ArenaOpen { result, .. } => {
                origins[result.index()].insert(result);
            }
            Inst::ArenaToken { result, mark, .. } => {
                origins[result.index()].insert(mark);
            }
            Inst::Const { result, .. } if cx.repr(result) == ValueRepr::Arena => {
                origins[result.index()].insert(result);
            }
            _ => {}
        }
    }
    loop {
        let before = origins.clone();
        for inst in &unit.instructions {
            let pair = match *inst {
                Inst::Read {
                    result,
                    reaching: value,
                    ..
                }
                | Inst::Copy {
                    result,
                    reaching: value,
                    ..
                }
                | Inst::CopyValue { result, value }
                | Inst::Weaken { result, value }
                | Inst::Recast { result, value }
                | Inst::Store {
                    version: result,
                    value,
                    ..
                }
                | Inst::Replace {
                    version: result,
                    value,
                    ..
                } if cx.repr(result) == ValueRepr::Arena => Some((result, value)),
                _ => None,
            };
            if let Some((result, value)) = pair {
                origins[result.index()].extend(before[value.index()].iter().copied());
            }
        }
        for edge in &unit.tables.edges {
            let arguments = edge.arguments.get(&unit.pools.values).unwrap_or(&[]);
            let parameters = unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[]);
            for (&a, &p) in arguments.iter().zip(parameters) {
                if cx.repr(p) == ValueRepr::Arena {
                    origins[p.index()].extend(before[a.index()].iter().copied());
                }
            }
        }
        if before == origins {
            break;
        }
    }
    let roots = |grant: ValueId| origins[grant.index()].clone();
    let borrowed: BTreeSet<_> = unit
        .parameters
        .iter()
        .enumerate()
        .filter_map(|(i, &v)| {
            (cx.repr(v) == ValueRepr::Arena
                && cx.layouts.0.fn_abis[unit.header.abi.0 as usize].parameters[i].convention
                    != hirsel_core_contract::Passing::Take)
                .then_some(v)
        })
        .collect();
    let heap: BTreeSet<ValueId> = unit
        .instructions
        .iter()
        .filter_map(|inst| match *inst {
            Inst::Const { result, constant }
                if unit.tables.constants[constant.index()].value == ConstantValue::Heap =>
            {
                Some(result)
            }
            _ => None,
        })
        .collect();
    // Reuse the validator's normal/failure CFG and monotone worklist; no
    // enumeration-order approximation is an arena state proof.
    let entry = super::flow::solve(
        &facts.cfg,
        BTreeSet::<ValueId>::new(),
        |block, initial| {
            let mut state = initial.clone();
            let mut edges = Vec::new();
            for inst in unit.blocks[block.index()]
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
            {
                if let Inst::SealArena { grant, .. } = inst {
                    state.extend(roots(*grant));
                }
                match *inst {
                    Inst::ArenaOpen { result, .. } => {
                        state.remove(&result);
                    }
                    Inst::ArenaToken { mark, .. } => {
                        state.remove(&mark);
                    }
                    _ => {}
                }
                if let Some(failure) = inst.failure() {
                    edges.push((unit.tables.failures[failure.index()].cleanup, state.clone()));
                }
            }
            for out in &facts.cfg.succ[block.index()] {
                if out.origin == super::flow::Origin::Terminator {
                    edges.push((out.edge, state.clone()));
                }
            }
            edges
        },
        |into, from| into.extend(from.iter().copied()),
    );
    for (b, consumed) in entry.into_iter().enumerate() {
        let Some(mut consumed) = consumed else {
            continue;
        };
        for (i, inst) in unit.blocks[b]
            .instructions
            .get(&unit.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            let site = Site::at(BlockId(b as u32), i as u32);
            // Every direct use must respect grant consumption, including a
            // plain-bit alias read or a token forwarded through a helper.
            // Scope close/recovery uses the distinct guard, not the grant.
            let definitions = inst.defs();
            if !matches!(inst, Inst::SealArena { .. }) {
                inst.visit(&mut |reference| {
                    if let Ref::Value(value) = reference
                        && !definitions.contains(&value)
                        && roots(value).iter().any(|root| consumed.contains(root))
                    {
                        report.add(site, K::Sealed("use of a consumed arena grant".into()));
                    }
                });
            }
            match *inst {
                Inst::SealArena { grant, .. } => {
                    let origin = roots(grant);
                    if origin.is_empty() || origin.iter().any(|root| consumed.contains(root)) {
                        report.add(
                            site,
                            K::Sealed("arena grant missing or consumed on a path".into()),
                        );
                    }
                    if origin.iter().any(|root| borrowed.contains(root)) {
                        report.add(
                            site,
                            K::Sealed("borrow-only arena is not a consumable grant".into()),
                        );
                    }
                    if origin.iter().any(|root| heap.contains(root)) {
                        report.add(
                            site,
                            K::Sealed("heap is not a consumable arena grant".into()),
                        );
                    }
                    consumed.extend(origin);
                }
                Inst::ArenaOpen { result, .. } => {
                    consumed.remove(&result);
                }
                Inst::ArenaToken { mark, .. } => {
                    consumed.remove(&mark);
                }
                Inst::ArenaInventory { arena, .. }
                    if roots(arena).iter().any(|root| consumed.contains(root)) =>
                {
                    report.add(
                        site,
                        K::Sealed("allocation through a consumed arena grant".into()),
                    );
                }
                _ => {}
            }
        }
        if let Terminator::Call { call, .. } = unit.terminators[b] {
            for argument in unit.tables.calls[call.index()]
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap_or(&[])
            {
                if let ArgumentValue::Value(value) = argument.value
                    && roots(value).iter().any(|root| consumed.contains(root))
                {
                    report.add(
                        Site::block(BlockId(b as u32)),
                        K::Sealed("call uses a consumed arena grant".into()),
                    );
                }
            }
        }
    }
}
