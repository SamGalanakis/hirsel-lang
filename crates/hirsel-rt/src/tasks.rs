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
//! - a task waiting for a join parks, and joins the back when the awaited child completes.
//!
//! A child runs the compiled `$native.task_main` closure the spawn received. On success that
//! body hands its `Some(result)` slot to [`hirsel_rt_v2_task_finish`] and, unless an awaiter
//! already waits, stays parked in that call: a completed child holds its result in its own
//! frame. A join moves the slot's whole `Option` image into the awaiter's slot and writes the
//! child's `None` back; a discard resumes the child with its slot intact, and the child's own
//! compiled cleanup drops the result. The runtime therefore moves results without knowing their
//! layouts. Either way the child then finishes its frame, the runtime releases its closure
//! environment, and the child completes: completion follows cleanup and precedes the join
//! (CONC-2). A failed child completes with its failure chain instead, which cancels its
//! siblings and its scope (cause `ChildFailed`).
//!
//! Handles are run-local: a Scope or Task word is a table index with a generation, so a stale
//! or foreign word is an invariant fault rather than another task's row.

use std::collections::VecDeque;

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
    /// Waiting for a join or a child's discard.
    Parked,
    /// Completed with a value it holds in its `$native.task_main` frame.
    Holding,
    /// Completed: its value was delivered or dropped, or it failed.
    Done,
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
    /// A task parked in a join of this one, and the slot it receives the result in.
    awaiter: Option<(u32, *mut u8)>,
    /// The task to resume directly when this one completes (a join or a discard ran it).
    handoff: Option<u32>,
    detached: bool,
    failure: Option<Vec<Chained>>,
}

struct Scope {
    generation: u32,
    children: Vec<u32>,
    canceled: Option<u32>,
    /// The task parked in this scope's exit until every child completes.
    joiner: Option<u32>,
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
        if ctx_shield > 0 {
            return None;
        }
        let current = self.current;
        let task = self.task(current);
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
            awaiter: None,
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

/// Run the front of the queue; the running task has parked or is holding.
///
/// # Safety
/// `ctx` is live.
unsafe fn run_next(ctx: *mut RunCtx) {
    let Some(next) = scheduler(ctx).runnable.pop_front() else {
        // CONC-8: no task can run and nothing outside the run can wake one.
        crate::environmental("every task of the run is blocked (CONC-8)");
    };
    unsafe { switch_to(ctx, next) };
}

/// The yield after a capability operation: when another task is runnable, the running task
/// joins the back of the queue and the front runs.
///
/// # Safety
/// `ctx` is live and no borrow of its Io adapter or scheduler is held.
pub unsafe fn yield_now(ctx: *mut RunCtx) {
    // SAFETY: the context is live.
    let Some(s) = (unsafe { (*ctx).tasks.as_deref_mut() }) else {
        return;
    };
    let Some(next) = s.runnable.pop_front() else {
        return;
    };
    s.runnable.push_back(s.current);
    unsafe { switch_to(ctx, next) };
}

/// Make a parked task runnable.
fn wake(ctx: *mut RunCtx, task: u32) {
    let s = scheduler(ctx);
    let row = s.task(task);
    if row.state != State::Parked {
        fault("waking a task that is not parked");
    }
    row.state = State::Ready;
    s.runnable.push_back(task);
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

/// The running child finished its frame (and its environment): record its outcome, wake
/// whoever waits for it and run another task.
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
    let awaiter = row.awaiter.take();
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
    if let Some((awaiter, _)) = awaiter {
        wake(ctx, awaiter);
    }
    notify_joiner(ctx, scope);
    let next = handoff.or_else(|| scheduler(ctx).runnable.pop_front());
    let Some(next) = next else {
        // CONC-8: no task can run and nothing outside the run can wake one.
        crate::environmental("every task of the run is blocked (CONC-8)");
    };
    next
}

/// Wake a scope's exit when its last child completes.
fn notify_joiner(ctx: *mut RunCtx, scope: u32) {
    let s = scheduler(ctx);
    let Some(row) = s.scopes[scope as usize].as_ref() else {
        return;
    };
    let Some(joiner) = row.joiner else {
        return;
    };
    let children = row.children.clone();
    if children
        .iter()
        .all(|&c| matches!(s.task(c).state, State::Holding | State::Done))
    {
        s.scope(scope).joiner = None;
        wake(ctx, joiner);
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
        joiner: None,
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
    if !children
        .iter()
        .all(|&c| matches!(s.task(c).state, State::Holding | State::Done))
    {
        s.scope(index).joiner = Some(current);
        s.task(current).state = State::Parked;
        // SAFETY: the context is live; the last child to complete wakes this task.
        unsafe { run_next(ctx) };
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
/// `slot`. An awaiter already waiting receives it now and the slot becomes `none`; a detached
/// child keeps it, to drop; otherwise the child parks holding it until a join takes it or its
/// scope discards it.
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
    if let Some((_, out)) = row.awaiter {
        // SAFETY: the awaiter is parked in its join, its slot live.
        unsafe { deliver(held, out, size) };
        return STATUS_OK;
    }
    if row.detached {
        return STATUS_OK;
    }
    row.slot = Some(held);
    row.state = State::Holding;
    let scope = row.scope.unwrap();
    notify_joiner(ctx, scope);
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
    let current = s.current;
    match s.task(child).state {
        State::Holding => {
            let held = s.task(child).slot.take().unwrap();
            // SAFETY: the child is parked in its finish, its slot live.
            unsafe {
                deliver(held, slot, size);
                hand_off(ctx, child);
                yield_now(ctx);
            }
        }
        State::Done => unsafe { yield_now(ctx) },
        State::Ready | State::Parked => {
            s.task(child).awaiter = Some((current, slot));
            s.task(current).state = State::Parked;
            // SAFETY: the child's completion wakes this task.
            unsafe { run_next(ctx) };
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
