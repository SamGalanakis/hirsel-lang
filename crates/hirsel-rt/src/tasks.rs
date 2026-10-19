//! Structured tasks on one thread (CONC-1, CONC-2, CONC-3, FAIL-2).
//!
//! The scheduler is cooperative and single-threaded: every task is a [`fiber`](crate::fiber)
//! of the run's one context, and a switch happens only at a capability operation, a join or a
//! completion. All tasks share the run's heap: no second thread exists, and the checker's
//! sendability rules (MODE-4, OWN-6) already keep every counted value owned by one task, so a
//! value crossing tasks is moved by copying its root. A threaded scheduler must reintroduce
//! per-thread heaps and a transfer of the moved graphs. What is a task's own is swapped with it
//! ([`Saved`]): its failure chain, finalizer shield, release worklist, innermost region and
//! measurement site.
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

use std::collections::{BTreeMap, VecDeque};
use std::ptr::NonNull;

use hirsel_kernels::schedule::{Algorithm, Schedule};

use crate::context::RunCtx;
use crate::failure::{Chained, FailureState};
use crate::fiber::{self, Stack};
use crate::region::Region;
use crate::stop::{Stop, Stopped};
use crate::{Header, STATUS_FAILURE, STATUS_OK, fault};

/// A spawned task's entry: compiled closure code taking its environment.
pub type TaskCode = unsafe extern "C" fn(*mut RunCtx, *mut Header, u64, u64) -> u8;

/// `CancelCause` ordinals (core library).
pub(crate) const CANCEL_REQUESTED: u32 = 0;
pub(crate) const CANCEL_PARENT: u32 = 1;
pub(crate) const CANCEL_CHILD_FAILED: u32 = 2;
pub(crate) const CANCEL_DEADLINE: u32 = 3;

/// The root task: the entry, on the thread's own stack.
const ROOT: u32 = 0;

/// What a suspended task keeps of the context while another runs.
#[derive(Default)]
struct Saved {
    failure: FailureState,
    shield: u32,
    work: Option<std::ptr::NonNull<Header>>,
    region: Option<std::ptr::NonNull<Region>>,
    #[cfg(feature = "measurement")]
    site: u64,
}

impl Saved {
    fn take(ctx: &mut RunCtx) -> Saved {
        Saved {
            failure: std::mem::take(&mut ctx.failure),
            shield: std::mem::take(&mut ctx.shield),
            work: std::ptr::NonNull::new(std::mem::replace(&mut ctx.work, std::ptr::null_mut())),
            region: std::ptr::NonNull::new(std::mem::replace(
                &mut ctx.regions.active,
                std::ptr::null_mut(),
            )),
            #[cfg(feature = "measurement")]
            site: std::mem::take(&mut ctx.heap.measurement.active_site),
        }
    }
    fn restore(self, ctx: &mut RunCtx) {
        ctx.failure = self.failure;
        ctx.shield = self.shield;
        ctx.work = self.work.map_or(std::ptr::null_mut(), |p| p.as_ptr());
        ctx.regions.active = self.region.map_or(std::ptr::null_mut(), |p| p.as_ptr());
        #[cfg(feature = "measurement")]
        {
            ctx.heap.measurement.active_site = self.site;
        }
    }
}

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
    Receive {
        channel: u32,
        slot: *mut *mut Header,
    },
    /// A host request that would block, parked under its readiness token
    /// (`crate::host_waits`); its poll re-issues the request.
    Host { token: u64 },
    /// `task.await_any`, until a member of task set `set` holds its result or failed.
    Any { set: u32 },
    /// `task.cancel_all`, until every member of task set `set` completed.
    Drain { set: u32 },
}

/// A completed child's result slot, inside its parked frame.
#[derive(Clone, Copy)]
struct Slot {
    slot: *mut u8,
    none: *const u8,
    size: usize,
}

struct Task {
    generation: u32,
    sp: *mut u8,
    stack: Option<Stack>,
    state: State,
    /// The scope it was spawned into (the root has none).
    scope: Option<u32>,
    path: Vec<u32>,
    spawned: u32,
    canceled: Option<u32>,
    /// Scopes this task has entered and not yet exited, innermost last.
    scopes: Vec<u32>,
    saved: Saved,
    code: Option<(TaskCode, *mut Header, u64)>,
    slot: Option<Slot>,
    /// What the task waits for while parked.
    wait: Option<Wait>,
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
    /// Its scheduling priority under `pct@1`, drawn when it was created.
    priority: u64,
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
    canceled: Option<u32>,
    /// `task.scope_until`'s deadline (CONC-3).
    deadline: Option<ScopeDeadline>,
}

/// A scope's deadline: its armed timer's host token until it fires or the scope exits.
struct ScopeDeadline {
    token: Option<u64>,
    expired: bool,
}

/// Every task and scope of one run, and the runnable queue.
#[derive(Default)]
pub(crate) struct Scheduler {
    tasks: Vec<Option<Task>>,
    vacant_tasks: Vec<u32>,
    scopes: Vec<Option<Scope>>,
    vacant_scopes: Vec<u32>,
    generation: u32,
    current: u32,
    runnable: VecDeque<u32>,
    /// Parked tasks by their place in park order.
    parked: BTreeMap<u64, u32>,
    parks: u64,
    /// Parked tasks to poll at the next scheduling point.
    woken: Vec<u32>,
    /// Tasks parked in `task.await_any` or `task.cancel_all`.
    set_waiters: Vec<u32>,
    pub(crate) channels: crate::channels::Channels,
    sync: crate::sync::Synchronizers,
    /// The chain the last `task.isolate` caught, which its body reads (`isolate.rs`).
    pub(crate) isolated: Vec<crate::failure::Chained>,
    idle_stacks: Vec<Stack>,
    /// The stack of a task that completed; the next task to run recycles it.
    retired: Option<Stack>,
    sets: Vec<Option<TaskSet>>,
    vacant_sets: Vec<u32>,
    /// Children that held their result or failed so far: the next one's completion order.
    completions: u64,
    /// The run's scheduler policy (`inputs.scheduler`).
    schedule: Schedule,
}

fn handle(index: u32, generation: u32) -> u64 {
    u64::from(generation) << 32 | u64::from(index)
}

impl Scheduler {
    fn new(mut schedule: Schedule) -> Scheduler {
        let mut root = Task::new(1, None, Vec::new(), None);
        root.priority = schedule.admit();
        Scheduler {
            tasks: vec![Some(root)],
            generation: 1,
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
    fn fresh_generation(&mut self) -> u32 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
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
        canceled_in(&self.tasks, &self.scopes, task, shield)
    }
    /// Cancel `task` with `cause` and its descendants with `Parent`; an earlier cause stays.
    /// Each canceled parked task is woken: its cancellation may end its wait.
    fn cancel(&mut self, task: u32, cause: u32) {
        let path = self.task(task).path.clone();
        let mut parked = Vec::new();
        for (index, row) in self.tasks.iter_mut().enumerate() {
            let Some(row) = row else { continue };
            if row.path.starts_with(&path) && row.scope.is_some() {
                row.canceled.get_or_insert(if row.path.len() == path.len() {
                    cause
                } else {
                    CANCEL_PARENT
                });
                parked.push(index as u32);
            }
        }
        for task in parked {
            self.wake(task);
        }
    }
    /// Poll parked `task` at the next scheduling point: an event may have ended its wait.
    fn wake(&mut self, task: u32) {
        if let Some(Some(row)) = self.tasks.get_mut(task as usize)
            && row.state == State::Parked
            && row.wait.is_some()
            && !row.woken
        {
            row.woken = true;
            self.woken.push(task);
        }
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
            if let Some(Wait::Any { set } | Wait::Drain { set }) = self.task(waiter).wait
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
        for channel in self.channels.take_touched() {
            let Some((queued, closed, waiting)) = self.channels.waiting(channel) else {
                continue;
            };
            let mut wake = Vec::new();
            let mut served = 0;
            for &task in waiting.values() {
                if !closed && served == queued {
                    break;
                }
                let shield = self.tasks[task as usize]
                    .as_ref()
                    .map_or(0, |t| t.saved.shield);
                if canceled_in(&self.tasks, &self.scopes, task, shield).is_none() {
                    served += 1;
                }
                wake.push(task);
            }
            for task in wake {
                self.wake(task);
            }
        }
    }
    /// The running task parks on `wait`: it is polled at the next scheduling point, and
    /// registered where the events that can end its wait find it.
    fn park(&mut self, wait: Wait) {
        let current = self.current;
        let at = self.parks;
        self.parks += 1;
        let row = self.task(current);
        row.state = State::Parked;
        row.wait = Some(wait);
        row.parked_at = at;
        row.woken = true;
        self.parked.insert(at, current);
        self.woken.push(current);
        match wait {
            Wait::Join { child, .. } => self.task(child).awaited_by = Some(current),
            Wait::Receive { channel, .. } => self.channels.wait(channel, at, current),
            Wait::Any { .. } | Wait::Drain { .. } => self.set_waiters.push(current),
            Wait::Exit { .. } | Wait::Host { .. } => {}
        }
    }
    /// A poll ended parked `task`'s wait: it leaves every registration and joins the queue.
    fn unpark(&mut self, task: u32) {
        let row = self.task(task);
        let (at, wait) = (row.parked_at, row.wait.take());
        row.state = State::Ready;
        self.parked.remove(&at);
        match wait {
            Some(Wait::Join { child, .. }) => {
                if let Some(Some(child)) = self.tasks.get_mut(child as usize) {
                    child.awaited_by = None;
                }
            }
            Some(Wait::Receive { channel, .. }) => self.channels.unwait(channel, at),
            Some(Wait::Any { .. } | Wait::Drain { .. }) => self.set_waiters.retain(|&t| t != task),
            _ => {}
        }
        self.runnable.push_back(task);
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
        row.canceled.get_or_insert(CANCEL_DEADLINE);
        let (owner, children) = (row.owner, row.children.clone());
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
        children
            .iter()
            .all(|&c| matches!(self.task(c).state, State::Holding | State::Done))
    }
    /// Vacate task set row `set`; its members stay children of their scope.
    fn close_set(&mut self, set: u32) {
        self.sets[set as usize] = None;
        self.vacant_sets.push(set);
    }
    /// `task` holds its result or failed: its place in completion order.
    fn order(&mut self, task: u32) {
        let order = self.completions;
        self.completions += 1;
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
    fn vacate_task(&mut self, index: u32) {
        let task = self.tasks[index as usize]
            .take()
            .unwrap_or_else(|| fault("a task completed twice"));
        if let Some(stack) = task.stack {
            self.idle_stacks.push(stack);
        }
        self.vacant_tasks.push(index);
    }
}

impl Task {
    fn new(generation: u32, scope: Option<u32>, path: Vec<u32>, canceled: Option<u32>) -> Task {
        Task {
            generation,
            sp: std::ptr::null_mut(),
            stack: None,
            state: State::Ready,
            scope,
            path,
            spawned: 0,
            canceled,
            scopes: Vec::new(),
            saved: Saved::default(),
            code: None,
            slot: None,
            wait: None,
            status: 0,
            handoff: None,
            detached: false,
            failure: None,
            completed: 0,
            parked_at: 0,
            woken: false,
            awaited_by: None,
            priority: 0,
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
) -> Option<u32> {
    if shield > 0 {
        return None;
    }
    let task = tasks[task as usize]
        .as_ref()
        .unwrap_or_else(|| fault("a vacant task row"));
    if let Some(cause) = task.canceled {
        return Some(cause);
    }
    task.scopes
        .iter()
        .rev()
        .find_map(|&s| scopes[s as usize].as_ref().and_then(|s| s.canceled))
}

/// The run's scheduler, created with the root task at the first scope.
pub(crate) fn scheduler(ctx: *mut RunCtx) -> &'static mut Scheduler {
    // SAFETY: the context is live for the whole run, and the scheduler is only reached
    // through it, by the one running task; no reference is kept across a switch.
    unsafe {
        (*ctx).tasks.get_or_insert_with(|| {
            let schedule = std::mem::take(&mut (*ctx).schedule);
            Box::new(Scheduler::new(schedule))
        })
    }
}

/// Outstanding waits for the test runner. This is host reporting, outside program Io.
pub(crate) fn test_waits(ctx: *mut RunCtx) -> String {
    let s = scheduler(ctx);
    let mut waits: Vec<_> = s
        .parked
        .values()
        .map(|&task| {
            let row = s.tasks[task as usize].as_ref().unwrap();
            let kind = match row.wait {
                Some(Wait::Join { .. }) => "task.await",
                Some(Wait::Exit { .. }) => "task.scope",
                Some(Wait::Receive { .. }) => "channel.receive",
                Some(Wait::Host { token }) => unsafe { crate::host_waits::kind(ctx, token) },
                Some(Wait::Any { .. }) => "task.await_any",
                Some(Wait::Drain { .. }) => "task.cancel_all",
                None => "task completion",
            };
            (row.path.clone(), kind)
        })
        .collect();
    waits.sort_by(|a, b| a.0.cmp(&b.0));
    waits
        .into_iter()
        .map(|(path, kind)| format!("\n  task {path:?}: {kind}"))
        .collect()
}

/// The running task's spawn path.
pub(crate) fn current_path(ctx: *mut RunCtx) -> Vec<u32> {
    let s = scheduler(ctx);
    let current = s.current;
    s.task(current).path.clone()
}

/// Suspend the running task and run `next`. The running task must already be queued, parked
/// or holding; it returns from here when something resumes it.
///
/// # Safety
/// `ctx` is live and `next` is a suspended or prepared task.
unsafe fn switch_to(ctx: *mut RunCtx, next: u32) {
    let s = scheduler(ctx);
    let from = s.current;
    if from == next {
        return;
    }
    // SAFETY: the context is live; neither borrow survives the switch.
    let (save, to) = unsafe {
        s.task(from).saved = Saved::take(&mut *ctx);
        std::mem::take(&mut s.task(next).saved).restore(&mut *ctx);
        s.current = next;
        let save = &raw mut s.task(from).sp;
        (save, s.task(next).sp)
    };
    // SAFETY: `to` is the suspended or prepared stack pointer of `next`.
    unsafe { fiber::switch(save, to) };
    recycle(ctx);
}

/// Leave a completed task for good and run `next`. The completed task's row may already be
/// vacant, so nothing of it is saved; its context state is empty.
///
/// # Safety
/// `ctx` is live, `next` is suspended and the running fiber is never resumed.
unsafe fn depart_to(ctx: *mut RunCtx, next: u32) -> ! {
    let s = scheduler(ctx);
    // SAFETY: the context is live.
    unsafe {
        let left = Saved::take(&mut *ctx);
        if left.failure.active() || left.shield != 0 || left.work.is_some() || left.region.is_some()
        {
            fault("a task completed with context state left");
        }
        // Nothing on this abandoned stack may own storage at the switch.
        drop(left);
        std::mem::take(&mut s.task(next).saved).restore(&mut *ctx);
    }
    s.current = next;
    s.task(next).state = State::Ready;
    let mut abandoned = std::ptr::null_mut();
    // SAFETY: `next` is suspended; nothing resumes the abandoned stack pointer.
    unsafe { fiber::switch(&raw mut abandoned, s.task(next).sp) };
    fault("a completed task was resumed")
}

/// A task resumed: the stack of the task that completed before it is free again.
fn recycle(ctx: *mut RunCtx) {
    let s = scheduler(ctx);
    if let Some(stack) = s.retired.take() {
        s.idle_stacks.push(stack);
    }
}

/// A scheduling point: poll the parked tasks whose wait may be over (those that just parked,
/// those an event woke, those whose host request the adapter reports ready), timers by
/// deadline (ties by park order), other waits by park order. Each whose wait is over has its
/// effect performed and joins the back of the queue.
///
/// # Safety
/// `ctx` is live.
unsafe fn poll_parked(ctx: *mut RunCtx) -> Stopped<()> {
    // SAFETY: the context is live.
    let ready = unsafe { crate::host_waits::ready(ctx) }?;
    // Scope deadlines first, so the parked tasks' polls see what they cancel (CONC-3).
    for scope in unsafe { crate::host_waits::expired(ctx, &ready) }? {
        scheduler(ctx).expire(scope);
    }
    for task in unsafe { crate::host_waits::woken(ctx, &ready) } {
        scheduler(ctx).wake(task);
    }
    let s = scheduler(ctx);
    s.wake_receivers();
    if s.woken.is_empty() {
        return Ok(());
    }
    let mut woken = std::mem::take(&mut s.woken);
    // Original request ticks preserve exact ties; readiness's host Instant is
    // only for sleeping and may have rounding or sampling skew.
    woken.sort_by_cached_key(|&task| {
        let row = scheduler(ctx).task(task);
        let deadline = match row.wait {
            // SAFETY: the context is live and `token` parked.
            Some(Wait::Host { token }) => unsafe { crate::host_waits::deadline(ctx, token) },
            _ => None,
        };
        (deadline, row.parked_at)
    });
    for task in woken {
        scheduler(ctx).task(task).woken = false;
        // SAFETY: the context is live and `task` is parked.
        if unsafe { poll(ctx, task, &ready) }? {
            scheduler(ctx).unpark(task);
        }
    }
    Ok(())
}

/// Whether parked `task`'s wait is over, after performing its effect if it is; `ready` is the
/// scheduling point's ready host requests.
///
/// # Safety
/// `ctx` is live and `task` is parked; a join's slot is in its frame.
unsafe fn poll(ctx: *mut RunCtx, task: u32, ready: &crate::io::Ready) -> Stopped<bool> {
    let s = scheduler(ctx);
    Ok(match s.task(task).wait {
        Some(Wait::Join { child, slot, size }) => match s.task(child).state {
            State::Holding => {
                let held = s.task(child).slot.take().unwrap();
                // SAFETY: the child is in its finish and the awaiter parked, both slots live.
                unsafe { deliver(held, slot, size) };
                true
            }
            State::Done => true,
            State::Ready | State::Parked => false,
        },
        Some(Wait::Exit { scope }) => s.settled(scope),
        Some(Wait::Receive { channel, slot }) => {
            let shield = s.task(task).saved.shield;
            // SAFETY: the slot is in the parked task's frame.
            match unsafe { receive_ready(s, task, shield, channel, slot) } {
                Some(status) => {
                    s.task(task).status = status;
                    true
                }
                None => false,
            }
        }
        Some(Wait::Host { token }) => {
            let shield = s.task(task).saved.shield;
            let cause = s.canceled_task(task, shield);
            // SAFETY: the context is live and `token` is this task's parked request.
            unsafe { crate::host_waits::poll(ctx, token, cause, ready.contains(token)) }?
        }
        Some(Wait::Any { set }) => s.first_completed(set).is_some(),
        // SAFETY: the parked task holds the set's row.
        Some(Wait::Drain { set }) => unsafe { poll_drained(s, set) },
        None => fault("a parked task without a wait"),
    })
}

/// The running task stopped (it parked, holds its result or completed): poll the parked tasks,
/// then run the front of the queue, which may be the running task itself.
///
/// # Safety
/// `ctx` is live.
unsafe fn run_next(ctx: *mut RunCtx) -> Stopped<()> {
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
unsafe fn pick(ctx: *mut RunCtx) -> Stopped<Option<u32>> {
    let s = scheduler(ctx);
    if s.schedule.algorithm() == Algorithm::Fifo || s.runnable.is_empty() {
        return Ok(s.runnable.pop_front());
    }
    if let Some(priority) = s.schedule.point()
        && let Some(Some(ran)) = s.tasks.get_mut(s.current as usize)
    {
        ran.priority = priority;
    }
    let priorities: Vec<u64> = s
        .runnable
        .iter()
        .map(|&task| s.tasks[task as usize].as_ref().map_or(0, |t| t.priority))
        .collect();
    let mut choice = s.schedule.choose(&priorities);
    // SAFETY: the context is live; a run without an Io binding records nothing.
    if s.schedule.recorded(priorities.len()) && !unsafe { (*ctx).io_data() }.is_null() {
        let binding = unsafe { crate::io::Binding::get(ctx) };
        let count = priorities.len() as u64;
        choice = match binding.adapter.schedule(count, choice as u64) {
            Ok(choice) if choice < count => choice as usize,
            Ok(_) => crate::fault("a scheduling input outside the runnable queue"),
            Err(error) => return Err(Stop::refusal(error)),
        };
    }
    Ok(scheduler(ctx).runnable.remove(choice))
}

/// Poll the parked tasks and take the next task to run ([`pick`]).
///
/// # Safety
/// `ctx` is live.
unsafe fn next_task(ctx: *mut RunCtx) -> Stopped<u32> {
    // SAFETY: the context is live.
    unsafe { poll_parked(ctx) }?;
    // With nothing runnable, sleep until a parked host request may complete, then poll again.
    while scheduler(ctx).runnable.is_empty() && unsafe { crate::host_waits::wait(ctx) }? {
        unsafe { poll_parked(ctx) }?;
    }
    // CONC-8: no task can run and nothing outside the run can wake one.
    unsafe { pick(ctx) }?.ok_or(Stop::Blocked)
}

/// The yield after a capability operation: the parked tasks are polled and, when another task
/// is runnable, the running task joins the back of the queue and the policy picks the task to
/// run, which may be the running task again.
///
/// # Safety
/// `ctx` is live and no borrow of its Io adapter or scheduler is held.
pub unsafe fn yield_now(ctx: *mut RunCtx) {
    // SAFETY: the context is live; the caller holds nothing a stop must drop.
    unsafe { crate::stop::check(ctx, try_yield(ctx)) }
}

/// [`yield_now`], returning a stop to its caller.
///
/// # Safety
/// As for [`yield_now`].
pub(crate) unsafe fn try_yield(ctx: *mut RunCtx) -> Stopped<()> {
    // SAFETY: the context is live.
    if unsafe { (*ctx).tasks.is_none() } {
        return Ok(());
    }
    // SAFETY: the context is live.
    unsafe { poll_parked(ctx) }?;
    let s = scheduler(ctx);
    if s.runnable.is_empty() {
        return Ok(());
    }
    s.runnable.push_back(s.current);
    // SAFETY: the context is live and the queue is nonempty.
    let next = unsafe { pick(ctx) }?.unwrap_or_else(|| crate::fault("an empty runnable queue"));
    unsafe { switch_to(ctx, next) };
    Ok(())
}

/// Park the running task until `wait` is over; returns once a poll performed its effect.
/// `yielded`: the operation yields first, and the task parks when it is next at the front.
///
/// # Safety
/// `ctx` is live; whatever `wait` names outlives the wait; the caller holds nothing a stop
/// must drop.
unsafe fn block(ctx: *mut RunCtx, wait: Wait, yielded: bool) {
    // SAFETY: caller contract.
    unsafe { crate::stop::check(ctx, try_block(ctx, wait, yielded)) }
}

/// [`block`], returning a stop to its caller.
///
/// # Safety
/// `ctx` is live; whatever `wait` names outlives the wait.
unsafe fn try_block(ctx: *mut RunCtx, wait: Wait, yielded: bool) -> Stopped<()> {
    if yielded {
        // SAFETY: the context is live.
        unsafe { poll_parked(ctx) }?;
        let s = scheduler(ctx);
        if !s.runnable.is_empty() {
            s.runnable.push_back(s.current);
            // SAFETY: the context is live and the queue is nonempty.
            let next =
                unsafe { pick(ctx) }?.unwrap_or_else(|| crate::fault("an empty runnable queue"));
            unsafe { switch_to(ctx, next) };
        }
    }
    let s = scheduler(ctx);
    s.park(wait);
    if let Wait::Host { token } = wait {
        let current = s.current;
        // SAFETY: the context is live, its Binding bound, and `token` blocked.
        unsafe { crate::host_waits::park(ctx, token, current) };
    }
    // SAFETY: the context is live; a poll makes this task runnable again.
    unsafe { run_next(ctx) }
}

/// Park the running task on host request `token` until a poll completes it (`crate::host_waits`).
/// `yielded`: the request yields first, and the task parks when it is next at the front.
///
/// # Safety
/// `ctx` is live and no borrow of its Io adapter or scheduler is held.
pub(crate) unsafe fn park_host(ctx: *mut RunCtx, token: u64, yielded: bool) -> Stopped<()> {
    // SAFETY: the context is live; the request outlives the wait.
    unsafe { try_block(ctx, Wait::Host { token }, yielded) }
}

/// Resume `child` directly (a join delivered its result, or its result is discarded) and
/// return when it completes.
///
/// # Safety
/// `ctx` is live and `child` is holding.
unsafe fn hand_off(ctx: *mut RunCtx, child: u32) {
    let s = scheduler(ctx);
    let current = s.current;
    s.task(current).state = State::Parked;
    let row = s.task(child);
    row.state = State::Ready;
    row.handoff = Some(current);
    unsafe { switch_to(ctx, child) };
}

/// The fiber entry of every spawned task.
unsafe extern "C" fn task_entry(ctx: *mut u8, task: *mut u8) -> ! {
    let ctx = ctx.cast::<RunCtx>();
    let index = task as usize as u32;
    recycle(ctx);
    let Some((code, environment, operation_site)) = scheduler(ctx).task(index).code.take() else {
        fault("a task started without code");
    };
    // SAFETY: compiled closure code and its owned environment, called once.
    let mut status = unsafe { code(ctx, environment, operation_site, operation_site) };
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
    unsafe { complete(ctx, index, status) }
}

/// The running child finished its frame (and its environment): record its outcome and run
/// another task.
///
/// # Safety
/// `ctx` is live and `index` is the running child.
unsafe fn complete(ctx: *mut RunCtx, index: u32, status: u8) -> ! {
    // This fiber is abandoned at the departure, so nothing it owns may still be live then.
    let next = unsafe { crate::stop::check(ctx, completed(ctx, index, status)) };
    // SAFETY: the context is live; this fiber is never resumed.
    unsafe { depart_to(ctx, next) }
}

/// Record the running child's completion and choose the task to run next.
///
/// # Safety
/// `ctx` is live and `index` is the running child.
unsafe fn completed(ctx: *mut RunCtx, index: u32, status: u8) -> Stopped<u32> {
    let s = scheduler(ctx);
    let path = s.task(index).path.clone();
    // SAFETY: the context is live.
    let chain = unsafe { (*ctx).failure.take_chain(&path) };
    let failed = status != STATUS_OK;
    if failed != !chain.is_empty() {
        fault("a task's status disagrees with its failure state");
    }
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
        let scope_row = s.scope(scope);
        scope_row.canceled.get_or_insert(CANCEL_CHILD_FAILED);
        let siblings = scope_row.children.clone();
        s.wake_owner(scope);
        for sibling in siblings {
            if sibling != index {
                s.cancel(sibling, CANCEL_CHILD_FAILED);
            }
        }
    }
    s.settle(index);
    if detached && !failed {
        // A detached child's result is gone at its completion; only live children stay rows.
        s.scope(scope).children.retain(|&c| c != index);
        s.retired = s.task(index).stack.take();
        s.vacate_task(index);
    } else {
        s.retired = s.task(index).stack.take();
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
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
) -> u8 {
    let (index, generation) = open_scope(ctx, None);
    // SAFETY: caller contract.
    unsafe {
        out.write(handle(index, generation));
        yield_now(ctx);
    }
    STATUS_OK
}

/// A fresh scope of the running task, its row and generation.
fn open_scope(ctx: *mut RunCtx, deadline: Option<ScopeDeadline>) -> (u32, u32) {
    let s = scheduler(ctx);
    let generation = s.fresh_generation();
    let current = s.current;
    let row = Scope {
        generation,
        owner: current,
        children: Vec::new(),
        canceled: None,
        deadline,
    };
    let index = match s.vacant_scopes.pop() {
        Some(index) => {
            s.scopes[index as usize] = Some(row);
            index
        }
        None => {
            s.scopes.push(Some(row));
            (s.scopes.len() - 1) as u32
        }
    };
    s.task(current).scopes.push(index);
    (index, generation)
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
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
    deadline: *const [u64; 2],
) -> u8 {
    // SAFETY: caller contract.
    let [clock, at] = unsafe { deadline.read() };
    let (index, generation) = open_scope(
        ctx,
        Some(ScopeDeadline {
            token: None,
            expired: false,
        }),
    );
    // SAFETY: the context is live and its Io bound.
    match unsafe { crate::stop::check(ctx, crate::host_waits::arm(ctx, index, clock, at)) } {
        Some(token) => {
            if let Some(d) = &mut scheduler(ctx).scope(index).deadline {
                d.token = Some(token);
            }
        }
        None => scheduler(ctx).expire(index),
    }
    // SAFETY: caller contract.
    unsafe {
        out.write(handle(index, generation));
        yield_now(ctx);
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
    ctx: *mut RunCtx,
    scope: *const u64,
    error: u8,
) -> u8 {
    // SAFETY: caller contract.
    unsafe { exit_scope(ctx, scope, error) }.0
}

/// `$intrinsic.TaskScopeExitUntil(s, error) -> bool`: the exit of a `task.scope_until` scope,
/// which removes its timer and writes whether its deadline fired, including while the exit
/// waited for children.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_scope_exit`]; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_scope_exit_until(
    ctx: *mut RunCtx,
    out: *mut u8,
    scope: *const u64,
    error: u8,
) -> u8 {
    // SAFETY: caller contract.
    let (status, expired) = unsafe { exit_scope(ctx, scope, error) };
    // SAFETY: caller contract.
    unsafe { out.write(u8::from(expired)) };
    status
}

/// A scope's exit: its status and whether its deadline fired.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_scope_exit`].
unsafe fn exit_scope(ctx: *mut RunCtx, scope: *const u64, error: u8) -> (u8, bool) {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let index = s.scope_handle(unsafe { scope.read() });
    let current = s.current;
    if s.task(current).scopes.pop() != Some(index) {
        fault("a scope exits before a scope entered inside it");
    }
    // SAFETY: the context is live.
    let failing = unsafe { (*ctx).failure.active() };
    if failing || error != 0 {
        for child in s.scope(index).children.clone() {
            s.cancel(child, CANCEL_PARENT);
        }
    }
    if !s.settled(index) {
        // SAFETY: the context is live; the exit is not a yield.
        unsafe { block(ctx, Wait::Exit { scope: index }, false) };
    }
    let s = scheduler(ctx);
    let children = s.scope(index).children.clone();
    for &child in children.iter().rev() {
        if s.task(child).state == State::Holding {
            s.task(child).slot = None;
            // SAFETY: the child is holding; its own cleanup drops the result.
            unsafe { hand_off(ctx, child) };
        }
    }
    let s = scheduler(ctx);
    let own = s.task(current).path.clone();
    for &child in &children {
        if let Some(chain) = s.task(child).failure.take() {
            // SAFETY: the context is live.
            unsafe { (*ctx).failure.merge_chain(&own, chain) };
        }
        s.vacate_task(child);
    }
    let deadline = s.scopes[index as usize].take().and_then(|row| row.deadline);
    s.vacant_scopes.push(index);
    // A scope that exits before its deadline removes the timer; nothing is recorded.
    if let Some(ScopeDeadline {
        token: Some(token), ..
    }) = &deadline
    {
        let token = *token;
        // SAFETY: the context is live and its Io bound, as the entry armed it.
        unsafe { crate::host_waits::disarm(ctx, token) };
    }
    let expired = deadline.is_some_and(|d| d.expired);
    // SAFETY: the context is live.
    let status = if unsafe { (*ctx).failure.active() } {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_spawn(
    ctx: *mut RunCtx,
    _caller_site: u64,
    operation_site: u64,
    _destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
    code: TaskCode,
    environment: *mut Header,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let current = s.current;
    // SAFETY: the context is live.
    let shield = unsafe { (*ctx).shield };
    let canceled = s.canceled(shield).map(|_| CANCEL_PARENT);
    let parent = s.task(current);
    let mut path = parent.path.clone();
    path.push(parent.spawned);
    parent.spawned += 1;
    let generation = s.fresh_generation();
    let mut task = Task::new(generation, Some(scope), path, canceled);
    task.code = Some((code, environment, operation_site));
    task.priority = s.schedule.admit();
    let index = match s.vacant_tasks.pop() {
        Some(index) => index,
        None => {
            s.tasks.push(None);
            (s.tasks.len() - 1) as u32
        }
    };
    let stack = s.idle_stacks.pop().unwrap_or_else(Stack::new);
    task.sp = stack.prepare(task_entry, ctx.cast(), index as usize as *mut u8);
    task.stack = Some(stack);
    s.tasks[index as usize] = Some(task);
    s.scope(scope).children.push(index);
    s.runnable.push_back(index);
    // SAFETY: caller contract.
    unsafe {
        out.write(handle(index, generation));
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.TaskFinish[R](mut slot, none, size)`: the running child computed `Some(r)` in
/// `slot`. A detached child keeps it, to drop; otherwise the child holds it, stopped in this
/// call, until a join's poll takes it (leaving `none`) and a join or a discard resumes it.
///
/// # Safety
/// `ctx` is live; `slot` and `none` are `Option[R]` images of `size` bytes, `slot` in the
/// running child's frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_finish(
    ctx: *mut RunCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
) -> u8 {
    let s = scheduler(ctx);
    let current = s.current;
    let held = Slot {
        slot,
        none,
        size: usize::try_from(size).unwrap_or_else(|_| fault("a result slot's size overflows")),
    };
    let row = s.task(current);
    if row.scope.is_none() {
        fault("the root task finished as a child");
    }
    if row.detached {
        return STATUS_OK;
    }
    row.slot = Some(held);
    row.state = State::Holding;
    s.order(current);
    s.settle(current);
    // SAFETY: the context is live; a join or a discard hands control back here.
    unsafe { crate::stop::check(ctx, run_next(ctx)) };
    STATUS_OK
}

/// `$intrinsic.TaskJoin[R](s, take h, mut slot, size)`: await `h` (CONC-2). Its result moves
/// into `slot` (an `Option[R]`, `None` on entry); a failed child's failures continue the running
/// task's chain and the status is 1. Joins are not cancellation points.
///
/// # Safety
/// `ctx` is live, `scope` and `task` name a child of a live scope, and `slot` is a `None` image
/// of `size` bytes in the running task's frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_join(
    ctx: *mut RunCtx,
    scope: *const u64,
    task: *mut u64,
    slot: *mut u8,
    size: u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let child = s.task_handle(unsafe { task.read() }, scope);
    // SAFETY: caller contract.
    unsafe { join(ctx, scope, child, slot, size) }
}

/// Join `child` of `scope`, as [`hirsel_rt_v2_task_join`] does.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_join`].
unsafe fn join(ctx: *mut RunCtx, scope: u32, child: u32, slot: *mut u8, size: u64) -> u8 {
    let s = scheduler(ctx);
    match s.task(child).state {
        State::Holding => {
            let held = s.task(child).slot.take().unwrap();
            // SAFETY: the child is stopped in its finish, its slot live.
            unsafe {
                deliver(held, slot, size);
                hand_off(ctx, child);
                yield_now(ctx);
            }
        }
        State::Done => unsafe { yield_now(ctx) },
        State::Ready | State::Parked => {
            // SAFETY: the slot is in this task's frame, parked with it.
            unsafe { block(ctx, Wait::Join { child, slot, size }, true) };
            // The poll moved a holding child's result; its frame finishes now.
            if scheduler(ctx).task(child).state == State::Holding {
                unsafe { hand_off(ctx, child) };
            }
        }
    }
    let s = scheduler(ctx);
    let chain = s.task(child).failure.take();
    s.scope(scope).children.retain(|&c| c != child);
    s.vacate_task(child);
    match chain {
        Some(chain) => {
            // SAFETY: the context is live.
            unsafe { (*ctx).failure.append_chain(chain) };
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
    ctx: *mut RunCtx,
    scope: *const u64,
    task: *const u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let child = s.task_handle(unsafe { task.read() }, scope);
    s.cancel(child, CANCEL_REQUESTED);
    // SAFETY: the context is live.
    unsafe { yield_now(ctx) };
    STATUS_OK
}

/// `task.detach(s, take h)`: the child keeps running inside its scope; its result is dropped
/// when it completes, at once when it already has.
///
/// # Safety
/// `ctx` is live and `scope`, `task` name a child of a live scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_detach(
    ctx: *mut RunCtx,
    scope: *const u64,
    task: *mut u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let child = s.task_handle(unsafe { task.read() }, scope);
    // SAFETY: the context is live.
    unsafe {
        detach(ctx, scope, child);
        yield_now(ctx);
    }
    STATUS_OK
}

/// Detach `child` of `scope`: a holding child drops its result now, and a completed one
/// leaves its scope's rows unless it failed.
///
/// # Safety
/// `ctx` is live and `child` a child of live `scope`.
unsafe fn detach(ctx: *mut RunCtx, scope: u32, child: u32) {
    let s = scheduler(ctx);
    s.task(child).detached = true;
    if s.task(child).state == State::Holding {
        s.task(child).slot = None;
        // SAFETY: the child is holding; its own cleanup drops the result.
        unsafe { hand_off(ctx, child) };
    }
    let s = scheduler(ctx);
    if let Some(row) = s.tasks[child as usize].as_ref()
        && row.state == State::Done
        && row.failure.is_none()
    {
        s.scope(scope).children.retain(|&c| c != child);
        s.vacate_task(child);
    }
}

/// `task.set(s) -> TaskSet[R] of s` (CONC-2): an empty task set of `s`.
///
/// # Safety
/// `ctx` is live, `out` writable and `scope` names a live scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_set(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let generation = s.fresh_generation() & 0x3fff_ffff;
    let row = TaskSet {
        generation,
        scope,
        members: Vec::new(),
    };
    let index = match s.vacant_sets.pop() {
        Some(index) => {
            s.sets[index as usize] = Some(row);
            index
        }
        None => {
            s.sets.push(Some(row));
            (s.sets.len() - 1) as u32
        }
    };
    // SAFETY: caller contract.
    unsafe {
        out.write(
            crate::channels::ENDPOINT | TASK_SET | u64::from(generation) << 32 | u64::from(index),
        );
        yield_now(ctx);
    }
    STATUS_OK
}

/// `task.add(s, mut set, take h)`: `h` becomes the set's next member.
///
/// # Safety
/// `ctx` is live and `set`, `task` name a task set and a child of live `scope`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_add(
    ctx: *mut RunCtx,
    scope: *const u64,
    set: *mut u64,
    task: *mut u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let (scope, set, word) = unsafe { (s.scope_handle(scope.read()), set.read(), task.read()) };
    s.task_handle(word, scope);
    let set = s.set_handle(set, scope);
    s.set(set).members.push(Some(word));
    // SAFETY: the context is live.
    unsafe { yield_now(ctx) };
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
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut IndexOption,
    scope: *const u64,
    set: *const u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let set = s.set_handle(unsafe { set.read() }, scope);
    let waiting = s.set(set).members.iter().any(Option::is_some);
    if waiting && s.first_completed(set).is_none() {
        // SAFETY: the set outlives the wait: the caller borrows it.
        unsafe { block(ctx, Wait::Any { set }, true) };
    } else {
        // SAFETY: the context is live.
        unsafe { yield_now(ctx) };
    }
    let index = if waiting {
        scheduler(ctx).first_completed(set)
    } else {
        None
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

/// `$intrinsic.TaskSetHas(s, set, index) -> bool`: whether member `index` is unawaited, which
/// `task.await_at`'s compiled body checks (CONC-2) before it takes and joins it. Not a yield.
///
/// # Safety
/// `ctx` is live, `out` writable and `set` names a task set of live `scope`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_set_has(
    ctx: *mut RunCtx,
    out: *mut u8,
    scope: *const u64,
    set: *const u64,
    index: u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let set = s.set_handle(unsafe { set.read() }, scope);
    let has = usize::try_from(index)
        .ok()
        .and_then(|i| s.set(set).members.get(i))
        .is_some_and(Option::is_some);
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
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
    set: *mut u64,
    index: u64,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let set = s.set_handle(unsafe { set.read() }, scope);
    let word = usize::try_from(index)
        .ok()
        .and_then(|i| s.set(set).members.get_mut(i))
        .and_then(Option::take)
        .unwrap_or_else(|| fault("a task set member taken twice"));
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
    ctx: *mut RunCtx,
    scope: *const u64,
    set: *mut u64,
) -> u8 {
    // SAFETY: caller contract.
    let set = unsafe { release_members(ctx, scope, set, true) };
    let s = scheduler(ctx);
    // SAFETY: the context is live; the set row stays until the drain is over.
    if unsafe { poll_drained(s, set) } {
        unsafe { yield_now(ctx) };
    } else {
        unsafe { block(ctx, Wait::Drain { set }, true) };
    }
    scheduler(ctx).close_set(set);
    STATUS_OK
}

/// `task.detach_all(s, take set)`: detach every unawaited member.
///
/// # Safety
/// `ctx` is live and `set` names a task set of live `scope`, which the call takes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_detach_all(
    ctx: *mut RunCtx,
    scope: *const u64,
    set: *mut u64,
) -> u8 {
    // SAFETY: caller contract.
    let set = unsafe { release_members(ctx, scope, set, false) };
    scheduler(ctx).close_set(set);
    // SAFETY: the context is live.
    unsafe { yield_now(ctx) };
    STATUS_OK
}

/// Detach every unawaited member of `set`, canceling each first when `cancel`.
///
/// # Safety
/// As for [`hirsel_rt_v2_task_cancel_all`].
unsafe fn release_members(ctx: *mut RunCtx, scope: *const u64, set: *mut u64, cancel: bool) -> u32 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let scope = s.scope_handle(unsafe { scope.read() });
    let set = s.set_handle(unsafe { set.read() }, scope);
    let members: Vec<u64> = s.set(set).members.iter().flatten().copied().collect();
    for word in members {
        let child = scheduler(ctx).task_handle(word, scope);
        if cancel {
            scheduler(ctx).cancel(child, CANCEL_REQUESTED);
        }
        // SAFETY: caller contract.
        unsafe { detach(ctx, scope, child) };
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
    ctx: *mut RunCtx,
    out: *mut u32,
    _scope: *const u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        out.write(canceled(ctx).map_or(0, |cause| cause + 1));
        yield_now(ctx);
    }
    STATUS_OK
}

/// The running task's cancellation cause, if any: for `task.check_canceled` and the
/// cancellation points of blocking Io (CONC-3). Shielded destruction observes none.
///
/// # Safety
/// `ctx` is live.
pub(crate) unsafe fn canceled(ctx: *mut RunCtx) -> Option<u32> {
    // SAFETY: the context is live.
    let shield = unsafe { (*ctx).shield };
    unsafe { (*ctx).tasks.as_deref_mut() }.and_then(|s| s.canceled(shield))
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
    slot: *mut *mut Header,
) -> Option<u32> {
    if let Some(cause) = s.canceled_task(task, shield) {
        return Some(crate::channels::CANCELED + cause);
    }
    match s.channels.receive(channel) {
        Ok(Some(message)) => {
            // SAFETY: caller contract.
            unsafe { slot.write(message.as_ptr()) };
            Some(crate::channels::DONE)
        }
        Ok(None) => None,
        Err(status) => Some(status),
    }
}

/// `channel.open(tasks) -> (Sender[T], Receiver[T])` (CONC-6).
///
/// # Safety
/// `ctx` is live and `out` writable for two words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_channel_open(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
) -> u8 {
    let (sender, receiver) = scheduler(ctx).channels.open();
    // SAFETY: caller contract.
    unsafe {
        out.write(sender);
        out.add(1).write(receiver);
        yield_now(ctx);
    }
    STATUS_OK
}

/// `sender.dup(tasks, s) -> Sender[T]`: another Sender of `s`'s channel.
///
/// # Safety
/// `ctx` is live, `out` writable and `sender` an open Sender word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_sender_dup(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    _tasks: *const u64,
    sender: *const u64,
) -> u8 {
    // SAFETY: caller contract.
    let word = scheduler(ctx).channels.dup(unsafe { sender.read() });
    unsafe {
        out.write(word);
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.ChannelSend[T](mut s, mut message)`: queue the `Some` block of `message`,
/// leaving `None`, unless the running task is canceled or no receiver is left; then the
/// message stays and the status says why.
///
/// # Safety
/// `ctx` is live, `out` writable, `sender` an open Sender word and `message` a `Some` slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_channel_send(
    ctx: *mut RunCtx,
    out: *mut u32,
    sender: *mut u64,
    message: *mut *mut Header,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let channel = s.channels.channel(unsafe { sender.read() }, true);
    let shield = unsafe { (*ctx).shield };
    let status = match s.canceled(shield) {
        Some(cause) => crate::channels::CANCELED + cause,
        None => {
            // SAFETY: caller contract.
            let block = NonNull::new(unsafe { message.read() })
                .unwrap_or_else(|| fault("a send of an empty message slot"));
            let status = s.channels.send(channel, block);
            if status == crate::channels::DONE {
                unsafe { message.write(std::ptr::null_mut()) };
            }
            status
        }
    };
    // SAFETY: caller contract.
    unsafe {
        out.write(status);
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.ChannelReceive[T](mut r, mut message)`: move the oldest message into the
/// `None` slot `message`, parking while the queue is empty and a sender is left.
///
/// # Safety
/// `ctx` is live, `out` writable, `receiver` an open Receiver word and `message` a `None` slot
/// in the running task's frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_channel_receive(
    ctx: *mut RunCtx,
    out: *mut u32,
    receiver: *mut u64,
    message: *mut *mut Header,
) -> u8 {
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let channel = s.channels.channel(unsafe { receiver.read() }, false);
    let current = s.current;
    let shield = unsafe { (*ctx).shield };
    // SAFETY: caller contract.
    let status = match unsafe { receive_ready(s, current, shield, channel, message) } {
        Some(status) => {
            // SAFETY: the context is live.
            unsafe { yield_now(ctx) };
            status
        }
        None => {
            // SAFETY: the slot is in this task's frame, parked with it.
            unsafe {
                block(
                    ctx,
                    Wait::Receive {
                        channel,
                        slot: message,
                    },
                    true,
                )
            };
            scheduler(ctx).task(current).status
        }
    };
    // SAFETY: caller contract.
    unsafe { out.write(status) };
    STATUS_OK
}

/// Close the channel endpoint `word` (a dropped Sender or Receiver); a last receiver releases
/// the messages nobody received, oldest first. Closing is not a yield.
///
/// # Safety
/// `ctx` is live and `word` an open endpoint of its run.
pub(crate) unsafe fn close_endpoint(
    ctx: *mut RunCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    if word & TASK_SET != 0 {
        // A dropped task set: its members stay children of their scope, joined at its exit.
        let s = scheduler(ctx);
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
        return STATUS_OK;
    }
    let left = scheduler(ctx).channels.close(word);
    let mut status = STATUS_OK;
    for message in left {
        // SAFETY: each queued message is one owned credit of its block.
        status |= unsafe {
            crate::release::release_block(
                &mut *ctx,
                message.as_ptr(),
                std::ptr::null(),
                anchor,
                operation_site,
            )
        };
    }
    status
}

/// Move the payload block out of the `Some` carrier at `payload`, leaving `None`.
///
/// # Safety
/// `payload` is a readable and writable `Option[List[T]]` holding `Some`.
unsafe fn carried(payload: *mut *mut Header) -> NonNull<Header> {
    // SAFETY: caller contract.
    let block = unsafe { payload.replace(std::ptr::null_mut()) };
    NonNull::new(block).unwrap_or_else(|| fault("a mutex payload carrier holding None"))
}

/// `$intrinsic.MutexCreate[T](mut payload)` (CONC-5): a mutex owning the payload block, taken
/// from the `Some` carrier, which is left `None`.
///
/// # Safety
/// `ctx` is live, `out` writable and `payload` a `Some` carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_create(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    payload: *mut *mut Header,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        let block = carried(payload);
        out.write(scheduler(ctx).sync.create_block(block));
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.MutexTake[T](m, mut payload)`: move the payload block into the `None` carrier
/// (status 0), or report the mutex dead (status 1). Acquiring is not a yield.
///
/// # Safety
/// `ctx` is live, `out` writable, `m` a Mutex word and `payload` a writable `None` carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_take(
    ctx: *mut RunCtx,
    out: *mut u32,
    m: *const u64,
    payload: *mut *mut Header,
) -> u8 {
    let sync = &mut scheduler(ctx).sync;
    // SAFETY: caller contract.
    let word = unsafe { m.read() };
    let status = if sync.dead(word).is_some() {
        1
    } else {
        // SAFETY: caller contract.
        unsafe { payload.write(sync.take(word).as_ptr()) };
        0
    };
    // SAFETY: caller contract.
    unsafe { out.write(status) };
    STATUS_OK
}

/// `$intrinsic.MutexPut[T](m, mut payload)`: the body returned; its payload's block goes back
/// from the `Some` carrier, which is left `None`, and the operation yields.
///
/// # Safety
/// `ctx` is live, `m` a Mutex word and `payload` a `Some` carrier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_put(
    ctx: *mut RunCtx,
    m: *const u64,
    payload: *mut *mut Header,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        let block = carried(payload);
        scheduler(ctx).sync.put(m.read(), block);
        yield_now(ctx);
    }
    STATUS_OK
}

/// The run ended: release the payload blocks its mutexes still hold. Their payloads are
/// copyable, so the release runs no user code and cannot fail.
pub(crate) fn end_run(ctx: &mut RunCtx) {
    let Some(s) = ctx.tasks.as_deref_mut() else {
        return;
    };
    for block in s.sync.end() {
        // SAFETY: the cell owned the block's credit.
        if unsafe { crate::release::release_block(ctx, block.as_ptr(), std::ptr::null(), 0, 0) }
            != STATUS_OK
        {
            fault("a mutex payload's release failed");
        }
    }
}

/// `$intrinsic.MutexKill[T](m)` (FAIL-4): the body failed; the mutex keeps the failure chain,
/// and the operation yields.
///
/// # Safety
/// `ctx` is live, its failure active, and `m` a Mutex word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_kill(ctx: *mut RunCtx, m: *const u64) -> u8 {
    let s = scheduler(ctx);
    let current = s.current;
    let path = s.task(current).path.clone();
    // SAFETY: the context is live.
    let chain = unsafe { (*ctx).failure.chain(&path) };
    // SAFETY: caller contract.
    unsafe {
        scheduler(ctx).sync.kill(m.read(), chain);
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.MutexCause[T](m)` (FAIL-4): continue the running task's failure, a dead mutex's
/// acquisition, with the chain that killed it.
///
/// # Safety
/// `ctx` is live and `m` a dead Mutex's word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_cause(ctx: *mut RunCtx, m: *const u64) -> u8 {
    // SAFETY: caller contract.
    let chain = scheduler(ctx)
        .sync
        .dead(unsafe { m.read() })
        .cloned()
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
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    value: *const u8,
    size: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        out.write(scheduler(ctx).sync.create_image(value, size));
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.AtomicLoad[A](a, mut slot, size)`: copy the atomic's image into the `None` slot.
///
/// # Safety
/// `ctx` is live, `a` an Atomic word and `slot` writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_atomic_load(
    ctx: *mut RunCtx,
    a: *const u64,
    slot: *mut u8,
    size: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        scheduler(ctx).sync.read(a.read(), slot, size);
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.AtomicStore[A](a, value, size)`: replace the atomic's image.
///
/// # Safety
/// `ctx` is live, `a` an Atomic word and `value` readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_atomic_store(
    ctx: *mut RunCtx,
    a: *const u64,
    value: *const u8,
    size: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        scheduler(ctx).sync.write(a.read(), value, size);
        yield_now(ctx);
    }
    STATUS_OK
}
