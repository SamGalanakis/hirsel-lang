//! Optional exact charge witnesses for canonical traces (owner ruling 618475).
//! Untraced execution retains only bounded Stretch summaries. Like stretch::reference,
//! this observer interprets charges in order; a rejected batch contributes only
//! the first failing charge, never its speculative suffix.
use super::stretch::{Budgets, Piece, Settled, Stretch};
use crate::ordered::footprint::TaskId;
use hirsel_run_schema::BudgetKind;
use std::collections::BTreeMap;

mod charge;
pub(crate) use charge::Charge;

/// Owner-local journal: meter callbacks hold a heap borrow and cannot reach a
/// scheduler. Read only the task's compiled fuel word and this disjoint field.
#[derive(Default)]
pub(crate) struct Local {
    journal: Option<Box<Journal>>,
}
#[derive(Default)]
struct Journal {
    fuel: i64,
    charges: Vec<Charge>,
}
impl Local {
    pub(crate) fn enable(&mut self) {
        self.journal.get_or_insert_default();
    }
    pub(crate) fn grant(&mut self, fuel: i64) {
        if let Some(journal) = &mut self.journal {
            journal.fuel = fuel;
        }
    }
    pub(crate) fn capture(&mut self, word: i64) {
        if let Some(journal) = &mut self.journal {
            let points = journal.fuel.saturating_sub(word).max(0) as u64;
            if points != 0 {
                journal.charges.push(Charge::Fuel(points));
            }
            journal.fuel = word.max(0);
        }
    }
    pub(crate) fn allocation(&mut self, bytes: u64) {
        if let Some(journal) = &mut self.journal {
            journal.charges.push(Charge::Alloc(bytes));
        }
    }
    pub(crate) fn free(&mut self, word: i64, bytes: u64) {
        self.capture(word);
        if let Some(journal) = &mut self.journal {
            journal.charges.push(Charge::Free(bytes));
        }
    }
    pub(crate) fn take(&mut self) -> Option<Vec<Charge>> {
        self.journal
            .as_mut()
            .map(|journal| std::mem::take(&mut journal.charges))
    }
    /// The committing callback reports these same logical deaths as receipt
    /// credit. They belong to that op, rather than the owner's next segment.
    pub(crate) fn committed_credit(&mut self, mut bytes: u64) {
        let Some(journal) = &mut self.journal else {
            return;
        };
        while bytes != 0 {
            let Some(Charge::Free(freed)) = journal.charges.pop() else {
                panic!("commit credit must end in its actual frees");
            };
            bytes = bytes.checked_sub(freed).expect("exact commit credit");
        }
    }
}

pub(crate) struct Observer {
    state: Settled,
    budgets: Budgets,
    pending: BTreeMap<TaskId, Vec<Charge>>,
    pub(crate) stopped: Option<BudgetKind>,
}
impl Observer {
    pub(crate) fn new(state: Settled, budgets: Budgets) -> Self {
        Self {
            state,
            budgets,
            pending: BTreeMap::new(),
            stopped: None,
        }
    }
    pub(crate) fn settle(&mut self, task: TaskId, stretch: &Stretch) -> Result<(), BudgetKind> {
        assert!(self.stopped.is_none(), "no charge after the selected cut");
        let pending = self.pending.entry(task).or_default();
        for &charge in &stretch.exact {
            let failure = match charge {
                Charge::Fuel(points) if points > self.state.fuel => {
                    append(pending, Charge::Fuel(self.state.fuel + 1));
                    self.state.fuel = 0;
                    Some(BudgetKind::Fuel)
                }
                Charge::Alloc(bytes)
                    if self.state.level.saturating_add(bytes) > self.state.limit =>
                {
                    append(pending, charge);
                    Some(BudgetKind::Memory)
                }
                _ => {
                    append(pending, charge);
                    match charge {
                        Charge::Fuel(points) if self.state.fuel != u64::MAX => {
                            self.state.fuel -= points
                        }
                        Charge::Alloc(bytes) => {
                            self.state.level = self.state.level.saturating_add(bytes)
                        }
                        Charge::Free(bytes) | Charge::Credit(bytes) => {
                            self.state.level = self.state.level.saturating_sub(bytes)
                        }
                        Charge::Fuel(_) => {}
                    }
                    None
                }
            };
            if let Some(kind) = failure {
                self.stopped = Some(kind);
                return Err(kind);
            }
        }
        Ok(())
    }
    pub(crate) fn settle_with_limit(
        &mut self,
        task: TaskId,
        stretch: &Stretch,
        limit: u64,
    ) -> Result<(), BudgetKind> {
        let run_limit = self.state.limit;
        self.state.limit = run_limit.min(limit);
        let result = self.settle(task, stretch);
        self.state.limit = run_limit;
        result
    }
    pub(crate) fn credit(&mut self, bytes: u64) {
        self.state.level = self.state.level.saturating_sub(bytes);
    }
    pub(crate) fn take(&mut self, task: TaskId) -> Stretch {
        summary(self.budgets, self.pending.remove(&task).unwrap_or_default())
    }
}

fn append(charges: &mut Vec<Charge>, charge: Charge) {
    if let (Some(Charge::Fuel(previous)), Charge::Fuel(points)) = (charges.last_mut(), charge) {
        *previous = previous.saturating_add(points);
    } else {
        charges.push(charge);
    }
}

/// An unbounded trace summary, built from the exact accepted prefix. Unlike
/// production, it can span several head settlements and envelope pauses.
fn summary(budgets: Budgets, charges: Vec<Charge>) -> Stretch {
    let mut stretch = Stretch::new(budgets);
    for &charge in &charges {
        match charge {
            Charge::Fuel(points) => {
                stretch.piece().fuel = stretch.piece().fuel.saturating_add(points)
            }
            Charge::Alloc(bytes) => {
                let piece = stretch.piece();
                piece.level = piece.level.saturating_add(bytes as i64);
                piece.net = piece.level;
                if budgets.memory && piece.level > piece.envelope.last().map_or(0, |&(_, h)| h) {
                    let u = if budgets.fuel { piece.fuel } else { 0 };
                    let high = piece.level;
                    match piece.envelope.last_mut() {
                        Some(last) if last.0 == u => *last = (u, high),
                        _ => piece.envelope.push((u, high)),
                    }
                }
            }
            Charge::Free(bytes) => {
                let piece = stretch.piece();
                piece.level = piece.level.saturating_sub(bytes as i64);
                piece.net = piece.level;
            }
            Charge::Credit(bytes) => stretch.pieces.push((bytes, Piece::default())),
        }
    }
    stretch.exact = charges;
    stretch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untraced_local_charges_keep_no_journal_or_buffer() {
        let mut local = Local::default();
        for _ in 0..128 {
            local.grant(64);
            local.capture(63);
            local.allocation(32);
            local.free(62, 32);
            local.committed_credit(32);
            assert!(local.take().is_none());
            assert!(local.journal.is_none());
        }
        local.enable();
        local.grant(64);
        local.capture(63);
        local.allocation(32);
        local.free(62, 32);
        local.committed_credit(32);
        assert_eq!(
            local.take(),
            Some(vec![Charge::Fuel(1), Charge::Alloc(32), Charge::Fuel(1)])
        );
        assert_eq!(local.take(), Some(vec![]));
    }

    use crate::budget::stretch::{Event, summarise};
    #[test]
    fn exact_witness_truncates_coalesced_highs_and_fuel_batches_after_first_stop() {
        for budgets in [
            Budgets {
                fuel: false,
                memory: true,
            },
            Budgets {
                fuel: true,
                memory: true,
            },
        ] {
            let state = Settled {
                level: 90,
                limit: 100,
                fuel: 1,
            };
            let serial = summarise(budgets, &[Event::Alloc(20)]).unwrap();
            let speculative = summarise(
                budgets,
                &[
                    Event::Alloc(20),
                    Event::Free(20),
                    Event::Alloc(80),
                    Event::Fuel,
                    Event::Fuel,
                    Event::Free(80),
                ],
            )
            .unwrap();
            assert_eq!(speculative.pieces().next().unwrap().1.envelope[0], (0, 80));
            let mut one = Observer::new(state, budgets);
            let mut wide = Observer::new(state, budgets);
            assert_eq!(one.settle(7, &serial), Err(BudgetKind::Memory));
            assert_eq!(wide.settle(7, &speculative), Err(BudgetKind::Memory));
            assert_eq!(one.take(7), wide.take(7));
        }
        let budgets = Budgets {
            fuel: true,
            memory: true,
        };
        let state = Settled {
            level: 90,
            limit: 100,
            fuel: 1,
        };
        let mut long = Stretch::new(budgets);
        long.fuel_points(1000);
        long.alloc(80).unwrap();
        let mut trace = Observer::new(state, budgets);
        assert_eq!(trace.settle(0, &long), Err(BudgetKind::Fuel));
        assert_eq!(trace.take(0).exact, [Charge::Fuel(2)]);
    }

    #[test]
    fn exact_witness_accumulates_head_settlements_without_inventing_boundaries() {
        let budgets = Budgets {
            fuel: true,
            memory: true,
        };
        let state = Settled {
            level: 50,
            limit: 100,
            fuel: 8,
        };
        let events = [
            Event::Fuel,
            Event::Alloc(20),
            Event::Free(10),
            Event::Fuel,
            Event::Alloc(50),
            Event::Free(50),
            Event::Fuel,
        ];
        let mut serial = Observer::new(state, budgets);
        for event in &events[..5] {
            let result = serial.settle(3, &summarise(budgets, &[*event]).unwrap());
            if result.is_err() {
                break;
            }
        }
        let mut wide = Observer::new(state, budgets);
        assert_eq!(
            wide.settle(3, &summarise(budgets, &events).unwrap()),
            Err(BudgetKind::Memory)
        );
        assert_eq!(serial.take(3), wide.take(3));
    }
}
