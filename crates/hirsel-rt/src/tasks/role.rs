//! Scheduler-role ownership, separate from a worker's compute permit.
//!
//! The mutex protects only the owner predicate. No MutexGuard crosses a fiber
//! switch, a wait, or user code. This is Rust std Condvar's predicate-under-lock
//! protocol (`library/std/src/sync/poison/condvar.rs`, wait_while), with a task's
//! stable context address as the owner so migration keeps ownership explicit.
//! The W1 run token remains the default; concurrent drivers enable this role
//! before starting workers, after installing boundary coverage.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

use crate::{TaskCtx, context::RunShared};

// Task contexts are aligned pointers; temporary claims use a u32 task ID.
// This marker is used only by acquire_idle, which never permits re-entry.
// The marker is even but not aligned like TaskCtx, and claims are odd.
const IDLE_OWNER: usize = usize::MAX - 1;

pub(crate) mod cell;

#[derive(Default)]
pub(crate) struct Role {
    enabled: AtomicBool,
    owner: Mutex<Option<usize>>,
    available: Condvar,
}

impl Role {
    /// Admission only, before any worker can execute Hirsel code.
    pub(crate) fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    fn acquire(&self, task: usize) {
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        while owner.is_some_and(|owner| owner != task) {
            owner = self
                .available
                .wait(owner)
                .unwrap_or_else(|e| e.into_inner());
        }
        *owner = Some(task);
    }

    fn acquire_idle(&self) {
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        // Unlike task acquisition, even the same marker must wait. Distinct
        // idle workers have no task identity and cannot share ownership.
        while owner.is_some() {
            owner = self
                .available
                .wait(owner)
                .unwrap_or_else(|e| e.into_inner());
        }
        *owner = Some(IDLE_OWNER);
    }

    fn try_acquire_idle(&self) -> bool {
        let mut owner = match self.owner.try_lock() {
            Ok(owner) => owner,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        if owner.is_some() {
            return false;
        }
        *owner = Some(IDLE_OWNER);
        true
    }

    fn release(&self, task: usize) {
        self.release_with(task, || {});
    }

    fn release_with(&self, task: usize, before: impl FnOnce()) {
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        if *owner == Some(task) {
            before();
            *owner = None;
            self.available.notify_one();
        }
    }
}

/// Role ownership for an idle callback, independent of any task context.
/// Its Drop may run after the callback vacates a completed task's row/stack.
/// No task identity or scheduler.current is installed by this guard.
pub(crate) struct IdleGuard {
    run: *mut RunShared,
    // A guard must stay on its idle thread and never cross a fiber switch.
    _local: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl IdleGuard {
    /// Nonblocking idle claim for the home loop, which must keep serving a
    /// worker's synchronous bridge while that worker owns the role.
    /// # Safety
    /// As for new; a failed claim acquires no runtime ownership.
    pub(crate) unsafe fn try_new(run: *mut RunShared) -> Option<Self> {
        let role = unsafe { &(*run).role };
        let enabled = role.enabled.load(Ordering::Acquire);
        if enabled && !role.try_acquire_idle() {
            return None;
        }
        Some(Self {
            run: if enabled { run } else { std::ptr::null_mut() },
            _local: std::marker::PhantomData,
        })
    }

    /// # Safety
    /// The caller is on an idle stack and holds no role or runtime borrow.
    /// `run` outlives the guard. Enabled callbacks must raw-project role-owned
    /// fields and cannot re-enter through scheduler(ctx), release the role,
    /// call a home bridge, or switch fibers while this guard is live.
    pub(crate) unsafe fn new(run: *mut RunShared) -> Self {
        // SAFETY: caller retains the run for the callback's entire lifetime.
        let role = unsafe { &(*run).role };
        let enabled = role.enabled.load(Ordering::Acquire);
        if enabled {
            role.acquire_idle();
        }
        Self {
            run: if enabled { run } else { std::ptr::null_mut() },
            _local: std::marker::PhantomData,
        }
    }
}

impl Drop for IdleGuard {
    fn drop(&mut self) {
        if !self.run.is_null() {
            // SAFETY: the callback retains RunShared, even if its task is gone.
            unsafe { (*self.run).role.release(IDLE_OWNER) };
        }
    }
}

/// Enable scheduler-role guards for a concurrent driver. Admission alone may
/// call this, after installing guarded dispatch and before starting any worker.
///
/// # Safety
/// `ctx` is the live root context before execution. The driver must obey the
/// documented enter/release and saved-fiber publication contracts.
pub unsafe fn enable(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    unsafe { (*(*ctx).run_ptr()).role.enable() };
}

/// Claim a suspended task by id, before even borrowing the moving task table.
/// The temporary owner is replaced under the same predicate lock, without a
/// gap in which another worker can mutate the table.
///
/// # Safety
/// The run and task are live, suspended, and claimed by exactly one worker.
pub(crate) unsafe fn claim(run: *mut crate::context::RunShared, task: u32) -> *mut TaskCtx {
    // SAFETY: caller contract.
    unsafe {
        let role = &(*run).role;
        let claim = ((task as usize) << 1) | 1;
        if role.enabled.load(Ordering::Acquire) {
            role.acquire(claim);
        }
        let ctx = (*(*run).tasks.get())
            .as_deref_mut()
            .expect("a run with tasks")
            .task(task)
            .ctx;
        if role.enabled.load(Ordering::Acquire) {
            let mut owner = role.owner.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(*owner, Some(claim));
            *owner = Some(ctx.addr());
        }
        ctx
    }
}

/// Re-enter a suspended runtime continuation without adding another semantic
/// boundary. Setting current is protected by role ownership, not a TLS cache.
///
/// # Safety
/// `ctx` belongs to the running fiber; no scheduler borrow is retained while
/// acquiring. The run outlives every caller.
pub(crate) unsafe fn enter(ctx: *mut TaskCtx) {
    // SAFETY: caller contract; these immutable fields outlive every worker.
    let role = unsafe { &(*(*ctx).run_ptr()).role };
    if !role.enabled.load(Ordering::Acquire) {
        return;
    }
    role.acquire(ctx.addr());
    // SAFETY: the role exclusively owns the scheduler. This intentionally
    // avoids scheduler(), which itself checks the running task's stop word.
    unsafe {
        if let Some(scheduler) = (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() {
            scheduler.current = (*ctx).task_id;
            super::snapshot_shield(scheduler, ctx);
        }
    }
}

/// Leave before a segment, foreign call, host wait or fiber switch. The
/// continuation re-enters before touching the scheduler again. Nested guards
/// stay on their own task's stack; none owns a thread's lock guard.
///
/// # Safety
/// `ctx` is live. No scheduler/object borrow survives this call.
pub(crate) unsafe fn release(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    let role = unsafe { &(*(*ctx).run_ptr()).role };
    if role.enabled.load(Ordering::Acquire) {
        role.release_with(ctx.addr(), || {
            // SAFETY: release_with still owns the role. Completed tasks may
            // already have vacated their row, so snapshot only a matching row.
            unsafe {
                if let Some(scheduler) = (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() {
                    super::snapshot_shield(scheduler, ctx);
                }
            }
        });
    }
}

/// One semantic runtime entry. Nested implementation calls do not leave the
/// role until the outermost entry returns to compiled code.
pub(crate) struct Guard(*mut TaskCtx);

impl Guard {
    /// # Safety
    /// As for enter. The guard stays on this task's own stack.
    pub(crate) unsafe fn new(ctx: *mut TaskCtx) -> Self {
        // SAFETY: caller contract.
        unsafe {
            if !(*(*ctx).run_ptr()).role.enabled.load(Ordering::Acquire) {
                return Self(std::ptr::null_mut());
            }
            enter(ctx);
            (*ctx).role_depth = (*ctx)
                .role_depth
                .checked_add(1)
                .expect("runtime entry depth");
        }
        Self(ctx)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: this guard's task is running again, on whichever worker.
        unsafe {
            (*self.0).role_depth -= 1;
            if (*self.0).role_depth == 0 {
                release(self.0);
            }
        }
    }
}

/// Runtime calls compiled user code (a finalizer, callback, scope body, or
/// task entry). That code's operations have independent guards, even while
/// its outer runtime continuation remains on the same fiber's stack.
pub(crate) struct Segment {
    ctx: *mut TaskCtx,
    depth: u32,
}

impl Segment {
    /// # Safety
    /// The caller holds no scheduler borrow across the compiled code call.
    pub(crate) unsafe fn new(ctx: *mut TaskCtx) -> Self {
        // SAFETY: caller contract.
        unsafe {
            let depth = std::mem::take(&mut (*ctx).role_depth);
            release(ctx);
            Self { ctx, depth }
        }
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        // SAFETY: the user call returned on this task's fiber.
        unsafe {
            assert_eq!(
                (*self.ctx).role_depth,
                0,
                "user code left a runtime entry open"
            );
            if self.depth != 0 {
                enter(self.ctx);
            }
            (*self.ctx).role_depth = self.depth;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn home_idle_claim_never_waits_for_a_worker_bridge_or_another_idle_callback() {
        let role = Role::default();
        role.enable();
        role.acquire(8);
        assert!(!role.try_acquire_idle());
        role.release(8);
        assert!(role.try_acquire_idle());
        assert!(!role.try_acquire_idle());
        role.release(IDLE_OWNER);
        assert!(role.try_acquire_idle());
        role.release(IDLE_OWNER);
    }

    struct Run(*mut TaskCtx);
    impl Run {
        fn new(enabled: bool) -> Self {
            let mut ctx = std::ptr::null_mut();
            // SAFETY: fresh context, no workers started.
            unsafe {
                assert_eq!(
                    crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                    0
                );
                if enabled {
                    enable(ctx);
                }
            }
            Self(ctx)
        }
        fn shared(&self) -> *mut RunShared {
            // SAFETY: test retains the context until every scoped thread joins.
            unsafe { (*self.0).run_ptr() }
        }
    }
    impl Drop for Run {
        fn drop(&mut self) {
            // SAFETY: the test has joined all workers and released all guards.
            unsafe { crate::context::hirsel_rt_v2_ctx_destroy(self.0) };
        }
    }

    #[test]
    fn idle_callbacks_exclude_each_other_and_task_owners() {
        let run = Run::new(true);
        let address = run.shared().addr();
        let inside = std::sync::atomic::AtomicUsize::new(0);
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|threads| {
            for worker in 0..8 {
                let (inside, start) = (&inside, &start);
                threads.spawn(move || {
                    let shared = address as *mut RunShared;
                    start.wait();
                    for _ in 0..1000 {
                        // SAFETY: the scoped test retains the run; these idle
                        // threads hold no ctx, service borrow, or other guard.
                        let idle = if worker < 4 {
                            Some(unsafe { IdleGuard::new(shared) })
                        } else {
                            unsafe { (*shared).role.acquire((worker + 1) * 2) };
                            None
                        };
                        assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0);
                        std::thread::yield_now();
                        assert_eq!(inside.fetch_sub(1, Ordering::SeqCst), 1);
                        if idle.is_none() {
                            unsafe { (*shared).role.release((worker + 1) * 2) };
                        }
                        drop(idle);
                    }
                });
            }
        });
        assert_eq!(unsafe { *(*run.shared()).role.owner.lock().unwrap() }, None);
    }

    #[test]
    fn idle_guard_can_drop_after_its_callback_vacates_task_metadata() {
        let run = Run::new(true);
        super::super::scheduler(run.0);
        unsafe { release(run.0) };
        let guard = unsafe { IdleGuard::new(run.shared()) };
        // No ctx-dependent guard may remain when completion vacates a row.
        unsafe {
            (*(*run.shared()).tasks.get())
                .as_deref_mut()
                .unwrap()
                .tasks
                .clear();
        }
        drop(guard);
        assert_eq!(unsafe { *(*run.shared()).role.owner.lock().unwrap() }, None);
    }

    #[test]
    fn finalizer_shield_is_published_with_and_without_role_guards() {
        for enabled in [false, true] {
            let run = Run::new(enabled);
            let s = super::super::scheduler(run.0);
            s.cancel(0, super::super::CANCEL_REQUESTED);
            unsafe { release(run.0) };
            assert_eq!(super::super::task_canceled(run.0, 0), Some(0));
            unsafe {
                crate::finalize::hirsel_rt_v2_finalize_begin(run.0, 0);
                assert_eq!(super::super::task_canceled(run.0, 0), None);
                crate::finalize::hirsel_rt_v2_finalize_begin(run.0, 0);
                crate::finalize::hirsel_rt_v2_finalize_end(run.0, 0);
                assert_eq!(super::super::task_canceled(run.0, 0), None);
                crate::finalize::hirsel_rt_v2_finalize_end(run.0, 0);
                assert_eq!(super::super::task_canceled(run.0, 0), Some(0));
                release(run.0);
            }
        }
    }

    #[test]
    fn remote_shield_reads_use_the_snapshot_while_local_state_changes() {
        let run = Run::new(true);
        let s = super::super::scheduler(run.0);
        s.cancel(0, super::super::CANCEL_REQUESTED);
        unsafe {
            (*run.0).shield = 1;
            release(run.0);
            // Stand in for an owner's task-local entry with exclusive ctx
            // access. Remote metadata must retain the published value.
            (*run.0).shield = 0;
            let _idle = IdleGuard::new(run.shared());
            let s = (*(*run.shared()).tasks.get()).as_deref_mut().unwrap();
            assert_eq!(s.task(0).shield(), 1);
            assert_eq!(
                s.canceled_task(0, s.tasks[0].as_ref().unwrap().shield()),
                None
            );
        }
    }

    #[test]
    fn owner_predicate_serializes_threads_and_reenters_for_the_same_task() {
        let role = Arc::new(Role::default());
        role.enable();
        let in_role = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::scope(|threads| {
            for task in 1..=8 {
                let role = Arc::clone(&role);
                let in_role = Arc::clone(&in_role);
                threads.spawn(move || {
                    for _ in 0..1000 {
                        role.acquire(task);
                        role.acquire(task);
                        assert_eq!(in_role.fetch_add(1, Ordering::SeqCst), 0);
                        std::thread::yield_now();
                        assert_eq!(in_role.fetch_sub(1, Ordering::SeqCst), 1);
                        role.release(task);
                    }
                });
            }
        });
        assert_eq!(*role.owner.lock().unwrap(), None);
    }
}
