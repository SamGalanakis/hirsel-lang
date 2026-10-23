//! Launcher-owned Ordered policy, prepared before any code or host effect.
//! Configuration is staged separately from the E1 production qualification.
//! Neither a placement request nor a wider substrate test grants admission.

use crate::TaskCtx;
use crate::ordered::Policy;
use hirsel_run_config::ValidatedRun;

// Enable only after the native differential, host, value, physical-bound and
// performance gates have qualified production E1. Keep this independent of F1.
const PRODUCTION_QUALIFIED: bool = cfg!(feature = "ordered-qualification");

pub(crate) fn qualified() -> bool {
    PRODUCTION_QUALIFIED
}

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

    pub(crate) fn enabled(&self) -> bool {
        PRODUCTION_QUALIFIED
    }

    /// Install after topology, budgets and the run's mode are configured, and
    /// before root publication. Width one uses the same saved-frame dispatcher
    /// as wider Ordered runs; the home thread keeps its service ownership.
    ///
    /// # Safety
    /// Admission's fresh root, before any code, with topology installed.
    /// Production qualification must cover every reachable runtime boundary
    /// and every service's alias, lifetime, physical-save and stop contract.
    pub(crate) unsafe fn install(self, ctx: *mut TaskCtx) {
        if !self.enabled() {
            return;
        }
        assert!(!unsafe { (*ctx).run_ref().free_mode });
        assert!(unsafe { crate::pool::wide((*ctx).run_ref()) });
        unsafe {
            crate::pool::free::enable(ctx);
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

    #[test]
    fn ordered_launch_stages_placement_without_installing_a_policy_or_dispatcher() {
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
                assert!(!admission.enabled());
                assert_eq!(admission.policy.commit, commit);
                assert_eq!(admission.policy.run_ahead, 7);
                assert_eq!(admission.policy.reorder, reorder);
                assert_eq!(admission.policy.speculation_bytes, 8192);
                let outcome = crate::host::BodyRun::new(1, false).run(
                    move |ctx| unsafe {
                        admission.install(ctx);
                        assert!(!super::super::engine::on(ctx));
                        assert!(!crate::pool::free::enabled((*ctx).run_ref()));
                        assert!((*(*ctx).run_ptr()).tasks.is_none());
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
            assert!(!admission.enabled());
        }
    }
}
