//! Physical dispatch of saved segments, independent of their logical policy.
//!
//! Notification/Queued/Running use the Tokio task-harness ownership protocol
//! in hirsel-free::frame. Only an idle continuation publishes a started stack.
//! The scheduler role still owns task rows and operation effects. Admission
//! must explicitly enable this dispatcher; the W1 run-token path is unchanged.

use super::*;
use hirsel_free::frame::{self, Notification, Queued, Running};
use hirsel_free::work::{Ready, Worker as ReadyWorker};

struct Image {
    ctx: *mut TaskCtx,
    task: u32,
    sp: *mut u8,
}

// SAFETY: the image is accessed only by its exclusive Running owner. Queued
// transfers it after the idle stack saved sp. Notifications never inspect it.
unsafe impl Send for Image {}

pub(crate) struct Frame {
    notification: Notification<Image>,
    owner: AtomicUsize,
    saved: AtomicBool,
}

impl Frame {
    pub(crate) fn is_saved(&self) -> bool {
        self.saved.load(Ordering::Acquire)
    }

    /// The idle callback owns the scheduler role before exposing slots or
    /// completion. Otherwise a join could retire this row before its callback.
    pub(crate) fn mark_saved(&self) {
        self.saved.store(true, Ordering::Release);
    }
}

/// Whether the task's parked stack is available to the logical policy for
/// join delivery, receive slots, shield reads, and retirement. A row becoming
/// Parked or Holding alone is insufficient: its old worker may still be saving.
///
/// # Safety
/// The caller holds the scheduler role; the task row is live.
pub(crate) unsafe fn is_saved(run: *mut RunShared, task: u32) -> bool {
    let (slot, _) = unsafe { crate::tasks::dispatch_frame(run, task) };
    slot.as_ref()
        .is_none_or(|frame| frame.saved.load(Ordering::Acquire))
}

struct Count {
    live: AtomicU32,
    home: std::sync::Weak<Exec>,
}

struct Lease(Arc<Count>);
impl Lease {
    fn new(count: &Arc<Count>) -> Self {
        count.live.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.0.live.fetch_sub(1, Ordering::AcqRel) == 1
            && let Some(home) = self.0.home.upgrade()
        {
            wake(&home);
        }
    }
}

pub(super) struct Saved {
    queued: Queued<Image>,
    frame: Arc<Frame>,
    // Drop execution ownership before releasing the activity reservation.
    lease: Lease,
}

pub(super) struct Active {
    running: Running<Image>,
    frame: Arc<Frame>,
    lease: Lease,
}

pub(super) enum Leave {
    Suspend { owner: Owner, yielded: bool },
    Complete,
}

pub(super) struct Dispatch {
    ready: Arc<Ready<Saved>>,
    locals: Mutex<Vec<ReadyWorker<Saved>>>,
    count: Arc<Count>,
}

fn owner_word(owner: Owner) -> usize {
    match owner {
        Owner::Any => 0,
        Owner::Home => 1,
        Owner::Pool(slot) => (usize::from(slot) << 2) | 2,
        Owner::Compute(address) => {
            assert_eq!(address & 3, 0);
            address | 3
        }
    }
}
fn word_owner(owner: usize) -> Owner {
    match owner & 3 {
        0 => {
            assert_eq!(owner, 0);
            Owner::Any
        }
        1 => {
            assert_eq!(owner, 1);
            Owner::Home
        }
        2 => Owner::Pool((owner >> 2) as u16),
        3 => Owner::Compute(owner & !3),
        _ => unreachable!(),
    }
}

/// Admission-only opt-in. Logical policies must route every ready/park/end
/// through this module and obey saved-frame gating of parked stack slots.
///
/// # Safety
/// No task has started; topology is installed, on its admission thread. All
/// service lifetime and scheduler role contracts must hold before activation.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx) {
    let execs = execs(ctx);
    unsafe { crate::tasks::install_cancel_metrics(ctx) };
    let metrics = unsafe { (*ctx).run_ref() }
        .worker_metrics
        .as_ref()
        .map(|m| m.ready_metrics());
    let (ready, locals) = hirsel_free::work::workers(MAX_WIDTH, metrics);
    assert!(
        execs
            .dispatch
            .set(Dispatch {
                ready,
                locals: Mutex::new(locals),
                count: Arc::new(Count {
                    live: AtomicU32::new(0),
                    home: Arc::downgrade(&execs.home),
                }),
            })
            .is_ok(),
        "physical dispatch enabled twice"
    );
    // SAFETY: admission, before the first task starts.
    unsafe { crate::tasks::role::enable(ctx) };
    // Workers started before opt-in may still be waiting in W1's inbox-only
    // predicate. A real post moves each to the physical ready predicate.
    for exec in &execs.compute.lock().unwrap().active {
        exec.post(Post::Wake, None);
    }
}

pub(crate) fn enabled(run: &RunShared) -> bool {
    run.execs
        .as_deref()
        .is_some_and(|execs| execs.dispatch.get().is_some())
}

pub(super) fn pending(execs: &Execs) -> bool {
    execs
        .dispatch
        .get()
        .is_some_and(|dispatch| dispatch.count.live.load(Ordering::Acquire) != 0)
}

/// # Safety
/// Idle only, after notifications were invalidated and the context's values
/// were consumed. No pointer into this stack may be accessed again.
pub(crate) unsafe fn recycle_stack(stack: Stack) {
    let me = own_exec();
    assert!(!me.is_null(), "stack retirement requires its idle worker");
    // SAFETY: this worker belongs exclusively to this idle thread.
    unsafe { (*(*me).worker.get()).idle_stacks.put(stack) };
}

/// Lend the running physical image to an immediate child's stable context.
/// The parent's continuation remains below it on the same native stack. A
/// child may save/migrate that stack; its return restores the parent's image.
/// # Safety
/// The caller owns the role and current image; neither row is published elsewhere.
pub(crate) unsafe fn lend(ctx: *mut TaskCtx, task: u32) {
    unsafe {
        let me = own_exec();
        assert!(!me.is_null());
        let mut active = (*me).active.take().expect("running image to lend");
        let image = active.running.value_mut();
        image.ctx = ctx;
        image.task = task;
        (*me).active.set(Some(active));
    }
}

/// Notify a task selected by the logical policy. A running notification
/// never publishes: its idle continuation does that after saving the stack.
///
/// # Safety
/// The caller holds the run's scheduler role (or is admitting its root). The
/// task row/context is live; its stack stays allocated until completion.
pub(crate) unsafe fn schedule_saved(run: *mut RunShared, task: u32, owner: Owner) {
    // SAFETY: the role owns this task lookup. No borrow survives publication.
    let (slot, ctx) = unsafe { crate::tasks::dispatch_frame(run, task) };
    let execs = unsafe { (*(*run).execs.get()).as_deref().expect("physical dispatch") };
    let dispatch = execs.dispatch.get().expect("physical dispatch enabled");
    let saved = unsafe {
        if let Some(frame) = slot {
            frame.owner.store(owner_word(owner), Ordering::Release);
            frame.notification.notify().map(|queued| Saved {
                queued,
                frame: frame.clone(),
                lease: Lease::new(&dispatch.count),
            })
        } else {
            let (notification, queued) = frame::prepared(Image {
                ctx,
                task,
                sp: (*ctx).task_sp,
            });
            assert!(!(*ctx).task_sp.is_null(), "a prepared task has a stack");
            let frame = Arc::new(Frame {
                notification,
                owner: AtomicUsize::new(owner_word(owner)),
                saved: AtomicBool::new(true),
            });
            *slot = Some(frame.clone());
            Some(Saved {
                queued,
                frame,
                lease: Lease::new(&dispatch.count),
            })
        }
    };
    if let Some(saved) = saved {
        // A notifier can keep executing indefinitely after this op. Work
        // cannot remain in its private LIFO slot until its next idle turn.
        publish(execs, saved, false, false);
    }
}

fn publish(execs: &Execs, saved: Saved, yielded: bool, idle: bool) {
    let dispatch = execs.dispatch.get().expect("physical dispatch enabled");
    match word_owner(saved.frame.owner.load(Ordering::Acquire)) {
        Owner::Any => {
            if home_computes(execs) {
                execs.home.post(Post::Saved(saved), None);
                return;
            }
            let me = own_exec();
            // SAFETY: an exec's local state belongs only to its OS thread.
            // Tokio's LIFO slot is private until the current poll returns.
            // A Hirsel pure segment need not return, so only an idle
            // continuation may use the slot: it searches again immediately.
            // Home may retain a local after a width-one interval. Once
            // wider, it serves host work rather than searching that local.
            let local =
                if idle && !me.is_null() && matches!(unsafe { (*me).kind }, Kind::Compute(_)) {
                    unsafe { (*me).ready.take() }
                } else {
                    None
                };
            if let Some(mut local) = local {
                local.push(&dispatch.ready, saved, yielded);
                unsafe { (*me).ready.set(Some(local)) };
            } else {
                dispatch.ready.inject([saved]);
            }
            wake_ready(execs);
        }
        owner => {
            let to = match owner {
                Owner::Home => Arc::as_ptr(&execs.home),
                Owner::Pool(slot) => pool_exec(execs, slot),
                Owner::Compute(address) => std::ptr::with_exposed_provenance(address),
                Owner::Any => unreachable!(),
            };
            // SAFETY: topology retains every destination until shutdown.
            unsafe { (*to).post(Post::Saved(saved), None) };
        }
    }
}

fn wake_ready(execs: &Execs) {
    let compute = execs.compute.lock().unwrap();
    let width = compute.active.len();
    if width == 0 {
        return;
    }
    let first = execs.cursor.fetch_add(1, Relaxed);
    for offset in 0..width {
        let exec = &compute.active[(first + offset) % width];
        if !exec.busy() {
            if exec.kind == Kind::Home {
                // Home also services host posts; its inbox predicate needs
                // an actual post, rather than a compute-search epoch alone.
                exec.post(Post::Wake, None);
                return;
            }
            // Publish before locking the sleep predicate: a search that missed
            // this queue either sees the epoch change or receives the bell.
            let _inbox = exec.inbox.lock().unwrap();
            exec.ready_epoch.fetch_add(1, Ordering::Release);
            exec.bell.notify_one();
            return;
        }
    }
}

/// Suspend a segment. The continuation returns with the scheduler role;
/// runtime/user callers must release it at their usual semantic boundary.
///
/// # Safety
/// The caller is the running task, holding the role. No scheduler, object,
/// worker, or run borrow may survive the switch. Its stack stays allocated.
pub(crate) unsafe fn suspend_current(ctx: *mut TaskCtx, yielded: bool) {
    let owner = crate::tasks::dispatch_owner(ctx);
    // SAFETY: caller contract; only this thread touches its Exec cells.
    unsafe {
        (*ctx).switch_out();
        let execs = execs(ctx);
        let me = current(execs);
        assert!(
            (*me)
                .leaving
                .replace(Some(Leave::Suspend { owner, yielded }))
                .is_none()
        );
        let idle = idle_of(ctx, execs, me);
        crate::tasks::role::release(ctx);
        fiber::switch(&raw mut (*ctx).task_sp, idle);
        crate::tasks::role::enter(ctx);
    }
}

/// Abandon completed execution ownership. The idle continuation invalidates
/// notifications before releasing its activity reservation or reusing a stack.
///
/// # Safety
/// As for suspend_current; no task-owned Rust value requiring Drop may remain
/// on this abandoned stack. The caller settled its logical completion.
pub(crate) unsafe fn finish_current(ctx: *mut TaskCtx) -> ! {
    unsafe {
        (*ctx).switch_out();
        let execs = execs(ctx);
        let me = current(execs);
        assert!((*me).leaving.replace(Some(Leave::Complete)).is_none());
        let idle = idle_of(ctx, execs, me);
        crate::tasks::role::release(ctx);
        let mut abandoned = std::ptr::null_mut();
        fiber::switch(&raw mut abandoned, idle);
    }
    fault("a completed physical segment was resumed")
}

/// Idle only: the switch has saved task_sp before this becomes reachable.
pub(super) unsafe fn saved(run: *mut RunShared, exec: &Exec) {
    let Some(leaving) = exec.leaving.take() else {
        return;
    };
    let Active {
        mut running,
        frame,
        lease,
    } = exec.active.take().expect("running physical frame");
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    execs.permits.release(exec);
    crate::stop::set_running(std::ptr::null_mut());
    crate::foreign::restore(crate::foreign::ForeignScope::NONE);
    match leaving {
        Leave::Complete => {
            let task = running.value().task;
            drop(running);
            // Keep busy while acquiring the role: a stop must retain this
            // worker's run, and no activity lock may precede the role lock.
            unsafe { crate::tasks::after_save(run, task, true) };
            let _activity = execs.activity.lock().unwrap();
            exec.busy.store(false, Ordering::SeqCst);
            drop(lease);
        }
        Leave::Suspend { owner, yielded } => {
            let image = running.value_mut();
            image.sp = unsafe { (*image.ctx).task_sp };
            frame.owner.store(owner_word(owner), Ordering::Release);
            let task = image.task;
            unsafe { crate::tasks::after_save(run, task, false) };
            let queued = running.save(yielded);
            let _activity = execs.activity.lock().unwrap();
            exec.busy.store(false, Ordering::SeqCst);
            if let Some(queued) = queued {
                publish(
                    execs,
                    Saved {
                        queued,
                        frame,
                        lease,
                    },
                    yielded,
                    true,
                );
            }
        }
    }
}

pub(super) fn next(execs: &Execs, exec: &Exec) -> Option<Saved> {
    if exec.kind == Kind::Home && !home_computes(execs) {
        // The idle owner releases its old width-one queue after growth.
        // Resize cannot touch this cell from another OS thread.
        retire(execs, exec);
        return None;
    }
    if !matches!(exec.kind, Kind::Compute(_) | Kind::Home) {
        return None;
    }
    let dispatch = execs.dispatch.get()?;
    let mut local = exec
        .ready
        .take()
        .or_else(|| dispatch.locals.lock().unwrap().pop())?;
    let saved = local.next(&dispatch.ready);
    exec.ready.set(Some(local));
    if saved.is_some() {
        // An injection/steal can leave more work behind while the selected
        // segment never yields again. Mark this claimant before choosing a
        // sleeper, so a burst of notifications cannot keep waking it instead
        // of a worker that can start the remaining work. Tokio similarly
        // notifies after searching (worker.rs:647-651,1207-1226 at ce685ee6).
        exec.busy.store(true, Ordering::SeqCst);
        wake_ready(execs);
    }
    saved
}

pub(super) fn retire(execs: &Execs, exec: &Exec) {
    if let Some(mut local) = exec.ready.take() {
        let dispatch = execs.dispatch.get().unwrap();
        local.drain(&dispatch.ready);
        dispatch.locals.lock().unwrap().push(local);
        wake_ready(execs);
    }
}

pub(super) unsafe fn resume(run: *mut RunShared, exec: &Exec, saved: Saved) {
    let Saved {
        queued,
        frame,
        lease,
    } = saved;
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    if exec.kind == Kind::Home
        && word_owner(frame.owner.load(Ordering::Acquire)) == Owner::Any
        && !home_computes(execs)
    {
        publish(
            execs,
            Saved {
                queued,
                frame,
                lease,
            },
            false,
            false,
        );
        return;
    }
    let running = queued.start();
    let image = running.value();
    let (ctx, task, sp) = (image.ctx, image.task, image.sp);
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    exec.busy.store(true, Ordering::SeqCst);
    if unsafe { (*run).control.stopped().is_some() } {
        drop(running);
        drop(lease);
        exec.busy.store(false, Ordering::SeqCst);
        wake(&execs.home);
        return;
    }
    // The exclusive saved image includes callback depth: Home/Pool C entry
    // needs no permit, but a callback continuation must reacquire its own.
    let permit = super::resumes_into_hirsel(ctx, exec);
    if permit
        && execs
            .permits
            .acquire(exec, Some(unsafe { &(*run).control }), unsafe {
                (*run).worker_metrics.as_deref()
            })
            .is_err()
    {
        drop(running);
        drop(lease);
        exec.busy.store(false, Ordering::SeqCst);
        wake(&execs.home);
        return;
    }
    // SAFETY: token ownership precedes every access to this saved task. Role
    // acquisition protects moving metadata; the continuation acquires it again.
    unsafe {
        crate::tasks::role::enter(ctx);
        debug_assert_eq!((*ctx).task_id, task);
        crate::tasks::record_dispatch(run, task, compute_index());
        frame.saved.store(false, Ordering::Release);
        TaskCtx::switch_in(ctx, exec.worker.get());
        crate::tasks::role::release(ctx);
        assert!(
            exec.active
                .replace(Some(Active {
                    running,
                    frame,
                    lease
                }))
                .is_none()
        );
        fiber::switch(exec.idle.as_ptr(), sp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_wake_waits_for_idle_save_and_the_root_drains_every_dispatch() {
        let mut run = crate::host::BodyRun::new(8, false);
        // SAFETY: this body uses only the explicit suspend/wake seams below;
        // it never borrows another task's stack or a home-owned service.
        unsafe { run.use_physical_dispatch() };
        let count = Arc::new(AtomicU32::new(0));
        let output = count.clone();
        let outcome = run.run(
            |_| {},
            move |ctx| {
                for _ in 0..1024 {
                    // These stand in for a logical frontier selecting this same
                    // segment while it is still executing. The saved-image CAS
                    // must remember, rather than publish, that notification.
                    unsafe {
                        crate::tasks::role::enter(ctx);
                        assert!(!is_saved((*ctx).run_ptr(), (*ctx).task_id));
                        schedule_saved((*ctx).run_ptr(), (*ctx).task_id, Owner::Any);
                        assert!(!is_saved((*ctx).run_ptr(), (*ctx).task_id));
                        suspend_current(ctx, false);
                        assert!(!is_saved((*ctx).run_ptr(), (*ctx).task_id));
                        crate::tasks::role::release(ctx);
                    }
                    output.fetch_add(1, Relaxed);
                }
                crate::STATUS_OK
            },
        );
        assert!(matches!(outcome.ended, Ok(crate::STATUS_OK)), "{outcome:?}");
        assert_eq!(outcome.pending, 0);
        assert_eq!(count.load(Relaxed), 1024);
    }
}
