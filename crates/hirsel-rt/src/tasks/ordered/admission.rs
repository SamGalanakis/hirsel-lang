//! Launcher-owned Ordered policy, prepared before any code or host effect.
//! The validated FIFO cooperative contract selects the production executor;
//! placement only configures its physical policy.

use crate::TaskCtx;
use crate::ordered::Policy;
use hirsel_run_config::ValidatedRun;

pub(crate) struct Admission {
    policy: Policy,
}

impl Admission {
    /// The provider and logical schedule select the executor; physical
    /// placement never changes either contract or the run's identity.
    pub(crate) fn prepare(run: Option<&ValidatedRun>) -> Result<Option<Self>, String> {
        let Some(run) = run else {
            return Ok(None);
        };
        let record = run.record();
        if record.io.as_ref().and_then(|io| io.tasks.as_deref()) != Some("tasks.cooperative@1")
            || record
                .inputs
                .scheduler
                .as_ref()
                .is_some_and(|scheduler| scheduler.code() != Some(0))
        {
            return Ok(None);
        }
        let placement = record.placement.clone().unwrap_or_default();
        let policy = Policy::admit(&placement, run.budgets().and_then(|b| b.memory_bytes));
        Ok(Some(Self { policy }))
    }

    /// Activate before the first Tasks projection, while the sole root owns
    /// the role. Earlier scalar effects have already completed in tape order.
    /// # Safety
    /// The caller supplies its live context; no runtime borrow crosses entry.
    pub(crate) unsafe fn activate(ctx: *mut TaskCtx) {
        if unsafe { (*ctx).run_ref().deferred_ordered.is_none() } {
            return;
        }
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        let admission = unsafe { (*ctx).run().deferred_ordered.take().unwrap() };
        // Return the root's unused fuel chunk before changing its budget owner.
        // Ordered initializes from the same held memory and remaining fuel.
        let worker = unsafe { (*ctx).worker_ptr() };
        unsafe {
            (*ctx).switch_out();
            admission.install(ctx);
            TaskCtx::switch_in(ctx, worker);
        }
    }

    /// Install after topology, budgets and the run's mode are configured.
    /// A sole root can defer this until its first Tasks projection. Width one
    /// uses the same saved-frame dispatcher as wider Ordered runs.
    ///
    /// # Safety
    /// The sole root, with topology installed and no Tasks objects yet.
    /// Production qualification must cover every reachable runtime boundary
    /// and every service's alias, lifetime, physical-save and stop contract.
    pub(crate) unsafe fn install(self, ctx: *mut TaskCtx) {
        assert!(!unsafe { (*ctx).run_ref().free_mode });
        assert!(unsafe { crate::pool::wide((*ctx).run_ref()) });
        unsafe {
            if !crate::pool::free::enabled((*ctx).run_ref()) {
                crate::pool::free::enable(ctx);
            }
            super::engine::enable(ctx, self.policy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_run_config::{Commit, Cores, Placement, Reorder, RunConfig};
    use hirsel_run_schema::{Budgets, IoGrants, LaunchKind, Scheduler};

    fn run(provider: &str) -> RunConfig {
        let mut run = RunConfig::empty([0; 32], "admission@1/main".into(), LaunchKind::Main);
        run.io = Some(IoGrants {
            tasks: Some(provider.into()),
            ..Default::default()
        });
        run
    }

    fn defer(ctx: *mut TaskCtx, fail_capture: bool) {
        let run = run("tasks.cooperative@1").validated().unwrap();
        let admission = Admission::prepare(Some(&run)).unwrap().unwrap();
        unsafe {
            let mut pending = crate::managed_cores::Pending::new(Placement {
                cores: Cores::Fixed(1),
                ..Default::default()
            });
            pending.fail_capture = fail_capture;
            (*ctx).run().deferred_capacity = Some(pending);
            (*ctx).run().deferred_ordered = Some(admission);
        }
    }

    #[test]
    fn a_scalar_run_keeps_its_result_when_capacity_files_are_unreadable() {
        for fail_capture in [false, true] {
            let mut run = crate::host::BodyRun::new(1, false);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| defer(ctx, fail_capture),
                |ctx| unsafe {
                    let block = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        0,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(block).cast::<u64>().write(42);
                    assert_eq!(crate::alloc::payload(block).cast::<u64>().read(), 42);
                    crate::release::hirsel_rt_v2_free(ctx, block);
                    Admission::activate(ctx);
                    assert!(super::super::engine::on(ctx));
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }

    #[test]
    fn the_first_pool_claim_stops_on_unreadable_capacity_before_its_effect() {
        let mut run = crate::host::BodyRun::new(1, false);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| defer(ctx, true),
            |ctx| unsafe {
                Admission::activate(ctx);
                crate::managed_cores::admit_capacity(ctx);
                panic!("a pool claim ran after capacity admission failed");
            },
        );
        assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn first_tasks_admission_preserves_existing_memory_charges_and_owns_later_release() {
        let mut run = crate::host::BodyRun::new(1, false);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                defer(ctx, false);
                (*ctx).set_budgets(Some(12345), Some(1 << 20));
            },
            |ctx| unsafe {
                let block =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 0, 128, 8, crate::alloc::DOMAIN_HEAP, 0);
                let held = (*ctx).heap_ref().meter.held();
                Admission::activate(ctx);
                crate::managed_cores::admit_capacity(ctx);
                assert!(super::super::engine::on(ctx));
                assert_eq!((*ctx).heap_ref().meter.held(), held);
                assert_eq!((*ctx).run_ref().fuel.remaining(), 12345);
                crate::release::hirsel_rt_v2_free(ctx, block);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn ordered_launch_installs_placement_policy_and_dispatcher_before_root_publication() {
        for commit in [Commit::Shared, Commit::Dedicated] {
            for reorder in [Reorder::Off, Reorder::Auto] {
                let mut record = run("tasks.cooperative@1");
                let identity = record.validated().unwrap().identity();
                record.placement = Some(Placement {
                    cores: Cores::Fixed(8),
                    commit: Some(commit),
                    run_ahead: Some(7),
                    reorder: Some(reorder),
                    speculation_bytes: Some(8192),
                    ..Default::default()
                });
                let record = record.validated().unwrap();
                assert_eq!(record.identity(), identity);
                let admission = Admission::prepare(Some(&record)).unwrap().unwrap();
                assert_eq!(admission.policy.commit, commit);
                assert_eq!(admission.policy.run_ahead, 7);
                assert_eq!(admission.policy.reorder, reorder);
                assert_eq!(admission.policy.speculation_bytes, 8192);
                let outcome = crate::host::BodyRun::new(1, false).run(
                    move |ctx| unsafe {
                        assert!((*(*ctx).run_ptr()).tasks.is_none());
                        crate::pool::start(ctx, 1, false);
                        admission.install(ctx);
                        assert!(super::super::engine::on(ctx));
                        assert!(crate::pool::free::enabled((*ctx).run_ref()));
                        assert!((*(*ctx).run_ptr()).tasks.is_some());
                        assert!(!(*ctx).run_ref().free_mode);
                    },
                    |_| crate::STATUS_OK,
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
            }
        }
    }

    #[test]
    fn ordered_launch_does_not_select_free_or_non_fifo_contracts() {
        assert!(Admission::prepare(None).unwrap().is_none());
        let mut empty = run("tasks.cooperative@1");
        empty.io.as_mut().unwrap().tasks = None;
        assert!(
            Admission::prepare(Some(&empty.validated().unwrap()))
                .unwrap()
                .is_none()
        );
        let free = run("tasks.free@1").validated().unwrap();
        assert!(Admission::prepare(Some(&free)).unwrap().is_none());
        for schedule in ["fifo@1:0", "random@1:7", "pct@1:3"] {
            let mut record = run("tasks.cooperative@1");
            record.inputs.scheduler = Scheduler::parse(schedule);
            let record = record.validated().unwrap();
            assert_eq!(
                Admission::prepare(Some(&record)).unwrap().is_some(),
                schedule.starts_with("fifo@")
            );
        }
    }

    #[test]
    fn ordered_launch_sizes_default_speculation_from_the_bound_memory_budget() {
        for limit in [None, Some(1), Some(15), Some(1600)] {
            let mut record = run("tasks.cooperative@1");
            record.budgets = limit.map(|memory_bytes| Budgets {
                provider: "budgets.limits@2".into(),
                fuel: None,
                memory_bytes: Some(memory_bytes),
            });
            let record = record.validated().unwrap();
            let admission = Admission::prepare(Some(&record)).unwrap().unwrap();
            assert_eq!(
                admission.policy.speculation_bytes,
                limit.map_or(u64::MAX, |n| n / 16)
            );
        }
    }
}
