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
    let descendant = |address| {
        path(unit, address)
            .is_some_and(|(s, p)| s == slot && p.starts_with(&prefix) && p.len() > prefix.len())
    };
    if !descendant(first) {
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
        descendant: &impl Fn(AddressId) -> bool,
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
        for instruction in code {
            match *instruction {
                Inst::FinalizeBegin {
                    address,
                    anchor: action,
                } => {
                    if action != anchor || !descendant(address) {
                        return None;
                    }
                    finalizer::exits(unit, block, address)?;
                    walk.parts.push((block, address));
                }
                Inst::FinalizeEnd {
                    address,
                    anchor: action,
                } => {
                    if action != anchor || !descendant(address) {
                        return None;
                    }
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
                if action != anchor || !descendant(address) {
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
    // A forged cut cannot omit another physical descendant of this exact
    // cleanup action, even when the retained prefix has valid completions.
    for (block, _) in unit.blocks.iter().enumerate() {
        if let Some((address, action)) = finalizer::begin(unit, BlockId(block as u32))
            && action == anchor
            && descendant(address)
            && !walk.parts.contains(&(BlockId(block as u32), address))
        {
            return None;
        }
    }
    (walk.normal && !walk.parts.is_empty()).then_some(walk.parts)
}
