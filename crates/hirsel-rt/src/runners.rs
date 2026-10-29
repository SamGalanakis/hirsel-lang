//! Runners (docs/spec/runners.md) of a compiled run: runner handles, the places' contracts and
//! the callee runs a sent call starts in this process, decision for decision as the
//! interpreter's `library/runners.rs` and `runs.rs` and the host provider's
//! `hirsel-driver/src/network/runners.rs` make them.
//!
//! `io.runner` projects a runner the run's configuration names, `test.runner` a `sim@1` node
//! modeling `local.threads@1`, and narrowing mints one over a subset of its grant set or
//! allowlist (RUN-2). A call checks authority and cancellation, then meets its place's
//! contract (`hirsel_run_schema::PlaceState`, shared with the interpreter). Its request goes
//! to the run's adapter as one `RunnerCall` (RUN-11): a call refused before it is sent
//! completes at once, and a sent one parks until its callee ends, when its outcome is
//! delivered and the caller's next scheduling point re-issues the request to complete it.
//! Replay starts no callee: the recorded completion stands in for it.
//!
//! A sent call's callee is its own run of this execution (RUN-10): a context, worker and heap
//! of its own (`hirsel_rt_v2_ctx_create`), an Io binding projected from the runner's grants
//! that parks on the caller's readiness under its own tag (RUN-12), a hash and Random seed
//! derived from the call key, and its own tape beside a recording caller's. Its root task
//! runs `$native.runner_main` on a fiber of its own. Nothing of one run's storage reaches
//! another: an argument, result, error or failure crosses as canonical codec bytes, which the
//! compiled bodies decode into fresh storage of the receiving run.
//!
//! A run executes until none of its tasks can proceed (`idle`, from the scheduler), then the
//! next ready run takes over, in the order runs became ready. At its idle point the run's new
//! calls start (`take_calls`) and the cancellations of its sent calls' tasks are forwarded
//! (`take_cancels`): a callee not yet started ends `Canceled` at once, a started one has its
//! root canceled and its caller keeps waiting (RUN-9). With nothing ready, the runs whose
//! adapter has a ready wait go first, then each run's adapter waits in run order; only when
//! none progresses is the execution Blocked, with every run's waits (CONC-8). A stop in any
//! run ends the execution, as a process crash ends caller and callees together.
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::c_void;
use std::rc::Rc;

use hirsel_run_schema::{
    CallForm, CallOutcome, CalleeFacts, CalleeOutcome, CodeReach, IoGrants, PlaceState, Placed,
    RunnerCompletion, RunnerEntry, RunnerRequest,
};

use crate::context::RunShared;
use crate::fiber::{self, Stack};
use crate::io::{AdapterError, Binding, Completion, GrantResult, Request};
use crate::io_real::RealIo;
use crate::stop::{Stop, Stopped};
use crate::tasks::TaskCode;
use crate::views::Slice;
use crate::{Header, STATUS_OK, TaskCtx, fault};

pub(crate) mod native;

type Concurrent = std::sync::Arc<dyn crate::io_concurrent::ConcurrentAdapter>;
type HomeOwner = Box<dyn std::any::Any>;
type Admitted = (Option<HomeOwner>, Option<Concurrent>);
type Admit = unsafe fn(
    *mut TaskCtx,
    Config,
    Option<[u8; 16]>,
    bool,
    Option<std::sync::Arc<crate::io_real::ConcurrentIo>>,
) -> Result<Admitted, String>;

/// The launcher names this provider only when its program reaches runner entries.
/// Embedding hosts retain the complete provider through the weak default root.
pub(crate) struct Launch {
    pub(crate) admit: Admit,
    pub(crate) finish_replay: unsafe fn(*mut TaskCtx) -> Stopped<u8>,
}

#[unsafe(export_name = "hirsel.runtime.runners.provider")]
static LAUNCH: Launch = Launch {
    admit,
    finish_replay: native::finish_replay,
};

unsafe fn admit(
    ctx: *mut TaskCtx,
    mut config: Config,
    instance: Option<[u8; 16]>,
    production_free: bool,
    io: Option<std::sync::Arc<crate::io_real::ConcurrentIo>>,
) -> Result<Admitted, String> {
    // The owner stays on admission. Detached workers hold only Slot's Send
    // transport, and never destroy Home's Rc adapters or sinks.
    let mut home: Option<HomeOwner> = None;
    let adapter = if production_free && !config.runners.is_empty() {
        let adapter = if let Some(io) = io {
            let slot =
                native::Slot::new(io).map_err(|error| format!("native runners: {error:?}"))?;
            unsafe { (*ctx).run().runner_home = Some(slot.clone()) };
            home = Some(unsafe { native::Home::new(slot.clone(), config.clone()) });
            Some(native::wrap(&slot))
        } else {
            None
        };
        config.stdout = None;
        adapter
    } else {
        io.map(|io| io as Concurrent)
    };
    unsafe { configure(ctx, config) };
    if let Some(instance) = instance {
        unsafe { set_instance(ctx, instance) };
    }
    Ok((home, adapter))
}

/// `RunnerWait`'s outcome codes, `kind << 4 | detail`, which the compiled bodies turn into
/// `Result[R, RunError[E]]`: `Ok`, `Failed` and `Faulted` (their bytes from `RunnerTake`),
/// then `NotRun(reason)`, `InDoubt(reason)` and `Canceled(cause)` by their variants' ordinals.
const OUTCOME_OK: u64 = 0;
const OUTCOME_FAILED: u64 = 1 << 4;
const OUTCOME_FAULTED: u64 = 2 << 4;
const OUTCOME_NOT_RUN: u64 = 3 << 4;
const OUTCOME_IN_DOUBT: u64 = 4 << 4;
const OUTCOME_CANCELED: u64 = 5 << 4;

/// `NotRunReason` and `InDoubtReason` ordinals (core library).
const DENIED: u32 = 0;
const LOST: u32 = 0;
const ABANDONED: u32 = 2;

/// `RunnerReply`'s tags: the entry's `Ok` or `Error` payload, or its caught failure.
const REPLY_OK: u64 = 0;
const REPLY_ERROR: u64 = 1;
const REPLY_FAILED: u64 = 2;

/// The runners a launch configures and what callee runs are made from.
#[derive(Clone, Default)]
pub struct Config {
    /// The root run's runners (RUN-1).
    pub runners: Vec<RunnerEntry>,
    /// The seed `sim@1` runners draw their faults from (`inputs.fault_seed`).
    pub fault_seed: u64,
    /// What the launcher found about each callee entry, by full path (RUN-8, RUN-11).
    pub callees: BTreeMap<String, CalleeFacts>,
    /// Compiler-derived request schemas, required by production runner requests.
    pub request_schemas: Option<crate::program::RunnerRequestSchemas>,
    /// The program's entries: each Entry's code word and its definition's full path.
    pub entries: Vec<(usize, String)>,
    /// The root run's tape: a callee records `<tape>.call-<ordinal>` beside it.
    pub tape: Option<std::path::PathBuf>,
    /// The process's standard output, which in-process callees print to.
    pub stdout: Option<crate::host::Sink>,
    /// The root run's runtime configuration, which each callee context is created with.
    pub runtime: crate::context::Config,
    pub failure_table: Option<crate::isolate::FailureTable>,
}

/// A runner handle's place and the authority its callees run under.
#[derive(Clone, Debug)]
struct RunnerGrant {
    entry: RunnerEntry,
    /// The entry's grant set, or a narrowing of it (RUN-2).
    grants: IoGrants,
    /// The definitions it runs, as full versioned paths; None runs any.
    definitions: Option<Vec<String>>,
}

/// One execution: its runs, the root first, in the order they started.
pub(crate) struct Execution {
    idle: unsafe fn(*mut TaskCtx) -> Stopped<()>,
    config: Config,
    runs: Vec<Run>,
    /// The run executing now.
    active: usize,
    ready: VecDeque<usize>,
    /// A call was sent: from now on the execution, not the run, decides what an idle run does.
    driving: bool,
    /// The root's cooperative readiness, which callee runs share under their own tags.
    readiness: Option<crate::readiness::Readiness>,
    tags: u64,
}

/// One run.
struct Run {
    /// The run's root context: its state, binding and scheduler are reached through it.
    root: *mut TaskCtx,
    /// The task that went idle while another run executes, and where it resumes.
    resume: *mut TaskCtx,
    sp: *mut u8,
    ended: bool,
    state: Runners,
    callee: Option<Callee>,
}

/// A callee run and the call it answers.
struct Callee {
    caller: usize,
    token: u64,
    started: bool,
    /// Suspensions so far, and the one after which its place crashes (`sim@1`).
    suspensions: u32,
    crash_after: Option<u32>,
    start: Start,
    /// The tag and bytes its root replied with, and a caught failure's chain.
    reply: Option<(u64, Vec<u8>)>,
    chain: Vec<crate::failure::Chained>,
    /// Its root fiber's stack, Io and binding.
    stack: Option<Stack>,
    io: Rc<RefCell<RealIo>>,
    binding: Box<Binding>,
    /// Its tape, when it records one.
    recording: bool,
}

/// What a callee run starts from: its main, its entry and its argument.
#[derive(Clone)]
struct Start {
    main: (usize, usize),
    entry: (usize, usize),
    argument: Vec<u8>,
    form: CallForm,
}

/// One run's runner handles and calls.
#[derive(Default)]
struct Runners {
    /// Runner words are indices into this list, plus one.
    grants: Vec<RunnerGrant>,
    calls: BTreeMap<u64, Call>,
    /// Sent calls whose callee starts at the run's next idle point.
    starts: Vec<u64>,
    /// Requests made so far (RUN-11's request ordinals).
    requests: u64,
    /// Each place's state in this run, by runner name.
    places: BTreeMap<String, PlaceState>,
    /// This run's instance: call keys and callee seeds derive from it.
    instance: [u8; 16],
}

struct Call {
    /// The task waiting for the outcome, once one waits.
    task: Option<u32>,
    grant: RunnerGrant,
    ordinal: u64,
    key: [u8; 32],
    /// The place a sent call occupies until its outcome is taken.
    place: Option<String>,
    /// `sim@1`'s `lose_reply`: the outcome is lost after the callee completes.
    lose_reply: bool,
    crash_after: Option<u32>,
    /// The cause of the cancellation forwarded to the callee, once forwarded.
    forwarded: Option<u32>,
    start: Option<Start>,
    /// The callee run, once started.
    callee: Option<usize>,
    completion: Option<RunnerCompletion>,
}

/// How a callee run ended.
enum End {
    /// Its root returned, having replied `tag` with these bytes.
    Replied(u64, Vec<u8>),
    Canceled,
    /// Its place crashed.
    Abandoned,
}

impl Execution {
    fn new(config: Config, root: *mut TaskCtx) -> Self {
        Execution {
            idle: drive_idle,
            config,
            runs: vec![Run {
                root,
                resume: root,
                sp: std::ptr::null_mut(),
                ended: false,
                state: Runners::default(),
                callee: None,
            }],
            active: 0,
            ready: VecDeque::new(),
            driving: false,
            readiness: None,
            tags: 0,
        }
    }
    fn ready(&mut self, index: usize) {
        if !self.ready.contains(&index) {
            self.ready.push_back(index);
        }
    }
}

impl Drop for Execution {
    /// The callee runs end with their execution, on the launcher's stack: each is abandoned
    /// as a stopped run is, then destroyed.
    fn drop(&mut self) {
        let running = crate::stop::running();
        for run in self.runs.drain(1..) {
            let Some(callee) = run.callee else { continue };
            // SAFETY: nothing of the callee run executes any more.
            unsafe {
                (*run.root).abandon();
                crate::context::hirsel_rt_v2_ctx_destroy(run.root);
            }
            drop(callee.stack);
            drop(callee.binding);
        }
        crate::stop::set_running(running);
    }
}

/// Configure `ctx`'s execution before its entry runs: the launch's runners.
///
/// # Safety
/// `ctx` is the live root context of a run that runs nothing yet.
pub unsafe fn configure(ctx: *mut TaskCtx, config: Config) {
    // SAFETY: caller contract.
    unsafe { *(*ctx).run().runners = Some(Box::new(Execution::new(config, ctx))) };
}

/// # Safety
/// Sole-root admission, before dispatch, using the validated tape/admission
/// instance. Harness seeds never supply production call identity.
pub(crate) unsafe fn set_instance(ctx: *mut TaskCtx, instance: [u8; 16]) {
    state(ctx).instance = instance;
}

/// The run state of the execution `ctx`'s run belongs to: its root's.
fn root_run(ctx: *mut TaskCtx) -> *mut RunShared {
    // SAFETY: the context is live, and a callee run's root outlives it.
    unsafe {
        let run = (*ctx).run_ptr();
        if (*run).root.is_null() {
            run
        } else {
            (*run).root
        }
    }
}

/// The context's execution. A run its launcher did not configure has no runners but may
/// make test runners; it is created at the first use, by the root, whose task runs.
fn execution(ctx: *mut TaskCtx) -> &'static mut Execution {
    // SAFETY: the execution lives as long as the root run, and only the running task reaches
    // it; no reference is kept across a switch.
    unsafe {
        (*(*root_run(ctx)).runners.get())
            .get_or_insert_with(|| Box::new(Execution::new(Config::default(), ctx)))
    }
}

fn state(ctx: *mut TaskCtx) -> &'static mut Runners {
    let e = execution(ctx);
    let active = e.active;
    &mut e.runs[active].state
}

/// Whether the execution decides what an idle run does: a call was sent.
///
/// # Safety
/// `ctx` is live.
pub(crate) unsafe fn driving(ctx: *mut TaskCtx) -> bool {
    if unsafe { (*ctx).run_ref().free_production } {
        return false;
    }
    // SAFETY: the context is live.
    unsafe { (*root_run(ctx)).runners.as_ref() }.is_some_and(|e| e.driving)
}

/// The active run has nothing runnable: start its new calls, forward its cancellations, then
/// let the execution run the ready runs until this one is next (RUN-12).
///
/// # Safety
/// `ctx` is the running task's, which is parked or holding, and the caller holds nothing a
/// stop must drop.
pub(crate) unsafe fn idle(ctx: *mut TaskCtx) -> Stopped<()> {
    // The launcher or a runner entry installed this driver. Merely reaching
    // a scheduling point must not root the code that starts callee runs.
    let idle = unsafe { (*root_run(ctx)).runners.as_ref() }
        .expect("runner idle without an execution")
        .idle;
    unsafe { idle(ctx) }
}

unsafe fn drive_idle(ctx: *mut TaskCtx) -> Stopped<()> {
    let me = execution(ctx).active;
    // SAFETY: the context is live.
    unsafe {
        take_calls(ctx, me)?;
        take_cancels(ctx, me)?;
    }
    if let Some(callee) = &mut execution(ctx).runs[me].callee {
        callee.suspensions += 1;
        if callee.crash_after.is_some_and(|at| callee.suspensions > at) {
            // The place crashed while its callee waited: the caller learns it is in doubt.
            // SAFETY: this run's fibers never resume.
            unsafe { retire(ctx, me, End::Abandoned) };
        }
    }
    let next = next_run()?;
    if next != me {
        // SAFETY: the context is live and nothing is borrowed across the switch.
        unsafe { switch(ctx, next) };
    }
    Ok(())
}

/// The run to execute next: the front of the ready queue; else, first the runs whose adapter
/// already has a ready wait, then the first run whose adapter made progress waiting, in run
/// order; Blocked when none can (CONC-8).
fn next_run() -> Stopped<usize> {
    let e = execution(crate::stop::running());
    loop {
        if let Some(next) = e.ready.pop_front() {
            return Ok(next);
        }
        let now = ready_now(e)?;
        if !now.is_empty() {
            e.ready.extend(now);
            continue;
        }
        let mut progressed = None;
        for index in 0..e.runs.len() {
            if e.runs[index].ended {
                continue;
            }
            // SAFETY: the run's root context is live and its binding bound.
            if unsafe { crate::host_waits::wait(e.runs[index].root) }? {
                progressed = Some(index);
                break;
            }
        }
        let Some(index) = progressed else {
            let mut waits = Vec::new();
            for run in e.runs.iter().filter(|run| !run.ended) {
                // SAFETY: the run's root context is live and nothing can run.
                if unsafe { (*run.root).run_ref() }.tasks.is_some() {
                    waits.extend(unsafe { crate::tasks::blocked_waits(run.root) });
                }
            }
            return Err(Stop::Blocked(waits));
        };
        // A shared readiness may have woken another run's wait.
        let now = ready_now(e)?;
        if now.is_empty() {
            e.ready(index);
        } else {
            e.ready.extend(now);
        }
    }
}

/// The live runs whose adapter has a ready wait, in run order.
fn ready_now(e: &mut Execution) -> Stopped<Vec<usize>> {
    let mut now = Vec::new();
    for (index, run) in e.runs.iter().enumerate() {
        // SAFETY: the run's root context is live and its binding bound.
        if !run.ended
            && unsafe { Binding::get(run.root) }
                .adapter
                .has_ready()
                .map_err(Stop::refusal)?
        {
            now.push(index);
        }
    }
    Ok(now)
}

/// Suspend the active run in its running task `ctx` and resume run `to`; returns when a later
/// switch resumes this run.
///
/// # Safety
/// `ctx` is the running task's, the caller holds nothing a stop must drop, and `to` is
/// suspended or prepared.
unsafe fn switch(ctx: *mut TaskCtx, to: usize) {
    let e = execution(ctx);
    let from = e.active;
    e.runs[from].resume = ctx;
    let save = &raw mut e.runs[from].sp;
    // SAFETY: caller contract.
    unsafe { enter(ctx, to) };
    // SAFETY: `to` is suspended or prepared, and the save slot outlives the switch.
    unsafe { fiber::switch(save, execution(ctx).runs[to].sp) };
}

/// Make run `to` the active one and its suspended task the running one on this thread.
///
/// # Safety
/// `ctx` is the running task's; the caller switches to `to`'s fiber next.
unsafe fn enter(ctx: *mut TaskCtx, to: usize) {
    let e = execution(ctx);
    e.active = to;
    let run = &mut e.runs[to];
    if let Some(callee) = &mut run.callee {
        callee.started = true;
    }
    let target = run.resume;
    // SAFETY: both contexts are live and each run has a worker of its own.
    unsafe {
        // A waiting caller cannot retain the last process permit. The callee
        // executes on this thread under its own executor/heap and returns the
        // permit again when the execution switches back.
        crate::pool::release_permit(ctx);
        (*ctx).switch_out();
        TaskCtx::switch_in(target, (*target).worker_ptr());
        crate::pool::acquire_permit(target);
    }
}

/// Leave the active callee run, which ended, for run `to` for good.
///
/// # Safety
/// As for [`switch`]; nothing resumes this fiber.
unsafe fn depart(ctx: *mut TaskCtx, to: usize) -> ! {
    unsafe {
        enter(ctx, to);
        let mut abandoned = std::ptr::null_mut();
        fiber::switch(&raw mut abandoned, execution(ctx).runs[to].sp);
    }
    fault("an ended run was resumed")
}

/// The Io projections a grant set gives its callee's root Io (RUN-4).
fn projections(grants: &IoGrants) -> std::collections::BTreeSet<String> {
    [
        ("stderr", grants.stderr.is_some()),
        ("stdout", grants.stdout.is_some()),
        ("tasks", grants.tasks.is_some()),
        ("net", grants.net.is_some()),
        ("process", grants.process.is_some()),
        ("random", grants.random.is_some()),
        ("dir", !grants.dirs.is_empty()),
    ]
    .into_iter()
    .filter(|(_, granted)| *granted)
    .map(|(name, _)| name.to_owned())
    .collect()
}

/// Start run `me`'s sent calls, each as a new run at the back of the ready queue. Replay
/// starts none: the recorded completions stand in for them.
///
/// # Safety
/// `ctx` is live.
unsafe fn take_calls(ctx: *mut TaskCtx, me: usize) -> Stopped<()> {
    let starts = std::mem::take(&mut execution(ctx).runs[me].state.starts);
    for token in starts {
        let e = execution(ctx);
        let Some(readiness) = e.readiness.clone() else {
            continue;
        };
        let index = e.runs.len();
        let call = e.runs[me]
            .state
            .calls
            .get_mut(&token)
            .unwrap_or_else(|| fault("a start without its call"));
        let start = call
            .start
            .take()
            .unwrap_or_else(|| fault("a call started twice"));
        call.callee = Some(index);
        let key = call.key;
        let seed = |name: &str| hirsel_run_schema::callee_seed(&key, name);
        let grants = call.grant.grants.clone();
        let (ordinal, crash_after) = (call.ordinal, call.crash_after);
        let recorded = call.grant.entry.recording.as_deref() != Some("none");
        e.tags += 1;
        let tag = e.tags;
        let stdout = e
            .config
            .stdout
            .clone()
            .unwrap_or_else(|| crate::host::Sink::new(crate::launch::RawStdout));
        let mut io = RealIo::from_pinned_mounts([], stdout).expect("an empty mount set");
        io.set_projections(projections(&grants));
        io.set_cooperative(readiness.tagged(tag));
        io.clock_grants(
            grants.monotonic_clock.is_some(),
            grants.wall_clock.is_some(),
        );
        io.set_random(
            hirsel_run_schema::ProviderContract::RandomSplitMix64,
            seed("random"),
        );
        // A runner entry recording `none` keeps its callees off tape.
        let tape = e.config.tape.as_ref().filter(|_| recorded).map(|tape| {
            let mut name = tape.clone().into_os_string();
            name.push(format!(".call-{ordinal}"));
            std::path::PathBuf::from(name)
        });
        if let Some(tape) = &tape {
            let refused = |error: String| {
                Stop::Environmental(hirsel_run_schema::Environmental::Recording(format!(
                    "the callee's Io tape: {error}"
                )))
            };
            let file = io
                .prepare_recording_with_files(tape, &[])
                .map_err(|e| refused(e.to_string()))?;
            let tree = io.snapshot().map_err(|e| refused(format!("{e:?}")))?;
            let record = crate::launch::Recording {
                arguments: vec![],
                run_identity: [0; 32],
                hash_seed: seed("hash"),
                random_seed: seed("random"),
                test_seed: seed("test"),
                binding: None,
                tree,
                events: vec![],
            };
            let writer = crate::launch::RecordingWriter::new(file, &record)
                .map_err(|e| refused(e.to_string()))?;
            io.record_to(writer);
        }
        io.discard_history();
        let io = Rc::new(RefCell::new(io));
        let mut binding = Box::new(Binding::from_box(Box::new(crate::launch::Shared(
            io.clone(),
        ))));
        let mut config = e.config.runtime;
        config.hash_seed = seed("hash");
        let mut callee = std::ptr::null_mut();
        // SAFETY: the configuration is the root's, which created a context already.
        if unsafe { crate::context::hirsel_rt_v2_ctx_create(&config, &mut callee) } != 0 {
            fault("a callee run's context could not be created");
        }
        // SAFETY: the callee context was just created and runs nothing yet.
        unsafe {
            (*callee).run().root = root_run(ctx);
            binding.bind(&mut *callee);
            if let Some(table) = e.config.failure_table {
                crate::isolate::attach(&mut *callee, table);
            }
        }
        let stack = Stack::new();
        let sp = stack.prepare(callee_entry, callee.cast(), std::ptr::null_mut());
        e.runs.push(Run {
            root: callee,
            resume: callee,
            sp,
            ended: false,
            state: Runners {
                instance: hirsel_run_schema::callee_instance(&key),
                ..Runners::default()
            },
            callee: Some(Callee {
                caller: me,
                token,
                started: false,
                suspensions: 0,
                crash_after,
                start,
                reply: None,
                chain: Vec::new(),
                stack: Some(stack),
                io,
                binding,
                recording: tape.is_some(),
            }),
        });
        e.ready.push_back(index);
    }
    Ok(())
}

/// RUN-9: record each cancellation of run `me`'s waiting tasks once, forwarded to the callee
/// of its sent call.
///
/// # Safety
/// `ctx` is live and `me` is the active run.
unsafe fn take_cancels(ctx: *mut TaskCtx, me: usize) -> Stopped<()> {
    let mut cancels = Vec::new();
    // SAFETY: the context is live.
    if unsafe { (*ctx).run_ref() }.tasks.is_some() {
        for (token, call) in &mut execution(ctx).runs[me].state.calls {
            if call.forwarded.is_none()
                && call.completion.is_none()
                && let Some(task) = call.task
                && let Some(cause) = crate::tasks::task_canceled(ctx, task)
            {
                call.forwarded = Some(cause);
                cancels.push((*token, call.key, cause));
            }
        }
    }
    for (token, key, cause) in cancels {
        // SAFETY: the context is live and its binding bound.
        let binding = unsafe { Binding::get(ctx) };
        match binding.adapter.call(Request::RunnerCancel { key, cause }) {
            Ok(Completion::Ready) => {}
            Ok(_) => return Err(Stop::refusal(AdapterError::InvalidCompletion)),
            Err(error) => return Err(Stop::refusal(error)),
        }
        let e = execution(ctx);
        let Some(callee) = e.runs[me].state.calls[&token].callee else {
            continue;
        };
        if e.runs[callee].ended {
            continue;
        }
        if e.runs[callee].callee.as_ref().is_some_and(|c| c.started) {
            // The callee observes the cancellation; its caller waits for its outcome.
            crate::tasks::cancel_root(e.runs[callee].root, crate::tasks::CANCEL_PARENT);
            e.ready(callee);
        } else {
            e.ready.retain(|&r| r != callee);
            // SAFETY: the callee never ran.
            unsafe { finish(callee, &End::Canceled) }?;
            e.runs[callee].ended = true;
            ended(me, token, End::Canceled);
        }
    }
    Ok(())
}

/// A callee run's end on its own tape: its own outcome's bytes, then the trailer.
///
/// # Safety
/// Run `index` is a live callee run.
unsafe fn finish(index: usize, end: &End) -> Stopped<()> {
    let e = execution(crate::stop::running());
    let table = e.config.failure_table;
    let callee = e.runs[index]
        .callee
        .as_mut()
        .unwrap_or_else(|| fault("the root ended as a callee"));
    let outcome = match end {
        End::Replied(tag, bytes) => Some(match *tag {
            REPLY_OK => CalleeOutcome::Ok(bytes.clone()),
            REPLY_ERROR => CalleeOutcome::Err(bytes.clone()),
            _ => CalleeOutcome::Failed(bytes.clone()),
        }),
        End::Canceled | End::Abandoned => None,
    };
    if let Some(outcome) = outcome {
        match callee.binding.adapter.call(Request::CallReturn(outcome)) {
            Ok(Completion::Ready) => {}
            Ok(_) => return Err(Stop::refusal(AdapterError::InvalidCompletion)),
            Err(error) => return Err(Stop::refusal(error)),
        }
    }
    if !callee.recording {
        return Ok(());
    }
    use hirsel_run_schema::{Returned, RunOutcome};
    let outcome = match end {
        End::Replied(REPLY_OK, _) => RunOutcome::Returned(Returned::Ok),
        End::Replied(REPLY_ERROR, _) => RunOutcome::Returned(Returned::Error),
        End::Replied(_, _) => {
            let table =
                table.unwrap_or_else(|| fault("a callee failure without the failure table"));
            let mut failures = callee.chain.iter().map(|(record, _)| table.failure(record));
            let primary = failures
                .next()
                .unwrap_or_else(|| fault("a faulted callee without its failure"));
            RunOutcome::Failed {
                primary,
                causes: failures.collect(),
            }
        }
        End::Canceled => {
            RunOutcome::Tooling("the call was cancelled before its callee started".into())
        }
        End::Abandoned => {
            RunOutcome::Tooling("the place stopped the callee without an outcome".into())
        }
    };
    callee
        .io
        .borrow_mut()
        .finish_recording(&outcome)
        .map_err(|e| {
            Stop::Environmental(hirsel_run_schema::Environmental::Recording(format!(
                "the callee's Io tape: {e}"
            )))
        })
}

/// Deliver a callee's end to its caller's call: its completion, which the caller's next
/// scheduling point takes, and make the caller ready.
fn ended(caller: usize, token: u64, end: End) {
    let e = execution(crate::stop::running());
    let run = &mut e.runs[caller];
    let call = run
        .state
        .calls
        .get(&token)
        .unwrap_or_else(|| fault("an outcome for an unknown runner call"));
    let callee = call
        .callee
        .map(|_| hirsel_run_schema::callee_instance(&call.key));
    // A crashed place sends no reply to lose.
    let outcome = if call.lose_reply && !matches!(end, End::Canceled | End::Abandoned) {
        CallOutcome::InDoubt(LOST)
    } else {
        match end {
            End::Replied(REPLY_OK, bytes) => CallOutcome::Ok(bytes),
            End::Replied(REPLY_ERROR, bytes) => CallOutcome::Failed(bytes),
            End::Replied(_, bytes) => CallOutcome::Faulted(bytes),
            End::Canceled => CallOutcome::Canceled(call.forwarded.unwrap_or(0)),
            End::Abandoned => CallOutcome::InDoubt(ABANDONED),
        }
    };
    let callee = if matches!(outcome, CallOutcome::Canceled(_)) {
        None
    } else {
        callee
    };
    // SAFETY: the caller's root context is live and its binding bound.
    unsafe { Binding::get(run.root) }
        .adapter
        .deliver(token, RunnerCompletion { outcome, callee });
    e.ready(caller);
}

/// End callee run `me` with `end`, deliver it to its caller and run the next run.
///
/// # Safety
/// `ctx` is the running task of callee run `me`, whose fibers never resume.
unsafe fn retire(ctx: *mut TaskCtx, me: usize, end: End) -> ! {
    unsafe {
        crate::stop::check(ctx, finish(me, &end));
        let e = execution(ctx);
        e.runs[me].ended = true;
        e.ready.retain(|&r| r != me);
        let callee = e.runs[me].callee.as_ref().unwrap();
        ended(callee.caller, callee.token, end);
        // A stop here ends the execution.
        let next = crate::stop::check(ctx, next_run());
        depart(ctx, next)
    }
}

/// The root fiber of a callee run: its main, then its end.
unsafe extern "C" fn callee_entry(ctx: *mut u8, _: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let me = execution(ctx).active;
    let (code, environment) = execution(ctx).runs[me]
        .callee
        .as_ref()
        .map(|c| c.start.main)
        .unwrap_or_else(|| fault("a callee started without its main"));
    // SAFETY: the main is compiled `fn() -> ()` code with a static environment, which the
    // caller sent.
    let status = unsafe {
        let code = std::mem::transmute::<usize, TaskCode>(code);
        code(ctx, environment as *mut Header, 0, 0)
    };
    if status != STATUS_OK {
        fault("a callee's main failed outside its catch");
    }
    let callee = execution(ctx).runs[me].callee.as_mut().unwrap();
    let reply = callee.reply.take();
    let end = match (callee.crash_after, reply) {
        (Some(_), _) => End::Abandoned,
        (None, Some((tag, bytes))) => End::Replied(tag, bytes),
        (None, None) => fault("a callee returned without its reply"),
    };
    // SAFETY: this fiber never resumes.
    unsafe { retire(ctx, me, end) }
}

fn runner(ctx: *mut TaskCtx, word: u64) -> RunnerGrant {
    word.checked_sub(1)
        .and_then(|i| state(ctx).grants.get(i as usize))
        .cloned()
        .unwrap_or_else(|| fault("an invalid Runner handle"))
}

fn runner_word(ctx: *mut TaskCtx, runner: RunnerGrant) -> u64 {
    let grants = &mut state(ctx).grants;
    grants.push(runner);
    grants.len() as u64
}

/// The full path of the definition an Entry's code word names.
fn entry_path(ctx: *mut TaskCtx, code: usize) -> String {
    let entries = &execution(ctx).config.entries;
    entries
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, path)| path.clone())
        .unwrap_or_else(|| fault("an Entry whose definition the program does not list"))
}

/// The completion's outcome code.
fn code(outcome: &CallOutcome) -> u64 {
    match outcome {
        CallOutcome::Ok(_) => OUTCOME_OK,
        CallOutcome::Failed(_) => OUTCOME_FAILED,
        CallOutcome::Faulted(_) => OUTCOME_FAULTED,
        CallOutcome::NotRun(reason) => OUTCOME_NOT_RUN | u64::from(*reason),
        CallOutcome::InDoubt(reason) => OUTCOME_IN_DOUBT | u64::from(*reason),
        CallOutcome::Canceled(cause) => OUTCOME_CANCELED | u64::from(*cause),
    }
}

/// RUN-4: the request, the checks in order, then the place's contract, for the running
/// task's call of the Entry with code word `entry.0`: the call's ticket. A refused call
/// completes at once; a sent one parks for its callee (`spawn`: a `runner.spawn` call,
/// whose child waits).
///
/// # Safety
/// `ctx` is live and the caller holds nothing a stop must drop.
unsafe fn send(
    ctx: *mut TaskCtx,
    runner: u64,
    entry: (usize, usize),
    argument: Vec<u8>,
    main: (usize, usize),
    form: CallForm,
    spawn: bool,
) -> Stopped<u64> {
    let grant = self::runner(ctx, runner);
    let path = entry_path(ctx, entry.0);
    let e = execution(ctx);
    let facts = e.config.callees.get(&path).cloned();
    let fault_seed = e.config.fault_seed;
    let request_schemas = e.config.request_schemas.clone();
    let run = state(ctx);
    let ordinal = run.requests;
    run.requests += 1;
    let key = hirsel_run_schema::call_key(&run.instance, ordinal);
    // The requested grant set and allowlist in canonical codec bytes (RUN-11).
    let Some(grants) = hirsel_run_schema::GrantSet::of(&grant.grants).canonical() else {
        fault("a runner's grant set has a net rule the codec cannot encode");
    };
    let definitions = hirsel_run_schema::allowlist_bytes(grant.definitions.as_deref());
    let (grants, definitions) = if let Some(schemas) = request_schemas {
        schemas.encode(&grants, &definitions)
    } else {
        if unsafe { (*ctx).run_ref().free_production } {
            fault("production runners require compiled request schemas");
        }
        (grants, definitions)
    };
    let (fingerprint, code_hash) = facts
        .as_ref()
        .map_or(([0; 32], [0; 32]), |f| (f.fingerprint, f.code));
    let request = RunnerRequest {
        runner: grant.entry.name.clone(),
        form,
        path: path.clone(),
        digest: hirsel_run_schema::request_digest(
            form,
            &path,
            &fingerprint,
            &code_hash,
            &grants,
            &definitions,
            &argument,
        ),
        grants,
        definitions,
        key,
        argument: argument.clone(),
    };
    // Authority at the caller: the allowlist, then the place's code policy; then
    // cancellation, which sends nothing. A spawned call's child is canceled with its
    // spawner, as its parent.
    let listed = grant
        .definitions
        .as_ref()
        .is_none_or(|allowed| allowed.contains(&path));
    let admitted =
        !grant.entry.admits_only_safe() || facts.is_some_and(|f| f.reach == CodeReach::Safe);
    // SAFETY: the context is live.
    let canceled = unsafe { crate::tasks::canceled(ctx) }.map(|cause| {
        if spawn {
            crate::tasks::CANCEL_PARENT
        } else {
            cause
        }
    });
    let placed = if !listed || !admitted {
        Err(CallOutcome::NotRun(DENIED))
    } else if let Some(cause) = canceled {
        Err(CallOutcome::Canceled(cause))
    } else {
        match run
            .places
            .entry(grant.entry.name.clone())
            .or_default()
            .place(&grant.entry, fault_seed, ordinal)
        {
            Placed::NotRun(reason) => Err(CallOutcome::NotRun(reason)),
            Placed::InDoubt(reason) => Err(CallOutcome::InDoubt(reason)),
            Placed::Sent {
                crash_after,
                lose_reply,
            } => Ok((crash_after, lose_reply)),
        }
    };
    if unsafe { (*ctx).run_ref().free_production } {
        drop(path);
        return unsafe {
            native::send(
                ctx,
                request,
                grant,
                ordinal,
                placed,
                Start {
                    main,
                    entry,
                    argument,
                    form,
                },
                spawn,
            )
        };
    }
    // SAFETY: the context is live and its binding bound.
    let readiness = unsafe { Binding::get(ctx) }.adapter.runner_readiness();
    let e = execution(ctx);
    e.driving = true;
    if e.active == 0 && e.readiness.is_none() {
        e.readiness = readiness;
    }
    let mut call = Call {
        task: None,
        grant,
        ordinal,
        key,
        place: None,
        lose_reply: false,
        crash_after: None,
        forwarded: None,
        start: None,
        callee: None,
        completion: None,
    };
    let token = match placed {
        Err(outcome) => {
            let completion = RunnerCompletion {
                outcome,
                callee: None,
            };
            // SAFETY: as above.
            let token = unsafe { crate::host_waits::next_token(ctx) };
            unsafe { Binding::get(ctx) }
                .adapter
                .deliver(token, completion.clone());
            // SAFETY: as above.
            match unsafe { crate::host_waits::send(ctx, Request::RunnerCall(Box::new(request))) }? {
                // Replay checks the recorded completion against this decision.
                Ok(Completion::Runner(recorded)) if recorded == completion => {}
                _ => return Err(Stop::refusal(AdapterError::InvalidCompletion)),
            }
            call.completion = Some(completion);
            token
        }
        Ok((crash_after, lose_reply)) => {
            call.place = Some(call.grant.entry.name.clone());
            call.lose_reply = lose_reply;
            call.crash_after = crash_after;
            // SAFETY: as above.
            match unsafe { crate::host_waits::send(ctx, Request::RunnerCall(Box::new(request))) }? {
                Err(token) => {
                    call.start = Some(Start {
                        main,
                        entry,
                        argument,
                        form,
                    });
                    state(ctx).starts.push(token);
                    token
                }
                _ => return Err(Stop::refusal(AdapterError::InvalidCompletion)),
            }
        }
    };
    state(ctx).calls.insert(token, call);
    Ok(token)
}

/// The outcome code of call `ticket`, parking the running task until its callee ends.
///
/// # Safety
/// `ctx` is live and the caller holds nothing a stop must drop.
unsafe fn wait(ctx: *mut TaskCtx, ticket: u64) -> Stopped<u64> {
    if unsafe { (*ctx).run_ref().free_production } {
        return unsafe { native::wait(ctx, ticket) };
    }
    let call = state(ctx)
        .calls
        .get_mut(&ticket)
        .unwrap_or_else(|| fault("a runner wait without its call"));
    if let Some(completion) = &call.completion {
        let code = code(&completion.outcome);
        // SAFETY: the context is live.
        unsafe { crate::tasks::try_yield(&mut *ctx) }?;
        return Ok(code);
    }
    call.task = Some(crate::tasks::current_task(ctx));
    // SAFETY: caller contract.
    let completion = match unsafe { crate::host_waits::await_sent(ctx, ticket) }? {
        Completion::Runner(completion) => completion,
        _ => return Err(Stop::refusal(AdapterError::InvalidCompletion)),
    };
    let run = state(ctx);
    let call = run.calls.get_mut(&ticket).unwrap();
    if let Some(place) = call.place.as_ref().and_then(|p| run.places.get_mut(p)) {
        place.answered();
    }
    let code = code(&completion.outcome);
    call.completion = Some(completion);
    Ok(code)
}

// ----- The Hirsel-ABI entries ---------------------------------------------------------------

/// A borrowed String's text.
///
/// # Safety
/// `owner`, `start` and `length` name a live String.
unsafe fn string(owner: *mut Header, start: u64, length: u64) -> String {
    let bytes = unsafe { crate::io::slice_bytes(owner, start, length) };
    String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| fault("a String that is not UTF-8"))
}

/// `io.runner(rt, name) -> Result[Runner, GrantError]`: the runner the run's configuration
/// names (RUN-1); a callee's world names none.
///
/// # Safety
/// The context is live, `io` a borrowed Io word, `owner`/`start`/`length` a live String and
/// `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_io_runner(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut GrantResult,
    _io: *const u64,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    // SAFETY: caller contract.
    let name = unsafe { string(owner, start, length) };
    let e = execution(ctx);
    let configured = if e.active == 0 {
        e.config.runners.iter().find(|r| r.name == name).cloned()
    } else {
        None
    };
    let result = match configured {
        Some(entry) => Ok(runner_word(
            ctx,
            RunnerGrant {
                grants: entry.grants.clone().unwrap_or_default(),
                definitions: entry.definitions.clone(),
                entry,
            },
        )),
        None => Err(crate::io::IoError::NotGranted),
    };
    unsafe {
        out.write(GrantResult::of(result));
        crate::tasks::yield_now(&mut *ctx);
    }
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// `test.runner(t) -> Runner`: a `sim@1` runner modeling `local.threads@1` with an empty
/// grant set (RUN-4).
///
/// # Safety
/// The context is live, `test` a borrowed Test word and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_runner(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
    _test: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    let entry = RunnerEntry {
        name: "test".into(),
        provider: "sim@1".into(),
        threads: None,
        queue: None,
        node: Some("test".into()),
        models: Some("local.threads@1".into()),
        faults: BTreeMap::new(),
        recording: None,
        retry: None,
        cache: None,
        code: None,
        admission: None,
        grants: None,
        definitions: None,
        endpoint: None,
        server_name: None,
        roots: None,
        identity: None,
        timeout_ns: None,
        reconnect: None,
        reconcile: false,
    };
    let word = runner_word(
        ctx,
        RunnerGrant {
            entry,
            grants: IoGrants::default(),
            definitions: None,
        },
    );
    unsafe {
        out.write(word);
        crate::tasks::yield_now(&mut *ctx);
    }
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// The six flags of a grant set, in `GrantSet`'s field order.
fn flags(grants: &IoGrants) -> [bool; 6] {
    [
        grants.stdout.is_some(),
        grants.stderr.is_some(),
        grants.tasks.is_some(),
        grants.monotonic_clock.is_some(),
        grants.wall_clock.is_some(),
        grants.random.is_some(),
    ]
}

/// `$intrinsic.RunnerGrants(r) -> u64`: the runner's grant set as `runner.grants`' compiled
/// body builds its `GrantSet`, one bit per flag in field order. A native grant set names no
/// directory, rule or program, so its lists are empty.
///
/// # Safety
/// The context is live, `r` a borrowed Runner word and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_grants(
    ctx: *mut TaskCtx,
    out: *mut u64,
    r: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    let grants = runner(ctx, unsafe { r.read() }).grants;
    let bits = flags(&grants)
        .into_iter()
        .enumerate()
        .fold(0, |bits, (i, set)| bits | u64::from(set) << i);
    unsafe {
        out.write(bits);
        crate::tasks::yield_now(&mut *ctx);
    }
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// `runner.narrow(r, keep) -> Result[Runner, GrantError]` (RUN-2): a runner over `keep` when
/// the runner's grant set covers it. `keep` is a `GrantSet`: six bools, then its `dirs`,
/// `net` and `process` Lists at 8, 16 and 24. A native runner's grant set has no directory,
/// rule or program, so a `keep` naming one is never covered.
///
/// # Safety
/// The context is live, `r` a borrowed Runner word, `keep` a borrowed GrantSet and `out`
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_narrow(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut GrantResult,
    r: *const u64,
    keep: *const u8,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    let runner = runner(ctx, unsafe { r.read() });
    let wanted: Vec<bool> = (0..6).map(|i| unsafe { keep.add(i).read() } != 0).collect();
    let lists = [8, 16, 24].iter().any(|&offset| unsafe {
        let block = keep.add(offset).cast::<*mut Header>().read();
        crate::alloc::payload(block).cast::<u64>().read() != 0
    });
    let held = flags(&runner.grants);
    let covered = !lists && wanted.iter().zip(held).all(|(&want, has)| !want || has);
    let result = if covered {
        let mut narrow = IoGrants::default();
        let g = &runner.grants;
        for (i, (granted, slot)) in [
            (&g.stdout, &mut narrow.stdout),
            (&g.stderr, &mut narrow.stderr),
            (&g.tasks, &mut narrow.tasks),
            (&g.monotonic_clock, &mut narrow.monotonic_clock),
            (&g.wall_clock, &mut narrow.wall_clock),
            (&g.random, &mut narrow.random),
        ]
        .into_iter()
        .enumerate()
        {
            if wanted[i] {
                *slot = granted.clone();
            }
        }
        Ok(runner_word(
            ctx,
            RunnerGrant {
                grants: narrow,
                ..runner
            },
        ))
    } else {
        Err(crate::io::IoError::NotGranted)
    };
    unsafe {
        out.write(GrantResult::of(result));
        crate::tasks::yield_now(&mut *ctx);
    }
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// `runner.only(r, definitions) -> Result[Runner, GrantError]` (RUN-2): a runner whose
/// allowlist is `definitions`, which must be sorted, unique and within the runner's.
///
/// # Safety
/// The context is live, `r` a borrowed Runner word, `definitions` a live `List[String]`
/// block and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_only(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut GrantResult,
    r: *const u64,
    definitions: *mut Header,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    let runner = runner(ctx, unsafe { r.read() });
    let listed = unsafe { crate::process::list(definitions, 24, |at| crate::process::text(at)) };
    let sorted = listed.windows(2).all(|pair| pair[0] < pair[1]);
    let within = runner
        .definitions
        .as_ref()
        .is_none_or(|allowed| listed.iter().all(|path| allowed.contains(path)));
    let result = if sorted && within {
        Ok(runner_word(
            ctx,
            RunnerGrant {
                definitions: Some(listed),
                ..runner
            },
        ))
    } else {
        Err(crate::io::IoError::NotGranted)
    };
    unsafe {
        out.write(GrantResult::of(result));
        crate::tasks::yield_now(&mut *ctx);
    }
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// `runner.entry_path(e) -> String` (RUN-3): the full path of the definition `e` names.
///
/// # Safety
/// The context is live, `entry` a borrowed Entry and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_entry_path(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut Slice,
    entry: *const usize,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    let path = entry_path(ctx, unsafe { entry.read() });
    unsafe { out.write(allocate_owned_bytes(ctx, path.into_bytes())) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// `$intrinsic.RunnerSend[F](r, e, argument, main, form) -> u64`: the running task's call of
/// `e` with `argument`'s bytes (RUN-4), sent or refused: its ticket, which `RunnerWait` and
/// `RunnerTake` read. `main` is the callee's root, `$native.runner_main[F]`; `form` is 1 for
/// `runner.call` and 2 for a `runner.spawn` call, whose child waits.
///
/// # Safety
/// The context is live, `r` a borrowed Runner word, `entry` a borrowed Entry, the argument a
/// live Bytes and `main` a static `fn() -> ()`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_send(
    ctx: *mut TaskCtx,
    out: *mut u64,
    r: *const u64,
    entry: *const usize,
    owner: *mut Header,
    start: u64,
    length: u64,
    main_code: *const c_void,
    main_environment: *const c_void,
    form: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    // SAFETY: caller contract.
    unsafe {
        let entry = (entry.read(), entry.add(1).read());
        let argument = crate::io::slice_bytes(owner, start, length).to_vec();
        let main = (main_code as usize, main_environment as usize);
        let (call, spawn) = match form {
            0 => (CallForm::Compute, false),
            1 => (CallForm::Call, false),
            _ => (CallForm::Call, true),
        };
        let ticket =
            crate::stop::check(ctx, send(ctx, r.read(), entry, argument, main, call, spawn));
        out.write(ticket);
    }
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// `$intrinsic.RunnerWait(ticket) -> u64`: the call's outcome code, parking until its callee
/// ends.
///
/// # Safety
/// The context is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_wait(
    ctx: *mut TaskCtx,
    out: *mut u64,
    ticket: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    let code = unsafe { crate::stop::check(ctx, wait(ctx, ticket)) };
    unsafe { out.write(code) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// `$intrinsic.RunnerTake(ticket) -> Bytes`: the bytes of the call's `Ok`, `Failed` or
/// `Faulted` outcome, fresh in this run, empty for the others; the call is over.
///
/// # Safety
/// The context is live, the call waited for and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_take(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut Slice,
    ticket: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::pin_home(ctx) };
    }
    let mut call = state(ctx)
        .calls
        .remove(&ticket)
        .unwrap_or_else(|| fault("a runner take without its call"));
    let bytes = match call.completion.take().map(|c| c.outcome) {
        Some(CallOutcome::Ok(bytes) | CallOutcome::Failed(bytes) | CallOutcome::Faulted(bytes)) => {
            bytes
        }
        Some(_) => Vec::new(),
        None => fault("a runner take before its wait"),
    };
    drop(call);
    unsafe { out.write(allocate_owned_bytes(ctx, bytes)) };
    if unsafe { (*ctx).run_ref().free_production } {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    STATUS_OK
}

/// The running callee run's call.
fn callee(ctx: *mut TaskCtx) -> &'static mut Callee {
    let e = execution(ctx);
    let active = e.active;
    e.runs[active]
        .callee
        .as_mut()
        .unwrap_or_else(|| fault("a runner callee entry outside a callee run"))
}

/// `$intrinsic.RunnerEntry[F]() -> F`: the running callee's entry.
///
/// # Safety
/// The context is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_entry(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut usize,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let (code, environment) = native::call(ctx)
        .map(|call| call.entry)
        .unwrap_or_else(|| callee(ctx).start.entry);
    unsafe {
        out.write(code);
        out.add(1).write(environment);
    }
    STATUS_OK
}

/// `$intrinsic.RunnerArgument() -> Bytes`: the running callee's argument bytes, fresh in its
/// run.
///
/// # Safety
/// The context is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_argument(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut Slice,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let argument = match native::call(ctx) {
        Some(call) => call.argument.lock().unwrap().take().unwrap_or_default(),
        None => std::mem::take(&mut callee(ctx).start.argument),
    };
    unsafe { out.write(allocate_owned_bytes(ctx, argument)) };
    STATUS_OK
}

/// Hold the removed call/argument payload outside an abandonable conversion
/// frame when Free native allocation can stop the run.
unsafe fn allocate_owned_bytes(ctx: *mut TaskCtx, bytes: Vec<u8>) -> Slice {
    unsafe {
        crate::tasks::free::host::native::convert_result(
            ctx,
            Completion::Bytes(Ok(bytes)),
            |completion| {
                let Completion::Bytes(Ok(bytes)) = completion else {
                    unreachable!()
                };
                crate::text::allocate_bytes(ctx, bytes)
            },
        )
    }
}

/// `$intrinsic.RunnerIo() -> Io`: the running callee's root Io (`runner.call`'s form).
///
/// # Safety
/// The context is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_io(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let form = native::call(ctx)
        .map(|call| call.form)
        .unwrap_or_else(|| callee(ctx).start.form);
    if form != CallForm::Call {
        fault("RunnerIo in a compute callee");
    }
    unsafe { out.write(crate::launch::IO_HANDLE) };
    STATUS_OK
}

/// `$intrinsic.RunnerTasks() -> Tasks`: the word the callee's catch begins with; the isolate
/// entries read no Tasks state.
///
/// # Safety
/// The context is live and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_tasks(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if native::call(ctx).is_none() {
        callee(ctx);
    }
    unsafe { out.write(0) };
    STATUS_OK
}

/// `$intrinsic.RunnerReply(tag, reply)`: the running callee's outcome bytes: its entry's
/// encoded `Ok` (tag 0) or `Error` (tag 1) payload, or its caught failure (tag 2), whose chain
/// its tape's trailer reports.
///
/// # Safety
/// The context is live and `owner`/`start`/`length` a live Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_runner_reply(
    ctx: *mut TaskCtx,
    tag: u64,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let bytes = unsafe { crate::io::slice_bytes(owner, start, length) }.to_vec();
    if let Some(call) = native::call(ctx) {
        let chain = if tag == REPLY_FAILED {
            crate::isolate::caught(ctx).clone()
        } else {
            Vec::new()
        };
        if call
            .reply
            .lock()
            .unwrap()
            .replace(native::Reply { tag, bytes, chain })
            .is_some()
        {
            fault("a callee replied twice");
        }
        return STATUS_OK;
    }
    if tag == REPLY_FAILED {
        // SAFETY: the context is live.
        callee(ctx).chain = crate::isolate::caught(ctx).clone();
    }
    if callee(ctx).reply.replace((tag, bytes)).is_some() {
        fault("a callee replied twice");
    }
    STATUS_OK
}

#[cfg(test)]
mod permit_tests {
    use super::*;

    #[test]
    fn nested_runs_return_the_last_permit_before_entering_a_callee() {
        const CHILD: &str = "HIRSEL_K1_NESTED_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "runners::permit_tests::nested_runs_return_the_last_permit_before_entering_a_callee"])
                .env(CHILD, "1").output().unwrap();
            assert!(child.status.success(), "{child:?}");
            return;
        }
        crate::pool::set_compute_grant(1);
        // Three genuine run contexts exercise the same enter path used by the
        // runner driver. The caller, callee and nested callee own separate heaps
        // and executors; only the active run can hold the one process permit.
        unsafe {
            let mut contexts = Vec::new();
            for _ in 0..3 {
                let mut ctx = std::ptr::null_mut();
                assert_eq!(
                    crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
                    0
                );
                contexts.push(ctx);
            }
            let root = contexts[0];
            let root_run = (*root).run_ptr();
            let mut execution = Execution::new(Config::default(), root);
            for &ctx in &contexts[1..] {
                (*ctx).run().root = root_run;
                execution.runs.push(Run {
                    root: ctx,
                    resume: ctx,
                    sp: std::ptr::null_mut(),
                    ended: false,
                    state: Runners::default(),
                    callee: None,
                });
            }
            *(*root).run().runners = Some(Box::new(execution));
            crate::pool::enter_root(root).unwrap();
            let mut active = 0;
            for target in [1, 2, 1, 0, 2, 0] {
                enter(contexts[active], target);
                assert_eq!(crate::pool::compute_permits(), (1, 1, 0));
                active = target;
            }
            crate::pool::leave_root(root);
            assert_eq!(crate::pool::compute_permits(), (1, 0, 0));
            for &ctx in contexts.iter().rev() {
                TaskCtx::attach(ctx, (*ctx).worker_ptr());
                crate::context::hirsel_rt_v2_ctx_destroy(ctx);
            }
            crate::stop::set_running(std::ptr::null_mut());
        }
    }
}
