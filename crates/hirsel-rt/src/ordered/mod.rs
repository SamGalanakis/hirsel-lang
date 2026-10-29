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
pub mod objects;
pub(crate) mod root_ids;
pub mod type_graph;

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

/// Borrowed evidence at an operation's retirement position. Reading it never
/// replays a commit, samples a later object version or changes an ordinal.
/// Writers copy only when tracing/recording is enabled.
#[derive(Clone, Copy, Debug)]
pub struct Retirement<'a> {
    pub index: u64,
    pub before: &'a Stretch,
    pub transition: &'a Next,
    pub credit: u64,
}

/// Object access happens under the scheduler role. An early send must enqueue
/// a pending message without touching its channel. `publish` clears pending
/// marks AND touches their channels, before the retirement's poll. `record`
/// alone writes external observations; `commit(early=true)` must never do so.
pub trait Backend<P> {
    type Result;

    /// A private physical owner may still be saving storage this operation
    /// adopts. Refusal retains the entire operation and its positional stretch;
    /// it does not settle, commit, poll or give the turn a retirement index.
    fn can_retire(&mut self, _task: TaskId, _op: &Operation<P>) -> bool {
        true
    }

    /// Dynamic readiness is separate from footprint independence. In
    /// particular an unavailable mutex must wait at its boundary, even when
    /// its type/object footprint would otherwise permit early commit.
    fn can_commit_early(&mut self, _op: &Operation<P>) -> bool {
        true
    }

    fn commit(&mut self, task: TaskId, op: &Operation<P>, early: bool) -> Receipt<Self::Result>;
    fn record(&mut self, task: TaskId, op: &Operation<P>, result: &Self::Result, retirement: u64);
    /// Complete metadata for canonical writers. Existing backends retain
    /// their record callback; no summary copy is needed when unused.
    fn record_retirement(
        &mut self,
        task: TaskId,
        op: &Operation<P>,
        result: &Self::Result,
        retirement: Retirement<'_>,
    ) {
        self.record(task, op, result, retirement.index);
    }
    /// A waiting operation may resume, yield or call without a second op.
    /// Its continuation has a positional stretch but consumes no ordinal.
    fn record_continuation(&mut self, _task: TaskId, _retirement: Retirement<'_>) {}
    fn record_budget_stop(
        &mut self,
        _task: TaskId,
        _before: &Stretch,
        _retirement: u64,
        _kind: BudgetKind,
    ) {
    }
    fn publish(&mut self, task: TaskId, op: &Operation<P>, result: &Self::Result);
    /// Enter one holding rest selected by a retiring call, including later
    /// members after a previous rest returns. No poll or user code may run.
    fn call(&mut self, _caller: TaskId, _child: TaskId) {}
    /// Process the next member of a bulk call even when it needs no rest.
    /// This keeps detach/cancellation effects in member order across calls.
    fn call_member(&mut self, _caller: TaskId, _child: TaskId) {}
    /// The final rest returned (or every member needed no call), before the
    /// caller's final transition. A bulk operation can release its owned set.
    fn call_returned(&mut self, _caller: TaskId) {}
    /// Native polling can call a home adapter and release the scheduler role.
    /// Return true to request an owned polling continuation instead of calling
    /// poll while Frontier is borrowed. Complete it with Frontier::complete_poll.
    fn defer_poll(&self) -> bool {
        false
    }
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
    /// Bytes in a still-running segment or an unaccepted Full command. They
    /// cannot be reclaimed merely because its last accepted log entry retires.
    open_speculative_bytes: u64,
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
            open_speculative_bytes: 0,
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
    /// The head is waiting for physical save acknowledgement, not for a
    /// language operation or a cooperative scheduling point.
    NeedSave(TaskId),
    Absorbed(TaskId),
    Called(TaskId),
    Retired,
    /// Restore policy metadata before a host bridge; no head is selected
    /// until the resulting wake list has been passed to complete_poll.
    Poll,
    Idle,
    Stopped,
}

/// The role's complete logical order. No task/turn runs a scheduling point
/// merely because a worker reached the end of its segment or logged a yield.
pub struct Frontier<P, R> {
    policy: Policy,
    tasks: BTreeMap<TaskId, Task<P, R>>,
    head: Option<TaskId>,
    polling: Option<(TaskId, Next)>,
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
    /// A terminal transition or budget settlement has closed the retired prefix.
    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn new(root: TaskId, policy: Policy, settled: Settled) -> Self {
        let mut tasks = BTreeMap::new();
        tasks.insert(root, Task::queued());
        let mut held = BTreeMap::new();
        held.insert(root, Held::default());
        Self {
            policy,
            tasks,
            head: Some(root),
            polling: None,
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

    /// Retirement awaiting its readiness continuation; there is no head until
    /// that continuation merges wakes and requeues the retiring task.
    pub fn retiring_task(&self) -> Option<TaskId> {
        self.polling.as_ref().map(|(task, _)| *task)
    }

    pub fn head(&self) -> Option<TaskId> {
        self.head
    }

    /// Logical identity alone does not permit another operation or charge.
    /// An owned log may still await its physical owner's save at this head.
    pub(crate) fn head_ready(&self, task: TaskId) -> bool {
        !self.stopped
            && self.polling.is_none()
            && self.head == Some(task)
            && self.tasks[&task].log.is_empty()
    }

    pub fn queue(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.queue.iter().copied()
    }

    /// Readiness after an idle host wait appends saved parked tasks without
    /// manufacturing a turn retirement or a yielding task's requeue.
    pub(crate) fn resume_idle(&mut self, woken: Vec<TaskId>) {
        assert!(self.head.is_none() && self.polling.is_none() && self.queue.is_empty());
        assert!(!self.stopped);
        for task in woken {
            let row = self.tasks.get_mut(&task).expect("a live waiter");
            assert_eq!(row.state, State::Parked);
            row.state = State::Queued;
            self.queue.push_back(task);
        }
        self.head = self.queue.pop_front();
        self.eligible_turns = self
            .queue
            .iter()
            .map(|&id| (id, self.tasks[&id].retiring))
            .collect();
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
        assert!(
            matches!(row.state, State::Queued | State::Ahead)
                || (self.head == Some(task) && row.state == State::Pending && row.log.is_empty())
        );
        row.state = State::Running(worker);
    }

    /// A running task whose saved frame left its worker's stack is adopted by
    /// another worker. Its turn, log, fuel and queue position are unchanged:
    /// only the physical owner of its claim moves.
    pub fn moved(&mut self, task: TaskId, worker: u16) {
        let row = self.tasks.get_mut(&task).expect("a live task");
        assert!(
            matches!(row.state, State::Running(_)),
            "only a running claim moves"
        );
        row.state = State::Running(worker);
    }

    /// Whether an offer of a class D op from non-head `task` is logged now
    /// rather than returned Full or Pending (`offer`).
    pub(crate) fn logs(&self, task: TaskId) -> bool {
        !self.stopped
            && self.polling.is_none()
            && self.tasks[&task].log.len() < usize::from(self.policy.run_ahead)
    }

    /// Whether non-head `task`'s next E op commits early whatever its
    /// footprint (`eligible`'s rules 1 and 3 with every footprint): the head
    /// and every task before it hold nothing an op could touch.
    pub(crate) fn commits_early(&self, task: TaskId) -> bool {
        let row = &self.tasks[&task];
        if self.stopped
            || self.polling.is_some()
            || row.log.len() > usize::from(self.policy.run_ahead)
            || self.policy.reorder == Reorder::Off
            || !self.auto
            || self.eligible_turns.get(&task) != Some(&row.producing)
        {
            return false;
        }
        let Some(head) = self.head else {
            return false;
        };
        let holds_nothing = |task: &TaskId| {
            self.held.get(task).is_some_and(|h| {
                !h.unknown
                    && h.objects.is_empty()
                    && h.obtains.is_empty()
                    && h.holding.is_empty()
                    && h.caller.is_none()
            })
        };
        holds_nothing(&head)
            && self
                .queue
                .iter()
                .take_while(|&&earlier| earlier != task)
                .all(holds_nothing)
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
        let row = self.tasks.get_mut(&task).expect("a live task");
        row.speculative_bytes += bytes;
        row.open_speculative_bytes += bytes;
        self.physical += bytes;
        Ok(())
    }

    pub fn free_speculative(&mut self, task: TaskId, bytes: u64) {
        let row = self.tasks.get_mut(&task).expect("a live task");
        assert!(bytes <= row.speculative_bytes);
        row.speculative_bytes -= bytes;
        // A free may release storage from an already logged stretch. Without
        // allocation identities, retain the open reservation conservatively;
        // only the total remaining storage is a proven smaller bound.
        row.open_speculative_bytes = row.open_speculative_bytes.min(row.speculative_bytes);
        self.physical -= bytes;
    }

    pub(crate) fn credit_speculative(&mut self, task: TaskId, bytes: u64) {
        let bytes = bytes.min(self.tasks[&task].speculative_bytes);
        self.free_speculative(task, bytes);
    }

    /// Revoke an unretired CPU allocation suffix, before any semantic log.
    pub(crate) fn discard_cpu_attempt(&mut self, task: TaskId) {
        let row = self.tasks.get_mut(&task).expect("a live CPU attempt");
        assert!(row.log.is_empty(), "CPU rollback cannot undo an operation");
        self.physical -= row.speculative_bytes;
        row.speculative_bytes = 0;
        row.open_speculative_bytes = 0;
    }

    /// Fuel already consumed in this task's logged future turns.
    pub(crate) fn logged_fuel(&self, task: TaskId) -> u64 {
        self.tasks[&task].log.iter().fold(0, |fuel, entry| {
            fuel.saturating_add(entry.before.fuel_charges())
        })
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
        if self.polling.is_some() {
            return (Offer::Full, Some((before, operation)));
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
        let early = self.eligible(task, &operation) && backend.can_commit_early(&operation);
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
        row.open_speculative_bytes = 0;
        if ends {
            row.producing += 1;
        }
        row.state = if row.calls.is_some() {
            // Its worker may still produce future log entries, but only the
            // called child's return can release this logical continuation.
            State::Parked
        } else if offer == Offer::Pending {
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
        assert!(
            self.polling.is_none(),
            "complete the poll before new head work"
        );
        let task = self.head.expect("a head");
        assert!(
            self.tasks[&task].log.is_empty(),
            "absorb the log before new head work"
        );
        self.tasks.get_mut(&task).unwrap().open_speculative_bytes = 0;
        if !backend.can_retire(task, &operation) {
            let row = self.tasks.get_mut(&task).unwrap();
            row.log.push_front(Entry {
                before,
                operation,
                committed: None,
                turn: row.retiring,
            });
            row.state = State::Pending;
            return Ok(Progress::NeedSave(task));
        }
        self.apply(task, before, operation, None, backend)
    }

    /// A newly selected head absorbs its existing log before running any new
    /// operation. An early receipt is taken over, never committed a second time.
    pub(crate) fn absorb_status(&self) -> Option<Progress> {
        if self.stopped {
            Some(Progress::Stopped)
        } else if self.polling.is_some() {
            Some(Progress::Poll)
        } else if let Some(task) = self.head {
            self.tasks[&task]
                .log
                .is_empty()
                .then_some(Progress::NeedSegment(task))
        } else {
            Some(Progress::Idle)
        }
    }

    pub fn absorb<B: Backend<P, Result = R>>(
        &mut self,
        backend: &mut B,
    ) -> Result<Progress, BudgetKind> {
        if let Some(progress) = self.absorb_status() {
            return Ok(progress);
        }
        let task = self.head.unwrap();
        let entry = self.tasks.get_mut(&task).unwrap().log.pop_front().unwrap();
        if entry.committed.is_none() && !backend.can_retire(task, &entry.operation) {
            self.tasks.get_mut(&task).unwrap().log.push_front(entry);
            return Ok(Progress::NeedSave(task));
        }
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
            backend.record_budget_stop(task, &before, self.retired, kind);
            self.stopped = true;
            return Err(kind);
        }
        let early = committed.is_some();
        let receipt = committed.unwrap_or_else(|| backend.commit(task, &operation, false));
        self.settled.level = self.settled.level.saturating_sub(receipt.credit);
        backend.record_retirement(
            task,
            &operation,
            &receipt.result,
            Retirement {
                index: self.retired,
                before: &before,
                transition: &receipt.next,
                credit: receipt.credit,
            },
        );
        if early {
            backend.publish(task, &operation, &receipt.result);
        }
        self.transition(task, receipt.next, backend)
    }

    /// Resume a waiting op or a called child's cleanup without inventing a
    /// second op/ordinal or an extra scheduling point. The native continuation
    /// effect is immediate, head-only, and runs after its stretch settles.
    pub fn continue_head<B: Backend<P, Result = R>>(
        &mut self,
        before: Stretch,
        backend: &mut B,
        effect: impl FnOnce() -> Next,
    ) -> Result<Progress, BudgetKind> {
        assert!(!self.stopped);
        assert!(self.polling.is_none(), "complete the retirement poll first");
        let task = self.head.expect("a continuation has a head");
        assert!(self.tasks[&task].log.is_empty(), "absorb before continuing");
        if let Err(kind) = self.settled.settle(&before) {
            backend.record_budget_stop(task, &before, self.retired, kind);
            self.stopped = true;
            return Err(kind);
        }
        self.tasks.get_mut(&task).unwrap().open_speculative_bytes = 0;
        let next = effect();
        backend.record_continuation(
            task,
            Retirement {
                index: self.retired,
                before: &before,
                transition: &next,
                credit: 0,
            },
        );
        self.transition(task, next, backend)
    }

    fn transition<B: Backend<P, Result = R>>(
        &mut self,
        task: TaskId,
        next: Next,
        backend: &mut B,
    ) -> Result<Progress, BudgetKind> {
        match next {
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
            next => Ok(self.finish_turn(task, next, backend)),
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
            backend.call_member(task, child);
            if self.tasks[&child].state != State::Holding {
                continue;
            }
            backend.call(task, child);
            self.tasks.get_mut(&task).unwrap().state = State::Parked;
            self.tasks.get_mut(&child).unwrap().state = State::Queued;
            self.held.get_mut(&child).unwrap().caller = Some(task);
            self.head = Some(child);
            // Calls preserve the current eligible snapshot. After a yield in
            // a child's rest, callers' remaining holding members still count.
            return Progress::Called(child);
        }
        let (_, then_yield) = self.tasks.get_mut(&task).unwrap().calls.take().unwrap();
        backend.call_returned(task);
        if then_yield {
            self.finish_turn(task, Next::Yield, backend)
        } else {
            Progress::Absorbed(task)
        }
    }

    fn finish_turn<B: Backend<P, Result = R>>(
        &mut self,
        task: TaskId,
        next: Next,
        backend: &mut B,
    ) -> Progress {
        assert!(self.polling.is_none(), "one retirement poll at a time");
        let row = self.tasks.get_mut(&task).unwrap();
        row.retiring += 1;
        row.producing = row.producing.max(row.retiring);
        // A log may span future turns. Their allocated storage remains part
        // of the physical allowance until those entries have been absorbed.
        if row.log.is_empty() {
            self.physical -= row.speculative_bytes - row.open_speculative_bytes;
            row.speculative_bytes = row.open_speculative_bytes;
        }
        row.state = match next {
            Next::Yield => State::Queued,
            Next::Park => State::Parked,
            Next::Hold => State::Holding,
            Next::End => State::Done,
            _ => unreachable!("only scheduling transitions finish a turn"),
        };
        self.head = None;
        self.polling = Some((task, next));
        if backend.defer_poll() {
            Progress::Poll
        } else {
            self.complete_poll(backend.poll())
        }
    }

    /// Complete exactly one retirement's poll after restoring the policy into
    /// its role-owned slot. Wake order precedes the yielding task's requeue;
    /// only then is the next head and rule-1 eligibility snapshot selected.
    pub fn complete_poll(&mut self, woken: Vec<TaskId>) -> Progress {
        let (task, next) = self.polling.take().expect("a deferred retirement poll");
        assert!(!self.stopped, "poll completion after a terminal prefix");
        for woken in woken {
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
        Progress::Retired
    }
}

#[cfg(test)]
pub(crate) mod runtime50_model;
#[cfg(test)]
mod tests;
