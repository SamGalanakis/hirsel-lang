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

use crate::TaskCtx;

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

    fn release(&self, task: usize) {
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        if *owner == Some(task) {
            *owner = None;
            self.available.notify_one();
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
        role.release(ctx.addr());
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
