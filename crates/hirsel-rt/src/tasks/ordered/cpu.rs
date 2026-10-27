//! Scoped CPU receipts retire through the existing Ordered head and Yield.
use super::{engine, events};
use crate::TaskCtx;
use crate::budget::stretch::{Jobs, Settled, Stretch};
use crate::ordered::Next;
use crate::ordered::footprint::TaskId;
use crate::stop::{Stop, Stopped};
use std::collections::BTreeMap;

/// The canonical claim binds an ordinal to a generation-qualified task, not
/// to a physical worker. Its ordinary task-local Stretch follows saves and
/// migration; only this batch may retire that prefix through Jobs.
pub(super) struct Batch {
    width: u32,
    limit: u64,
    claims: Vec<Option<TaskId>>,
    publisher: Option<TaskId>,
    owners: BTreeMap<TaskId, usize>,
    jobs: Jobs,
    retirement: u64,
    pending: BTreeMap<TaskId, (u32, u64)>,
    summaries: BTreeMap<TaskId, *mut super::ahead::cpu_handoff::Summary>,
}

pub(super) struct RetiredSummary {
    pub charges: Stretch,
    pub result: Result<(), hirsel_run_schema::BudgetKind>,
    pub record: crate::io::CpuSummary,
}

impl Batch {
    fn new(grant: &hirsel_run_schema::CpuGrant, width: u32, count: u64, state: Settled) -> Self {
        let count = usize::try_from(count).expect("bounded CPU count");
        let jobs = Jobs::cpu(grant, width, count, state);
        Self {
            width,
            limit: grant.max_batch_bytes,
            claims: vec![None; count],
            publisher: None,
            owners: BTreeMap::new(),
            jobs,
            retirement: 0,
            pending: BTreeMap::new(),
            summaries: BTreeMap::new(),
        }
    }

    fn claim(&mut self, task: TaskId, width: u32, ordinal: u64) {
        assert_eq!(width, self.width, "claim uses its admitted width");
        let ordinal = usize::try_from(ordinal).expect("bounded CPU ordinal");
        let claim = self
            .claims
            .get_mut(ordinal)
            .expect("claim belongs to batch");
        assert!(claim.replace(task).is_none(), "one claim per CPU job");
        assert!(
            self.owners.insert(task, ordinal).is_none(),
            "one job per task"
        );
    }

    pub(super) fn pending(&self, parent: TaskId, width: u32, ordinal: u64) -> bool {
        self.pending.get(&parent) == Some(&(width, ordinal))
    }

    pub(super) fn bind(&mut self, parent: TaskId, child: TaskId) -> bool {
        if let Some((width, ordinal)) = self.pending.remove(&parent) {
            assert!(self.publisher.is_none_or(|owner| owner == parent));
            self.publisher = Some(parent);
            self.claim(child, width, ordinal);
            true
        } else {
            false
        }
    }

    /// Keep the publisher runnable until its bounded children are installed.
    /// Starting an early child here can monopolize its publisher's CPU and
    /// scheduler role before the remaining workers have any work to claim.
    pub(super) fn publishing(&self, task: TaskId, head: TaskId) -> bool {
        self.publisher == Some(head) && self.owners.contains_key(&task)
    }

    pub(super) fn limit(&self, task: TaskId) -> Option<u64> {
        self.owners.contains_key(&task).then_some(self.limit)
    }

    pub(super) fn block_owner(
        &self,
        task: TaskId,
    ) -> Option<crate::pool::cpu_scoped::ledger::blocks::Owner> {
        self.owners
            .get(&task)
            .map(|&ordinal| crate::pool::cpu_scoped::ledger::blocks::Owner {
                task,
                ordinal: ordinal as u64,
            })
    }

    pub(super) fn owns(&self, task: TaskId, ordinal: u64) -> bool {
        self.owners.get(&task).copied().map(|n| n as u64) == Some(ordinal)
    }

    /// The saved call owns this value and its retained roots until retirement.
    /// Publishing transfers its Stretch, so a physical retry cannot append it
    /// again or mistake it for settled budget.
    pub(super) fn publish(
        &mut self,
        task: TaskId,
        before: Stretch,
        summary: *mut super::ahead::cpu_handoff::Summary,
    ) {
        let job = self.owners[&task];
        assert!(self.summaries.insert(task, summary).is_none());
        self.jobs.pause(job, before);
    }

    pub(super) fn retire_summary(
        &mut self,
        task: TaskId,
        state: &mut Settled,
    ) -> Option<RetiredSummary> {
        let &summary = self.summaries.get(&task)?;
        let (charges, result) = self.jobs.retire_published_at(self.owners[&task], state);
        if result.is_ok() {
            self.summaries.remove(&task);
            // SAFETY: publication borrows a saved mapper's exclusive record.
            // Only its logical head retires it, before returning to that frame.
            unsafe { (*summary).phase = super::ahead::cpu_handoff::SUMMARY_RETIRED };
        }
        let settled = self.jobs.settled();
        let record = crate::io::CpuSummary {
            version: 1,
            retirement: self.retirement,
            ordinal: self.owners[&task] as u64,
            // SAFETY: the mapper's saved frame still owns the borrowed record.
            // Do not record its addresses or potentially padded value carrier.
            descriptor: unsafe { (*summary).descriptor },
            value_bytes: unsafe { (*summary).value_bytes },
            retained_count: unsafe { (*summary).retained_count },
            pieces: crate::io::CpuSummary::pieces(&charges),
            level: settled.level,
            limit: settled.limit,
            fuel: settled.fuel,
            stop: result.as_ref().err().map(|stop| stop.kind),
        };
        self.retirement = self
            .retirement
            .checked_add(1)
            .expect("CPU retirement overflow");
        Some(RetiredSummary {
            charges,
            result: result.map_err(|stop| stop.kind),
            record,
        })
    }

    pub(super) fn settle(
        &mut self,
        task: TaskId,
        before: &Stretch,
        state: &mut Settled,
        complete: bool,
    ) -> Option<Result<(), hirsel_run_schema::BudgetKind>> {
        let &job = self.owners.get(&task)?;
        let result = self
            .jobs
            .retire_prefix_at(job, before.clone(), complete, state)
            .map_err(|stop| stop.kind);
        if complete && result.is_ok() {
            self.owners.remove(&task);
        }
        Some(result)
    }
}

/// Mapper/input cleanup precedes TaskFinish. The held result and the worker's
/// later rest follow ordinary canonical ownership and cleanup, after this job.
pub(super) fn finish_job(ctx: *mut TaskCtx) -> Stopped<()> {
    let task = engine::current(ctx);
    let owner = engine::with_driver(ctx, |d| {
        d.cpu_batch
            .as_ref()
            .and_then(|batch| batch.block_owner(task))
    });
    let result = engine::with_driver(ctx, |d| {
        let batch = d.cpu_batch.as_mut()?;
        let empty = Stretch::new(d.budgets);
        let state = &mut d.frontier.as_mut().unwrap().settled;
        batch.settle(task, &empty, state, true)
    });
    result.unwrap_or(Ok(())).map_err(Stop::BudgetExceeded)?;
    if let Some(owner) = owner
        && let Some(ledger) = unsafe { (*ctx).heap_ref().meter.cpu.get() }
    {
        ledger.blocks.lock().unwrap().disown(owner);
    }
    Ok(())
}

/// The head has settled this prefix. No driver, saved-frame or adapter borrow
/// crosses the home bridge, and physical retries never enter this function.
pub(super) fn record_summary(ctx: *mut TaskCtx, summary: &crate::io::CpuSummary) -> Stopped<()> {
    if unsafe { (*ctx).io_data() }.is_null() {
        return Ok(());
    }
    let mut result = Ok(());
    unsafe {
        crate::pool::call_home(ctx, &mut || {
            result = crate::io::Binding::get(ctx).adapter.cpu_summary(summary);
        })
    }?;
    result.map_err(Stop::refusal)
}

fn receipt(
    ctx: *mut TaskCtx,
    width: u32,
    action: u32,
    count: u64,
    value: Option<u32>,
) -> Stopped<Option<u32>> {
    if unsafe { (*ctx).io_data() }.is_null() {
        return Ok(value);
    }
    let mut result = Ok(None);
    unsafe {
        crate::pool::call_home(ctx, &mut || {
            result = crate::io::Binding::get(ctx)
                .adapter
                .cpu_control(width, action, count, value);
        })
    }?;
    result.map_err(Stop::refusal)
}

pub(crate) fn control(
    ctx: *mut TaskCtx,
    width: u32,
    action: u32,
    count: u64,
    live: impl FnOnce() -> u32,
) -> Stopped<u32> {
    engine::at_head(ctx)?;
    // The physical lifecycle may migrate and save this same head. No frontier
    // borrow or adapter loan spans startup, migration or join.
    let value = match receipt(ctx, width, action, count, None)? {
        Some(value) => value,
        None => {
            let value = live();
            receipt(ctx, width, action, count, Some(value))?;
            value
        }
    };
    let (_, progress) = engine::head_canonical(
        ctx,
        "cpu.control",
        || ((), if action == 5 { Next::Keep } else { Next::Yield }),
        |_| {
            Some(vec![(
                events::CanonicalItem::Root,
                events::CanonicalOutcome::CpuControl {
                    width,
                    action,
                    count,
                    outcome: value,
                },
            )])
        },
    )?;
    if action == 1 {
        // Leaving drains the physical pool even when it reports cancellation
        // or lost capacity. A later map starts a fresh bounded batch.
        engine::with_driver(ctx, |d| d.cpu_batch = None);
    } else if value == 0 {
        let task = engine::current(ctx);
        engine::with_driver(ctx, |d| match action {
            2 => {
                assert!(d.cpu_batch.is_none(), "one bounded CPU batch");
                let grant =
                    unsafe { (*ctx).run_ref().cpu_grant.as_ref() }.expect("an admitted CPU grant");
                d.cpu_batch = Some(Batch::new(
                    grant,
                    width,
                    count,
                    d.frontier.as_ref().unwrap().settled,
                ));
            }
            5 => {
                let batch = d.cpu_batch.as_mut().expect("preclaim follows CPU batch");
                assert_eq!(width, batch.width);
                assert!(count < batch.claims.len() as u64);
                assert!(batch.pending.insert(task, (width, count)).is_none());
            }
            3 => d
                .cpu_batch
                .as_mut()
                .expect("claim follows bounded CPU batch")
                .claim(task, width, count),
            _ => {}
        });
    }
    if action == 3 && value == 0 && super::ahead::cpu_worker::armed(ctx) {
        // The claim receipt retired at head. Its CPU-only tail may overlap
        // later turns, just like an Ordered completed read's certified tail.
        engine::continue_after_read(ctx, progress)?;
    } else {
        engine::continue_at_head(ctx, progress)?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{Adapter, AdapterError, Completion, Event, Request, Tree};
    use crate::io_sim::SimIo;

    fn event(width: u32, action: u32, count: u64, outcome: u32) -> Event {
        Event {
            request: Request::CpuControl {
                width,
                action,
                count,
                outcome,
            },
            completion: Completion::Ready,
            fault: None,
            resources: None,
        }
    }

    #[test]
    fn replay_checks_the_entire_receipt_before_consuming_it_and_refuses_a_cut() {
        for (width, action, count) in [(1, 0, 0), (4, 1, 0), (4, 0, 1)] {
            let mut replay = SimIo::new(Tree::default(), vec![event(4, 0, 0, 4)]);
            assert_eq!(
                replay.cpu_control(width, action, count, None),
                Err(AdapterError::ReplayMismatch(0))
            );
            assert_eq!(replay.cpu_control(4, 0, 0, None), Ok(Some(4)));
            assert_eq!(
                replay.cpu_control(4, 1, 0, None),
                Err(AdapterError::MissingInput(1))
            );
        }
    }

    #[test]
    fn replay_at_one_permit_uses_receipts_without_entering_the_physical_lifecycle() {
        let mut run = crate::host::BodyRun::new(1, false);
        run.adapter = Some(Box::new(SimIo::new(
            Tree::default(),
            vec![event(16, 0, 0, 0), event(16, 2, 32, 0), event(16, 1, 0, 0)],
        )));
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                (*ctx).run().cpu_grant = Some(grant(1 << 20));
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                for (action, count) in [(0, 0), (2, 32), (1, 0)] {
                    assert_eq!(
                        control(ctx, 16, action, count, || panic!(
                            "replay queried physical admission"
                        )),
                        Ok(0)
                    );
                    assert!(!crate::tasks::cpu_scoped(ctx));
                }
                crate::stop::check(ctx, engine::root_end(ctx, 0));
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    fn grant(limit: u64) -> hirsel_run_schema::CpuGrant {
        hirsel_run_schema::CpuGrant {
            provider: "cpu.scoped@1".into(),
            max_workers: 16,
            max_domains: 1,
            max_physical_bytes: 2 << 30,
            max_batch_bytes: limit,
        }
    }

    #[test]
    fn canonical_claims_settle_job_prefixes_and_cleanup_with_a_grant_only_limit() {
        for width in [1, 4, 8, 16] {
            let outcome = crate::host::BodyRun::new(1, false).run(
                |ctx| unsafe {
                    (*ctx).run().cpu_grant = Some(grant(64));
                    (*ctx).set_budgets(Some(100), None);
                    engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                    events::enable(ctx);
                },
                move |ctx| unsafe {
                    let _role = crate::tasks::role::Guard::new(ctx);
                    assert_eq!(control(ctx, width, 2, 2, || 0), Ok(0));
                    for ordinal in 0..2 {
                        assert_eq!(control(ctx, width, 3, ordinal, || 0), Ok(0));
                        (*ctx).view.compiled.fuel -= 3;
                        let block = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            0,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        (*ctx).heap().free(block);
                        engine::at_head(ctx).unwrap();
                        finish_job(ctx).unwrap();
                    }
                    engine::with_driver(ctx, |d| {
                        let state = d.frontier.as_ref().unwrap().settled;
                        assert_eq!(state.level, 0);
                        assert_eq!(state.limit, u64::MAX, "grant does not replace run limit");
                        assert_eq!(state.fuel, 94);
                        let batch = d.cpu_batch.as_mut().unwrap();
                        assert!(batch.owners.is_empty());
                        assert_eq!(batch.jobs.retire(), Ok(2));
                    });
                    assert_eq!(control(ctx, width, 1, 0, || 0), Ok(0));
                    assert!(engine::with_driver(ctx, |d| d.cpu_batch.is_none()));
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }

    #[test]
    fn cumulative_job_stop_matches_replay_without_querying_physical_admission() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        for width in [4, 8, 16] {
            let mut live_trace = None;
            for replay in [false, true] {
                let selected = Arc::new(AtomicU64::new(u64::MAX));
                let seen = selected.clone();
                let mut run = crate::host::BodyRun::new(1, false);
                if replay {
                    let receipts = std::iter::once(event(width, 2, 4, 0))
                        .chain((0..4).map(|job| event(width, 3, job, 0)))
                        .collect();
                    run.adapter = Some(Box::new(SimIo::new(Tree::default(), receipts)));
                }
                let (outcome, trace) = run.run_traced(
                    |ctx| unsafe {
                        let bytes = crate::alloc::allocation_charge(ctx, 0, 16, 8);
                        (*ctx).run().cpu_grant = Some(grant(bytes * 2));
                        (*ctx).set_budgets(Some(100), None);
                        engine::enable(
                            ctx,
                            crate::ordered::Policy::admit(&Default::default(), None),
                        );
                        events::enable(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = crate::tasks::role::Guard::new(ctx);
                        let live = || {
                            assert!(!replay, "replay entered physical CPU admission");
                            0
                        };
                        assert_eq!(control(ctx, width, 2, 4, live), Ok(0));
                        for ordinal in 0..4 {
                            assert_eq!(control(ctx, width, 3, ordinal, live), Ok(0));
                            seen.store(ordinal, Ordering::Relaxed);
                            (*ctx).view.compiled.fuel -= 1;
                            let _block = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                0,
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            engine::at_head(ctx).unwrap();
                            finish_job(ctx).unwrap();
                        }
                        panic!("third job must exceed the batch grant");
                    },
                );
                assert_eq!(
                    outcome.ended,
                    Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory))
                );
                assert_eq!(
                    selected.load(Ordering::Relaxed),
                    2,
                    "width {width}, replay {replay}"
                );
                assert_eq!(outcome.pending, 0);
                let trace = trace.unwrap();
                if replay {
                    assert_eq!(Some(trace.bytes()), live_trace.as_deref());
                } else {
                    live_trace = Some(trace.bytes().to_vec());
                }
            }
        }
    }

    #[test]
    fn job_frontier_imports_orchestration_charges_and_preserves_the_run_limit() {
        use crate::budget::stretch::{Budgets, Event, summarise};
        let budgets = Budgets {
            memory: true,
            fuel: true,
        };
        let mut state = Settled {
            level: 10,
            limit: 90,
            fuel: 20,
        };
        let mut batch = Batch::new(&grant(100), 4, 2, state);
        batch.claim(7, 4, 0);
        assert_eq!(
            batch.settle(
                7,
                &summarise(budgets, &[Event::Alloc(60)]).unwrap(),
                &mut state,
                true
            ),
            Some(Ok(()))
        );
        // A parent/result rest can credit and charge between two jobs.
        state.level = 30;
        state.fuel = 12;
        batch.claim(9, 4, 1);
        assert_eq!(
            batch.settle(
                9,
                &summarise(budgets, &[Event::Fuel, Event::Alloc(61)]).unwrap(),
                &mut state,
                false
            ),
            Some(Err(hirsel_run_schema::BudgetKind::Memory))
        );
        assert_eq!(state.level, 30);
        assert_eq!(state.fuel, 12);
        assert_eq!(state.limit, 90);
        assert_eq!(batch.jobs.retire().unwrap_err().job, 1);
    }

    #[test]
    fn published_cpu_values_and_retained_roots_survive_until_once_only_head_retirement() {
        use super::super::ahead::cpu_handoff::*;
        use crate::budget::stretch::{Budgets, Event, summarise};
        for width in [1, 4, 8, 16] {
            let mut state = Settled {
                level: 0,
                limit: 100,
                fuel: 20,
            };
            let mut batch = Batch::new(&grant(100), width, 2, state);
            batch.claim(7, width, 0);
            batch.claim(9, width, 1);
            assert!(
                !batch.owns(9 + (1 << 32), 1),
                "a recycled slot has no claim"
            );
            assert!(!batch.owns(9, 0), "the ordinal is bound to its task");
            let value = 42u64;
            let retained = [std::ptr::NonNull::<crate::Header>::dangling().as_ptr()];
            let mut summary = Summary {
                version: VERSION,
                phase: SUMMARY_PUBLISHED,
                ordinal: 1,
                value: (&raw const value).cast(),
                value_bytes: 8,
                descriptor: 0,
                retained_count: 1,
                retained: retained.as_ptr(),
            };
            let budgets = Budgets {
                memory: true,
                fuel: true,
            };
            batch.publish(
                9,
                summarise(budgets, &[Event::Fuel, Event::Alloc(60)]).unwrap(),
                &raw mut summary,
            );
            for _ in 0..3 {
                assert_eq!(summary.phase, SUMMARY_PUBLISHED);
                assert_eq!(state.level, 0, "publication never settles a younger charge");
                // SAFETY: this test retains both borrowed tables until retirement.
                unsafe {
                    assert_eq!(summary.value.cast::<u64>().read(), 42);
                    assert_eq!(summary.retained.read(), retained[0]);
                }
            }
            assert_eq!(
                batch.settle(7, &Stretch::new(budgets), &mut state, true),
                Some(Ok(()))
            );
            assert_eq!(summary.phase, SUMMARY_PUBLISHED);
            let RetiredSummary { result, record, .. } =
                batch.retire_summary(9, &mut state).unwrap();
            assert_eq!(record.ordinal, 1);
            assert_eq!(record.retirement, 0);
            assert_eq!(record.level, 60);
            assert_eq!(record.fuel, 19);
            assert_eq!(record.stop, None);
            record.validate().unwrap();
            assert_eq!(result, Ok(()));
            assert_eq!(summary.phase, SUMMARY_RETIRED);
            assert_eq!(
                state,
                Settled {
                    level: 60,
                    limit: 100,
                    fuel: 19
                }
            );
            assert!(batch.retire_summary(9, &mut state).is_none());
            assert_eq!(
                state,
                Settled {
                    level: 60,
                    limit: 100,
                    fuel: 19
                }
            );
            assert_eq!(
                batch.settle(
                    9,
                    &summarise(budgets, &[Event::Free(60)]).unwrap(),
                    &mut state,
                    true
                ),
                Some(Ok(()))
            );
            assert_eq!(
                state,
                Settled {
                    level: 0,
                    limit: 100,
                    fuel: 19
                }
            );
            assert!(batch.owners.is_empty());
        }
    }

    #[test]
    fn retired_summary_tape_follows_job_order_and_preserves_selected_budget_stops() {
        use super::super::ahead::cpu_handoff::*;
        use crate::budget::stretch::{Budgets, Event as Charge, summarise};
        use crate::io::{CpuSummary, Event};
        use hirsel_run_schema::BudgetKind;
        for (limit, fuel, stop) in [(64, 20, BudgetKind::Memory), (128, 1, BudgetKind::Fuel)] {
            let mut state = Settled {
                level: 0,
                limit,
                fuel,
            };
            let mut batch = Batch::new(&grant(128), 4, 2, state);
            let budgets = Budgets {
                fuel: true,
                memory: true,
            };
            let mut summaries = [0, 1].map(|ordinal| Summary {
                version: VERSION,
                phase: SUMMARY_PUBLISHED,
                ordinal,
                value: std::ptr::null(),
                value_bytes: 0,
                descriptor: 0,
                retained_count: 0,
                retained: std::ptr::null(),
            });
            batch.claim(7, 4, 0);
            batch.claim(9, 4, 1);
            // The younger physical publication must leave the tape untouched.
            batch.publish(
                9,
                summarise(budgets, &[Charge::Fuel, Charge::Alloc(48)]).unwrap(),
                &raw mut summaries[1],
            );
            batch.publish(
                7,
                summarise(budgets, &[Charge::Fuel, Charge::Alloc(32)]).unwrap(),
                &raw mut summaries[0],
            );
            assert_eq!(state.level, 0);
            let first = batch.retire_summary(7, &mut state).unwrap();
            assert_eq!(first.result, Ok(()));
            assert_eq!((first.record.ordinal, first.record.retirement), (0, 0));
            assert_eq!(
                batch.settle(7, &Stretch::new(budgets), &mut state, true),
                Some(Ok(()))
            );
            let second = batch.retire_summary(9, &mut state).unwrap();
            assert_eq!(second.result, Err(stop));
            assert_eq!((second.record.ordinal, second.record.retirement), (1, 1));
            assert_eq!(second.record.stop, Some(stop));
            assert_eq!(
                summaries[1].phase, SUMMARY_PUBLISHED,
                "a stopped mapper never resumes"
            );
            let records: [CpuSummary; 2] = [first.record, second.record];
            let events = records
                .iter()
                .map(|record| Event {
                    request: Request::CpuSummary(record.clone()),
                    completion: Completion::Ready,
                    fault: None,
                    resources: None,
                })
                .collect();
            let mut replay = SimIo::new(Tree::default(), events);
            assert_eq!(
                replay.cpu_summary(&records[1]),
                Err(AdapterError::ReplayMismatch(0))
            );
            for record in records {
                replay.cpu_summary(&record).unwrap();
            }
            replay.finish().unwrap();
        }
    }

    unsafe extern "C" fn job(ctx: *mut TaskCtx, env: *mut crate::Header, _: u64, _: u64) -> u8 {
        unsafe {
            let [width, ordinal] = crate::alloc::payload(env).cast::<[u64; 2]>().read();
            let _role = crate::tasks::role::Guard::new(ctx);
            assert_eq!(control(ctx, width as u32, 3, ordinal, || 0), Ok(0));
            (*ctx).view.compiled.fuel -= 1;
            let block =
                crate::alloc::hirsel_rt_v2_alloc(ctx, 0, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
            (*ctx).heap().free(block);
            crate::STATUS_OK
        }
    }

    #[test]
    fn claimed_task_prefixes_follow_native_selection_and_completion() {
        use crate::tasks::{
            hirsel_rt_v2_task_scope_enter, hirsel_rt_v2_task_scope_exit, hirsel_rt_v2_task_spawn,
        };
        for width in [1, 4, 8, 16] {
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    (*ctx).run().cpu_grant = Some(grant(1 << 20));
                    (*ctx).set_budgets(Some(1000), None);
                    engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        0
                    );
                    {
                        let _role = crate::tasks::role::Guard::new(ctx);
                        assert_eq!(control(ctx, width, 2, u64::from(width) * 2, || 0), Ok(0));
                    }
                    for ordinal in 0..u64::from(width) * 2 {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            0,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<[u64; 2]>()
                            .write([u64::from(width), ordinal]);
                        let mut handle = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut handle,
                                &scope,
                                job,
                                env,
                                0
                            ),
                            0
                        );
                    }
                    assert_eq!(hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), 0);
                    let _role = crate::tasks::role::Guard::new(ctx);
                    engine::with_driver(ctx, |d| {
                        let batch = d.cpu_batch.as_mut().unwrap();
                        assert_eq!(batch.jobs.retire(), Ok(width as usize * 2));
                        assert!(batch.owners.is_empty());
                    });
                    assert_eq!(control(ctx, width, 1, 0, || 0), Ok(0));
                    crate::stop::check(ctx, engine::root_end(ctx, 0));
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }
}
