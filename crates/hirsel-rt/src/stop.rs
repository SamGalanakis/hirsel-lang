//! Ending one run without ending the process.
//!
//! A run's root task runs on a fiber of its own ([`run_root`]), so the launcher keeps its stack
//! while the run executes. A [`Stop`] (Blocked, a spent budget, an adapter refusal, a replay
//! divergence or a codec limit) ends the run from wherever it is raised: the runtime's internal
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
use hirsel_run_config::{BudgetKind, Environmental};

use crate::RunCtx;
use crate::fiber;
use crate::io::AdapterError;

/// Why a run stopped before its entry returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stop {
    /// Every task waits on another and nothing outside the run can wake one (CONC-8).
    Blocked,
    BudgetExceeded(BudgetKind),
    /// The adapter refused a request.
    Refused(String),
    /// A replay's requests left its recording.
    Diverged(String),
    Environmental(Environmental),
}

impl Stop {
    /// The stop an adapter error ends the run with.
    pub(crate) fn refusal(error: AdapterError) -> Stop {
        match error {
            AdapterError::ReplayMismatch(_)
            | AdapterError::MissingInput(_)
            | AdapterError::UnconsumedInput(_) => Stop::Diverged(format!("{error:?}")),
            AdapterError::RecordingFailed(_) => {
                Stop::Environmental(Environmental::Recording(format!("{error:?}")))
            }
            _ => Stop::Refused(format!("{error:?}")),
        }
    }
    /// The ending a host reports for this stop.
    pub fn outcome(self) -> hirsel_run_config::RunOutcome {
        use hirsel_run_config::RunOutcome;
        match self {
            Stop::Blocked => RunOutcome::Blocked,
            Stop::BudgetExceeded(kind) => RunOutcome::BudgetExceeded(kind),
            Stop::Refused(reason) => RunOutcome::Refused(reason),
            Stop::Diverged(reason) => RunOutcome::Diverged(reason),
            Stop::Environmental(cause) => RunOutcome::Environmental(cause),
        }
    }
}

thread_local! {
    /// The run executing on this thread, for reports raised without a context argument.
    static RUNNING: std::cell::Cell<*mut RunCtx> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// The context of the run executing on this thread, or null.
pub(crate) fn running() -> *mut RunCtx {
    RUNNING.with(std::cell::Cell::get)
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
pub(crate) unsafe fn check<T>(ctx: *mut RunCtx, result: Stopped<T>) -> T {
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
pub(crate) unsafe fn raise(ctx: *mut RunCtx, stop: Stop) -> ! {
    if crate::foreign::active() {
        eprintln!("hirsel-rt: a run stopped inside a foreign call ({stop:?}); the process ends");
        std::process::abort();
    }
    // SAFETY: the context is live.
    let driver = unsafe { &mut (*ctx).driver };
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
    let ctx = ctx.cast::<RunCtx>();
    // SAFETY: `run_root` keeps the frame live until this fiber switches back for the last time.
    unsafe {
        let root = &mut *root.cast::<Root<'_>>();
        root.status = (root.call)();
        let mut finished = std::ptr::null_mut();
        fiber::switch(&raw mut finished, (*ctx).driver.sp);
    }
    crate::fault("a finished root task was resumed")
}

/// Run `call` (the compiled entry) as the run's root task on a fresh fiber: its status, or the
/// stop that ended the run. Either way no fiber of the run runs again.
///
/// # Safety
/// `ctx` is live, bound for the whole call, and drives no other run.
pub(crate) unsafe fn run_root(ctx: *mut RunCtx, call: &mut dyn FnMut() -> u8) -> Stopped<u8> {
    let stack = fiber::Stack::new();
    let mut root = Root { call, status: 0 };
    let sp = stack.prepare(root_entry, ctx.cast(), (&raw mut root).cast());
    // SAFETY: the context is live; the root fiber returns here when it finishes or stops.
    unsafe {
        if !(*ctx).driver.sp.is_null() {
            crate::fault("a run context driven twice");
        }
        let enclosing = RUNNING.with(|r| r.replace(ctx));
        fiber::switch(&raw mut (*ctx).driver.sp, sp);
        RUNNING.with(|r| r.set(enclosing));
        (*ctx).driver.sp = std::ptr::null_mut();
        match (*ctx).driver.stop.take() {
            Some(stop) => Err(stop),
            None => Ok(root.status),
        }
    }
}
