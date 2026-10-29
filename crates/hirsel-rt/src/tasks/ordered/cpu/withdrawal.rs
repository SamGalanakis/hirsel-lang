//! A saved publisher lends its prefix to the backing-pressure owner temporarily.
use super::*;
use crate::tasks::ordered::ahead::cpu_handoff::{SUMMARY_PUBLISHED, SUMMARY_WITHDRAWN, Summary};
use std::collections::BTreeSet;

pub(in crate::tasks::ordered) struct Withdrawn {
    task: TaskId,
    ordinal: usize,
    summary: *mut Summary,
    value: Vec<u8>,
    charges: Stretch,
    complete: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Header;
    use crate::budget::stretch::{Budgets, Event, summarise};
    use crate::tasks::ordered::ahead::cpu_handoff::{SUMMARY_RETIRED, VERSION};

    #[test]
    fn withdrawn_cpu_prefixes_restore_exact_results_and_charge_once() {
        for width in [1, 4, 8, 16] {
            let budgets = Budgets {
                fuel: true,
                memory: true,
            };
            let grant = hirsel_run_schema::CpuGrant {
                provider: "cpu.scoped@1".into(),
                max_workers: 16,
                max_domains: 1,
                max_physical_bytes: 2 << 30,
                max_batch_bytes: 100,
            };
            let mut state = Settled {
                level: 0,
                limit: 100,
                fuel: 20,
            };
            let initial = state;
            let head = (3 << 32) | 7;
            let younger = (5 << 32) | 9;
            let mut batch = Batch::new(&grant, width, 2, state);
            batch.claim_for_test(head, width, 0);
            batch.claim_for_test(younger, width, 1);
            let value = 42u64;
            let roots = [std::ptr::dangling_mut::<Header>()];
            let mut summary = Summary {
                version: VERSION,
                phase: SUMMARY_PUBLISHED,
                ordinal: 1,
                value: (&raw const value).cast(),
                value_bytes: 8,
                descriptor: 0,
                retained_count: 1,
                retained: roots.as_ptr(),
            };
            let charges = summarise(budgets, &[Event::Fuel, Event::Alloc(60)]).unwrap();
            batch
                .publish(
                    younger,
                    charges.clone(),
                    &raw mut summary,
                    value.to_le_bytes().to_vec(),
                )
                .unwrap();
            for _ in 0..3 {
                let withdrawn = batch.withdraw(head, &BTreeSet::from([younger]));
                assert_eq!(withdrawn.len(), 1);
                assert_eq!(withdrawn[0].charges, charges);
                assert_eq!(withdrawn[0].value, value.to_le_bytes());
                assert_eq!(summary.phase, SUMMARY_WITHDRAWN);
                assert!(batch.records.iter().all(|row| row.summary.is_none()));
                assert!(batch.retire_summary(younger, &mut state).is_none());
                assert!(batch.withdraw(head, &BTreeSet::from([younger])).is_empty());
                assert_eq!(state, initial, "withdrawal cannot settle charges");
                assert_eq!(summary.value, (&raw const value).cast());
                assert_eq!(summary.retained, roots.as_ptr());
                batch.restore(withdrawn);
                assert_eq!(summary.phase, SUMMARY_PUBLISHED);
                assert_eq!(
                    batch.records[1].summary.as_ref().unwrap().0,
                    &raw mut summary
                );
                assert_eq!(
                    batch.records[1].summary.as_ref().unwrap().1,
                    value.to_le_bytes()
                );
                assert_eq!(state, initial, "restoration cannot settle charges");
            }
            assert_eq!(
                batch.settle(head, &Stretch::new(budgets), &mut state, true),
                Some(Ok(()))
            );
            let retired = batch.retire_summary(younger, &mut state).unwrap();
            assert_eq!(retired.result, Ok(()));
            assert_eq!(retired.charges, charges);
            assert_eq!(retired.record.unwrap().value, value.to_le_bytes());
            assert_eq!(summary.phase, SUMMARY_RETIRED);
            assert_eq!(
                state,
                Settled {
                    level: 60,
                    limit: 100,
                    fuel: 19
                },
                "restored prefixes charge once"
            );
            assert!(batch.retire_summary(younger, &mut state).is_none());
        }
    }
}

impl Batch {
    /// Called only after the dispatcher acknowledges every younger physical
    /// owner. The saved frame continues owning the result and its root table.
    pub(in crate::tasks::ordered) fn withdraw(
        &mut self,
        head: TaskId,
        targets: &BTreeSet<TaskId>,
    ) -> Vec<Withdrawn> {
        for &task in targets {
            self.materialize(task);
        }
        let head = self.ordinal(head).expect("live head CPU job");
        assert!(
            self.records[head + 1..]
                .iter()
                .all(|row| row.summary.is_none() || targets.contains(&row.task.unwrap()))
        );
        self.jobs
            .withdraw_younger(head)
            .into_iter()
            .map(|(ordinal, charges, complete)| {
                let row = &mut self.records[ordinal];
                let task = row.task.unwrap();
                let (summary, value) = row.summary.take().unwrap();
                // SAFETY: all publisher stacks are acknowledged and blocked
                // from selection until restore returns their prefixes.
                unsafe {
                    assert_eq!((*summary).phase, SUMMARY_PUBLISHED);
                    (*summary).phase = SUMMARY_WITHDRAWN;
                }
                Withdrawn {
                    task,
                    ordinal,
                    summary,
                    value,
                    charges,
                    complete,
                }
            })
            .collect()
    }

    /// Transfer each existing prefix back once. No allocation or fuel event is
    /// repeated, and no pointer or charge is reconstructed from the mapper.
    pub(in crate::tasks::ordered) fn restore(&mut self, withdrawn: Vec<Withdrawn>) {
        for row in withdrawn {
            assert_eq!(self.ordinal(row.task), Some(row.ordinal));
            assert!(
                self.records[row.ordinal]
                    .summary
                    .replace((row.summary, row.value))
                    .is_none()
            );
            if row.complete {
                self.jobs.publish(row.ordinal, row.charges);
            } else {
                self.jobs.pause(row.ordinal, row.charges);
            }
            // SAFETY: the same acknowledged publisher still owns this record.
            unsafe {
                assert_eq!((*row.summary).phase, SUMMARY_WITHDRAWN);
                (*row.summary).phase = SUMMARY_PUBLISHED;
            }
        }
    }
}
