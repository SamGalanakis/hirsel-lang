//! Independent physical-holder tally. RC instructions never update it.
//! Allocation cells journal their writes; SSA bindings journal credit writes.
//! Incoming-cell positions let a lifetime/generation change find old pointers
//! without scanning unrelated storage. Ordered candidates preserve recount order.
use super::*;
use std::collections::BTreeSet;

type Position = (AllocId, u64);

#[derive(Clone, Default)]
pub(super) struct Holders {
    expected: Vec<i128>,
    incoming: Vec<BTreeMap<Position, Ptr>>,
    headers: Vec<Option<(u32, AllocState)>>,
    touched: BTreeSet<AllocId>,
    #[cfg(test)]
    count_comparisons: u64,
    #[cfg(test)]
    cell_checks: u64,
}

impl Holders {
    fn extend(&mut self, n: usize) {
        self.expected.resize(n.max(self.expected.len()), 0);
        self.incoming
            .resize_with(n.max(self.incoming.len()), BTreeMap::new);
        self.headers.resize(n.max(self.headers.len()), None);
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
                candidates.extend(self.incoming[id.0 as usize].keys().copied());
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
            let reference = self.recount_counts();
            if incremental != reference {
                return fail(
                    V::InternalAudit,
                    format!(
                        "incremental/recount disagreement at step {}: incremental {incremental:?}; recount {reference:?}",
                        self.steps
                    ),
                );
            }
            self.cross_check_holders()?;
        }
        incremental
    }

    /// Independently recount the tally and its reverse index, without ledger
    /// counts as an input. Terminal and scheduled checks catch missed hooks.
    pub(super) fn cross_check_holders(&self) -> Checked<()> {
        let holders = self.recount_holders(false)?;
        if holders
            .iter()
            .map(|n| i128::from(*n))
            .ne(self.holders.expected.iter().copied())
        {
            return fail(
                V::InternalAudit,
                format!(
                    "physical-holder tally disagrees with full recount at step {}",
                    self.steps
                ),
            );
        }
        let mut incoming = vec![BTreeMap::new(); self.ledger.allocations.len()];
        for (i, a) in self.ledger.allocations.iter().enumerate() {
            if a.state == AllocState::Dead {
                continue;
            }
            for (&offset, cell) in &a.extent.cells {
                if let Cell::Data {
                    ptr, counted: true, ..
                } = cell
                {
                    incoming[ptr.alloc.0 as usize].insert((AllocId(i as u32), offset), *ptr);
                }
            }
        }
        if incoming != self.holders.incoming {
            return fail(
                V::InternalAudit,
                format!(
                    "incoming-holder index disagrees with full recount at step {}",
                    self.steps
                ),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Layouts;
    use crate::program::NoOperations;
    use hirsel_lir::layout_types::TargetSpec;
    use hirsel_lir::stage::Ready;

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
