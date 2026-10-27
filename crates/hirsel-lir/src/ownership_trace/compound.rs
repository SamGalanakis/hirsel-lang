//! Exact physical finalizer groups inside one pass-authored aggregate cleanup.
//! Addresses name checked declaration paths, never payload bits or byte offsets.
use super::*;

fn path<S>(unit: &LirUnit<S>, address: AddressId) -> Option<(SlotId, Vec<u32>)> {
    let mut at = address;
    let mut segments = Vec::new();
    for _ in 0..unit.addresses.len() {
        let address = unit.addresses.get(at.index())?;
        let fields = address.projection.get(&unit.pools.projections)?;
        let fields = fields
            .iter()
            .map(|step| match *step {
                ProjectionStep::Field(field) => Some(field),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        segments.push(fields);
        match address.base {
            AddressBase::Slot(slot) => {
                return Some((slot, segments.into_iter().rev().flatten().collect()));
            }
            AddressBase::Address(parent) => at = parent,
            _ => return None,
        }
    }
    None
}

pub(super) fn parts<S: Stage>(
    unit: &LirUnit<S>,
    point: Point,
    root: AddressId,
) -> Option<Vec<(BlockId, AddressId)>> {
    let Point::CompoundDrop {
        block,
        normal,
        index,
        failure,
    } = point
    else {
        return None;
    };
    if block == failure || normal == failure {
        return None;
    }
    let (first, anchor) = finalizer::begin(unit, block)?;
    let (slot, prefix) = path(unit, root)?;
    // The containing Resource has its own finalizer before its child actions.
    // The exact slot/prefix and common cleanup anchor still bind the whole cut.
    let descendant = |address: AddressId, strict: bool| {
        path(unit, address).is_some_and(|(s, p)| {
            s == slot && p.starts_with(&prefix) && (!strict || p.len() > prefix.len())
        })
    };
    if !descendant(first, false) {
        return None;
    }
    struct Walk {
        state: Vec<u8>,
        parts: Vec<(BlockId, AddressId)>,
        normal: bool,
    }
    #[allow(clippy::too_many_arguments)]
    fn visit<S: Stage>(
        unit: &LirUnit<S>,
        block: BlockId,
        entry: BlockId,
        normal: BlockId,
        index: u32,
        failure: BlockId,
        anchor: CleanupAnchorId,
        descendant: &impl Fn(AddressId, bool) -> bool,
        walk: &mut Walk,
    ) -> Option<()> {
        if block == failure {
            return Some(());
        }
        let state = walk.state.get_mut(block.index())?;
        match *state {
            1 => return None,
            2 => return Some(()),
            _ => *state = 1,
        }
        let code = unit
            .blocks
            .get(block.index())?
            .instructions
            .get(&unit.instructions)?;
        let code = if block == normal {
            code.get(..index as usize)?
        } else {
            code
        };
        // Preparing an address before the semantic drop enters its first
        // phase can fail into another cleanup action. That action retains its
        // own original marker; it is not a completion of this drop.
        let code = if block == entry {
            let start = code
                .iter()
                .position(|inst| matches!(inst, Inst::FinalizeBegin { .. }))?;
            &code[start..]
        } else {
            code
        };
        let mut next = Vec::new();
        for (index, instruction) in code.iter().enumerate() {
            match *instruction {
                Inst::FinalizeBegin {
                    address,
                    anchor: action,
                } => {
                    if action != anchor || !descendant(address, false) {
                        return None;
                    }
                    finalizer::exits(unit, block, address)?;
                    walk.parts.push((block, address));
                }
                Inst::FinalizeEnd {
                    address,
                    anchor: action,
                } => {
                    if action != anchor || !descendant(address, false) {
                        return None;
                    }
                }
                Inst::Release {
                    value,
                    anchor: action,
                    live: MaskRef::All,
                    ..
                } => {
                    let Inst::Read {
                        result,
                        address,
                        mode: ReadMode::Take,
                        ..
                    } = *code.get(index.checked_sub(1)?)?
                    else {
                        return None;
                    };
                    if action != anchor || result != value || !descendant(address, true) {
                        return None;
                    }
                    // This proves the physical child release belongs to this
                    // cut. Native preparation independently requires coverage
                    // of every selected Ready transition, so a selected child
                    // cannot disappear through an unobserved release.
                }
                Inst::Release { .. } | Inst::Drop { .. } => return None,
                _ => (),
            }
            if let Some(failure) = instruction.failure() {
                next.push(unit.tables.failures.get(failure.index())?.cleanup);
            }
        }
        if block == normal {
            walk.normal = true;
        } else {
            let term = unit.terminators.get(block.index())?;
            if matches!(term, Terminator::Call { .. }) {
                let (address, action) = finalizer::begin(unit, block)?;
                if action != anchor || !descendant(address, false) {
                    return None;
                }
            } else if !matches!(term, Terminator::Jump(_) | Terminator::Branch { .. }) {
                return None;
            }
            next.extend(term.normal_edges(&unit.pools));
            for failure in term.failure_edges(&unit.tables) {
                next.push(unit.tables.failures.get(failure.index())?.cleanup);
            }
        }
        for edge in next {
            visit(
                unit,
                unit.tables.edges.get(edge.index())?.target,
                entry,
                normal,
                index,
                failure,
                anchor,
                descendant,
                walk,
            )?;
        }
        walk.state[block.index()] = 2;
        Some(())
    }
    let mut walk = Walk {
        state: vec![0; unit.blocks.len()],
        parts: Vec::new(),
        normal: false,
    };
    visit(
        unit,
        block,
        block,
        normal,
        index,
        failure,
        anchor,
        &descendant,
        &mut walk,
    )?;
    // Alternative semantic drops can share a source cleanup anchor. Their
    // finalizers outside this entry's CFG are independent obligations. A cut
    // is incomplete only if its own continuation still reaches a descendant
    // retirement before leaving or reinitializing the original storage.
    let mut pending = vec![(normal, index)];
    let mut seen = std::collections::BTreeSet::new();
    while let Some((block, start)) = pending.pop() {
        if block == failure || !seen.insert((block, start)) {
            continue;
        }
        let code = unit
            .blocks
            .get(block.index())?
            .instructions
            .get(&unit.instructions)?;
        let mut stopped = false;
        for (relative, inst) in code.get(start as usize..)?.iter().enumerate() {
            match *inst {
                Inst::StorageDead { slot: at, .. } | Inst::StorageLive { slot: at }
                    if at == slot =>
                {
                    stopped = true;
                    break;
                }
                Inst::Release {
                    value,
                    anchor: action,
                    ..
                } => {
                    let at = start as usize + relative;
                    if let Some(Inst::Read {
                        result,
                        address,
                        mode: ReadMode::Take,
                        ..
                    }) = at.checked_sub(1).and_then(|at| code.get(at))
                        && *result == value
                        && action == anchor
                        && descendant(*address, true)
                    {
                        return None;
                    }
                    stopped = true;
                    break;
                }
                Inst::FinalizeBegin {
                    address,
                    anchor: action,
                } => {
                    if action == anchor && descendant(address, false) {
                        return None;
                    }
                    stopped = true;
                    break;
                }
                _ => (),
            }
        }
        if stopped {
            continue;
        }
        let term = unit.terminators.get(block.index())?;
        // An unrelated source call starts another operation. It cannot be a
        // completion of this cleanup; ordinary C checks the remaining program.
        if matches!(term, Terminator::Call { .. }) {
            continue;
        }
        for edge in term.normal_edges(&unit.pools) {
            pending.push((unit.tables.edges.get(edge.index())?.target, 0));
        }
        for failure in term.failure_edges(&unit.tables) {
            let edge = unit.tables.failures.get(failure.index())?.cleanup;
            pending.push((unit.tables.edges.get(edge.index())?.target, 0));
        }
    }
    (walk.normal && !walk.parts.is_empty()).then_some(walk.parts)
}
