//! Ordered selection policy over the shared saved-frame dispatcher.
//!
//! Physical queue ownership stays in pool::free. These states describe which
//! logical continuations may be notified, so repeated readiness scans cannot
//! notify a running segment and accidentally bypass a pending head boundary.
//! No scheduler borrow or context pointer is retained here.

use std::collections::BTreeMap;

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
    pub(super) frontier: Option<Frontier<Command, Outcome>>,
    pub(super) objects: Registry,
    execution: BTreeMap<TaskId, Execution>,
    pub(super) submitted: BTreeMap<TaskId, u64>,
    pub(super) recorded: BTreeMap<TaskId, u64>,
    pub(super) waiting: BTreeMap<TaskId, u64>,
    pub(super) returned: BTreeMap<TaskId, super::Returned>,
    pub(super) host_poll: bool,
}

impl Driver {
    pub(crate) fn new(root: TaskId, policy: Policy, settled: Settled, objects: Registry) -> Self {
        let mut frontier = Frontier::new(root, policy, settled);
        frontier.carries = objects.carries.clone();
        Self {
            frontier: Some(frontier),
            objects,
            execution: BTreeMap::from([(root, Execution::Saved { head_only: false })]),
            submitted: BTreeMap::new(),
            recorded: BTreeMap::new(),
            waiting: BTreeMap::new(),
            returned: BTreeMap::new(),
            host_poll: false,
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

    /// Return notifications in logical order, without retaining row references.
    /// Selected frames are omitted from later scans until claimed and saved.
    /// During a deferred poll no new turn may be selected before wake merge.
    pub(crate) fn select(&mut self) -> Vec<TaskId> {
        let Some(frontier) = &self.frontier else {
            return Vec::new();
        };
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

    /// Called under the role immediately before the owner switches to idle.
    /// A Full log or an allocation pause must wait for head, even if the last
    /// accepted D operation left its protocol state Ahead rather than Pending.
    pub(crate) fn suspending(&mut self, task: TaskId, head_only: bool) {
        let execution = self.execution.get_mut(&task).expect("a live task");
        assert_eq!(*execution, Execution::Running);
        *execution = Execution::Saving { head_only };
    }

    pub(crate) fn finishing(&mut self, task: TaskId) {
        let execution = self.execution.get_mut(&task).expect("a live task");
        assert_eq!(*execution, Execution::Running);
        *execution = Execution::Finishing;
    }

    /// Only the generic idle callback calls this, after publishing physical
    /// saved=true while holding IdleGuard. A logical park alone is insufficient.
    /// Completion metadata cannot be reused until the physical token ends.
    pub(crate) fn saved(&mut self, task: TaskId, completed: bool) {
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
) -> Vec<(u32, crate::pool::Owner)> {
    // The generation is part of the logical identity, even if the runtime
    // later recycles this native row index for a different child.
    let id = super::super::handle(task, s.task(task).generation);
    let super::super::Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    driver.saved(id, completed);
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

    struct BackendStub;
    impl Backend<Command> for BackendStub {
        type Result = Outcome;
        fn commit(&mut self, _: TaskId, _: &Operation<Command>, _: bool) -> Receipt<Outcome> {
            Receipt {
                result: Outcome::image(0, Vec::new()),
                next: Next::Yield,
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
                &mut BackendStub
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
