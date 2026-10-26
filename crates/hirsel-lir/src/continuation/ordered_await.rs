//! Complete post-join wrapper coverage. C supplies language ownership; this
//! certificate ties the retained destination to both actual continuation edges
//! and every reachable cleanup site. It does not authorize a cleanup callback
//! or an entered runtime helper to leave its stack.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AwaitCleanup {
    pub block: BlockId,
    pub instruction: u32,
    pub address: AddressId,
    pub released: Option<ValueId>,
    pub anchor: CleanupAnchorId,
    pub failure: FailureId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderedAwaitProtocol {
    pub body: ContentHash,
    pub digest: ContentHash,
    pub block: BlockId,
    pub call: CallId,
    pub normal: EdgeId,
    pub failure: FailureId,
    pub result: RuntimeResultSlot,
    pub normal_suffix: Vec<BlockId>,
    pub failure_suffix: Vec<BlockId>,
    pub cleanup: Vec<AwaitCleanup>,
}

fn cleanup(
    u: &LirUnit<Ready>,
    block: BlockId,
    instruction: usize,
    inst: &Inst,
) -> Option<AwaitCleanup> {
    let (address, released, anchor, failure) = match *inst {
        Inst::Drop {
            address,
            anchor,
            failure,
            ..
        } => (address, None, anchor, failure),
        Inst::Release {
            value,
            layout,
            live: MaskRef::All,
            anchor,
            failure,
        } => {
            // RC lowers Drop to a whole-slot Take followed by Release. The
            // released SSA owner must be exactly that Take, in this block;
            // a payload projection or an unrelated owner cannot stand in.
            let instructions = u
                .blocks
                .get(block.index())?
                .instructions
                .get(&u.instructions)?;
            let previous = instruction.checked_sub(1)?;
            let Inst::Read {
                result,
                address,
                mode: ReadMode::Take,
                ..
            } = *instructions.get(previous)?
            else {
                return None;
            };
            let info = u.addresses.get(address.index())?;
            if result != value
                || info.projection.len != 0
                || !info.unchecked.is_empty()
                || u.types.get(info.ty.index())?.layout != layout
            {
                return None;
            }
            (address, Some(value), anchor, failure)
        }
        _ => return None,
    };
    Some(AwaitCleanup {
        block,
        instruction: instruction as u32,
        address,
        released,
        anchor,
        failure,
    })
}

fn root(u: &LirUnit<Ready>, mut address: AddressId) -> Option<SlotId> {
    for _ in 0..=u.addresses.len() {
        match u.addresses.get(address.index())?.base {
            AddressBase::Slot(slot) => return Some(slot),
            AddressBase::Address(base) => address = base,
            AddressBase::Value(_) => return None,
        }
    }
    None
}

/// Only the small, call-free await suffix is covered. A cleanup remains an
/// explicit site, including its failure successor, even for an owning result.
fn suffix(u: &LirUnit<Ready>, start: BlockId, slot: SlotId, loan: LoanId) -> Option<Vec<BlockId>> {
    let mut work = vec![(start, true, BTreeSet::new())];
    let mut seen = BTreeSet::new();
    while let Some((block, mut lent, mut path)) = work.pop() {
        if !path.insert(block) {
            return None;
        }
        // Inspect each incoming loan state; neither outcome may retain the
        // runtime's mutable loan while extracting or dropping the Option.
        if !seen.insert((block, lent)) {
            continue;
        }
        let instructions = u
            .blocks
            .get(block.index())?
            .instructions
            .get(&u.instructions)?;
        for (i, inst) in instructions.iter().enumerate() {
            match *inst {
                Inst::EndLoan {
                    loan: ended,
                    if_active: false,
                } if ended == loan && lent => lent = false,
                Inst::EndLoan { loan: ended, .. } | Inst::BeginLoan { loan: ended, .. }
                    if ended == loan =>
                {
                    return None;
                }
                Inst::Store { address, .. } | Inst::Replace { address, .. }
                    if root(u, address) == Some(slot) =>
                {
                    return None;
                }
                Inst::Read { address, .. } | Inst::Drop { address, .. }
                    if root(u, address) == Some(slot) && lent =>
                {
                    return None;
                }
                Inst::Release { .. }
                    if lent
                        || cleanup(u, block, i, inst)
                            .is_none_or(|c| root(u, c.address) != Some(slot)) =>
                {
                    return None;
                }
                Inst::Reset { .. } | Inst::FrameEnter { .. } => return None,
                _ => {}
            }
            if let Some(failure) = inst.failure() {
                let edge = u.tables.failures.get(failure.index())?.cleanup;
                work.push((u.tables.edges.get(edge.index())?.target, lent, path.clone()));
            }
        }
        let term = u.terminators.get(block.index())?;
        if !matches!(
            term,
            Terminator::Jump(_)
                | Terminator::Switch { .. }
                | Terminator::Return(_)
                | Terminator::Propagate
                | Terminator::InvariantFault { .. }
        ) {
            return None;
        }
        let edges = term.normal_edges(&u.pools);
        if edges.is_empty() && lent {
            return None;
        }
        for edge in edges {
            work.push((u.tables.edges.get(edge.index())?.target, lent, path.clone()));
        }
    }
    Some(
        seen.into_iter()
            .map(|(b, _)| b)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    )
}

impl OrderedAwaitProtocol {
    pub fn derive(
        unit: &ValidatedUnit<Ready>,
        table: &LayoutTable,
        block: BlockId,
        frame: &[LiveStorage],
    ) -> Option<Self> {
        let u = unit.unit();
        let result = ordered_join_result(u, table, block, frame)?;
        let RuntimeResultSource::ArgumentSlot { slot, loan, .. } = result.source else {
            return None;
        };
        // The reviewed body has exactly one call. Additional callbacks or
        // wrapper operations need their own complete protocol before entry.
        if u.tables.calls.len() != 1 {
            return None;
        }
        let Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        } = *u.terminators.get(block.index())?
        else {
            return None;
        };
        let normal_suffix = suffix(u, u.tables.edges.get(normal.index())?.target, slot, loan)?;
        let failure_edge = u.tables.failures.get(failure.index())?.cleanup;
        let failure_suffix = suffix(
            u,
            u.tables.edges.get(failure_edge.index())?.target,
            slot,
            loan,
        )?;
        let covered: BTreeSet<_> = normal_suffix
            .iter()
            .chain(&failure_suffix)
            .copied()
            .collect();
        let mut cleanup_sites = Vec::new();
        for block in covered {
            for (i, inst) in u.blocks[block.index()]
                .instructions
                .get(&u.instructions)?
                .iter()
                .enumerate()
            {
                if let Some(site) = cleanup(u, block, i, inst) {
                    if root(u, site.address) != Some(slot) {
                        return None;
                    }
                    cleanup_sites.push(site);
                }
            }
        }
        Some(Self {
            body: u.header.instance,
            digest: u.digest(),
            block,
            call,
            normal,
            failure,
            result,
            normal_suffix,
            failure_suffix,
            cleanup: cleanup_sites,
        })
    }

    /// Reconstruct edge coverage and loan release independently of `derive`.
    /// An imported digest, suffix list or cleanup cursor is never authority.
    pub fn validate(
        &self,
        unit: &ValidatedUnit<Ready>,
        table: &LayoutTable,
        frame: &[LiveStorage],
    ) -> bool {
        let u = unit.unit();
        if self.body != u.header.instance
            || self.digest != u.digest()
            || u.tables.calls.len() != 1
            || !check_ordered_join_result(u, table, self.block, frame, &self.result)
            || !matches!(u.terminators.get(self.block.index()),Some(Terminator::Call {call,normal:Some(normal),failure}) if *call==self.call && *normal==self.normal && *failure==self.failure)
        {
            return false;
        }
        let RuntimeResultSource::ArgumentSlot { slot, loan, .. } = self.result.source else {
            return false;
        };
        let Some(normal) = u.tables.edges.get(self.normal.index()) else {
            return false;
        };
        let Some(failure) = u
            .tables
            .failures
            .get(self.failure.index())
            .and_then(|f| u.tables.edges.get(f.cleanup.index()))
        else {
            return false;
        };
        for (start, claimed) in [
            (normal.target, &self.normal_suffix),
            (failure.target, &self.failure_suffix),
        ] {
            let claimed_set: BTreeSet<_> = claimed.iter().copied().collect();
            if claimed_set.len() != claimed.len() {
                return false;
            }
            let mut pending = vec![(start, true, Vec::new())];
            let mut reached = BTreeSet::new();
            let mut states = BTreeSet::new();
            while let Some((b, mut active, mut path)) = pending.pop() {
                if path.contains(&b) || !claimed_set.contains(&b) {
                    return false;
                }
                path.push(b);
                reached.insert(b);
                if !states.insert((b, active)) {
                    continue;
                }
                let Some(instructions) = u
                    .blocks
                    .get(b.index())
                    .and_then(|b| b.instructions.get(&u.instructions))
                else {
                    return false;
                };
                for (i, inst) in instructions.iter().enumerate() {
                    match *inst {
                        Inst::EndLoan { loan: l, if_active } if l == loan => {
                            if if_active || !active {
                                return false;
                            }
                            active = false;
                        }
                        Inst::BeginLoan { loan: l, .. } if l == loan => return false,
                        Inst::Store { address, .. } | Inst::Replace { address, .. }
                            if root(u, address) == Some(slot) =>
                        {
                            return false;
                        }
                        Inst::Read { address, .. } | Inst::Drop { address, .. }
                            if active && root(u, address) == Some(slot) =>
                        {
                            return false;
                        }
                        Inst::Release { .. }
                            if active
                                || cleanup(u, b, i, inst)
                                    .is_none_or(|c| root(u, c.address) != Some(slot)) =>
                        {
                            return false;
                        }
                        Inst::Reset { .. } | Inst::FrameEnter { .. } => {
                            return false;
                        }
                        _ => {}
                    }
                    if let Some(f) = inst.failure() {
                        let Some(target) = u
                            .tables
                            .failures
                            .get(f.index())
                            .and_then(|f| u.tables.edges.get(f.cleanup.index()))
                            .map(|e| e.target)
                        else {
                            return false;
                        };
                        pending.push((target, active, path.clone()));
                    }
                }
                let Some(term) = u.terminators.get(b.index()) else {
                    return false;
                };
                match term {
                    Terminator::Jump(_) | Terminator::Switch { .. } => {
                        for edge in term.normal_edges(&u.pools) {
                            let Some(e) = u.tables.edges.get(edge.index()) else {
                                return false;
                            };
                            pending.push((e.target, active, path.clone()));
                        }
                    }
                    Terminator::Return(_)
                    | Terminator::Propagate
                    | Terminator::InvariantFault { .. }
                        if !active => {}
                    _ => return false,
                }
            }
            if reached != claimed_set {
                return false;
            }
        }
        let covered: BTreeSet<_> = self
            .normal_suffix
            .iter()
            .chain(&self.failure_suffix)
            .copied()
            .collect();
        let mut supplied = BTreeSet::new();
        for site in &self.cleanup {
            if !covered.contains(&site.block)
                || !supplied.insert((site.block, site.instruction))
                || root(u, site.address) != Some(slot)
            {
                return false;
            }
            let Some(inst) = u
                .blocks
                .get(site.block.index())
                .and_then(|b| b.instructions.get(&u.instructions))
                .and_then(|xs| xs.get(site.instruction as usize))
            else {
                return false;
            };
            if cleanup(u, site.block, site.instruction as usize, inst).as_ref() != Some(site) {
                return false;
            }
        }
        for b in covered {
            let Some(instructions) = u.blocks[b.index()].instructions.get(&u.instructions) else {
                return false;
            };
            for (i, inst) in instructions.iter().enumerate() {
                if cleanup(u, b, i, inst).is_some() && !supplied.contains(&(b, i as u32)) {
                    return false;
                }
            }
        }
        true
    }
}
