//! Exact physical finalizer groups inside one pass-authored aggregate cleanup.
//! Addresses name checked declaration paths, never payload bits or byte offsets.
use super::*;

fn path<S>(unit: &LirUnit<S>, address: AddressId) -> Option<(SlotId, Vec<ProjectionStep>)> {
    let mut at = address;
    let mut segments = Vec::new();
    for _ in 0..unit.addresses.len() {
        let address = unit.addresses.get(at.index())?;
        let fields = address.projection.get(&unit.pools.projections)?;
        let fields = fields
            .iter()
            .map(|step| match *step {
                ProjectionStep::Field(_) | ProjectionStep::VariantField { .. } => Some(*step),
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

// A static variant address immediately inside its unique selecting arm has
// already passed its tag test. Its synthetic failure edge is infeasible.
// Require the actual borrowed scrutinee, exact slot/path, unique incoming edge,
// and no intervening instruction except ending the switch's loan.
pub(super) fn selected_addresses<S: Stage>(
    unit: &LirUnit<S>,
) -> Option<std::collections::BTreeSet<(BlockId, u32)>> {
    let mut incoming = vec![0usize; unit.blocks.len()];
    let mut add = |edge: EdgeId| -> Option<()> {
        let target = unit.tables.edges.get(edge.index())?.target;
        let count = incoming.get_mut(target.index())?;
        *count = count.checked_add(1)?;
        Some(())
    };
    for term in &unit.terminators {
        for edge in term.normal_edges(&unit.pools) {
            add(edge)?;
        }
        for failure in term.failure_edges(&unit.tables) {
            add(unit.tables.failures.get(failure.index())?.cleanup)?;
        }
    }
    for block in &unit.blocks {
        for inst in block.instructions.get(&unit.instructions)? {
            if let Some(failure) = inst.failure() {
                add(unit.tables.failures.get(failure.index())?.cleanup)?;
            }
        }
    }
    let mut selected = std::collections::BTreeSet::new();
    for (at, term) in unit.terminators.iter().enumerate() {
        let Terminator::Switch { value, arms, .. } = term else {
            continue;
        };
        let code = unit.blocks.get(at)?.instructions.get(&unit.instructions)?;
        let Some(Inst::Read {
            result,
            address,
            mode: ReadMode::Borrow,
            ..
        }) = code.last()
        else {
            continue;
        };
        if result != value {
            continue;
        }
        let Some((slot, prefix)) = path(unit, *address) else {
            continue;
        };
        for arm in arms.get(&unit.pools.switch_arms)? {
            let block = unit.tables.edges.get(arm.edge.index())?.target;
            if incoming[block.index()] != 1 {
                continue;
            }
            let code = unit
                .blocks
                .get(block.index())?
                .instructions
                .get(&unit.instructions)?;
            for (index, inst) in code.iter().enumerate() {
                match *inst {
                    Inst::EndLoan {
                        if_active: false, ..
                    } => (),
                    Inst::Address { address, .. } => {
                        let Some((target, steps)) = path(unit, address) else {
                            break;
                        };
                        if target != slot || !steps.starts_with(&prefix) {
                            break;
                        }
                        let Some((ProjectionStep::VariantField { variant, .. }, tail)) =
                            steps[prefix.len()..].split_first()
                        else {
                            break;
                        };
                        if *variant != arm.value
                            || !tail.iter().all(|s| matches!(s, ProjectionStep::Field(_)))
                        {
                            break;
                        }
                        selected.insert((block, index as u32));
                    }
                    _ => break,
                }
            }
        }
    }
    Some(selected)
}

// A completion flag is a runtime bool, even when every incoming SSA argument
// is the same constant. Prove that fact from actual definitions and every used
// edge. InitFlag metadata alone cannot justify excluding a cleanup branch.
fn constant_branches<S>(unit: &LirUnit<S>) -> Option<BTreeMap<BlockId, EdgeId>> {
    let mut known = BTreeMap::new();
    for block in &unit.blocks {
        for inst in block.instructions.get(&unit.instructions)? {
            if let Inst::Const { result, constant } = *inst
                && let ConstantValue::Bool(value) =
                    unit.tables.constants.get(constant.index())?.value
            {
                known.insert(result, value);
            }
        }
    }
    let mut arguments: BTreeMap<ValueId, Vec<ValueId>> = BTreeMap::new();
    for edge in check::used_edges(unit) {
        let edge = unit.tables.edges.get(edge.index())?;
        let parameters = unit
            .blocks
            .get(edge.target.index())?
            .parameters
            .get(&unit.pools.values)?;
        let values = edge.arguments.get(&unit.pools.values)?;
        if parameters.len() != values.len() {
            return None;
        }
        for (&parameter, &value) in parameters.iter().zip(values) {
            arguments.entry(parameter).or_default().push(value);
        }
    }
    let mut pending = BTreeMap::new();
    let mut users: BTreeMap<ValueId, Vec<ValueId>> = BTreeMap::new();
    for (parameter, values) in arguments {
        pending.insert(parameter, (values.len(), 3u8));
        for value in values {
            users.entry(value).or_default().push(parameter);
        }
    }
    let mut work: std::collections::VecDeque<_> = known.keys().copied().collect();
    while let Some(value) = work.pop_front() {
        for &parameter in users.get(&value).into_iter().flatten() {
            let (remaining, possibilities) = pending.get_mut(&parameter)?;
            *remaining = remaining.checked_sub(1)?;
            *possibilities &= if known[&value] { 2 } else { 1 };
            if *remaining == 0 && *possibilities != 0 {
                if known.insert(parameter, *possibilities == 2).is_some() {
                    return None;
                }
                work.push_back(parameter);
            }
        }
    }
    Some(
        unit.terminators
            .iter()
            .enumerate()
            .filter_map(|(i, term)| {
                let Terminator::Branch { condition, yes, no } = term else {
                    return None;
                };
                known
                    .get(condition)
                    .map(|value| (BlockId(i as u32), if *value { *yes } else { *no }))
            })
            .collect(),
    )
}

pub(super) fn parts<S: Stage>(
    unit: &LirUnit<S>,
    point: Point,
    root: AddressId,
) -> Option<Vec<(BlockId, AddressId)>> {
    let Point::CompoundDrop {
        anchor,
        staged,
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
    let (storage, start) = if let Some((at, storage)) = staged {
        let code = unit
            .blocks
            .get(block.index())?
            .instructions
            .get(&unit.instructions)?;
        let [
            Inst::Read {
                result,
                address,
                mode: ReadMode::Take,
                ..
            },
            Inst::StorageLive { slot },
            Inst::Store {
                address: to, value, ..
            },
        ] = code.get(at as usize..at.checked_add(3)? as usize)?
        else {
            return None;
        };
        let target = unit.addresses.get(storage.index())?;
        if *address != root
            || *result != *value
            || *to != storage
            || root == storage
            || target.base != AddressBase::Slot(*slot)
            || target.projection.len != 0
            || unit.slots.get(slot.index())?.kind != SlotKind::Temporary
            || unit.addresses.get(root.index())?.ty != target.ty
        {
            return None;
        }
        (storage, at)
    } else {
        let code = unit
            .blocks
            .get(block.index())?
            .instructions
            .get(&unit.instructions)?;
        let (first, start) = if let Some((first, actual)) = finalizer::begin(unit, block) {
            if actual != anchor {
                return None;
            }
            (
                first,
                code.iter()
                    .position(|inst| matches!(inst, Inst::FinalizeBegin { .. }))?,
            )
        } else {
            let Terminator::Switch { value, .. } = unit.terminators.get(block.index())? else {
                return None;
            };
            let [
                Inst::BeginLoan {
                    loan,
                    address,
                    mutable: false,
                    if_inactive: false,
                },
                Inst::Read {
                    result,
                    address: read,
                    mode: ReadMode::Borrow,
                    ..
                },
            ] = code.get(code.len().checked_sub(2)?..)?
            else {
                return None;
            };
            if result != value
                || read != address
                || unit.values.get(value.index())?.holding != Holding::Borrowed(*loan)
            {
                return None;
            }
            (*address, code.len() - 2)
        };
        if !path(unit, first).is_some_and(|(s, p)| {
            path(unit, root).is_some_and(|(r, q)| s == r && p.starts_with(&q))
        }) {
            return None;
        }
        (root, u32::try_from(start).ok()?)
    };
    let (slot, prefix) = path(unit, storage)?;
    // The containing Resource has its own finalizer before its child actions.
    // The exact slot/prefix and common cleanup anchor still bind the whole cut.
    let descendant = |address: AddressId, strict: bool| {
        path(unit, address).is_some_and(|(s, p)| {
            s == slot && p.starts_with(&prefix) && (!strict || p.len() > prefix.len())
        })
    };
    struct Walk {
        selected: std::collections::BTreeSet<(BlockId, u32)>,
        branches: BTreeMap<BlockId, EdgeId>,
        state: Vec<u8>,
        parts: Vec<(BlockId, AddressId)>,
        normal: bool,
        prefix_len: usize,
    }
    #[allow(clippy::too_many_arguments)]
    fn visit<S: Stage>(
        unit: &LirUnit<S>,
        block: BlockId,
        entry: BlockId,
        start: u32,
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
        let offset = if block == entry { start as usize } else { 0 };
        let code = code.get(offset..)?;
        let mut next = Vec::new();
        for (index, instruction) in code.iter().enumerate() {
            match *instruction {
                Inst::Address { address, .. }
                    if descendant(address, false)
                        && path(unit, address)?.1[walk.prefix_len..]
                            .iter()
                            .any(|step| matches!(step, ProjectionStep::VariantField { .. })) =>
                {
                    if !walk.selected.contains(&(block, (offset + index) as u32)) {
                        return None;
                    }
                }
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
            if let Some(failure) = instruction.failure()
                && !(matches!(instruction, Inst::Address { .. })
                    && walk.selected.contains(&(block, (offset + index) as u32)))
            {
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
            } else if let Terminator::Switch { value, .. } = term {
                let address = code.iter().find_map(|inst| match inst {
                    Inst::Read {
                        result,
                        address,
                        mode: ReadMode::Borrow,
                        ..
                    } if result == value => Some(*address),
                    _ => None,
                })?;
                if !descendant(address, false) {
                    return None;
                }
            } else if !matches!(term, Terminator::Jump(_) | Terminator::Branch { .. }) {
                if matches!(term, Terminator::Unreachable) && code.is_empty() {
                    walk.state[block.index()] = 2;
                    return Some(());
                }
                return None;
            }
            if let Some(&edge) = walk.branches.get(&block) {
                next.push(edge);
            } else {
                next.extend(term.normal_edges(&unit.pools));
            }
            for failure in term.failure_edges(&unit.tables) {
                next.push(unit.tables.failures.get(failure.index())?.cleanup);
            }
        }
        for edge in next {
            visit(
                unit,
                unit.tables.edges.get(edge.index())?.target,
                entry,
                start,
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
        selected: selected_addresses(unit)?,
        branches: constant_branches(unit)?,
        state: vec![0; unit.blocks.len()],
        parts: Vec::new(),
        normal: false,
        prefix_len: prefix.len(),
    };
    visit(
        unit,
        block,
        block,
        start,
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
