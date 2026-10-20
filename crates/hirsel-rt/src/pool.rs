//! The threads a run's foreign intervals run on (S-13, design §6 Extern dispatch): the home
//! thread, which admitted the run, and a bounded pool of blocking threads, each with a
//! [`Worker`] and heap of its own.
//!
//! - **Ownership.** A task inside a foreign interval on a pool thread is owned by that thread
//!   (`Task::owner`): it runs only there, from the dispatch until the interval's leave. Every
//!   other task belongs to the home thread.
//! - **Handoff.** A thread hands the run to another by posting the task to run next to that
//!   thread's [`Exec`] and switching to its own idle fiber, which waits for a post addressed to
//!   it. The post happens on the idle fiber, after the switch saved the running fiber's stack
//!   pointer, so no thread resumes a fiber another is still leaving. At width 1 exactly one
//!   thread runs the run's code at a time, so a run's observations do not depend on which
//!   thread ran it: the schedule stays the cooperative scheduler's, and a pool dispatch
//!   occupies the run as an inline call does (extern calls are head-only in ordered mode).
//! - **Permits.** A thread holds one of the run's compute permits while it runs Hirsel code
//!   ([`Permits`]): it gives it back when it goes idle and when a task it runs enters C on it,
//!   and takes one before it resumes a task into Hirsel code and at a callback's entry. A
//!   blocking call holds none while it is in C.
//! - **Lifetime.** Pool threads start on first use and stop when the run ends
//!   ([`shutdown`]), after the run's last task stopped; their heaps stay in the run's registry.
//!
//! Lane W1 adds compute workers beside these threads; the handoff is the seam it widens.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::context::{RunShared, TaskCtx, Worker};
use crate::fiber::{self, Stack};
use crate::{environmental, fault};

/// The most blocking-pool threads one run starts.
pub(crate) const POOL_THREADS: usize = 64;

/// What a thread is asked to do next.
#[derive(Clone, Copy, Debug)]
enum Post {
    /// Resume task `task`, taking a compute permit first when `permit` (a task resumed into C
    /// takes none).
    Run { task: u32, permit: bool },
    /// The run ended: leave.
    Exit,
}

/// One thread that runs a run's code: home or a pool thread.
pub(crate) struct Exec {
    inbox: Mutex<Option<(Post, Option<Instant>)>>,
    bell: Condvar,
    /// The pool slot, or `None` for home.
    pub(crate) slot: Option<u16>,
    /// Touched only by the exec's own thread: its idle fiber's stack pointer, the post its
    /// idle fiber makes on regaining control, its worker and whether it holds a permit.
    idle: Cell<*mut u8>,
    deferred: Cell<Option<(*const Exec, Post)>>,
    worker: Cell<*mut Worker>,
    held: Cell<bool>,
}

// SAFETY: the cells are touched only by the exec's own thread (see `Exec`); the inbox is
// behind its lock.
unsafe impl Send for Exec {}
// SAFETY: as for `Send`.
unsafe impl Sync for Exec {}

impl Exec {
    fn new(slot: Option<u16>, worker: *mut Worker, held: bool) -> Arc<Exec> {
        Arc::new(Exec {
            inbox: Mutex::new(None),
            bell: Condvar::new(),
            slot,
            idle: Cell::new(std::ptr::null_mut()),
            deferred: Cell::new(None),
            worker: Cell::new(worker),
            held: Cell::new(held),
        })
    }

    fn post(&self, post: Post, queued: Option<Instant>) {
        let mut inbox = self.inbox.lock().unwrap();
        if inbox.is_some() {
            fault("a thread was handed two tasks at once");
        }
        *inbox = Some((post, queued));
        self.bell.notify_one();
    }

    fn take(&self) -> (Post, Option<Instant>) {
        let mut inbox = self.inbox.lock().unwrap();
        loop {
            if let Some(post) = inbox.take() {
                return post;
            }
            inbox = self.bell.wait(inbox).unwrap();
        }
    }
}

/// A run's compute permits: as many as its width (1 until lane W1).
pub(crate) struct Permits {
    available: Mutex<u32>,
    freed: Condvar,
}

impl Permits {
    /// Take a permit for `exec`'s thread unless it holds one: whether it took one.
    fn acquire(&self, exec: &Exec) -> bool {
        if exec.held.get() {
            return false;
        }
        let mut available = self.available.lock().unwrap();
        while *available == 0 {
            available = self.freed.wait(available).unwrap();
        }
        *available -= 1;
        exec.held.set(true);
        true
    }
    fn release(&self, exec: &Exec) {
        if !exec.held.replace(false) {
            return;
        }
        *self.available.lock().unwrap() += 1;
        self.freed.notify_one();
    }
    /// Permits no thread holds.
    pub(crate) fn available(&self) -> u32 {
        *self.available.lock().unwrap()
    }
}

struct PoolThread {
    exec: Arc<Exec>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// A task's interval is on it.
    busy: bool,
}

/// A run's threads beyond the one that admitted it, created at its first pool dispatch.
pub(crate) struct Execs {
    home: Arc<Exec>,
    /// The home thread's idle fiber, prepared when home first waits.
    home_idle: Option<Stack>,
    pool: Vec<PoolThread>,
    pub(crate) permits: Arc<Permits>,
}

/// The extern row's headroom counters (design §5 Headroom, B1's `extern` row in
/// `bench/multicore/manifest.json`). A run holds them only when metrics are enabled: every
/// collection point is behind an `Option<Arc<ExternMetrics>>`, so a run without metrics pays
/// one branch and reads no clock.
#[derive(Debug, Default)]
pub struct ExternMetrics {
    calls: AtomicU64,
    extern_ns: AtomicU64,
    dispatches: AtomicU64,
    pool_queue_ns: AtomicU64,
    gate_entries: AtomicU64,
    gate_wait_ns: AtomicU64,
}

fn nanos(waited: Duration) -> u64 {
    u64::try_from(waited.as_nanos()).unwrap_or(u64::MAX)
}

impl ExternMetrics {
    pub(crate) fn call(&self, took: Duration) {
        self.calls.fetch_add(1, Relaxed);
        self.extern_ns.fetch_add(nanos(took), Relaxed);
    }
    fn queued(&self, waited: Duration) {
        self.dispatches.fetch_add(1, Relaxed);
        self.pool_queue_ns.fetch_add(nanos(waited), Relaxed);
    }
    /// An outermost `serial` gate entry, which waited `waited`.
    pub(crate) fn gate_entry(&self, waited: Duration) {
        self.gate_entries.fetch_add(1, Relaxed);
        self.gate_wait_ns.fetch_add(nanos(waited), Relaxed);
    }
    /// Blocking calls dispatched to the pool so far.
    pub fn dispatches(&self) -> u64 {
        self.dispatches.load(Relaxed)
    }
    /// The row's metrics by their names in `bench/multicore/telemetry.json`: `extern_ns`, the
    /// mean time of an extern call from its dispatch to its return to the caller's thread;
    /// `pool_queue_ns`, the mean time a blocking call's post waited for its pool thread to
    /// take it; `gate_wait_ns`, the mean time an outermost `serial` gate entry waited. A
    /// metric with nothing counted is zero.
    pub fn values(&self) -> [(&'static str, f64); 3] {
        let mean = |total: &AtomicU64, count: &AtomicU64| match count.load(Relaxed) {
            0 => 0.0,
            count => total.load(Relaxed) as f64 / count as f64,
        };
        [
            ("extern_ns", mean(&self.extern_ns, &self.calls)),
            ("pool_queue_ns", mean(&self.pool_queue_ns, &self.dispatches)),
            ("gate_wait_ns", mean(&self.gate_wait_ns, &self.gate_entries)),
        ]
    }
}

thread_local! {
    /// The pool thread's exec; null on every other thread.
    static EXEC: Cell<*const Exec> = const { Cell::new(std::ptr::null()) };
}

/// The calling thread's pool exec, or null on home. Out of line, like every thread-local
/// access, so no caller keeps a thread's address across a suspension.
#[inline(never)]
fn pool_exec() -> *const Exec {
    EXEC.with(Cell::get)
}

/// Whether the calling thread is one of a run's blocking-pool threads.
#[inline(never)]
pub(crate) fn on_pool() -> bool {
    !pool_exec().is_null()
}

/// The run's execs, created with the home thread holding the run's one permit.
fn execs(ctx: *mut TaskCtx) -> &'static mut Execs {
    // SAFETY: the context and its run are live for the whole run; the execs are reached by the
    // thread that runs the run's code, one at a time.
    unsafe {
        let run = (*ctx).run_ptr();
        (*run).execs.get_or_insert_with(|| {
            let home = if on_pool() {
                fault("a run's first dispatch came from a pool thread");
            } else {
                Exec::new(None, (*ctx).worker_ptr(), true)
            };
            Box::new(Execs {
                home,
                home_idle: None,
                pool: Vec::new(),
                permits: Arc::new(Permits {
                    available: Mutex::new(0),
                    freed: Condvar::new(),
                }),
            })
        })
    }
}

/// The exec of the calling thread.
fn current(execs: &Execs) -> *const Exec {
    let exec = pool_exec();
    if exec.is_null() {
        Arc::as_ptr(&execs.home)
    } else {
        exec
    }
}

/// The exec that runs task rows owned by `owner` (home for `None`).
fn exec_of(execs: &Execs, owner: Option<u16>) -> *const Exec {
    match owner {
        None => Arc::as_ptr(&execs.home),
        Some(slot) => Arc::as_ptr(&execs.pool[slot as usize].exec),
    }
}

/// Whether a switch to a task owned by `owner` is a handoff to another thread. False before
/// the run's first pool dispatch: every task is home's.
pub(crate) fn crosses(ctx: *mut TaskCtx, owner: Option<u16>) -> bool {
    // SAFETY: the context is live.
    let Some(execs) = (unsafe { (*(*ctx).run_ptr()).execs.as_deref() }) else {
        return false;
    };
    current(execs) != exec_of(execs, owner)
}

/// Suspend the running fiber, saving its stack pointer at `save`, and hand the run to the
/// thread that owns task `next` (`owner`), which resumes it: the calling thread waits on its
/// idle fiber for its next post. `permit`: `next` resumes into Hirsel code.
///
/// # Safety
/// `ctx` is the running task's, already switched out; `save` stays writable until whoever
/// resumes this fiber reads it; `next` is suspended at its row's stack pointer.
pub(crate) unsafe fn hand_off(
    ctx: *mut TaskCtx,
    save: *mut *mut u8,
    owner: Option<u16>,
    next: u32,
    permit: bool,
) {
    let execs = execs(ctx);
    let me = current(execs);
    let to = exec_of(execs, owner);
    // SAFETY: `me` is this thread's exec, alive as long as the run's execs; `to` is idle, so
    // its worker's heap holds no task.
    unsafe {
        // The run's fuel travels with the run, so a budget charges every thread's code once.
        let (from, into) = ((*me).worker.get(), (*to).worker.get());
        if from != into {
            let fuel = (*from).heap().parked_fuel();
            (*into).heap().park_fuel(fuel);
        }
        (*me)
            .deferred
            .set(Some((to, Post::Run { task: next, permit })));
        if (*me).idle.get().is_null() {
            if (*me).slot.is_some() {
                fault("a pool thread without its idle fiber");
            }
            let stack = Stack::new();
            let sp = stack.prepare(home_idle, (*ctx).run_ptr().cast(), std::ptr::null_mut());
            execs.home_idle = Some(stack);
            (*me).idle.set(sp);
        }
        let idle = (*me).idle.get();
        fiber::switch(save, idle);
    }
}

/// Make the deferred post of `exec`'s thread, after its idle fiber regained control.
///
/// # Safety
/// `exec` is the calling thread's.
unsafe fn post_deferred(run: *mut RunShared, exec: &Exec) {
    let Some((to, post)) = exec.deferred.take() else {
        return;
    };
    // SAFETY: the run's execs outlive every post.
    unsafe {
        if let Some(execs) = (*run).execs.as_deref() {
            execs.permits.release(exec);
        }
        let queued = (*run).extern_metrics.as_ref().map(|_| Instant::now());
        (*to).post(post, queued);
    }
}

/// Resume task `task` on the calling thread's worker.
///
/// # Safety
/// `exec` is the calling thread's, idle; `task` is suspended at its row's stack pointer.
unsafe fn resume(run: *mut RunShared, exec: &Exec, task: u32, permit: bool) {
    // SAFETY: the run is live; its execs and tasks outlive every resume.
    unsafe {
        if permit && let Some(execs) = (*run).execs.as_deref() {
            execs.permits.acquire(exec);
        }
        let (ctx, sp) = crate::tasks::resume_row(run, task);
        TaskCtx::switch_in(ctx, exec.worker.get());
        let save = exec.idle.as_ptr();
        fiber::switch(save, sp);
    }
}

/// Wait for a post and serve it; false at the run's end.
///
/// # Safety
/// `exec` is the calling thread's, idle.
unsafe fn serve(run: *mut RunShared, exec: &Exec) -> bool {
    // SAFETY: caller contract.
    unsafe { post_deferred(run, exec) };
    let (post, queued) = exec.take();
    match post {
        Post::Exit => false,
        Post::Run { task, permit } => {
            // SAFETY: the run outlives its threads.
            if let (Some(queued), Some(metrics)) =
                (queued, unsafe { (*run).extern_metrics.as_ref() })
                && exec.slot.is_some()
                && !permit
            {
                metrics.queued(queued.elapsed());
            }
            // SAFETY: caller contract; the post names a suspended task.
            unsafe { resume(run, exec, task, permit) };
            true
        }
    }
}

/// The home thread's idle fiber: it serves posts for as long as the run lasts. A stop never
/// returns here, and the fiber is abandoned with the run.
unsafe extern "C" fn home_idle(run: *mut u8, _: *mut u8) -> ! {
    let run = run.cast::<RunShared>();
    loop {
        // SAFETY: the run is live while any of its fibers runs.
        let home = unsafe {
            Arc::clone(
                &(*run)
                    .execs
                    .as_deref()
                    .unwrap_or_else(|| fault("a home idle fiber without execs"))
                    .home,
            )
        };
        // SAFETY: this is the home thread, idle.
        if !unsafe { serve(run, &home) } {
            fault("the home thread was asked to exit");
        }
    }
}

/// A run pointer a pool thread carries.
struct RunPtr(*mut RunShared);
// SAFETY: the run outlives its pool threads (`shutdown` joins them), and each reaches it only
// while it holds the run.
unsafe impl Send for RunPtr {}

/// A pool thread: a worker of its own, then posts until the run ends.
fn pool_main(run: RunPtr, exec: Arc<Exec>) {
    let run = run.0;
    EXEC.with(|e| e.set(Arc::as_ptr(&exec)));
    let worker = exec.worker.get();
    // SAFETY: this is the exec's own thread, idle between posts.
    while unsafe { serve(run, &exec) } {}
    EXEC.with(|e| e.set(std::ptr::null()));
    // SAFETY: no task is bound to the worker any more.
    unsafe { Worker::retire(worker) };
}

/// A pool slot for an interval: an idle pool thread, a new one, or `None` when all
/// [`POOL_THREADS`] are busy.
pub(crate) fn claim(ctx: *mut TaskCtx) -> Option<u16> {
    let run = RunPtr(
        // SAFETY: the context is live.
        unsafe { (*ctx).run_ptr() },
    );
    let execs = execs(ctx);
    if let Some(slot) = execs.pool.iter().position(|thread| !thread.busy) {
        execs.pool[slot].busy = true;
        return Some(slot as u16);
    }
    if execs.pool.len() >= POOL_THREADS {
        return None;
    }
    let slot = execs.pool.len() as u16;
    // SAFETY: the run outlives the worker, which only the new thread uses from here on.
    let worker = unsafe { Worker::create(run.0) };
    let exec = Exec::new(Some(slot), worker, false);
    let thread = std::thread::Builder::new()
        .name(format!("hirsel-pool-{slot}"))
        .spawn({
            let exec = Arc::clone(&exec);
            move || pool_main(run, exec)
        })
        .unwrap_or_else(|_| environmental("a blocking-pool thread could not start"));
    execs.pool.push(PoolThread {
        exec,
        thread: Some(thread),
        busy: true,
    });
    Some(slot)
}

/// The interval on pool slot `slot` ended: the slot takes another.
pub(crate) fn release(ctx: *mut TaskCtx, slot: u16) {
    execs(ctx).pool[slot as usize].busy = false;
}

/// Whether some pool slot is free or can still start.
pub(crate) fn has_slot(ctx: *mut TaskCtx) -> bool {
    let execs = execs(ctx);
    execs.pool.len() < POOL_THREADS || execs.pool.iter().any(|thread| !thread.busy)
}

/// The calling thread's compute permit goes back: the task it runs enters C.
pub(crate) fn release_permit(ctx: *mut TaskCtx) {
    // SAFETY: the context is live.
    if let Some(execs) = unsafe { (*(*ctx).run_ptr()).execs.as_deref() } {
        // SAFETY: the current exec is this thread's.
        execs.permits.release(unsafe { &*current(execs) });
    }
}

/// The calling thread takes a compute permit unless it holds one, as a callback's Hirsel code
/// begins: whether it took one. Before the run's first dispatch its one thread holds the
/// run's one permit.
pub(crate) fn acquire_permit(ctx: *mut TaskCtx) -> bool {
    // SAFETY: the context is live.
    match unsafe { (*(*ctx).run_ptr()).execs.as_deref() } {
        // SAFETY: the current exec is this thread's.
        Some(execs) => execs.permits.acquire(unsafe { &*current(execs) }),
        None => false,
    }
}

/// The run's compute permits no thread holds, or `None` before its first dispatch.
pub(crate) fn available_permits(ctx: *mut TaskCtx) -> Option<u32> {
    // SAFETY: the context is live.
    unsafe { (*(*ctx).run_ptr()).execs.as_deref() }.map(|execs| execs.permits.available())
}

/// The run ended: every pool thread leaves (each is idle, since the run's code ran on the
/// thread ending it) and is joined.
pub(crate) fn shutdown(run: &mut RunShared) {
    let Some(mut execs) = run.execs.take() else {
        return;
    };
    for thread in &mut execs.pool {
        thread.exec.post(Post::Exit, None);
        if let Some(handle) = thread.thread.take()
            && handle.join().is_err()
        {
            fault("a blocking-pool thread panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extern_row_reports_means_and_zero_when_nothing_was_counted() {
        let metrics = ExternMetrics::default();
        assert_eq!(
            metrics.values(),
            [
                ("extern_ns", 0.0),
                ("pool_queue_ns", 0.0),
                ("gate_wait_ns", 0.0)
            ]
        );
        metrics.call(Duration::from_nanos(100));
        metrics.call(Duration::from_nanos(300));
        metrics.queued(Duration::from_nanos(50));
        metrics.gate_entry(Duration::ZERO);
        metrics.gate_entry(Duration::from_nanos(40));
        assert_eq!(
            metrics.values(),
            [
                ("extern_ns", 200.0),
                ("pool_queue_ns", 50.0),
                ("gate_wait_ns", 20.0)
            ]
        );
        assert_eq!(metrics.dispatches(), 1);
    }

    #[test]
    fn a_thread_holding_a_permit_takes_none_and_gives_back_only_what_it_holds() {
        let permits = Permits {
            available: Mutex::new(1),
            freed: Condvar::new(),
        };
        let exec = Exec::new(Some(0), std::ptr::null_mut(), false);
        assert!(permits.acquire(&exec));
        assert!(!permits.acquire(&exec));
        assert_eq!(permits.available(), 0);
        permits.release(&exec);
        permits.release(&exec);
        assert_eq!(permits.available(), 1);
    }
}
