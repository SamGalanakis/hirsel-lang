//! Ending one run without ending the process.
//!
//! A run's root task runs on a fiber of its own ([`run_root`]), so the launcher keeps its stack
//! while the run executes. A [`Stop`] (Blocked, a spent budget, an adapter refusal, a replay
//! divergence, a codec limit or a failed serializable check) ends the run from wherever it is raised: the runtime's internal
//! functions return `Result<T, Stop>` up to the entry compiled code called, which checks it
//! ([`check`]) and switches back to the launcher's stack ([`raise`]). The Rust frames below that
//! entry have returned and dropped their locals by then; the entry's own frame and compiled
//! frames hold plain values only, and every fiber of the stopped run is abandoned. The launcher
//! then destroys the run context, which returns the run's storage wholesale, and hands the
//! outcome to its host.
//!
//! Process-fatal by design: a broken invariant ([`crate::fault`]), host exhaustion
//! ([`crate::environmental`]), a PROG-4 violation, a stack overflow, and a stop raised inside a
//! foreign call, whose C frames cannot be abandoned.
use hirsel_run_schema::{BudgetKind, Environmental, RunOutcome};

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::TaskCtx;
use crate::fiber;
use crate::io::AdapterError;

/// Why a run stopped before its entry returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stop {
    /// Every task waits on another and nothing outside the run can wake one (CONC-8).
    Blocked(Vec<hirsel_run_schema::Wait>),
    BudgetExceeded(BudgetKind),
    /// The adapter refused a request.
    Refused(String),
    /// A replay's requests left its recording.
    Diverged(String),
    Environmental(Environmental),
    /// A failure the runtime cannot unwind through: a serializable check failed inside the
    /// codec (FAIL-3). The run ends with its recorded failure, as if its entry had failed,
    /// with no cleanup.
    Failed,
    /// A scripted signal took its default action (`signals.script@1`).
    Signaled(hirsel_run_schema::SignalKind),
    /// The embedding host stopped the run ([`crate::host::RunControl::stop`]).
    Requested,
    /// An unchecked precondition failed; no language failure or cleanup follows.
    UndefinedBehavior(hirsel_run_schema::ExecutionDiagnostic),
}

impl Stop {
    /// The stop an adapter error ends the run with.
    pub(crate) fn refusal(error: AdapterError) -> Stop {
        match error {
            AdapterError::ReplayMismatch(_)
            | AdapterError::MissingInput(_)
            | AdapterError::UnconsumedInput(_) => Stop::Diverged(format!("{error:?}")),
            AdapterError::Signaled(kind) => Stop::Signaled(kind),
            AdapterError::RecordingFailed(_) => {
                Stop::Environmental(Environmental::Recording(format!("{error:?}")))
            }
            AdapterError::Stopped => Stop::Environmental(Environmental::Stopped),
            _ => Stop::Refused(format!("{error:?}")),
        }
    }
    /// The ending a host reports for this stop.
    pub fn outcome(self) -> hirsel_run_schema::RunOutcome {
        use hirsel_run_schema::RunOutcome;
        match self {
            Stop::Blocked(waits) => RunOutcome::Blocked(waits),
            Stop::BudgetExceeded(kind) => RunOutcome::BudgetExceeded(kind),
            Stop::Refused(reason) => RunOutcome::Refused(reason),
            Stop::Diverged(reason) => RunOutcome::Diverged(reason),
            Stop::Environmental(cause) => RunOutcome::Environmental(cause),
            Stop::Failed => crate::fault("a failed run reported as a stop"),
            Stop::Signaled(kind) => RunOutcome::Signaled(kind),
            Stop::UndefinedBehavior(diagnostic) => RunOutcome::UndefinedBehavior(diagnostic),
            Stop::Requested => RunOutcome::Environmental(Environmental::Stopped),
        }
    }
}

thread_local! {
    /// The task running on this thread, for reports raised without a context argument.
    static RUNNING: std::cell::Cell<*mut TaskCtx> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The context of the task running on this thread, or null. Like every thread-local access,
/// it is out of line, so no caller keeps a thread's address across a suspension.
#[inline(never)]
pub(crate) fn running() -> *mut TaskCtx {
    RUNNING.with(std::cell::Cell::get)
}

/// `ctx`'s task runs on this thread from here on (`TaskCtx::switch_in`).
#[inline(never)]
pub(crate) fn set_running(ctx: *mut TaskCtx) {
    RUNNING.with(|r| r.set(ctx));
}

/// A runtime result that may stop the run.
pub(crate) type Stopped<T> = Result<T, Stop>;

/// Where a width-1 run's stop returns: the launcher's suspended stack.
pub(crate) struct Driver {
    sp: *mut u8,
}

impl Default for Driver {
    fn default() -> Self {
        Driver {
            sp: std::ptr::null_mut(),
        }
    }
}

/// A run's stop word and what its host may ask of it from any thread (design §6 Stops and
/// worker lifetime). The first stop requested wins ([`Control::request`]), whichever thread
/// raised it, and every later one takes the winner's place: every runtime entry that is an
/// operation, every switch-in and every fuel refill reads the word with acquire ordering
/// and ends the run's code there ([`observe`]).
#[derive(Default)]
pub(crate) struct Control {
    pub(crate) width: AtomicU32,
    pub(crate) live_width: AtomicU32,
    worker_phase: AtomicU8,
    stopped: AtomicBool,
    stop: Mutex<Option<Stop>>,
    /// Installed at admission. Closing the recorded prefix precedes publication
    /// of the stop, so no waiter can destroy the run before its cutter finishes.
    terminal: Mutex<Option<Arc<dyn Terminal>>>,
    /// The home thread's exec, while a run with compute workers runs: a stop wakes its loop.
    home: Mutex<Option<Arc<crate::pool::Exec>>>,
    /// A host job woke home before the pool published it (`wake_home`).
    home_woken: AtomicBool,
    /// A descriptor a stop makes readable, which ends a width-1 run's idle host wait.
    wake: Mutex<Option<OwnedFd>>,
    /// Workers still inside the stopped run when its launcher returned.
    pending: AtomicU32,
    /// The threads holding a detached run, which count down as they leave it.
    holders: Mutex<Option<Arc<crate::pool::Holders>>>,
    /// The launcher returned the run's outcome.
    finished: AtomicBool,
    /// A host holds a stop handle ([`crate::host::RunControl`]).
    hosted: AtomicBool,
}

/// A terminal observer owns its recording state, never a task context or an
/// adapter borrow. Stops are requested only outside operation sections.
pub(crate) trait Terminal: Send + Sync {
    fn stop(&self, cause: &Stop) -> std::io::Result<()>;
    fn stop_at(
        &self,
        cause: &Stop,
        _task: Option<&[u32]>,
        _outcome: Option<&RunOutcome>,
    ) -> std::io::Result<()> {
        self.stop(cause)
    }
}

impl Control {
    pub(crate) fn worker_phase(&self) -> hirsel_run_config::managed_cores::WorkerPhase {
        use hirsel_run_config::managed_cores::WorkerPhase;
        match self.worker_phase.load(Ordering::Acquire) {
            0 => WorkerPhase::Starting,
            1 => WorkerPhase::Running,
            2 => WorkerPhase::Draining,
            3 => WorkerPhase::Finished,
            _ => unreachable!("invalid worker phase"),
        }
    }

    pub(crate) fn set_worker_phase(&self, phase: hirsel_run_config::managed_cores::WorkerPhase) {
        self.worker_phase.fetch_max(phase as u8, Ordering::Release);
    }

    /// Admission only, before any worker can enter a recording section. An
    /// embedding host may already have stopped its not-yet-started run.
    pub(crate) fn terminal(&self, terminal: Arc<dyn Terminal>) {
        let mut won = self.stop.lock().unwrap();
        let mut admitted = self.terminal.lock().unwrap();
        assert!(
            !self.finished.load(Ordering::Acquire),
            "terminal admission after finish"
        );
        assert!(admitted.is_none(), "terminal admitted twice");
        if let Some(stop) = &*won
            && let Err(error) = terminal.stop(stop)
        {
            *won = Some(Stop::Environmental(Environmental::Recording(
                error.to_string(),
            )));
        }
        *admitted = Some(terminal);
    }

    /// Request `stop`: the run's stop, unless another won first. The run's waits wake.
    pub(crate) fn request(&self, stop: Stop) -> Stop {
        self.decide(stop, true, None, None)
    }
    /// Request `stop` from the thread running the run's code, which then leaves its task and
    /// wakes the home loop itself once idle (`crate::pool::abandon`): the loop then finds the
    /// thread no longer holding the run.
    pub(crate) fn request_inside(&self, stop: Stop) -> Stop {
        self.decide(stop, false, None, None)
    }
    fn decide(
        &self,
        stop: Stop,
        wake_home: bool,
        task: Option<&[u32]>,
        outcome: Option<&RunOutcome>,
    ) -> Stop {
        let mut won = self.stop.lock().unwrap();
        if let Some(winner) = &*won {
            return winner.clone();
        }
        // Keep first-winner arbitration locked until the observer closes its
        // prefix. Other callers cannot return an outcome while the cutter is
        // still scanning. The fast stop bit stays clear during this work:
        // bounded sections may finish without waiting on this mutex.
        let stop = match self
            .terminal
            .lock()
            .unwrap()
            .as_ref()
            .map(|terminal| terminal.stop_at(&stop, task, outcome))
        {
            Some(Err(error)) => Stop::Environmental(Environmental::Recording(error.to_string())),
            _ => stop,
        };
        *won = Some(stop.clone());
        self.stopped.store(true, Ordering::SeqCst);
        drop(won);
        crate::pool::wake_permits();
        if wake_home && let Some(home) = &*self.home.lock().unwrap() {
            crate::pool::wake(home);
        }
        if let Some(fd) = &*self.wake.lock().unwrap() {
            let one = 1u64;
            // SAFETY: an eventfd takes an 8-byte count; a full counter is already readable.
            let _ = unsafe { libc::write(fd.as_raw_fd(), (&raw const one).cast(), 8) };
        }
        stop
    }
    /// The run's stop, once one was requested.
    #[inline]
    pub(crate) fn stopped(&self) -> Option<Stop> {
        if !self.stopped.load(Ordering::Acquire) {
            return None;
        }
        self.stop.lock().unwrap().clone()
    }
    /// A host took a stop handle: a stop may come from outside the run.
    pub(crate) fn host(&self) {
        self.hosted.store(true, Ordering::SeqCst);
    }
    pub(crate) fn hosted(&self) -> bool {
        self.hosted.load(Ordering::Relaxed)
    }
    pub(crate) fn set_home(&self, home: Arc<crate::pool::Exec>) {
        let mut slot = self.home.lock().unwrap();
        if self.home_woken.swap(false, Ordering::Relaxed) {
            crate::pool::wake(&home);
        }
        *slot = Some(home);
    }
    /// Wake the home exec. A wake before the pool publishes home (workers may
    /// already run jobs) is kept under the same lock and posted by set_home.
    pub(crate) fn wake_home(&self) {
        match &*self.home.lock().unwrap() {
            Some(home) => crate::pool::wake(home),
            None => self.home_woken.store(true, Ordering::Relaxed),
        }
    }
    /// The descriptor a stop makes readable, created at the first ask.
    pub(crate) fn wake_fd(&self) -> Option<std::os::fd::RawFd> {
        let mut wake = self.wake.lock().unwrap();
        if wake.is_none() {
            // SAFETY: a fresh nonblocking eventfd, owned from here on.
            let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
            if fd < 0 {
                return None;
            }
            // SAFETY: as above.
            *wake = Some(unsafe { OwnedFd::from_raw_fd(fd) });
            if self.stopped.load(Ordering::SeqCst) {
                let one = 1u64;
                // SAFETY: as in `request`.
                let _ = unsafe { libc::write(fd, (&raw const one).cast(), 8) };
            }
        }
        wake.as_ref().map(|fd| fd.as_raw_fd())
    }
    /// The launcher returned, leaving `pending` workers inside the run.
    pub(crate) fn finish(&self, pending: u32) {
        // A retained stop handle must not rewrite a completed tape or keep
        // its recording buffers alive after the launcher returned. Synchronize
        // with an in-progress terminal callback in the same lock order.
        let _stop = self.stop.lock().unwrap();
        self.terminal.lock().unwrap().take();
        self.pending.store(pending, Ordering::SeqCst);
        self.finished.store(true, Ordering::SeqCst);
        self.set_worker_phase(hirsel_run_config::managed_cores::WorkerPhase::Finished);
    }
    /// The launcher leaves the run to `holders`, its threads: the status counts them down.
    pub(crate) fn track(&self, holders: Arc<crate::pool::Holders>) {
        *self.holders.lock().unwrap() = Some(holders);
    }
    /// What a host asks of a run from any thread.
    pub(crate) fn status(&self) -> crate::host::RunStatus {
        let finished = self.finished.load(Ordering::SeqCst);
        let holders = self.holders.lock().unwrap();
        let (pending, lost) = match holders.as_ref() {
            Some(holders) if finished => (holders.left(), holders.lost_permits()),
            _ => (self.pending.load(Ordering::SeqCst), 0),
        };
        crate::host::RunStatus {
            stop_requested: self.stopped.load(Ordering::SeqCst),
            home_busy: self
                .home
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|home| home.busy()),
            finished,
            reclamation_pending: pending,
            lost_permits: lost,
        }
    }
}

/// End the run's code here when its stop was requested: the next runtime entry that is an
/// operation, switch-in or fuel refill of every worker observes it.
#[inline]
pub(crate) fn observe(ctx: *mut TaskCtx) -> Stopped<()> {
    // SAFETY: the context and its run are live; a callee run's root outlives it, and the
    // root's stop ends the whole execution (RUN-10).
    let run = unsafe {
        let run = (*ctx).run_ptr();
        &*if (*run).root.is_null() {
            run
        } else {
            (*run).root
        }
    };
    match run.control.stopped() {
        None => Ok(()),
        // A run no launcher drives any more (its exit's releases) runs on to its end.
        Some(_) if run.driver.sp.is_null() && !crate::pool::wide(run) => Ok(()),
        Some(stop) => Err(stop),
    }
}

/// The value of `result`, or the end of the run.
///
/// # Safety
/// `ctx` is live and the caller is an entry compiled code called, holding no value that needs
/// dropping.
#[inline]
pub(crate) unsafe fn check<T>(ctx: *mut TaskCtx, result: Stopped<T>) -> T {
    match result {
        Ok(value) => value,
        // SAFETY: caller contract.
        Err(stop) => unsafe { raise(ctx, stop) },
    }
}

/// End the run: request `stop` (the first stop requested wins) and leave the running task for
/// good. At width 1 the thread returns to its launcher; at width > 1 it goes idle and the
/// home thread's loop returns the outcome. A context no launcher drives (a harness calling
/// entries directly) ends environmentally, as every stop did before runs could stop.
///
/// # Safety
/// As [`check`].
#[cold]
pub(crate) unsafe fn raise(ctx: *mut TaskCtx, stop: Stop) -> ! {
    // A rejected replay phase has unwound its temporary runtime owners. End
    // this task at its admitted cut without requesting an external run stop.
    // Keep this scheduler transition at the executor's terminal boundary.
    if crate::tasks::free::replay::cut_rejection(ctx, &stop) {
        crate::tasks::park_final(ctx);
    }
    if crate::foreign::active() && stop != Stop::Requested {
        eprintln!("hirsel-rt: a run stopped inside a foreign call ({stop:?}); the process ends");
        std::process::abort();
    }
    // A speculative task's local stop cannot close an older turn's prefix.
    // The cause remains owned on this stack while its prior commands retire.
    // External stop requests already won the run control and bypass this gate.
    let stop = if unsafe { (*ctx).run_ref().ordered_mode.load(Ordering::Acquire) }
        && unsafe { (*ctx).run_ref().control.stopped().is_none() }
        && stop != Stop::Requested
    {
        let role = unsafe { crate::tasks::role::Guard::new(ctx) };
        let stop = match crate::tasks::ordered::engine::terminal(ctx, &stop) {
            Ok(()) => stop,
            Err(earlier) => earlier,
        };
        // Publish the selected cause before releasing the role. Otherwise
        // another worker can observe Frontier::Stopped and replace it with
        // a generic failure in the gap before this task requests its stop.
        let stop = unsafe { (*ctx).run_ref().control.request_inside(stop) };
        drop(role);
        stop
    } else {
        stop
    };
    // SAFETY: the context is live.
    let run = unsafe { (*ctx).run_ref() };
    if crate::pool::wide(run) {
        // A wide run has no callee runs (runners execute at width 1, `crate::launch`).
        request_from(ctx, &run.control, stop, false);
        // SAFETY: caller contract: nothing on this fiber is needed again.
        unsafe { crate::pool::abandon(ctx) }
    }
    // SAFETY: the context is live; a callee run's stop ends its execution, at the root's
    // launcher (RUN-10).
    let run = unsafe {
        let run = (*ctx).run_ptr();
        &*if (*run).root.is_null() {
            run
        } else {
            (*run).root
        }
    };
    if run.driver.sp.is_null() {
        crate::environmental(&format!("a run stopped without a launcher: {stop:?}"));
    }
    request_from(ctx, &run.control, stop, true);
    let mut abandoned = std::ptr::null_mut();
    // SAFETY: the launcher is suspended at `driver.sp` in `run_root` and nothing else resumes it.
    unsafe { fiber::switch(&raw mut abandoned, run.driver.sp) };
    crate::fault("a stopped run was resumed")
}

// Drop the owned path before abandoning the caller's continuation.
fn request_from(ctx: *mut TaskCtx, control: &Control, stop: Stop, wake_home: bool) {
    let path = unsafe { (*ctx).recording.as_ref() }.map(|task| task.path().to_vec());
    let outcome = (stop == Stop::Failed && unsafe { (*ctx).run_ref().free_production })
        .then(|| crate::free_launch::failure(ctx));
    control.decide(stop, wake_home, path.as_deref(), outcome.as_ref());
}

/// The root task's entry: compiled entry code that writes the run's result into its words.
pub(crate) type Entry = Box<dyn FnMut(*mut TaskCtx, *mut EntryWords) -> u8 + Send>;

/// The words a compiled entry reads and writes: its result buffer (zeroed and 16-aligned, for
/// a result of any supported alignment) and its root handles.
#[repr(C, align(16))]
pub(crate) struct EntryWords {
    pub(crate) result: [u128; crate::launch::RESULT_MAX as usize / 16],
    pub(crate) rt: u64,
    pub(crate) args: u64,
    pub(crate) roots: [u64; 5],
}

/// What a run's root task needs, owned by its run, not by the launcher's stack (design §6
/// Stops, the split launch frame): the entry, its words, the root's stack and the status it
/// completed with. At width > 1 the root runs on a compute worker and completes into the
/// frame: a stop may let the launcher return while the root still runs, and the root later
/// finishes into the frame, never into the launcher's stack. The report of a returned
/// error (PROG-3) reaches the launcher's streams, home state, so it runs on the home thread.
pub(crate) struct LaunchFrame {
    entry: Entry,
    pub(crate) words: Box<EntryWords>,
    report: Option<*mut (dyn FnMut(&crate::exit_report::Returned) + 'static)>,
    stack: fiber::Stack,
    status: u8,
    done: AtomicBool,
}

// SAFETY: the entry is `Send`; the report closure is called only on the home thread, while
// its launcher waits in `run_root`.
unsafe impl Send for LaunchFrame {}

impl LaunchFrame {
    /// A frame for `entry`, with no words and no report: a harness's root.
    pub(crate) fn bare(entry: Entry) -> Box<LaunchFrame> {
        let mut no_report = |_: &crate::exit_report::Returned| {};
        // SAFETY: the frame holds no report.
        let mut frame = unsafe { LaunchFrame::new(entry, 0, 0, [0; 5], &mut no_report) };
        frame.report = None;
        frame
    }
    /// A frame for `entry`, whose words hold `rt`, `args` and `roots`, and whose returned
    /// error is reported through `report` on the home thread.
    ///
    /// # Safety
    /// `report` outlives the run's `run_root`.
    pub(crate) unsafe fn new(
        entry: Entry,
        rt: u64,
        args: u64,
        roots: [u64; 5],
        report: &mut dyn FnMut(&crate::exit_report::Returned),
    ) -> Box<LaunchFrame> {
        Box::new(LaunchFrame {
            entry,
            words: Box::new(EntryWords {
                result: [0; crate::launch::RESULT_MAX as usize / 16],
                rt,
                args,
                roots,
            }),
            // SAFETY: caller contract; the frame calls it only while `run_root` runs.
            report: Some(unsafe { std::mem::transmute(report) }),
            stack: fiber::Stack::new(),
            status: 0,
            done: AtomicBool::new(false),
        })
    }
}

/// Run `body` as a run's root, without a report: a harness's stand-in for compiled entry
/// code ([`run_root`]).
///
/// # Safety
/// As for [`run_root`]; `body` outlives the run's code.
#[doc(hidden)]
pub unsafe fn run_body(ctx: *mut TaskCtx, body: &mut dyn FnMut() -> u8) -> Stopped<u8> {
    struct Body(*mut (dyn FnMut() -> u8 + 'static));
    // SAFETY: the body runs on one thread at a time, while its caller waits.
    unsafe impl Send for Body {}
    // SAFETY: caller contract.
    let body = Body(unsafe { std::mem::transmute::<&mut dyn FnMut() -> u8, _>(body) });
    let entry: Entry = Box::new(move |_, _| {
        let body = &body;
        // SAFETY: as above.
        unsafe { (*body.0)() }
    });
    // SAFETY: caller contract.
    unsafe { run_root(ctx, LaunchFrame::bare(entry)) }
}

unsafe extern "C" fn root_entry(ctx: *mut u8, frame: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let frame = frame.cast::<LaunchFrame>();
    // SAFETY: the run owns the frame until its last thread leaves it.
    unsafe {
        let _role = crate::tasks::role::Guard::new(ctx);
        check(ctx, observe(ctx));
        crate::tasks::free::replay::start(ctx);
        let words = &raw mut *(*frame).words;
        let mut status = {
            let _segment = crate::tasks::role::Segment::new(ctx);
            ((*frame).entry)(ctx, words)
        };
        check(ctx, observe(ctx));
        if status == crate::STATUS_OK {
            crate::free::root_end(ctx);
        }
        if status == crate::STATUS_OK
            && let Some(returned) = (*(*(*ctx).run_ptr()).entry_result.get()).take()
        {
            if (*ctx).run_ref().ordered_mode.load(Ordering::Acquire) {
                check(
                    ctx,
                    crate::tasks::ordered::engine::root_report(ctx, &returned),
                );
            }
            if let Some(report) = (*frame).report {
                crate::tasks::at_home(ctx, &mut || (*report)(&returned));
            }
            // Cleanup is mandatory even if the host's reporting sink failed.
            let mut cleanup = std::mem::MaybeUninit::<crate::finalize::Cleanup>::uninit();
            status = crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                returned.layout,
                returned.value,
                std::ptr::null(),
                cleanup.as_mut_ptr(),
                0,
                0,
            );
        }
        if (*ctx).run_ref().ordered_mode.load(Ordering::Acquire) {
            check(ctx, crate::tasks::ordered::engine::root_end(ctx, status));
        }
        if (*ctx).run_ref().free_production {
            crate::free_launch::root_end(ctx, status, &(*frame).words.result);
            crate::tasks::free::replay::start(ctx);
        }
        (*frame).status = status;
        (*frame).done.store(true, Ordering::Release);
        if crate::pool::wide((*ctx).run_ref()) {
            crate::pool::abandon(ctx);
        }
        let mut finished = std::ptr::null_mut();
        fiber::switch(&raw mut finished, (*(*(*ctx).run_ptr()).driver.get()).sp);
    }
    crate::fault("a finished root task was resumed")
}

/// A root published to the existing executor, whose home owner continues
/// servicing an execution instead of waiting for this individual run.
/// This owns no context lifetime: the caller retains/detaches that run exactly
/// as run_root does, including after a stop with a still-computing worker.
pub(crate) struct StartedRoot {
    run: *mut crate::context::RunShared,
    frame: *mut LaunchFrame,
}

impl StartedRoot {
    /// Observe a stopped or fully saved completed root, without waiting for
    /// code, a permit, an adapter, or another run's scheduler role.
    /// # Safety
    /// The home owner retains this run and its launch frame while polling.
    pub(crate) unsafe fn poll(&self) -> Option<Stopped<u8>> {
        unsafe {
            if let Some(stop) = (*self.run).control.stopped() {
                return Some(Err(stop));
            }
            if (*self.frame).done.load(Ordering::Acquire)
                && crate::pool::busy(&*self.run) == 0
                && !crate::pool::saved_pending(&*self.run)
            {
                return Some(Ok((*self.frame).status));
            }
        }
        None
    }
}

/// Publish a root without entering a waiting home loop. Runner orchestration
/// can retain several such runs on home; all computation uses the existing
/// executor and process permits, not an orchestration thread's stack.
/// # Safety
/// A fresh, bound root with physical executor topology already installed;
/// the home owner retains the context until every busy worker leaves, or
/// detaches it using the normal run lifetime protocol.
pub(crate) unsafe fn start_root(ctx: *mut TaskCtx, frame: Box<LaunchFrame>) -> StartedRoot {
    unsafe {
        let run = (*ctx).run_ptr();
        assert!(
            crate::pool::wide(&*run),
            "an asynchronous root needs executor topology"
        );
        crate::io::Binding::bridge(ctx);
        let (frame, sp) = prepare_root(ctx, frame);
        crate::tasks::start_root(ctx, sp);
        StartedRoot { run, frame }
    }
}

/// Install the same run-owned frame for synchronous and polled launchers.
unsafe fn prepare_root(
    ctx: *mut TaskCtx,
    mut frame: Box<LaunchFrame>,
) -> (*mut LaunchFrame, *mut u8) {
    unsafe {
        let run = (*ctx).run_ptr();
        if !(*(*run).driver.get()).sp.is_null() || (*(*run).launch.get()).is_some() {
            crate::fault("a run context driven twice");
        }
        frame.stack.account((*run).cpu_ledger.as_ref());
        let frame: *mut LaunchFrame = &raw mut **(*(*run).launch.get()).insert(frame);
        let sp = (*frame).stack.prepare(root_entry, ctx.cast(), frame.cast());
        (frame, sp)
    }
}

/// Run the run's root task on a fresh fiber, from `frame`, which the run owns from here on:
/// its status, or the stop that ended the run. Either way no fiber of the run runs again,
/// unless a stopped run's workers still hold it ([`crate::host::RunStatus`]'s
/// `reclamation_pending`).
///
/// At width 1 the root runs on the calling thread, which returns here when it finishes or
/// stops. At width > 1 the calling thread is the home thread: the root starts on the first
/// compute worker, and the home thread serves its loop (`crate::pool::serve_home`) until the
/// root completes into the frame or the run stops, which returns at once: a worker still
/// computing keeps running until its next runtime entry, outside the home thread.
///
/// # Safety
/// `ctx` is live, bound for the whole call, and drives no other run.
pub(crate) unsafe fn run_root(ctx: *mut TaskCtx, frame: Box<LaunchFrame>) -> Stopped<u8> {
    // SAFETY: the context is live; the root fiber returns here when it finishes or stops.
    unsafe {
        let run = (*ctx).run_ptr();
        if crate::pool::wide(&*run) {
            crate::io::Binding::bridge(ctx);
        }
        let (frame, sp) = prepare_root(ctx, frame);
        if crate::pool::wide(&*run) {
            crate::tasks::start_root(ctx, sp);
            let enclosing = crate::foreign::save();
            crate::pool::serve_home(run, &|| (*frame).done.load(Ordering::Acquire));
            crate::foreign::restore(enclosing);
            if let Some(stop) = (*run).control.stopped() {
                return Err(stop);
            }
            if (*frame).done.load(Ordering::Acquire) {
                return Ok((*frame).status);
            }
            return Err((*run)
                .control
                .stopped()
                .unwrap_or_else(|| crate::fault("the home loop returned with no stop")));
        }
        crate::pool::enter_root(ctx)?;
        let enclosing_foreign = crate::foreign::save();
        let enclosing = RUNNING.with(|r| r.replace(ctx));
        fiber::switch(&raw mut (*(*run).driver.get()).sp, sp);
        // A stop may leave any task of the run bound to the worker.
        (*ctx).rebind();
        crate::pool::leave_root(ctx);
        RUNNING.with(|r| r.set(enclosing));
        crate::foreign::restore(enclosing_foreign);
        (*(*run).driver.get()).sp = std::ptr::null_mut();
        if (*frame).done.load(Ordering::Acquire) {
            return Ok((*frame).status);
        }
        Err((*run)
            .control
            .stopped()
            .unwrap_or_else(|| crate::fault("a root left with no stop")))
    }
}

#[cfg(test)]
mod polled_tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicBool};

    #[test]
    fn home_can_stop_a_busy_run_and_service_another_under_one_compute_permit() {
        const CHILD: &str = "HIRSEL_POLLED_ROOT_ONE_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "stop::polled_tests::home_can_stop_a_busy_run_and_service_another_under_one_compute_permit", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(child.status.success(), "{child:?}");
            return;
        }
        crate::pool::set_compute_grant(1);
        for physical in [false, true] {
            let home = std::thread::current().id();
            let entered = Arc::new(AtomicBool::new(false));
            let release = Arc::new(AtomicBool::new(false));
            let bridged = Arc::new(AtomicBool::new(false));
            unsafe {
                let mut contexts = Vec::new();
                for _ in 0..2 {
                    let mut ctx = std::ptr::null_mut();
                    assert_eq!(
                        crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                        0
                    );
                    crate::pool::start(ctx, 1, false);
                    if physical {
                        (*ctx).run().free_mode = true;
                        crate::tasks::free::enable(ctx);
                        crate::pool::free::enable(ctx);
                    }
                    contexts.push(ctx);
                }
                let (seen, allow) = (entered.clone(), release.clone());
                let first = start_root(
                    contexts[0],
                    LaunchFrame::bare(Box::new(move |_, _| {
                        seen.store(true, Ordering::Release);
                        // A still-computing segment keeps its one permit. Home must
                        // be able to stop and poll it without preempting the segment.
                        while !allow.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                        crate::STATUS_OK
                    })),
                );
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !entered.load(Ordering::Acquire) {
                    assert!(std::time::Instant::now() < until);
                    std::thread::yield_now();
                }
                let output = bridged.clone();
                let second = start_root(
                    contexts[1],
                    LaunchFrame::bare(Box::new(move |ctx, _| {
                        crate::pool::call_home(ctx, &mut || {
                            assert_eq!(std::thread::current().id(), home);
                            output.store(true, Ordering::Release);
                        })
                        .unwrap();
                        crate::STATUS_OK
                    })),
                );
                assert_eq!(first.poll(), None);
                assert_eq!(second.poll(), None);
                assert!(!bridged.load(Ordering::Acquire));
                (*contexts[0]).run_ref().control.request(Stop::Requested);
                assert_eq!(first.poll(), Some(Err(Stop::Requested)));
                assert_eq!(crate::pool::compute_permits().1, 1);
                release.store(true, Ordering::Release);
                loop {
                    crate::pool::poll_home_posts((*contexts[1]).run_ptr());
                    if let Some(ended) = second.poll() {
                        assert_eq!(ended, Ok(crate::STATUS_OK));
                        break;
                    }
                    assert!(std::time::Instant::now() < until);
                    std::thread::yield_now();
                }
                assert!(bridged.load(Ordering::Acquire));
                while crate::pool::busy((*contexts[0]).run_ref()) != 0 {
                    assert!(std::time::Instant::now() < until);
                    std::thread::yield_now();
                }
                for ctx in contexts {
                    crate::context::hirsel_rt_v2_ctx_destroy(ctx);
                }
            }
            assert_eq!(crate::pool::compute_permits(), (1, 0, 0));
        }
    }
}
