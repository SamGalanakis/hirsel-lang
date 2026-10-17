//! Independent physical-holder tally. RC instructions never update it.
//! Allocation cells journal their writes; SSA bindings journal credit writes.
//! Incoming-cell positions let a lifetime/generation change find old pointers
//! without scanning unrelated storage. Ordered candidates preserve recount order.
use super::*;
use crate::snapshot::Snapshot;
use std::collections::BTreeSet;
use std::sync::Arc;

type Position = (AllocId, u64);

/// Sparse physical positions, ordered by allocation and byte offset. Both
/// levels share unchanged branches, including a target with many incoming cells.
#[derive(Clone, Default)]
struct Incoming {
    sources: Snapshot<Snapshot<Ptr, 15>>,
}
impl Incoming {
    fn get(&self, &(source, offset): &Position) -> Option<&Ptr> {
        self.sources.get(source.0 as usize)?.get(offset as usize)
    }
    fn insert(&mut self, (source, offset): Position, ptr: Ptr) {
        let i = source.0 as usize;
        if self.sources.get(i).is_none() {
            self.sources.set(i, Snapshot::default());
        }
        self.sources[i].set(offset as usize, ptr);
    }
    fn remove(&mut self, &(source, offset): &Position) {
        let i = source.0 as usize;
        if let Some(cells) = self.sources.get_mut(i) {
            cells.remove(offset as usize);
            if cells.is_empty() {
                self.sources.remove(i);
            }
        }
    }
    fn entries(&self) -> impl Iterator<Item = (Position, Ptr)> {
        self.sources.indexed().flat_map(|(source, cells)| {
            cells
                .indexed()
                .map(move |(offset, ptr)| ((AllocId(source as u32), offset as u64), *ptr))
        })
    }
    fn keys(&self) -> impl Iterator<Item = Position> {
        self.entries().map(|(p, _)| p)
    }
    fn differences(&self, old: &Self, mut visit: impl FnMut(Position)) {
        self.sources
            .differences(&old.sources, |source, before, after| {
                let empty = Snapshot::default();
                after
                    .unwrap_or(&empty)
                    .differences(before.unwrap_or(&empty), |offset, a, b| {
                        if a != b {
                            visit((AllocId(source as u32), offset as u64));
                        }
                    });
            });
    }
}

#[derive(Clone, Default)]
pub(super) struct Holders {
    expected: Snapshot<i128>,
    incoming: Snapshot<Incoming>,
    headers: Snapshot<Option<(u32, AllocState)>>,
    touched: BTreeSet<AllocId>,
    #[cfg(test)]
    count_comparisons: u64,
    #[cfg(test)]
    cell_checks: u64,
}

impl Holders {
    fn extend(&mut self, n: usize) {
        while self.expected.len() < n {
            self.expected.push(0);
            self.incoming.push(Incoming::default());
            self.headers.push(None);
        }
    }

    fn add(&mut self, ptr: Ptr, n: i128) {
        if n == 0 {
            return;
        }
        self.extend(ptr.alloc.0 as usize + 1);
        self.expected[ptr.alloc.0 as usize] += n;
        self.touched.insert(ptr.alloc);
    }

    pub(super) fn value(&mut self, val: &Val, credits: i64) {
        if credits == 0 {
            return;
        }
        match val {
            Val::Ptr(p) | Val::Callable { env: Some(p), .. } => self.add(*p, i128::from(credits)),
            Val::Image(e) => {
                for cell in e.cells.values() {
                    if let Cell::Data {
                        ptr, counted: true, ..
                    } = cell
                    {
                        self.add(*ptr, i128::from(credits));
                    }
                }
            }
            _ => {}
        }
    }

    /// Apply only physical cell edits and index lifetime/generation changes.
    pub(super) fn sync(&mut self, ledger: &mut Ledger) -> BTreeSet<Position> {
        self.extend(ledger.allocations.len());
        let mut changed = std::mem::take(&mut ledger.changed);
        let mut candidates = BTreeSet::new();
        for id in &changed {
            let a = std::sync::Arc::make_mut(&mut ledger.allocations[id.0 as usize]);
            a.audit_dirty = false;
            let edits = a
                .extent
                .holder_edits
                .as_mut()
                .expect("allocation extent journals writes");
            for edit in edits.drain(..) {
                let pos = (*id, edit.offset);
                if let Some(old) = edit.old {
                    self.add(old, -1);
                    self.incoming[old.alloc.0 as usize].remove(&pos);
                }
                if let Some(new) = edit.new {
                    self.add(new, 1);
                    self.incoming[new.alloc.0 as usize].insert(pos, new);
                    candidates.insert(pos);
                }
            }
            if a.owner == Owner::Heap {
                self.touched.insert(*id);
            }
        }
        for &id in &changed {
            let a = ledger.get(id);
            let header = (a.generation, a.state);
            if self.headers[id.0 as usize] != Some(header) {
                candidates.extend(self.incoming[id.0 as usize].keys());
                self.headers[id.0 as usize] = Some(header);
            }
        }
        changed.clear();
        ledger.changed = changed;
        candidates
    }

    /// Check changed counts and pointers, in the original recount order.
    pub(super) fn check(&mut self, ledger: &mut Ledger) -> Checked<()> {
        let candidates = self.sync(ledger);
        for (source, offset) in candidates {
            let a = ledger.get(source);
            if a.state == AllocState::Dead {
                continue;
            }
            if let Some(Cell::Data {
                ptr, counted: true, ..
            }) = a.extent.cells.get(&offset)
            {
                #[cfg(test)]
                {
                    self.cell_checks += 1;
                }
                let target = ledger.get(ptr.alloc);
                if matches!(target.state, AllocState::Dead | AllocState::Raw(_))
                    || target.generation != ptr.generation
                {
                    return fail(
                        V::UseAfterFree,
                        format!(
                            "{source:?}+{offset} holds a credit of freed {:?}",
                            ptr.alloc
                        ),
                    );
                }
            }
        }
        for id in std::mem::take(&mut self.touched) {
            let a = ledger.get(id);
            if a.owner != Owner::Heap || matches!(a.state, AllocState::Dead | AllocState::Raw(_)) {
                continue;
            }
            #[cfg(test)]
            {
                self.count_comparisons += 1;
            }
            let ideal = self.expected[id.0 as usize];
            if let Some(Count::Finite(n)) = a.count
                && i128::from(n) != ideal
            {
                return fail(
                    V::CountMismatch,
                    format!("{id:?} has count {n} but {ideal} holders"),
                );
            }
        }
        Ok(())
    }
}

/// Region escape checks include borrowed cells as well as counted holders.
/// Retain their physical reverse index independently of the holder journal,
/// including the proof for dead storage inherited by a failure fork.
#[derive(Clone, Default)]
pub(super) struct References {
    allocations: Snapshot<Arc<crate::ledger::Allocation>>,
    incoming: Snapshot<Incoming>,
    #[cfg(test)]
    visited: u64,
}

impl References {
    fn refresh(&mut self, ledger: &Ledger) {
        let mut changed = Vec::new();
        ledger
            .allocations
            .differences(&self.allocations, |i, a, b| {
                if !matches!((a,b), (Some(a),Some(b)) if Arc::ptr_eq(a,b)) {
                    changed.push(i);
                }
            });
        fork::scanned(changed.len());
        #[cfg(test)]
        {
            self.visited += changed.len() as u64;
        }
        for i in changed {
            let source = AllocId(i as u32);
            let old = self
                .allocations
                .get(i)
                .filter(|a| a.state != AllocState::Dead)
                .map(|a| &a.extent.cells);
            let a = ledger.get(source);
            let new = (a.state != AllocState::Dead).then_some(&a.extent.cells);
            if let Some(old) = old {
                for (&offset, cell) in old {
                    let Cell::Data { ptr, .. } = cell else {
                        continue;
                    };
                    if new.and_then(|c| c.get(&offset)) == Some(cell) {
                        continue;
                    }
                    let target = ptr.alloc.0 as usize;
                    if let Some(incoming) = self.incoming.get_mut(target) {
                        incoming.remove(&(source, offset));
                        if incoming.sources.is_empty() {
                            self.incoming.remove(target);
                        }
                    }
                }
            }
            if let Some(new) = new {
                for (&offset, cell) in new {
                    let Cell::Data { ptr, .. } = cell else {
                        continue;
                    };
                    if old.and_then(|c| c.get(&offset)) == Some(cell) {
                        continue;
                    }
                    let target = ptr.alloc.0 as usize;
                    if self.incoming.get(target).is_none() {
                        self.incoming.set(target, Incoming::default());
                    }
                    self.incoming[target].insert((source, offset), *ptr);
                }
            }
        }
        self.allocations = ledger.allocations.clone();
    }

    pub(super) fn escaping(
        &mut self,
        ledger: &Ledger,
        inside: &Snapshot<AllocId>,
    ) -> Option<(AllocId, u64, Ptr)> {
        self.refresh(ledger);
        let inside: BTreeSet<_> = inside.iter().copied().collect();
        let mut first = None;
        for id in &inside {
            let Some(incoming) = self.incoming.get(id.0 as usize) else {
                continue;
            };
            for ((source, offset), ptr) in incoming.entries() {
                #[cfg(test)]
                {
                    self.visited += 1;
                }
                let a = ledger.get(source);
                if a.state == AllocState::Dead
                    || matches!(a.owner, Owner::Frame | Owner::Temporary)
                    || inside.contains(&source)
                {
                    continue;
                }
                // The full scan reports source allocation, then byte offset,
                // regardless of the order of the region's target allocations.
                if first.is_none_or(|(s, o, _)| (source, offset) < (s, o)) {
                    first = Some((source, offset, ptr));
                }
            }
        }
        first
    }
}

/// A full physical audit with a retained proof for unchanged tree branches.
/// Unlike the fast tally, it reads storage itself, never holder-edit journals.
#[derive(Clone, Default)]
pub(super) struct Physical {
    pub(super) references: References,
    allocations: Snapshot<Arc<crate::ledger::Allocation>>,
    tokens: Snapshot<crate::ledger::Token>,
    resources: Snapshot<crate::ledger::Resource>,
    regions: Snapshot<crate::ledger::Region>,
    tally: Holders,
    last_expected: Snapshot<i128>,
    last_incoming: Snapshot<Incoming>,
    held: BTreeMap<AllocId, i128>,
    frames: Vec<Snapshot<Option<Held>>>,
    forgotten: Snapshot<Held>,
    pub(super) terminal_allocations: Snapshot<()>,
    pub(super) open_handles: Snapshot<(), 15>,
    handles: Snapshot<(crate::ledger::HandleState, bool), 15>,
    exempt_allocations: Snapshot<()>,
    exemptions_seen: usize,
    pub(super) pending_tokens: Snapshot<()>,
    pub(super) open_resources: Snapshot<()>,
    pub(super) live_regions: Snapshot<()>,
    bad_counts: BTreeSet<AllocId>,
    bad_tallies: BTreeSet<AllocId>,
    bad_tally_shape: bool,
    bad_index_shape: bool,
    bad_indexes: BTreeSet<(AllocId, Position)>,
    bad_cells: BTreeMap<Position, Ptr>,
}

impl Physical {
    #[allow(clippy::too_many_arguments)]
    fn refresh(
        &mut self,
        ledger: &Ledger,
        actual: &Holders,
        stack: &[Activity],
        roots: &BTreeMap<u64, Held>,
        forgotten: &Snapshot<Held>,
        finished: &Option<Finished>,
    ) {
        self.references.refresh(ledger);
        self.bad_tally_shape = actual.expected.len() != ledger.allocations.len();
        self.bad_index_shape = actual.incoming.len() != ledger.allocations.len();
        self.tally.extend(ledger.allocations.len());
        let mut changed = Vec::new();
        ledger
            .allocations
            .differences(&self.allocations, |i, old, new| {
                if !matches!((old,new), (Some(a),Some(b)) if Arc::ptr_eq(a,b)) {
                    changed.push(AllocId(i as u32));
                }
            });
        fork::scanned(changed.len());
        let mut targets = BTreeSet::new();
        let mut cells = BTreeSet::new();
        let mut index_cells = BTreeSet::new();
        // Remove and add physical cells from each changed allocation. This is
        // independent of both the mutation journal and RC instruction counts.
        for &id in &changed {
            targets.insert(id);
            let before = self.allocations.get(id.0 as usize);
            let after = ledger.get(id);
            let old_cells = before
                .filter(|a| a.state != AllocState::Dead)
                .map(|a| &a.extent.cells);
            let new_cells = (after.state != AllocState::Dead).then_some(&after.extent.cells);
            if let Some(old_cells) = old_cells {
                for (&offset, cell) in old_cells {
                    let Cell::Data {
                        ptr, counted: true, ..
                    } = cell
                    else {
                        continue;
                    };
                    if new_cells.and_then(|c| c.get(&offset)) == Some(cell) {
                        continue;
                    }
                    self.tally.add(*ptr, -1);
                    self.tally.incoming[ptr.alloc.0 as usize].remove(&(id, offset));
                    index_cells.insert((ptr.alloc, (id, offset)));
                    cells.insert((id, offset));
                }
            }
            if let Some(new_cells) = new_cells {
                for (&offset, cell) in new_cells {
                    let Cell::Data {
                        ptr, counted: true, ..
                    } = cell
                    else {
                        continue;
                    };
                    if old_cells.and_then(|c| c.get(&offset)) == Some(cell) {
                        continue;
                    }
                    self.tally.add(*ptr, 1);
                    self.tally.incoming[ptr.alloc.0 as usize].insert((id, offset), *ptr);
                    index_cells.insert((ptr.alloc, (id, offset)));
                    cells.insert((id, offset));
                }
            }
        }
        for &id in &changed {
            let before = self.allocations.get(id.0 as usize);
            let after = ledger.get(id);
            if before.is_none_or(|a| (a.generation, a.state) != (after.generation, after.state)) {
                cells.extend(self.tally.incoming[id.0 as usize].keys());
            }
        }
        // Compare physical register snapshots, independently of credit-write
        // hooks. A scalar register history is never scanned at every site.
        let frames: Vec<_> = stack
            .iter()
            .filter_map(|a| match a {
                Activity::Frame(f) => Some(f.values.clone()),
                _ => None,
            })
            .collect();
        let old_frames = std::mem::take(&mut self.frames);
        let mut physical_value = |h: &Held, sign: i128| {
            if h.credits == 0 {
                return;
            }
            let n = sign * i128::from(h.credits);
            match &h.val {
                Val::Ptr(p) | Val::Callable { env: Some(p), .. } => self.tally.add(*p, n),
                Val::Image(e) => {
                    for cell in e.cells.values() {
                        if let Cell::Data {
                            ptr, counted: true, ..
                        } = cell
                        {
                            self.tally.add(*ptr, n);
                        }
                    }
                }
                _ => {}
            }
        };
        for i in 0..frames.len().max(old_frames.len()) {
            let empty = Snapshot::default();
            let old = old_frames.get(i).unwrap_or(&empty);
            let new = frames.get(i).unwrap_or(&empty);
            new.differences(old, |_, a, b| {
                if let Some(Some(h)) = a {
                    physical_value(h, -1);
                }
                if let Some(Some(h)) = b {
                    physical_value(h, 1);
                }
            });
        }
        forgotten.differences(&self.forgotten, |_, a, b| {
            if let Some(h) = a {
                physical_value(h, -1);
            }
            if let Some(h) = b {
                physical_value(h, 1);
            }
        });
        self.frames = frames;
        self.forgotten = forgotten.clone();
        // Harness roots and a returned value are bounded by live cleanup work.
        let mut held = BTreeMap::<AllocId, i128>::new();
        let mut value = |h: &Held| {
            let mut add = |p: Ptr| {
                *held.entry(p.alloc).or_default() += i128::from(h.credits);
            };
            match &h.val {
                Val::Ptr(p) | Val::Callable { env: Some(p), .. } => add(*p),
                Val::Image(e) => {
                    for cell in e.cells.values() {
                        if let Cell::Data {
                            ptr, counted: true, ..
                        } = cell
                        {
                            add(*ptr);
                        }
                    }
                }
                _ => {}
            }
        };
        for h in roots.values() {
            value(h);
        }
        if let Some(Finished::Returned(Some((h, _, _)))) = finished {
            value(h);
        }
        for (&id, &n) in &self.held {
            let delta = held.get(&id).copied().unwrap_or(0) - n;
            if delta != 0 {
                self.tally.expected[id.0 as usize] += delta;
                targets.insert(id);
            }
        }
        for (&id, &n) in &held {
            if !self.held.contains_key(&id) && n != 0 {
                self.tally.expected[id.0 as usize] += n;
                targets.insert(id);
            }
        }
        self.held = held;
        targets.append(&mut self.tally.touched);
        actual.expected.differences(&self.last_expected, |i, a, b| {
            if a != b {
                targets.insert(AllocId(i as u32));
            }
        });
        actual.incoming.differences(&self.last_incoming, |i, a, b| {
            let empty = Incoming::default();
            b.unwrap_or(&empty)
                .differences(a.unwrap_or(&empty), |position| {
                    index_cells.insert((AllocId(i as u32), position));
                });
        });
        for id in targets {
            let i = id.0 as usize;
            if self.tally.expected.get(i) != actual.expected.get(i) {
                self.bad_tallies.insert(id);
            } else {
                self.bad_tallies.remove(&id);
            }
            let Some(a) = ledger.allocations.get(i) else {
                continue;
            };
            let mismatch = a.owner == Owner::Heap
                && !matches!(a.state, AllocState::Dead | AllocState::Raw(_))
                && matches!(a.count,Some(Count::Finite(n)) if i128::from(n)!=self.tally.expected[i]);
            if mismatch {
                self.bad_counts.insert(id);
            } else {
                self.bad_counts.remove(&id);
            }
        }
        for (target, position) in index_cells {
            let i = target.0 as usize;
            if self.tally.incoming.get(i).and_then(|v| v.get(&position))
                != actual.incoming.get(i).and_then(|v| v.get(&position))
            {
                self.bad_indexes.insert((target, position));
            } else {
                self.bad_indexes.remove(&(target, position));
            }
        }
        for position @ (id, offset) in cells {
            let a = ledger.get(id);
            let ptr = if a.state == AllocState::Dead {
                None
            } else {
                match a.extent.cells.get(&offset) {
                    Some(Cell::Data {
                        ptr, counted: true, ..
                    }) => Some(*ptr),
                    _ => None,
                }
            };
            if let Some(ptr) = ptr {
                let target = ledger.get(ptr.alloc);
                if matches!(target.state, AllocState::Dead | AllocState::Raw(_))
                    || target.generation != ptr.generation
                {
                    self.bad_cells.insert(position, ptr);
                    continue;
                }
            }
            self.bad_cells.remove(&position);
        }
        // These independent indexes retain every outstanding terminal obligation,
        // including one inherited unchanged by a fork and released by cleanup.
        ledger.tokens.differences(&self.tokens, |i, _, new| {
            if new.is_some_and(|t| !t.consumed) {
                self.pending_tokens.set(i, ());
            } else {
                self.pending_tokens.remove(i);
            }
        });
        ledger.resources.differences(&self.resources, |i, _, new| {
            if new.is_some_and(|r| r.state != ResourceState::Dead && !r.forgotten) {
                self.open_resources.set(i, ());
            } else {
                self.open_resources.remove(i);
            }
        });
        ledger.regions.differences(&self.regions, |i, _, new| {
            if new.is_some_and(|r| r.live) {
                self.live_regions.set(i, ());
            } else {
                self.live_regions.remove(i);
            }
        });
        ledger
            .handles
            .entries
            .differences(&self.handles, |i, _, new| {
                if new.is_some_and(|(state, context)| {
                    *state == crate::ledger::HandleState::Open && !context
                }) {
                    self.open_handles.set(i, ());
                } else {
                    self.open_handles.remove(i);
                }
            });
        self.handles = ledger.handles.entries.clone();
        let mut obligations: BTreeSet<_> = changed.into_iter().collect();
        for i in self.exemptions_seen..ledger.exemptions.len() {
            match ledger.exemptions.get(i).expect("append-only exemption") {
                Exemption::Static { alloc, .. } | Exemption::Overflow { alloc } => {
                    self.exempt_allocations.set(alloc.0 as usize, ());
                    obligations.insert(*alloc);
                }
                Exemption::Forget { allocations, .. } => {
                    for alloc in allocations.iter() {
                        self.exempt_allocations.set(alloc.0 as usize, ());
                        obligations.insert(*alloc);
                    }
                }
                Exemption::Context { .. } => {}
            }
        }
        self.exemptions_seen = ledger.exemptions.len();
        for id in obligations {
            let a = ledger.get(id);
            let i = id.0 as usize;
            if a.state != AllocState::Dead
                && (matches!(a.state, AllocState::Raw(_))
                    || self.exempt_allocations.get(i).is_none())
            {
                self.terminal_allocations.set(i, ());
            } else {
                self.terminal_allocations.remove(i);
            }
        }
        self.allocations = ledger.allocations.clone();
        self.tokens = ledger.tokens.clone();
        self.resources = ledger.resources.clone();
        self.regions = ledger.regions.clone();
        self.last_expected = actual.expected.clone();
        self.last_incoming = actual.incoming.clone();
    }

    fn check(&self, steps: u64) -> Checked<()> {
        if self.bad_tally_shape || !self.bad_tallies.is_empty() {
            return fail(
                V::InternalAudit,
                format!("physical-holder tally disagrees with full recount at step {steps}"),
            );
        }
        if self.bad_index_shape || !self.bad_indexes.is_empty() {
            return fail(
                V::InternalAudit,
                format!("incoming-holder index disagrees with full recount at step {steps}"),
            );
        }
        Ok(())
    }

    pub(super) fn counts(&self, ledger: &Ledger) -> Checked<()> {
        if let Some((&(source, offset), ptr)) = self.bad_cells.first_key_value() {
            return fail(
                V::UseAfterFree,
                format!(
                    "{source:?}+{offset} holds a credit of freed {:?}",
                    ptr.alloc
                ),
            );
        }
        if let Some(&id) = self.bad_counts.first() {
            let Some(Count::Finite(n)) = ledger.get(id).count else {
                unreachable!()
            };
            return fail(
                V::CountMismatch,
                format!(
                    "{id:?} has count {n} but {} holders",
                    self.tally.expected[id.0 as usize]
                ),
            );
        }
        Ok(())
    }
}

impl<S: Stage> Machine<'_, S> {
    pub(super) fn set_root(&mut self, id: u64, held: Option<Held>) {
        if let Some(old) = self.roots.remove(&id) {
            self.holders.value(&old.val, -i64::from(old.credits));
        }
        if let Some(held) = held {
            self.holders.value(&held.val, i64::from(held.credits));
            self.roots.insert(id, held);
        }
    }

    pub fn check_counts(&mut self) -> Checked<()> {
        let incremental = self.holders.check(&mut self.ledger);
        let due = std::mem::take(&mut self.full_recount_due);
        let full = self.config.full_recount_every_step
            || self.steps.is_power_of_two()
            || due
            || incremental.is_err();
        if full {
            // The old per-step oracle also determines the exact violation text
            // and order. Any difference is an H implementation fault.
            let independent = self.cross_check_holders();
            let reference = if self.config.full_recount_every_step {
                self.recount_counts()
            } else {
                self.physical.counts(&self.ledger)
            };
            if incremental != reference {
                return fail(
                    V::InternalAudit,
                    format!(
                        "incremental/recount disagreement at step {}: incremental {incremental:?}; recount {reference:?}",
                        self.steps
                    ),
                );
            }
            independent?;
        }
        incremental
    }

    /// Independently cross-check every holder, sharing the proof for unchanged
    /// physical storage. The audit does not consume the write journals.
    pub(super) fn refresh_holder_audit(&mut self) {
        self.physical.refresh(
            &self.ledger,
            &self.holders,
            &self.stack,
            &self.roots,
            &self.forgotten,
            &self.finished,
        );
    }
    pub(super) fn cross_check_holders(&mut self) -> Checked<()> {
        self.refresh_holder_audit();
        self.physical.check(self.steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Layouts;
    use crate::program::NoOperations;
    use hirsel_lir::layout_types::TargetSpec;
    use hirsel_lir::stage::Ready;

    #[test]
    fn region_escape_order_and_borrowed_twins_share_dead_history() {
        let mut work = Vec::new();
        for n in [128, 8192] {
            let mut ledger = Ledger::default();
            for _ in 0..n {
                let p = ledger.alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(0)));
                ledger.free(p.alloc).unwrap();
            }
            let first = ledger.alloc(Owner::Region(0), None, 8, 8, 8, Some(Count::Immortal));
            let second = ledger.alloc(Owner::Region(0), None, 8, 8, 8, Some(Count::Immortal));
            let inside: Snapshot<_> = [second.alloc, first.alloc].into_iter().collect();
            let frame = ledger.alloc(Owner::Frame, None, 8, 8, 8, None);
            let earlier = ledger.alloc(Owner::Heap, None, 16, 16, 8, Some(Count::Finite(1)));
            let later = ledger.alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(1)));
            for (source, offset, ptr) in [
                (frame.alloc, 0, first),
                (earlier.alloc, 0, second),
                (earlier.alloc, 8, first),
                (later.alloc, 0, first),
            ] {
                // Borrowed cells are absent from the counted-holder journal.
                ledger.get_mut(source).extent.cells.insert(
                    offset,
                    Cell::Data {
                        ptr,
                        counted: false,
                        borrow: None,
                    },
                );
            }
            let mut prefix = References::default();
            prefix.refresh(&ledger);
            let mut visited = 0;
            for fixed in [false, true] {
                let mut fork = ledger.clone();
                let mut references = prefix.clone();
                let before = references.visited;
                if fixed {
                    for source in [earlier.alloc, later.alloc] {
                        let size = fork.get(source).size;
                        fork.get_mut(source).extent.take(0, size, FREE_POISON, None);
                    }
                }
                let actual = references.escaping(&fork, &inside);
                // The original independent full-table region escape search.
                let expected = fork.allocations.indexed().find_map(|(i, a)| {
                    let source = AllocId(i as u32);
                    if a.state == AllocState::Dead
                        || matches!(a.owner, Owner::Frame | Owner::Temporary)
                        || inside.contains(&source)
                    {
                        return None;
                    }
                    a.extent
                        .cells
                        .iter()
                        .find_map(|(&offset, cell)| match cell {
                            Cell::Data { ptr, .. } if inside.contains(&ptr.alloc) => {
                                Some((source, offset, *ptr))
                            }
                            _ => None,
                        })
                });
                assert_eq!(actual, expected);
                if fixed {
                    assert!(actual.is_none(), "a frame's borrowed cell may remain");
                } else {
                    assert_eq!(actual, Some((earlier.alloc, 0, second)));
                    assert!(Arc::ptr_eq(
                        &ledger.allocations[earlier.alloc.0 as usize],
                        &fork.allocations[earlier.alloc.0 as usize]
                    ));
                }
                visited += references.visited - before;
            }
            work.push(visited);
        }
        assert_eq!(
            work[0], work[1],
            "region exits must not revisit dead storage"
        );
        assert!(work[0] <= 8, "{work:?}");
    }

    /// Hand-built executor programs. Each command is one audit boundary and
    /// selects either a count-corrupting command or its corrected twin.
    #[derive(Clone, Copy, Debug)]
    enum Mutation {
        MissingDup,
        ExtraRelease,
        OverwrittenCell,
        SilentlyDroppedHeld,
        StaleCell,
    }

    fn execute(mutation: Mutation, fixed: bool, full: bool) -> (u64, Checked<()>) {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let payload = layouts.scalar(ScalarKind::U64);
        let (target, layouts) = layouts.finish();
        let program = Program::<Ready> {
            target,
            layouts,
            units: vec![],
            statics: vec![],
        };
        let config = Config {
            full_recount_every_step: full,
            ..Config::default()
        };
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let p = m
            .ledger
            .alloc(Owner::Heap, Some(payload), 8, 8, 8, Some(Count::Finite(1)));
        let storage = m.ledger.alloc(Owner::Frame, None, 8, 8, 8, None);
        let held = |p| Held {
            val: Val::Ptr(p),
            credits: 1,
            borrow: None,
        };
        let cell = |p| Cell::Data {
            ptr: p,
            counted: true,
            borrow: None,
        };
        m.set_root(0, Some(held(p)));
        m.steps = 1;
        m.check_counts()
            .expect("initial allocation matches its holder");
        // The overwritten/stale controls start with the credit physically in
        // memory; the other controls start with a Held credit.
        m.steps = 2;
        if matches!(mutation, Mutation::OverwrittenCell | Mutation::StaleCell) {
            m.set_root(0, None);
            m.ledger
                .get_mut(storage.alloc)
                .extent
                .put_cell(0, 8, cell(p));
        }
        m.check_counts()
            .expect("holder transfer preserves the count");
        m.steps = 3;
        match mutation {
            Mutation::MissingDup => {
                m.set_root(1, Some(held(p)));
                if fixed {
                    m.dup(p).unwrap();
                }
            }
            Mutation::ExtraRelease => {
                m.count_down(p).unwrap();
                if fixed {
                    m.set_root(0, None);
                }
            }
            Mutation::OverwrittenCell => {
                m.ledger
                    .get_mut(storage.alloc)
                    .extent
                    .put_cell(0, 8, Cell::Code(None));
                if fixed {
                    m.count_down(p).unwrap();
                }
            }
            Mutation::SilentlyDroppedHeld => {
                m.set_root(0, None);
                if fixed {
                    m.count_down(p).unwrap();
                }
            }
            Mutation::StaleCell => {
                m.ledger.get_mut(p.alloc).generation += 1;
                if fixed {
                    m.ledger
                        .get_mut(storage.alloc)
                        .extent
                        .set_cell(0, cell(Ptr { generation: 1, ..p }));
                }
            }
        }
        (
            m.steps,
            if full {
                m.recount_counts()
            } else {
                m.check_counts()
            },
        )
    }

    #[test]
    fn five_count_corruptions_match_the_old_recount_at_the_exact_step() {
        for mutation in [
            Mutation::MissingDup,
            Mutation::ExtraRelease,
            Mutation::OverwrittenCell,
            Mutation::SilentlyDroppedHeld,
            Mutation::StaleCell,
        ] {
            for fixed in [false, true] {
                let incremental = execute(mutation, fixed, false);
                let reference = execute(mutation, fixed, true);
                assert_eq!(incremental, reference, "{mutation:?}, fixed={fixed}");
                assert_eq!(incremental.0, 3);
                if fixed {
                    incremental.1.unwrap();
                } else {
                    let expected = if matches!(mutation, Mutation::StaleCell) {
                        V::UseAfterFree
                    } else {
                        V::CountMismatch
                    };
                    assert_eq!(incremental.1.unwrap_err().kind, expected, "{mutation:?}");
                }
            }
        }
    }

    #[test]
    fn a_missed_physical_write_is_an_internal_fault_on_the_geometric_cross_check() {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let payload = layouts.scalar(ScalarKind::U64);
        let (target, layouts) = layouts.finish();
        let program = Program::<Ready> {
            target,
            layouts,
            units: vec![],
            statics: vec![],
        };
        let config = Config::default();
        let mut ops = NoOperations;
        let mut m = Machine::new(&program, &config, &mut ops);
        let p = m
            .ledger
            .alloc(Owner::Heap, Some(payload), 8, 8, 8, Some(Count::Finite(1)));
        let storage = m.ledger.alloc(Owner::Frame, None, 8, 8, 8, None);
        m.set_root(
            0,
            Some(Held {
                val: Val::Ptr(p),
                credits: 1,
                borrow: None,
            }),
        );
        m.steps = 1;
        m.check_counts().unwrap();
        // Deliberately bypass the write seam. The tally agrees with the count,
        // while the original recount finds an extra physical holder.
        m.ledger.get_mut(storage.alloc).extent.cells.insert(
            0,
            Cell::Data {
                ptr: p,
                counted: true,
                borrow: None,
            },
        );
        m.steps = 2;
        assert_eq!(m.check_counts().unwrap_err().kind, V::InternalAudit);
    }

    #[test]
    fn unchanged_holder_graph_size_does_not_increase_step_audit_work() {
        for n in [128, 4096] {
            let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
            let payload = layouts.scalar(ScalarKind::U64);
            let (target, layouts) = layouts.finish();
            let program = Program::<Ready> {
                target,
                layouts,
                units: vec![],
                statics: vec![],
            };
            let config = Config::default();
            let mut ops = NoOperations;
            let mut m = Machine::new(&program, &config, &mut ops);
            let storage = m.ledger.alloc(Owner::Frame, None, 8 * n, 8 * n, 8, None);
            let mut targets = Vec::new();
            for i in 0..n {
                let p = m
                    .ledger
                    .alloc(Owner::Heap, Some(payload), 8, 8, 8, Some(Count::Finite(1)));
                m.ledger.get_mut(storage.alloc).extent.put_cell(
                    8 * i,
                    8,
                    Cell::Data {
                        ptr: p,
                        counted: true,
                        borrow: None,
                    },
                );
                targets.push(p);
            }
            m.steps = 1;
            m.check_counts().unwrap();
            let counts = m.holders.count_comparisons;
            let cells = m.holders.cell_checks;
            // Overwriting one holder and releasing that target checks one
            // count, irrespective of the other n-1 unchanged cells/targets.
            m.ledger
                .get_mut(storage.alloc)
                .extent
                .put_cell(0, 8, Cell::Code(None));
            m.count_down(targets[0]).unwrap();
            m.steps = 3;
            m.check_counts().unwrap();
            assert_eq!(m.holders.count_comparisons - counts, 1);
            assert_eq!(m.holders.cell_checks - cells, 0);
            // A generation change must find an old cell in unchanged storage.
            let cells = m.holders.cell_checks;
            m.ledger.get_mut(targets[1].alloc).generation += 1;
            m.steps = 5;
            assert_eq!(m.check_counts().unwrap_err().kind, V::UseAfterFree);
            assert_eq!(m.holders.cell_checks - cells, 1);
        }
    }
}
