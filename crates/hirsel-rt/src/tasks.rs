//! Structured tasks with a cooperative run token (CONC-1, CONC-2, CONC-3, FAIL-2).
//!
//! The scheduler is cooperative and single-threaded: every task is a [`fiber`](crate::fiber)
//! with a [`TaskCtx`] of its own at the top of its stack (the root's is the run's first
//! context), and a switch happens only at a capability operation, a join or a completion. A
//! switch moves the worker from one task's context to the next (`TaskCtx::switch_out`,
//! `TaskCtx::switch_in`): what is a task's own (its failure chain, finalizer shield, release
//! worklist, innermost region, isolate scratch and foreign scope) stays in its context. All
//! tasks share the scheduler, while the run token may move between workers and heaps. The checker's
//! sendability rules (MODE-4, OWN-6) already keep every counted value owned by one task, so a
//! value crossing tasks is moved by copying its root.
//!
//! The schedule is the reference interpreter's, so both observe the same interleaving:
//!
//! - one queue of runnable tasks; a spawned child joins its back;
//! - after every capability operation ([`yield_now`]) the running task, when another is
//!   runnable, joins the back and the run's scheduler policy ([`pick`]) chooses the task to
//!   run: under `fifo@1` the front, under a seeded policy (`hirsel_kernels::schedule`) any,
//!   which may be the running task itself;
//! - a task that must wait (a join, a scope exit) parks with what it waits for ([`Wait`]).
//!   Scheduling points are every yield and whenever the running task stops (it parked, holds
//!   its result or completed). At each, the parked tasks whose wait may be over are polled:
//!   a task that just parked, and those an event woke ([`Scheduler::wake`]): a child that
//!   holds its result or completed wakes its joiner, its scope's exit and its sets' waits, a
//!   send or a closed channel wakes as many receivers as it can serve, a cancellation wakes
//!   the canceled tasks, and the adapter reports the host requests that are ready. Each point
//!   thus costs what happened, never the number of parked tasks, and polls exactly the tasks
//!   whose poll could succeed, so the schedule is the one polling every parked task gives.
//!   Timers go in deadline order (ties in park order), other waits in park order. A
//!   successful poll performs the wait's effect and moves the task to the back of the queue.
//!   An operation that parks yields first, and its task parks when it is next at the front.
//!
//! A child runs the compiled `$native.task_main` closure the spawn received, which drops the
//! callable (and so its captures) once it returns. On success that body hands its `Some(result)`
//! slot to [`hirsel_rt_v2_task_finish`] and stays in that call: a completed child holds its
//! result in its own frame. A join's poll moves the slot's whole `Option` image into the
//! awaiter's slot and writes the child's `None` back; a discard resumes the child with its slot
//! intact, and the child's own compiled cleanup drops the result. The runtime therefore moves results without knowing their
//! layouts. Either way the child then finishes its frame, the runtime releases its closure
//! environment, and the child completes: completion follows cleanup and precedes the join
//! (CONC-2). A failed child completes with its failure chain instead, which cancels its
//! siblings and its scope (cause `ChildFailed`).
//!
//! Handles are run-local: a Scope or Task word is a table index with a generation, so a stale
//! or foreign word is an invariant fault rather than another task's row.

pub(crate) mod compact;
pub(crate) mod entered_yield;
pub(crate) mod free;
pub(crate) mod ordered;
#[cfg(feature = "measurement")]
pub(crate) mod profile;
mod readiness;
pub(crate) mod registry;
pub mod role;

use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

use hirsel_free::phase::Phase;
use hirsel_kernels::schedule::{Algorithm, Schedule};

use crate::context::TaskCtx;
use crate::failure::Chained;
use crate::fiber::{self, Stack};
use crate::pool::Owner;
use crate::stop::{Stop, Stopped};
use crate::{Header, STATUS_FAILURE, STATUS_OK, fault};
use registry::TaskKey;

/// A spawned task's entry: the unit-result case of cross-unit ABI 3. It has
/// no value word, so it returns bare status rather than a scalar result pair.
/// The two-scalar callable carries this code pointer and its environment.
pub type TaskCode = unsafe extern "C" fn(*mut TaskCtx, *mut Header, u64, u64) -> u8;

/// `CancelCause` ordinals (core library).
pub(crate) const CANCEL_REQUESTED: u32 = 0;
pub(crate) const CANCEL_PARENT: u32 = 1;
pub(crate) const CANCEL_CHILD_FAILED: u32 = 2;
pub(crate) const CANCEL_DEADLINE: u32 = 3;

/// The root task: the entry, on the thread's own stack.
const ROOT: u32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    /// Running or in the runnable queue.
    Ready,
    /// In the parked list, or resuming a child it handed off to.
    Parked,
    /// Completed with a value it holds in its `$native.task_main` frame.
    Holding,
    /// Completed: its value was delivered or dropped, or it failed.
    Done,
}

/// What a parked task waits for; its poll performs the wait's effect.
#[derive(Clone, Copy)]
enum Wait {
    /// A join of `child`, which moves its result into `slot` (`size` bytes).
    Join {
        child: u32,
        slot: *mut u8,
        size: u64,
    },
    /// A scope exit, until every child of `scope` holds its result or completed.
    Exit { scope: u32 },
    /// A receive on `channel`, which moves a message into `slot` (CONC-6).
    Receive { channel: u32, slot: *mut u8 },
    /// Native Free receive: only readiness is polled; its resumed owner moves
    /// the image and publishes Done through its own recorder.
    ReceiveReady { channel: u32 },
    /// A host request that would block, parked under its readiness token
    /// (`crate::host_waits`); its poll re-issues the request.
    Host { token: u64 },
    /// `task.await_any`, until a member of task set `set` holds its result or failed; in a
    /// free-mode replay until the recorded `member` (an add-order index) has.
    Any { set: u32, member: Option<u64> },
    /// `task.cancel_all`, until every member of task set `set` completed.
    Drain { set: u32 },
    /// Free mode: parked for good, at its cut or at a phase the cut does not admit.
    Final,
    /// Free mode: replay-pending (or a live wait on another task's op), until the next
    /// publish; a `cancelable` one (a receive) also until its task is canceled.
    Retry { cancelable: bool },
    /// An outermost `gate.enter` (S-13): until `serial` gate `gate` is free, which the poll
    /// then takes.
    Gate { gate: u32 },
    /// A blocking extern call, until a blocking-pool thread is free to take it.
    Pool,
}

/// A `serial` package's foreign gate (S-13, design §6 Extern dispatch): a logical object of
/// the run that one call chain holds at a time, from its outermost `gate.enter` to its
/// outermost `gate.leave`. Its owner's nested calls to the package re-enter by depth.
#[derive(Default)]
struct ForeignGate {
    owner: Option<u32>,
    depth: u32,
    /// Bumped by each outermost entry and leave: the order the tasks won the gate.
    version: u64,
    /// Tasks parked on [`Wait::Gate`] for it.
    waiters: Vec<u32>,
}

/// A completed child's result slot, inside its parked frame, and the `None`
/// image a join leaves behind, which its row owns ([`Task::held_none`]).
#[derive(Clone, Copy)]
struct Slot {
    slot: *mut u8,
    none: *const u8,
    size: usize,
}

impl Slot {
    /// The slot of a result at `slot`, with a copy of its `None` carrier for
    /// the ending task's row: the carrier may lie on a stack its compact
    /// frame gives up (`compact::can_hold`). Allocates, so it runs before
    /// any recorder section.
    ///
    /// # Safety
    /// `none` is readable for `size` bytes.
    unsafe fn held(slot: *mut u8, none: *const u8, size: u64) -> (Self, Box<[u8]>) {
        let size =
            usize::try_from(size).unwrap_or_else(|_| fault("a result slot's size overflows"));
        // SAFETY: caller contract.
        let image: Box<[u8]> = unsafe { std::slice::from_raw_parts(none, size) }.into();
        let none = image.as_ptr();
        (Self { slot, none, size }, image)
    }
}

struct Task {
    generation: u32,
    #[cfg(feature = "ownership-trace")]
    ownership_queue: Option<crate::ownership_trace::QueueBinding>,
    /// Stable control block, created at the first lease. Holders keep the
    /// slot from reuse; they never reach this row or its context.
    control: Option<registry::TaskLease>,
    /// Waiters subscribed to this task's cancellation.
    cancel_subs: Vec<registry::CancelSub>,
    host_cancel: Option<registry::CancelRegistration>,
    /// Role-owned notification metadata. Remote wakes never read a running
    /// TaskCtx, whose local runtime entries may hold an exclusive borrow.
    dispatch: Option<std::sync::Arc<crate::pool::free::Frame>>,
    /// Its context: the root's is the run's first, a child's lies at the top of its stack
    /// until its first compact save moves it to `context_home`.
    ctx: *mut TaskCtx,
    stack: Option<Stack>,
    /// A compact task's context, once its first stack is gone. Its stack, if any, is lent.
    context_home: Option<Box<compact::ContextHome>>,
    /// While its resumable body runs: the bytes of its compact frame image.
    compact_frame: Option<(usize, usize)>,
    /// Its result is held for its parent from a saved compact frame: the
    /// reissued task end continues after the hold (`compact::hold`).
    hold_left: bool,
    /// The `None` image its [`Slot`] leaves behind, freed with the row.
    held_none: Option<Box<[u8]>>,
    /// Fast children own a stable context independently of the lent fiber.
    inline_context: Option<Box<std::cell::UnsafeCell<TaskCtx>>>,
    /// Parent destination while an immediate child runs on its parent's fiber.
    inline_result: Option<(*mut u8, u64)>,
    /// An op-free child's trivially droppable result, already off its frame.
    inline_value: Option<(Vec<u128>, usize)>,
    inline_running: bool,
    state: State,
    /// The scope it was spawned into (the root has none).
    scope: Option<u32>,
    path: Vec<u32>,
    spawned: u32,
    canceled: hirsel_free::cancel::Flag,
    cancel_cache: hirsel_free::cancel::Cache,
    /// Actual spawning parent and position in its child list. A task can be
    /// spawned into a scope owned by another task. Retirement promotes its
    /// live descendants before this row can be reused.
    parent: Option<(u32, usize)>,
    children: Vec<u32>,
    /// Role-owned finalizer-shield snapshot. Remote cancellation/poll readers
    /// never borrow TaskCtx while its owner executes task-local runtime code.
    shield: u32,
    /// Scopes this task has entered and not yet exited, innermost last.
    scopes: Vec<u32>,
    /// Entered Free scope exits retain their cursor and detached runtime rows
    /// independently of the fiber. Nested runtime calls use distinct records.
    scope_exits: Vec<free::lifecycle::scope_exit::Operation>,
    scope_entries: Vec<free::lifecycle::scope_enter::Entry>,
    scope_exit_generation: u64,
    code: Option<(TaskCode, *mut Header, u64)>,
    slot: Option<Slot>,
    /// What the task waits for while parked.
    wait: Option<Wait>,
    wait_site: u64,
    /// The status a receive's poll produced.
    status: u32,
    /// The task to resume directly when this one completes (a join or a discard ran it).
    handoff: Option<u32>,
    detached: bool,
    failure: Option<Vec<Chained>>,
    /// Its place in completion order (CONC-2), set when it holds its result or failed.
    completed: u64,
    /// While parked: its place in park order, and whether it is to be polled at the next
    /// scheduling point.
    parked_at: u64,
    woken: bool,
    /// The task parked joining it.
    awaited_by: Option<u32>,
    /// Free mode: a publish since it parked on [`Wait::Retry`].
    retried: bool,
    /// Free mode: its end op ran (when it came to hold its result, else at completion).
    ended: bool,
    /// The child left its last runtime frame; only idle save may publish Done
    /// and recycle the stack. A join must not retire a still-running stack.
    finishing: bool,
    /// Its scheduling priority under `pct@1`, drawn when it was created.
    priority: u64,
    /// The threads that may resume it (`crate::pool`): any compute worker, the home thread
    /// (a home foreign interval, or the run's Io binding in use) or the blocking-pool
    /// thread its interval runs on.
    owner: Owner,
    /// The task belongs to the active closed CPU batch.
    cpu_scoped: bool,
    cpu_job: Option<std::sync::Arc<crate::pool::cpu_scoped::probe::Job>>,
    /// It is the home thread's only until its next suspension, for the run's Io binding.
    pinned: bool,
    /// The compute worker it last ran on, at width > 1.
    last: Option<u16>,
}

/// A task set (CONC-2): its scope and its members' Task words in add order, `None` once
/// awaited.
struct TaskSet {
    generation: u32,
    scope: u32,
    members: Vec<Option<u64>>,
}

/// Task set words are run-local endpoint words with this bit too, so their drop's handle close
/// reaches the scheduler; channel endpoint generations stay below it.
pub(crate) const TASK_SET: u64 = 1 << 62;

struct Scope {
    generation: u32,
    /// The task whose body runs in the scope.
    owner: u32,
    children: Vec<u32>,
    canceled: hirsel_free::cancel::Flag,
    /// `task.scope_until`'s deadline (CONC-3).
    deadline: Option<ScopeDeadline>,
    /// Completion order belongs to the parent scope, including children that
    /// finish before a TaskSet exists or before their handles enter it.
    completions: AtomicU64,
}

/// A scope's deadline: its armed timer's host token until it fires or the scope exits.
struct ScopeDeadline {
    token: Option<u64>,
    expired: bool,
}

/// Logical scheduling is independent of saved-stack ownership and workers.
#[derive(Default)]
enum Logical {
    #[default]
    Cooperative,
    Free(free::Driver),
    Ordered(ordered::driver::Driver),
}

/// Every task and scope of one run, and the runnable queue.
#[derive(Default)]
pub(crate) struct Scheduler {
    logical: Logical,
    cancel_epoch: std::sync::Arc<hirsel_free::cancel::Epoch>,
    /// New cancellation causes, excluding scope topology changes. An empty
    /// observation stays empty when an uncanceled scope is entered or left.
    cancel_causes: std::sync::Arc<hirsel_free::cancel::Epoch>,
    /// The cancellation generation home last rechecked saved host waits for.
    home_epoch: u64,
    cancel_metrics: Option<std::sync::Arc<crate::pool::WorkerMetrics>>,
    tasks: Vec<Option<Task>>,
    task_slots: registry::TaskSlots,
    scopes: Vec<Option<Scope>>,
    scope_slots: registry::Slots<{ u32::MAX }>,
    current: u32,
    runnable: VecDeque<u32>,
    /// Park places handed out so far; a parked task holds its place in `Task::parked_at`.
    parks: u64,
    /// Parked tasks to poll at the next scheduling point.
    woken: Vec<u32>,
    /// The storage of the woken list a scheduling point is polling, and of the channels it
    /// serves, kept across points so a point allocates nothing.
    polling: Vec<u32>,
    serving: Vec<u32>,
    /// Tasks parked in `task.await_any` or `task.cancel_all`.
    set_waiters: Vec<u32>,
    pub(crate) channels: crate::channels::Channels,
    sync: crate::sync::Synchronizers,
    sets: Vec<Option<TaskSet>>,
    set_slots: registry::Slots<SET_GENERATIONS>,
    /// The run's scheduler policy (`inputs.scheduler`).
    schedule: Schedule,
    /// The `serial` packages' gates, by package ordinal (S-13).
    gates: Vec<ForeignGate>,
    /// Tasks parked on [`Wait::Pool`].
    pool_waiters: Vec<u32>,
    /// Replay enablement and live free-mode object waits.
    retrying: Vec<u32>,
    /// Cancellation subscriptions a cancellation touched, until drained.
    host_waiters: std::collections::BTreeSet<u32>,
    cancel_notices: Vec<registry::CancelNotice>,
    cancel_registrations: u64,
}

/// TaskSet generations stay below the endpoint and TaskSet word bits.
const SET_GENERATIONS: u32 = 0x3fff_ffff;

fn handle(index: u32, generation: u32) -> u64 {
    u64::from(generation) << 32 | u64::from(index)
}

impl Scheduler {
    fn new(mut schedule: Schedule, root_ctx: *mut TaskCtx) -> Scheduler {
        let mut root = Task::new(1, root_ctx, None, Vec::new(), None);
        root.priority = schedule.admit();
        Scheduler {
            tasks: vec![Some(root)],
            schedule,
            ..Scheduler::default()
        }
    }
    fn task(&mut self, index: u32) -> &mut Task {
        self.tasks[index as usize]
            .as_mut()
            .unwrap_or_else(|| fault("a vacant task row"))
    }
    fn scope(&mut self, index: u32) -> &mut Scope {
        self.scopes[index as usize]
            .as_mut()
            .unwrap_or_else(|| fault("a vacant scope row"))
    }
    /// A task slot for a new row and its generation. The slot's previous
    /// row has left and every owner of its storage has released it.
    #[inline]
    fn take_task_slot(&mut self) -> (u32, u32) {
        let (index, generation) = self.task_slots.take(self.tasks.len());
        if index as usize == self.tasks.len() {
            self.tasks.push(None);
        }
        (index, generation)
    }
    #[inline]
    fn take_scope_slot(&mut self) -> (u32, u32) {
        let (index, generation) = self.scope_slots.take(self.scopes.len());
        if index as usize == self.scopes.len() {
            self.scopes.push(None);
        }
        (index, generation)
    }
    #[inline]
    fn take_set_slot(&mut self) -> (u32, u32) {
        let (index, generation) = self.set_slots.take(self.sets.len());
        if index as usize == self.sets.len() {
            self.sets.push(None);
        }
        (index, generation)
    }
    /// Remove scope row `index`; its slot is reusable at once.
    #[inline]
    fn take_scope(&mut self, index: u32) -> Scope {
        let row = self.scopes[index as usize]
            .take()
            .unwrap_or_else(|| fault("a scope exited twice"));
        self.scope_slots.release(index, row.generation);
        row
    }
    /// The live row's key, if `slot` holds one.
    pub(crate) fn task_key(&self, slot: u32) -> Option<registry::TaskKey> {
        let row = self.tasks.get(slot as usize)?.as_ref()?;
        Some(registry::TaskKey {
            slot,
            generation: u64::from(row.generation),
        })
    }
    /// Live row `index`'s key, as the policy that owns it names it to dispatch.
    pub(crate) fn key(&self, index: u32) -> registry::TaskKey {
        self.task_key(index)
            .unwrap_or_else(|| fault("a key of a vacant task row"))
    }
    /// The row `key` names while it is live; never a later occupant.
    pub(crate) fn live_task(&self, key: registry::TaskKey) -> Option<u32> {
        (self.task_key(key.slot) == Some(key)).then_some(key.slot)
    }
    pub(crate) fn scope_key(&self, slot: u32) -> Option<registry::ScopeKey> {
        let row = self.scopes.get(slot as usize)?.as_ref()?;
        Some(registry::ScopeKey {
            slot,
            generation: u64::from(row.generation),
        })
    }
    pub(crate) fn live_scope(&self, key: registry::ScopeKey) -> Option<u32> {
        (self.scope_key(key.slot) == Some(key)).then_some(key.slot)
    }
    /// A retained reference to live task `slot`'s control block. Its slot is
    /// not reused until the lease drops, though the row may retire.
    pub(crate) fn lease(&mut self, slot: u32) -> registry::TaskLease {
        let key = self
            .task_key(slot)
            .unwrap_or_else(|| fault("a lease of a vacant task row"));
        self.task(slot)
            .control
            .get_or_insert_with(|| registry::TaskLease::new(key))
            .clone()
    }
    /// Subscribe `token` to live task `task`'s cancellation. The result
    /// carries the post-registration recheck, so an earlier cancellation is
    /// never missed.
    pub(crate) fn subscribe_cancel(
        &mut self,
        task: u32,
        token: u64,
    ) -> registry::CancelRegistration {
        let key = self
            .task_key(task)
            .unwrap_or_else(|| fault("a cancellation subscription of a vacant task row"));
        self.cancel_registrations = self
            .cancel_registrations
            .checked_add(1)
            .unwrap_or_else(|| fault("cancellation registration counter exhausted"));
        let registration = self.cancel_registrations;
        self.task(task).cancel_subs.push(registry::CancelSub {
            token,
            registration,
        });
        let shield = self.task(task).shield();
        let canceled = self.canceled_task(task, shield);
        if canceled.is_some() {
            // Already canceled: no later event names this registration, so
            // its service drains the notice like any other.
            self.cancel_notices.push(registry::CancelNotice {
                task: key,
                token,
                registration,
            });
        }
        registry::CancelRegistration {
            key,
            registration,
            canceled,
        }
    }
    /// Remove a subscription; a retired row's subscriptions left with it.
    pub(crate) fn unsubscribe_cancel(&mut self, task: registry::TaskKey, registration: u64) {
        if let Some(slot) = self.live_task(task) {
            self.task(slot)
                .cancel_subs
                .retain(|sub| sub.registration != registration);
        }
    }
    /// The subscriptions cancellations touched since the last drain, in
    /// cancellation order. Untouched subscriptions are never visited.
    pub(crate) fn drain_cancel_notices(&mut self, out: &mut Vec<registry::CancelNotice>) {
        out.append(&mut self.cancel_notices);
    }
    /// As [`Self::drain_cancel_notices`], for the notices whose token has
    /// `token & mask == value` only; other services' notices stay queued.
    pub(crate) fn drain_cancel_notices_matching(
        &mut self,
        mask: u64,
        value: u64,
        out: &mut Vec<registry::CancelNotice>,
    ) {
        self.cancel_notices.retain(|notice| {
            let matched = notice.token & mask == value;
            if matched {
                out.push(*notice);
            }
            !matched
        });
    }
    /// A cancellation touched `task`: queue its subscriptions' notices.
    /// The notice capacity was reserved with the cancellation's targets.
    fn notify_cancel(&mut self, task: u32) {
        let Some(Some(row)) = self.tasks.get(task as usize) else {
            return;
        };
        let key = registry::TaskKey {
            slot: task,
            generation: u64::from(row.generation),
        };
        for sub in &row.cancel_subs {
            self.cancel_notices.push(registry::CancelNotice {
                task: key,
                token: sub.token,
                registration: sub.registration,
            });
        }
    }
    /// A prepared request is not yet a parked event source. Only a saved
    /// host waiter authorizes home readiness without an armed scope timer.
    fn has_saved_host_waiter(&self) -> bool {
        !self.host_waiters.is_empty()
    }
    /// Drain only cancellation events addressed to this host service. The
    /// registration check rejects delayed notices after retry or row reuse.
    fn host_cancellations(&mut self, mask: u64, value: u64) -> Vec<u64> {
        let mut notices = Vec::new();
        self.drain_cancel_notices_matching(mask, value, &mut notices);
        notices
            .into_iter()
            .filter_map(|notice| {
                let task = self.live_task(notice.task)?;
                let row = self.tasks[task as usize].as_ref()?;
                let registration = row.host_cancel?;
                if registration.key != notice.task
                    || registration.registration != notice.registration
                    || !matches!(row.wait, Some(Wait::Host {token}) if token == notice.token)
                {
                    return None;
                }
                self.canceled_task(task, row.shield()).map(|_| notice.token)
            })
            .collect()
    }
    /// The task a Task word names, which must belong to `scope`.
    fn task_handle(&mut self, word: u64, scope: u32) -> u32 {
        let index = word as u32;
        match self.tasks.get(index as usize).and_then(Option::as_ref) {
            Some(task) if task.generation == (word >> 32) as u32 && task.scope == Some(scope) => {
                index
            }
            _ => fault("a Task handle of another scope or a completed run"),
        }
    }
    fn scope_handle(&mut self, word: u64) -> u32 {
        let index = word as u32;
        match self.scopes.get(index as usize).and_then(Option::as_ref) {
            Some(scope) if scope.generation == (word >> 32) as u32 => index,
            _ => fault("a Scope handle outside its callback"),
        }
    }
    /// The running task's cancellation: its own, else its innermost canceled scope's.
    fn canceled(&mut self, ctx_shield: u32) -> Option<u32> {
        let current = self.current;
        self.canceled_task(current, ctx_shield)
    }
    /// `task`'s cancellation, with `shield` its finalizer shield (the context's while it runs).
    fn canceled_task(&self, task: u32, shield: u32) -> Option<u32> {
        canceled_in(
            &self.tasks,
            &self.scopes,
            task,
            shield,
            self.cancel_epoch.load(),
            self.cancel_metrics.as_deref(),
        )
    }
    /// Cancel `task` with `cause` and its descendants with `Parent`; an earlier cause stays.
    /// Each canceled parked task is woken: its cancellation may end its wait.
    fn cancel(&mut self, task: u32, cause: u32) {
        let targets = self.prepare_cancel(task);
        self.cancel_prepared(task, cause, &targets);
    }
    /// Freeze the linked-child walk and reserve all wake capacity before a
    /// recording section. The role protects this topology until publication.
    fn prepare_cancel(&mut self, task: u32) -> Vec<u32> {
        // Go context's linked-child walk, rather than scanning unrelated run
        // rows. Links stay until the child row is vacated, including scope exit.
        let mut pending = vec![task];
        let mut targets = Vec::new();
        let mut subs = 0;
        while let Some(index) = pending.pop() {
            targets.push(index);
            let row = self.tasks[index as usize].as_ref().unwrap();
            subs += row.cancel_subs.len();
            pending.extend_from_slice(&row.children);
        }
        self.woken.reserve(targets.len());
        self.cancel_notices.reserve(subs);
        targets
    }
    fn cancel_prepared(&mut self, task: u32, cause: u32, targets: &[u32]) {
        let mut changed = false;
        for &index in targets {
            changed |=
                self.task(index)
                    .canceled
                    .cancel(if index == task { cause } else { CANCEL_PARENT });
            self.wake(index);
            self.notify_cancel(index);
        }
        if changed {
            self.cancel_causes.changed();
            self.cancel_epoch.changed();
        }
    }
    fn cancel_scope(&mut self, scope: u32, cause: u32) {
        if self.scope(scope).canceled.cancel(cause) {
            self.cancel_causes.changed();
            self.cancel_epoch.changed();
            // Only the owner enters a scope; children are canceled by name.
            let owner = self.scope(scope).owner;
            self.notify_cancel(owner);
        }
    }
    /// Poll parked `task` at the next scheduling point: an event may have ended its wait.
    fn wake(&mut self, task: u32) {
        wake_in(&mut self.tasks, &mut self.woken, task);
    }
    /// `scope` was canceled: its owner's cancellation may end the owner's wait.
    fn wake_owner(&mut self, scope: u32) {
        let owner = self.scope(scope).owner;
        self.wake(owner);
    }
    /// `child` holds its result or completed: wake its joiner, its scope's exit and the set
    /// waits of its scope.
    fn settle(&mut self, child: u32) {
        let row = self.task(child);
        let (awaiter, scope) = (row.awaited_by, row.scope);
        if let Some(awaiter) = awaiter {
            self.wake(awaiter);
        }
        let Some(scope) = scope else { return };
        let owner = self.scope(scope).owner;
        if matches!(self.task(owner).wait, Some(Wait::Exit { scope: s }) if s == scope) {
            self.wake(owner);
        }
        for i in 0..self.set_waiters.len() {
            let waiter = self.set_waiters[i];
            if let Some(Wait::Any { set, .. } | Wait::Drain { set }) = self.task(waiter).wait
                && self.sets[set as usize]
                    .as_ref()
                    .is_some_and(|row| row.scope == scope)
            {
                self.wake(waiter);
            }
        }
    }
    /// For each channel a send or a sender's close touched: wake the receivers its poll
    /// serves in park order, as many as it holds messages (the canceled ones, which a
    /// cancellation woke, take none), or every one once no sender is left.
    fn wake_receivers(&mut self) {
        let mut touched = std::mem::take(&mut self.serving);
        self.channels.take_touched(&mut touched);
        for &channel in &touched {
            let Some((queued, closed, waiting)) = self.channels.waiting(channel) else {
                continue;
            };
            let mut served = 0;
            for &(_, task) in waiting {
                if !closed && served == queued {
                    break;
                }
                let shield = self.tasks[task as usize].as_ref().map_or(0, |t| t.shield());
                if canceled_in(
                    &self.tasks,
                    &self.scopes,
                    task,
                    shield,
                    self.cancel_epoch.load(),
                    self.cancel_metrics.as_deref(),
                )
                .is_none()
                {
                    served += 1;
                }
                wake_in(&mut self.tasks, &mut self.woken, task);
            }
        }
        touched.clear();
        self.serving = touched;
    }
    /// The running task parks on `wait`: it is polled at the next scheduling point, and
    /// registered where the events that can end its wait find it.
    fn park(&mut self, wait: Wait) {
        self.park_task(self.current, wait);
    }
    /// [`Self::park`] for `current`, which need not be the role's task: a
    /// role-free object park's save callback parks its saved row.
    fn park_task(&mut self, current: u32, wait: Wait) {
        let at = self.parks;
        self.parks += 1;
        let row = self.task(current);
        row.state = State::Parked;
        row.wait = Some(wait);
        row.parked_at = at;
        row.woken = true;
        self.woken.push(current);
        if matches!(self.logical, Logical::Free(_))
            && let Wait::Host { token } = wait
        {
            let registration = self.subscribe_cancel(current, token);
            assert!(
                self.task(current)
                    .host_cancel
                    .replace(registration)
                    .is_none()
            );
        }
        match wait {
            Wait::Join { child, .. } => self.task(child).awaited_by = Some(current),
            Wait::Receive { channel, .. } => self.channels.wait(channel, at, current),
            // Its wake token, registered under the channel's latch, alone
            // ends a Free receive's wait; its poll observes cancellation.
            Wait::ReceiveReady { .. } => {}
            Wait::Any { .. } | Wait::Drain { .. } => self.set_waiters.push(current),
            Wait::Gate { gate } => self.gate(gate).waiters.push(current),
            Wait::Pool => self.pool_waiters.push(current),
            Wait::Exit { .. } | Wait::Host { .. } | Wait::Final | Wait::Retry { .. } => {}
        }
    }
    /// The parked tasks (exactly those with a wait) and their waits, in park order. Only
    /// quiescence and test reporting walk them, so no index is kept.
    fn parked(&self) -> Vec<(u32, Wait)> {
        let mut parked: Vec<_> = self
            .tasks
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let row = row.as_ref()?;
                Some((row.parked_at, index as u32, row.wait?))
            })
            .collect();
        parked.sort_unstable_by_key(|&(at, _, _)| at);
        parked
            .into_iter()
            .map(|(_, task, wait)| (task, wait))
            .collect()
    }
    /// A poll ended parked `task`'s wait: it leaves every registration and joins the queue.
    fn unpark(&mut self, task: u32) {
        self.end_wait(task);
        self.runnable.push_back(task);
    }
    /// Make parked `task` ready and unregister its wait, without queueing it.
    fn end_wait(&mut self, task: u32) {
        self.host_waiters.remove(&task);
        if let Some(registration) = self.task(task).host_cancel.take() {
            self.unsubscribe_cancel(registration.key, registration.registration);
        }
        let row = self.task(task);
        let (at, wait) = (row.parked_at, row.wait.take());
        row.state = State::Ready;
        match wait {
            Some(Wait::Join { child, .. }) => {
                if let Some(Some(child)) = self.tasks.get_mut(child as usize) {
                    child.awaited_by = None;
                }
            }
            Some(Wait::Receive { channel, .. }) => self.channels.unwait(channel, at),
            Some(Wait::Any { .. } | Wait::Drain { .. }) => self.set_waiters.retain(|&t| t != task),
            Some(Wait::Gate { gate }) => self.gate(gate).waiters.retain(|&t| t != task),
            Some(Wait::Pool) => self.pool_waiters.retain(|&t| t != task),
            _ => {}
        }
    }
    /// `serial` gate `gate`, created free at its first use.
    fn gate(&mut self, gate: u32) -> &mut ForeignGate {
        let index = gate as usize;
        if self.gates.len() <= index {
            self.gates.resize_with(index + 1, ForeignGate::default);
        }
        &mut self.gates[index]
    }
    /// `scope`'s deadline fired (CONC-3): the scope is canceled `DeadlineExceeded`, as is each
    /// of its children, and the children of the scopes its owner entered inside it `Parent`.
    pub(crate) fn expire(&mut self, scope: u32) {
        let row = self.scope(scope);
        let Some(deadline) = &mut row.deadline else {
            fault("a scope without a deadline expired");
        };
        if deadline.expired {
            return;
        }
        deadline.expired = true;
        deadline.token = None;
        let (owner, children) = (row.owner, row.children.clone());
        self.cancel_scope(scope, CANCEL_DEADLINE);
        self.wake(owner);
        for child in children {
            self.cancel(child, CANCEL_DEADLINE);
        }
        let inner: Vec<u32> = self
            .task(owner)
            .scopes
            .iter()
            .skip_while(|&&s| s != scope)
            .skip(1)
            .copied()
            .collect();
        for inner in inner {
            for child in self.scope(inner).children.clone() {
                self.cancel(child, CANCEL_PARENT);
            }
        }
    }
    /// Whether every child of `scope` holds its result or completed.
    fn settled(&mut self, scope: u32) -> bool {
        let children = self.scope(scope).children.clone();
        children.iter().all(|&c| {
            let row = self.task(c);
            matches!(row.state, State::Holding | State::Done)
                && row.dispatch.as_ref().is_none_or(|frame| frame.is_saved())
        })
    }
    /// Vacate task set row `set`; its members stay children of their scope.
    fn close_set(&mut self, set: u32) {
        drop(self.take_set(set));
    }
    /// Remove task set row `set`; its slot is reusable at once.
    fn take_set(&mut self, set: u32) -> TaskSet {
        let row = self.sets[set as usize]
            .take()
            .unwrap_or_else(|| fault("a task set closed twice"));
        self.set_slots.release(set, row.generation);
        row
    }
    /// `task` holds its result or failed: its place in completion order.
    fn order(&mut self, task: u32) {
        let scope = self
            .task(task)
            .scope
            .expect("a completed child has a scope");
        let order = self
            .scope(scope)
            .completions
            .fetch_add(1, Ordering::Relaxed);
        if order == u64::MAX {
            fault("a scope's completion rank overflowed");
        }
        self.task(task).completed = order;
    }
    /// The task set a TaskSet word names, which must belong to `scope`.
    fn set_handle(&mut self, word: u64, scope: u32) -> u32 {
        let index = word as u32;
        match self.sets.get(index as usize).and_then(Option::as_ref) {
            Some(set)
                if word & TASK_SET != 0
                    && set.generation
                        == ((word & !(crate::channels::ENDPOINT | TASK_SET)) >> 32) as u32
                    && set.scope == scope =>
            {
                index
            }
            _ => fault("a TaskSet handle of another scope or a closed set"),
        }
    }
    fn set(&mut self, index: u32) -> &mut TaskSet {
        self.sets[index as usize]
            .as_mut()
            .unwrap_or_else(|| fault("a vacant task set row"))
    }
    /// The row a member's Task word names while the child has one.
    fn member(&self, word: u64) -> Option<u32> {
        let index = word as u32;
        self.tasks
            .get(index as usize)
            .and_then(Option::as_ref)
            .is_some_and(|t| t.generation == (word >> 32) as u32)
            .then_some(index)
    }
    /// The add-order index of the unawaited member of `set` that completed first, if one has.
    fn first_completed(&mut self, set: u32) -> Option<u64> {
        let members = self.set(set).members.clone();
        members
            .iter()
            .enumerate()
            .filter_map(|(i, word)| {
                let row = self.tasks[self.member((*word)?)? as usize].as_ref()?;
                matches!(row.state, State::Holding | State::Done)
                    .then_some((row.completed, i as u64))
            })
            .min()
            .map(|(_, i)| i)
    }
    /// Whether member `index` of `set` is unawaited and holds its result or failed.
    fn member_completed(&mut self, set: u32, index: u64) -> bool {
        let word = usize::try_from(index)
            .ok()
            .and_then(|i| self.set(set).members.get(i).copied().flatten());
        word.and_then(|word| self.member(word))
            .is_some_and(|t| matches!(self.task(t).state, State::Holding | State::Done))
    }
    fn vacate_task(&mut self, index: u32) {
        drop(self.retire_task(index));
    }
    /// Detach the row without destroying owned storage in a recording section.
    fn retire_task(&mut self, index: u32) -> Task {
        self.host_waiters.remove(&index);
        let task = self.tasks[index as usize]
            .take()
            .unwrap_or_else(|| fault("a task completed twice"));
        if task.stack.is_some() || task.context_home.is_some() {
            fault("a task left its rows before it completed");
        }
        // Tokio cancellation_token/tree_node.rs:201-239 at ce685ee6916549ed:
        // remove this node and promote its children to its parent. The role
        // replaces Tokio's node locks; indices replace Arc-owned nodes.
        if let Some((parent, pos)) = task.parent {
            let children = &mut self.task(parent).children;
            assert_eq!(children.swap_remove(pos), index);
            if let Some(&moved) = children.get(pos) {
                self.task(moved).parent = Some((parent, pos));
            }
            for &child in &task.children {
                let pos = self.task(parent).children.len();
                self.task(child).parent = Some((parent, pos));
                self.task(parent).children.push(child);
            }
        } else {
            assert!(
                task.children.is_empty(),
                "a root retired with live children"
            );
        }
        if let Some(job) = &task.cpu_job {
            job.event("retire");
        }
        self.task_slots.retire(
            index,
            task.generation,
            task.control.as_ref(),
            task.dispatch.as_ref(),
        );
        task
    }
    /// Capacity for `additional` task retirements inside a recording section.
    fn reserve_retirements(&mut self, additional: usize) {
        self.task_slots.reserve(additional);
    }

    fn link_child(&mut self, parent: u32, child: u32) {
        let pos = self.task(parent).children.len();
        assert!(self.task(child).parent.replace((parent, pos)).is_none());
        self.task(parent).children.push(child);
    }
}

impl Task {
    fn new(
        generation: u32,
        ctx: *mut TaskCtx,
        scope: Option<u32>,
        path: Vec<u32>,
        canceled: Option<u32>,
    ) -> Task {
        #[cfg(feature = "ownership-trace")]
        if !ctx.is_null() {
            // This context is still exclusively owned at construction. The
            // diagnostic key stays immutable across execution and compact saves.
            unsafe {
                (*ctx).ownership_task = if path.is_empty() {
                    0
                } else {
                    handle((*ctx).task_id, generation)
                }
            };
        }
        Task {
            #[cfg(feature = "ownership-trace")]
            ownership_queue: None,
            generation,
            control: None,
            cancel_subs: Vec::new(),
            host_cancel: None,
            dispatch: None,
            ctx,
            stack: None,
            context_home: None,
            compact_frame: None,
            hold_left: false,
            held_none: None,
            inline_context: None,
            inline_result: None,
            inline_value: None,
            inline_running: false,
            state: State::Ready,
            scope,
            path,
            spawned: 0,
            canceled: canceled.into(),
            cancel_cache: Default::default(),
            parent: None,
            children: Vec::new(),
            shield: 0,
            scopes: Vec::new(),
            scope_exits: Vec::new(),
            scope_entries: Vec::new(),
            scope_exit_generation: 0,
            code: None,
            slot: None,
            wait: None,
            wait_site: 0,
            status: 0,
            handoff: None,
            detached: false,
            failure: None,
            completed: 0,
            parked_at: 0,
            woken: false,
            awaited_by: None,
            retried: false,
            ended: false,
            finishing: false,
            priority: 0,
            owner: Owner::Any,
            cpu_scoped: false,
            cpu_job: None,
            pinned: false,
            last: None,
        }
    }
}

impl Task {
    /// Its shield at the last role handoff, without accessing its context.
    fn shield(&self) -> u32 {
        self.shield
    }
}

impl Task {
    /// Exclusive/quiescent Saved owner only. This performs no release or charge.
    fn restore_ahead_custody(&mut self) {
        unsafe {
            if let Some(local) = (*self.ctx).ahead.as_mut() {
                assert!(
                    local.native.is_none()
                        || self.dispatch.as_ref().is_none_or(|frame| frame.is_saved()),
                    "discard before Saved acknowledgement"
                );
                local.restore(self.generation);
            }
        }
    }
}

impl Scheduler {
    /// Before wholesale heap reclamation, while every captured mask is still
    /// mapped. Promoted rows already adopted custody and need no restoration.
    /// The caller owns quiescent run metadata; reservations are bounded by8.
    pub(crate) fn restore_reserved_custody(&mut self) {
        ordered::ahead::restore_reserved_custody(self);
    }
}

impl Drop for Task {
    /// A task that still has its stack or context home (it never completed: its run stopped
    /// or ended) drops the context lying there; a completed task's context gave up its
    /// contents at its departure.
    fn drop(&mut self) {
        if self.stack.is_some() || self.context_home.is_some() {
            self.restore_ahead_custody();
            // SAFETY: the context lies in this task's stack or home, which it outlives no longer.
            unsafe { std::ptr::drop_in_place(self.ctx) };
        }
    }
}

/// `task`'s cancellation, with `shield` its finalizer shield: its own, else its innermost
/// canceled scope's.
fn canceled_in(
    tasks: &[Option<Task>],
    scopes: &[Option<Scope>],
    task: u32,
    shield: u32,
    epoch: u64,
    metrics: Option<&crate::pool::WorkerMetrics>,
) -> Option<u32> {
    if shield > 0 {
        return None;
    }
    let task = tasks[task as usize]
        .as_ref()
        .unwrap_or_else(|| fault("a vacant task row"));
    let mut depth = 0;
    let cause = task.cancel_cache.get(epoch, || {
        if let Some(cause) = task.canceled.cause() {
            return Some(cause);
        }
        for &scope in task.scopes.iter().rev() {
            if metrics.is_some() {
                depth += 1;
            }
            if let Some(cause) = scopes[scope as usize]
                .as_ref()
                .and_then(|s| s.canceled.cause())
            {
                return Some(cause);
            }
        }
        None
    });
    if let Some(metrics) = metrics {
        metrics.cancellation(depth);
    }
    cause
}

/// Mark parked `task` to be polled at the next scheduling point, once.
fn wake_in(tasks: &mut [Option<Task>], woken: &mut Vec<u32>, task: u32) {
    if let Some(Some(row)) = tasks.get_mut(task as usize)
        && row.state == State::Parked
        && row.wait.is_some()
        && !row.woken
    {
        row.woken = true;
        woken.push(task);
    }
}

/// Test fixtures' broad access to the run's scheduler. Production code
/// borrows the tables only lexically, through [`role::with_role`].
#[cfg(test)]
pub(crate) fn scheduler(ctx: *mut TaskCtx) -> &'static mut Scheduler {
    role::unlent("a broad scheduler access");
    role_tables(ctx)
}

/// Enter the role and reach the tables, creating them with the root task at
/// its first scope, for [`role::with_role`] and the test fixtures' accessor.
#[inline(always)]
fn role_tables(ctx: *mut TaskCtx) -> &'static mut Scheduler {
    // SAFETY: callers retain no scheduler reference across a role leave.
    unsafe { role::enter(ctx) };
    // Admission also builds row zero before code starts; only code entries
    // may abandon a fiber here. Observe a host stop before a synchronizing
    // entry touches the object, rather than after its effect at yield.
    if crate::stop::running() == ctx {
        unsafe {
            crate::stop::check(ctx, crate::stop::observe(ctx));
        }
    }
    // SAFETY: the context and its run are live for the whole run, and the scheduler is only
    // reached through them, by the one running task; no reference is kept across a switch.
    match unsafe { (&mut *(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() } {
        Some(s) => s,
        None => create_scheduler(ctx),
    }
}

/// The tables of a role the caller already holds, for [`role::with_held`].
/// # Safety
/// As for [`role::with_held`].
#[inline(always)]
unsafe fn held_tables(ctx: *mut TaskCtx) -> &'static mut Scheduler {
    // SAFETY: the held role exclusively owns the tables; see role_tables.
    match unsafe { (&mut *(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() } {
        Some(s) => s,
        None => fault("a held role without its tables"),
    }
}

/// A finalizer changed this owner's shield. Publish while its semantic entry
/// owns the role (or W1's cooperative token); do not create a scheduler here.
/// # Safety
/// `ctx` is the calling task's, and no scheduler borrow survives this call.
pub(crate) unsafe fn shield_changed(ctx: *mut TaskCtx) {
    // SAFETY: caller owns this context and the run's scheduler role/token.
    unsafe {
        if let Some(scheduler) = (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() {
            snapshot_shield(scheduler, ctx);
        }
    }
}

/// Publish the current owner's task-local shield while the role is held.
/// A completed task may already have vacated its row before its final leave.
/// # Safety
/// `ctx` belongs to the role owner, so reading its task-local fields is legal.
unsafe fn snapshot_shield(s: &mut Scheduler, ctx: *mut TaskCtx) {
    // SAFETY: caller owns this task and the scheduler role.
    unsafe {
        if let Some(Some(row)) = s.tasks.get_mut((*ctx).task_id as usize)
            && row.ctx == ctx
        {
            row.shield = (*ctx).shield;
        }
    }
}

/// [`role_tables`]' first call, kept out of every caller.
#[cold]
#[inline(never)]
fn create_scheduler(ctx: *mut TaskCtx) -> &'static mut Scheduler {
    // SAFETY: as for `scheduler`.
    unsafe {
        let run = (*ctx).run_ptr();
        let schedule = (*run).schedule.clone();
        let scheduler = (*(*run).tasks.get()).insert(Box::new(Scheduler::new(schedule, ctx)));
        snapshot_shield(scheduler, ctx);
        scheduler
    }
}

/// The running task.
pub(crate) fn current_task(ctx: *mut TaskCtx) -> u32 {
    role::with_role(ctx, |t| t.current)
}

/// `task`'s cancellation, with its own finalizer shield.
pub(crate) fn task_canceled(ctx: *mut TaskCtx, task: u32) -> Option<u32> {
    role::with_role(ctx, |t| {
        let shield = t.task(task).shield();
        t.canceled_task(task, shield)
    })
}

/// RUN-9: a callee run's place forwards its caller's cancellation: the run's root task is
/// canceled with `cause` and every other task with `Parent`, and the parked ones are polled.
pub(crate) fn cancel_root(ctx: *mut TaskCtx, cause: u32) {
    role::with_role(ctx, |t| t.cancel(ROOT, cause));
}

/// Outstanding waits for the test runner. This is host reporting, outside program Io.
pub(crate) fn test_waits(ctx: *mut TaskCtx) -> String {
    let parked = role::with_role(ctx, |t| {
        t.parked()
            .into_iter()
            .map(|(task, wait)| (t.task(task).path.clone(), wait))
            .collect::<Vec<_>>()
    });
    let mut waits: Vec<_> = parked
        .into_iter()
        .map(|(path, wait)| {
            let kind = match wait {
                Wait::Join { .. } => "task.await",
                Wait::Exit { .. } => "task.scope",
                Wait::Receive { .. } | Wait::ReceiveReady { .. } => "channel.receive",
                Wait::Host { token } => unsafe { crate::host_waits::kind(ctx, token) },
                Wait::Any { .. } => "task.await_any",
                Wait::Drain { .. } => "task.cancel_all",
                Wait::Final => "free.final",
                Wait::Retry { .. } => "free.retry",
                Wait::Gate { .. } => "foreign.gate",
                Wait::Pool => "foreign.pool",
            };
            (path, kind)
        })
        .collect();
    waits.sort_by(|a, b| a.0.cmp(&b.0));
    waits
        .into_iter()
        .map(|(path, kind)| format!("\n  task {path:?}: {kind}"))
        .collect()
}

/// Free mode: park the running task for good (`crate::free`). The run then ends Blocked
/// unless other tasks finish it.
pub(crate) fn park_final(ctx: *mut TaskCtx) -> ! {
    // SAFETY: the context is live; no poll ends this wait.
    let site = unsafe { (*ctx).operation_site };
    unsafe { block(&mut *ctx, Wait::Final, false, site) };
    fault("a task parked for good was resumed")
}

/// Free mode: park the running task until the next publish (or, `cancelable`, its
/// cancellation); the caller checks its condition again.
pub(crate) fn park_retry(ctx: *mut TaskCtx, cancelable: bool) {
    role::with_role(ctx, |t| {
        let current = t.current;
        if !t.retrying.contains(&current) {
            t.retrying.push(current);
        }
    });
    // SAFETY: the context is live; the caller holds nothing a stop must drop.
    unsafe { block(&mut *ctx, Wait::Retry { cancelable }, false, 0) };
}

/// Free mode: a publish happened; `tasks` parked replay-pending are polled at the next
/// scheduling point.
pub(crate) fn wake_all(ctx: *mut TaskCtx, tasks: &[u32]) {
    role::with_role(ctx, |t| t.wake_retried(tasks));
}

impl Scheduler {
    fn wake_retried(&mut self, tasks: &[u32]) {
        for &task in tasks {
            if let Some(Some(row)) = self.tasks.get_mut(task as usize)
                && matches!(row.wait, Some(Wait::Retry { .. }))
            {
                row.retried = true;
                self.wake(task);
            }
        }
    }
}

/// A phase publication can enable object or replay waiters. They recheck
/// outside the publishing phase, and never hold an object while waiting.
pub(crate) fn wake_retrying(ctx: *mut TaskCtx) {
    role::with_role(ctx, |t| {
        let retrying = std::mem::take(&mut t.retrying);
        t.wake_retried(&retrying);
    });
}

/// The running task's spawn path.
pub(crate) fn current_path(ctx: *mut TaskCtx) -> Vec<u32> {
    role::with_role(ctx, |t| {
        let current = t.current;
        t.task(current).path.clone()
    })
}

/// Copy immutable issuer metadata while the caller owns the role/run token.
/// Unlike current_path this introduces no scheduler creation or stop observation.
/// # Safety
/// The caller owns its task and scheduler role; no borrow crosses the home loan.
pub(crate) unsafe fn issuer_path(ctx: *mut TaskCtx) -> Vec<u32> {
    unsafe {
        (*(*(*ctx).run_ptr()).tasks.get())
            .as_deref()
            .map(|s| {
                s.tasks[(*ctx).task_id as usize]
                    .as_ref()
                    .unwrap()
                    .path
                    .clone()
            })
            .unwrap_or_default()
    }
}

/// Suspend the running task and run `next`. The running task must already be queued, parked
/// or holding; it returns from here when something resumes it, on whichever worker that is.
///
/// # Safety
/// `ctx` is the running task's, inside an operation whose own entry has
/// already taken the role; `next` is a suspended or prepared task.
#[inline(never)]
unsafe fn switch_to(ctx: &mut TaskCtx, next: u32) {
    // SAFETY: the operation holds the role and its tables exist.
    let next_row = unsafe {
        role::with_held(ctx, |t| {
            let from = t.current;
            // A suspension ends the running task's hold on the home thread for the Io binding.
            if let Some(Some(row)) = t.tasks.get_mut(from as usize) {
                unpin(row);
            }
            (from != next).then(|| {
                let row = t.task(next);
                (row.owner, row.last, row.ctx)
            })
        })
    };
    let Some((owner, last, to_ctx)) = next_row else {
        // SAFETY: caller contract.
        unsafe { migrate_self(ctx) };
        return;
    };
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.switch();
    }
    if let Some(to) = crate::pool::target(ctx, owner, last) {
        // `next` runs on another thread: this one hands the run over and waits (`crate::pool`).
        let permit = crate::pool::resumes_into_hirsel(to_ctx, to);
        ctx.switch_out();
        let save = &raw mut ctx.task_sp;
        // SAFETY: `next` is suspended; `save` is written before any other thread runs.
        unsafe { crate::pool::hand_off(ctx, save, to, next, permit) };
        recycle(ctx);
        return;
    }
    let save = &raw mut ctx.task_sp;
    let to = unsafe { (*to_ctx).task_sp };
    ctx.switch_out();
    // SAFETY: no scheduler borrow is used after transferring the role.
    unsafe {
        role::release(ctx);
        role::enter(to_ctx);
    }
    // SAFETY: `next`'s context is live and nothing else runs on this worker.
    unsafe { TaskCtx::switch_in(to_ctx, ctx.worker_ptr()) };
    // SAFETY: `next` entered the role above.
    unsafe { ran_here(to_ctx, next) };
    // SAFETY: `to` is the suspended or prepared stack pointer of `next`.
    unsafe { fiber::switch(save, to) };
    unsafe { role::enter(ctx) };
    recycle(ctx);
}

/// End `row`'s hold on the home thread for the Io binding, at its suspension.
fn unpin(row: &mut Task) {
    if std::mem::take(&mut row.pinned) {
        row.owner = Owner::Any;
    }
}

/// Task `task` becomes the running task on the calling thread: at width > 1, on a compute
/// worker, it records the worker (counting a migration when it last ran on another).
/// # Safety
/// `ctx` is `task`'s, switched in, and holds the role; its run's tables exist.
unsafe fn ran_here(ctx: *mut TaskCtx, task: u32) {
    // SAFETY: caller contract; the run outlives its tasks.
    let run = unsafe { (*ctx).run_ref() };
    let here = if crate::pool::wide(run) {
        crate::pool::compute_index()
    } else {
        None
    };
    // SAFETY: caller contract.
    unsafe {
        role::with_held(ctx, |t| {
            t.current = task;
            record_worker(t.task(task), here, run.worker_metrics.as_deref())
        })
    }
}

fn record_worker(row: &mut Task, here: Option<u16>, metrics: Option<&crate::pool::WorkerMetrics>) {
    let Some(here) = here else { return };
    if row.last.is_some_and(|last| last != here)
        && let Some(metrics) = metrics
    {
        metrics.migration();
    }
    row.last = Some(here);
}

/// The running task goes on running after a scheduling point: on another thread when its
/// owner says so (it left the home thread, or the migrate-every-suspension mode moves it).
///
/// # Safety
/// `ctx` is the running task's; the caller holds nothing a stop must drop.
unsafe fn migrate_self(ctx: &mut TaskCtx) {
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::engine::migrate(ctx)) };
        return;
    }
    if free::enabled(ctx) {
        unsafe { free::migrate(ctx) };
        return;
    }
    let Some((me, owner, last)) = role::with_role(ctx, |t| {
        let me = t.current;
        let row = t.tasks.get(me as usize)?.as_ref()?;
        Some((me, row.owner, row.last))
    }) else {
        return;
    };
    let Some(to) = crate::pool::target(ctx, owner, last) else {
        return;
    };
    let permit = crate::pool::resumes_into_hirsel(ctx, to);
    ctx.switch_out();
    let save = &raw mut ctx.task_sp;
    // SAFETY: this task is suspended at `save` before the other thread resumes it.
    unsafe { crate::pool::hand_off(ctx, save, to, me, permit) };
    recycle(ctx);
}

/// The running task holds the run's Io binding, home state, until its next suspension: at
/// width > 1 it moves to the home thread, unless it runs there already or sits in a foreign
/// interval of another thread (whose callbacks reach the binding where they run). A stopped
/// run's home thread takes no task: the running one is left there, never resumed.
///
/// # Safety
/// `ctx` is the running task's and no borrow of its scheduler is held.
pub(crate) unsafe fn pin_home(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    let ctx = unsafe { &mut *ctx };
    let run = ctx.run_ref();
    if !crate::pool::wide(run)
        || crate::pool::compute_index().is_none()
        || crate::foreign::active()
        || run.tasks.is_none()
    {
        return;
    }
    let pinned = role::with_role(ctx, |t| {
        let me = t.current;
        let Some(Some(row)) = t.tasks.get_mut(me as usize) else {
            return false;
        };
        if row.owner != Owner::Any {
            return false;
        }
        row.owner = Owner::Home;
        row.pinned = true;
        true
    });
    if pinned {
        // SAFETY: caller contract.
        unsafe { migrate_self(ctx) };
    }
}

/// The running task's Io operation ended without a scheduling point (a handle's close): its
/// hold on the home thread ends here, so the home thread runs no ordinary Hirsel code.
///
/// # Safety
/// As for [`pin_home`]; the caller holds nothing a stop must drop.
pub(crate) unsafe fn leave_home(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    let ctx = unsafe { &mut *ctx };
    if !crate::pool::wide(ctx.run_ref()) || ctx.run_ref().tasks.is_none() {
        return;
    }
    let unpinned = role::with_role(ctx, |t| {
        let me = t.current;
        match t.tasks.get_mut(me as usize) {
            Some(Some(row)) if row.pinned => {
                unpin(row);
                true
            }
            _ => false,
        }
    });
    if unpinned {
        // SAFETY: caller contract.
        unsafe { migrate_self(ctx) };
    }
}

/// Run `call` on the home thread for the running task, which goes back to a compute worker
/// after it: the root's report of a returned error, home state at width > 1. A stopped
/// run's home thread takes no task: the running one is left there, never resumed.
///
/// # Safety
/// `ctx` is the running task's; `call` does not suspend.
pub(crate) unsafe fn at_home(ctx: *mut TaskCtx, call: &mut dyn FnMut()) {
    // SAFETY: caller contract.
    unsafe {
        // After a stop the root reports nothing.
        crate::stop::check(ctx, crate::stop::observe(ctx));
        pin_home(ctx);
        call();
        role::with_role(ctx, |t| {
            let me = t.current;
            unpin(t.task(me));
        });
        migrate_self(&mut *ctx);
    }
}

/// The running task enters a home foreign interval (S-13): at width > 1 it moves to the home
/// thread, which alone resumes it until [`from_home`]. Whether it moved.
///
/// # Safety
/// `ctx` is the running task's, outside any foreign interval; it holds nothing a stop must
/// drop.
pub(crate) unsafe fn to_home(ctx: &mut TaskCtx) -> bool {
    if !crate::pool::wide(ctx.run_ref()) {
        return false;
    }
    role::with_role(ctx, |t| {
        let me = t.current;
        let row = t.task(me);
        row.owner = Owner::Home;
        row.pinned = false;
    });
    // SAFETY: caller contract.
    unsafe { migrate_self(ctx) };
    true
}

/// The running task's home foreign interval ended: it goes back to a compute worker.
///
/// # Safety
/// `ctx` is the running task's, on the home thread, after its interval's leave.
pub(crate) unsafe fn from_home(ctx: &mut TaskCtx) {
    role::with_role(ctx, |t| {
        let me = t.current;
        t.task(me).owner = Owner::Any;
    });
    // SAFETY: caller contract.
    unsafe { migrate_self(ctx) };
}

/// Start a width > 1 run's root task as row 0 on the first compute worker, suspended at `sp`
/// (its prepared fiber).
///
/// # Safety
/// `ctx` is the run's root context; the run has compute workers and has not started.
pub(crate) unsafe fn start_root(ctx: *mut TaskCtx, sp: *mut u8) {
    let (root, key) = role::with_role(ctx, |t| (t.task(ROOT).ctx, t.key(ROOT)));
    unsafe { (*root).task_sp = sp };
    // SAFETY: caller contract.
    unsafe {
        (*ctx).switch_out();
        role::release(ctx);
        crate::pool::start_root((*ctx).run_ptr(), key);
    }
}

/// Leave a completed task for good and run `next`. The completed task's row may already be
/// vacant, so nothing of it is saved; its context state is empty, and it gives up what it
/// holds, since the context lies in a stack the next spawn reuses.
///
/// # Safety
/// `ctx` is the running task's, `next` is suspended and the running fiber is never resumed.
#[inline(never)]
unsafe fn depart_to(ctx: &mut TaskCtx, next: u32) -> ! {
    // `next` is Ready before this task gives up the role, as at a hand-off.
    // SAFETY: the completion holds the role and its tables exist.
    let (owner, last, to_ctx) = unsafe {
        role::with_held(ctx, |t| {
            let row = t.task(next);
            row.state = State::Ready;
            (row.owner, row.last, row.ctx)
        })
    };
    if ctx.failure.active() || ctx.shield != 0 || !ctx.work.is_null() || !ctx.region.is_null() {
        fault("a task completed with context state left");
    }
    // Nothing on this abandoned stack may own storage at the switch.
    drop(std::mem::take(&mut ctx.failure));
    drop(std::mem::take(&mut ctx.frozen));
    drop(std::mem::take(&mut ctx.isolated));
    drop(std::mem::take(&mut ctx.isolations));
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.switch();
    }
    if let Some(to) = crate::pool::target(ctx, owner, last) {
        let permit = crate::pool::resumes_into_hirsel(to_ctx, to);
        ctx.switch_out();
        let mut abandoned = std::ptr::null_mut();
        // SAFETY: `next` is suspended; nothing resumes the abandoned stack pointer.
        unsafe { crate::pool::hand_off(ctx, &raw mut abandoned, to, next, permit) };
        fault("a completed task was resumed");
    }
    let to = unsafe { (*to_ctx).task_sp };
    ctx.switch_out();
    // SAFETY: the completed fiber retains no shared borrow.
    unsafe {
        role::release(ctx);
        role::enter(to_ctx);
    }
    // SAFETY: `next`'s context is live and nothing else runs on this worker.
    unsafe { TaskCtx::switch_in(to_ctx, ctx.worker_ptr()) };
    // SAFETY: `next` entered the role above.
    unsafe { ran_here(to_ctx, next) };
    let mut abandoned = std::ptr::null_mut();
    // SAFETY: `next` is suspended; nothing resumes the abandoned stack pointer.
    unsafe { fiber::switch(&raw mut abandoned, to) };
    fault("a completed task was resumed")
}

/// The context and stack pointer of suspended task `task`, which the calling thread resumes
/// next (`crate::pool`): it becomes the running task.
///
/// # Safety
/// `run` is live and has its scheduler; `task` is suspended.
pub(crate) unsafe fn resume_row(
    run: *mut crate::context::RunShared,
    task: u32,
    here: Option<u16>,
) -> (*mut TaskCtx, *mut u8) {
    // SAFETY: the task is suspended; claim protects its table lookup from a
    // concurrent spawn before the context's stable pointer is known.
    let ctx = unsafe { role::claim(run, task) };
    // SAFETY: caller contract; the thread resuming it is the only one running the run.
    let (s, metrics) = unsafe {
        (
            (*(*run).tasks.get())
                .as_deref_mut()
                .unwrap_or_else(|| fault("a handoff in a run without tasks")),
            (*run).worker_metrics.as_deref(),
        )
    };
    s.current = task;
    let row = s.task(task);
    record_worker(row, here, metrics);
    (ctx, unsafe { (*ctx).task_sp })
}

/// Stable physical-dispatch identity: the frame and context of the task
/// `key` names, or `None` when the key is stale. The caller already owns the
/// scheduler role; acquiring a different task's role here would deadlock a
/// notifier.
///
/// # Safety
/// The run is live and the caller owns its role or root admission.
pub(crate) unsafe fn dispatch_frame(
    run: *mut crate::context::RunShared,
    key: registry::TaskKey,
) -> Option<(
    &'static mut Option<std::sync::Arc<crate::pool::free::Frame>>,
    *mut TaskCtx,
)> {
    unsafe {
        let s = (*(*run).tasks.get()).as_deref_mut().unwrap();
        let task = ordered::ahead::physical(s, key)?;
        Some((&mut task.dispatch, task.ctx))
    }
}

/// Whether published row `task`'s frame is saved (`pool::free::is_saved`).
/// # Safety
/// The caller owns the role; `task` is a live row it has just read.
pub(crate) unsafe fn frame_saved(run: *mut crate::context::RunShared, task: u32) -> bool {
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    s.task(task)
        .dispatch
        .as_ref()
        .is_none_or(|frame| frame.is_saved())
}

/// The running task's key.
pub(crate) fn running_key(ctx: *mut TaskCtx) -> registry::TaskKey {
    role::with_role(ctx, |t| t.key(t.current))
}

pub(crate) fn cpu_job_entry(ctx: *mut TaskCtx, width: u32, ordinal: u64) {
    let Some(probe) = (unsafe { (*ctx).run_ref().cpu_probe.as_ref() }) else {
        return;
    };
    role::with_role(ctx, |s| {
        let me = s.current;
        let scope = s.task(me).scope.expect("CPU mapper scope");
        let batch = handle(scope, s.scope(scope).generation);
        let key = s.key(me).word();
        let row = s.task(me);
        if row.cpu_job.is_none() {
            row.cpu_job = Some(probe.job(key, batch, width, ordinal, &row.path));
        }
        row.cpu_job.as_ref().unwrap().enter(width, ordinal);
    });
}

fn cpu_job_event(row: &Task, kind: &str) {
    if let Some(job) = &row.cpu_job {
        job.event(kind);
    }
}

pub(crate) fn cpu_job_claim(ctx: *mut TaskCtx) {
    role::with_role(ctx, |s| {
        let me = s.current;
        cpu_job_event(s.task(me), "claim");
    });
}

pub(crate) fn dispatch_owner(ctx: *mut TaskCtx) -> Owner {
    if ordered::ahead::private(ctx) {
        return Owner::Any;
    }
    role::with_role(ctx, |t| {
        let me = t.current;
        t.task(me).owner
    })
}

/// The scheduler role owns scoped membership and addressed affinity. It never
/// stores a pointer to a caller's runtime frame.
pub(crate) fn cpu_scoped(ctx: *mut TaskCtx) -> bool {
    role::with_role(ctx, |s| s.task(unsafe { (*ctx).task_id }).cpu_scoped)
}

/// Ordinary continuations that can become canonical head during a CPU batch.
/// Their existing executors need retained credits, independently of CPU jobs.
pub(crate) fn cpu_control_owners(ctx: *mut TaskCtx) -> Vec<Owner> {
    role::with_role(ctx, |s| {
        let mut owners = Vec::new();
        for (index, row) in s.tasks.iter().enumerate() {
            if let Some(row) = row
                && index != unsafe { (*ctx).task_id } as usize
                && !row.cpu_scoped
                && matches!(row.state, State::Ready | State::Parked)
                && !owners.contains(&row.owner)
            {
                owners.push(row.owner);
            }
        }
        owners
    })
}

pub(crate) fn cpu_owner(ctx: *mut TaskCtx, owner: Owner, member: bool) -> Owner {
    role::with_role(ctx, |s| {
        let row = s.task(unsafe { (*ctx).task_id });
        row.cpu_scoped = member;
        std::mem::replace(&mut row.owner, owner)
    })
}

fn inherit_cpu(ctx: *mut TaskCtx, task: &mut Task) {
    if cpu_scoped(ctx) {
        task.cpu_scoped = true;
        task.owner = crate::pool::cpu_scoped::next_owner(ctx);
    }
}

/// Record a physical claim without invoking a stop-aware task entry before
/// the idle worker has installed its Running token.
/// # Safety
/// The idle claimant owns the scheduler role and the exclusive saved image.
pub(crate) unsafe fn record_dispatch(
    run: *mut crate::context::RunShared,
    task: registry::TaskKey,
    here: Option<u16>,
) {
    let run = unsafe { &*run };
    let s = unsafe { (*run.tasks.get()).as_deref_mut().unwrap() };
    record_worker(
        ordered::ahead::physical(s, task).unwrap_or_else(|| fault("a claimed frame's row left")),
        here,
        run.worker_metrics.as_deref(),
    );
}

/// A role-free object wake (`pool::free::notify`) publishes a registered
/// waiter without its logical unpark. Its dispatch ends that object wait here,
/// under the role it takes to start the task, as the policy's unpark would;
/// the waiter's loop checks its condition again. `false`: the task is parked
/// on a wait no object wake ends, or holds its result until its parent's
/// hand-off (which alone makes it Ready), so a stale or remembered wake woke
/// it; or `key` no longer names its slot's row. The caller returns its frame
/// unstarted.
/// # Safety
/// The caller holds the scheduler role and is starting `key`'s frame.
pub(crate) unsafe fn resumed(run: *mut crate::context::RunShared, key: registry::TaskKey) -> bool {
    let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    if ordered::ahead::physical(s, key).is_none() {
        return false;
    }
    let task = key.slot;
    if !matches!(s.logical, Logical::Free(_)) {
        return true;
    }
    let Some(Some(row)) = s.tasks.get_mut(task as usize) else {
        return true;
    };
    if row.state == State::Holding {
        return false;
    }
    if row.state != State::Parked {
        return true;
    }
    if !matches!(
        row.wait,
        Some(Wait::Retry { .. } | Wait::ReceiveReady { .. })
    ) {
        return false;
    }
    row.retried = false;
    // A polled row must still be parked: drop a pending poll of this wait.
    if std::mem::take(&mut row.woken) {
        s.woken.retain(|&woken| woken != task);
    }
    s.end_wait(task);
    true
}

/// A physical notification token became idle after its scheduler save callback,
/// or a stale wake returned unstarted. Recheck only after that transition: two
/// concurrent save callbacks can each see the other's still-running token and
/// defer CONC-8, leaving no later semantic operation to publish Blocked.
/// # Safety
/// The caller is on an idle stack and holds no role or runtime borrow; its
/// dispatch still holds the frame's Lease, whose drop wakes home.
pub(crate) unsafe fn saved_idle(run: *mut crate::context::RunShared) {
    let _role = unsafe { role::IdleGuard::idle(run) };
    if unsafe { (*run).control.stopped().is_some() } {
        return;
    }
    unsafe { free::check_quiet(run, None) };
}

/// # Safety
/// Admission only; no worker has started a task.
pub(crate) unsafe fn install_cancel_metrics(ctx: *mut TaskCtx) {
    let metrics = unsafe { (*ctx).run_ref() }.worker_metrics.clone();
    role::with_role(ctx, |t| t.cancel_metrics = metrics);
}

/// A physical stack reached idle, before its dispatch activity reservation
/// is released. Under the idle role this publishes its saved flag. Policies
/// may now expose parked slots and settle completion, including vacating a
/// completed row. Neither the callback nor its guard borrows that context.
///
/// # Safety
/// The calling idle worker retains the run and no role/runtime borrow. A
/// completed image was invalidated before this call; a suspended image is
/// still owned by its idle continuation until this callback returns.
pub(crate) unsafe fn after_save(
    run: *mut crate::context::RunShared,
    key: registry::TaskKey,
    completed: bool,
) {
    unsafe { saved(run, key, completed, std::ptr::null_mut()) };
}

/// [`after_save`] for a role-free object park (`free::park_registered`):
/// under the same idle role, before the frame's notification state settles,
/// `key`'s still running row parks on the wait `ctx` left, as `try_block`
/// would have parked it, with `ctx`'s shield. A wake that beat this save is
/// remembered on the frame and its dispatch ends the wait (`resumed`). The
/// owner its frame resumes on, `None` when the row is gone.
///
/// # Safety
/// As for [`after_save`]; `ctx` is the suspended image the caller owns.
pub(crate) unsafe fn after_park(
    run: *mut crate::context::RunShared,
    key: registry::TaskKey,
    ctx: *mut TaskCtx,
) -> Option<Owner> {
    unsafe { saved(run, key, false, ctx) }
}

/// The save callback; a non-null `parked` is a role-free object park's image.
unsafe fn saved(
    run: *mut crate::context::RunShared,
    key: registry::TaskKey,
    completed: bool,
    parked: *mut TaskCtx,
) -> Option<Owner> {
    // SAFETY: the worker stays busy and retains its Lease throughout this
    // guard; it cannot be reclaimed even if a callback vacates this task.
    let _role = unsafe { role::IdleGuard::idle(run) };
    let mut resume_on = None;
    let ready = {
        // SAFETY: IdleGuard exclusively owns role-protected task metadata.
        let Some(s) = (unsafe { (*(*run).tasks.get()).as_deref_mut() }) else {
            return resume_on;
        };
        // SAFETY: the idle continuation owns the suspended image.
        let wait = (!parked.is_null())
            .then(|| unsafe { free::objects::parked(parked) }.expect("a role-free park's wait"));
        if let Some((wait, site)) = wait
            && let Some(row) = ordered::ahead::physical(s, key)
        {
            assert!(
                row.state == State::Ready && row.wait.is_none(),
                "a role-free park's row runs"
            );
            row.wait_site = site;
            unpin(row);
            resume_on = Some(row.owner);
            // SAFETY: the idle continuation owns the suspended image.
            unsafe { snapshot_shield(s, parked) };
            s.park_task(key.slot, wait);
        }
        // The saved frame pins its slot (`registry::TaskSlots::retire`): a
        // later occupant here is a broken retirement, not a stale wake.
        if s.tasks
            .get(key.slot as usize)
            .and_then(Option::as_ref)
            .is_some_and(|row| u64::from(row.generation) != key.generation)
        {
            fault("a saved frame's slot was reused before its save");
        }
        let task = key.slot;
        if let Some(ready) = ordered::ahead::after_save(s, key, completed, unsafe {
            (*run).control.stopped().is_some()
        }) {
            let mut batch = unsafe { crate::pool::free::ReadyBatch::new(run) };
            for (task, owner) in ready {
                unsafe { batch.schedule(task, owner) };
            }
            return resume_on;
        }
        if let Some(Some(row)) = s.tasks.get(task as usize)
            && let Some(frame) = &row.dispatch
        {
            frame.mark_saved();
            if matches!(row.wait, Some(Wait::Host { .. })) {
                s.host_waiters.insert(task);
            }
        }
        // The flag and policy callback share one role acquisition: no join
        // can observe completion, vacate this row, and reuse its id in between.
        if unsafe { (*run).control.stopped().is_some() } {
            return resume_on;
        }
        match s.logical {
            Logical::Cooperative => return resume_on,
            Logical::Free(_) => unsafe {
                free::after_save(s, task, completed, crate::pool::has_slot_run(run))
            },
            Logical::Ordered(_) => ordered::driver::after_save(
                s,
                task,
                completed,
                unsafe {
                    (*run).host_waits.load(std::sync::atomic::Ordering::Acquire) != 0
                        || ordered::host::concurrent::pending(run)
                },
                unsafe { crate::pool::has_slot_run(run) },
            ),
        }
    };
    // No Scheduler or Task reference survives a fresh raw table lookup.
    if !ready.is_empty() {
        // SAFETY: the idle guard owns the role and the policy retained rows.
        let mut batch = unsafe { crate::pool::free::ReadyBatch::new(run) };
        for (task, owner) in ready {
            unsafe { batch.schedule(task, owner) };
        }
    }
    if unsafe {
        ((*run).free_mode
            || ((*run)
                .ordered_mode
                .load(std::sync::atomic::Ordering::Acquire)
                && (*(*run).tasks.get())
                    .as_deref()
                    .is_some_and(ordered::home::needed_after_save)))
            && ((*run).host_waits.load(std::sync::atomic::Ordering::Acquire) != 0
                || free::host::concurrent::pending(run)
                || ordered::host::concurrent::pending(run))
    } {
        unsafe { crate::pool::wake_home(run) };
    }
    unsafe { free::check_quiet(run, Some(key.slot)) };
    resume_on
}

/// Dispatch the running task's foreign interval to a blocking-pool thread (S-13): it waits
/// for a free one ([`Wait::Pool`]), then resumes there, owned by it, and this returns on that
/// thread with no compute permit, for the C call.
///
/// # Safety
/// `ctx` is the running task's, on the home thread, outside any foreign interval; it holds
/// nothing a stop must drop.
pub(crate) unsafe fn to_pool(ctx: &mut TaskCtx, site: u64) -> u16 {
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::foreign::to_pool(ctx, site)) };
    }
    let slot = loop {
        if let Some(slot) = crate::pool::claim(ctx) {
            break slot;
        }
        // SAFETY: caller contract; a pool interval's end wakes the waiters.
        unsafe { block(ctx, Wait::Pool, false, site) };
    };
    let me = role::with_role(ctx, |t| {
        let me = t.current;
        let row = t.task(me);
        unpin(row);
        row.owner = Owner::Pool(slot);
        me
    });
    if free::enabled(ctx) {
        unsafe { free::migrate(ctx) };
        return slot;
    }
    let to = crate::pool::target(ctx, Owner::Pool(slot), None)
        .unwrap_or_else(|| fault("a pool dispatch from its own pool thread"));
    ctx.switch_out();
    let save = &raw mut ctx.task_sp;
    // SAFETY: this task is suspended at `save` before the pool thread resumes it.
    unsafe { crate::pool::hand_off(ctx, save, to, me, false) };
    recycle(ctx);
    slot
}

/// The running task's pool interval ended (S-13): it goes back to the home thread, and this
/// returns there; its pool thread takes the next blocking call, and tasks waiting for one
/// are polled.
///
/// # Safety
/// `ctx` is the running task's, on pool thread `slot` that owns it.
pub(crate) unsafe fn from_pool(ctx: &mut TaskCtx, slot: u16) {
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::foreign::from_pool(ctx, slot)) };
        return;
    }
    if free::enabled(ctx) {
        unsafe { free::foreign::from_pool(ctx, slot) };
        return;
    }
    role::with_role(ctx, |t| {
        let me = t.current;
        let row = t.task(me);
        if std::mem::replace(&mut row.owner, Owner::Any) != Owner::Pool(slot) {
            fault("a pool interval left from a thread that does not own it");
        }
    });
    // SAFETY: this task is suspended before the thread its owner names resumes it.
    unsafe { migrate_self(ctx) };
    crate::pool::release(ctx, slot);
    role::with_role(ctx, |t| {
        for task in std::mem::take(&mut t.pool_waiters) {
            t.wake(task);
        }
    });
}

/// An outermost entry of `serial` gate `gate` by the running task, or a nested re-entry by
/// the task that holds it: whether it was outermost. An outermost entry waits until the
/// gate is free (a wait for CONC-8), then takes it.
///
/// # Safety
/// `ctx` is the running task's and holds nothing a stop must drop.
pub(crate) unsafe fn gate_enter(ctx: &mut TaskCtx, gate: u32, site: u64) -> bool {
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::foreign::gate_enter(ctx, gate, site)) };
    }
    if free::enabled(ctx) {
        return unsafe { crate::stop::check(ctx, free::gate::enter(ctx, gate, site)) };
    }
    let (me, owner) = role::with_role(ctx, |t| {
        let me = t.current;
        let row = t.gate(gate);
        if row.owner == Some(me) {
            row.depth += 1;
        }
        (me, row.owner)
    });
    if owner == Some(me) {
        return false;
    }
    if crate::free::on(ctx) {
        let started = ctx
            .run_ref()
            .extern_metrics
            .as_ref()
            .map(|_| std::time::Instant::now());
        // Entry is admitted separately: releases precede the gate wait, and
        // replay-pending owns neither gate nor compute permit.
        crate::free::enter(ctx, Phase::Entered);
        let object = crate::free::gate(ctx, gate);
        crate::free::publish(ctx, Phase::Entered);
        crate::free::wait_until(ctx, |ctx| {
            role::with_role(ctx, |t| t.gate(gate).owner.is_none())
                && crate::free::enabled(ctx, object)
        });
        role::with_role(ctx, |t| {
            let row = t.gate(gate);
            row.owner = Some(me);
            row.depth = 1;
            row.version += 1;
        });
        crate::free::touch(ctx, object, "gate enter");
        crate::free::done(ctx);
        if let (Some(started), Some(metrics)) = (started, &ctx.run_ref().extern_metrics) {
            metrics.gate_entry(started.elapsed());
        }
        return true;
    }
    if owner.is_none() {
        role::with_role(ctx, |t| {
            let row = t.gate(gate);
            row.owner = Some(me);
            row.depth = 1;
            row.version += 1;
        });
        if let Some(metrics) = &ctx.run_ref().extern_metrics {
            metrics.gate_entry(std::time::Duration::ZERO);
        }
        return true;
    }
    let started = ctx
        .run_ref()
        .extern_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    // SAFETY: caller contract; the poll takes the gate.
    unsafe { block(ctx, Wait::Gate { gate }, false, site) };
    if let (Some(started), Some(metrics)) = (started, &ctx.run_ref().extern_metrics) {
        metrics.gate_entry(started.elapsed());
    }
    true
}

/// The running task leaves `serial` gate `gate`: an outermost leave frees it and polls the
/// tasks waiting for it; a nested one only lowers its depth.
pub(crate) fn gate_leave(ctx: *mut TaskCtx, gate: u32, outermost: bool) {
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::foreign::gate_leave(ctx, gate, outermost)) };
        return;
    }
    if free::enabled(ctx) {
        unsafe { crate::stop::check(ctx, free::gate::leave(ctx, gate, outermost)) };
        return;
    }
    let object = if outermost && crate::free::enter(ctx, Phase::Done) {
        let object = crate::free::gate(ctx, gate);
        crate::free::touch(ctx, object, "gate leave");
        Some(object)
    } else {
        None
    };
    role::with_role(ctx, |t| {
        let me = t.current;
        let row = t.gate(gate);
        if row.owner != Some(me) || row.depth == 0 || (row.depth == 1) != outermost {
            fault("a serial gate left by a task that does not hold it");
        }
        row.depth -= 1;
        if outermost {
            row.owner = None;
            row.version += 1;
            for task in std::mem::take(&mut row.waiters) {
                t.wake(task);
            }
        }
    });
    if object.is_some() {
        crate::free::done(ctx);
    }
}

/// `serial` gate `gate`'s version: the outermost entries and leaves so far.
pub(crate) fn gate_version(ctx: *mut TaskCtx, gate: u32) -> u64 {
    // This probe can run between foreign_enter and callback_context, before
    // the callback takes a permit. Never retain the role while it waits.
    let _role = unsafe { role::Guard::new(ctx) };
    role::with_role(ctx, |t| t.gate(gate).version)
}

/// A task resumed: the stack of the task that completed before it is free again.
fn recycle(ctx: &mut TaskCtx) {
    let worker = ctx.worker();
    if let Some(stack) = worker.retired.take() {
        worker.idle_stacks.put(stack);
    }
}

/// A scheduling point: poll the parked tasks whose wait may be over (those that just parked,
/// those an event woke, those whose host request the adapter reports ready), timers by
/// deadline (ties by park order), other waits by park order. Each whose wait is over has its
/// effect performed and joins the back of the queue.
///
/// # Safety
/// `ctx` is live.
unsafe fn poll_parked(ctx: *mut TaskCtx) -> Stopped<()> {
    // SAFETY: the context is live.
    let ready = unsafe { crate::host_waits::ready(ctx) }?;
    // Scope deadlines first, so the parked tasks' polls see what they cancel (CONC-3).
    for scope in unsafe { crate::host_waits::expired(ctx, &ready) }? {
        unsafe { role::with_held(ctx, |t| ordered::events::expire(t, scope)) };
    }
    let canceled: Vec<u64> = unsafe {
        role::with_held(ctx, |t| {
            t.tasks
                .iter()
                .enumerate()
                .filter_map(|(task, row)| {
                    let row = row.as_ref()?;
                    let Wait::Host { token } = row.wait? else {
                        return None;
                    };
                    t.canceled_task(task as u32, row.shield()).map(|_| token)
                })
                .collect()
        })
    };
    let batch = unsafe { crate::host_waits::process_batch(ctx, &canceled) }?;
    let ready = crate::host_waits::merge_ready(ready, batch);
    let woken = unsafe { crate::host_waits::woken(ctx, &ready) };
    let Some(mut woken) = (unsafe {
        role::with_held(ctx, |t| {
            for task in woken {
                t.wake(task);
            }
            t.wake_receivers();
            if t.woken.is_empty() {
                return None;
            }
            let polling = std::mem::take(&mut t.polling);
            Some(std::mem::replace(&mut t.woken, polling))
        })
    }) else {
        return Ok(());
    };
    // Original request ticks preserve exact ties; readiness's host Instant is
    // only for sleeping and may have rounding or sampling skew. Park places are
    // unique, so the order is total.
    if woken.len() > 1 {
        woken.sort_unstable_by_key(|&task| {
            // The home bridge may release the role and another worker may
            // grow the task table. Copy before it; no row borrow may cross it.
            let (wait, parked_at) = unsafe {
                role::with_held(ctx, |t| {
                    let row = t.task(task);
                    (row.wait, row.parked_at)
                })
            };
            let deadline = match wait {
                // SAFETY: the context is live and `token` parked.
                Some(Wait::Host { token }) => unsafe { crate::host_waits::deadline(ctx, token) },
                _ => None,
            };
            (deadline, parked_at)
        });
    }
    for &task in &woken {
        // A role-free object wake may have ended this wait at its dispatch.
        let parked = unsafe {
            role::with_held(ctx, |t| {
                let row = t.task(task);
                row.woken = false;
                row.state == State::Parked
            })
        };
        if !parked {
            continue;
        }
        // SAFETY: the context is live and `task` is parked.
        if unsafe { poll(ctx, task, &ready) }? {
            #[cfg(feature = "measurement")]
            if let Some(profile) = unsafe { &mut (*(*(*ctx).run_ptr()).mc_profile.get()) }.as_mut()
            {
                profile.ready(unsafe { role::with_held(ctx, |t| t.task(task).path.clone()) });
            }
            unsafe { role::with_held(ctx, |t| t.unpark(task)) };
        }
    }
    woken.clear();
    unsafe { role::with_held(ctx, |t| t.polling = woken) };
    Ok(())
}

/// Whether parked `task`'s wait is over, after performing its effect if it is; `ready` is the
/// scheduling point's ready host requests.
///
/// # Safety
/// `ctx` is live and `task` is parked; a join's slot is in its frame.
unsafe fn poll(ctx: *mut TaskCtx, task: u32, ready: &crate::io::Ready) -> Stopped<bool> {
    // A logical park precedes the idle continuation's physical save. In that
    // interval the owner can still hold &mut TaskCtx and frame-slot borrows.
    // The after-save policy must wake/re-poll waits deferred by this check.
    // SAFETY: callers hold the scheduler role and task is a live parked row.
    if !unsafe { crate::pool::free::is_saved((*ctx).run_ptr(), task) } {
        return Ok(false);
    }
    let wait = unsafe { role::with_held(ctx, |t| t.task(task).wait) };
    if let Some(Wait::Join { child, .. }) = wait
        // SAFETY: the role owns the child's live row. Its Holding state
        // alone cannot expose the result until its stack has been saved.
        && !unsafe { crate::pool::free::is_saved((*ctx).run_ptr(), child) }
    {
        return Ok(false);
    }
    // Host and pool polls reach the binding or the pool outside the borrow.
    enum Outside {
        Host(u64, Option<u32>),
        Pool,
    }
    let polled = unsafe {
        role::with_held(ctx, |s| {
            Ok(match wait {
                Some(Wait::Join { child, slot, size }) => match s.task(child).state {
                    State::Holding => {
                        let held = s.task(child).slot.take().unwrap();
                        // SAFETY: the child is in its finish and the awaiter parked, both slots live.
                        deliver(held, slot, size);
                        cpu_job_event(s.task(child), "transfer");
                        true
                    }
                    State::Done => true,
                    State::Ready | State::Parked => false,
                },
                Some(Wait::Exit { scope }) => s.settled(scope),
                Some(Wait::Receive { channel, slot }) => {
                    let shield = s.task(task).shield();
                    // SAFETY: the slot is in the parked task's frame.
                    match receive_ready(s, task, shield, channel, slot) {
                        Some(status) => {
                            s.task(task).status = status;
                            // SAFETY: saved-frame checks above protect the slot;
                            // capture before the owner can be unparked.
                            ordered::events::record_receive_poll(s, task, slot, status);
                            true
                        }
                        None => false,
                    }
                }
                Some(Wait::ReceiveReady { channel }) => {
                    let shield = s.task(task).shield();
                    s.canceled_task(task, shield).is_some() || s.channels.ready(channel)
                }
                Some(Wait::Host { token }) => {
                    let shield = s.task(task).shield();
                    return Err(Outside::Host(token, s.canceled_task(task, shield)));
                }
                Some(Wait::Any { set, member: None }) => s.first_completed(set).is_some(),
                Some(Wait::Any {
                    set,
                    member: Some(index),
                }) => s.member_completed(set, index),
                // SAFETY: the parked task holds the set's row.
                Some(Wait::Drain { set }) => poll_drained(s, set),
                Some(Wait::Final) => false,
                Some(Wait::Gate { gate }) => {
                    let row = s.gate(gate);
                    if row.owner.is_none() {
                        row.owner = Some(task);
                        row.depth = 1;
                        row.version += 1;
                        true
                    } else {
                        false
                    }
                }
                Some(Wait::Pool) => return Err(Outside::Pool),
                Some(Wait::Retry { cancelable }) => {
                    let shield = s.task(task).shield();
                    let canceled = cancelable && s.canceled_task(task, shield).is_some();
                    std::mem::take(&mut s.task(task).retried) || canceled
                }
                None => fault("a parked task without a wait"),
            })
        })
    };
    Ok(match polled {
        Ok(done) => done,
        // SAFETY: the context is live and `token` is this task's parked request.
        Err(Outside::Host(token, cause)) => {
            unsafe { crate::host_waits::poll(ctx, token, cause, ready.contains(token)) }?
        }
        Err(Outside::Pool) => crate::pool::has_slot(ctx),
    })
}

/// The running task stopped (it parked, holds its result or completed): poll the parked tasks,
/// then run the front of the queue, which may be the running task itself.
///
/// # Safety
/// `ctx` is live.
unsafe fn run_next(ctx: &mut TaskCtx) -> Stopped<()> {
    if free::enabled(ctx) {
        unsafe {
            free::dispatch_ready(ctx);
            free::suspend(ctx, false);
        }
        recycle(ctx);
        return crate::stop::observe(ctx);
    }
    // SAFETY: the context is live.
    let next = unsafe { next_task(ctx) }?;
    unsafe { switch_to(ctx, next) };
    Ok(())
}

/// A scheduling point: the task that ran last stopped or joined the back of the queue, and
/// the policy takes the task to run next off the nonempty queue. At a `pct@1` change point the
/// task that ran last takes its lowered priority first. A choice the policy could have made
/// otherwise is a scheduling input of the run, recorded on its tape and, in replay, taken
/// from it.
///
/// # Safety
/// `ctx` is live and no borrow of its Io adapter or scheduler is held.
unsafe fn pick(ctx: *mut TaskCtx) -> Stopped<Option<u32>> {
    // SAFETY: the context is live.
    queued(unsafe { &*ctx });
    let picked = unsafe {
        role::with_held(ctx, |s| {
            if s.schedule.algorithm() == Algorithm::Fifo || s.runnable.is_empty() {
                return Err(s.runnable.pop_front());
            }
            let current = s.current as usize;
            if let Some(priority) = s.schedule.point()
                && let Some(Some(ran)) = s.tasks.get_mut(current)
            {
                ran.priority = priority;
            }
            let priorities: Vec<u64> = s
                .runnable
                .iter()
                .map(|&task| s.tasks[task as usize].as_ref().map_or(0, |t| t.priority))
                .collect();
            let choice = s.schedule.choose(&priorities);
            let recorded = s.schedule.recorded(priorities.len());
            Ok((choice, priorities.len(), recorded))
        })
    };
    let (mut choice, count, recorded) = match picked {
        Ok(picked) => picked,
        Err(next) => return Ok(next),
    };
    // SAFETY: the context is live; a run without an Io binding records nothing.
    if recorded && !unsafe { (*ctx).io_data() }.is_null() {
        let count = count as u64;
        let mut recorded = Ok(0);
        // The binding is home state: the home thread asks it (`crate::pool`).
        unsafe {
            crate::pool::call_home(ctx, &mut || {
                let binding = crate::io::Binding::get(ctx);
                recorded = binding.adapter.schedule(count, choice as u64);
            })
        }?;
        choice = match recorded {
            Ok(choice) if choice < count => choice as usize,
            Ok(_) => crate::fault("a scheduling input outside the runnable queue"),
            Err(error) => return Err(Stop::refusal(error)),
        };
    }
    Ok(unsafe { role::with_held(ctx, |t| t.runnable.remove(choice)) })
}

/// Poll the parked tasks and take the next task to run ([`pick`]).
///
/// # Safety
/// `ctx` is live.
unsafe fn next_task(ctx: *mut TaskCtx) -> Stopped<u32> {
    // SAFETY: the context is live.
    unsafe { poll_parked(ctx) }?;
    if unsafe { crate::runners::driving(ctx) } {
        // An execution of several runs: with nothing runnable here, the others run until
        // this run is next, and only the execution is Blocked (RUN-12).
        while unsafe { role::with_held(ctx, |t| t.runnable.is_empty()) } {
            unsafe {
                crate::runners::idle(ctx)?;
                poll_parked(ctx)?;
            }
        }
    }
    // With nothing runnable, sleep until a parked host request may complete, then poll again.
    while unsafe { role::with_held(ctx, |t| t.runnable.is_empty()) }
        && unsafe { crate::host_waits::wait(ctx) }?
    {
        // A host's stop ends the wait too (`crate::host::RunControl`).
        crate::stop::observe(ctx)?;
        unsafe { poll_parked(ctx) }?;
    }
    // CONC-8: no task can run and nothing outside the run can wake one.
    match unsafe { pick(ctx) }? {
        Some(next) => Ok(next),
        None => Err(Stop::Blocked(unsafe { blocked_waits(ctx) })),
    }
}

/// Capture parked waits only when the run is Blocked, before its context is abandoned.
///
/// # Safety
/// `ctx` is live and no task can run.
pub(crate) unsafe fn blocked_waits(ctx: *mut TaskCtx) -> Vec<hirsel_run_schema::Wait> {
    let table = unsafe { (*ctx).run_ref().failure_table };
    let members = |s: &mut Scheduler, set: u32| -> Vec<u64> {
        s.set(set)
            .members
            .iter()
            .flatten()
            .map(|word| u64::from(*word as u32))
            .collect()
    };
    let parked = role::with_role(ctx, |s| {
        s.parked()
            .into_iter()
            .map(|(index, wait)| {
                let targets = match wait {
                    Wait::Join { child, .. } => vec![u64::from(child)],
                    Wait::Exit { scope } => s
                        .scope(scope)
                        .children
                        .iter()
                        .copied()
                        .map(u64::from)
                        .collect(),
                    Wait::Receive { channel, .. } | Wait::ReceiveReady { channel } => {
                        vec![u64::from(channel)]
                    }
                    Wait::Any { set, .. } | Wait::Drain { set } => members(s, set),
                    Wait::Gate { gate } => vec![u64::from(gate)],
                    Wait::Host { token } => vec![token],
                    Wait::Final | Wait::Retry { .. } | Wait::Pool => vec![],
                };
                let row = s.task(index);
                (row.path.clone(), row.wait_site, wait, targets)
            })
            .collect::<Vec<_>>()
    });
    let mut waits = Vec::new();
    for (spawn_path, wait_site, wait, mut targets) in parked {
        let site = table
            .filter(|_| wait_site != 0)
            .map(|table| table.located(wait_site))
            .unwrap_or_default();
        let kind = match wait {
            Wait::Join { .. } => "task.await",
            Wait::Exit { .. } => "task.scope",
            Wait::Receive { .. } | Wait::ReceiveReady { .. } => "channel.receive",
            Wait::Any { .. } => "task.await_any",
            Wait::Drain { .. } => "task.cancel_all",
            // SAFETY: the context and its binding are live.
            Wait::Host { token } if unsafe { crate::host_waits::is_runner_call(ctx, token) } => {
                targets.clear();
                "runner.call"
            }
            Wait::Host { .. } => "host request",
            Wait::Final => {
                if unsafe { (*ctx).run_ref().free_replay } {
                    "free.cut"
                } else {
                    "free.final"
                }
            }
            Wait::Retry { .. } => "free.retry",
            Wait::Gate { .. } => "foreign.gate",
            Wait::Pool => "foreign.pool",
        };
        waits.push(hirsel_run_schema::Wait {
            spawn_path,
            site,
            kind: kind.into(),
            targets,
        });
    }
    waits.sort_by(|a, b| a.spawn_path.cmp(&b.spawn_path));
    waits
}

/// The single-compute-worker shortcut seam: a run with compute workers (or forced migration,
/// which runs on them) moves its tasks at every suspension and its Io belongs to the home
/// thread, so it takes no shortcut, and neither does a task on a pool thread. Free-mode points
/// must always reach their recording policy.
unsafe fn single_worker_fast_path(ctx: *mut TaskCtx) -> bool {
    let run = unsafe { (*ctx).run_ref() };
    !crate::pool::wide(run) && !crate::pool::on_pool() && run.free.is_none()
}

/// Whether the running root is the run's only possible runnable task. The
/// caller separately checks host waits and scope deadlines before waiting inline.
/// # Safety
/// The context is live and reached on its home thread.
pub(crate) unsafe fn sole_root(ctx: *mut TaskCtx) -> bool {
    if !unsafe { single_worker_fast_path(ctx) } {
        return false;
    }
    let run = unsafe { (*ctx).run_ref() };
    run.tasks
        .as_ref()
        .is_none_or(|s| s.tasks.len() == 1 && s.runnable.is_empty() && s.woken.is_empty())
}

/// The yield after a capability operation: the parked tasks are polled and, when another task
/// is runnable, the running task joins the back of the queue and the policy picks the task to
/// run, which may be the running task again.
///
/// # Safety
/// `ctx` is live and no borrow of its Io adapter or scheduler is held.
pub unsafe fn yield_now(ctx: &mut TaskCtx) {
    // SAFETY: the context is live; the caller holds nothing a stop must drop.
    unsafe { crate::stop::check(ctx, try_yield(ctx)) }
}

/// [`yield_now`], returning a stop to its caller.
///
/// # Safety
/// As for [`yield_now`].
pub(crate) unsafe fn try_yield(ctx: &mut TaskCtx) -> Stopped<()> {
    // SAFETY: the running task owns this entry until its suspension or return.
    let _role = unsafe { role::Guard::new(ctx) };
    #[cfg(feature = "diagnostic")]
    // SAFETY: the context and its roots are live.
    unsafe {
        crate::root_trace::point(ctx)
    };
    crate::stop::observe(ctx)?;
    if ctx.run_ref().tasks.is_none() {
        return Ok(());
    }
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.op();
    }
    if ordered::engine::on(ctx) {
        return ordered::host::yield_now(ctx);
    }
    if free::enabled(ctx) {
        return unsafe { free::try_yield(ctx) };
    }
    // Tokio ce685ee6 current_thread/mod.rs:818-879 keeps polling its sole
    // runnable root without entering the driver. Hirsel additionally preserves
    // explicit scope deadlines and free-mode phase recording at every point.
    if unsafe { single_worker_fast_path(ctx) }
        && unsafe {
            role::with_held(ctx, |t| {
                t.tasks.len() == 1 && t.runnable.is_empty() && t.woken.is_empty()
            })
        }
        && unsafe { crate::host_waits::idle(ctx) }
    {
        return Ok(());
    }
    // SAFETY: the context is live.
    unsafe { poll_parked(ctx) }?;
    let alone = unsafe {
        role::with_held(ctx, |t| {
            let alone = t.runnable.is_empty();
            if !alone {
                let current = t.current;
                t.runnable.push_back(current);
            }
            alone
        })
    };
    if alone {
        // SAFETY: the context is live; the caller holds nothing a stop must drop.
        unsafe { go_on(ctx) };
        return crate::stop::observe(ctx);
    }
    queued(ctx);
    // SAFETY: the context is live and the queue is nonempty.
    let next = unsafe { pick(ctx) }?.unwrap_or_else(|| crate::fault("an empty runnable queue"));
    unsafe { switch_to(ctx, next) };
    crate::stop::observe(ctx)
}

/// The running task goes on after a scheduling point that switched to no other task: its
/// hold on the home thread for the Io binding ends, and it moves where its owner says.
///
/// # Safety
/// `ctx` is the running task's; the caller holds nothing a stop must drop.
pub(crate) unsafe fn go_on(ctx: &mut TaskCtx) {
    unsafe {
        role::with_held(ctx, |t| {
            let me = t.current;
            if let Some(Some(row)) = t.tasks.get_mut(me as usize) {
                unpin(row);
            }
        })
    };
    // SAFETY: caller contract.
    unsafe { migrate_self(ctx) };
}

/// A runnable queue push or pop, for the scheduling rows' counters.
fn queued(ctx: &TaskCtx) {
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.queue_op();
    }
}

/// Park the running task until `wait` is over; returns once a poll performed its effect.
/// `yielded`: the operation yields first, and the task parks when it is next at the front.
///
/// # Safety
/// `ctx` is live; whatever `wait` names outlives the wait; the caller holds nothing a stop
/// must drop.
unsafe fn block(ctx: &mut TaskCtx, wait: Wait, yielded: bool, site: u64) {
    // SAFETY: caller contract.
    unsafe { crate::stop::check(ctx, try_block(ctx, wait, yielded, site)) }
}

/// [`block`], returning a stop to its caller.
///
/// # Safety
/// `ctx` is live; whatever `wait` names outlives the wait.
/// [`try_block`]'s Free park for a task whose compact frame leaves its stack
/// instead of suspending on it: the task waits as it would, and the frame's
/// wake reissues the op that parked (`compact::can_wait`).
///
/// # Safety
/// As for [`try_block`]; the caller returns `STATUS_SUSPENDED` to its
/// boundary call.
pub(crate) unsafe fn park_left(ctx: *mut TaskCtx, wait: Wait) -> Stopped<()> {
    unsafe { park_left_at(ctx, wait, 0) }
}

/// Park a certified entered operation, retaining its original failure site.
/// # Safety
/// As for park_left; the operation owns its wait and retained invocation.
pub(crate) unsafe fn park_left_at(ctx: *mut TaskCtx, wait: Wait, site: u64) -> Stopped<()> {
    // SAFETY: the running task owns this entry until its suspension or return.
    let _role = unsafe { role::Guard::new(ctx) };
    crate::stop::observe(ctx)?;
    unsafe {
        role::with_held(ctx, |t| {
            let me = t.current;
            t.task(me).wait_site = site;
            t.park(wait);
        });
        free::dispatch_ready(ctx);
    }
    Ok(())
}

unsafe fn try_block(ctx: &mut TaskCtx, wait: Wait, yielded: bool, site: u64) -> Stopped<()> {
    // SAFETY: the running task owns this entry until its suspension or return.
    let _role = unsafe { role::Guard::new(ctx) };
    #[cfg(feature = "diagnostic")]
    // SAFETY: the context and its roots are live.
    unsafe {
        crate::root_trace::point(ctx)
    };
    crate::stop::observe(ctx)?;
    // A first blocked host call can precede every task operation. The role
    // guard excludes other owners but does not create the root's tables;
    // polling and parking below borrow those tables through with_held.
    if unsafe { (*ctx.run_ref().tasks.get()).is_none() } {
        create_scheduler(ctx);
    }
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.op();
    }
    if free::enabled(ctx) {
        let (me, generation) = unsafe {
            role::with_held(ctx, |t| {
                let me = t.current;
                let row = t.task(me);
                // The chain budget continues only through this worker's LIFO slot.
                row.wait_site = site;
                unpin(row);
                let generation = row.generation;
                t.park(wait);
                (me, generation)
            })
        };
        if let Wait::Host { token } = wait {
            let concurrent = ctx
                .run_ref()
                .free_io
                .get()
                .is_some_and(|io| io.park(token, me, generation));
            if !concurrent {
                let _segment = unsafe { role::Segment::new(ctx) };
                unsafe { crate::host_waits::park(ctx, token, me) };
            }
        }
        return unsafe { run_next(ctx) };
    }
    if yielded {
        // SAFETY: the context is live.
        unsafe { poll_parked(ctx) }?;
        if unsafe {
            role::with_held(ctx, |t| {
                let others = !t.runnable.is_empty();
                if others {
                    let current = t.current;
                    t.runnable.push_back(current);
                }
                others
            })
        } {
            queued(ctx);
            // SAFETY: the context is live and the queue is nonempty.
            let next =
                unsafe { pick(ctx) }?.unwrap_or_else(|| crate::fault("an empty runnable queue"));
            unsafe { switch_to(ctx, next) };
        }
    }
    let current = unsafe {
        role::with_held(ctx, |t| {
            let current = t.current;
            t.task(current).wait_site = site;
            t.park(wait);
            current
        })
    };
    if let Wait::Host { token } = wait {
        // SAFETY: the context is live, its Binding bound, and `token` blocked.
        unsafe { crate::host_waits::park(ctx, token, current) };
    }
    // SAFETY: the context is live; a poll makes this task runnable again.
    unsafe { run_next(ctx) }?;
    crate::stop::observe(ctx)
}

/// Park the running task on host request `token` until a poll completes it (`crate::host_waits`).
/// `yielded`: the request yields first, and the task parks when it is next at the front.
///
/// # Safety
/// `ctx` is live and no borrow of its Io adapter or scheduler is held.
pub(crate) unsafe fn park_host(ctx: &mut TaskCtx, token: u64, yielded: bool) -> Stopped<()> {
    if ordered::engine::on(ctx) {
        return unsafe { ordered::host::park(ctx, token, yielded) };
    }
    // SAFETY: the context is live; the request outlives the wait.
    unsafe { try_block(ctx, Wait::Host { token }, yielded, 0) }
}

/// Resume `child` directly (a join delivered its result, or its result is discarded) and
/// return when it completes.
///
/// # Safety
/// `ctx` is live and `child` is holding.
unsafe fn hand_off(ctx: &mut TaskCtx, child: u32) {
    let free = free::enabled(ctx);
    let _path = unsafe {
        role::with_held(ctx, |t| {
            let me = t.current;
            t.task(me).state = State::Parked;
            let row = t.task(child);
            row.state = State::Ready;
            row.handoff = Some(me);
            let path = cfg!(feature = "measurement").then(|| row.path.clone());
            if free {
                t.runnable.push_back(child);
            }
            path
        })
    };
    if free {
        unsafe { crate::stop::check(ctx, run_next(ctx)) };
        return;
    }
    #[cfg(feature = "measurement")]
    if let Some(profile) = &mut unsafe { &mut (*(*ctx.run_ptr()).mc_profile.get()) } {
        profile.ready(_path.unwrap());
    }
    unsafe { switch_to(ctx, child) };
}

/// The fiber entry of every spawned task, with its own context.
/// Its argument is the task's key word ([`registry::TaskKey::word`]).
unsafe extern "C" fn task_entry(ctx: *mut u8, task: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let key = registry::TaskKey::from_word(task as usize as u64);
    let index = key.slot;
    // SAFETY: this new fiber owns its stable task context.
    let _role = unsafe { role::Guard::physical(ctx) };
    // SAFETY: the task's context lies at the top of this stack and is bound to its worker.
    recycle(unsafe { &mut *ctx });
    free::replay::start(ctx);
    let Some((code, environment, operation_site)) = role::with_role(ctx, |t| {
        ordered::ahead::physical(t, key).and_then(|row| row.code.take())
    }) else {
        fault("a task started without code");
    };
    #[cfg(feature = "measurement")]
    profile::resume(ctx);
    // SAFETY: compiled closure code and its owned environment, called once.
    let mut status = {
        // SAFETY: no scheduler borrow survives the call into compiled code.
        let _segment = unsafe { role::Segment::new(ctx) };
        unsafe {
            crate::pool::cpu_scoped::probe::resume(ctx);
            code(ctx, environment, operation_site, operation_site)
        }
    };
    // An uncommitted prefix cannot run environment cleanup or publish an end.
    unsafe { ordered::ahead::barrier(ctx) };
    // The caller of a closure releases its environment wholesale.
    status |= unsafe {
        crate::release::release_block(
            &mut *ctx,
            environment,
            std::ptr::null(),
            operation_site,
            operation_site,
        )
    };
    unsafe { complete(&mut *ctx, index, status) }
}

/// Canonical suffix of the bounded startup. Admission precedes every call here.
unsafe extern "C" fn ahead_complete(ctx: *mut TaskCtx, mut status: u8) -> ! {
    let _role = unsafe { role::Guard::physical(ctx) };
    assert!(!ordered::ahead::private(ctx));
    let index = unsafe { (*ctx).task_id };
    let (_, environment, site) =
        role::with_role(ctx, |t| t.task(index).code.take()).expect("ahead closure custody");
    status |= unsafe {
        crate::release::release_block(&mut *ctx, environment, std::ptr::null(), site, site)
    };
    unsafe { complete(&mut *ctx, index, status) }
}

/// A v4 ahead body saved at a boundary (`ahead::native::start`): its frame
/// continues in the body's ordinary dispatcher, leaving this stack when the
/// task can, and the task then ends as its entry would. Never its entry again.
pub(crate) unsafe extern "C" fn ahead_suspended(
    ctx: *mut TaskCtx,
    native: *mut ordered::ahead::native::Native,
) -> ! {
    let _role = unsafe { role::Guard::physical(ctx) };
    assert!(!ordered::ahead::private(ctx), "a private ahead body saved");
    let index = unsafe { (*ctx).task_id };
    let (_, environment, site) =
        role::with_role(ctx, |t| t.task(index).code.take()).expect("ahead closure custody");
    // Inert cleanup a host segment deferred is task-owned: settle it first.
    unsafe { ordered::ahead::host_return::flush_logical(ctx, native) };
    let (dispatcher, frame) = unsafe { (*native).take_resume() };
    #[cfg(feature = "diagnostic")]
    crate::compact_trace::ahead(index, frame.state());
    let mut status = {
        // SAFETY: the task's body holds the role at depth one, as in resume_run.
        let _body = unsafe { role::Guard::new(ctx) };
        unsafe {
            crate::continuation::continue_saved(ctx, dispatcher, environment, site, site, frame)
        }
    };
    status |= unsafe {
        crate::release::release_block(&mut *ctx, environment, std::ptr::null(), site, site)
    };
    unsafe { complete(&mut *ctx, index, status) }
}

/// The running child finished its frame (and its environment): record its outcome and run
/// another task.
///
/// # Safety
/// `ctx` is live and `index` is the running child.
unsafe fn complete(ctx: &mut TaskCtx, index: u32, status: u8) -> ! {
    if ordered::engine::on(ctx) {
        unsafe { ordered::lifecycle::complete(ctx, index, status) }
    }
    if free::enabled(ctx) {
        unsafe { free::complete(ctx, index, status) }
    }
    // This fiber is abandoned at the departure, so nothing it owns may still be live then.
    let next = unsafe { crate::stop::check(ctx, completed(ctx, index, status)) };
    #[cfg(feature = "ownership-trace")]
    crate::ownership_trace::task_end(ctx);
    // SAFETY: the context is live; this fiber is never resumed.
    unsafe { depart_to(ctx, next) }
}

/// Record the running child's completion and choose the task to run next.
///
/// # Safety
/// `ctx` is live and `index` is the running child.
unsafe fn completed(ctx: &mut TaskCtx, index: u32, status: u8) -> Stopped<u32> {
    #[cfg(feature = "measurement")]
    std::mem::forget(profile::Operation::new(ctx, "task.complete"));
    let path = role::with_role(ctx, |t| {
        let row = t.task(index);
        row.ended = true;
        row.path.clone()
    });
    crate::free::end(ctx, &path);
    let chain = ctx.failure.take_chain(&path);
    let failed = status != STATUS_OK;
    if failed != !chain.is_empty() {
        fault("a task's status disagrees with its failure state");
    }
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.complete();
    }
    let (handoff, stack, retired) = unsafe {
        role::with_held(ctx, |s| {
            let row = s.task(index);
            row.state = State::Done;
            let scope = row
                .scope
                .unwrap_or_else(|| fault("the root task completed as a child"));
            let handoff = row.handoff.take();
            let detached = row.detached;
            if failed {
                row.failure = Some(chain);
                s.order(index);
                // FAIL-2: a failed child cancels its siblings and its scope's later blocking effects.
                s.cancel_scope(scope, CANCEL_CHILD_FAILED);
                let siblings = s.scope(scope).children.clone();
                s.wake_owner(scope);
                for sibling in siblings {
                    if sibling != index {
                        s.cancel(sibling, CANCEL_CHILD_FAILED);
                    }
                }
            }
            s.settle(index);
            let stack = s.task(index).stack.take();
            // A detached child's result is gone at its completion; only live children stay rows.
            let retired = (detached && !failed).then(|| {
                s.scope(scope).children.retain(|&c| c != index);
                s.retire_task(index)
            });
            (handoff, stack, retired)
        })
    };
    ctx.worker().retired = stack;
    drop(retired);
    #[cfg(feature = "measurement")]
    if let Some(next) = handoff
        && let Some(profile) = &mut unsafe { &mut (*(*ctx.run_ptr()).mc_profile.get()) }
    {
        profile.ready(unsafe { role::with_held(ctx, |t| t.task(next).path.clone()) });
    }
    match handoff {
        // A join or a discard ran the rest of a holding child: no schedule point.
        Some(next) => Ok(next),
        // SAFETY: the context is live.
        None => unsafe { next_task(ctx) },
    }
}

/// Move a holding child's `Some` image into `out` and leave its `None` behind.
///
/// # Safety
/// Both slots are live `Option` images of `slot.size` bytes.
unsafe fn deliver(slot: Slot, out: *mut u8, size: u64) {
    if slot.size as u64 != size {
        fault("a join's slot differs from its child's");
    }
    // SAFETY: caller contract; the slots are distinct frames' storage.
    unsafe {
        std::ptr::copy_nonoverlapping(slot.slot, out, slot.size);
        std::ptr::copy_nonoverlapping(slot.none, slot.slot, slot.size);
    }
}

/// `$intrinsic.TaskScopeEnter(tasks)`: a fresh scope of the running task (CONC-2).
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_scope_enter(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_scope_enter");
    if ordered::engine::on(ctx) {
        let binding = unsafe { crate::continuation::binding::take_bound(ctx, 16) };
        let status = unsafe { crate::stop::check(ctx, ordered::lifecycle::scope_enter(ctx, out)) };
        if let Some(binding) = binding {
            unsafe { crate::continuation::binding::complete_bound(ctx, binding) };
        }
        return status;
    }
    if free::enabled(ctx) {
        return unsafe { crate::stop::check(ctx, free::lifecycle::scope_enter(ctx, out)) };
    }
    crate::free::enter(ctx, Phase::Done);
    let (index, generation) = open_scope(ctx, None);
    crate::free::done(ctx);
    // SAFETY: caller contract.
    unsafe {
        out.write(handle(index, generation));
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// A fresh scope of the running task, its row and generation.
fn open_scope(ctx: *mut TaskCtx, deadline: Option<ScopeDeadline>) -> (u32, u32) {
    role::with_role(ctx, |s| s.open_scope(deadline))
}

impl Scheduler {
    fn open_scope(&mut self, deadline: Option<ScopeDeadline>) -> (u32, u32) {
        let s = self;
        let (index, generation) = s.take_scope_slot();
        let current = s.current;
        s.scopes[index as usize] = Some(Scope {
            generation,
            owner: current,
            children: Vec::new(),
            canceled: Default::default(),
            deadline,
            completions: AtomicU64::new(0),
        });
        s.task(current).scopes.push(index);
        s.cancel_epoch.changed();
        (index, generation)
    }
}

/// `$intrinsic.TaskScopeEnterUntil(tasks, deadline)`: a fresh scope of the running task that
/// its deadline (the clock handle, then the Instant's ticks) cancels (CONC-3). Its timer is
/// armed under a host token of its own before the entry's yield; a deadline already reached,
/// or a clock that cannot be read, cancels the scope at once.
///
/// # Safety
/// `ctx` is live with its Io bound, `out` writable and `deadline` two readable words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_scope_enter_until(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
    deadline: *const [u64; 2],
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_scope_enter_until");
    // SAFETY: caller contract.
    if ordered::engine::on(ctx) {
        let binding = unsafe { crate::continuation::binding::take_bound(ctx, 17) };
        let status = unsafe {
            crate::stop::check(
                ctx,
                ordered::lifecycle::scope_enter_until(ctx, out, deadline),
            )
        };
        if let Some(binding) = binding {
            unsafe { crate::continuation::binding::complete_bound(ctx, binding) };
        }
        return status;
    }
    if free::enabled(ctx) {
        return unsafe {
            crate::stop::check(ctx, free::lifecycle::scope_enter_until(ctx, out, deadline))
        };
    }
    let [clock, at] = unsafe { deadline.read() };
    crate::free::enter(ctx, Phase::Done);
    let (index, generation) = open_scope(
        ctx,
        Some(ScopeDeadline {
            token: None,
            expired: false,
        }),
    );
    // SAFETY: the context is live and its Io bound.
    match unsafe {
        crate::stop::check(
            ctx,
            crate::host_waits::arm(ctx, index, generation, clock, at),
        )
    } {
        Some(token) => {
            role::with_role(ctx, |t| {
                if let Some(d) = &mut t.scope(index).deadline {
                    d.token = Some(token);
                }
            });
        }
        None => role::with_role(ctx, |t| t.expire(index)),
    }
    crate::free::done(ctx);
    // SAFETY: caller contract.
    unsafe {
        out.write(handle(index, generation));
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `$intrinsic.TaskScopeExit(s, error)`, after the scope body returned (`error` when it
/// returned an `Error`) or failed (the running task's failure is active). A failing or error
/// exit cancels the children (`Parent`). Every child is then joined; results nobody awaited are
/// dropped, the last spawned first; and the failures of failed children join the running task's
/// chain, the earliest spawn path leading (FAIL-2). Returns 1 when that chain is active.
///
/// # Safety
/// `ctx` is live and `scope` names a scope the running task entered last.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_scope_exit(
    ctx: *mut TaskCtx,
    caller_site: u64,
    _operation_site: u64,
    scope: *const u64,
    error: u8,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_scope_exit");
    if ordered::engine::on(ctx) {
        let binding = unsafe { crate::continuation::binding::take_bound(ctx, 18) };
        let status = unsafe {
            crate::stop::check(
                ctx,
                ordered::lifecycle::scope_exit(ctx, scope, error, caller_site),
            )
        };
        if let Some(binding) = binding {
            unsafe { crate::continuation::binding::complete_bound(ctx, binding) };
        }
        return status;
    }
    if free::enabled(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                free::lifecycle::scope_exit(ctx, scope.read(), error, caller_site),
            )
        };
    }
    // SAFETY: caller contract.
    unsafe { exit_scope(&mut *ctx, scope, error, caller_site) }.0
}

/// `$intrinsic.TaskScopeExitUntil(s, error) -> bool`: the exit of a `task.scope_until` scope,
/// which removes its timer and writes whether its deadline fired, including while the exit
/// waited for children.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_scope_exit`]; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_scope_exit_until(
    ctx: *mut TaskCtx,
    caller_site: u64,
    _operation_site: u64,
    out: *mut u8,
    scope: *const u64,
    error: u8,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_scope_exit_until");
    if ordered::engine::on(ctx) {
        let binding = unsafe { crate::continuation::binding::take_bound(ctx, 19) };
        let status = unsafe {
            crate::stop::check(
                ctx,
                ordered::lifecycle::scope_exit_until(ctx, scope, error, caller_site, out),
            )
        };
        if let Some(binding) = binding {
            unsafe { crate::continuation::binding::complete_bound(ctx, binding) };
        }
        return status;
    }
    if free::enabled(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                free::lifecycle::scope_exit_until(ctx, scope.read(), error, caller_site, out),
            )
        };
    }
    // SAFETY: caller contract.
    let (status, expired) = unsafe { exit_scope(&mut *ctx, scope, error, caller_site) };
    // SAFETY: caller contract.
    unsafe { out.write(u8::from(expired)) };
    status
}

/// A scope's exit: its status and whether its deadline fired.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_scope_exit`].
unsafe fn exit_scope(
    ctx: &mut TaskCtx,
    scope: *const u64,
    error: u8,
    caller_site: u64,
) -> (u8, bool) {
    let free = crate::free::enter_call(ctx);
    // SAFETY: caller contract.
    let word = unsafe { scope.read() };
    let cancel = ctx.failure.active() || error != 0;
    let (index, current, waited) = role::with_role(ctx, |s| {
        let index = s.scope_handle(word);
        let current = s.current;
        if s.task(current).scopes.pop() != Some(index) {
            fault("a scope exits before a scope entered inside it");
        }
        s.cancel_epoch.changed();
        if cancel {
            for child in s.scope(index).children.clone() {
                s.cancel(child, CANCEL_PARENT);
            }
        }
        (index, current, !s.settled(index))
    });
    if waited {
        crate::free::publish(ctx, Phase::Entered);
        // SAFETY: the context is live; the exit is not a yield.
        unsafe { block(ctx, Wait::Exit { scope: index }, false, caller_site) };
    }
    let children = unsafe { role::with_held(ctx, |t| t.scope(index).children.clone()) };
    for &child in children.iter().rev() {
        let holding = unsafe {
            role::with_held(ctx, |t| {
                let row = t.task(child);
                let holding = row.state == State::Holding;
                if holding {
                    row.slot = None;
                }
                holding
            })
        };
        if holding {
            // SAFETY: the child is holding; its own cleanup drops the result.
            unsafe { hand_off(ctx, child) };
        }
    }
    let (own, chains, retired, deadline) = unsafe {
        role::with_held(ctx, |s| {
            let own = s.task(current).path.clone();
            let mut chains = Vec::new();
            let mut retired = Vec::with_capacity(children.len());
            for &child in &children {
                chains.extend(s.task(child).failure.take());
                retired.push(s.retire_task(child));
            }
            (own, chains, retired, s.take_scope(index).deadline)
        })
    };
    for chain in chains {
        ctx.failure.merge_chain(&own, chain);
    }
    drop(retired);
    // A scope that exits before its deadline removes the timer; nothing is recorded.
    if let Some(ScopeDeadline {
        token: Some(token), ..
    }) = &deadline
    {
        let token = *token;
        // SAFETY: the context is live and its Io bound, as the entry armed it.
        unsafe { crate::host_waits::disarm(ctx, token) };
    }
    let expired = deadline.as_ref().is_some_and(|d| d.expired);
    let expired = if deadline.is_some() {
        crate::free::expired(ctx, expired)
    } else {
        expired
    };
    if free {
        // A free op yields, unless it waited.
        crate::free::done(ctx);
        if !waited {
            // SAFETY: the context is live.
            unsafe { yield_now(&mut *ctx) };
        }
    }
    let status = if ctx.failure.active() {
        STATUS_FAILURE
    } else {
        STATUS_OK
    };
    (status, expired)
}

/// `$intrinsic.TaskSpawn[R](s, take main)`: a child of `s` running `main` (the compiled
/// `$native.task_main[R]` closure, whose environment it takes). The child joins the back of the
/// queue; the spawner then yields, so a child spawned with nothing else runnable starts at once.
///
/// # Safety
/// `ctx` is live, `out` writable, `scope` names a live scope and `code`/`environment` are an
/// owned once closure of `fn() -> ()`.
#[cfg_attr(not(feature = "operation-latency"), unsafe(no_mangle))]
pub unsafe extern "C" fn hirsel_rt_v2_task_spawn(
    ctx: *mut TaskCtx,
    _caller_site: u64,
    operation_site: u64,
    _destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
    code: TaskCode,
    environment: *mut Header,
    flags: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_spawn");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::lifecycle::spawn(ctx, out, scope, code, environment, operation_site),
            )
        };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::lifecycle::spawn(
                    ctx,
                    out,
                    scope.read(),
                    code,
                    environment,
                    operation_site,
                    flags,
                ),
            )
        };
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    // SAFETY: caller owns the closure and the named live scope.
    let (index, generation, child) =
        unsafe { spawn_child(ctx, scope, code, environment, operation_site) };
    if crate::free::start(ctx, &child) {
        // SAFETY: the context is live.
        queued(unsafe { &*ctx });
        let _path = unsafe {
            role::with_held(ctx, |t| {
                t.runnable.push_back(index);
                cfg!(feature = "measurement").then(|| t.task(index).path.clone())
            })
        };
        #[cfg(feature = "measurement")]
        if let Some(profile) = unsafe { &mut (*(*(*ctx).run_ptr()).mc_profile.get()) }.as_mut() {
            profile.ready(_path.unwrap());
        }
    }
    crate::free::done(ctx);
    // SAFETY: caller contract.
    unsafe {
        out.write(handle(index, generation));
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// Allocate and link one child at its spawn's commit. Physical notification
/// belongs to the logical policy after this bounded mutation returns.
/// # Safety
/// The role is held; scope, closure and environment satisfy TaskSpawn's ABI.
unsafe fn spawn_child(
    ctx: *mut TaskCtx,
    scope: *const u64,
    code: TaskCode,
    environment: *mut Header,
    operation_site: u64,
) -> (u32, u32, Vec<u32>) {
    // SAFETY: the context is live.
    let started = unsafe { (*ctx).run_ref() }
        .worker_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    // SAFETY: the context is live.
    let (worker, run, shield) = unsafe { ((*ctx).worker_ptr(), (*ctx).run_ptr(), (*ctx).shield) };
    let stack = unsafe { (*ctx).worker() }.idle_stacks.take();
    let child = stack.context();
    // SAFETY: the slot lies in this stack, whose previous task's context gave up its contents.
    unsafe { child.write(TaskCtx::new(worker, run)) };
    // SAFETY: caller contract.
    let scope = unsafe { scope.read() };
    let cpu_owner = cpu_scoped(ctx).then(|| crate::pool::cpu_scoped::next_owner(ctx));
    let (index, generation, child) = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        let current = s.current;
        let canceled = s.canceled(shield).map(|_| CANCEL_PARENT);
        let parent = s.task(current);
        let mut path = parent.path.clone();
        path.push(parent.spawned);
        parent.spawned += 1;
        let (index, generation) = s.take_task_slot();
        #[cfg(feature = "ownership-trace")]
        unsafe {
            (*child).task_id = index;
            crate::ownership_trace::task((*child).run_ref(), handle(index, generation), &path);
            let binding = crate::ownership_trace::take_spawn(ctx);
            crate::ownership_trace::spawned((*child).run_ref(), binding, handle(index, generation));
        }
        let mut task = Task::new(generation, child, Some(scope), path, canceled);
        if let Some(owner) = cpu_owner {
            task.owner = owner;
            task.cpu_scoped = true;
        }
        task.code = Some((code, environment, operation_site));
        task.priority = s.schedule.admit();
        unsafe {
            (*child).task_id = index;
            (*child).task_sp = stack.prepare(
                task_entry,
                child.cast(),
                handle(index, generation) as usize as *mut u8,
            );
        }
        task.stack = Some(stack);
        let path = task.path.clone();
        s.tasks[index as usize] = Some(task);
        s.scope(scope).children.push(index);
        s.link_child(current, index);
        (index, generation, path)
    });
    // A free-mode replay never starts a child its cut lacks.
    // SAFETY: the context is live.
    if let (Some(started), Some(metrics)) =
        (started, unsafe { (*ctx).run_ref() }.worker_metrics.as_ref())
    {
        metrics.spawn(started.elapsed());
    }
    (index, generation, child)
}

/// `$intrinsic.TaskFinish[R](mut slot, none, size)`: the running child computed `Some(r)` in
/// `slot`. A detached child keeps it, to drop; otherwise the child holds it, stopped in this
/// call, until a join's poll takes it (leaving `none`) and a join or a discard resumes it.
///
/// # Safety
/// `ctx` is live; `slot` and `none` are `Option[R]` images of `size` bytes, `slot` in the
/// running child's frame. ABI38 adds its linked descriptor; u32::MAX is
/// unavailable only for legacy Rust stand-ins and never certifies payload bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_finish(
    ctx: *mut TaskCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
    descriptor: u32,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_finish");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::lifecycle::finish(ctx, slot, none, size, descriptor),
            )
        };
    }
    if free::enabled(ctx) {
        return unsafe { crate::stop::check(ctx, free::lifecycle::finish(ctx, slot, none, size)) };
    }
    // SAFETY: caller contract.
    let (held, image) = unsafe { Slot::held(slot, none, size) };
    let Some((current, path)) = role::with_role(ctx, |s| {
        let current = s.current;
        let row = s.task(current);
        if row.scope.is_none() {
            fault("the root task finished as a child");
        }
        if row.detached {
            return None;
        }
        // Publish result readiness now. Once a join or scope exit resumes its cleanup,
        // completed publishes a separate actual-completion op, including deferred roots.
        row.ended = true;
        Some((current, row.path.clone()))
    }) else {
        return STATUS_OK;
    };
    crate::free::end(ctx, &path);
    unsafe {
        role::with_held(ctx, |s| {
            let row = s.task(current);
            row.held_none = Some(image);
            row.slot = Some(held);
            row.state = State::Holding;
            s.order(current);
            s.settle(current);
        })
    };
    // SAFETY: the context is live; a join or a discard hands control back here.
    unsafe { crate::stop::check(ctx, run_next(&mut *ctx)) };
    crate::free::resume_rest(ctx);
    STATUS_OK
}

/// `$intrinsic.TaskJoin[R](s, take h, mut slot, size)`: await `h` (CONC-2). Its result moves
/// into `slot` (an `Option[R]`, `None` on entry); a failed child's failures continue the running
/// task's chain and the status is 1. Joins are not cancellation points.
///
/// # Safety
/// `ctx` is live, `scope` and `task` name a child of a live scope, and `slot` is a `None` image
/// of `size` bytes in the running task's frame.
#[cfg_attr(not(feature = "operation-latency"), unsafe(no_mangle))]
pub unsafe extern "C" fn hirsel_rt_v2_task_join(
    ctx: *mut TaskCtx,
    caller_site: u64,
    _operation_site: u64,
    scope: *const u64,
    task: *mut u64,
    slot: *mut u8,
    size: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_join");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::lifecycle::join(ctx, scope, task, slot, size, caller_site),
            )
        };
    }
    if free::enabled(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                free::lifecycle::join(ctx, scope.read(), task.read(), slot, size, caller_site),
            )
        };
    }
    // SAFETY: caller contract.
    let (scope, child) = unsafe { child_handle(ctx, scope.read(), task.read()) };
    crate::free::enter_call(ctx);
    // SAFETY: caller contract.
    unsafe { join(&mut *ctx, scope, child, slot, size, caller_site) }
}

/// Join `child` of `scope`, as [`hirsel_rt_v2_task_join`] does.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_join`].
unsafe fn join(
    ctx: &mut TaskCtx,
    scope: u32,
    child: u32,
    slot: *mut u8,
    size: u64,
    caller_site: u64,
) -> u8 {
    let mut state = unsafe { role::with_held(ctx, |t| t.task(child).state) };
    if free::enabled(ctx) && !unsafe { crate::pool::free::is_saved(ctx.run_ptr(), child) } {
        state = State::Parked;
    }
    match state {
        State::Holding => {
            let held = unsafe { role::with_held(ctx, |t| t.task(child).slot.take().unwrap()) };
            // SAFETY: the child is stopped in its finish, its slot live.
            unsafe {
                deliver(held, slot, size);
                role::with_held(ctx, |s| cpu_job_event(s.task(child), "transfer"));
                hand_off(ctx, child);
                crate::free::done(ctx);
                yield_now(ctx);
            }
        }
        State::Done => unsafe {
            crate::free::done(ctx);
            yield_now(ctx)
        },
        State::Ready | State::Parked => {
            crate::free::publish(ctx, Phase::Entered);
            // SAFETY: the slot is in this task's frame, parked with it.
            unsafe { block(ctx, Wait::Join { child, slot, size }, true, caller_site) };
            // The poll moved a holding child's result; its frame finishes now.
            if unsafe { role::with_held(ctx, |t| t.task(child).state == State::Holding) } {
                unsafe { hand_off(ctx, child) };
            }
            crate::free::done(ctx);
        }
    }
    let (chain, retired) = unsafe {
        role::with_held(ctx, |t| {
            let chain = t.task(child).failure.take();
            t.scope(scope).children.retain(|&c| c != child);
            (chain, t.retire_task(child))
        })
    };
    // The row's owned storage is destroyed outside the table borrow.
    drop(retired);
    match chain {
        Some(chain) => {
            ctx.failure.append_chain(chain);
            STATUS_FAILURE
        }
        None => STATUS_OK,
    }
}

/// `task.cancel(s, h)`: cancel the child's subtree (`Requested` for the child, `Parent` below).
///
/// # Safety
/// `ctx` is live and `scope`, `task` name a child of a live scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_cancel(
    ctx: *mut TaskCtx,
    scope: *const u64,
    task: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_cancel");
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::sets::cancel(ctx, scope, task)) };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::cancellation::cancel(ctx, scope.read(), task.read()),
            )
        };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let (_, child) = unsafe { child_handle(ctx, scope.read(), task.read()) };
    crate::free::enter(ctx, Phase::Done);
    unsafe { role::with_held(ctx, |t| t.cancel(child, CANCEL_REQUESTED)) };
    crate::free::done(ctx);
    // SAFETY: the context is live.
    unsafe { yield_now(&mut *ctx) };
    STATUS_OK
}

/// `task.detach(s, take h)`: the child keeps running inside its scope; its result is dropped
/// when it completes, at once when it already has.
///
/// # Safety
/// `ctx` is live and `scope`, `task` name a child of a live scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_detach(
    ctx: *mut TaskCtx,
    scope: *const u64,
    task: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_detach");
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::lifecycle::detach(ctx, scope, task)) };
    }
    if free::enabled(ctx) {
        unsafe { crate::stop::check(ctx, free::lifecycle::detach(ctx, scope.read(), task.read())) };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let (scope, child) = unsafe { child_handle(ctx, scope.read(), task.read()) };
    crate::free::enter_call(ctx);
    // SAFETY: the context is live.
    unsafe {
        detach(&mut *ctx, scope, child);
        crate::free::done(ctx);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// Detach `child` of `scope`: a holding child drops its result now, and a completed one
/// leaves its scope's rows unless it failed.
///
/// # Safety
/// `ctx` is live and `child` a child of live `scope`.
unsafe fn detach(ctx: &mut TaskCtx, scope: u32, child: u32) {
    let saved = !free::enabled(ctx) || unsafe { crate::pool::free::is_saved(ctx.run_ptr(), child) };
    let holding = role::with_role(ctx, |t| {
        let row = t.task(child);
        row.detached = true;
        let holding = saved && row.state == State::Holding;
        if holding {
            row.slot = None;
        }
        holding
    });
    if holding {
        // SAFETY: the child is holding; its own cleanup drops the result.
        unsafe { hand_off(ctx, child) };
    }
    let retired = role::with_role(ctx, |t| {
        let done = t.tasks[child as usize]
            .as_ref()
            .is_some_and(|row| row.state == State::Done && row.failure.is_none());
        (done && saved).then(|| {
            t.scope(scope).children.retain(|&c| c != child);
            t.retire_task(child)
        })
    });
    drop(retired);
}

/// The live scope a Scope word names and its task set a TaskSet word names.
///
/// # Safety
/// The words are the caller's; a stale or foreign one faults.
unsafe fn set_handle(ctx: *mut TaskCtx, scope: u64, set: u64) -> (u32, u32) {
    role::with_role(ctx, |t| {
        let scope = t.scope_handle(scope);
        (scope, t.set_handle(set, scope))
    })
}

/// The live scope a Scope word names and its child a Task word names.
///
/// # Safety
/// The words are the caller's; a stale or foreign one faults.
unsafe fn child_handle(ctx: *mut TaskCtx, scope: u64, task: u64) -> (u32, u32) {
    role::with_role(ctx, |t| {
        let scope = t.scope_handle(scope);
        (scope, t.task_handle(task, scope))
    })
}

/// `task.set(s) -> TaskSet[R] of s` (CONC-2): an empty task set of `s`.
///
/// # Safety
/// `ctx` is live, `out` writable and `scope` names a live scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_set(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_set");
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::sets::create(ctx, scope, out)) };
    }
    if free::enabled(ctx) {
        unsafe { crate::stop::check(ctx, free::sets::create(ctx, scope.read(), out)) };
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    // SAFETY: caller contract.
    let scope = unsafe { scope.read() };
    let (index, generation) = role::with_role(ctx, |t| {
        let scope = t.scope_handle(scope);
        let (index, generation) = t.take_set_slot();
        t.sets[index as usize] = Some(TaskSet {
            generation,
            scope,
            members: Vec::new(),
        });
        (index, generation)
    });
    crate::free::done(ctx);
    // SAFETY: caller contract.
    unsafe {
        out.write(
            crate::channels::ENDPOINT | TASK_SET | u64::from(generation) << 32 | u64::from(index),
        );
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `task.add(s, mut set, take h)`: `h` becomes the set's next member.
///
/// # Safety
/// `ctx` is live and `set`, `task` name a task set and a child of live `scope`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_add(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *mut u64,
    task: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_add");
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::sets::add(ctx, scope, set, task)) };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::sets::add(ctx, scope.read(), set.read(), task.read()),
            )
        };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let (scope, set, word) = unsafe { (scope.read(), set.read(), task.read()) };
    let set = role::with_role(ctx, |t| {
        let scope = t.scope_handle(scope);
        t.task_handle(word, scope);
        t.set_handle(set, scope)
    });
    crate::free::enter(ctx, Phase::Done);
    role::with_role(ctx, |t| t.set(set).members.push(Some(word)));
    crate::free::done(ctx);
    // SAFETY: the context is live.
    unsafe { yield_now(&mut *ctx) };
    STATUS_OK
}

/// `Option[u64]`'s image: the tag, then the payload.
#[repr(C)]
pub struct IndexOption {
    tag: u32,
    padding: u32,
    index: u64,
}

/// `task.await_any(s, set) -> Option[u64]`: the add-order index of the unawaited member that
/// completed first, waiting until one has; `None` when every member was awaited. A join, not
/// a cancellation point; completion order is the schedule's, so a replay chooses the same.
///
/// # Safety
/// `ctx` is live, `out` writable and `set` names a task set of live `scope`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_await_any(
    ctx: *mut TaskCtx,
    caller_site: u64,
    _operation_site: u64,
    _destination: *mut u8,
    out: *mut IndexOption,
    scope: *const u64,
    set: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_await_any");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::sets::await_any(ctx, scope, set, out, caller_site),
            )
        };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::sets::await_any(ctx, scope.read(), set.read(), out, caller_site),
            )
        };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let (_, set) = unsafe { set_handle(ctx, scope.read(), set.read()) };
    let waiting = role::with_role(ctx, |t| t.set(set).members.iter().any(Option::is_some));
    let index = if crate::free::enter(ctx, Phase::Entered) {
        // SAFETY: the set outlives the wait: the caller borrows it.
        unsafe { free_await_any(ctx, set, waiting, caller_site) }
    } else {
        if waiting && role::with_role(ctx, |t| t.first_completed(set)).is_none() {
            // SAFETY: the set outlives the wait: the caller borrows it.
            unsafe {
                block(
                    &mut *ctx,
                    Wait::Any { set, member: None },
                    true,
                    caller_site,
                )
            };
        } else {
            // SAFETY: the context is live.
            unsafe { yield_now(&mut *ctx) };
        }
        if waiting {
            role::with_role(ctx, |t| t.first_completed(set))
        } else {
            None
        }
    };
    // SAFETY: caller contract.
    unsafe {
        out.write(IndexOption {
            tag: u32::from(index.is_some()),
            padding: 0,
            index: index.unwrap_or(0),
        })
    };
    STATUS_OK
}

/// `task.await_any` in free mode: the member is live the first completed and replayed the
/// recorded one, waited for (k, entered) until it completed; then (k, done).
///
/// # Safety
/// `ctx` is live and `set` a task set row that outlives the wait.
unsafe fn free_await_any(
    ctx: *mut TaskCtx,
    set: u32,
    waiting: bool,
    caller_site: u64,
) -> Option<u64> {
    let mut entered = false;
    let index = loop {
        if !waiting {
            break None;
        }
        let (live, finished) = role::with_role(ctx, |s| {
            let live = s.first_completed(set);
            let count = s.set(set).members.len() as u64;
            let finished: Vec<bool> = (0..count).map(|i| s.member_completed(set, i)).collect();
            (live, finished)
        });
        let member = crate::free::await_any(ctx, live, |i| {
            finished.get(i as usize).copied().unwrap_or(false)
        });
        if member.is_some() {
            break member;
        }
        if !entered {
            crate::free::publish(ctx, Phase::Entered);
            entered = true;
        }
        let member = crate::free::await_any_member(ctx);
        // SAFETY: caller contract.
        unsafe { block(&mut *ctx, Wait::Any { set, member }, false, caller_site) };
    };
    crate::free::done(ctx);
    if !entered {
        // SAFETY: the context is live.
        unsafe { yield_now(&mut *ctx) };
    }
    index
}

/// `$intrinsic.TaskSetHas(s, set, index) -> bool`: whether member `index` is unawaited, which
/// `task.await_at`'s compiled body checks (CONC-2) before it takes and joins it. Not a yield.
///
/// # Safety
/// `ctx` is live, `out` writable and `set` names a task set of live `scope`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_set_has(
    ctx: *mut TaskCtx,
    out: *mut u8,
    scope: *const u64,
    set: *const u64,
    index: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_set_has");
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::sets::has(ctx, scope, set, index, out)) };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::sets::has(ctx, scope.read(), set.read(), index, out),
            )
        };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let (_, set) = unsafe { set_handle(ctx, scope.read(), set.read()) };
    let has = role::with_role(ctx, |t| {
        usize::try_from(index)
            .ok()
            .and_then(|i| t.set(set).members.get(i))
            .is_some_and(Option::is_some)
    });
    // SAFETY: caller contract.
    unsafe { out.write(u8::from(has)) };
    STATUS_OK
}

/// `$intrinsic.TaskSetTake(s, mut set, index) -> Task[R] of s`: unawaited member `index`'s Task
/// word, which the set gives up for the join that follows. Not a yield.
///
/// # Safety
/// `ctx` is live, `out` writable, `set` names a task set of live `scope` and its member
/// `index` is unawaited.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_set_take(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
    set: *mut u64,
    index: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_set_take");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(ctx, ordered::sets::take(ctx, scope, set, index, out))
        };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::sets::take(ctx, scope.read(), set.read(), index, out),
            )
        };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let (_, set) = unsafe { set_handle(ctx, scope.read(), set.read()) };
    let word = role::with_role(ctx, |t| {
        usize::try_from(index)
            .ok()
            .and_then(|i| t.set(set).members.get_mut(i))
            .and_then(Option::take)
            .unwrap_or_else(|| fault("a task set member taken twice"))
    });
    // SAFETY: caller contract.
    unsafe { out.write(word) };
    STATUS_OK
}

/// `task.cancel_all(s, take set)`: cancel every unawaited member (`Requested`), detach it, and
/// wait until each has completed; their results are dropped and a failure among them belongs
/// to the scope, as a detached child's does.
///
/// # Safety
/// `ctx` is live and `set` names a task set of live `scope`, which the call takes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_cancel_all(
    ctx: *mut TaskCtx,
    caller_site: u64,
    _operation_site: u64,
    scope: *const u64,
    set: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_cancel_all");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::sets::release(ctx, scope, set, true, caller_site),
            )
        };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::sets::release(ctx, scope.read(), set.read(), true, caller_site),
            )
        };
        return STATUS_OK;
    }
    crate::free::enter_call(ctx);
    // SAFETY: caller contract.
    let set = unsafe { release_members(ctx, scope, set, true) };
    // SAFETY: the context is live; the set row stays until the drain is over.
    if role::with_role(ctx, |t| unsafe { poll_drained(t, set) }) {
        crate::free::done(ctx);
        unsafe { yield_now(&mut *ctx) };
    } else {
        crate::free::publish(ctx, Phase::Entered);
        unsafe { block(&mut *ctx, Wait::Drain { set }, true, caller_site) };
        crate::free::done(ctx);
    }
    role::with_role(ctx, |t| t.close_set(set));
    STATUS_OK
}

/// `task.detach_all(s, take set)`: detach every unawaited member.
///
/// # Safety
/// `ctx` is live and `set` names a task set of live `scope`, which the call takes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_detach_all(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_detach_all");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(ctx, ordered::sets::release(ctx, scope, set, false, 0))
        };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::sets::release(ctx, scope.read(), set.read(), false, 0),
            )
        };
        return STATUS_OK;
    }
    crate::free::enter_call(ctx);
    // SAFETY: caller contract.
    let set = unsafe { release_members(ctx, scope, set, false) };
    role::with_role(ctx, |t| t.close_set(set));
    crate::free::done(ctx);
    // SAFETY: the context is live.
    unsafe { yield_now(&mut *ctx) };
    STATUS_OK
}

/// Detach every unawaited member of `set`, canceling each first when `cancel`.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_cancel_all`].
unsafe fn release_members(
    ctx: *mut TaskCtx,
    scope: *const u64,
    set: *mut u64,
    cancel: bool,
) -> u32 {
    // SAFETY: caller contract.
    let (scope, set) = unsafe { set_handle(ctx, scope.read(), set.read()) };
    let members: Vec<u64> = role::with_role(ctx, |t| {
        t.set(set).members.iter().flatten().copied().collect()
    });
    for word in members {
        let child = role::with_role(ctx, |t| {
            let child = t.task_handle(word, scope);
            if cancel {
                t.cancel(child, CANCEL_REQUESTED);
            }
            child
        });
        // SAFETY: caller contract.
        unsafe { detach(&mut *ctx, scope, child) };
    }
    set
}

/// Whether every unawaited member of `set` has completed.
///
/// # Safety
/// `set` is a live task set row.
unsafe fn poll_drained(s: &mut Scheduler, set: u32) -> bool {
    let members = s.set(set).members.clone();
    members.iter().flatten().all(|&word| {
        s.member(word)
            .is_none_or(|t| s.task(t).state == State::Done)
    })
}

/// `$intrinsic.TaskCanceled(s)` (CONC-3): 0, or 1 + the running task's cancellation cause
/// (`CancelCause`'s ordinal); the operation yields, as `task.check_canceled` does.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_canceled(
    ctx: *mut TaskCtx,
    out: *mut u32,
    _scope: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "task_canceled");
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::sets::canceled(ctx, out)) };
    }
    if free::enabled(ctx) {
        unsafe { crate::stop::check(ctx, free::cancellation::check(ctx, out)) };
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    // SAFETY: the context is live.
    let cause = unsafe { observed_cancel(ctx) };
    crate::free::done(ctx);
    // SAFETY: caller contract.
    unsafe {
        out.write(cause.map_or(0, |cause| cause + 1));
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// The running task's cancellation cause, if any: for `task.check_canceled` and the
/// cancellation points of blocking Io (CONC-3). Shielded destruction observes none.
///
/// # Safety
/// `ctx` is live.
pub(crate) unsafe fn canceled(ctx: *mut TaskCtx) -> Option<u32> {
    // SAFETY: the context is live.
    let shield = unsafe { (*ctx).shield };
    unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() }.and_then(|s| s.canceled(shield))
}

/// The running task's cancellation at a check: live its own; in a free-mode replay, unless
/// shielded, the recorded one.
///
/// # Safety
/// `ctx` is live.
unsafe fn observed_cancel(ctx: *mut TaskCtx) -> Option<u32> {
    // SAFETY: the context is live.
    let live = unsafe { canceled(ctx) };
    if unsafe { (*ctx).shield } != 0 {
        return live;
    }
    crate::free::cancel_check(ctx, live)
}

/// A receive's outcome for `task`, unless it must wait: canceled, else the oldest message
/// (moved into `slot`), else closed once no sender is left.
///
/// # Safety
/// `slot` is a `None` message slot in `task`'s frame.
unsafe fn receive_ready(
    s: &mut Scheduler,
    task: u32,
    shield: u32,
    channel: u32,
    slot: *mut u8,
) -> Option<u32> {
    #[cfg(feature = "ownership-trace")]
    let owner = handle(task, s.task(task).generation);
    if let Some(cause) = s.canceled_task(task, shield) {
        #[cfg(feature = "ownership-trace")]
        {
            let mut binding = s.task(task).ownership_queue.take();
            s.channels.payload(channel).canceled_observed(
                owner,
                &mut binding,
                crate::channels::CANCELED + cause,
            );
            s.task(task).ownership_queue = binding;
        }
        return Some(crate::channels::CANCELED + cause);
    }
    // SAFETY: caller contract.
    match unsafe {
        #[cfg(not(feature = "ownership-trace"))]
        {
            s.channels.receive(channel, slot)
        }
        #[cfg(feature = "ownership-trace")]
        {
            let mut binding = s.task(task).ownership_queue.take();
            let result = s
                .channels
                .receive_observed(channel, slot, owner, &mut binding);
            s.task(task).ownership_queue = binding;
            result
        }
    } {
        Ok(true) => Some(crate::channels::DONE),
        Ok(false) => None,
        Err(status) => Some(status),
    }
}

/// `$intrinsic.ChannelOpen[T](tasks, none, descriptor) -> (Sender[T], Receiver[T])` (CONC-6):
/// a channel of `Option[T]` images, `descriptor` their layout's index and `none` the `None`
/// image a send leaves behind.
///
/// # Safety
/// `ctx` is live, `out` writable for two words, `descriptor` an index of the run's registry
/// and `none` a `None` image of that layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_channel_open(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
    none: *const u8,
    descriptor: u32,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "channel_open");
    if ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                ordered::engine::channel_open(ctx, out, none, descriptor),
            )
        };
        return STATUS_OK;
    }
    if free::enabled(ctx) {
        unsafe { crate::stop::check(ctx, free::channel::open(ctx, out, none, descriptor)) };
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    // SAFETY: caller contract.
    let layout = unsafe { (*ctx).descriptor(descriptor) };
    let element = unsafe { crate::channels::Element::new(descriptor, layout, none) };
    // SAFETY: a registry descriptor is immutable program data.
    let (sender, receiver, channel) = role::with_role(ctx, |t| {
        let (sender, receiver) = t.channels.open(element, unsafe { &*layout });
        let channel = t.channels.channel(sender, true);
        #[cfg(feature = "ownership-trace")]
        unsafe {
            crate::ownership_trace::queue_created_here(ctx, &t.channels.payload(channel));
        }
        (sender, receiver, channel)
    });
    crate::free::create(ctx, (crate::free::CHANNEL, u64::from(channel)));
    crate::free::done(ctx);
    // SAFETY: caller contract.
    unsafe {
        out.write(sender);
        out.add(1).write(receiver);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `sender.dup(tasks, s) -> Sender[T]`: another Sender of `s`'s channel.
///
/// # Safety
/// `ctx` is live, `out` writable and `sender` an open Sender word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_sender_dup(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
    sender: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "sender_dup");
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::engine::sender_dup(ctx, sender.read(), out)) };
        return STATUS_OK;
    }
    if free::enabled(ctx) {
        let left = unsafe { crate::stop::check(ctx, free::channel::dup(ctx, sender.read(), out)) };
        return if left {
            crate::STATUS_SUSPENDED
        } else {
            STATUS_OK
        };
    }
    crate::free::enter(ctx, Phase::Done);
    // SAFETY: caller contract.
    let sender = unsafe { sender.read() };
    let channel = role::with_role(ctx, |t| t.channels.channel(sender, true));
    crate::free::touch(ctx, (crate::free::CHANNEL, u64::from(channel)), "dup");
    let word = role::with_role(ctx, |t| t.channels.dup(sender));
    crate::free::done(ctx);
    unsafe {
        out.write(word);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `$intrinsic.ChannelSend[T](mut s, mut message)`: queue the `Some` image of `message`,
/// leaving `None`, unless no receiver is left; then the message stays and the status is
/// `Closed`. Send never checks cancellation (CONC-3).
///
/// # Safety
/// `ctx` is live, `out` writable, `sender` an open Sender word and `message` a `Some` image of
/// its channel's element.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_channel_send(
    ctx: *mut TaskCtx,
    out: *mut u32,
    sender: *mut u64,
    message: *mut u8,
) -> u8 {
    // SAFETY: caller contract; a resolved immediate send needs no role. A
    // reissued send continues its replay wait under the role.
    if !unsafe { (*ctx).wait_left }
        && let Some(result) =
            unsafe { free::objects::channel_send(ctx, sender.read(), message, out) }
    {
        return unsafe { free::objects::finish(ctx, result) };
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "channel_send");
    if ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                ordered::engine::channel_send(ctx, sender.read(), message, out),
            )
        };
        return STATUS_OK;
    }
    if free::enabled(ctx) {
        let left = unsafe {
            crate::stop::check(ctx, free::channel::send(ctx, sender.read(), message, out))
        };
        return if left {
            crate::STATUS_SUSPENDED
        } else {
            STATUS_OK
        };
    }
    crate::free::enter(ctx, Phase::Done);
    // SAFETY: caller contract.
    let sender = unsafe { sender.read() };
    let channel = role::with_role(ctx, |t| t.channels.channel(sender, true));
    let object = (crate::free::CHANNEL, u64::from(channel));
    crate::free::wait_until(ctx, |ctx| crate::free::enabled(ctx, object));
    // SAFETY: caller contract.
    let status = role::with_role(ctx, |t| unsafe {
        #[cfg(not(feature = "ownership-trace"))]
        {
            t.channels.send(channel, message)
        }
        #[cfg(feature = "ownership-trace")]
        {
            t.channels
                .send_observed(
                    channel,
                    message,
                    false,
                    (*ctx).ownership_task,
                    (*ctx).ownership_queue.take(),
                )
                .0
        }
    });
    let outcome = if status == crate::channels::DONE {
        "send"
    } else {
        "send closed"
    };
    crate::free::touch(ctx, object, outcome);
    crate::free::done(ctx);
    // SAFETY: caller contract.
    unsafe {
        out.write(status);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `$intrinsic.ChannelReceive[T](mut r, mut message)`: move the oldest message into the
/// `None` slot `message`, parking while the queue is empty and a sender is left.
///
/// # Safety
/// `ctx` is live, `out` writable, `receiver` an open Receiver word and `message` a `None` image
/// of its channel's element in the running task's frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_channel_receive(
    ctx: *mut TaskCtx,
    caller_site: u64,
    operation_site: u64,
    out: *mut u32,
    receiver: *mut u64,
    message: *mut u8,
) -> u8 {
    // A replay may stop at the receive, before the ordinary wait stores
    // caller_site. Retain the checked operation code in this task's context.
    let previous = unsafe { (*ctx).run_ref().free_replay }
        .then(|| unsafe { std::mem::replace(&mut (*ctx).operation_site, operation_site) });
    // SAFETY: caller contract; a resolved receive needs no role.
    if let Some(result) =
        unsafe { free::objects::channel_receive(ctx, receiver.read(), message, out, caller_site) }
    {
        let status = unsafe { free::objects::finish(ctx, result) };
        if let Some(previous) = previous {
            unsafe { (*ctx).operation_site = previous };
        }
        return status;
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "channel_receive");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::lifecycle::receive(ctx, receiver, message, out, caller_site),
            )
        };
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::channel::receive(ctx, receiver.read(), message, out, caller_site),
            )
        };
        if let Some(previous) = previous {
            unsafe { (*ctx).operation_site = previous };
        }
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let receiver = unsafe { receiver.read() };
    let channel = role::with_role(ctx, |t| t.channels.channel(receiver, false));
    if crate::free::enter(ctx, Phase::Entered) {
        // SAFETY: caller contract.
        let status = unsafe { free_receive(ctx, channel, message) };
        unsafe { out.write(status) };
        return STATUS_OK;
    }
    let shield = unsafe { (*ctx).shield };
    // SAFETY: caller contract.
    let (current, ready) = role::with_role(ctx, |s| {
        let current = s.current;
        #[cfg(feature = "ownership-trace")]
        if let Some(binding) = unsafe { (*ctx).ownership_queue.take() } {
            s.task(current).ownership_queue = Some(binding);
        }
        (current, unsafe {
            receive_ready(s, current, shield, channel, message)
        })
    });
    let status = match ready {
        Some(status) => {
            // SAFETY: the context is live.
            unsafe { yield_now(&mut *ctx) };
            status
        }
        None => {
            // SAFETY: the slot is in this task's frame, parked with it.
            unsafe {
                block(
                    &mut *ctx,
                    Wait::Receive {
                        channel,
                        slot: message,
                    },
                    true,
                    caller_site,
                )
            };
            role::with_role(ctx, |t| t.task(current).status)
        }
    };
    // SAFETY: caller contract.
    unsafe { out.write(status) };
    STATUS_OK
}

/// A receive in free mode: it waits (k, entered) replay-pending, live until a send, a
/// sender's close or a cancellation, for a message, `Closed` or the cancellation its one
/// check observes; then it acts and publishes (k, done).
///
/// # Safety
/// As for [`hirsel_rt_v2_channel_receive`].
unsafe fn free_receive(ctx: *mut TaskCtx, channel: u32, message: *mut u8) -> u32 {
    let mut entered = false;
    loop {
        // SAFETY: the context is live.
        let live = unsafe { canceled(ctx) };
        let shielded = unsafe { (*ctx).shield } != 0;
        let cancel = if shielded {
            live
        } else {
            crate::free::cancel_peek(ctx, live)
        };
        if cancel.is_some() || role::with_role(ctx, |t| t.channels.ready(channel)) {
            break;
        }
        if !entered {
            crate::free::publish(ctx, Phase::Entered);
            entered = true;
        }
        crate::tasks::park_retry(ctx, true);
    }
    // SAFETY: the context is live.
    let (status, outcome) = match unsafe { observed_cancel(ctx) } {
        Some(cause) => {
            #[cfg(feature = "ownership-trace")]
            role::with_role(ctx, |s| unsafe {
                s.channels.payload(channel).canceled_observed(
                    (*ctx).ownership_task,
                    &mut (*ctx).ownership_queue,
                    crate::channels::CANCELED + cause,
                );
            });
            (
                crate::channels::CANCELED + cause,
                format!("canceled {cause}"),
            )
        }
        // SAFETY: caller contract.
        None => match role::with_role(ctx, |t| unsafe {
            #[cfg(not(feature = "ownership-trace"))]
            {
                t.channels.receive(channel, message)
            }
            #[cfg(feature = "ownership-trace")]
            {
                t.channels.receive_observed(
                    channel,
                    message,
                    (*ctx).ownership_task,
                    &mut (*ctx).ownership_queue,
                )
            }
        }) {
            Ok(true) => (crate::channels::DONE, "msg".into()),
            Err(status) => (status, "closed".into()),
            Ok(false) => fault("a free-mode receive acted on an empty open channel"),
        },
    };
    crate::free::receive(ctx, channel, &outcome);
    crate::free::done(ctx);
    if !entered {
        // SAFETY: the context is live.
        unsafe { yield_now(&mut *ctx) };
    }
    status
}

/// Close the channel endpoint `word` (a dropped Sender or Receiver); a last receiver releases
/// the messages nobody received, oldest first. Closing is not a yield.
///
/// # Safety
/// `ctx` is live and `word` an open endpoint of its run.
pub(crate) unsafe fn close_endpoint(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "endpoint.close");
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::drops::endpoint(ctx, word, anchor, operation_site),
            )
        };
    }
    if word & TASK_SET != 0 {
        if free::enabled(ctx) {
            unsafe { crate::stop::check(ctx, free::sets::close(ctx, word)) };
            return STATUS_OK;
        }
        // A dropped task set: its members stay children of their scope, joined at its exit.
        role::with_role(ctx, |s| {
            let index = word as u32;
            match s.sets.get(index as usize).and_then(Option::as_ref) {
                Some(set)
                    if set.generation
                        == ((word & !(crate::channels::ENDPOINT | TASK_SET)) >> 32) as u32 =>
                {
                    s.close_set(index)
                }
                _ => fault("a dropped TaskSet handle of a closed set"),
            }
        });
        return STATUS_OK;
    }
    // SAFETY: caller contract; a resolved endpoint's close needs no role.
    if let Some(result) = unsafe { free::objects::channel_close(ctx, word, anchor, operation_site) }
    {
        return unsafe { free::objects::closed(ctx, result) };
    }
    if free::enabled(ctx) {
        return unsafe {
            crate::stop::check(ctx, free::channel::close(ctx, word, anchor, operation_site))
        };
    }
    let (channel, sender) = role::with_role(ctx, |t| t.channels.endpoint_of(word));
    let outcome = if sender {
        "close sender"
    } else {
        "close receiver"
    };
    if crate::free::close(ctx, (crate::free::CHANNEL, u64::from(channel)), outcome) == Some(false) {
        return STATUS_OK;
    }
    let Some((left, descriptor)) = role::with_role(ctx, |t| {
        #[cfg(not(feature = "ownership-trace"))]
        {
            t.channels.close(word)
        }
        #[cfg(feature = "ownership-trace")]
        {
            t.channels
                .close_observed(word, false, unsafe { (*ctx).ownership_task })
        }
    }) else {
        return STATUS_OK;
    };
    let mut status = STATUS_OK;
    // The detached queue outlives every drop, whatever channels the drops open or close.
    for i in 0..left.len() {
        #[cfg(feature = "ownership-trace")]
        let mut observation =
            std::mem::MaybeUninit::<crate::ownership_trace::QueueCleanupScope>::uninit();
        #[cfg(feature = "ownership-trace")]
        // SAFETY: this fiber frame keeps scope and detached image stable through finalization.
        let observing = unsafe {
            crate::ownership_trace::queue_cleanup_begin(
                ctx,
                observation.as_mut_ptr(),
                left.cleanup_tag(),
                i,
                left.at(i),
            )
        };
        let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
        // SAFETY: each queued image owns its value, which nothing else reaches.
        status |= unsafe {
            crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                descriptor,
                left.at(i),
                std::ptr::null(),
                continuation.as_mut_ptr(),
                anchor,
                operation_site,
            )
        };
        #[cfg(feature = "ownership-trace")]
        if observing {
            // SAFETY: the same task's complete ordinary finalization returned.
            unsafe { crate::ownership_trace::queue_cleanup_end(ctx, observation.as_mut_ptr()) };
        }
    }
    status
}

/// `$intrinsic.MutexCreate[T]` (CONC-5): move the `Option[T]` image into a
/// fresh cell and return its counted handle wrapper, leaving `None` in `payload`.
///
/// # Safety
/// `ctx` is live; `out` is writable; `descriptor` describes the `Some` image
/// at `payload` and the `None` image at `none`; `wrapper` describes an eight-byte
/// handle with a CloseHandle drop step.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_create(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    payload: *mut u8,
    none: *const u8,
    descriptor: u32,
    wrapper: u32,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "mutex_create");
    // SAFETY: caller contract.
    if ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                ordered::engine::mutex_create(ctx, out, payload, none, descriptor, wrapper),
            )
        };
        return STATUS_OK;
    }
    if free::mutex::live(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::mutex::create(ctx, out, payload, none, descriptor, wrapper),
            )
        };
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    unsafe {
        let layout = (*ctx).descriptor(descriptor);
        let element = crate::channels::Element::new(descriptor, layout, none);
        let word = role::with_role(ctx, |t| t.sync.create_mutex(element, &*layout, payload));
        let block =
            crate::alloc::hirsel_rt_v2_alloc(ctx, wrapper, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        block.add(1).cast::<u64>().write(word);
        crate::free::create(ctx, (crate::free::MUTEX, word));
        crate::free::done(ctx);
        out.write(block as u64);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `$intrinsic.MutexTake[T](m, mut payload)`: move the payload image into the `None` slot
/// (status 0), or report the mutex dead (status 1). Acquiring is not a yield.
///
/// # Safety
/// `ctx` is live, `out` writable, `m` borrows a counted Mutex wrapper and `payload` a writable `None` carrier.
#[cfg_attr(not(feature = "operation-latency"), unsafe(no_mangle))]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_take(
    ctx: *mut TaskCtx,
    out: *mut u32,
    m: *const u64,
    payload: *mut u8,
) -> u8 {
    // SAFETY: caller contract; a resolved available take needs no role. A
    // reissued take continues its replay wait under the role.
    if !unsafe { (*ctx).wait_left }
        && let Some(result) = unsafe { free::objects::mutex_take(ctx, mutex_word(m), payload, out) }
    {
        return unsafe { free::objects::finish(ctx, result) };
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "mutex_take");
    // SAFETY: caller contract.
    let word = unsafe { mutex_word(m) };
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::engine::mutex_take(ctx, word, payload, out)) };
        return STATUS_OK;
    }
    if free::mutex::live(ctx) {
        let left = unsafe { crate::stop::check(ctx, free::mutex::take(ctx, word, payload, out)) };
        return if left {
            crate::STATUS_SUSPENDED
        } else {
            STATUS_OK
        };
    }
    let free = crate::free::enter(ctx, Phase::Entered);
    let mut entered = false;
    if free {
        // A take waits (k, entered) for its mutex, which another task's body may hold
        // across that body's ops, and in a replay for its version.
        let object = (crate::free::MUTEX, word);
        while !(role::with_role(ctx, |t| t.sync.available(word))
            && crate::free::enabled(ctx, object))
        {
            if !entered {
                crate::free::publish(ctx, Phase::Entered);
                entered = true;
            }
            park_retry(ctx, false);
        }
        crate::free::touch(ctx, object, "take");
    }
    let status = role::with_role(ctx, |t| {
        if t.sync.is_dead(word) {
            1
        } else {
            // SAFETY: caller contract.
            unsafe { t.sync.take(word, payload) };
            0
        }
    });
    // SAFETY: caller contract.
    unsafe { out.write(status) };
    if free {
        crate::free::done(ctx);
        if !entered {
            // SAFETY: the context is live; the payload is in the caller's carrier.
            unsafe { yield_now(&mut *ctx) };
        }
    }
    STATUS_OK
}

/// `$intrinsic.MutexPut[T](m, mut payload)`: the body returned; its payload image goes back
/// from the `Some` slot, which is left `None`, and the operation yields.
///
/// # Safety
/// `ctx` is live, `m` borrows a counted Mutex wrapper and `payload` a `Some` carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_put(
    ctx: *mut TaskCtx,
    m: *const u64,
    payload: *mut u8,
) -> u8 {
    if let Some(status) =
        unsafe { free::tail::invoke_mutex(ctx, free::tail::Kind::Put, mutex_word(m), payload) }
    {
        return status;
    }
    // SAFETY: caller contract; only a registered successor's wake needs R.
    if let Some(result) = unsafe { free::objects::mutex_put(ctx, mutex_word(m), payload) } {
        return unsafe { free::objects::finish(ctx, result) };
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "mutex_put");
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::engine::mutex_put(ctx, mutex_word(m), payload)) };
        return STATUS_OK;
    }
    if free::mutex::live(ctx) {
        unsafe { crate::stop::check(ctx, free::mutex::put(ctx, mutex_word(m), payload)) };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    unsafe {
        let word = mutex_word(m);
        mutex_release_op(ctx, word, "put");
        role::with_role(ctx, |t| t.sync.put(word, payload));
        crate::free::done(ctx);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// Free mode: a mutex's put or kill after its body is an op on the mutex, enabled at once
/// (its take held it); then the caller acts and publishes (k, done).
fn mutex_release_op(ctx: *mut TaskCtx, word: u64, outcome: &str) {
    if !crate::free::enter(ctx, Phase::Done) {
        return;
    }
    let object = (crate::free::MUTEX, word);
    if !crate::free::enabled(ctx, object) {
        fault("free-mode replay diverged: a mutex put before its version");
    }
    crate::free::touch(ctx, object, outcome);
}

/// The run ended after compiled cleanup released every mutex handle.
pub(crate) fn end_run(ctx: &mut TaskCtx) {
    let wide = crate::pool::wide(ctx.run_ref());
    // The run's code ran on one thread last, so its other threads are idle: they leave.
    crate::pool::shutdown(ctx.run_ref());
    if let Some(metrics) = ctx.run_ref().worker_metrics.clone() {
        metrics.stack_rss(stack_rss(ctx));
    }
    if wide {
        // At width > 1 the root, switched out on a compute worker or the home thread, goes
        // back to the home worker, whose heap the exit reads first.
        ctx.home_again();
    }
    crate::pool::retire(ctx.run());
    // Rows the root's own role-free closes left retire before the tables end.
    // SAFETY: the run's code ran on this thread last; it owns the tables.
    unsafe { free::objects::retire(ctx) };
    if let Some(s) = ctx.run().tasks.as_deref_mut() {
        s.sync.end();
    }
}

/// # Safety
/// `m` borrows a live counted mutex wrapper.
unsafe fn mutex_word(m: *const u64) -> u64 {
    unsafe { (m.read() as *const Header).add(1).cast::<u64>().read() }
}

/// # Safety
/// `word` is owned by the last wrapper and `ctx` is live.
pub(crate) unsafe fn close_mutex(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    if ordered::engine::on(ctx) {
        return unsafe {
            crate::stop::check(
                ctx,
                ordered::drops::mutex(ctx, word, anchor, operation_site),
            )
        };
    }
    // SAFETY: caller contract; a resolved mutex's close needs no role.
    if let Some(result) = unsafe { free::objects::mutex_close(ctx, word, anchor, operation_site) } {
        return unsafe { free::objects::closed(ctx, result) };
    }
    if free::enabled(ctx) {
        return unsafe {
            crate::stop::check(ctx, free::mutex::close(ctx, word, anchor, operation_site))
        };
    }
    if crate::free::close(ctx, (crate::free::MUTEX, word), "close mutex") == Some(false) {
        return STATUS_OK;
    }
    let Some((image, descriptor)) = role::with_role(ctx, |t| t.sync.close(word)) else {
        return STATUS_OK;
    };
    let mut continuation = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
    unsafe {
        crate::finalize::hirsel_rt_v2_finalize(
            ctx,
            descriptor,
            image.at(0),
            std::ptr::null(),
            continuation.as_mut_ptr(),
            anchor,
            operation_site,
        )
    }
}

/// The touched bytes of the run's task stacks, for the scheduling rows' counters: every
/// stack a row still holds and every idle stack of the run's workers.
fn stack_rss(ctx: &mut TaskCtx) -> u64 {
    let mut bytes = 0;
    if let Some(s) = ctx.run_ref().tasks.as_deref() {
        bytes += s
            .tasks
            .iter()
            .flatten()
            .filter_map(|row| row.stack.as_ref())
            .map(Stack::resident_bytes)
            .sum::<u64>();
    }
    for worker in crate::pool::workers(ctx.run_ref()) {
        // SAFETY: the run's threads are idle, so their workers' stacks are not in use.
        bytes += unsafe { &*worker }
            .idle_stacks
            .iter()
            .map(Stack::resident_bytes)
            .sum::<u64>();
    }
    bytes
}

/// `$intrinsic.MutexKill[T](m)` (FAIL-4): the body failed; the mutex keeps the failure chain,
/// and the operation yields.
///
/// # Safety
/// `ctx` is live, its failure active, and `m` borrows a counted Mutex wrapper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_kill(ctx: *mut TaskCtx, m: *const u64) -> u8 {
    if let Some(status) = unsafe {
        free::tail::invoke_mutex(
            ctx,
            free::tail::Kind::Kill,
            mutex_word(m),
            std::ptr::null_mut(),
        )
    } {
        return status;
    }
    // SAFETY: caller contract; a resolved held mutex dies without the role.
    if let Some(result) = unsafe { free::objects::mutex_kill(ctx, mutex_word(m)) } {
        return unsafe { free::objects::finish(ctx, result) };
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "mutex_kill");
    if ordered::engine::on(ctx) {
        return unsafe { crate::stop::check(ctx, ordered::drops::kill(ctx, mutex_word(m))) };
    }
    if free::mutex::live(ctx) {
        unsafe { crate::stop::check(ctx, free::mutex::kill(ctx, mutex_word(m))) };
        return STATUS_OK;
    }
    let path = current_path(ctx);
    // SAFETY: the context is live.
    let chain = unsafe { (*ctx).failure.chain(&path) };
    // SAFETY: caller contract.
    unsafe {
        let word = mutex_word(m);
        mutex_release_op(ctx, word, "kill");
        role::with_role(ctx, |t| t.sync.kill(word, chain));
        crate::free::done(ctx);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `$intrinsic.MutexCause[T](m)` (FAIL-4): continue the running task's failure, a dead mutex's
/// acquisition, with the chain that killed it.
///
/// # Safety
/// `ctx` is live and `m` borrows a dead Mutex's wrapper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_cause(ctx: *mut TaskCtx, m: *const u64) -> u8 {
    if let Some(status) = unsafe {
        free::tail::invoke_mutex(
            ctx,
            free::tail::Kind::Cause,
            mutex_word(m),
            std::ptr::null_mut(),
        )
    } {
        return status;
    }
    // SAFETY: caller contract; a resolved dead mutex's chain is cell state.
    if let Some(result) = unsafe { free::objects::mutex_cause(ctx, mutex_word(m)) } {
        return unsafe { free::objects::finish(ctx, result) };
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "mutex_cause");
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::engine::mutex_cause(ctx, mutex_word(m))) };
        return STATUS_OK;
    }
    if free::mutex::live(ctx) {
        unsafe { crate::stop::check(ctx, free::mutex::cause(ctx, mutex_word(m))) };
        return STATUS_OK;
    }
    // SAFETY: caller contract.
    let word = unsafe { mutex_word(m) };
    let chain = role::with_role(ctx, |t| t.sync.dead(word))
        .unwrap_or_else(|| fault("the cause of a mutex that is not dead"));
    // SAFETY: the context is live.
    unsafe { (*ctx).failure.append_chain(chain) };
    STATUS_OK
}

/// `$intrinsic.AtomicCreate[A](value, size)` (CONC-4): an atomic holding the `Some` image.
///
/// # Safety
/// `ctx` is live, `out` writable and `value` readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_atomic_create(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    value: *const u8,
    size: u64,
    payload_offset: u64,
    payload_size: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "atomic_create");
    // SAFETY: caller contract.
    if ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                ordered::engine::atomic_create(ctx, out, value, size, payload_offset, payload_size),
            )
        };
        return STATUS_OK;
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                free::record::atomic_create(ctx, out, value, size, payload_offset, payload_size),
            );
            yield_now(&mut *ctx);
        }
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    unsafe {
        out.write(role::with_role(ctx, |t| {
            t.sync
                .create_image(value, size, payload_offset, payload_size)
        }));
        crate::free::done(ctx);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `$intrinsic.AtomicLoad[A](a, mut slot, size)`: copy the atomic's image into the `None` slot.
///
/// # Safety
/// `ctx` is live, `a` an Atomic word and `slot` writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_atomic_load(
    ctx: *mut TaskCtx,
    a: *const u64,
    slot: *mut u8,
    size: u64,
) -> u8 {
    // SAFETY: caller contract; a resolved load needs no role.
    if let Some(result) = unsafe { free::objects::atomic_load(ctx, a.read(), slot, size) } {
        return unsafe { free::objects::finish(ctx, result) };
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "atomic_load");
    // SAFETY: caller contract.
    if ordered::engine::on(ctx) {
        unsafe { crate::stop::check(ctx, ordered::engine::atomic_load(ctx, a.read(), slot, size)) };
        return STATUS_OK;
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(ctx, free::record::atomic_load(ctx, a.read(), slot, size));
            yield_now(&mut *ctx);
        }
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    unsafe {
        role::with_role(ctx, |t| t.sync.read(a.read(), slot, size));
        if crate::free::on(ctx) {
            // The load's image is recorded live and substituted in a replay.
            let image = role::with_role(ctx, |t| t.sync.image(a.read(), size));
            let carrier = std::slice::from_raw_parts_mut(slot, image.size());
            let live = image
                .bits(carrier)
                .unwrap_or_else(|| fault("a free-mode atomic scalar wider than 16 bytes"));
            let bits = crate::free::load(ctx, live);
            if !image.substitute(carrier, bits) {
                fault("a recorded atomic load does not fit its scalar");
            }
        }
        crate::free::done(ctx);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

/// `$intrinsic.AtomicStore[A](a, value, size)`: replace the atomic's image.
///
/// # Safety
/// `ctx` is live, `a` an Atomic word and `value` readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_atomic_store(
    ctx: *mut TaskCtx,
    a: *const u64,
    value: *const u8,
    size: u64,
) -> u8 {
    // SAFETY: caller contract; a resolved store needs no role.
    if let Some(result) = unsafe { free::objects::atomic_store(ctx, a.read(), value, size) } {
        return unsafe { free::objects::finish(ctx, result) };
    }
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    #[cfg(feature = "measurement")]
    let _segment = profile::Operation::new(ctx, "atomic_store");
    // SAFETY: caller contract.
    if ordered::engine::on(ctx) {
        unsafe {
            crate::stop::check(
                ctx,
                ordered::engine::atomic_store(ctx, a.read(), value, size),
            )
        };
        return STATUS_OK;
    }
    if free::enabled(ctx) {
        unsafe {
            crate::stop::check(ctx, free::record::atomic_store(ctx, a.read(), value, size));
            yield_now(&mut *ctx);
        }
        return STATUS_OK;
    }
    crate::free::enter(ctx, Phase::Done);
    unsafe {
        role::with_role(ctx, |t| t.sync.write(a.read(), value, size));
        crate::free::done(ctx);
        yield_now(&mut *ctx);
    }
    STATUS_OK
}

#[cfg(test)]
mod free_policy_tests {
    use super::*;

    #[test]
    fn cancellation_cache_tracks_scope_precedence_and_skips_repeated_ancestor_walks() {
        let metrics = std::sync::Arc::new(crate::pool::WorkerMetrics::default());
        let mut s = Scheduler::default();
        s.cancel_metrics = Some(metrics.clone());
        s.tasks
            .push(Some(Task::new(1, std::ptr::null_mut(), None, vec![], None)));
        s.scopes = vec![Some(scope()), Some(scope())];
        s.task(ROOT).scopes = vec![0, 1];
        assert_eq!(s.canceled_task(ROOT, 0), None);
        assert_eq!(s.canceled_task(ROOT, 0), None);
        let values = metrics
            .values()
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(values["ancestor_depth"], 1.0);
        s.cancel_scope(0, CANCEL_CHILD_FAILED);
        assert_eq!(s.canceled_task(ROOT, 0), Some(CANCEL_CHILD_FAILED));
        s.cancel_scope(1, CANCEL_DEADLINE);
        assert_eq!(s.canceled_task(ROOT, 0), Some(CANCEL_DEADLINE));
        // Shielding neither observes nor overwrites the cached cause.
        assert_eq!(s.canceled_task(ROOT, 1), None);
        assert_eq!(s.canceled_task(ROOT, 0), Some(CANCEL_DEADLINE));
        s.task(ROOT).scopes.pop();
        s.cancel_epoch.changed();
        assert_eq!(s.canceled_task(ROOT, 0), Some(CANCEL_CHILD_FAILED));
        s.cancel(ROOT, CANCEL_REQUESTED);
        assert_eq!(s.canceled_task(ROOT, 0), Some(CANCEL_REQUESTED));
        s.cancel(ROOT, CANCEL_DEADLINE);
        assert_eq!(s.canceled_task(ROOT, 0), Some(CANCEL_REQUESTED));
    }

    #[test]
    fn cancellation_walks_only_linked_descendants_and_unlinks_before_row_reuse() {
        let mut s = Scheduler::default();
        s.scopes.push(Some(scope()));
        for task in 0..5 {
            s.tasks.push(Some(Task::new(
                1,
                std::ptr::null_mut(),
                (task != ROOT).then_some(0),
                vec![task],
                None,
            )));
        }
        s.link_child(ROOT, 1);
        s.link_child(ROOT, 4);
        s.link_child(1, 2);
        s.link_child(1, 3);
        s.cancel(1, CANCEL_DEADLINE);
        assert_eq!(s.task(ROOT).canceled.cause(), None);
        assert_eq!(s.task(1).canceled.cause(), Some(CANCEL_DEADLINE));
        assert_eq!(s.task(2).canceled.cause(), Some(CANCEL_PARENT));
        assert_eq!(s.task(3).canceled.cause(), Some(CANCEL_PARENT));
        assert_eq!(s.task(4).canceled.cause(), None);
        // Scope 0 belongs to root, but task 2's actual parent is task 1.
        s.vacate_task(2);
        s.tasks[2] = Some(Task::new(2, std::ptr::null_mut(), Some(0), vec![9], None));
        s.link_child(ROOT, 2);
        s.cancel(1, CANCEL_REQUESTED);
        assert_eq!(s.task(2).canceled.cause(), None);
        s.cancel(ROOT, CANCEL_REQUESTED);
        assert_eq!(s.task(2).canceled.cause(), Some(CANCEL_PARENT));
        assert_eq!(s.task(4).canceled.cause(), Some(CANCEL_PARENT));
    }

    #[test]
    fn retiring_a_spawner_promotes_descendants_before_its_row_is_reused() {
        let mut s = Scheduler::default();
        s.scopes.push(Some(scope()));
        for task in 0..5 {
            s.tasks.push(Some(Task::new(
                1,
                std::ptr::null_mut(),
                (task != ROOT).then_some(0),
                vec![task],
                None,
            )));
        }
        s.link_child(ROOT, 1);
        s.link_child(ROOT, 4);
        s.link_child(1, 2);
        s.link_child(2, 3);
        s.vacate_task(1);
        assert_eq!(s.task(ROOT).children, [4, 2]);
        assert_eq!(s.task(4).parent, Some((ROOT, 0)));
        assert_eq!(s.task(2).parent, Some((ROOT, 1)));
        s.tasks[1] = Some(Task::new(2, std::ptr::null_mut(), Some(0), vec![9], None));
        s.link_child(ROOT, 1);
        s.cancel(1, CANCEL_REQUESTED);
        assert_eq!(s.task(2).canceled.cause(), None);
        assert_eq!(s.task(3).canceled.cause(), None);
        s.cancel(ROOT, CANCEL_DEADLINE);
        assert_eq!(s.task(1).canceled.cause(), Some(CANCEL_REQUESTED));
        for child in [2, 3, 4] {
            assert_eq!(s.task(child).canceled.cause(), Some(CANCEL_PARENT));
        }
        // Promotion can repeat, and must repair the swapped child's position.
        s.vacate_task(2);
        assert_eq!(s.task(ROOT).children, [4, 1, 3]);
        assert_eq!(s.task(1).parent, Some((ROOT, 1)));
        assert_eq!(s.task(3).parent, Some((ROOT, 2)));
    }

    #[test]
    fn a_logically_parked_running_frame_cannot_be_polled_before_idle_save() {
        let mut run = crate::host::BodyRun::new(2, false);
        // SAFETY: the body uses the root's existing physical-dispatch end and
        // restores its logical state; it never schedules a second task.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |_| {},
            |ctx| {
                unsafe {
                    role::enter(ctx);
                    let current = (*ctx).task_id;
                    assert!(!crate::pool::free::is_saved((*ctx).run_ptr(), current));
                    let row = scheduler(ctx).task(current);
                    row.state = State::Parked;
                    // No live output/channel is needed: a poll is forbidden
                    // from reaching either while the frame remains running.
                    row.wait = Some(Wait::Receive {
                        channel: u32::MAX,
                        slot: std::ptr::dangling_mut(),
                    });
                    assert!(!poll(ctx, current, &crate::io::Ready::Tokens(Vec::new())).unwrap());
                    let row = scheduler(ctx).task(current);
                    row.state = State::Ready;
                    row.wait = None;
                    // An already-saved awaiter also cannot read the result
                    // of a Holding child whose physical stack is still live.
                    row.state = State::Holding;
                    row.slot = Some(Slot {
                        slot: std::ptr::dangling_mut(),
                        none: std::ptr::dangling(),
                        size: 1,
                    });
                    let s = scheduler(ctx);
                    let mut waiter = Task::new(1, std::ptr::null_mut(), None, vec![0], None);
                    waiter.state = State::Parked;
                    waiter.wait = Some(Wait::Join {
                        child: current,
                        slot: std::ptr::dangling_mut(),
                        size: 1,
                    });
                    let waiter_id = s.tasks.len() as u32;
                    s.tasks.push(Some(waiter));
                    assert!(!poll(ctx, waiter_id, &crate::io::Ready::Tokens(Vec::new())).unwrap());
                    let s = scheduler(ctx);
                    s.tasks.pop();
                    s.task(current).state = State::Ready;
                    s.task(current).slot = None;
                    role::release(ctx);
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    struct ConcurrentSegments {
        started: std::sync::atomic::AtomicU32,
        finished: std::sync::atomic::AtomicU32,
        timed_out: std::sync::atomic::AtomicBool,
    }

    fn rendezvous(segments: &ConcurrentSegments) {
        segments.started.fetch_add(1, Ordering::AcqRel);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while segments.started.load(Ordering::Acquire) != 8 {
            if std::time::Instant::now() >= until {
                segments.timed_out.store(true, Ordering::Release);
                break;
            }
            std::thread::yield_now();
        }
    }

    unsafe extern "C" fn concurrent_segment(ctx: *mut u8, data: *mut u8) -> ! {
        let ctx = ctx.cast::<TaskCtx>();
        // SAFETY: admission transfers this Box to this prepared stack once.
        let segments = unsafe { Box::from_raw(data.cast::<std::sync::Arc<ConcurrentSegments>>()) };
        rendezvous(&segments);
        segments.finished.fetch_add(1, Ordering::Release);
        drop(segments);
        // SAFETY: no task-owned Rust value remains on this abandoned stack.
        unsafe {
            role::enter(ctx);
            crate::pool::free::finish_current(ctx);
        }
    }

    #[test]
    fn newly_published_segments_run_while_the_spawner_keeps_its_worker() {
        // Process permits are shared by all runs in one test binary. Follow
        // runners::permit_tests: isolate the manager grant from other tests.
        const CHILD: &str = "HIRSEL_F1_CONCURRENT_SEGMENTS";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tasks::free_policy_tests::newly_published_segments_run_while_the_spawner_keeps_its_worker",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }
        crate::pool::set_compute_grant(8);
        for _ in 0..32 {
            concurrent_segments_run();
        }
    }

    fn concurrent_segments_run() {
        let mut run = crate::host::BodyRun::new(8, false);
        // SAFETY: these bodies use only explicit physical dispatch, never a
        // home service, logical wait, or another task's stack slots.
        unsafe { run.use_physical_dispatch() };
        let segments = std::sync::Arc::new(ConcurrentSegments {
            started: Default::default(),
            finished: Default::default(),
            timed_out: Default::default(),
        });
        let observed = segments.clone();
        let outcome = run.run(
            |_| {},
            move |ctx| {
                // Build seven independent, real task stacks under the role.
                // The root is the eighth segment and deliberately keeps its
                // worker until all seven have started; hidden local work must
                // not depend on its next operation or suspension.
                unsafe {
                    role::enter(ctx);
                    for _ in 0..7 {
                        let stack = Stack::new();
                        let child = stack.context();
                        child.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                        let s = scheduler(ctx);
                        let index = s.tasks.len() as u32;
                        (*child).task_id = index;
                        (*child).task_sp = stack.prepare(
                            concurrent_segment,
                            child.cast(),
                            Box::into_raw(Box::new(segments.clone())).cast(),
                        );
                        let mut row = Task::new(1, child, None, vec![index], None);
                        row.stack = Some(stack);
                        s.tasks.push(Some(row));
                        crate::pool::free::schedule_saved(
                            (*ctx).run_ptr(),
                            crate::tasks::scheduler(ctx).key(index),
                            Owner::Any,
                        );
                    }
                    role::release(ctx);
                }
                rendezvous(&segments);
                crate::STATUS_OK
            },
        );
        assert!(matches!(outcome.ended, Ok(crate::STATUS_OK)), "{outcome:?}");
        assert_eq!(outcome.pending, 0);
        assert_eq!(observed.started.load(Ordering::Acquire), 8);
        assert_eq!(observed.finished.load(Ordering::Acquire), 7);
        assert!(!observed.timed_out.load(Ordering::Acquire));
    }

    fn scope() -> Scope {
        Scope {
            generation: 1,
            owner: ROOT,
            children: Vec::new(),
            canceled: Default::default(),
            deadline: None,
            completions: AtomicU64::new(0),
        }
    }

    #[test]
    fn completion_ranks_are_per_scope_even_before_set_creation_or_insertion() {
        let mut s = Scheduler::default();
        s.scopes = vec![Some(scope()), Some(scope())];
        for parent in [0, 1, 1, 0] {
            let mut task = Task::new(1, std::ptr::null_mut(), Some(parent), vec![], None);
            task.state = State::Holding;
            s.tasks.push(Some(task));
        }
        for id in 0..4 {
            s.order(id);
        }
        assert_eq!(s.task(0).completed, 0);
        assert_eq!(s.task(1).completed, 0);
        assert_eq!(s.task(2).completed, 1);
        assert_eq!(s.task(3).completed, 1);
        // Create a set after completion, and insert in the opposite order.
        s.sets.push(Some(TaskSet {
            generation: 1,
            scope: 0,
            members: vec![Some(handle(3, 1)), Some(handle(0, 1))],
        }));
        assert_eq!(s.first_completed(0), Some(1));
        s.set(0).members[1] = None;
        assert_eq!(s.first_completed(0), Some(0));
    }
}

#[cfg(test)]
#[path = "tasks/runtime50/modules.rs"]
pub(crate) mod runtime50;
