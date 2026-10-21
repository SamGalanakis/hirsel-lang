//! Ordered turn protocol (multicore design §4.3 and §6 Budgets).
//!
//! The scheduler role serialises calls into [`Frontier`]. Workers execute only
//! segments, hand it owned operations, and retain no scheduler/object borrow.
//! A logical queue entry never acquires a numeric retirement slot: calls insert
//! holding children's rests before it. Only completed turns receive indices.
//!
//! Structure follows the checked `proofs/ordered/Ordered.tla`: `offer` is
//! AheadSucc, `absorb` is Absorb, `finish_turn` is TurnEnd, and `eligible` is
//! Eligible. The native adapter, not this protocol, owns fibers and ABI images.

pub mod capture;
pub mod footprint;
pub mod native;

use std::collections::{BTreeMap, VecDeque};

use hirsel_rt_operations::descriptors::{Class, E, OpDescriptor, Transition};
use hirsel_run_config::{Commit, Placement, Reorder};
use hirsel_run_schema::BudgetKind;

use crate::budget::stretch::{Settled, Stretch};
use crate::multicore_policy::{self, AutoSample};
use footprint::{Carries, Footprint, Held, TaskId};

/// Admission choices, all outside run identity. Explicit zero disables logging.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub commit: Commit,
    pub run_ahead: u8,
    pub reorder: Reorder,
    pub speculation_bytes: u64,
}

impl Policy {
    pub fn admit(placement: &Placement, memory_bytes: Option<u64>) -> Self {
        Self {
            commit: placement.commit.unwrap_or(Commit::Shared),
            run_ahead: placement
                .run_ahead
                .unwrap_or(multicore_policy::AUTO_RUN_AHEAD_OPS as u8),
            reorder: placement.reorder.unwrap_or(Reorder::Auto),
            speculation_bytes: placement
                .speculation_bytes
                .unwrap_or_else(|| memory_bytes.map_or(u64::MAX, |limit| limit / 16)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Queued,
    Running(u16),
    Pending,
    Ahead,
    Parked,
    Holding,
    Done,
}

/// Result fixed by committing an op. Calls are resolved at commit, not when
/// the deferrable op was logged. A detach_all backend returns one member at a
/// time through Call, preserving member order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Next {
    Keep,
    Yield,
    Park,
    Call {
        members: Vec<TaskId>,
        then_yield: bool,
    },
    Hold,
    End,
    Stop,
}

/// Every byte referenced by P must be owned by the operation (or its suspended
/// fiber). A log may outlive the stack slot that originally held an argument.
pub struct Operation<P> {
    pub descriptor: &'static OpDescriptor,
    pub footprint: Footprint,
    pub payload: P,
}

/// The backend retains the already-fixed outcome and object versions in R.
/// `credit` is the storage death discovered by commit, at this op's position.
pub struct Receipt<R> {
    pub result: R,
    pub next: Next,
    pub credit: u64,
}

/// Object access happens under the scheduler role. An early send must enqueue
/// a pending message without touching its channel. `publish` clears pending
/// marks AND touches their channels, before the retirement's poll. `record`
/// alone writes external observations; `commit(early=true)` must never do so.
pub trait Backend<P> {
    type Result;

    fn commit(&mut self, task: TaskId, op: &Operation<P>, early: bool) -> Receipt<Self::Result>;
    fn record(&mut self, task: TaskId, op: &Operation<P>, result: &Self::Result, retirement: u64);
    fn publish(&mut self, task: TaskId, op: &Operation<P>, result: &Self::Result);
    /// Poll once, returning woken tasks in the reference scheduler's order.
    fn poll(&mut self) -> Vec<TaskId>;
}

struct Entry<P, R> {
    before: Stretch,
    operation: Operation<P>,
    committed: Option<Receipt<R>>,
    /// The task-local turn, not a slot in the global retirement order.
    turn: u64,
}

struct Task<P, R> {
    state: State,
    /// Turn containing the next operation a worker will offer.
    producing: u64,
    /// Turn whose log is next absorbed by the head.
    retiring: u64,
    log: VecDeque<Entry<P, R>>,
    speculative_bytes: u64,
    calls: Option<(VecDeque<TaskId>, bool)>,
}

impl<P, R> Task<P, R> {
    fn queued() -> Self {
        Self {
            state: State::Queued,
            producing: 0,
            retiring: 0,
            log: VecDeque::new(),
            speculative_bytes: 0,
            calls: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Offer {
    /// Outcome is fixed by E; the worker may consume it and run its continuation.
    Early,
    /// Statically OK D op: continue with no shared outcome or a provisional id.
    Logged,
    /// The owned op is pending; resume only when its log was absorbed at head.
    Pending,
    /// Log at its cap: no ownership was transferred, wait until head then retry.
    Full,
    /// Terminal prefix decided; the returned operation must be abandoned.
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pause {
    Allowance,
    Fuel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    NeedSegment(TaskId),
    Absorbed(TaskId),
    Called(TaskId),
    Retired,
    Idle,
    Stopped,
}

/// The role's complete logical order. No task/turn runs a scheduling point
/// merely because a worker reached the end of its segment or logged a yield.
pub struct Frontier<P, R> {
    policy: Policy,
    tasks: BTreeMap<TaskId, Task<P, R>>,
    head: Option<TaskId>,
    queue: VecDeque<TaskId>,
    /// Rule 1: queue and first turn captured when this head started.
    eligible_turns: BTreeMap<TaskId, u64>,
    pub held: BTreeMap<TaskId, Held>,
    pub carries: Carries,
    pub settled: Settled,
    retired: u64,
    physical: u64,
    auto: bool,
    stopped: bool,
}

impl<P, R> Frontier<P, R> {
    pub fn new(root: TaskId, policy: Policy, settled: Settled) -> Self {
        let mut tasks = BTreeMap::new();
        tasks.insert(root, Task::queued());
        let mut held = BTreeMap::new();
        held.insert(root, Held::default());
        Self {
            policy,
            tasks,
            head: Some(root),
            queue: VecDeque::new(),
            eligible_turns: BTreeMap::new(),
            held,
            carries: Carries::default(),
            settled,
            retired: 0,
            physical: 0,
            // A bounded initial probe is necessary to collect the first window.
            auto: true,
            stopped: false,
        }
    }

    /// Called only when a spawn commits or a wait's poll completes, never when
    /// a provisional task id is minted by a non-head worker.
    pub fn enqueue(&mut self, task: TaskId, handles: Held) {
        assert!(!self.stopped, "enqueue after the terminal prefix");
        assert!(!self.tasks.contains_key(&task), "task id reused while live");
        self.tasks.insert(task, Task::queued());
        self.held.insert(task, handles);
        self.queue.push_back(task);
    }

    pub fn head(&self) -> Option<TaskId> {
        self.head
    }

    pub fn queue(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.queue.iter().copied()
    }

    pub fn state(&self, task: TaskId) -> State {
        self.tasks[&task].state
    }

    pub fn retired(&self) -> u64 {
        self.retired
    }

    /// Worker claim, performed while holding the scheduler role.
    pub fn start(&mut self, task: TaskId, worker: u16) {
        let row = self.tasks.get_mut(&task).expect("a live task");
        assert!(matches!(row.state, State::Queued | State::Ahead));
        row.state = State::Running(worker);
    }

    /// Apply M0b's measured fallback. Neither this nor placement can relax E.
    pub fn tune(&mut self, sample: AutoSample) {
        self.auto = multicore_policy::reorder(sample);
    }

    /// Periodic bounded probe after a strict window, for recovery on a new shape.
    pub fn probe(&mut self) {
        self.auto = true;
    }

    /// Reserve physical storage before a non-head allocation. The head never
    /// waits on speculative allowance. A refused allocation has no effect.
    pub fn reserve(&mut self, task: TaskId, bytes: u64, fuel: u64) -> Result<(), Pause> {
        if self.head == Some(task) {
            return Ok(());
        }
        if fuel > self.settled.fuel {
            return Err(Pause::Fuel);
        }
        let room = self.policy.speculation_bytes.saturating_sub(self.physical);
        if bytes > room {
            return Err(Pause::Allowance);
        }
        self.tasks
            .get_mut(&task)
            .expect("a live task")
            .speculative_bytes += bytes;
        self.physical += bytes;
        Ok(())
    }

    pub fn free_speculative(&mut self, task: TaskId, bytes: u64) {
        let row = self.tasks.get_mut(&task).expect("a live task");
        assert!(bytes <= row.speculative_bytes);
        row.speculative_bytes -= bytes;
        self.physical -= bytes;
    }

    /// All four rules. Rule 4 is the Backend's pending/publish contract.
    pub fn eligible(&self, task: TaskId, operation: &Operation<P>) -> bool {
        let row = &self.tasks[&task];
        if self.stopped
            || self.policy.reorder == Reorder::Off
            || !self.auto
            || self.eligible_turns.get(&task) != Some(&row.producing)
            || !E.contains(&operation.descriptor.name)
            || !operation.descriptor.objects_only()
            || !operation.footprint.objects_only()
        {
            return false;
        }
        // Rule 2 includes earlier logged keep ops in this same turn. A logged
        // close is not ignored just because it returns no value.
        if row.log.iter().any(|entry| {
            entry.turn == row.producing
                && entry.committed.is_none()
                && entry.operation.footprint.conflicts(&operation.footprint)
        }) {
            return false;
        }
        let Some(head) = self.head else {
            return false;
        };
        if !self
            .carries
            .disjoint(head, &self.held, &operation.footprint)
        {
            return false;
        }
        for &earlier in &self.queue {
            if earlier == task {
                return true;
            }
            if !self
                .carries
                .disjoint(earlier, &self.held, &operation.footprint)
            {
                return false;
            }
        }
        false
    }

    /// Non-head worker reached an op. Full returns the op to the caller, which
    /// suspends without rerunning its segment. Every accepted entry owns its
    /// stretch; budget settlement happens before that op, in program order.
    pub fn offer<B: Backend<P, Result = R>>(
        &mut self,
        task: TaskId,
        before: Stretch,
        operation: Operation<P>,
        backend: &mut B,
    ) -> (Offer, Option<(Stretch, Operation<P>)>) {
        if self.stopped {
            return (Offer::Stopped, Some((before, operation)));
        }
        assert_ne!(self.head, Some(task), "head ops use apply_head");
        let row = &self.tasks[&task];
        assert!(!matches!(
            row.state,
            State::Pending | State::Done | State::Holding
        ));
        // One pending boundary is always admitted, including when cap is zero.
        if row.log.len() > usize::from(self.policy.run_ahead) {
            return (Offer::Full, Some((before, operation)));
        }
        let early = self.eligible(task, &operation);
        let can_log = operation.descriptor.class() == Class::D
            && row.log.len() < usize::from(self.policy.run_ahead);
        let committed = early.then(|| backend.commit(task, &operation, true));
        if let Some(receipt) = &committed {
            assert_eq!(receipt.credit, 0, "an E op cannot discover credits");
            assert!(matches!(receipt.next, Next::Keep | Next::Yield));
        }
        let offer = if early {
            Offer::Early
        } else if can_log {
            Offer::Logged
        } else {
            Offer::Pending
        };
        let row = self.tasks.get_mut(&task).unwrap();
        let next = committed.as_ref().map(|receipt| &receipt.next);
        // Deferrable calls do not end here: their actual transition is only
        // known at retirement, where a holding member becomes a call.
        let ends = next == Some(&Next::Yield)
            || (!early
                && operation
                    .descriptor
                    .transitions
                    .contains(&Transition::Yield));
        row.log.push_back(Entry {
            before,
            operation,
            committed,
            turn: row.producing,
        });
        if ends {
            row.producing += 1;
        }
        row.state = if offer == Offer::Pending {
            State::Pending
        } else {
            State::Ahead
        };
        (offer, None)
    }

    /// Take the outcome of the last E op without removing its retirement log.
    pub fn early_result(&self, task: TaskId) -> Option<&R> {
        self.tasks
            .get(&task)?
            .log
            .back()?
            .committed
            .as_ref()
            .map(|r| &r.result)
    }

    pub fn apply_head<B: Backend<P, Result = R>>(
        &mut self,
        before: Stretch,
        operation: Operation<P>,
        backend: &mut B,
    ) -> Result<Progress, BudgetKind> {
        if self.stopped {
            return Ok(Progress::Stopped);
        }
        let task = self.head.expect("a head");
        assert!(
            self.tasks[&task].log.is_empty(),
            "absorb the log before new head work"
        );
        self.apply(task, before, operation, None, backend)
    }

    /// A newly selected head absorbs its existing log before running any new
    /// operation. An early receipt is taken over, never committed a second time.
    pub fn absorb<B: Backend<P, Result = R>>(
        &mut self,
        backend: &mut B,
    ) -> Result<Progress, BudgetKind> {
        if self.stopped {
            return Ok(Progress::Stopped);
        }
        let Some(task) = self.head else {
            return Ok(Progress::Idle);
        };
        let Some(entry) = self.tasks.get_mut(&task).unwrap().log.pop_front() else {
            return Ok(Progress::NeedSegment(task));
        };
        self.apply(
            task,
            entry.before,
            entry.operation,
            entry.committed,
            backend,
        )
    }

    fn apply<B: Backend<P, Result = R>>(
        &mut self,
        task: TaskId,
        before: Stretch,
        operation: Operation<P>,
        committed: Option<Receipt<R>>,
        backend: &mut B,
    ) -> Result<Progress, BudgetKind> {
        if let Err(kind) = self.settled.settle(&before) {
            self.stopped = true;
            return Err(kind);
        }
        let early = committed.is_some();
        let receipt = committed.unwrap_or_else(|| backend.commit(task, &operation, false));
        self.settled.level = self.settled.level.saturating_sub(receipt.credit);
        backend.record(task, &operation, &receipt.result, self.retired);
        if early {
            backend.publish(task, &operation, &receipt.result);
        }
        match receipt.next {
            Next::Keep => Ok(Progress::Absorbed(task)),
            Next::Call {
                members,
                then_yield,
            } => {
                self.tasks.get_mut(&task).unwrap().calls = Some((members.into(), then_yield));
                Ok(self.continue_call(task, backend))
            }
            Next::End if self.held[&task].caller.is_some() => {
                let caller = self.held.get_mut(&task).unwrap().caller.take().unwrap();
                self.tasks.get_mut(&task).unwrap().state = State::Done;
                self.tasks.get_mut(&caller).unwrap().state = State::Queued;
                self.held.get_mut(&caller).unwrap().holding.remove(&task);
                self.head = Some(caller);
                Ok(self.continue_call(caller, backend))
            }
            Next::Stop => {
                self.stopped = true;
                Ok(Progress::Stopped)
            }
            next => {
                self.finish_turn(task, next, backend);
                Ok(Progress::Retired)
            }
        }
    }

    fn continue_call<B: Backend<P, Result = R>>(
        &mut self,
        task: TaskId,
        backend: &mut B,
    ) -> Progress {
        while let Some(child) = self
            .tasks
            .get_mut(&task)
            .unwrap()
            .calls
            .as_mut()
            .unwrap()
            .0
            .pop_front()
        {
            if self.tasks[&child].state != State::Holding {
                continue;
            }
            self.tasks.get_mut(&task).unwrap().state = State::Parked;
            self.tasks.get_mut(&child).unwrap().state = State::Queued;
            self.held.get_mut(&child).unwrap().caller = Some(task);
            self.head = Some(child);
            // Calls preserve the current eligible snapshot. After a yield in
            // a child's rest, callers' remaining holding members still count.
            return Progress::Called(child);
        }
        let (_, then_yield) = self.tasks.get_mut(&task).unwrap().calls.take().unwrap();
        if then_yield {
            self.finish_turn(task, Next::Yield, backend);
            Progress::Retired
        } else {
            Progress::Absorbed(task)
        }
    }

    fn finish_turn<B: Backend<P, Result = R>>(
        &mut self,
        task: TaskId,
        next: Next,
        backend: &mut B,
    ) {
        let row = self.tasks.get_mut(&task).unwrap();
        row.retiring += 1;
        row.producing = row.producing.max(row.retiring);
        // A log may span future turns. Their allocated storage remains part
        // of the physical allowance until those entries have been absorbed.
        if row.log.is_empty() {
            self.physical -= std::mem::take(&mut row.speculative_bytes);
        }
        row.state = match next {
            Next::Yield => State::Queued,
            Next::Park => State::Parked,
            Next::Hold => State::Holding,
            Next::End => State::Done,
            _ => unreachable!("only scheduling transitions finish a turn"),
        };
        for woken in backend.poll() {
            let waiter = self.tasks.get_mut(&woken).expect("poll names a live task");
            assert_eq!(waiter.state, State::Parked);
            waiter.state = State::Queued;
            self.queue.push_back(woken);
        }
        if next == Next::Yield {
            self.queue.push_back(task);
        }
        self.head = self.queue.pop_front();
        self.retired += 1;
        self.eligible_turns = self
            .queue
            .iter()
            .map(|&task| (task, self.tasks[&task].retiring))
            .collect();
    }
}

#[cfg(test)]
mod tests;
