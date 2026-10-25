//! A semantic finalizer drop has one identity and several exclusive exits.
//! Check its exact phase/address/action and call edges rather than accepting a
//! nearby FinalizeEnd with an equal source span. Ordinary V still judges the
//! finalizer's semantic phase protocol independently of this observation.
use super::*;
use hirsel_core_contract::Passing;

pub(super) fn begin<S: crate::stage::Stage>(
    unit: &LirUnit<S>,
    block: BlockId,
) -> Option<(AddressId, CleanupAnchorId)> {
    let code = unit
        .blocks
        .get(block.index())?
        .instructions
        .get(&unit.instructions)?;
    let mut begins = code.iter().filter_map(|instruction| match *instruction {
        Inst::FinalizeBegin { address, anchor } => Some((address, anchor)),
        _ => None,
    });
    let begin = begins.next()?;
    begins.next().is_none().then_some(begin)
}

/// Physical commits for one logical marker, derived only after its binding is
/// checked. This does not create extra marker identities for branch alternatives.
pub(super) fn exits<S: crate::stage::Stage>(
    unit: &LirUnit<S>,
    block: BlockId,
    address: AddressId,
) -> Option<Vec<Point>> {
    let (at, anchor) = begin(unit, block)?;
    if at != address {
        return None;
    }
    let code = unit
        .blocks
        .get(block.index())?
        .instructions
        .get(&unit.instructions)?;
    let Terminator::Call {
        call,
        normal: Some(normal),
        failure,
    } = *unit.terminators.get(block.index())?
    else {
        return None;
    };
    let call = unit.tables.calls.get(call.index())?;
    if !matches!(call.callee, Callee::Direct(_))
        || call.result.is_some()
        || call.pretransfer_failure.is_some()
    {
        return None;
    }
    let [argument] = call.arguments.get(&unit.pools.call_arguments)? else {
        return None;
    };
    if argument.parameter != 0 {
        return None;
    }
    let mut rejected = None;
    match (argument.passing, argument.value) {
        (Passing::Borrow, ArgumentValue::Value(value)) => {
            let mut reads = code.iter().filter_map(|instruction| match *instruction {
                Inst::Read {
                    result,
                    address: from,
                    mode: ReadMode::Borrow,
                    ..
                } if result == value => Some(from),
                _ => None,
            });
            if reads.next() != Some(address) || reads.next().is_some() {
                return None;
            }
            let mut guards = code.iter().filter_map(|instruction| match *instruction {
                Inst::FrameEnter { failure } => Some(failure),
                _ => None,
            });
            rejected = Some(guards.next()?);
            if guards.next().is_some() {
                return None;
            }
        }
        (Passing::Mut, ArgumentValue::Address { address: from, .. }) if from == address => {
            if code
                .iter()
                .any(|instruction| matches!(instruction, Inst::FrameEnter { .. }))
            {
                return None;
            }
        }
        _ => return None,
    }
    let mut targets = vec![unit.tables.edges.get(normal.index())?.target];
    for failure in [Some(failure), rejected].into_iter().flatten() {
        let edge = unit.tables.failures.get(failure.index())?.cleanup;
        targets.push(unit.tables.edges.get(edge.index())?.target);
    }
    let mut unique = std::collections::BTreeSet::new();
    let mut exits = vec![];
    for target in targets {
        if !unique.insert(target) {
            return None;
        }
        let code = unit
            .blocks
            .get(target.index())?
            .instructions
            .get(&unit.instructions)?;
        let mut ends =
            code.iter()
                .enumerate()
                .filter_map(|(index, instruction)| match *instruction {
                    Inst::FinalizeEnd {
                        address: at,
                        anchor: action,
                    } if at == address && action == anchor => Some(index),
                    _ => None,
                });
        let index = ends.next()?;
        if ends.next().is_some()
            || !code[..index].iter().all(|instruction| {
                matches!(
                    instruction,
                    Inst::FrameLeave
                        | Inst::EndLoan {
                            if_active: false,
                            ..
                        }
                )
            })
        {
            return None;
        }
        exits.push(Point::Instruction {
            block: target,
            index: index as u32,
            moment: Moment::After,
        });
    }
    Some(exits)
}

/// The original semantic Drop failure edge becomes these exclusive jumps,
/// after body failure or rejected finalizer admission has completed cleanup.
pub(super) fn failure_edges<S: crate::stage::Stage>(
    unit: &LirUnit<S>,
    block: BlockId,
    parameter: ValueId,
) -> Option<Vec<EdgeId>> {
    let (address, _) = begin(unit, block)?;
    let commits = exits(unit, block, address)?;
    let mut found = std::collections::BTreeSet::new();
    let mut states = vec![0u8; unit.blocks.len()];
    fn visit<S>(
        unit: &LirUnit<S>,
        edge: EdgeId,
        parameter: ValueId,
        states: &mut [u8],
        found: &mut std::collections::BTreeSet<EdgeId>,
    ) -> Option<()> {
        let row = unit.tables.edges.get(edge.index())?;
        let block = unit.blocks.get(row.target.index())?;
        if block
            .parameters
            .get(&unit.pools.values)?
            .contains(&parameter)
        {
            found.insert(edge);
            return Some(());
        }
        let at = row.target.index();
        match states[at] {
            1 => return None, // a cleanup cycle cannot prove completion
            2 => return Some(()),
            _ => (),
        }
        if !matches!(block.kind, BlockKind::Cleanup | BlockKind::FailureCleanup) {
            return None;
        }
        states[at] = 1;
        let term = unit.terminators.get(at)?;
        let mut successors = term.normal_edges(&unit.pools);
        for failure in term.failure_edges(&unit.tables) {
            successors.push(unit.tables.failures.get(failure.index())?.cleanup);
        }
        for inst in block.instructions.get(&unit.instructions)? {
            if let Some(failure) = inst.failure() {
                successors.push(unit.tables.failures.get(failure.index())?.cleanup);
            }
        }
        if successors.is_empty() {
            return None;
        }
        successors.sort();
        successors.dedup();
        for next in successors {
            visit(unit, next, parameter, states, found)?;
        }
        states[at] = 2;
        Some(())
    }
    for point in commits.into_iter().skip(1) {
        let Point::Instruction { block, .. } = point else {
            return None;
        };
        let Terminator::Jump(edge) = *unit.terminators.get(block.index())? else {
            return None;
        };
        visit(unit, edge, parameter, &mut states, &mut found)?;
    }
    (!found.is_empty()).then(|| found.into_iter().collect())
}
