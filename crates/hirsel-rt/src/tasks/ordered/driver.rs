//! Ordered selection policy over the shared saved-frame dispatcher.
//!
//! Physical queue ownership stays in pool::free. These states describe which
//! logical continuations may be notified, so repeated readiness scans cannot
//! notify a running segment and accidentally bypass a pending head boundary.
//! No scheduler borrow or context pointer is retained here.

use std::collections::{BTreeMap, BTreeSet};

use crate::budget::stretch::Settled;
use crate::ordered::footprint::{Held, TaskId};
use crate::ordered::objects::Registry;
use crate::ordered::{Frontier, Policy, State};

use super::{Command, Outcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Execution {
    Saved { head_only: bool },
    Selected,
    Running,
    Saving { head_only: bool },
    Finishing,
    Done,
}

/// Policy metadata intended for Scheduler.logical::Ordered. Native boundary,
/// budget and canonical-event adapters must be installed before admission can
/// select this driver; constructing this metadata alone activates no workers.
pub(crate) struct Driver {
    pub(super) tuning: super::tuning::Tuning,
    pub(super) frontier: Option<Frontier<Command, Outcome>>,
    pub(super) objects: Registry,
    execution: BTreeMap<TaskId, Execution>,
    pub(super) reserved: BTreeMap<u32, super::ahead::Reserved>,
    pub(super) awaiting_save: bool,
    pub(super) private_enabled: bool,
    pub(super) submitted: BTreeMap<TaskId, u64>,
    pub(super) recorded: BTreeMap<TaskId, u64>,
    pub(super) events: Option<super::events::Log>,
    pub(super) waiting: BTreeMap<TaskId, u64>,
    pub(super) returned: BTreeMap<TaskId, super::Returned>,
    pub(super) host_poll: bool,
    pub(super) host_ops: BTreeMap<TaskId, &'static str>,
    pub(super) next_host_token: u64,
    pub(super) fuel_initialized: bool,
    pub(super) fuel_grants: BTreeMap<TaskId, u64>,
    pub(super) stretches: BTreeMap<TaskId, crate::budget::stretch::Stretch>,
    pub(super) spawn_ordinals: BTreeMap<TaskId, u32>,
    pub(super) detaching: BTreeMap<TaskId, u32>,
    pub(super) plain_layouts: BTreeSet<u32>,
    pub(super) memory: bool,
    pub(super) budgets: crate::budget::stretch::Budgets,
    /// A park/holding/end boundary cannot poll slots before its physical save.
    pub(super) poll_after_save: Option<TaskId>,
    return_after_save: Option<TaskId>,
    pub(super) carriers: BTreeMap<(crate::ordered::objects::Kind, u64), super::engine::Carrier>,
}

impl Driver {
    pub(crate) fn new(root: TaskId, policy: Policy, settled: Settled, objects: Registry) -> Self {
        let mut frontier = Frontier::new(root, policy, settled);
        frontier.carries = objects.carries.clone();
        Self {
            tuning: super::tuning::Tuning::new(policy.reorder == hirsel_run_config::Reorder::Auto),
            frontier: Some(frontier),
            objects,
            execution: BTreeMap::from([(root, Execution::Saved { head_only: false })]),
            reserved: BTreeMap::new(),
            awaiting_save: false,
            private_enabled: policy.run_ahead != 0
                && policy.reorder == hirsel_run_config::Reorder::Auto,
            submitted: BTreeMap::new(),
            recorded: BTreeMap::new(),
            events: None,
            waiting: BTreeMap::new(),
            returned: BTreeMap::new(),
            host_poll: false,
            host_ops: BTreeMap::new(),
            next_host_token: 1,
            fuel_initialized: false,
            fuel_grants: BTreeMap::new(),
            stretches: BTreeMap::new(),
            spawn_ordinals: BTreeMap::new(),
            detaching: BTreeMap::new(),
            plain_layouts: BTreeSet::new(),
            memory: false,
            budgets: crate::budget::stretch::Budgets {
                fuel: false,
                memory: false,
            },
            poll_after_save: None,
            return_after_save: None,
            carriers: BTreeMap::new(),
        }
    }

    /// A committed spawn joins both the logical queue and physical candidates.
    /// A reserved spawn is not an enqueue and cannot become a speculative turn.
    pub(crate) fn enqueue(&mut self, task: TaskId, held: Held) {
        self.frontier.as_mut().unwrap().enqueue(task, held);
        assert!(
            self.execution
                .insert(task, Execution::Saved { head_only: false })
                .is_none()
        );
    }

    /// Permission for new head work includes all outstanding retirement gates.
    /// A save notification only enables retry: Frontier still owns the log.
    pub(super) fn head_ready(&self, task: TaskId) -> bool {
        !self.awaiting_save
            && !self.host_poll
            && !self.poll_deferred()
            && self.frontier.as_ref().is_some_and(|f| f.head_ready(task))
    }

    /// Return notifications in logical order, without retaining row references.
    /// Selected frames are omitted from later scans until claimed and saved.
    /// During a deferred poll no new turn may be selected before wake merge.
    pub(crate) fn select(&mut self) -> Vec<TaskId> {
        if self.awaiting_save || self.poll_after_save.is_some() || self.return_after_save.is_some()
        {
            return Vec::new();
        }
        let Some(frontier) = &mut self.frontier else {
            return Vec::new();
        };
        self.tuning.retire(frontier);
        let Some(head) = frontier.head() else {
            return Vec::new();
        };
        let order: Vec<_> = std::iter::once(head).chain(frontier.queue()).collect();
        let mut ready = Vec::new();
        for task in order {
            let Some(Execution::Saved { head_only }) = self.execution.get(&task).copied() else {
                continue;
            };
            let state = frontier.state(task);
            if matches!(state, State::Parked | State::Holding | State::Done)
                || (task != head && (head_only || !matches!(state, State::Queued | State::Ahead)))
            {
                continue;
            }
            self.execution.insert(task, Execution::Selected);
            ready.push(task);
        }
        ready
    }

    /// The shared dispatch token exclusively owns this frame before the claim.
    /// A head with a pending log resumes its boundary to absorb that log first.
    pub(crate) fn claimed(&mut self, task: TaskId, worker: u16) {
        let execution = self.execution.get_mut(&task).expect("a dispatched task");
        assert_eq!(*execution, Execution::Selected);
        *execution = Execution::Running;
        let frontier = self.frontier.as_mut().expect("restored frontier");
        if matches!(frontier.state(task), State::Queued | State::Ahead) {
            frontier.start(task, worker);
        } else {
            assert_eq!(
                frontier.head(),
                Some(task),
                "only a head resumes a pending boundary"
            );
        }
    }

    pub(super) fn enter(&mut self, task: TaskId, worker: u16) {
        match self.execution[&task] {
            Execution::Selected => self.claimed(task, worker),
            Execution::Running => {}
            _ => panic!("a native boundary without dispatch ownership"),
        }
    }

    /// Defer exactly this retirement until its owner publishes a saved frame.
    /// Native lifecycle adapters call this before suspending or finishing.
    pub(crate) fn defer_poll_until_saved(&mut self, task: TaskId) {
        assert!(!self.host_poll, "a home poll already owns this retirement");
        assert!(self.frontier.as_ref().unwrap().retiring_task().is_some());
        assert!(self.poll_after_save.replace(task).is_none());
    }

    pub(in crate::tasks) fn quiet(&self) -> bool {
        !self.host_poll
            && !self.poll_deferred()
            && self
                .frontier
                .as_ref()
                .is_some_and(|f| f.head().is_none() && f.queue().next().is_none())
    }

    pub(super) fn poll_deferred(&self) -> bool {
        self.poll_after_save.is_some() || self.return_after_save.is_some()
    }

    /// A child's no-poll return still waits for physical completion before its
    /// caller can retire the child's row or stack.
    pub(super) fn defer_return_until_saved(&mut self, task: TaskId) {
        assert!(self.return_after_save.replace(task).is_none());
    }

    /// Called under the role immediately before the owner switches to idle.
    /// A Full log or an allocation pause must wait for head, even if the last
    /// accepted D operation left its protocol state Ahead rather than Pending.
    pub(crate) fn suspending(&mut self, task: TaskId, head_only: bool) {
        let execution = self.execution.get_mut(&task).expect("a live task");
        assert_eq!(*execution, Execution::Running);
        *execution = Execution::Saving { head_only };
    }

    pub(super) fn is_finishing(&self, task: TaskId) -> bool {
        matches!(
            self.execution.get(&task),
            Some(Execution::Finishing | Execution::Done)
        )
    }

    pub(crate) fn finishing(&mut self, task: TaskId) {
        let execution = self.execution.get_mut(&task).expect("a live task");
        assert_eq!(*execution, Execution::Running);
        *execution = Execution::Finishing;
    }

    #[cfg(test)]
    pub(super) fn head_paused(&self, task: TaskId) -> bool {
        self.execution.get(&task) == Some(&Execution::Saved { head_only: true })
    }

    /// A retirement barrier belongs to a full logical identity, not a reusable
    /// native row. Detached completion may release that row before home finishes
    /// its host poll; Done remains proof that this generation physically saved.
    pub(super) fn physically_saved(&self, task: TaskId) -> bool {
        matches!(
            self.execution.get(&task),
            Some(Execution::Saved { .. } | Execution::Done)
        )
    }

    /// Only the generic idle callback calls this, after publishing physical
    /// saved=true while holding IdleGuard. A logical park alone is insufficient.
    /// Completion metadata cannot be reused until the physical token ends.
    pub(crate) fn saved(&mut self, task: TaskId, completed: bool) {
        if completed {
            self.tuning.finished(task);
        }
        let execution = self.execution.get_mut(&task).expect("a saving task");
        *execution = match (*execution, completed) {
            (Execution::Saving { head_only }, false) => Execution::Saved { head_only },
            (Execution::Finishing, true) => Execution::Done,
            _ => panic!("logical completion disagrees with physical save"),
        };
    }
}

/// IdleGuard protects this entire callback. Physical save publication has
/// already happened; no context, host service or fiber switch is involved.
pub(crate) fn after_save(
    s: &mut super::super::Scheduler,
    task: u32,
    completed: bool,
    host_timers: bool,
    pool_available: bool,
) -> Vec<(u32, crate::pool::Owner)> {
    // The generation is part of the logical identity, even if the runtime
    // later recycles this native row index for a different child.
    let id = super::super::handle(task, s.task(task).generation);
    let super::super::Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    driver.saved(id, completed);
    super::lifecycle::after_save(s, task, completed);
    let super::super::Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    if driver.return_after_save == Some(id) {
        assert!(completed, "a call returns only after child completion");
        driver.return_after_save = None;
    }
    let deferred = driver.poll_after_save;
    if let Some(retiring) = deferred {
        if !host_timers && driver.physically_saved(retiring)
            // SAFETY: generic after_save owns IdleGuard and published saved.
            && unsafe { super::super::readiness::poll_ordered(s, pool_available) }
        {
            let ready = std::mem::take(&mut s.runnable)
                .into_iter()
                .map(|task| super::super::handle(task, s.task(task).generation))
                .collect();
            let super::super::Logical::Ordered(driver) = &mut s.logical else {
                unreachable!()
            };
            assert!(driver.frontier.as_ref().unwrap().retiring_task().is_some());
            driver.frontier.as_mut().unwrap().complete_poll(ready);
            driver.poll_after_save = None;
        }
    }
    if completed {
        super::lifecycle::retire_detached(s, task);
    }
    let super::super::Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let selected = driver.select();
    selected
        .into_iter()
        .map(|id| {
            let task = id as u32;
            assert_eq!(
                s.task(task).generation,
                (id >> 32) as u32,
                "selected stale task identity"
            );
            (task, s.task(task).owner)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::stretch::{Budgets, Stretch};
    use crate::ordered::{Backend, Next, Operation, Receipt};
    use hirsel_rt_operations::descriptors::op_descriptor;

    struct BackendStub(Next);
    impl Backend<Command> for BackendStub {
        type Result = Outcome;
        fn commit(&mut self, _: TaskId, _: &Operation<Command>, _: bool) -> Receipt<Outcome> {
            Receipt {
                result: Outcome::image(0, Vec::new()),
                next: self.0.clone(),
                credit: 0,
            }
        }
        fn record(&mut self, _: TaskId, _: &Operation<Command>, _: &Outcome, _: u64) {}
        fn publish(&mut self, _: TaskId, _: &Operation<Command>, _: &Outcome) {}
        fn defer_poll(&self) -> bool {
            true
        }
        fn poll(&mut self) -> Vec<TaskId> {
            panic!("driver must restore policy before polling")
        }
    }

    fn driver() -> Driver {
        let mut driver = Driver::new(
            0,
            Policy::admit(&Default::default(), None),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
            Registry::default(),
        );
        driver
            .frontier
            .as_mut()
            .unwrap()
            .held
            .insert(0, Held::empty());
        driver.enqueue(1, Held::empty());
        driver.enqueue(2, Held::empty());
        driver
    }
    fn end_turn(driver: &mut Driver) {
        let op = Operation {
            descriptor: op_descriptor("atomic.load").unwrap(),
            footprint: Default::default(),
            payload: Command::AtomicLoad { word: 0, bytes: 8 },
        };
        assert_eq!(
            driver.frontier.as_mut().unwrap().apply_head(
                Stretch::new(Budgets {
                    fuel: false,
                    memory: false
                }),
                op,
                &mut BackendStub(Next::Yield)
            ),
            Ok(crate::ordered::Progress::Poll)
        );
    }

    #[test]
    fn the_ordered_idle_callback_resumes_real_saved_root_frames_without_duplicate_notifications() {
        let mut run = crate::host::BodyRun::new(8, false);
        // SAFETY: this body routes every suspension/end through saved-frame
        // dispatch. It uses no task/host effect or remote stack slot.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| {
                let s = super::super::super::scheduler(ctx);
                let root = super::super::super::handle(0, s.task(0).generation);
                let mut driver = Driver::new(
                    root,
                    Policy::admit(&Default::default(), None),
                    Settled {
                        level: 0,
                        limit: u64::MAX,
                        fuel: u64::MAX,
                    },
                    Registry::default(),
                );
                assert_eq!(driver.select(), vec![root]);
                s.logical = super::super::super::Logical::Ordered(driver);
            },
            |ctx| {
                unsafe {
                    super::super::super::role::enter(ctx);
                    let s = super::super::super::scheduler(ctx);
                    let task = (*ctx).task_id;
                    let id = super::super::super::handle(task, s.task(task).generation);
                    let super::super::super::Logical::Ordered(driver) = &mut s.logical else {
                        unreachable!()
                    };
                    driver.claimed(id, 0);
                    for _ in 0..256 {
                        let s = super::super::super::scheduler(ctx);
                        let super::super::super::Logical::Ordered(driver) = &mut s.logical else {
                            unreachable!()
                        };
                        assert!(driver.select().is_empty());
                        driver.suspending(id, false);
                        assert!(driver.select().is_empty());
                        crate::pool::free::suspend_current(ctx, false);
                        let s = super::super::super::scheduler(ctx);
                        let super::super::super::Logical::Ordered(driver) = &mut s.logical else {
                            unreachable!()
                        };
                        driver.claimed(id, 0);
                        assert!(driver.select().is_empty());
                    }
                    let s = super::super::super::scheduler(ctx);
                    let super::super::super::Logical::Ordered(driver) = &mut s.logical else {
                        unreachable!()
                    };
                    driver.finishing(id);
                    super::super::super::role::release(ctx);
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn a_retirement_poll_waits_for_the_real_idle_save_before_waking_its_owner() {
        let mut run = crate::host::BodyRun::new(8, false);
        // SAFETY: the fixture uses runtime Retry waits and explicit physical
        // saves only. The backend models a park; it performs no native op.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                super::super::engine::enable(ctx, Policy::admit(&Default::default(), None));
            },
            |ctx| unsafe {
                use super::super::super::{State as NativeState, Wait, role, scheduler};
                role::enter(ctx);
                let id = {
                    let s = scheduler(ctx);
                    super::super::super::handle(s.current, s.task(s.current).generation)
                };
                for turn in 0..256 {
                    let s = scheduler(ctx);
                    let super::super::super::Logical::Ordered(driver) = &mut s.logical else {
                        unreachable!()
                    };
                    driver.enter(id, 0);
                    let op =
                        Command::AtomicLoad { word: 0, bytes: 8 }.operation(Default::default());
                    assert_eq!(
                        driver.frontier.as_mut().unwrap().apply_head(
                            Stretch::new(Budgets {
                                fuel: false,
                                memory: false
                            }),
                            op,
                            &mut BackendStub(Next::Park),
                        ),
                        Ok(crate::ordered::Progress::Poll)
                    );
                    driver.defer_poll_until_saved(id);
                    driver.suspending(id, false);
                    assert!(driver.select().is_empty());
                    let me = s.current;
                    s.task(me).retried = true;
                    s.park(Wait::Retry { cancelable: false });
                    assert!(!super::super::super::readiness::poll_ordered(s, true));
                    assert!(s.task(me).retried, "preflight must not consume the retry");
                    crate::pool::free::suspend_current(ctx, false);
                    let s = scheduler(ctx);
                    assert_eq!(s.task(me).state, NativeState::Ready);
                    assert!(s.task(me).wait.is_none());
                    let super::super::super::Logical::Ordered(driver) = &mut s.logical else {
                        unreachable!()
                    };
                    assert_eq!(driver.frontier.as_ref().unwrap().retired(), turn + 1);
                    assert_eq!(driver.frontier.as_ref().unwrap().head(), Some(id));
                    assert_eq!(driver.poll_after_save, None);
                }
                let s = scheduler(ctx);
                let super::super::super::Logical::Ordered(driver) = &mut s.logical else {
                    unreachable!()
                };
                driver.enter(id, 0);
                driver.finishing(id);
                role::release(ctx);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn a_detached_end_keeps_its_saved_barrier_after_row_retirement_and_reuse() {
        use super::super::super::{Logical, Scheduler, Scope, Task, handle};
        let root = handle(0, 1);
        let child = handle(1, 2);
        let sibling = handle(2, 3);
        let mut s = Scheduler::default();
        s.tasks = vec![
            Some(Task::new(1, std::ptr::null_mut(), None, vec![], None)),
            Some(Task::new(2, std::ptr::null_mut(), Some(0), vec![0], None)),
            Some(Task::new(3, std::ptr::null_mut(), Some(0), vec![1], None)),
        ];
        s.scopes.push(Some(Scope {
            generation: 4,
            owner: 0,
            children: vec![1, 2],
            canceled: Default::default(),
            deadline: None,
            completions: Default::default(),
        }));
        s.link_child(0, 1);
        s.link_child(0, 2);
        s.task(1).detached = true;
        s.task(1).finishing = true;
        let mut driver = Driver::new(
            child,
            Policy::admit(&Default::default(), None),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
            Registry::default(),
        );
        driver
            .frontier
            .as_mut()
            .unwrap()
            .held
            .insert(child, Held::empty());
        driver.enqueue(root, Held::empty());
        driver.enqueue(sibling, Held::empty());
        driver.execution.insert(child, Execution::Finishing);
        driver
            .execution
            .insert(sibling, Execution::Saving { head_only: true });
        assert_eq!(
            driver.frontier.as_mut().unwrap().apply_head(
                Stretch::new(Budgets {
                    fuel: false,
                    memory: false
                }),
                Command::AtomicLoad { word: 0, bytes: 8 }.operation(Default::default()),
                &mut BackendStub(Next::End),
            ),
            Ok(crate::ordered::Progress::Poll)
        );
        driver.defer_poll_until_saved(child);
        s.logical = Logical::Ordered(driver);
        assert!(after_save(&mut s, 1, true, true, true).is_empty());
        assert!(s.tasks[1].is_none(), "a detached end releases its row");
        // Another speculative frame can save while home still owns the poll.
        assert!(after_save(&mut s, 2, false, true, true).is_empty());
        // Reuse the same native index for a different, not-yet-saved generation.
        s.tasks[1] = Some(Task::new(5, std::ptr::null_mut(), Some(0), vec![2], None));
        let Logical::Ordered(driver) = &mut s.logical else {
            unreachable!()
        };
        driver
            .execution
            .insert(handle(1, 5), Execution::Saving { head_only: true });
        driver
            .execution
            .insert(sibling, Execution::Saving { head_only: true });
        // The old generation's barrier is satisfied independently of that row.
        assert!(!after_save(&mut s, 2, false, false, true).is_empty());
        let Logical::Ordered(driver) = &s.logical else {
            unreachable!()
        };
        assert!(driver.poll_after_save.is_none());
        assert_eq!(driver.frontier.as_ref().unwrap().retired(), 1);
    }

    #[test]
    fn selecting_or_running_a_frame_does_not_notify_it_again() {
        let mut driver = driver();
        assert_eq!(driver.select(), vec![0, 1, 2]);
        assert!(driver.select().is_empty());
        for task in 0..3 {
            driver.claimed(task, task as u16);
        }
        assert!(driver.select().is_empty());
        driver.suspending(0, false);
        assert!(
            driver.select().is_empty(),
            "logical suspension is not a saved stack"
        );
        driver.saved(0, false);
        assert_eq!(driver.select(), vec![0]);
        assert!(driver.select().is_empty());
    }

    #[test]
    fn a_full_log_waiter_requires_both_idle_save_and_logical_head() {
        let mut driver = driver();
        assert_eq!(driver.select(), vec![0, 1, 2]);
        for task in 0..3 {
            driver.claimed(task, task as u16);
        }
        driver.suspending(1, true);
        driver.saved(1, false);
        assert!(driver.select().is_empty());
        end_turn(&mut driver);
        assert!(
            driver.select().is_empty(),
            "host poll has not returned its wake list"
        );
        driver.frontier.as_mut().unwrap().complete_poll(Vec::new());
        assert_eq!(driver.select(), vec![1]);
        assert!(driver.select().is_empty());
    }

    #[test]
    fn completion_never_requeues_an_abandoned_frame() {
        let mut driver = driver();
        driver.select();
        driver.claimed(0, 0);
        driver.finishing(0);
        assert!(driver.select().is_empty());
        driver.saved(0, true);
        assert!(driver.select().is_empty());
    }
}
