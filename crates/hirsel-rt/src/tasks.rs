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
//! - one FIFO queue of runnable tasks; a spawned child joins its back;
//! - after every capability operation ([`yield_now`]) the running task, when another is
//!   runnable, joins the back and the front runs;
//! - a task that must wait (a join, a scope exit) parks with what it waits for ([`Wait`]).
//!   Parked tasks are polled in park order at every yield and whenever the running task stops
//!   (it parked, holds its result or completed); a successful poll performs the wait's effect
//!   and moves the task to the back of the queue. An operation that parks yields first, and its
//!   task parks when it is next at the front.
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

use std::collections::VecDeque;
use std::ptr::NonNull;

use crate::context::RunCtx;
use crate::failure::{Chained, FailureState};
use crate::fiber::{self, Stack};
use crate::region::Region;
use crate::{Header, STATUS_FAILURE, STATUS_OK, fault};

/// A spawned task's entry: compiled closure code taking its environment.
pub type TaskCode = unsafe extern "C" fn(*mut RunCtx, *mut Header) -> u8;

/// `CancelCause` ordinals (core library).
pub(crate) const CANCEL_REQUESTED: u32 = 0;
pub(crate) const CANCEL_PARENT: u32 = 1;
pub(crate) const CANCEL_CHILD_FAILED: u32 = 2;

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
    code: Option<(TaskCode, *mut Header)>,
    slot: Option<Slot>,
    /// What the task waits for while parked.
    wait: Option<Wait>,
    /// The status a receive's poll produced.
    status: u32,
    /// The task to resume directly when this one completes (a join or a discard ran it).
    handoff: Option<u32>,
    detached: bool,
    failure: Option<Vec<Chained>>,
}

struct Scope {
    generation: u32,
    children: Vec<u32>,
    canceled: Option<u32>,
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
    /// Parked tasks, in park order.
    parked: Vec<u32>,
    pub(crate) channels: crate::channels::Channels,
    sync: crate::sync::Synchronizers,
    idle_stacks: Vec<Stack>,
    /// The stack of a task that completed; the next task to run recycles it.
    retired: Option<Stack>,
}

fn handle(index: u32, generation: u32) -> u64 {
    u64::from(generation) << 32 | u64::from(index)
}

impl Scheduler {
    fn new() -> Scheduler {
        let mut scheduler = Scheduler::default();
        scheduler
            .tasks
            .push(Some(Task::new(1, None, Vec::new(), None)));
        scheduler.generation = 1;
        scheduler
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
    fn canceled_task(&mut self, task: u32, shield: u32) -> Option<u32> {
        if shield > 0 {
            return None;
        }
        let task = self.task(task);
        if let Some(cause) = task.canceled {
            return Some(cause);
        }
        let scopes = task.scopes.clone();
        scopes
            .iter()
            .rev()
            .find_map(|&s| self.scopes[s as usize].as_ref().and_then(|s| s.canceled))
    }
    /// Cancel `task` with `cause` and its descendants with `Parent`; an earlier cause stays.
    fn cancel(&mut self, task: u32, cause: u32) {
        let path = self.task(task).path.clone();
        for row in self.tasks.iter_mut().flatten() {
            if row.path.starts_with(&path) && row.scope.is_some() {
                row.canceled.get_or_insert(if row.path.len() == path.len() {
                    cause
                } else {
                    CANCEL_PARENT
                });
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
        }
    }
}

/// The run's scheduler, created with the root task at the first scope.
fn scheduler(ctx: *mut RunCtx) -> &'static mut Scheduler {
    // SAFETY: the context is live for the whole run, and the scheduler is only reached
    // through it, by the one running task; no reference is kept across a switch.
    unsafe {
        (*ctx)
            .tasks
            .get_or_insert_with(|| Box::new(Scheduler::new()))
    }
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

/// Poll every parked task in park order: each whose wait is over has its effect performed and
/// joins the back of the queue.
///
/// # Safety
/// `ctx` is live.
unsafe fn poll_parked(ctx: *mut RunCtx) {
    let s = scheduler(ctx);
    let parked = std::mem::take(&mut s.parked);
    let mut waiting = Vec::with_capacity(parked.len());
    for task in parked {
        // SAFETY: the context is live and `task` is parked.
        if unsafe { poll(ctx, task) } {
            let s = scheduler(ctx);
            let row = s.task(task);
            row.state = State::Ready;
            row.wait = None;
            s.runnable.push_back(task);
        } else {
            waiting.push(task);
        }
    }
    // A poll never parks a task.
    scheduler(ctx).parked = waiting;
}

/// Whether parked `task`'s wait is over, after performing its effect if it is.
///
/// # Safety
/// `ctx` is live and `task` is parked; a join's slot is in its frame.
unsafe fn poll(ctx: *mut RunCtx, task: u32) -> bool {
    let s = scheduler(ctx);
    match s.task(task).wait {
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
        None => fault("a parked task without a wait"),
    }
}

/// The running task stopped (it parked, holds its result or completed): poll the parked tasks,
/// then run the front of the queue, which may be the running task itself.
///
/// # Safety
/// `ctx` is live.
unsafe fn run_next(ctx: *mut RunCtx) {
    // SAFETY: the context is live.
    let next = unsafe { next_task(ctx) };
    unsafe { switch_to(ctx, next) };
}

/// Poll the parked tasks and take the front of the queue.
///
/// # Safety
/// `ctx` is live.
unsafe fn next_task(ctx: *mut RunCtx) -> u32 {
    // SAFETY: the context is live.
    unsafe { poll_parked(ctx) };
    let Some(next) = scheduler(ctx).runnable.pop_front() else {
        // CONC-8: no task can run and nothing outside the run can wake one.
        crate::environmental("every task of the run is blocked (CONC-8)");
    };
    next
}

/// The yield after a capability operation: the parked tasks are polled and, when another task
/// is runnable, the running task joins the back of the queue and the front runs.
///
/// # Safety
/// `ctx` is live and no borrow of its Io adapter or scheduler is held.
pub unsafe fn yield_now(ctx: *mut RunCtx) {
    // SAFETY: the context is live.
    if unsafe { (*ctx).tasks.is_none() } {
        return;
    }
    // SAFETY: the context is live.
    unsafe { poll_parked(ctx) };
    let s = scheduler(ctx);
    let Some(next) = s.runnable.pop_front() else {
        return;
    };
    s.runnable.push_back(s.current);
    unsafe { switch_to(ctx, next) };
}

/// Park the running task until `wait` is over; returns once a poll performed its effect.
/// `yielded`: the operation yields first, and the task parks when it is next at the front.
///
/// # Safety
/// `ctx` is live; whatever `wait` names outlives the wait.
unsafe fn block(ctx: *mut RunCtx, wait: Wait, yielded: bool) {
    if yielded {
        // SAFETY: the context is live.
        unsafe { poll_parked(ctx) };
        let s = scheduler(ctx);
        if let Some(next) = s.runnable.pop_front() {
            s.runnable.push_back(s.current);
            unsafe { switch_to(ctx, next) };
        }
    }
    let s = scheduler(ctx);
    let current = s.current;
    let row = s.task(current);
    row.state = State::Parked;
    row.wait = Some(wait);
    s.parked.push(current);
    // SAFETY: the context is live; a poll makes this task runnable again.
    unsafe { run_next(ctx) };
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
    let Some((code, environment)) = scheduler(ctx).task(index).code.take() else {
        fault("a task started without code");
    };
    // SAFETY: compiled closure code and its owned environment, called once.
    let mut status = unsafe { code(ctx, environment) };
    // The caller of a closure releases its environment wholesale.
    status |= unsafe { crate::release::release_block(&mut *ctx, environment, std::ptr::null(), 0) };
    unsafe { complete(ctx, index, status) }
}

/// The running child finished its frame (and its environment): record its outcome and run
/// another task.
///
/// # Safety
/// `ctx` is live and `index` is the running child.
unsafe fn complete(ctx: *mut RunCtx, index: u32, status: u8) -> ! {
    // This fiber is abandoned at the departure, so nothing it owns may still be live then.
    let next = unsafe { completed(ctx, index, status) };
    // SAFETY: the context is live; this fiber is never resumed.
    unsafe { depart_to(ctx, next) }
}

/// Record the running child's completion and choose the task to run next.
///
/// # Safety
/// `ctx` is live and `index` is the running child.
unsafe fn completed(ctx: *mut RunCtx, index: u32, status: u8) -> u32 {
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
        // FAIL-2: a failed child cancels its siblings and its scope's later blocking effects.
        let scope_row = s.scope(scope);
        scope_row.canceled.get_or_insert(CANCEL_CHILD_FAILED);
        let siblings = scope_row.children.clone();
        for sibling in siblings {
            if sibling != index {
                s.cancel(sibling, CANCEL_CHILD_FAILED);
            }
        }
    }
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
        Some(next) => next,
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
    let s = scheduler(ctx);
    let generation = s.fresh_generation();
    let row = Scope {
        generation,
        children: Vec::new(),
        canceled: None,
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
    let current = s.current;
    s.task(current).scopes.push(index);
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
    let s = scheduler(ctx);
    // SAFETY: caller contract.
    let index = s.scope_handle(unsafe { scope.read() });
    let current = s.current;
    if s.task(current).scopes.pop() != Some(index) {
        fault("a scope exits before a scope entered inside it");
    }
    // SAFETY: the context is live.
    let failing = unsafe { (*ctx).failure.active() };
    let children = s.scope(index).children.clone();
    if failing || error != 0 {
        for &child in &children {
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
    s.scopes[index as usize] = None;
    s.vacant_scopes.push(index);
    // SAFETY: the context is live.
    if unsafe { (*ctx).failure.active() } {
        STATUS_FAILURE
    } else {
        STATUS_OK
    }
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
    task.code = Some((code, environment));
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
    // SAFETY: the context is live; a join or a discard hands control back here.
    unsafe { run_next(ctx) };
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
    // SAFETY: the context is live.
    unsafe { yield_now(ctx) };
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
pub(crate) unsafe fn close_endpoint(ctx: *mut RunCtx, word: u64, anchor: u64) -> u8 {
    let left = scheduler(ctx).channels.close(word);
    let mut status = STATUS_OK;
    for message in left {
        // SAFETY: each queued message is one owned credit of its block.
        status |= unsafe {
            crate::release::release_block(&mut *ctx, message.as_ptr(), std::ptr::null(), anchor)
        };
    }
    status
}

/// `$intrinsic.MutexCreate[T](value, size)` (CONC-5): a mutex holding the `Some` image `value`.
///
/// # Safety
/// `ctx` is live, `out` writable and `value` readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_create(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    value: *const u8,
    size: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        out.write(scheduler(ctx).sync.create(value, size));
        yield_now(ctx);
    }
    STATUS_OK
}

/// `$intrinsic.MutexTake[T](m, mut slot, size)`: move the payload's image into the `None` slot
/// (status 0), or report the mutex dead (status 1). Acquiring is not a yield.
///
/// # Safety
/// `ctx` is live, `out` writable, `m` a Mutex word and `slot` writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_take(
    ctx: *mut RunCtx,
    out: *mut u32,
    m: *const u64,
    slot: *mut u8,
    size: u64,
) -> u8 {
    let sync = &mut scheduler(ctx).sync;
    // SAFETY: caller contract.
    let word = unsafe { m.read() };
    let status = if sync.dead(word).is_some() {
        1
    } else {
        // SAFETY: caller contract.
        unsafe { sync.read(word, slot, size, true) };
        0
    };
    // SAFETY: caller contract.
    unsafe { out.write(status) };
    STATUS_OK
}

/// `$intrinsic.MutexPut[T](m, value, size)`: the body returned; its payload's image goes back,
/// and the operation yields.
///
/// # Safety
/// `ctx` is live, `m` a Mutex word and `value` readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_mutex_put(
    ctx: *mut RunCtx,
    m: *const u64,
    value: *const u8,
    size: u64,
) -> u8 {
    // SAFETY: caller contract.
    unsafe {
        scheduler(ctx).sync.write(m.read(), value, size);
        yield_now(ctx);
    }
    STATUS_OK
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
        out.write(scheduler(ctx).sync.create(value, size));
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
        scheduler(ctx).sync.read(a.read(), slot, size, false);
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
