//! The threads a run's code runs on (S-13, design §6 Context, Extern dispatch, Stops and
//! worker lifetime): the home thread, which admitted the run; at width > 1 the run's compute
//! workers; and a bounded pool of blocking threads. Each runs tasks on a [`Worker`] and heap of
//! its own (home on the run's first worker).
//!
//! - **Ownership.** Every task has an [`Owner`]: a task inside a foreign interval on a pool
//!   thread is owned by that thread from the dispatch until the interval's leave; a task in a
//!   home foreign interval, or holding the run's Io binding, is owned by the home thread; every
//!   other task runs on any compute worker. At width 1 the home thread is the run's one
//!   compute worker, as it always was. At width > 1 it runs no ordinary Hirsel computation:
//!   only the launcher's service loop, home intervals with their callbacks, the run's Io
//!   binding and home-state destruction.
//! - **Handoff.** A thread hands the run to another by posting the task to run next to that
//!   thread's [`Exec`] and switching to its own idle fiber, which waits for a post addressed to
//!   it. The post happens on the idle fiber, after the switch saved the running fiber's stack
//!   pointer, so no thread resumes a fiber another is still leaving. One thread runs the run's
//!   code at a time (the run token travels with the posts), so a run's observations do not
//!   depend on which thread ran it: the schedule stays the cooperative scheduler's. A task
//!   resumed on another thread than it last ran on has migrated: its context names the task,
//!   never the worker (`TaskCtx`), so compiled frames stay valid.
//! - **Migrate every suspension.** The test mode that resumes every task on a different
//!   compute worker than it last ran on at each suspension, which shows that nothing caches a
//!   thread's state across one (the W1 gate).
//! - **Permits.** A thread holds one of the process's compute permits while it runs Hirsel code
//!   ([`Permits`]): it gives it back when it goes idle and when a task it runs enters C on it,
//!   and takes one before it resumes a task into Hirsel code and at a callback's entry. A
//!   blocking call holds none while it is in C; at width > 1 the home thread holds none
//!   except for a home callback's Hirsel code.
//!   An assigned scoped reservation keeps its process credit across idle transitions;
//!   its native worker's exit returns that credit, independently of task fibers.
//! - **Lifetime.** Compute workers start with the run; pool threads on first use. Both stop
//!   when the run ends ([`shutdown`]), after the run's last task stopped; their heaps stay in
//!   the run's registry. A run that stopped while a worker still runs its code (a pure loop
//!   with no runtime entry) is detached instead ([`detach`]): its last thread to leave frees
//!   it.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering, Ordering::Relaxed,
};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::context::{RunShared, TaskCtx, Worker};
use crate::fiber::{self, Stack};
use crate::stop::{Stop, Stopped};
use crate::{environmental, fault};

pub(crate) mod cpu_scoped;
pub(crate) mod free;
pub(crate) mod ordered_commit;

/// The most blocking-pool threads one run starts.
pub(crate) const POOL_THREADS: usize = 64;

/// The most compute workers one run starts.
pub(crate) const MAX_WIDTH: u32 = 256;

/// Which threads may run a task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Owner {
    /// Any compute worker (the home thread at width 1).
    Any,
    /// The home thread: a home foreign interval, or the run's Io binding in use.
    Home,
    /// Pool slot `0`'s thread, during a blocking foreign interval.
    Pool(u16),
    /// This exact compute exec, whose address the topology retains through shutdown.
    /// A worker index is insufficient: growth may reuse a retiring worker's index.
    Compute(usize),
}

/// What an exec's thread is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Home,
    Compute(u16),
    /// A CPU-scoped worker serves addressed frames, never the ordinary queue.
    Scoped(u16),
    Pool(u16),
    Commit,
}

/// A closure the home thread runs for a thread that waits for it ([`call_home`]).
#[derive(Clone)]
pub(crate) struct Call {
    claim: Arc<hirsel_free::claim::Request<()>>,
    run: *mut (dyn FnMut() + 'static),
    done: *const Done,
}

/// A [`Call`]'s completion, on its caller's stack.
#[derive(Default)]
struct Done {
    ended: Mutex<Option<bool>>,
    bell: Condvar,
}

/// What a thread is asked to do next.
enum Post {
    /// Resume task `task`, taking a compute permit first when `permit` (a task resumed into C
    /// or into the home thread's Io takes none).
    Run { task: u32, permit: bool },
    /// An exclusive saved physical segment, selected independently of policy.
    Saved(free::Saved),
    /// Home only: run a closure for a waiting thread.
    Call(Call),
    /// Home only: the run's state changed (its root completed or it stopped): look again.
    Wake,
    /// The run ended: leave.
    Exit,
}

struct Inbox {
    posts: VecDeque<(Post, Option<Instant>)>,
    /// A closed inbox takes no post: the home thread after its run stopped.
    closed: bool,
}

/// One thread that runs a run's code: home, a compute worker or a pool thread.
pub(crate) struct Exec {
    inbox: Mutex<Inbox>,
    bell: Condvar,
    kind: Kind,
    /// The thread holds the run: from the post of a task to it until it goes idle.
    busy: AtomicBool,
    /// Touched only by the exec's own thread: its idle fiber's stack pointer, the post its
    /// idle fiber makes on regaining control, its worker and whether it holds a permit.
    pub(crate) idle: Cell<*mut u8>,
    deferred: Cell<Option<(*const Exec, Post)>>,
    worker: Cell<*mut Worker>,
    held: AtomicBool,
    /// A scoped domain owns this worker's compute credit even while it is idle.
    reserved: AtomicBool,
    scoped_slot: AtomicU32,
    exited: AtomicBool,
    lost: AtomicBool,
    ready: Cell<Option<hirsel_free::work::Worker<free::Saved>>>,
    active: Cell<Option<free::Active>>,
    /// The Free operation budget of the immediate chain this exec runs.
    chain: Cell<hirsel_free::schedule::Budget>,
    pub(crate) leaving: Cell<Option<free::Leave>>,
    ready_epoch: AtomicU64,
    /// Foreign frames may suspend here, but may never resume on another thread.
    foreign_frames: AtomicU32,
    exit_pending: Cell<bool>,
    /// Optional execution-level home wake. Set before a polled child starts;
    /// the notification carries no adapter or context pointer.
    home_driver: std::sync::OnceLock<crate::readiness::Readiness>,
    home_signal: std::sync::OnceLock<Arc<crate::readiness::Signal>>,
    /// This executor's recording slot, from its start serving the run until it leaves.
    recording: crate::tasks::free::record::Registration,
}

// SAFETY: the cells are touched only by the exec's own thread while it runs.
// After exited is acquired, the run-token holder may retire and clear its worker
// under the compute lock. The inbox is behind its own lock.
unsafe impl Send for Exec {}
// SAFETY: as for `Send`.
unsafe impl Sync for Exec {}

impl Exec {
    fn new(kind: Kind, worker: *mut Worker, held: bool) -> Arc<Exec> {
        Arc::new(Exec {
            inbox: Mutex::new(Inbox {
                posts: VecDeque::new(),
                closed: false,
            }),
            bell: Condvar::new(),
            kind,
            busy: AtomicBool::new(false),
            idle: Cell::new(std::ptr::null_mut()),
            deferred: Cell::new(None),
            worker: Cell::new(worker),
            held: AtomicBool::new(held),
            reserved: AtomicBool::new(false),
            scoped_slot: AtomicU32::new(match kind {
                Kind::Scoped(index) => u32::from(index),
                _ => u32::MAX,
            }),
            exited: AtomicBool::new(false),
            lost: AtomicBool::new(false),
            ready: Cell::new(None),
            active: Cell::new(None),
            chain: Cell::default(),
            leaving: Cell::new(None),
            ready_epoch: AtomicU64::new(0),
            foreign_frames: AtomicU32::new(0),
            exit_pending: Cell::new(false),
            home_driver: Default::default(),
            home_signal: Default::default(),
            recording: Default::default(),
        })
    }

    pub(super) fn scoped_index(&self) -> Option<u32> {
        let index = self.scoped_slot.load(Ordering::Acquire);
        (index != u32::MAX).then_some(index)
    }

    /// Post `post`; false when the inbox is closed. A task posted makes the exec busy before
    /// the poster goes idle, so the run's holder is never unaccounted for.
    fn post(&self, post: Post, queued: Option<Instant>) -> bool {
        let mut inbox = self.inbox.lock().unwrap();
        if inbox.closed {
            return false;
        }
        if matches!(post, Post::Run { .. }) {
            if inbox
                .posts
                .iter()
                .any(|(post, _)| matches!(post, Post::Run { .. }))
            {
                fault("a thread was handed two tasks at once");
            }
            self.busy.store(true, Ordering::SeqCst);
        }
        inbox.posts.push_back((post, queued));
        self.bell.notify_one();
        if let Some(driver) = self.home_driver.get() {
            driver.notify().expect("an admitted execution home wake");
        }
        if let Some(signal) = self.home_signal.get() {
            signal.notify();
        }
        true
    }

    fn take(&self) -> (Post, Option<Instant>) {
        let mut inbox = self.inbox.lock().unwrap();
        loop {
            if let Some(post) = inbox.posts.pop_front() {
                return post;
            }
            inbox = self.bell.wait(inbox).unwrap();
        }
    }

    /// Close the inbox: no thread posts to it again.
    fn close(&self) {
        self.inbox.lock().unwrap().closed = true;
    }

    /// Whether the thread holds the run (or runs a home interval, for home).
    pub(crate) fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

/// One process-wide compute permit pool (§7). Capacity changes do not manufacture
/// replacements for stopped workers: a worker keeps its permit until it leaves code.
/// The predicate and notification use the same lock, as in Rust std's Condvar
/// sync/poison/condvar.rs:182-194 (ea137335b788) wait_while protocol;
/// Tokio batch_semaphore.rs:334-368 (ce685ee69165) likewise
/// returns capacity under the wait-list lock before waking waiters. Here waiters
/// are OS workers rather than futures, so a condition variable replaces wakers.
pub(crate) struct Permits {
    state: Mutex<PermitState>,
    freed: Condvar,
}
struct PermitState {
    limit: u32,
    held: u32,
    waiting: u32,
    lost: u32,
    contraction: u64,
}

static PROCESS_PERMITS: std::sync::OnceLock<Arc<Permits>> = std::sync::OnceLock::new();
static PROCESS_ADMITTED: Mutex<bool> = Mutex::new(false);
fn process_permits() -> Arc<Permits> {
    Arc::clone(PROCESS_PERMITS.get_or_init(|| {
        // No filesystem discovery for a sole root. The first physical claim
        // replaces this single-root grant with its strict snapshot before starting work.
        Arc::new(Permits::new(1))
    }))
}

/// The first real snapshot seeds the process grant; explicit embedding grants
/// and the credits of already-admitted runs keep their owner.
pub(crate) fn init_compute_grant(limits: &hirsel_run_config::managed_cores::HostLimits) {
    let mut admitted = PROCESS_ADMITTED.lock().unwrap();
    if *admitted {
        return;
    }
    let affinity = limits.admission_affinity.len();
    let limit = limits
        .quota
        .map_or(affinity, |q| affinity.min(q.floor().max(1)));
    update_compute_grant(u32::try_from(limit.max(1)).unwrap_or(u32::MAX));
    *admitted = true;
}

fn ensure_compute_grant() {
    let mut admitted = PROCESS_ADMITTED.lock().unwrap();
    if *admitted {
        return;
    }
    let limit = std::thread::available_parallelism()
        .map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX));
    update_compute_grant(limit);
    *admitted = true;
}

/// Set the process's compute grant. Only the embedding manager calls this;
/// a contraction lets existing holders leave, and admits no replacement meanwhile.
/// All runs, pool callbacks and nested local runner code use this same pool.
pub fn set_compute_grant(limit: u32) {
    assert!(limit > 0, "a compute grant must be positive");
    let mut admitted = PROCESS_ADMITTED.lock().unwrap();
    update_compute_grant(limit);
    *admitted = true;
}

fn update_compute_grant(limit: u32) {
    let permits = process_permits();
    {
        let mut state = permits.state.lock().unwrap();
        if limit < state.limit {
            state.contraction = state.contraction.wrapping_add(1);
        }
        state.limit = limit;
    }
    permits.freed.notify_all();
}

/// Current process permit accounting, available to the embedding manager.
/// A stopped divergent worker stays in `held` until it leaves code.
pub fn compute_permits() -> (u32, u32, u32) {
    let permits = process_permits();
    let state = permits.state.lock().unwrap();
    (state.limit, state.held, state.waiting)
}

/// Permits retained by stopped runs across the entire process. They remain
/// deducted from capacity, including after those runs' handles are dropped.
pub fn lost_compute_permits() -> u32 {
    process_permits().state.lock().unwrap().lost
}

/// Wake threads waiting for a permit when their run stops. Keeping the lock
/// through notification prevents a stop from being lost just before a wait.
pub(crate) fn wake_permits() {
    if let Some(permits) = PROCESS_PERMITS.get() {
        let _state = permits.state.lock().unwrap();
        permits.freed.notify_all();
    }
}

impl Permits {
    fn new(limit: u32) -> Self {
        Self {
            state: Mutex::new(PermitState {
                limit,
                held: 0,
                waiting: 0,
                lost: 0,
                contraction: 0,
            }),
            freed: Condvar::new(),
        }
    }
    /// Whether this thread took a permit (nested callbacks keep the existing one).
    /// A newcomer leaves a free permit to each thread already waiting, so a
    /// worker that releases its permit cannot take it straight back past the
    /// waiter its release woke: a barging semaphore starves that waiter
    /// whenever the releaser asks again first (Go's `semrelease1` hands its
    /// count to a waiter for the same reason when `handoff` is set).
    fn acquire(
        &self,
        exec: &Exec,
        control: Option<&crate::stop::Control>,
        metrics: Option<&WorkerMetrics>,
    ) -> Stopped<bool> {
        if exec.held.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let started = metrics.map(|_| Instant::now());
        let mut state = self.state.lock().unwrap();
        let mut waited = false;
        loop {
            if exec.reserved.load(Ordering::Acquire) {
                if let Some(stop) = control.and_then(|c| c.stopped()) {
                    return Err(stop);
                }
                exec.held.store(true, Ordering::Release);
                return Ok(true);
            }
            if let Some(stop) = control.and_then(|c| c.stopped()) {
                return Err(stop);
            }
            let reserved = if waited { 0 } else { state.waiting };
            if state.held + reserved < state.limit {
                break;
            }
            state.waiting += 1;
            state = self.freed.wait(state).unwrap();
            state.waiting -= 1;
            waited = true;
        }
        state.held += 1;
        if waited && state.held < state.limit && state.waiting > 0 {
            // A free permit a deferred newcomer left stays reachable.
            self.freed.notify_one();
        }
        exec.held.store(true, Ordering::Release);
        if let (Some(metrics), Some(started)) = (metrics, started) {
            metrics
                .permit_wait_ns
                .fetch_add(nanos(started.elapsed()), Relaxed);
            metrics.permit_acquires.fetch_add(1, Relaxed);
        }
        Ok(true)
    }
    fn release(&self, exec: &Exec) {
        if !exec.held.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if !exec.held.swap(false, Ordering::AcqRel) {
            return;
        }
        if !exec.reserved.load(Ordering::Acquire) {
            state.held -= 1;
            if exec.lost.swap(false, Ordering::AcqRel) {
                state.lost -= 1;
            }
        }
        self.freed.notify_one();
    }
    #[cfg(test)]
    fn available(&self) -> u32 {
        let state = self.state.lock().unwrap();
        state.limit.saturating_sub(state.held)
    }
}

struct PoolThread {
    exec: Arc<Exec>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// A task's interval is on it.
    busy: bool,
}

/// The threads of a run with compute workers, or with a pool, which a detached run's last
/// thread to leave frees it through.
pub(crate) struct Holders {
    /// The launcher and every live compute and pool thread.
    count: AtomicU32,
    /// The launcher left the run to its threads (`detach`).
    detached: AtomicBool,
    /// The final owner has destroyed the detached run's storage.
    reclaimed: AtomicBool,
    workers: Mutex<Vec<std::sync::Weak<Exec>>>,
}

impl Holders {
    /// The threads still holding a detached run.
    pub(crate) fn lost_permits(&self) -> u32 {
        self.workers
            .lock()
            .unwrap()
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .filter(|exec| {
                exec.held.load(Ordering::Acquire) || exec.reserved.load(Ordering::Acquire)
            })
            .count() as u32
    }
    pub(crate) fn left(&self) -> u32 {
        let count = self.count.load(Ordering::SeqCst);
        // The last thread decrements count before reclaiming. Retain its
        // reclamation credit until task contexts and recorder owners drop.
        if count == 0
            && self.detached.load(Ordering::Acquire)
            && !self.reclaimed.load(Ordering::Acquire)
        {
            1
        } else {
            count
        }
    }

    pub(crate) fn reclaimed(&self) {
        self.reclaimed.store(true, Ordering::Release);
    }
}

/// The holders of `run`'s threads, when it has any.
pub(crate) fn holders(run: &RunShared) -> Option<Arc<Holders>> {
    run.execs.as_deref().map(|execs| Arc::clone(&execs.holders))
}

/// Scoped CPU cleanup is linked only by its admitted start.
pub(super) struct ScopedCleanup {
    request_exit: fn(&Execs),
    shutdown: fn(&Execs),
}

/// A run's threads beyond the one that admitted it: its compute workers from its start at
/// width > 1, else from its first pool dispatch.
pub(crate) struct Execs {
    run: RunPtr,
    demand: bool,
    home: Arc<Exec>,
    /// The home thread's idle fiber at width 1, prepared when home first waits.
    home_idle: Mutex<Option<Stack>>,
    compute: Mutex<ComputeWorkers>,
    pool: Mutex<Vec<PoolThread>>,
    scoped: Mutex<Option<cpu_scoped::Workers>>,
    scoped_cleanup: std::sync::OnceLock<&'static ScopedCleanup>,
    pub(crate) permits: Arc<Permits>,
    holders: Arc<Holders>,
    /// The next compute worker a task with no worker of its own goes to.
    cursor: AtomicUsize,
    /// Migrate every suspension (the W1 test mode).
    migrate: bool,
    /// Production Ordered width one uses the admission thread as its worker.
    home_at_one: bool,
    calls: hirsel_free::claim::Requests<()>,
    next_call: AtomicU64,
    activity: Mutex<()>,
    dispatch: std::sync::OnceLock<free::Dispatch>,
}

/// Active destinations and workers draining after a shrink. Only the run-token
/// holder changes membership, under this lock; admission can snapshot it safely.
#[derive(Default)]
struct ComputeWorkers {
    active: Vec<Arc<Exec>>,
    retiring: Vec<Arc<Exec>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// The scheduling row's headroom counters (design §5 Headroom, B1's `switch_queue`,
/// `fiber_creation` and `migration` rows in `bench/multicore/manifest.json`). A run holds
/// them only when metrics are enabled: every collection point is behind an
/// `Option<Arc<WorkerMetrics>>`, so a run without metrics pays one branch and reads no
/// clock.
#[derive(Debug)]
pub struct WorkerMetrics {
    started: Instant,
    ops: AtomicU64,
    switches: AtomicU64,
    elisions: AtomicU64,
    queue_ops: AtomicU64,
    ready_metrics: std::sync::OnceLock<Arc<hirsel_free::work::Metrics>>,
    cancel_checks: AtomicU64,
    ancestor_links: AtomicU64,
    spawns: AtomicU64,
    spawn_ns: AtomicU64,
    migrations: AtomicU64,
    refreshes: AtomicU64,
    refresh_ns: AtomicU64,
    resident: AtomicU64,
    resident_peak: AtomicU64,
    stack_rss: AtomicU64,
    permit_wait_ns: AtomicU64,
    permit_acquires: AtomicU64,
}

impl Default for WorkerMetrics {
    fn default() -> Self {
        WorkerMetrics {
            started: Instant::now(),
            ops: AtomicU64::new(0),
            switches: AtomicU64::new(0),
            elisions: AtomicU64::new(0),
            queue_ops: AtomicU64::new(0),
            ready_metrics: std::sync::OnceLock::new(),
            cancel_checks: AtomicU64::new(0),
            ancestor_links: AtomicU64::new(0),
            spawns: AtomicU64::new(0),
            spawn_ns: AtomicU64::new(0),
            migrations: AtomicU64::new(0),
            refreshes: AtomicU64::new(0),
            refresh_ns: AtomicU64::new(0),
            resident: AtomicU64::new(0),
            resident_peak: AtomicU64::new(0),
            stack_rss: AtomicU64::new(0),
            permit_wait_ns: AtomicU64::new(0),
            permit_acquires: AtomicU64::new(0),
        }
    }
}

fn nanos(waited: Duration) -> u64 {
    u64::try_from(waited.as_nanos()).unwrap_or(u64::MAX)
}

impl WorkerMetrics {
    /// A semantic operation: a scheduling point.
    pub(crate) fn op(&self) {
        self.ops.fetch_add(1, Relaxed);
    }
    /// A task switch: one switch-out and switch-in pair.
    pub(crate) fn switch(&self) {
        self.switches.fetch_add(1, Relaxed);
    }
    /// Task switches so far.
    #[cfg(test)]
    pub(crate) fn switches(&self) -> u64 {
        self.switches.load(Relaxed)
    }
    /// A completed immediate op retained its task's worker.
    pub(crate) fn elision(&self) {
        self.elisions.fetch_add(1, Relaxed);
    }
    /// Semantic ops whose fiber switch was elided (diagnostic count).
    pub fn elisions(&self) -> u64 {
        self.elisions.load(Relaxed)
    }
    /// A runnable queue push, pop or steal.
    pub(crate) fn queue_op(&self) {
        self.queue_ops.fetch_add(1, Relaxed);
    }
    fn ready_metrics(&self) -> Arc<hirsel_free::work::Metrics> {
        self.ready_metrics.get_or_init(Default::default).clone()
    }
    pub(crate) fn cancellation(&self, depth: u64) {
        self.cancel_checks.fetch_add(1, Relaxed);
        self.ancestor_links.fetch_add(depth, Relaxed);
    }
    /// A child task created in `took`; it is resident until it completes.
    pub(crate) fn spawn(&self, took: Duration) {
        self.spawns.fetch_add(1, Relaxed);
        self.spawn_ns.fetch_add(nanos(took), Relaxed);
        let resident = self.resident.fetch_add(1, Relaxed) + 1;
        self.resident_peak.fetch_max(resident, Relaxed);
    }
    /// A child task completed.
    pub(crate) fn complete(&self) {
        self.resident.fetch_sub(1, Relaxed);
    }
    /// A started task resumed on another worker than it last ran on.
    pub(crate) fn migration(&self) {
        self.migrations.fetch_add(1, Relaxed);
    }
    /// A switch-in refreshed its view's pages, in `took`.
    pub(crate) fn refresh(&self, took: Duration) {
        self.refreshes.fetch_add(1, Relaxed);
        self.refresh_ns.fetch_add(nanos(took), Relaxed);
    }
    /// The touched bytes of the run's task stacks at its end.
    pub(crate) fn stack_rss(&self, bytes: u64) {
        self.stack_rss.store(bytes, Relaxed);
    }
    /// The rows' metrics by their names in `bench/multicore/telemetry.json`:
    /// `switches_per_op`, `queue_ops_per_op`, `spawn_ns` (mean per child),
    /// `rss_bytes_per_task` (touched stack bytes per simultaneously resident task),
    /// `migrations_per_s` and `refresh_ns` (mean per refresh). A metric whose denominator
    /// is zero is left out, never reported as zero.
    pub fn values(&self) -> Vec<(&'static str, f64)> {
        let ratio = |total: &AtomicU64, count: u64| match count {
            0 => None,
            count => Some(total.load(Relaxed) as f64 / count as f64),
        };
        let ops = self.ops.load(Relaxed);
        let seconds = self.started.elapsed().as_secs_f64();
        let ready = self.ready_metrics.get().map(|metrics| metrics.counts());
        let queue_ops = self.queue_ops.load(Relaxed) + ready.map_or(0, |(ops, _, _)| ops);
        let mut values = vec![
            (
                "ancestor_depth",
                ratio(&self.ancestor_links, self.cancel_checks.load(Relaxed)),
            ),
            ("switches_per_op", ratio(&self.switches, ops)),
            (
                "permit_wait_ns",
                ratio(&self.permit_wait_ns, self.permit_acquires.load(Relaxed)),
            ),
            (
                "queue_ops_per_op",
                (ops != 0).then(|| queue_ops as f64 / ops as f64),
            ),
            ("spawn_ns", ratio(&self.spawn_ns, self.spawns.load(Relaxed))),
            (
                "rss_bytes_per_task",
                ratio(&self.stack_rss, self.resident_peak.load(Relaxed)),
            ),
            (
                "migrations_per_s",
                (seconds > 0.0).then(|| self.migrations.load(Relaxed) as f64 / seconds),
            ),
            (
                "refresh_ns",
                ratio(&self.refresh_ns, self.refreshes.load(Relaxed)),
            ),
        ];
        if let Some((_, steals, _)) = ready {
            values.push((
                "steals_per_s",
                (seconds > 0.0).then(|| steals as f64 / seconds),
            ));
        }
        values
            .into_iter()
            .filter_map(|(name, value)| Some((name, value?)))
            .collect()
    }
    /// Started tasks resumed on another worker so far.
    pub fn migrations(&self) -> u64 {
        self.migrations.load(Relaxed)
    }
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
    /// The compute or pool thread's exec; null on every other thread (home included).
    static EXEC: Cell<*const Exec> = const { Cell::new(std::ptr::null()) };
}

/// The calling thread's compute or pool exec, or null on home. Out of line, like every
/// thread-local access, so no caller keeps a thread's address across a suspension.
#[inline(never)]
fn own_exec() -> *const Exec {
    EXEC.with(Cell::get)
}

/// Whether this running task/driver is on its run's admission thread.
/// # Safety
/// The caller owns a live task or saved-frame claim in `run`. A null EXEC is
/// admission's direct path; all compute/pool/commit threads install their Exec.
pub(crate) unsafe fn on_home(run: *mut RunShared) -> bool {
    let Some(execs) = (unsafe { (*(*run).execs.get()).as_deref() }) else {
        return false;
    };
    let current = own_exec();
    current.is_null() || std::ptr::eq(current, Arc::as_ptr(&execs.home))
}

/// Whether a running task's quantum may continue here without the placement
/// check of a scheduling point: on a compute worker of a run that does not
/// force migration, or on home while no compute worker is active. A run whose
/// execs are not initialized yet is not settled: only the role initializes them. A retiring
/// worker is left at the task's next scheduling point.
/// # Safety
/// As for on_home.
pub(crate) unsafe fn settled(run: *mut RunShared) -> bool {
    let Some(execs) = (unsafe { (*(*run).execs.get()).as_deref() }) else {
        return false;
    };
    let current = own_exec();
    if execs.migrate || current.is_null() {
        return false;
    }
    // SAFETY: a thread's own exec lives while the thread serves its run.
    match unsafe { (*current).kind } {
        Kind::Compute(_) | Kind::Scoped(_) => true,
        Kind::Home => execs.compute.lock().unwrap().active.is_empty(),
        Kind::Pool(_) | Kind::Commit => false,
    }
}

/// Whether the calling thread is one of a run's blocking-pool threads.
#[inline(never)]
pub(crate) fn on_pool() -> bool {
    let exec = own_exec();
    // SAFETY: a thread's own exec lives while the thread serves its run.
    !exec.is_null() && matches!(unsafe { (*exec).kind }, Kind::Pool(_))
}

/// The calling thread's compute worker, or `None` on home and pool threads.
#[inline(never)]
pub(crate) fn compute_index() -> Option<u16> {
    let exec = own_exec();
    if exec.is_null() {
        return None;
    }
    // SAFETY: as for `on_pool`.
    match unsafe { (*exec).kind } {
        Kind::Compute(index) => Some(index),
        _ => None,
    }
}

/// The calling thread's executor registration, or `None` on a thread serving no exec.
pub(crate) fn registration<T>(
    f: impl FnOnce(Option<&crate::tasks::free::record::Registration>) -> T,
) -> T {
    let exec = own_exec();
    // SAFETY: the calling thread owns this exec until it exits the run, and only it touches
    // the registration.
    f(unsafe { exec.as_ref() }.map(|exec| &exec.recording))
}

/// A test executor that a thread enters as serve_home and poll_home_posts enter home.
#[cfg(test)]
pub(crate) struct TestExec(Arc<Exec>);

#[cfg(test)]
impl TestExec {
    pub(crate) fn new() -> Self {
        Self(Exec::new(Kind::Home, std::ptr::null_mut(), false))
    }

    /// Run `f` with this as the calling thread's exec, then restore the enclosing one.
    pub(crate) fn enter<T>(
        &self,
        f: impl FnOnce(&crate::tasks::free::record::Registration) -> T,
    ) -> T {
        let enclosing = EXEC.with(|e| e.replace(Arc::as_ptr(&self.0)));
        let result = f(&self.0.recording);
        EXEC.with(|e| e.set(enclosing));
        result
    }
}

/// Preferred recording section slot of the calling executor, as it registers. A retiring
/// compute exec and its replacement may share an index; the replacement then registers an
/// idle slot. Home and pool have disjoint preferred ranges.
pub(crate) fn recorder_slot() -> usize {
    let exec = own_exec();
    if exec.is_null() {
        return 0;
    }
    // SAFETY: the calling thread owns this exec until it exits the run.
    match unsafe { (*exec).kind } {
        Kind::Home => 0,
        Kind::Compute(index) => usize::from(index) + 1,
        Kind::Scoped(index) => MAX_WIDTH as usize + POOL_THREADS + 2 + usize::from(index),
        Kind::Pool(index) => MAX_WIDTH as usize + 1 + usize::from(index),
        Kind::Commit => MAX_WIDTH as usize + 1 + POOL_THREADS,
    }
}

/// Pin a foreign interval to its caller's compute exec. Only this thread may
/// begin/end a pin; the count is atomic because resize may request its exit.
/// Home caller intervals also keep their exact Exec across width changes;
/// explicit home intervals and Pool already have exact owner routes.
pub(crate) fn pin_foreign_compute() -> Option<Owner> {
    let exec = own_exec();
    if exec.is_null()
        || !matches!(
            unsafe { (*exec).kind },
            Kind::Compute(_) | Kind::Scoped(_) | Kind::Home
        )
    {
        return None;
    }
    unsafe { (*exec).foreign_frames.fetch_add(1, Ordering::AcqRel) };
    Some(Owner::Compute(exec.expose_provenance()))
}

pub(crate) fn unpin_foreign_compute(owner: Owner) {
    let Owner::Compute(address) = owner else {
        fault("a foreign compute pin lost its owner");
    };
    let exec = own_exec();
    assert_eq!(exec.expose_provenance(), address);
    assert!(unsafe { (*exec).foreign_frames.fetch_sub(1, Ordering::AcqRel) } > 0);
}

/// Whether `run` uses the physical executor topology, including Ordered home at width one.
#[inline]
pub(crate) fn wide(run: &RunShared) -> bool {
    run.execs
        .as_deref()
        .is_some_and(|e| !e.compute.lock().unwrap().active.is_empty())
}

/// The run's execs, created with the home thread holding the run's one permit.
fn execs(ctx: *mut TaskCtx) -> &'static Execs {
    // SAFETY: execution may share Execs, whose live mutations are interior.
    let existing = unsafe { (*(*(*ctx).run_ptr()).execs.get()).as_deref() };
    if let Some(execs) = existing {
        return execs;
    }
    // SAFETY: the first initialization is admission or scheduler-role owned.
    unsafe { init_execs(ctx) }
}

static CLEANUP: crate::context::PoolCleanup = crate::context::PoolCleanup {
    shutdown,
    detach,
    forget,
};

unsafe fn init_execs(ctx: *mut TaskCtx) -> &'static mut Execs {
    // SAFETY: the context and its run are live for the whole run; the execs are reached by the
    // thread that runs the run's code, one at a time, and their shared parts are behind
    // locks or atomics.
    unsafe {
        let run = (*ctx).run_ptr();
        (*run).pool_cleanup.get_or_init(|| &CLEANUP);
        (*(*run).execs.get()).get_or_insert_with(|| {
            if on_pool() {
                fault("a run's first dispatch came from a pool thread");
            }
            Box::new(Execs {
                run: RunPtr(run),
                demand: false,
                home: Exec::new(Kind::Home, (*ctx).worker_ptr(), false),
                home_idle: Mutex::new(None),
                compute: Mutex::new(ComputeWorkers::default()),
                pool: Mutex::new(Vec::new()),
                scoped: Mutex::new(None),
                scoped_cleanup: std::sync::OnceLock::new(),
                permits: process_permits(),
                holders: Arc::new(Holders {
                    count: AtomicU32::new(1),
                    detached: AtomicBool::new(false),
                    reclaimed: AtomicBool::new(false),
                    workers: Mutex::new(Vec::new()),
                }),
                cursor: AtomicUsize::new(0),
                migrate: false,
                home_at_one: false,
                calls: Default::default(),
                next_call: AtomicU64::new(0),
                activity: Mutex::new(()),
                dispatch: std::sync::OnceLock::new(),
            })
        })
    }
}

/// The exec of the calling thread.
fn current(execs: &Execs) -> *const Exec {
    let exec = own_exec();
    if exec.is_null() {
        Arc::as_ptr(&execs.home)
    } else {
        exec
    }
}

fn pool_exec(execs: &Execs, slot: u16) -> *const Exec {
    Arc::as_ptr(&execs.pool.lock().unwrap()[slot as usize].exec)
}

/// The exec to run a task owned by `owner`, which last ran on compute worker `last`, or
/// `None` when the calling thread runs it. A task any compute worker may run stays on the
/// calling worker, except in the migrate-every-suspension mode when it last ran there; from
/// home or a pool thread it goes back to its last worker (another one in that mode). Before
/// a width-1 run's first pool dispatch every task is home's.
pub(crate) fn target(ctx: *mut TaskCtx, owner: Owner, last: Option<u16>) -> Option<*const Exec> {
    // SAFETY: the context is live.
    let execs = unsafe { (*(*(*ctx).run_ptr()).execs.get()).as_deref() }?;
    resize(ctx);
    let compute = execs.compute.lock().unwrap();
    let me = current(execs);
    let to = match owner {
        Owner::Home => Arc::as_ptr(&execs.home),
        Owner::Pool(slot) => pool_exec(execs, slot),
        Owner::Compute(address) => std::ptr::with_exposed_provenance(address),
        Owner::Any if compute.active.is_empty() || compute.active[0].kind == Kind::Home => {
            Arc::as_ptr(&execs.home)
        }
        Owner::Any => {
            // SAFETY: the calling thread's exec lives while it serves the run.
            let here = match unsafe { (*me).kind } {
                Kind::Compute(index) => Some(index),
                _ => None,
            };
            let width = compute.active.len();
            let here = here.filter(|&index| {
                compute
                    .active
                    .get(usize::from(index))
                    .is_some_and(|e| Arc::as_ptr(e) == me)
            });
            let index = match (here, last) {
                (Some(here), Some(last)) if execs.migrate && here == last => {
                    (usize::from(here) + 1) % width
                }
                (Some(_), _) => return None,
                (None, Some(last)) if execs.migrate => (usize::from(last) + 1) % width,
                (None, Some(last)) => usize::from(last) % width,
                (None, None) => execs.cursor.fetch_add(1, Relaxed) % width,
            };
            Arc::as_ptr(&compute.active[index])
        }
    };
    (to != me).then_some(to)
}

/// Whether the posted task resumes into Hirsel code on `to`. Compute workers
/// and width-1 home execution take a permit. A saved callback also takes one
/// on Home or Pool; initial dispatch into C (callback depth zero) takes none.
pub(crate) fn resumes_into_hirsel(ctx: *mut TaskCtx, to: *const Exec) -> bool {
    // SAFETY: the caller owns this running or exclusively saved task.
    if unsafe { (*ctx).callback_depth } != 0 {
        return true;
    }
    // SAFETY: `to` is one of the run's live execs.
    match unsafe { (*to).kind } {
        Kind::Compute(_) | Kind::Scoped(_) => true,
        Kind::Home => {
            // SAFETY: the context is live.
            let run = unsafe { (*ctx).run_ref() };
            !wide(run) || run.execs.as_deref().is_some_and(home_computes)
        }
        Kind::Pool(_) => false,
        Kind::Commit => false,
    }
}

/// Suspend the running fiber, saving its stack pointer at `save`, and hand the run to `to`,
/// which resumes task `next`: the calling thread waits on its idle fiber for its next post.
/// `permit`: `next` resumes into Hirsel code. When `to` takes no post any more (its run
/// stopped), `next` is never resumed and the calling thread goes idle.
///
/// # Safety
/// `ctx` is the running task's, already switched out; `save` stays writable until whoever
/// resumes this fiber reads it; `next` is suspended at its row's stack pointer; `to` is one
/// of the run's execs.
pub(crate) unsafe fn hand_off(
    ctx: *mut TaskCtx,
    save: *mut *mut u8,
    to: *const Exec,
    next: u32,
    permit: bool,
) {
    let execs = execs(ctx);
    let me = current(execs);
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
        let idle = idle_of(ctx, execs, me);
        crate::tasks::role::release(ctx);
        fiber::switch(save, idle);
        crate::tasks::role::enter(ctx);
    }
}

/// `me`'s idle fiber: home's at width 1 is prepared when it first waits.
///
/// # Safety
/// `me` is the calling thread's exec.
unsafe fn idle_of(ctx: *mut TaskCtx, execs: &Execs, me: *const Exec) -> *mut u8 {
    // SAFETY: caller contract.
    unsafe {
        if !(*ctx).ahead_physical.is_null() {
            return (*(*ctx).ahead_physical).idle_sp;
        }
        if (*me).idle.get().is_null() {
            if (*me).kind != Kind::Home {
                fault("a worker thread without its idle fiber");
            }
            let mut stack = Stack::new();
            stack.account((*ctx).run_ref().cpu_ledger.as_ref());
            let sp = stack.prepare(home_idle, (*ctx).run_ptr().cast(), std::ptr::null_mut());
            *execs.home_idle.lock().unwrap() = Some(stack);
            (*me).idle.set(sp);
        }
        (*me).idle.get()
    }
}

/// The calling thread leaves the running task for good (the run stopped, or its root
/// completed into the launch frame) and goes idle: its idle fiber wakes the home thread,
/// whose loop then sees the run's state. Never returns.
///
/// # Safety
/// `ctx` is the running task's, at width > 1; nothing on its fiber is needed again.
pub(crate) unsafe fn abandon(ctx: *mut TaskCtx) -> ! {
    // SAFETY: caller contract.
    unsafe {
        if free::enabled((*ctx).run_ref()) {
            free::finish_current(ctx);
        }
        (*ctx).switch_out();
        let execs = execs(ctx);
        let me = current(execs);
        let home = Arc::as_ptr(&execs.home);
        (*me).deferred.set(Some((home, Post::Wake)));
        let idle = idle_of(ctx, execs, me);
        let mut abandoned = std::ptr::null_mut();
        crate::tasks::role::release(ctx);
        fiber::switch(&raw mut abandoned, idle);
    }
    fault("an abandoned fiber was resumed")
}

/// Make the deferred post of `exec`'s thread, after its idle fiber regained control: it
/// gives back its permit and goes idle. Under the activity lock it clears its own
/// busy flag before posting, so a fast bounce cannot have its new busy flag erased.
/// The home snapshot uses that lock to exclude the transit gap.
///
/// # Safety
/// `exec` is the calling thread's.
unsafe fn post_deferred(run: *mut RunShared, exec: &Exec) {
    let Some((to, post)) = exec.deferred.take() else {
        return;
    };
    // SAFETY: the run's execs outlive every post.
    unsafe {
        if let Some(execs) = (*(*run).execs.get()).as_deref() {
            execs.permits.release(exec);
        }
        crate::stop::set_running(std::ptr::null_mut());
        crate::foreign::restore(crate::foreign::ForeignScope::NONE);
        let queued = (*run).extern_metrics.as_ref().map(|_| Instant::now());
        let execs = (*(*run).execs.get()).as_deref().unwrap();
        // Clear before publishing: the receiver can immediately bounce a task
        // back, whose fresh busy=true must never be erased by this sender.
        // The snapshot lock prevents the launcher from seeing the transit gap.
        let _activity = execs.activity.lock().unwrap();
        exec.busy.store(false, Ordering::SeqCst);
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
        if permit && let Some(execs) = (*(*run).execs.get()).as_deref() {
            if execs
                .permits
                .acquire(
                    exec,
                    Some(&(*run).control),
                    (*run).worker_metrics.as_deref(),
                )
                .is_err()
            {
                exec.busy.store(false, Ordering::SeqCst);
                wake(&execs.home);
                return;
            }
        }
        let here = match exec.kind {
            Kind::Compute(index) => Some(index),
            _ => None,
        };
        let (ctx, sp) = crate::tasks::resume_row(run, task, here);
        TaskCtx::switch_in(ctx, exec.worker.get());
        let save = exec.idle.as_ptr();
        fiber::switch(save, sp);
    }
}

/// Run a [`Call`] on the home thread and signal its caller.
///
/// # Safety
/// `call` came from a live [`call_home`].
pub(crate) unsafe fn run_call(call: Call) {
    crate::tasks::role::unlent("a home call");
    // SAFETY: the caller waits for `done`, so its closure and completion are live.
    unsafe {
        let claimed = call.claim.claim();
        if claimed {
            (*call.run)();
            call.claim.finish();
        }
        let done = &*call.done;
        // The caller owns Done on its stack. Keep the predicate locked until
        // notification returns, so a fast caller cannot destroy the condition
        // variable while notify_one still accesses its futex word.
        let mut ended = done.ended.lock().unwrap();
        *ended = Some(claimed);
        done.bell.notify_one();
    }
}

/// Take only a synchronous bridge while home waits to merge under the role.
/// Saved fibers and scheduling posts retain their original queue order.
/// # Safety
/// Admission's home thread; no role or adapter borrow is held by the caller.
pub(crate) unsafe fn take_home_call(run: *mut RunShared) -> Option<Call> {
    let execs = unsafe { (*(*run).execs.get()).as_deref() }?;
    let mut inbox = execs.home.inbox.lock().unwrap();
    let index = inbox
        .posts
        .iter()
        .position(|(post, _)| matches!(post, Post::Call(_)))?;
    let (Post::Call(call), _) = inbox.posts.remove(index).unwrap() else {
        unreachable!()
    };
    Some(call)
}

/// Wait for a post and serve it; false at the run's end.
///
/// # Safety
/// `exec` is the calling thread's, idle.
unsafe fn serve(run: *mut RunShared, exec: &Exec) -> bool {
    // SAFETY: caller contract.
    unsafe { post_deferred(run, exec) };
    unsafe { free::saved(run, exec) };
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    if exec.exit_pending.get() && exec.foreign_frames.load(Ordering::Acquire) == 0 {
        free::retire(execs, exec);
        return false;
    }
    let (post, queued) = if execs.dispatch.get().is_none() {
        exec.take()
    } else {
        loop {
            let epoch = exec.ready_epoch.load(Ordering::Acquire);
            if let Some(post) = exec.inbox.lock().unwrap().posts.pop_front() {
                break post;
            }
            if let Some(saved) = free::next(execs, exec) {
                break (Post::Saved(saved), None);
            }
            let mut inbox = exec.inbox.lock().unwrap();
            if let Some(post) = inbox.posts.pop_front() {
                break post;
            }
            if exec.ready_epoch.load(Ordering::Acquire) == epoch {
                unsafe { cpu_scoped::probe::worker(run, exec, "park") };
                drop(exec.bell.wait(inbox).unwrap());
                unsafe { cpu_scoped::probe::worker(run, exec, "wake") };
            }
        }
    };
    if matches!(&post, Post::Saved(_)) {
        exec.busy.store(true, Ordering::SeqCst);
    }
    free::cancel_search(execs, exec);
    if matches!(&post, Post::Run { .. } | Post::Saved(_)) {
        unsafe { cpu_scoped::probe::worker(run, exec, "run") };
    }
    match post {
        Post::Exit => {
            // A resize may have posted Exit before the running segment pinned
            // itself. Its callbacks must still use this idle stack and thread.
            // Stop abandons saved foreign frames, so shutdown need not wait.
            if exec.foreign_frames.load(Ordering::Acquire) != 0
                && unsafe { (*run).control.stopped().is_none() }
            {
                exec.exit_pending.set(true);
                true
            } else {
                free::retire(execs, exec);
                false
            }
        }
        Post::Wake => true,
        Post::Call(call) => {
            // SAFETY: a call's closure waits for it.
            unsafe { run_call(call) };
            true
        }
        Post::Saved(saved) => {
            unsafe { free::resume(run, exec, saved) };
            true
        }
        Post::Run { task, permit } => {
            // SAFETY: the run outlives its threads.
            if let (Some(queued), Some(metrics)) =
                (queued, unsafe { (*run).extern_metrics.as_ref() })
                && matches!(exec.kind, Kind::Pool(_))
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

/// The home thread's idle fiber at width 1: it serves posts for as long as the run lasts. A
/// stop never returns here, and the fiber is abandoned with the run.
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

/// A run pointer a worker thread carries.
struct RunPtr(*mut RunShared);
// SAFETY: the run outlives its threads (`shutdown` joins them, and a detached run is freed
// by its last thread), and each reaches it only while it holds the run.
unsafe impl Send for RunPtr {}

/// A compute or pool thread: posts until the run ends, then it leaves; a detached run's last
/// thread frees it.
fn worker_main(run: RunPtr, exec: Arc<Exec>, holders: Arc<Holders>) {
    worker_main_reserved(run, exec, holders, None, None);
}

fn worker_main_reserved(
    run: RunPtr,
    exec: Arc<Exec>,
    holders: Arc<Holders>,
    assigned: Option<cpu_scoped::Assigned>,
    ready: Option<std::sync::mpsc::SyncSender<()>>,
) {
    let run = run.0;
    EXEC.with(|e| e.set(Arc::as_ptr(&exec)));
    // SAFETY: this exec's own thread, before its first post: no role or section is held. A
    // starting executor may wait for a retiring one's slot.
    unsafe { exec.recording.register(&*run, recorder_slot(), true) };
    if let Some(ready) = ready {
        // A disconnected startup owner already requested rollback. Still serve
        // its Exit post, so recorder retirement and native-stack cleanup run.
        let _ = ready.send(());
    }
    // The idle fiber of a worker thread is the thread's own stack, here.
    // SAFETY: this is the exec's own thread, idle between posts.
    while unsafe { serve(run, &exec) } {}
    // SAFETY: the exec's last phase ended; `exited` below acknowledges its retirement.
    unsafe { exec.recording.retire(&*run) };
    unsafe { cpu_scoped::probe::worker(run, &exec, "exit") };
    EXEC.with(|e| e.set(std::ptr::null()));
    if matches!(exec.kind, Kind::Compute(_)) {
        unsafe { (&(*run).control).live_width.fetch_sub(1, Ordering::AcqRel) };
    }
    drop(assigned);
    exec.exited.store(true, Ordering::Release);
    if holders.count.fetch_sub(1, Ordering::AcqRel) == 1 && holders.detached.load(Ordering::Acquire)
    {
        // SAFETY: the launcher left the run and this is its last thread.
        unsafe { crate::context::reclaim(run) };
    }
}

/// Start `ctx`'s run on `width` compute workers beside its home thread (the calling thread,
/// which admitted the run), migrating every suspension in the test mode `migrate`. The home
/// thread holds no compute permit from here on.
///
/// # Safety
/// `ctx` is the run's root context, on the admission thread, before the run's code runs;
/// `width` is 1 to [`MAX_WIDTH`]. Physical executor reference tests can
/// use one compute worker beside home; ordinary width1 stays on home.
pub(crate) unsafe fn start(ctx: *mut TaskCtx, width: u32, migrate: bool) {
    unsafe { start_with_home(ctx, width, migrate, false, false) };
}

/// Production Ordered keeps the width-one caller on home, using the same
/// physical saved-frame dispatcher and permits as its wider topology.
/// # Safety
/// As for start, before root publication.
pub(crate) unsafe fn start_ordered(ctx: *mut TaskCtx, width: u32, migrate: bool) {
    unsafe { start_with_home(ctx, width, migrate, true, false) };
}

/// Production Ordered starts additional workers only for runnable work.
/// # Safety
/// As for start, before root publication.
pub(crate) unsafe fn start_ordered_demand(ctx: *mut TaskCtx, width: u32, migrate: bool) {
    let demand = !unsafe { (*ctx).run_ref().capacity.as_ref() }
        .is_some_and(crate::managed_cores::Service::observed);
    unsafe { start_with_home(ctx, width, migrate, true, demand) };
}

/// Production Free keeps home available for stop/service work while the
/// first runnable root uses one compute thread. Further workers follow ready work.
/// # Safety
/// As for start, before root publication.
pub(crate) unsafe fn start_demand(ctx: *mut TaskCtx, width: u32, migrate: bool) {
    let demand = !unsafe { (*ctx).run_ref().capacity.as_ref() }
        .is_some_and(crate::managed_cores::Service::observed);
    unsafe { start_with_home(ctx, width, migrate, false, demand) };
}

fn home_computes(execs: &Execs) -> bool {
    execs
        .compute
        .lock()
        .unwrap()
        .active
        .first()
        .is_some_and(|e| e.kind == Kind::Home)
}

unsafe fn start_with_home(
    ctx: *mut TaskCtx,
    width: u32,
    migrate: bool,
    home_at_one: bool,
    demand: bool,
) {
    if unsafe { (*ctx).run_ref().deferred_capacity.is_none() } {
        ensure_compute_grant();
    }
    // SAFETY: caller contract.
    let run = unsafe { (*ctx).run_ptr() };
    let execs = unsafe { init_execs(ctx) };
    if !execs.compute.lock().unwrap().active.is_empty() || !execs.pool.lock().unwrap().is_empty() {
        fault("a run's workers started twice");
    }
    execs.home.held.store(false, Ordering::Release);
    execs.migrate = migrate;
    execs.home_at_one = home_at_one;
    execs.demand = demand;
    // Live topology and thread handles are published under the compute lock.
    // No exclusive Execs borrow crosses the first thread spawn.
    let execs: &Execs = execs;
    unsafe { (&(*run).control).width.store(width, Ordering::Release) };
    let mut compute = execs.compute.lock().unwrap();
    if home_at_one && (width == 1 || demand) {
        compute.active.push(Arc::clone(&execs.home));
        unsafe { (&(*run).control).live_width.fetch_add(1, Ordering::AcqRel) };
    } else {
        grow(run, execs, &mut compute, if demand { 1 } else { width });
    }
    drop(compute);
    // SAFETY: as above.
    unsafe { (*run).control.set_home(Arc::clone(&execs.home)) };
    unsafe {
        (*run)
            .control
            .set_worker_phase(hirsel_run_config::managed_cores::WorkerPhase::Running)
    };
}

/// The compute lock serializes membership and thread creation. Worker::create
/// uses the shared heap registry; all other run fields read here are immutable
/// after startup. The run remains owned until these handles have joined.
fn grow(run: *mut RunShared, execs: &Execs, compute: &mut ComputeWorkers, width: u32) {
    for index in compute.active.len()..width as usize {
        // SAFETY: the run outlives each thread, and this thread holds its run token.
        let worker = unsafe { Worker::create(run) };
        let exec = Exec::new(Kind::Compute(index as u16), worker, false);
        execs
            .holders
            .workers
            .lock()
            .unwrap()
            .push(Arc::downgrade(&exec));
        execs.holders.count.fetch_add(1, Ordering::AcqRel);
        unsafe { (&(*run).control).live_width.fetch_add(1, Ordering::AcqRel) };
        let thread = std::thread::Builder::new()
            .name(format!("hirsel-worker-{index}"))
            .stack_size(2 * 1024 * 1024)
            .spawn({
                let (exec, holders, run) =
                    (Arc::clone(&exec), Arc::clone(&execs.holders), RunPtr(run));
                move || worker_main(run, exec, holders)
            })
            .unwrap_or_else(|_| environmental("a compute worker could not start"));
        compute.active.push(exec);
        compute.threads.push(thread);
    }
}

/// Apply a host request at a scheduling boundary. Surplus workers receive Exit
/// after their outgoing handoff; no C frame or pure segment is preempted. Keep
/// retired execs alive until shutdown, so a task's last-worker pointer stays valid.
fn resize(ctx: *mut TaskCtx) {
    if let Some(capacity) = unsafe { (*ctx).run_ref().capacity.as_ref() } {
        capacity.refresh();
    }
    let run = unsafe { (*ctx).run_ptr() };
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    let width = unsafe { (&(*run).control).width.load(Ordering::Acquire) };
    if width == 0 {
        return;
    }
    let mut compute = execs.compute.lock().unwrap();
    if compute.active.is_empty() {
        return;
    }
    let before = compute.active.len();
    if execs.home_at_one && !execs.demand {
        let on_home = compute.active[0].kind == Kind::Home;
        if on_home && width > 1 {
            compute.active.clear();
            unsafe { (&(*run).control).live_width.fetch_sub(1, Ordering::AcqRel) };
        } else if !on_home && width == 1 {
            for exec in &compute.active {
                exec.post(Post::Exit, None);
            }
            let retiring = std::mem::take(&mut compute.active);
            compute.retiring.extend(retiring);
            compute.active.push(Arc::clone(&execs.home));
            unsafe { (&(*run).control).live_width.fetch_add(1, Ordering::AcqRel) };
        }
    }
    if compute.active.len() > width as usize {
        let retiring = compute.active.split_off(width as usize);
        for exec in &retiring {
            exec.post(Post::Exit, None);
        }
        compute.retiring.extend(retiring);
    }
    // A retired worker is joined and orphaned only after leaving its idle fiber.
    // No wait on a still-running segment and no stop-the-world barrier.
    let mut i = 0;
    while i < compute.threads.len() {
        if compute.threads[i].is_finished() {
            let thread = compute.threads.swap_remove(i);
            thread
                .join()
                .unwrap_or_else(|_| fault("a retiring worker panicked"));
        } else {
            i += 1;
        }
    }
    for exec in &compute.retiring {
        if !exec.busy() && exec.exited.load(Ordering::Acquire) {
            let worker = exec.worker.replace(std::ptr::null_mut());
            if !worker.is_null() {
                // SAFETY: exited is published after the worker's last access.
                unsafe { Worker::retire(worker) };
            }
        }
    }
    if !execs.demand {
        grow(run, execs, &mut compute, width);
    }
    if compute.active.len() != before {
        free::set_width(execs, compute.active.len());
    }
}

/// Post the root task `task` (suspended at its row) to the first compute worker.
///
/// # Safety
/// The run has compute workers and has not started.
pub(crate) unsafe fn start_root(run: *mut RunShared, task: crate::tasks::registry::TaskKey) {
    // SAFETY: caller contract.
    let execs = unsafe { (*(*run).execs.get()).as_deref() }
        .unwrap_or_else(|| fault("a run without workers"));
    if execs.dispatch.get().is_some() {
        // SAFETY: root admission owns the scheduler, before its first dispatch.
        unsafe { free::schedule_saved(run, task, Owner::Any) };
        return;
    }
    execs.compute.lock().unwrap().active[0].post(
        Post::Run {
            task: task.slot,
            permit: true,
        },
        None,
    );
}

/// The home thread's service loop at width > 1: it runs home intervals, the Io binding's
/// work and calls for the compute workers, until the run stops or `finished` holds. Its
/// stack is the home thread's idle fiber. Then its inbox closes, so no thread posts to it
/// again: the threads still holding the run, which [`busy`] counts, leave it at their next
/// runtime entry.
///
/// # Safety
/// The calling thread is `run`'s home thread, at width > 1, on the launcher's stack.
pub(crate) unsafe fn serve_home(run: *mut RunShared, finished: &dyn Fn() -> bool) {
    // SAFETY: caller contract.
    let home = unsafe { Arc::clone(&(*(*run).execs.get()).as_deref().unwrap().home) };
    // Saved-frame cleanup and immediate-child lending need this idle Exec,
    // including when home itself is the single compute worker.
    let enclosing_exec = EXEC.with(|e| e.replace(Arc::as_ptr(&home)));
    // SAFETY: the home thread, idle: home registers once and never waits for a slot.
    unsafe { home.recording.register(&*run, recorder_slot(), false) };
    loop {
        // SAFETY: this is the home thread, idle on its own stack.
        unsafe { post_deferred(run, &home) };
        unsafe { free::saved(run, &home) };
        // SAFETY: the run is live.
        if unsafe { (*run).control.stopped().is_some() }
            || (finished()
                && busy(unsafe { &*run }) == 0
                && !free::pending(unsafe { (*run).execs.as_deref().unwrap() }))
        {
            break;
        }
        if unsafe { crate::runners::native::service(run) } {
            continue;
        }
        if unsafe { crate::tasks::ordered::home::service(run) }
            || unsafe { crate::tasks::free::home::service(run) }
        {
            continue;
        }
        let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
        if let Some(saved) = free::next(execs, &home) {
            unsafe { free::resume(run, &home, saved) };
            continue;
        }
        match home.take().0 {
            Post::Run { task, permit } => {
                // SAFETY: the post names a suspended task.
                unsafe { resume(run, &home, task, permit) }
            }
            Post::Saved(saved) => unsafe { free::resume(run, &home, saved) },
            // SAFETY: a call's closure waits for it.
            Post::Call(call) => unsafe { run_call(call) },
            Post::Wake => {}
            Post::Exit => fault("the home thread was asked to exit"),
        }
    }
    EXEC.with(|e| e.set(enclosing_exec));
    unsafe { close_home(run) };
}

/// Close and withdraw a polled run's home bridge before dropping its adapter.
/// # Safety
/// Its sole home owner calls this once, after completion or a published stop.
pub(crate) unsafe fn close_home(run: *mut RunShared) {
    unsafe {
        (*run)
            .control
            .set_worker_phase(hirsel_run_config::managed_cores::WorkerPhase::Draining)
    };
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    let home = &execs.home;
    if home_computes(execs) {
        unsafe { (&(*run).control).live_width.fetch_sub(1, Ordering::AcqRel) };
    }
    home.close();
    // Close uses the same claim/withdraw protocol as R6's request bridge. A
    // queued call is withdrawn before home state is destroyed; its caller still
    // owns the borrowed buffers until the completion below wakes it.
    unsafe {
        (*(*run).execs.get()).as_deref().unwrap().calls.sweep();
    }
    loop {
        let post = home.inbox.lock().unwrap().posts.pop_front();
        match post {
            // SAFETY: as above.
            Some((Post::Call(call), _)) => unsafe { run_call(call) },
            Some(_) => {}
            None => break,
        }
    }
    // SAFETY: the home thread after its last call; its slot leaves with the run.
    unsafe { home.recording.retire(&*run) };
}

/// Publish an execution's shared readiness before this child starts. Posts
/// (including the completion post after physical save) wake the same home
/// driver as the child's host interests.
/// # Safety
/// Sole home admission, after topology creation and before root publication.
pub(crate) unsafe fn home_driver(run: *mut RunShared, driver: crate::readiness::Readiness) {
    driver.notify().expect("an execution home wake descriptor");
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    assert!(execs.home.home_driver.set(driver).is_ok());
}

/// # Safety
/// As home_driver. The signal owns no adapter or context borrow and may
/// outlive the home owner when a stopped child still computes.
pub(crate) unsafe fn home_signal(run: *mut RunShared, signal: Arc<crate::readiness::Signal>) {
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    assert!(execs.home.home_signal.set(signal).is_ok());
}

/// Service one already-published home continuation without blocking on an
/// inbox, permit or host readiness source. A multi-run home owner polls each
/// run's messages before choosing an execution-level wait.
/// # Safety
/// Called on this run's home thread, outside program code and scheduler roles.
/// No other home loop serves this run; the caller retains its context.
pub(crate) unsafe fn poll_home_posts(run: *mut RunShared) -> bool {
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    let home = &execs.home;
    let enclosing = EXEC.with(|e| e.replace(Arc::as_ptr(home)));
    // SAFETY: as in serve_home.
    unsafe { home.recording.register(&*run, recorder_slot(), false) };
    unsafe { post_deferred(run, home) };
    unsafe { free::saved(run, home) };
    let progress = if let Some(saved) = free::next(execs, home) {
        unsafe { free::resume(run, home, saved) };
        true
    } else {
        let post = home.inbox.lock().unwrap().posts.pop_front();
        if let Some((post, _)) = post {
            match post {
                Post::Run { task, permit } => unsafe { resume(run, home, task, permit) },
                Post::Saved(saved) => unsafe { free::resume(run, home, saved) },
                Post::Call(call) => unsafe { run_call(call) },
                Post::Wake => {}
                Post::Exit => fault("the home thread was asked to exit"),
            }
            true
        } else {
            false
        }
    };
    EXEC.with(|e| e.set(enclosing));
    progress
}

/// Outstanding physical saved-frame ownership, used by polled root launchers
/// before they publish normal completion to their home-owned execution.
pub(crate) fn saved_pending(run: &RunShared) -> bool {
    run.execs.as_deref().is_some_and(free::pending)
}

/// The compute and pool threads of `run` that hold it: a stopped run's workers still inside
/// it (`reclamation_pending`).
pub(crate) fn busy(run: &RunShared) -> u32 {
    let Some(execs) = run.execs.as_deref() else {
        return 0;
    };
    let _activity = execs.activity.lock().unwrap();
    let compute = execs.compute.lock().unwrap();
    let compute = compute
        .active
        .iter()
        .chain(&compute.retiring)
        .filter(|exec| exec.busy())
        .count();
    let pool = execs
        .pool
        .lock()
        .unwrap()
        .iter()
        .filter(|thread| thread.exec.busy())
        .count();
    (compute + pool + cpu_scoped::busy(execs)) as u32
}

/// Run `call` on the home thread: at once on home and in a run without compute workers;
/// otherwise the calling thread, which holds the run, posts it and waits. A stopped run's
/// home thread takes no call: its stop.
/// The synchronous loan retains the caller's compute permit. A genuinely
/// blocking wait releases it explicitly before calling (host_waits::wait).
///
/// # Safety
/// `ctx` is live; `call` does not suspend and holds nothing of the calling thread's.
pub(crate) unsafe fn call_home(ctx: *mut TaskCtx, call: &mut dyn FnMut()) -> Stopped<()> {
    unsafe { call_home_in((*ctx).run_ptr(), call) }
}

/// As call_home, with an explicit run for a facade whose caller may be home.
pub(crate) unsafe fn call_home_in(run: *mut RunShared, call: &mut dyn FnMut()) -> Stopped<()> {
    crate::tasks::role::unlent("a home call");
    // SAFETY: the facade is owned by this live run.
    let run = unsafe { &*run };
    let caller = own_exec();
    if caller.is_null() || unsafe { (*caller).kind } == Kind::Home {
        call();
        return Ok(());
    }
    if let Some(stop) = run.control.stopped() {
        return Err(stop);
    }
    let done = Done::default();
    let execs = run.execs.as_deref().unwrap();
    let id = execs.next_call.fetch_add(1, Relaxed);
    let claim = execs.calls.prepare(id, ());
    // SAFETY: this frame waits until the home thread signals `done`, so the closure outlives
    // its run there.
    let call: *mut (dyn FnMut() + 'static) = unsafe { std::mem::transmute(call) };
    let home = &run.execs.as_deref().unwrap().home;
    if !home.post(
        Post::Call(Call {
            claim,
            run: call,
            done: &done,
        }),
        None,
    ) {
        execs.calls.retire(id);
        return Err(run.control.stopped().unwrap_or(Stop::Requested));
    }
    run.role.notify_home_call();
    let mut ended = done.ended.lock().unwrap();
    let exec = unsafe { &*current(execs) };
    let held = exec.held.load(Ordering::Relaxed);
    // The caller can lend R and scheduler borrows to this synchronous loan.
    // Returning C here would require R -> C on return, while another selected
    // worker can hold C waiting for R. Keep the existing permit, as for the
    // dedicated commit loan; home serves Call without acquiring either C or R.
    while ended.is_none() {
        ended = done.bell.wait(ended).unwrap();
    }
    let claimed = ended.unwrap();
    drop(ended);
    execs.calls.retire(id);
    if !claimed {
        return Err(run.control.stopped().unwrap_or(Stop::Requested));
    }
    // The old permit reacquisition observed a stop after the borrowed callback
    // completed. Keep that boundary, even though no permit wait is needed.
    if held && let Some(stop) = run.control.stopped() {
        return Err(stop);
    }
    Ok(())
}

/// Wake `exec`'s thread, the home thread of a run whose stop was requested from outside
/// its code: its loop sees the stop.
pub(crate) fn wake(exec: &Exec) {
    exec.post(Post::Wake, None);
}

/// Notify home that a saved Ordered service continuation can be retried.
/// # Safety
/// The run retains its execs for this callback; no activity lock is held.
pub(crate) unsafe fn wake_home(run: *mut RunShared) {
    if let Some(io) = unsafe { (*run).ordered_io.get() } {
        io.notify();
    }
    if let Some(io) = unsafe { (*run).free_io.get() } {
        io.notify();
    }
    if let Some(execs) = unsafe { (*(*run).execs.get()).as_deref() } {
        wake(&execs.home);
    }
}

/// Wake home from a thread that may outlive the run (an owned host job). It
/// retains only the run's control, which learns the home Exec when the pool
/// starts (admission installs services before that) and whose closed inbox
/// ignores late posts.
/// # Safety
/// The run is live while this is called.
pub(crate) unsafe fn home_wake(run: *mut RunShared) -> Option<Arc<dyn Fn() + Send + Sync>> {
    let control = Arc::clone(unsafe { &(*run).control });
    Some(Arc::new(move || control.wake_home()))
}

/// A pool slot for an interval: an idle pool thread, a new one, or `None` when all
/// [`POOL_THREADS`] are busy.
pub(crate) fn claim(ctx: *mut TaskCtx) -> Option<u16> {
    ensure_compute_grant();
    if let Some(capacity) = unsafe { (*ctx).run_ref().capacity.as_ref() } {
        capacity.refresh();
    }
    unsafe {
        crate::io::Binding::bridge(ctx);
    }
    let run = RunPtr(
        // SAFETY: the context is live.
        unsafe { (*ctx).run_ptr() },
    );
    let execs = execs(ctx);
    let mut pool = execs.pool.lock().unwrap();
    if let Some(slot) = pool.iter().position(|thread| !thread.busy) {
        pool[slot].busy = true;
        return Some(slot as u16);
    }
    if pool.len() >= POOL_THREADS {
        return None;
    }
    let slot = pool.len() as u16;
    // SAFETY: the run outlives the worker, which only the new thread uses from here on.
    let worker = unsafe { Worker::create(run.0) };
    let exec = Exec::new(Kind::Pool(slot), worker, false);
    execs
        .holders
        .workers
        .lock()
        .unwrap()
        .push(Arc::downgrade(&exec));
    execs.holders.count.fetch_add(1, Ordering::AcqRel);
    let thread = std::thread::Builder::new()
        .name(format!("hirsel-pool-{slot}"))
        .stack_size(2 * 1024 * 1024)
        .spawn({
            let (exec, holders) = (Arc::clone(&exec), Arc::clone(&execs.holders));
            move || worker_main(run, exec, holders)
        })
        .unwrap_or_else(|_| environmental("a blocking-pool thread could not start"));
    pool.push(PoolThread {
        exec,
        thread: Some(thread),
        busy: true,
    });
    Some(slot)
}

/// The interval on pool slot `slot` ended: the slot takes another.
pub(crate) fn release(ctx: *mut TaskCtx, slot: u16) {
    execs(ctx).pool.lock().unwrap()[slot as usize].busy = false;
}

/// Whether some pool slot is free or can still start.
pub(crate) fn has_slot(ctx: *mut TaskCtx) -> bool {
    unsafe { has_slot_run((*ctx).run_ptr()) }
}

/// A bounded capacity snapshot, without a host bridge, task lookup or allocation.
/// # Safety
/// The run retains its topology through this idle/runtime callback.
pub(crate) unsafe fn has_slot_run(run: *mut RunShared) -> bool {
    let Some(execs) = (unsafe { (*(*run).execs.get()).as_deref() }) else {
        return true;
    };
    let pool = execs.pool.lock().unwrap();
    pool.len() < POOL_THREADS || pool.iter().any(|thread| !thread.busy)
}

/// The calling thread's compute permit goes back: the task it runs enters C.
pub(crate) fn release_permit(ctx: *mut TaskCtx) {
    if let Some(capacity) = unsafe { (*ctx).run_ref().capacity.as_ref() } {
        capacity.refresh();
    }
    // SAFETY: the context is live.
    if let Some(execs) = unsafe { (*(*(*ctx).run_ptr()).execs.get()).as_deref() } {
        // SAFETY: the current exec is this thread's.
        execs.permits.release(unsafe { &*current(execs) });
    }
}

/// The calling thread takes a compute permit unless it holds one, as a callback's Hirsel code
/// begins: whether it took one. Before a width-1 run's first dispatch its one thread holds
/// the run's one permit.
pub(crate) fn acquire_permit(ctx: *mut TaskCtx) -> bool {
    if let Some(capacity) = unsafe { (*ctx).run_ref().capacity.as_ref() } {
        capacity.refresh();
    }
    let execs = execs(ctx);
    // A callee belongs to its execution root's stop, while using its own
    // executor and heap on this physical thread.
    let run = unsafe { (*ctx).run_ptr() };
    let control = unsafe {
        &*if (*run).root.is_null() {
            run
        } else {
            (*run).root
        }
    };
    let result = execs.permits.acquire(
        unsafe { &*current(execs) },
        Some(&control.control),
        control.worker_metrics.as_deref(),
    );
    // SAFETY: the caller is at a supported code-entry boundary.
    unsafe { crate::stop::check(ctx, result) }
}

/// Enter ordinary width-1 code on the admission thread. The launcher's frame
/// returns the permit even when a stop abandons every program frame.
pub(crate) fn enter_root(ctx: *mut TaskCtx) -> Stopped<()> {
    if let Some(capacity) = unsafe { (*ctx).run_ref().capacity.as_ref() } {
        capacity.refresh();
    }
    let execs = execs(ctx);
    execs.permits.acquire(
        &execs.home,
        Some(unsafe { &(*(*ctx).run_ptr()).control }),
        unsafe { (*(*ctx).run_ptr()).worker_metrics.as_deref() },
    )?;
    unsafe {
        (&(*(*ctx).run_ptr()).control)
            .live_width
            .store(1, Ordering::Release)
    };
    unsafe {
        (*(*ctx).run_ptr())
            .control
            .set_worker_phase(hirsel_run_config::managed_cores::WorkerPhase::Running)
    };
    Ok(())
}
pub(crate) fn leave_root(ctx: *mut TaskCtx) {
    let execs = execs(ctx);
    unsafe {
        (*(*ctx).run_ptr())
            .control
            .set_worker_phase(hirsel_run_config::managed_cores::WorkerPhase::Draining)
    };
    execs.permits.release(&execs.home);
    unsafe {
        (&(*(*ctx).run_ptr()).control)
            .live_width
            .store(0, Ordering::Release)
    };
}

/// The run ended and every one of its threads is idle: each leaves and is joined. Their
/// workers stay, for the caller to read and then retire ([`retire`]); the run's root
/// context goes back to the home worker, which holds the run's storage for its exit.
pub(crate) fn shutdown(run: &RunShared) {
    run.control
        .set_worker_phase(hirsel_run_config::managed_cores::WorkerPhase::Draining);
    let Some(execs) = run.execs.as_deref() else {
        return;
    };
    execs.home.close();
    if let Some(cleanup) = execs.scoped_cleanup.get() {
        (cleanup.shutdown)(execs);
    }
    let mut compute = execs.compute.lock().unwrap();
    for exec in compute.active.iter().chain(&compute.retiring) {
        exec.post(Post::Exit, None);
    }
    let compute_threads = std::mem::take(&mut compute.threads);
    drop(compute);
    for thread in compute_threads {
        if thread.join().is_err() {
            fault("a compute worker panicked");
        }
    }
    let pool_threads: Vec<_> = execs
        .pool
        .lock()
        .unwrap()
        .iter_mut()
        .filter_map(|thread| {
            thread.exec.post(Post::Exit, None);
            thread.thread.take()
        })
        .collect();
    for thread in pool_threads {
        if thread.join().is_err() {
            fault("a blocking-pool thread panicked");
        }
    }
}

/// The workers of `run`'s compute and pool threads.
pub(crate) fn workers(run: &RunShared) -> Vec<*mut Worker> {
    let Some(execs) = run.execs.as_deref() else {
        return Vec::new();
    };
    let compute = execs.compute.lock().unwrap();
    compute
        .active
        .iter()
        .chain(&compute.retiring)
        .filter(|exec| exec.kind != Kind::Home)
        .map(|exec| exec.worker.get())
        .filter(|worker| !worker.is_null())
        .chain(
            execs
                .pool
                .lock()
                .unwrap()
                .iter()
                .map(|thread| thread.exec.worker.get()),
        )
        .chain(cpu_scoped::workers(execs))
        .collect()
}

/// After [`shutdown`]: the compute and pool workers retire, orphaning their heaps into the
/// run's registry, and the run's execs go.
pub(crate) fn retire(run: &mut RunShared) {
    for worker in workers(run) {
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        // SAFETY: the threads have left; keep run-level immortalization
        // accounting after their worker-local records go away.
        unsafe {
            (*run.home_worker()).immortalized += (*worker).immortalized;
        }

        // SAFETY: the worker's thread left; the task last bound to it was switched out.
        unsafe { Worker::retire(worker) };
    }
    *run.execs = None;
}

/// The launcher leaves a stopped run whose workers still hold it: every thread leaves at its
/// next post (a busy one once it abandons its task), and the last to leave frees the run,
/// its root context and its workers ([`crate::context::reclaim`]). False when no thread is
/// left, so the launcher frees it now.
pub(crate) fn detach(run: &RunShared) -> bool {
    let Some(execs) = run.execs.as_deref() else {
        return false;
    };
    execs.home.close();
    if let Some(cleanup) = execs.scoped_cleanup.get() {
        (cleanup.request_exit)(execs);
    }
    let compute = execs.compute.lock().unwrap();
    for exec in compute.active.iter().chain(&compute.retiring) {
        exec.post(Post::Exit, None);
    }
    for thread in execs.pool.lock().unwrap().iter() {
        thread.exec.post(Post::Exit, None);
    }
    let holders = Arc::clone(&execs.holders);
    drop(compute);
    {
        let mut state = execs.permits.state.lock().unwrap();
        for exec in holders
            .workers
            .lock()
            .unwrap()
            .iter()
            .filter_map(std::sync::Weak::upgrade)
        {
            if (exec.held.load(Ordering::Acquire) || exec.reserved.load(Ordering::Acquire))
                && !exec.lost.swap(true, Ordering::AcqRel)
            {
                state.lost += 1;
            }
        }
    }
    holders.detached.store(true, Ordering::Release);
    holders.count.fetch_sub(1, Ordering::AcqRel) != 1
}

/// Drop the threads' handles of a detached run (they are never joined) and retire its
/// workers: called by its last thread, which no longer runs any of them.
pub(crate) fn forget(run: &mut RunShared) {
    let Some(mut execs) = run.execs.take() else {
        return;
    };
    let scoped = cpu_scoped::forget(&mut execs);
    let compute = execs.compute.get_mut().unwrap();
    compute.threads.clear();
    let workers: Vec<_> = compute
        .active
        .iter()
        .chain(&compute.retiring)
        .filter(|exec| exec.kind != Kind::Home)
        .map(|exec| exec.worker.get())
        .chain(execs.pool.get_mut().unwrap().iter_mut().map(|thread| {
            thread.thread.take();
            thread.exec.worker.get()
        }))
        .chain(scoped)
        .collect();
    for worker in workers.into_iter().filter(|worker| !worker.is_null()) {
        // SAFETY: every thread of the run left.
        unsafe { Worker::retire(worker) };
    }
}

/// # Safety
/// Sole home owner, no inbox loan escapes this readiness test.
pub(crate) unsafe fn home_posts(run: *mut RunShared) -> bool {
    let execs = unsafe { (*(*run).execs.get()).as_deref().unwrap() };
    !execs.home.inbox.lock().unwrap().posts.is_empty()
}

#[cfg(test)]
#[path = "pool/modules.rs"]
mod audit_modules;

#[cfg(test)]
mod progress_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_role_merge_serves_a_bridge_from_the_role_owner() {
        const CHILD: &str = "HIRSEL_HOME_ROLE_BRIDGE_TEST";
        if std::env::var_os(CHILD).is_none() {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "pool::tests::home_role_merge_serves_a_bridge_from_the_role_owner",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .spawn()
                .unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "bridge child: {status}");
                    return;
                }
                if Instant::now() >= until {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("home waited for the role while its owner waited for a home call");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        for queued_before_merge in [false, true] {
            let mut ctx = std::ptr::null_mut();
            unsafe {
                assert_eq!(
                    crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                    0
                );
                crate::tasks::role::enable(ctx);
                init_execs(ctx);
            }
            let address = ctx.addr();
            let run = unsafe { (*ctx).run_ptr() };
            let (held_tx, held_rx) = std::sync::mpsc::channel();
            let called = AtomicBool::new(false);
            std::thread::scope(|scope| {
                let called = &called;
                let worker = scope.spawn(move || unsafe {
                    let ctx = address as *mut TaskCtx;
                    let run = (*ctx).run_ptr();
                    let exec = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
                    EXEC.with(|e| e.set(Arc::as_ptr(&exec)));
                    crate::tasks::role::enter(ctx);
                    held_tx.send(()).unwrap();
                    if !queued_before_merge {
                        // Force publication after home's queue check / wait
                        // subscription, independent of OS scheduling speed.
                        while !(*run).role.home_waiting() {
                            std::thread::yield_now();
                        }
                    }
                    call_home_in(run, &mut || called.store(true, Ordering::Release)).unwrap();
                    crate::tasks::role::release(ctx);
                    EXEC.with(|e| e.set(std::ptr::null()));
                });
                held_rx.recv().unwrap();
                if queued_before_merge {
                    while unsafe { (*(*run).execs.get()).as_deref().unwrap() }
                        .home
                        .inbox
                        .lock()
                        .unwrap()
                        .posts
                        .is_empty()
                    {
                        std::thread::yield_now();
                    }
                }
                let guard = unsafe { crate::tasks::role::IdleGuard::home(run) };
                assert!(called.load(Ordering::Acquire));
                drop(guard);
                worker.join().unwrap();
            });
            unsafe { crate::context::hirsel_rt_v2_ctx_destroy(ctx) };
        }
    }

    #[test]
    fn a_detached_run_stays_pending_until_its_storage_is_reclaimed() {
        let holders = Holders {
            count: AtomicU32::new(1),
            detached: AtomicBool::new(true),
            reclaimed: AtomicBool::new(false),
            workers: Mutex::new(Vec::new()),
        };
        assert_eq!(holders.left(), 1);
        // The last worker has left code, but still owns the final drop of
        // task contexts, recorder owners, stacks and heaps.
        holders.count.fetch_sub(1, Ordering::AcqRel);
        assert_eq!(holders.left(), 1);
        holders.reclaimed();
        assert_eq!(holders.left(), 0);
    }

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
    fn the_scheduling_rows_leave_out_what_they_could_not_measure() {
        let metrics = WorkerMetrics::default();
        let names = |m: &WorkerMetrics| m.values().into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names(&metrics), ["migrations_per_s"]);
        metrics.op();
        metrics.op();
        metrics.switch();
        metrics.queue_op();
        metrics.queue_op();
        metrics.queue_op();
        metrics.spawn(Duration::from_nanos(30));
        metrics.spawn(Duration::from_nanos(10));
        metrics.stack_rss(8192);
        metrics.refresh(Duration::from_nanos(12));
        let values: std::collections::BTreeMap<_, _> = metrics.values().into_iter().collect();
        assert_eq!(values["switches_per_op"], 0.5);
        assert_eq!(values["queue_ops_per_op"], 1.5);
        assert_eq!(values["spawn_ns"], 20.0);
        assert_eq!(values["rss_bytes_per_task"], 4096.0);
        assert_eq!(values["refresh_ns"], 12.0);
    }

    #[test]
    fn a_thread_holding_a_permit_takes_none_and_gives_back_only_what_it_holds() {
        let permits = Permits::new(1);
        let exec = Exec::new(Kind::Pool(0), std::ptr::null_mut(), false);
        assert!(permits.acquire(&exec, None, None).unwrap());
        assert!(!permits.acquire(&exec, None, None).unwrap());
        assert_eq!(permits.available(), 0);
        permits.release(&exec);
        permits.release(&exec);
        assert_eq!(permits.available(), 1);
    }

    /// The 128-op quantum's hand-off (`free::quantum_tests`): a worker that
    /// releases its only permit and asks again at once queues behind the
    /// worker its release woke, whatever the threads' timing.
    #[test]
    fn a_released_permit_goes_to_its_waiter_before_its_releaser_takes_it_back() {
        let permits = Arc::new(Permits::new(1));
        let order = Arc::new(Mutex::new(Vec::new()));
        let root = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
        assert!(permits.acquire(&root, None, None).unwrap());
        let waiter = {
            let (permits, order) = (permits.clone(), order.clone());
            std::thread::spawn(move || {
                let exec = Exec::new(Kind::Compute(1), std::ptr::null_mut(), false);
                assert!(permits.acquire(&exec, None, None).unwrap());
                order.lock().unwrap().push("waiter");
                permits.release(&exec);
            })
        };
        while permits.state.lock().unwrap().waiting == 0 {
            std::thread::yield_now();
        }
        permits.release(&root);
        assert!(permits.acquire(&root, None, None).unwrap());
        order.lock().unwrap().push("releaser");
        permits.release(&root);
        waiter.join().unwrap();
        assert_eq!(*order.lock().unwrap(), ["waiter", "releaser"]);
        assert_eq!(permits.available(), 1);
    }

    #[test]
    fn a_closed_inbox_takes_nothing_and_a_task_posted_makes_its_exec_busy() {
        let exec = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
        assert!(!exec.busy());
        assert!(exec.post(Post::Wake, None));
        assert!(!exec.busy());
        assert!(exec.post(
            Post::Run {
                task: 1,
                permit: true
            },
            None
        ));
        assert!(exec.busy());
        assert!(matches!(exec.take().0, Post::Wake));
        exec.close();
        assert!(!exec.post(Post::Exit, None));
        assert!(matches!(exec.take().0, Post::Run { task: 1, .. }));
    }
}
