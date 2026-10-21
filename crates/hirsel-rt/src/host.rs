//! The host side of runs (design.md, Host and runs). A [`Host`] admits runs and starts them;
//! each run of a loaded [`Program`] gets its own [`Sinks`], its own context and failure table,
//! and ends in one [`RunOutcome`] handed back by [`RunHandle::wait`]. Nothing here exits the
//! process: only a standalone launcher turns an outcome into an exit status
//! ([`RunOutcome::exit_code`]).
//!
//! The process's standard input is one resource for all of its runs: a run whose record grants
//! `io.stdin` holds the take-once [`ProcessStdin`] token from admission until its Io is
//! dropped, and admission refuses a second run while one holds it.
use std::cell::RefCell;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hirsel_run_config::{AdmittedRun, BoundRun};
use hirsel_run_schema::{RunOutcome, RunRefusal};

use crate::io_real::RealIo;
use crate::io_sim::SimIo;
use crate::launch::Config;
use crate::program::Program;

static STDIN_FREE: AtomicBool = AtomicBool::new(true);

/// The process's standard input, held by at most one run at a time. Dropping it returns it.
#[derive(Debug)]
pub struct ProcessStdin(());
impl ProcessStdin {
    /// The token, or `None` while another run holds it.
    pub fn take() -> Option<Self> {
        STDIN_FREE.swap(false, Ordering::AcqRel).then_some(Self(()))
    }
}
impl Drop for ProcessStdin {
    fn drop(&mut self) {
        STDIN_FREE.store(true, Ordering::Release);
    }
}

/// One output stream of a run, shared by the run's Io and its launcher.
#[derive(Clone)]
pub struct Sink(Rc<RefCell<dyn Write>>);
impl Sink {
    pub fn new(output: impl Write + 'static) -> Self {
        Self(Rc::new(RefCell::new(output)))
    }
    /// A sink that keeps what the run writes, readable through the returned buffer.
    pub fn capture() -> (Self, Captured) {
        let buffer = Captured::default();
        (Self::new(buffer.clone()), buffer)
    }
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

/// An output stream that any thread may write: what a
/// [`ConcurrentAdapter`](crate::io_concurrent::ConcurrentAdapter) writes to (design §6 Io).
/// Clones share one storage, whose identity is its logical key ([`SendSink::storage`]).
#[derive(Clone)]
pub struct SendSink(Arc<Mutex<dyn Write + Send>>);
impl SendSink {
    pub fn new(output: impl Write + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(output)))
    }
    /// A sink that keeps what the run writes, readable through the returned buffer.
    pub fn capture() -> (Self, SendCaptured) {
        let buffer = SendCaptured::default();
        (Self::new(buffer.clone()), buffer)
    }
    /// The identity of this sink's storage, shared by its clones.
    pub fn storage(&self) -> usize {
        Arc::as_ptr(&self.0).cast::<()>() as usize
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, dyn Write + Send + 'static> {
        // A writer that panicked mid-write leaves bytes, not broken invariants.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
impl Write for SendSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.lock().write(bytes)
    }
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        // One lock for the whole write, so concurrent writers never interleave within it.
        self.lock().write_all(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.lock().flush()
    }
}

/// What a [`SendSink::capture`] sink received.
#[derive(Clone, Default)]
pub struct SendCaptured(Arc<Mutex<Vec<u8>>>);
impl SendCaptured {
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}
impl Write for SendCaptured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What a [`Sink::capture`] sink received.
#[derive(Clone, Default)]
pub struct Captured(Rc<RefCell<Vec<u8>>>);
impl Captured {
    pub fn bytes(&self) -> Vec<u8> {
        self.0.borrow().clone()
    }
}
impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A run's output streams: the program's granted stdout and stderr, and `report`, where the
/// runtime writes the run's own reports (failure chains, PROG-3 error reports, refusals and
/// stops) when no launcher reads them from an observation record.
#[derive(Clone)]
pub struct Sinks {
    pub stdout: Sink,
    pub stderr: Sink,
    pub report: Sink,
}
impl Sinks {
    /// The process's own streams. Standard output is written unbuffered, so a write error
    /// reaches the program as a recorded outcome (`RawStdout`).
    pub fn process() -> Self {
        Self {
            stdout: Sink::new(crate::launch::RawStdout),
            stderr: Sink::new(std::io::stderr()),
            report: Sink::new(std::io::stderr()),
        }
    }
}

/// The runs that decide CONC-8 together: a run and the in-process callee runs it starts. A
/// run [`Host::start`] starts begins its own Execution, and is Blocked when none of its tasks
/// can proceed; callee runs started as calls (runner-core's `local.threads@1`) are to join
/// their caller's Execution, which is Blocked only when no run of it can proceed.
#[derive(Clone, Default)]
pub struct Execution(Rc<RefCell<u32>>);
impl Execution {
    /// The runs started in this Execution so far.
    pub fn runs(&self) -> u32 {
        *self.0.borrow()
    }
    fn join(&self) {
        *self.0.borrow_mut() += 1;
    }
}

/// What a started run executes.
pub enum Selection {
    /// A harness launch without a run record: the launcher's environment configures Io.
    Harness,
    /// A run this host admitted, performing live effects; `tape` records them.
    Live {
        admitted: AdmittedRun,
        io: RealIo,
        tape: Option<std::path::PathBuf>,
    },
    /// A recorded run replayed with no host effect.
    Replay { run: BoundRun, io: SimIo },
}

/// Admits runs and starts them; see the module documentation.
pub struct Host {
    clock: fn() -> Option<i64>,
}
impl Host {
    /// A host whose admissions read the system clock.
    pub fn process() -> Self {
        Self {
            clock: || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|n| i64::try_from(n.as_nanos()).ok())
            },
        }
    }
    /// A host whose admissions happen at `clock`'s time (nanoseconds since the Unix epoch).
    pub fn with_clock(clock: fn() -> Option<i64>) -> Self {
        Self { clock }
    }

    /// Load a compiled program; see [`Program::load`].
    ///
    /// # Safety
    /// As for [`Program::load`].
    pub unsafe fn load(
        &self,
        descriptor: *const crate::program::ProgramDescriptor,
    ) -> Result<Program, String> {
        // SAFETY: caller contract.
        unsafe { Program::load(descriptor) }
    }

    /// Admit a bound run on this host with `sinks` as its granted streams, once: grants,
    /// mounts, revocations (from `revocations`), validity time and the process's stdin.
    pub fn admit(
        &self,
        run: &BoundRun,
        revocations: Option<&Path>,
        sinks: &Sinks,
    ) -> Result<(AdmittedRun, RealIo), RunRefusal> {
        let mut host = crate::host_launch::HostGrants::new(sinks.stdout.clone())
            .with_stderr(sinks.stderr.clone());
        if let Some(path) = revocations {
            host.read_revocations(path)?;
        }
        let now = (self.clock)().ok_or_else(|| RunRefusal::Invalid {
            field: "admission.clock".into(),
            detail: "out of range".into(),
        })?;
        let admitted = run.admit(&mut host, now)?;
        Ok((admitted, host.into_io()))
    }

    /// Start a run of `program`: `selection` names what it executes, `arguments` are the
    /// harness's process arguments (a run with a record reads its own), and `config` its
    /// seeds and identity. The run executes on this thread when it is waited for.
    pub fn start<'p>(
        &self,
        program: &'p Program,
        config: Config,
        selection: Selection,
        arguments: Vec<OsString>,
        sinks: Sinks,
    ) -> RunHandle<'p> {
        let execution = Execution::default();
        execution.join();
        RunHandle {
            program,
            config,
            selection,
            arguments,
            sinks,
            execution,
            control: Default::default(),
            workers: None,
            metrics: None,
        }
    }
}

/// A started run's stop handle, which any thread may use while the run runs and after
/// (design §6 Stops and worker lifetime).
#[derive(Clone)]
pub struct RunControl(std::sync::Arc<crate::stop::Control>);
impl RunControl {
    /// Stop the run. The first stop of a run wins, whoever asks: a later one, or a stop the
    /// run raised itself, leaves it as it is. A stopped run ends `Environmental(Stopped)` at
    /// its next operation, scheduling point or fuel refill, never inside a foreign call: one
    /// in C ends when the call returns.
    pub fn stop(&self) {
        self.0.request(crate::stop::Stop::Requested);
    }
    /// Where the run stands.
    pub fn status(&self) -> RunStatus {
        self.0.status()
    }
}

/// Where a run stands, as its [`RunControl`] reports it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunStatus {
    /// A stop was requested, by the host or by the run itself.
    pub stop_requested: bool,
    /// The admission thread is inside a home foreign interval (a home extern in C, or its
    /// callback's Hirsel code): `wait` cannot return until it leaves it.
    pub home_busy: bool,
    /// `wait` returned the run's outcome.
    pub finished: bool,
    /// The workers still inside the run's code when `wait` returned; the last to leave frees
    /// the run.
    pub reclamation_pending: u32,
    /// Compute permits still held by a stopped run's workers. The process pool
    /// keeps accounting them until those workers leave; a new run cannot replace them.
    pub lost_permits: u32,
}

/// A started run; [`RunHandle::wait`] runs it to its outcome.
pub struct RunHandle<'p> {
    program: &'p Program,
    config: Config,
    selection: Selection,
    arguments: Vec<OsString>,
    sinks: Sinks,
    execution: Execution,
    control: std::sync::Arc<crate::stop::Control>,
    workers: Option<(u32, bool)>,
    metrics: Option<std::sync::Arc<crate::pool::WorkerMetrics>>,
}
impl RunHandle<'_> {
    pub fn execution(&self) -> &Execution {
        &self.execution
    }
    /// The run's stop handle. A run whose host holds one takes its fuel in chunks and wakes
    /// its idle waits for a stop.
    pub fn control(&self) -> RunControl {
        self.control.host();
        RunControl(std::sync::Arc::clone(&self.control))
    }
    /// Run on `width` compute workers beside the admission thread (`width` > 1), migrating
    /// every task at every suspension under `migrate`: a test knob until multi-worker
    /// execution is proven equal to one worker. Without it the environment decides
    /// (`HIRSEL_RT_WORKERS`, `HIRSEL_RT_MIGRATE`), else one worker.
    #[doc(hidden)]
    pub fn with_workers(mut self, width: u32, migrate: bool) -> Self {
        self.workers = Some((width, migrate));
        self
    }
    /// The run's scheduling row counters (design §5 Headroom), which only a run asked for
    /// them collects.
    pub fn worker_metrics(&mut self) -> std::sync::Arc<crate::pool::WorkerMetrics> {
        std::sync::Arc::clone(self.metrics.get_or_insert_with(Default::default))
    }
    /// Run to the end and return how the run ended. A refusal before entry is reported as
    /// the launch's refusal (`refused` in an observation record, else on the report sink).
    pub fn wait(self) -> RunOutcome {
        let Self {
            program,
            config,
            selection,
            arguments,
            sinks,
            execution: _,
            control,
            workers,
            metrics,
        } = self;
        let options = crate::launch::Options {
            control,
            workers,
            metrics,
        };
        // SAFETY: a loaded program's code and data live for the process.
        match unsafe {
            crate::launch::execute(program, &config, selection, arguments, &sinks, options)
        } {
            Ok(outcome) => outcome,
            Err(reason) => crate::launch::refused(&sinks, reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time_takes_the_process_stdin() {
        let held = ProcessStdin::take().expect("free stdin");
        assert!(ProcessStdin::take().is_none());
        drop(held);
        assert!(ProcessStdin::take().is_some());
    }
}

/// A run of a Rust stand-in for compiled entry code, launched as [`RunHandle::wait`]
/// launches a program: its workers, budget, Io binding and stop handle, the launch frame and
/// a stopped run's detachment (design §6 Stops and worker lifetime). For the runtime's own
/// tests.
#[doc(hidden)]
pub struct BodyRun {
    pub config: crate::Config,
    pub width: u32,
    pub migrate: bool,
    pub fuel: Option<u64>,
    pub adapter: Option<Box<dyn crate::io::Adapter>>,
    control: std::sync::Arc<crate::stop::Control>,
    metrics: Option<std::sync::Arc<crate::pool::WorkerMetrics>>,
}

/// How a [`BodyRun`] ended.
#[doc(hidden)]
#[derive(Debug)]
pub struct BodyOutcome {
    /// The root's status, or the stop that ended the run.
    pub ended: Result<u8, crate::stop::Stop>,
    /// The workers still inside the run's code when it returned.
    pub pending: u32,
}

impl BodyRun {
    pub fn new(width: u32, migrate: bool) -> Self {
        BodyRun {
            config: crate::Config::new(),
            width,
            migrate,
            fuel: None,
            adapter: None,
            control: Default::default(),
            metrics: None,
        }
    }
    pub fn control(&self) -> RunControl {
        self.control.host();
        RunControl(std::sync::Arc::clone(&self.control))
    }
    pub fn worker_metrics(&mut self) -> std::sync::Arc<crate::pool::WorkerMetrics> {
        std::sync::Arc::clone(self.metrics.get_or_insert_with(Default::default))
    }
    /// Run `body` as the run's root on its first compute worker (on this thread at width
    /// 1), after `setup` prepared the fresh root context on this thread.
    pub fn run(
        self,
        setup: impl FnOnce(*mut crate::TaskCtx),
        body: impl FnMut(*mut crate::TaskCtx) -> u8 + Send + 'static,
    ) -> BodyOutcome {
        let mut body = body;
        let mut ctx = std::ptr::null_mut();
        // SAFETY: a valid configuration and out-parameter.
        if unsafe { crate::context::hirsel_rt_v2_ctx_create(&self.config, &mut ctx) } != 0 {
            crate::fault("a body run's configuration");
        }
        setup(ctx);
        // SAFETY: the fresh root context, on this thread, before the run's code runs.
        unsafe {
            (*ctx).run().control = std::sync::Arc::clone(&self.control);
            (*ctx).run().worker_metrics = self.metrics.clone();
            if self.width > 1 {
                crate::pool::start(ctx, self.width, self.migrate);
            }
            if self.fuel.is_some() {
                (*ctx).set_budgets(self.fuel, None);
            }
        }
        let mut binding = self
            .adapter
            .map(|adapter| Box::new(crate::io::Binding::from_box(adapter)));
        if let Some(binding) = &mut binding {
            // SAFETY: the binding outlives the run's code on this thread.
            unsafe { binding.bind(&mut *ctx) };
            if self.control.hosted()
                && let Some(fd) = self.control.wake_fd()
            {
                binding.adapter.wake_on(fd);
            }
        }
        let entry: crate::stop::Entry = Box::new(move |ctx, _| body(ctx));
        // SAFETY: the root context, driven once.
        let ended = unsafe { crate::stop::run_root(ctx, crate::stop::LaunchFrame::bare(entry)) };
        let pending = match &ended {
            // SAFETY: the context is live.
            Err(_) => crate::pool::busy(unsafe { (*ctx).run_ref() }),
            Ok(_) => 0,
        };
        // SAFETY: the root context; a detached run's workers free it.
        unsafe {
            if pending > 0 {
                if let Some(holders) = crate::pool::holders((*ctx).run_ref()) {
                    self.control.track(holders);
                }
                crate::context::detach(ctx);
            } else {
                if ended.is_err() {
                    (*ctx).abandon();
                }
                crate::context::hirsel_rt_v2_ctx_destroy(ctx);
            }
        }
        drop(binding);
        self.control.finish(pending);
        BodyOutcome { ended, pending }
    }
}
