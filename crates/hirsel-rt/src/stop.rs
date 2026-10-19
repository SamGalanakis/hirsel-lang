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
use hirsel_run_schema::{BudgetKind, Environmental};

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

/// Where a run's stop returns: the launcher's suspended stack and the stop it receives.
pub(crate) struct Driver {
    sp: *mut u8,
    stop: Option<Stop>,
}

impl Default for Driver {
    fn default() -> Self {
        Driver {
            sp: std::ptr::null_mut(),
            stop: None,
        }
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

/// End the run: return to its launcher with `stop`. A context no launcher drives (a harness
/// calling entries directly) ends environmentally, as every stop did before runs could stop.
///
/// # Safety
/// As [`check`].
#[cold]
pub(crate) unsafe fn raise(ctx: *mut TaskCtx, stop: Stop) -> ! {
    if crate::foreign::active() {
        eprintln!("hirsel-rt: a run stopped inside a foreign call ({stop:?}); the process ends");
        std::process::abort();
    }
    // SAFETY: the context is live.
    let driver = unsafe { &mut (*ctx).run().driver };
    if driver.sp.is_null() {
        crate::environmental(&format!("a run stopped without a launcher: {stop:?}"));
    }
    driver.stop = Some(stop);
    let mut abandoned = std::ptr::null_mut();
    // SAFETY: the launcher is suspended at `driver.sp` in `run_root` and nothing else resumes it.
    unsafe { fiber::switch(&raw mut abandoned, driver.sp) };
    crate::fault("a stopped run was resumed")
}

/// The root fiber's frame: the entry call and its status.
struct Root<'a> {
    call: &'a mut dyn FnMut() -> u8,
    status: u8,
}

unsafe extern "C" fn root_entry(ctx: *mut u8, root: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    // SAFETY: `run_root` keeps the frame live until this fiber switches back for the last time.
    unsafe {
        let root = &mut *root.cast::<Root<'_>>();
        root.status = (root.call)();
        let mut finished = std::ptr::null_mut();
        fiber::switch(&raw mut finished, (*ctx).run().driver.sp);
    }
    crate::fault("a finished root task was resumed")
}

/// Run `call` (the compiled entry) as the run's root task on a fresh fiber: its status, or the
/// stop that ended the run. Either way no fiber of the run runs again.
///
/// # Safety
/// `ctx` is live, bound for the whole call, and drives no other run.
pub(crate) unsafe fn run_root(ctx: *mut TaskCtx, call: &mut dyn FnMut() -> u8) -> Stopped<u8> {
    let stack = fiber::Stack::new();
    let mut root = Root { call, status: 0 };
    let sp = stack.prepare(root_entry, ctx.cast(), (&raw mut root).cast());
    // SAFETY: the context is live; the root fiber returns here when it finishes or stops.
    unsafe {
        let run = (*ctx).run_ptr();
        if !(*run).driver.sp.is_null() {
            crate::fault("a run context driven twice");
        }
        let enclosing = RUNNING.with(|r| r.replace(ctx));
        fiber::switch(&raw mut (*run).driver.sp, sp);
        // A stop may leave any task of the run bound to the worker.
        (*ctx).rebind();
        RUNNING.with(|r| r.set(enclosing));
        (*run).driver.sp = std::ptr::null_mut();
        match (*run).driver.stop.take() {
            Some(stop) => Err(stop),
            None => Ok(root.status),
        }
    }
}
