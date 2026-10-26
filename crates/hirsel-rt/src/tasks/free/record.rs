//! Native recorder admission at concrete runtime boundaries. The encoder is
//! task-local; only stable logs are shared with a cutter. Effects and their
//! records execute inside the same production recorder phase, before its
//! progress publication. No section or recorder borrow crosses a suspension.
//!
//! Ordinary `tasks.free@1` launches install these owners; explicit runtime
//! proofs also install them directly. All admitted
//! boundaries and stops share this protocol; the recording-off host path uses
//! the same request ownership without sections, ordinals or tape allocation.

use crate::TaskCtx;
use crate::stop::{Stop, Stopped};
use crate::tasks::role;
use hirsel_free::concurrent::{ExecutorSlot, Recorder, Writer};
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::projection::Stamped;
use hirsel_free::record::RecordBound;
use std::sync::{Arc, Mutex};

/// The bound of a phase without a payload: at most a few writer calls (a gap, a load, a
/// cancellation change, a receive). Host phases declare their key positions, payloads and
/// outputs explicitly with [`publish_bounded`].
pub(super) const PHASE: RecordBound = RecordBound::records(4);

/// A physical executor's registration in its run's recording, owned by its dispatcher `Exec`:
/// registered once when the executor starts serving the run and retired when it leaves, after
/// its section word is acknowledged inactive. Each phase on the executor checks the slot out
/// and returns it; a nested callback that finds it checked out, a caller on no executor of
/// this run and a thread serving another run's recording use a distinct transient slot.
/// Touched only by the executor's own thread.
#[derive(Default)]
pub(crate) struct Registration(std::cell::Cell<Option<ExecutorSlot>>);

impl Registration {
    /// Register a starting executor that prefers `preferred`. A compute or blocking executor
    /// may wait for a retiring one (`wait`); home never waits behind another executor.
    /// # Safety
    /// The executor's own thread, as it starts serving `run`, outside any role or section.
    pub(crate) unsafe fn register(
        &self,
        run: &crate::context::RunShared,
        preferred: usize,
        wait: bool,
    ) {
        if let Some(recording) = run.recording.get() {
            unsafe { self.register_with(recording, preferred, wait) };
        }
    }

    /// # Safety
    /// As for [`register`](Self::register), in `recording`'s run.
    pub(crate) unsafe fn register_with(&self, recording: &Recording, preferred: usize, wait: bool) {
        let recorder = &recording.recorder;
        if let Some(slot) = self.0.take() {
            // Registered once: a home owner that polls again keeps its slot.
            assert!(recorder.owns(&slot), "an executor serves one recording");
            self.0.set(Some(slot));
            return;
        }
        self.0.set(if wait {
            recorder.register(preferred)
        } else {
            recorder.executor_available(preferred)
        });
    }

    /// Retire the executor's registration as it leaves `run`.
    /// # Safety
    /// The executor's own thread, after its last phase for `run`.
    pub(crate) unsafe fn retire(&self, run: &crate::context::RunShared) {
        if let Some(recording) = run.recording.get() {
            unsafe { self.retire_from(recording) };
        }
    }

    /// # Safety
    /// As for [`retire`](Self::retire), in `recording`'s run.
    pub(crate) unsafe fn retire_from(&self, recording: &Recording) {
        if let Some(slot) = self.0.take() {
            recording.recorder.retire_executor(slot);
        }
    }

    /// The executor's own slot when it is registered with `recorder` and not checked out.
    fn take(&self, recorder: &Recorder<Sink, crate::io::Request>) -> Option<ExecutorSlot> {
        let slot = self.0.take()?;
        if recorder.owns(&slot) {
            return Some(slot);
        }
        self.0.set(Some(slot));
        None
    }

    #[cfg(test)]
    pub(crate) fn index(&self) -> Option<usize> {
        let slot = self.0.take();
        let index = slot.as_ref().map(ExecutorSlot::index);
        self.0.set(slot);
        index
    }
}

/// The first section word past every executor's preferred one: transient registrations
/// search from here.
const TRANSIENT: usize = crate::pool::MAX_WIDTH as usize + crate::pool::POOL_THREADS + 2;

/// A phase's executor slot: the running executor's own, or a transient one.
struct Checkout {
    slot: ExecutorSlot,
    own: bool,
}

/// The slot for one phase on this thread, outside any section. Registration never waits here:
/// a phase may hold the scheduler role. Capacity holds every live, retiring, home and blocking
/// executor beside the transient ones, so exhaustion is a runtime fault, not a wait.
fn checkout(recording: &Recording) -> Checkout {
    let recorder = &recording.recorder;
    if let Some(slot) = crate::pool::registration(|own| own.and_then(|own| own.take(recorder))) {
        return Checkout { slot, own: true };
    }
    #[cfg(test)]
    recording
        .transients
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let slot = recorder
        .executor_available(TRANSIENT)
        .unwrap_or_else(|| crate::fault("recording executor slots exhausted"));
    Checkout { slot, own: false }
}

/// Return a slot after its section closed: the executor keeps its own; a transient one
/// retires at once.
fn checkin(recording: &Recording, checkout: Checkout) {
    if checkout.own {
        crate::pool::registration(|own| {
            own.expect("the executor still runs")
                .0
                .set(Some(checkout.slot))
        });
    } else {
        recording.recorder.retire_executor(checkout.slot);
    }
}

/// Production recordings stream to a pinned file; explicit runtime tests keep
/// the same bytes and barrier offsets in memory. No production tape is retained
/// a second time in a growing Vec.
#[derive(Default)]
pub(crate) struct Sink {
    pub(crate) bytes: Vec<u8>,
    pub(crate) marks: Vec<usize>,
    file: Option<std::fs::File>,
    durable: bool,
}

impl hirsel_free::recorder::TapeSink for Sink {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        match &mut self.file {
            Some(file) => file.write_all(bytes),
            None => {
                self.bytes.extend_from_slice(bytes);
                Ok(())
            }
        }
    }

    fn barrier(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        match &mut self.file {
            Some(file) => {
                file.flush()?;
                if self.durable {
                    file.sync_all()?;
                }
            }
            None => self.marks.push(self.bytes.len()),
        }
        Ok(())
    }
}

/// The host stop path retains only this owner and the stable recorder logs.
/// It never needs the scheduler role, a saved stack, or the home adapter.
pub(crate) struct Terminal {
    recording: Arc<Recording>,
    receipt: Mutex<Option<hirsel_free::claim::Sweep>>,
}

impl Terminal {
    /// Admission only; the caller installs this on the control of the same run
    /// before publishing the run to workers or an embedding host.
    pub(crate) fn new(recording: Arc<Recording>) -> Arc<Self> {
        Arc::new(Self {
            recording,
            receipt: Mutex::new(None),
        })
    }
}

impl crate::stop::Terminal for Terminal {
    fn stop(&self, cause: &Stop) -> std::io::Result<()> {
        self.stop_at(cause, None, None)
    }
    fn stop_at(
        &self,
        cause: &Stop,
        task: Option<&[u32]>,
        observed: Option<&hirsel_run_schema::RunOutcome>,
    ) -> std::io::Result<()> {
        use crate::tape_wire::Wire;
        let mut bytes = Vec::new();
        cause.encode(&mut bytes);
        let outcome = format!("native-stop-v1/{}", hirsel_run_schema::hex(&bytes));
        let receipt = self.recording.recorder.end_with(
            |sweep| {
                if self.recording.production {
                    match observed {
                        Some(outcome) => {
                            assert_eq!(
                                *cause,
                                Stop::Failed,
                                "only internal failure supplies an outcome"
                            );
                            crate::free_launch::terminal_bytes("native-failure-v2", outcome, sweep)
                        }
                        None => crate::free_launch::terminal_bytes("native-stop-v2", cause, sweep),
                    }
                } else {
                    outcome
                }
            },
            task,
        )?;
        if let Some(receipt) = receipt {
            *self.receipt.lock().unwrap() = Some(receipt);
        }
        Ok(())
    }
}

/// Bind an admitted native recorder to the run's actual stop authority. F0's
/// cooperative test session has its own lifecycle and is deliberately excluded.
/// Call before starting workers. A stop requested before launch closes the
/// already registered START prefix at this admission boundary.
pub(crate) fn bind_terminal(ctx: &TaskCtx) {
    if let Some(task) = &ctx.recording {
        ctx.run_ref()
            .control
            .terminal(Terminal::new(task.shared.clone()));
    }
}

#[cfg(test)]
#[path = "record/terminal_tests.rs"]
mod terminal_tests;

pub(crate) struct Recording {
    pub(crate) recorder: Recorder<Sink, crate::io::Request>,
    trace: Mutex<Vec<Stamped>>,
    requests: std::sync::atomic::AtomicU64,
    pub(super) host_keys: crate::io_keys::KeyTable,
    pub(super) authorities: super::host::requests::Authorities,
    production: bool,
    /// Phases that ran on a transient slot rather than their executor's registration.
    #[cfg(test)]
    pub(crate) transients: std::sync::atomic::AtomicU64,
}

pub(crate) struct Task {
    shared: Arc<Recording>,
    log: hirsel_free::concurrent::Task,
    ordinal: u64,
    host_handles: u64,
}

pub(super) struct Child {
    shared: Arc<Recording>,
    log: hirsel_free::concurrent::PreparedChild,
    owner: Box<std::mem::MaybeUninit<Task>>,
}

pub(super) fn prepare_child(ctx: *mut TaskCtx) -> Option<Child> {
    unsafe { (*ctx).recording.as_ref() }.map(|task| Child {
        shared: task.shared.clone(),
        log: task.log.prepare_child(),
        owner: Box::new_uninit(),
    })
}

/// Register START and install its prepared runtime owner in the parent's
/// current spawn phase. The box and stable log were allocated before entry.
pub(super) fn spawn<T>(
    ctx: *mut TaskCtx,
    child: Option<Child>,
    action: impl FnOnce(Option<Box<Task>>) -> T,
) -> Stopped<T> {
    publish(ctx, Phase::Done, |writer| {
        let owner = match (child, writer) {
            (Some(mut child), Some(writer)) => {
                child.owner.write(Task {
                    shared: child.shared,
                    log: writer.spawn_prepared(child.log),
                    ordinal: 0,
                    host_handles: 0,
                });
                // SAFETY: the exclusively owned box was initialized above.
                Some(unsafe { child.owner.assume_init() })
            }
            (None, None) => None,
            _ => unreachable!("a prepared child retains its parent's admission"),
        };
        action(owner)
    })
}

impl Drop for Task {
    fn drop(&mut self) {
        if !self.log.trace().is_empty() {
            self.shared
                .trace
                .lock()
                .unwrap()
                .extend_from_slice(self.log.trace());
        }
    }
}

pub(crate) fn on(ctx: *mut TaskCtx) -> bool {
    // SAFETY: only the running owner reaches its task-local recorder.
    unsafe { (*ctx).recording.is_some() }
}

pub(super) fn owner(ctx: *mut TaskCtx) -> Option<Arc<Recording>> {
    unsafe { (*ctx).recording.as_ref() }.map(|task| task.shared.clone())
}

pub(super) fn position(ctx: *mut TaskCtx) -> (Vec<u32>, Progress) {
    let task = unsafe { (*ctx).recording.as_ref() }.expect("a recorded host request");
    (
        task.log.path().to_vec(),
        Progress::new(task.ordinal, Phase::Entered),
    )
}

impl Recording {
    pub(super) fn prepare_request(
        &self,
        request: crate::io::Request,
    ) -> Arc<hirsel_free::claim::Request<crate::io::Request>> {
        let id = self
            .requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_ne!(id, u64::MAX, "host request identity overflow");
        self.recorder.prepare(id, request)
    }
}

pub(super) fn reserve_host_handle(ctx: *mut TaskCtx) -> u64 {
    let task = unsafe { (*ctx).recording.as_mut() }.expect("recorded handle creation");
    task.host_handles = task
        .host_handles
        .checked_add(1)
        .expect("host handle ordinal overflow");
    task.host_handles
}

/// Transfer the encoder into task-owned request storage before a synchronous
/// home loan. The worker cannot publish another phase until it is restored.
/// # Safety
/// The running owner holds the role and retains the loan in task-owned storage.
pub(super) unsafe fn lend_host(ctx: *mut TaskCtx) -> Box<Task> {
    unsafe { (*ctx).recording.take() }.expect("a recorded home request")
}

/// # Safety
/// The synchronous home call finished or was withdrawn; no home borrower remains.
pub(super) unsafe fn restore_host(ctx: *mut TaskCtx, task: Box<Task>) {
    assert!(unsafe { (*ctx).recording.is_none() });
    unsafe { (*ctx).recording = Some(task) };
}

impl Task {
    /// Home owns this encoder exclusively while its posting worker waits.
    /// Claimed has no root tail, scheduler effect, or TaskCtx access.
    pub(super) fn claim_host(
        &mut self,
        request: &hirsel_free::claim::Request<crate::io::Request>,
    ) -> Stopped<bool> {
        let mut worker = checkout(&self.shared);
        let claimed = self
            .shared
            .recorder
            .phase(
                &mut worker.slot,
                &mut self.log,
                Progress::new(self.ordinal, Phase::Claimed),
                PHASE,
                |writer| self.shared.recorder.claim(writer, request),
            )
            .map_err(|_| Stop::Requested);
        checkin(&self.shared, worker);
        claimed
    }
}

/// The running task supplies bounded runtime work. Table actions hold the
/// scheduler role; resolved cell actions instead hold their independent access.
/// The local recorder is removed from ctx before action borrows the context,
/// and restored before any stop or physical suspension can leave this stack.
fn phase<T>(
    ctx: *mut TaskCtx,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    begin(ctx);
    publish(ctx, Phase::Done, action)
}

/// Begin a semantic operation once, including when it later parks/retries.
pub(super) fn begin(ctx: *mut TaskCtx) {
    // SAFETY: the running task owns its context.
    unsafe { (*ctx).point_ops = (*ctx).point_ops.saturating_add(1) };
    if super::replay::at_cut(ctx) {
        crate::tasks::park_final(ctx);
    }
    entry(ctx);
    super::replay::begin(ctx);
    if let Some(task) = unsafe { (*ctx).recording.as_mut() } {
        task.ordinal = task
            .ordinal
            .checked_add(1)
            .expect("native op ordinal overflow");
    }
}

/// [`begin`] for an immediate op whose task token checked its cut: replay
/// keeps no recorder ordinal, and entry has nothing pending to apply.
pub(super) fn begin_replayed(ctx: *mut TaskCtx, replay: &super::replay::Token) {
    // SAFETY: the running task owns its context.
    unsafe { (*ctx).point_ops = (*ctx).point_ops.saturating_add(1) };
    entry(ctx);
    replay.begin();
}

/// Publish another phase of the current ordinal; no section escapes action.
pub(super) fn publish<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    publish_bounded(ctx, phase, PHASE, action)
}

/// [`publish`] for a phase that may write more than [`PHASE`]: `bound` covers all of it.
pub(super) fn publish_bounded<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    bound: RecordBound,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    let replay = super::replay::token(ctx);
    publish_with_replay_bounded(ctx, phase, replay.as_ref(), bound, action)
}

/// Cell owners capture the replay token while holding the scheduler role,
/// then publish here without looking up the scheduler inside their access.
pub(super) fn publish_with_replay<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    replay: Option<&super::replay::Token>,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    publish_with_replay_bounded(ctx, phase, replay, PHASE, action)
}

fn publish_with_replay_bounded<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    replay: Option<&super::replay::Token>,
    bound: RecordBound,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    if !on(ctx) {
        if let Some(replay) = replay {
            return replay.publish(ctx, phase, || action(None));
        }
        // Live free mode has no recorder, section gate, ordinal, or encoder.
        return Ok(action(None));
    }
    if phase == Phase::Done && unsafe { (*ctx).roots.has_pending() } {
        // A nested phase tail may change the outcome this op observes. Finish
        // entry before acting, under the same ordinal, even for an immediate op.
        publish_phase(ctx, Phase::Entered, RecordBound::default(), |_| ())?;
    }
    publish_phase(ctx, phase, bound, action)
}

/// The recorder admits the phase only with room for all of `bound` reserved; a short
/// reservation is made up before the prefix, under the same progress.
fn publish_phase<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    bound: RecordBound,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    let mut roots = crate::roots::PhaseRoots::prepare(ctx);
    let mut task = unsafe { (*ctx).recording.take() }.expect("an admitted native recorder");
    let shared = task.shared.clone();
    let mut worker = checkout(&shared);
    let progress = Progress::new(task.ordinal, phase);
    let mut action = Some(action);
    let result = loop {
        let attempt = shared.recorder.phase_if(
            &mut worker.slot,
            &mut task.log,
            progress,
            bound,
            || roots.apply(ctx),
            |writer| action.take().expect("one admitted action")(Some(writer)),
        );
        match attempt {
            Ok(Some(result)) => break Ok(result),
            Err(stop) => break Err(stop),
            Ok(None) => crate::roots::wait_phase(ctx),
        }
    };
    checkin(&shared, worker);
    if result.is_ok() {
        // The entry is now published and no section/writer survives. A cut
        // admitting it also admits its full deterministic frozen-graph tail.
        roots.finish(ctx);
    }
    unsafe { (*ctx).recording = Some(task) };
    result.map_err(|_| Stop::Requested)
}

/// Begin a certified entered operation. A replay cut rejects before flushing a
/// deferred child or advancing the ordinal; the invocation retains Begin.
pub(super) fn begin_entered(ctx: *mut TaskCtx) -> bool {
    if super::replay::at_cut(ctx) {
        return false;
    }
    // Entry's deferred-child flush cannot leave this native callchain. Live
    // frozen-root traversal likewise has no language callbacks or suspension.
    unsafe { (*ctx).point_ops = (*ctx).point_ops.saturating_add(1) };
    entry(ctx);
    super::replay::begin(ctx);
    if let Some(task) = unsafe { (*ctx).recording.as_mut() } {
        task.ordinal = task
            .ordinal
            .checked_add(1)
            .expect("native op ordinal overflow");
    }
    true
}

/// One admission attempt for a task-owned phase. None has no prefix effects:
/// the same typed phase can recreate its bounded action on another worker.
/// No FnOnce, executor checkout, section or recorder borrow survives return.
pub(super) fn poll_publish<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<Option<T>> {
    let replay = super::replay::token(ctx);
    poll_publish_with_replay(ctx, phase, replay.as_ref(), action)
}

/// Use the owner-local token captured before taking a cell lock.
pub(super) fn poll_publish_with_replay<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    replay: Option<&super::replay::Token>,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<Option<T>> {
    if !on(ctx) {
        return match replay {
            Some(replay) => replay.poll_publish(ctx, phase, || action(None)),
            None => Ok(Some(action(None))),
        };
    }
    if phase == Phase::Done
        && unsafe { (*ctx).roots.has_pending() }
        && poll_phase(ctx, Phase::Entered, |_| ())?.is_none()
    {
        return Ok(None);
    }
    poll_phase(ctx, phase, action)
}

/// The action is shared with the synchronous adapter; only admission may
/// return Pending. No role lookup is performed while a cell lock is held.
pub(super) fn publish_entered<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    replay: Option<&super::replay::Token>,
    polling: bool,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<Option<T>> {
    if polling {
        poll_publish_with_replay(ctx, phase, replay, action)
    } else {
        publish_with_replay(ctx, phase, replay, action).map(Some)
    }
}

fn poll_phase<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<Option<T>> {
    // Preparation reserves storage only. A rejected prefix has not taken or
    // decremented pending roots; a successful prefix and action publish once.
    let mut roots = crate::roots::PhaseRoots::prepare(ctx);
    let mut task = unsafe { (*ctx).recording.take() }.expect("an admitted native recorder");
    let shared = task.shared.clone();
    let mut worker = checkout(&shared);
    let progress = Progress::new(task.ordinal, phase);
    let result = shared.recorder.try_phase_if(
        &mut worker.slot,
        &mut task.log,
        progress,
        PHASE,
        || roots.apply(ctx),
        |writer| action(Some(writer)),
    );
    checkin(&shared, worker);
    if matches!(result, Ok(Some(_))) {
        // Frozen graph tails cannot run language code or suspend. Finish the
        // admitted tail synchronously; never repeat apply or finish on rebind.
        roots.finish(ctx);
    }
    unsafe { (*ctx).recording = Some(task) };
    result.map_err(|_| Stop::Requested)
}

/// Track an object the next phase names (touches, creates or receives from), before its
/// section: the section then inserts no entry.
pub(super) fn track(ctx: *mut TaskCtx, object: Option<&hirsel_free::record::ObjectId>) {
    if let (Some(task), Some(object)) = (unsafe { (*ctx).recording.as_mut() }, object) {
        task.log.track(object);
    }
}

/// Track the host keys the next phase takes positions on, before its section.
pub(super) fn track_keys(ctx: *mut TaskCtx, keys: impl IntoIterator<Item = u32>) {
    if let Some(task) = unsafe { (*ctx).recording.as_mut() } {
        for key in keys {
            task.log.track_key(key);
        }
    }
}

pub(super) fn object(ctx: *mut TaskCtx) -> Option<hirsel_free::record::ObjectId> {
    unsafe { (*ctx).recording.as_ref() }.map(|task| hirsel_free::record::ObjectId::Made {
        path: task.log.path().to_vec(),
        op: task.ordinal,
    })
}

pub(crate) fn end<T>(ctx: *mut TaskCtx, action: impl FnOnce() -> T) -> Stopped<T> {
    begin(ctx);
    publish(ctx, Phase::Entered, |_| ())?;
    publish(ctx, Phase::Done, |_| action())
}

/// A root's public outcome is deterministic engine data, checked before its
/// final phase. Keep the bytes task-owned across deterministic release tails.
pub(crate) fn end_observed(ctx: *mut TaskCtx, bytes: Vec<u8>) -> Stopped<()> {
    assert!(unsafe { (*ctx).free_terminal.is_none() });
    let bound = PHASE.payload(bytes.len());
    unsafe { (*ctx).free_terminal = Some(bytes) };
    begin(ctx);
    let token = super::replay::token(ctx);
    publish_with_replay(ctx, Phase::Entered, token.as_ref(), |_| ())?;
    publish_with_replay_bounded(ctx, Phase::Done, token.as_ref(), bound, |writer| {
        let bytes = unsafe { (*ctx).free_terminal.as_ref().unwrap() };
        if let Some(writer) = writer {
            writer.observation(bytes);
        } else if let Some(token) = &token {
            token.observation(bytes)?;
        }
        Ok(())
    })??;
    unsafe { (*ctx).free_terminal.take() };
    Ok(())
}

fn entry(ctx: *mut TaskCtx) {
    // A failed path or any operation other than the promised adjacent join
    // publishes the child normally before performing its own effect.
    unsafe { super::spawn::flush(ctx) };
    if !on(ctx) && !super::replay::on(ctx) {
        // Live entry releases happen under the role, before obtaining a cell
        // or relinquishing scheduler ownership. Recorded roots instead apply
        // in the admitted prefix, with their deterministic tail afterward.
        crate::roots::apply_pending(ctx);
    }
}

/// # Safety
/// The role is held; root is a live borrowed Shared root and out writable.
pub(crate) unsafe fn thaw(
    ctx: *mut TaskCtx,
    root: *mut crate::Header,
    out: *mut u8,
) -> Stopped<()> {
    begin(ctx);
    if super::replay::on(ctx) {
        return unsafe { super::replay::thaw(ctx, root, out) };
    }
    publish(ctx, Phase::Done, |writer| unsafe {
        let last = crate::roots::sole(root);
        if let Some(writer) = writer {
            writer.thaw(last);
        }
        out.write(u8::from(last));
    })?;
    unsafe { super::try_yield(&mut *ctx) }
}

/// # Safety
/// This is a supported atomic runtime entry, holding the scheduler role;
/// out and value satisfy AtomicCreate's carrier contract.
pub(crate) unsafe fn atomic_create(
    ctx: *mut TaskCtx,
    out: *mut u64,
    value: *const u8,
    size: u64,
    payload_offset: u64,
    payload_size: u64,
) -> Stopped<()> {
    entry(ctx);
    // The image and its table entry are allocated here; the section only
    // commits them. Its RAII owner must be dropped normally before any stop
    // or suspension, so the borrow inside it observes no stop.
    let (image, reservation) = role::with_role(ctx, |s| unsafe {
        let image = s
            .sync
            .prepare_image(value, size, payload_offset, payload_size);
        (image, s.sync.reserve_image())
    });
    phase(ctx, |_| unsafe {
        let word = reservation.word();
        role::with_held(ctx, |s| s.sync.initialize_image_cell(reservation, image));
        out.write(word);
    })
}

/// # Safety
/// The role is held; word is a live atomic and slot writable for size bytes.
pub(crate) unsafe fn atomic_load(
    ctx: *mut TaskCtx,
    word: u64,
    slot: *mut u8,
    size: u64,
) -> Stopped<()> {
    begin(ctx);
    let replay = super::replay::token(ctx);
    let image = role::with_role(ctx, |s| s.sync.image(word, size));
    super::objects::keep_image(ctx, word, &image);
    unsafe { crate::tasks::role::release(ctx) };
    let result = unsafe { load_image(ctx, &image, slot, size, replay.as_ref()) };
    drop(image);
    unsafe { crate::tasks::role::enter(ctx) };
    result
}

/// A begun load's action and record, holding no scheduler borrow.
/// # Safety
/// As for atomic_load; image is the word's resolved cell.
pub(super) unsafe fn load_image(
    ctx: *mut TaskCtx,
    image: &crate::sync::AtomicImage,
    slot: *mut u8,
    size: u64,
    replay: Option<&super::replay::Token>,
) -> Stopped<()> {
    let size_bytes = usize::try_from(size).expect("an atomic image fits memory");
    // The wide-image stripe is acquired outside the section. Retain it until
    // progress is published, so its next accessor cannot observe an action
    // that the cutter cannot yet see.
    let access = image.access();
    let result = publish_with_replay(ctx, Phase::Done, replay, |writer| unsafe {
        let out = std::slice::from_raw_parts_mut(slot, size_bytes);
        access.load(out);
        if writer.is_some() || replay.is_some() {
            let bits = image.bits(out).ok_or_else(|| {
                Stop::Diverged("a recorded atomic scalar exceeds 128 bits".into())
            })?;
            if let Some(writer) = writer {
                writer.load(bits);
            } else {
                let bits = replay.unwrap().load(bits)?;
                if !image.substitute(out, bits) {
                    return Err(Stop::Diverged(
                        "a recorded atomic load does not fit its scalar".into(),
                    ));
                }
            }
        }
        Ok(())
    })
    .and_then(|result| result);
    drop(access);
    result
}

/// # Safety
/// The role is held; word is live and value readable for size bytes.
pub(crate) unsafe fn atomic_store(
    ctx: *mut TaskCtx,
    word: u64,
    value: *const u8,
    size: u64,
) -> Stopped<()> {
    begin(ctx);
    let replay = super::replay::token(ctx);
    let image = role::with_role(ctx, |s| s.sync.image(word, size));
    super::objects::keep_image(ctx, word, &image);
    unsafe { crate::tasks::role::release(ctx) };
    let result = unsafe { store_image(ctx, &image, value, replay.as_ref()) };
    drop(image);
    unsafe { crate::tasks::role::enter(ctx) };
    result
}

/// A begun store's action, holding no scheduler borrow.
/// # Safety
/// As for atomic_store; image is the word's resolved cell.
pub(super) unsafe fn store_image(
    ctx: *mut TaskCtx,
    image: &crate::sync::AtomicImage,
    value: *const u8,
    replay: Option<&super::replay::Token>,
) -> Stopped<()> {
    let value = unsafe { std::slice::from_raw_parts(value, image.size()) };
    let access = image.access();
    let result = publish_with_replay(ctx, Phase::Done, replay, |_| access.store(value));
    drop(access);
    result
}

impl Recording {
    /// The caller pinned and checked this destination against every granted
    /// mount and output. Header bytes bind the complete admission evidence.
    pub(crate) fn open(
        file: std::fs::File,
        binding: &[u8],
        durable: bool,
    ) -> std::io::Result<Arc<Self>> {
        Self::with_sink(
            Sink {
                file: Some(file),
                durable,
                ..Default::default()
            },
            binding,
            durable,
            false,
            None,
        )
    }

    fn with_sink(
        sink: Sink,
        binding: &[u8],
        durable: bool,
        trace: bool,
        metrics: Option<Arc<hirsel_free::concurrent::Metrics>>,
    ) -> std::io::Result<Arc<Self>> {
        let production = sink.file.is_some();
        let observer = (production && std::env::var_os(crate::free_replay::Report::VAR).is_some())
            .then(|| Arc::new(hirsel_free::concurrent::Observer::default()));
        let mut recorder = Recorder::new(
            sink,
            binding,
            // Home, a full compute topology and the blocking pool register for their lifetime;
            // the second half holds retiring executors not yet exited and transient callers.
            2 * (crate::pool::MAX_WIDTH as usize + crate::pool::POOL_THREADS + 2),
            durable,
            trace,
            metrics,
        )?;
        if let Some(observer) = &observer {
            recorder.observe(observer.clone());
        }
        Ok(Arc::new(Self {
            production,
            recorder,
            trace: Mutex::new(Vec::new()),
            requests: std::sync::atomic::AtomicU64::new(1),
            host_keys: Default::default(),
            authorities: Default::default(),
            #[cfg(test)]
            transients: Default::default(),
        }))
    }

    pub(crate) fn projection(&self) -> Option<hirsel_free::projection::Projection> {
        self.recorder.projection()
    }

    /// A traced in-memory recording whose sections arm the test allocator's trap.
    #[cfg(test)]
    pub(crate) fn new(binding: &[u8], durable: bool) -> Arc<Self> {
        let trap = hirsel_free::concurrent::Metrics::with_section_hook(
            crate::io_real::borrowed_net_tests::section_hook,
        );
        Self::with_sink(
            Sink::default(),
            binding,
            durable,
            true,
            Some(Arc::new(trap)),
        )
        .unwrap()
    }

    /// # Safety
    /// Admission only, before any task runs; no F0 session is installed.
    pub(crate) unsafe fn root(self: &Arc<Self>, ctx: *mut TaskCtx) {
        assert!(unsafe { (*ctx).recording.is_none() && (*ctx).run_ref().free.is_none() });
        if unsafe { (*ctx).run_ref().free_production } {
            self.authorities
                .root(unsafe { (*ctx).run_ref().free_root_env });
        }
        // Admission is the home thread before any executor starts: it registers home's word
        // for the root's START, then retires it before home registers for the run.
        assert!(
            unsafe { (*ctx).run_ref() }
                .recording
                .set(self.clone())
                .is_ok(),
            "a run has one recording"
        );
        let mut worker = self
            .recorder
            .executor(hirsel_free::concurrent::HOME)
            .unwrap();
        let log = self.recorder.root(&mut worker).unwrap();
        self.recorder.retire_executor(worker);
        unsafe {
            (*ctx).recording = Some(Box::new(Task {
                shared: self.clone(),
                log,
                ordinal: 0,
                host_handles: 0,
            }))
        };
    }

    /// The run's canonical trace. No recorder section in this test process has allocated
    /// or freed so far.
    #[cfg(test)]
    pub(crate) fn trace(&self) -> Vec<Stamped> {
        let (sections, trapped) = crate::io_real::borrowed_net_tests::section_traps();
        assert!(sections > 0, "the section trap was armed");
        assert_eq!(trapped, 0, "a Free recorder section allocated or freed");
        self.trace.lock().unwrap().clone()
    }
}

impl Task {
    pub(crate) fn path(&self) -> &[u32] {
        self.log.path()
    }
}

/// A prepared child's registration is a phase of its spawning parent. The
/// test transfers this owner into a fresh TaskCtx before scheduling its image.
#[cfg(test)]
pub(crate) fn child(ctx: *mut TaskCtx) -> Stopped<Task> {
    let shared = unsafe { (*ctx).recording.as_ref().unwrap().shared.clone() };
    let prepared = unsafe { (*ctx).recording.as_ref().unwrap().log.prepare_child() };
    let log = phase(ctx, |writer| writer.unwrap().spawn_prepared(prepared))?;
    Ok(Task {
        shared,
        log,
        ordinal: 0,
        host_handles: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::*;
    use hirsel_free::projection::Projection;
    use hirsel_free::session::Session;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    const OPS: u64 = 32;

    #[test]
    fn live_free_atomics_use_stable_cells_without_a_recorder() {
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { crate::tasks::free::enable(ctx) },
            |ctx| {
                assert!(!on(ctx));
                for size in [1, 2, 4, 8, 16, 32] {
                    let value: Vec<u8> = (0..size as u8).map(|i| i.wrapping_mul(7)).collect();
                    let mut word = 0;
                    let mut out = vec![0; size];
                    unsafe {
                        hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut word,
                            out.as_ptr(),
                            size as u64,
                            0,
                            size as u64,
                        );
                        hirsel_rt_v2_atomic_store(ctx, &word, value.as_ptr(), size as u64);
                        hirsel_rt_v2_atomic_load(ctx, &word, out.as_mut_ptr(), size as u64);
                    }
                    assert_eq!(out, value);
                }
                assert!(!on(ctx));
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    struct Race {
        word: u64,
        started: AtomicU32,
        finished: AtomicU32,
        go: AtomicBool,
        middle: AtomicU32,
        resume: AtomicBool,
        timed_out: AtomicBool,
    }

    fn exercise(ctx: *mut TaskCtx, race: &Race) {
        race.started.fetch_add(1, Ordering::AcqRel);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !race.go.load(Ordering::Acquire) {
            if std::time::Instant::now() >= until {
                race.timed_out.store(true, Ordering::Release);
                return;
            }
            std::thread::yield_now();
        }
        for value in 1..=OPS {
            let mut loaded = 0;
            unsafe {
                assert_eq!(
                    hirsel_rt_v2_atomic_store(ctx, &race.word, (&raw const value).cast(), 8),
                    STATUS_OK
                );
                assert_eq!(
                    hirsel_rt_v2_atomic_load(ctx, &race.word, (&raw mut loaded).cast(), 8),
                    STATUS_OK
                );
            }
            assert!((1..=OPS).contains(&loaded));
            if value == OPS / 2 {
                race.middle.fetch_add(1, Ordering::AcqRel);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !race.resume.load(Ordering::Acquire) {
                    if std::time::Instant::now() >= until {
                        race.timed_out.store(true, Ordering::Release);
                        return;
                    }
                    std::thread::yield_now();
                }
            }
        }
        race.finished.fetch_add(1, Ordering::Release);
    }

    unsafe extern "C" fn segment(ctx: *mut u8, data: *mut u8) -> ! {
        let ctx = ctx.cast::<TaskCtx>();
        let race = unsafe { Box::from_raw(data.cast::<Arc<Race>>()) };
        exercise(ctx, &race);
        drop(race);
        unsafe {
            role::enter(ctx);
            crate::pool::free::finish_current(ctx);
        }
    }

    #[test]
    fn native_atomic_boundaries_record_eight_started_stacks_during_cuts() {
        const CHILD: &str = "HIRSEL_F1_NATIVE_RECORDING";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tasks::free::record::tests::native_atomic_boundaries_record_eight_started_stacks_during_cuts", "--nocapture"])
                .env(CHILD, "1")
                .output().unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }
        crate::pool::set_compute_grant(8);
        for durable in [false, true] {
            wide(durable);
        }
    }

    fn wide(durable: bool) {
        let shared = Recording::new(b"native atomic phases", durable);
        let install = shared.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let (started, ready) = std::sync::mpsc::sync_channel::<Arc<Race>>(1);
        let cutter = std::thread::spawn({
            let shared = shared.clone();
            move || {
                let race = ready.recv().unwrap();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while race.started.load(Ordering::Acquire) != 8 {
                    assert!(
                        std::time::Instant::now() < until,
                        "all eight stacks must start"
                    );
                    std::thread::yield_now();
                }
                // A cut while all eight actual stacks are inside segments:
                // every registered child's START must be visible, yet no
                // atomic load has acted. A replay starts none of them here.
                let first = shared.recorder.flush().unwrap().unwrap();
                assert_eq!(first.progress.len(), 8);
                race.go.store(true, Ordering::Release);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while race.middle.load(Ordering::Acquire) != 8 {
                    assert!(
                        std::time::Instant::now() < until,
                        "all eight stacks must publish their first half"
                    );
                    std::thread::yield_now();
                }
                let middle = shared.recorder.flush().unwrap().unwrap();
                for (path, progress) in middle.progress {
                    assert_eq!(progress.op, OPS + if path.is_empty() { 8 } else { 0 });
                }
                race.resume.store(true, Ordering::Release);
                for _ in 0..12 {
                    shared.recorder.flush().unwrap().unwrap();
                }
                race
            }
        });
        let outcome = run.run(
            move |ctx| unsafe {
                crate::tasks::free::enable(ctx);
                install.root(ctx);
            },
            move |ctx| {
                let mut word = 0;
                let initial = 0u64;
                unsafe {
                    hirsel_rt_v2_atomic_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut word,
                        (&raw const initial).cast(),
                        8,
                        0,
                        8,
                    )
                };
                let race = Arc::new(Race {
                    word,
                    started: AtomicU32::new(0),
                    finished: AtomicU32::new(0),
                    go: AtomicBool::new(false),
                    middle: AtomicU32::new(0),
                    resume: AtomicBool::new(false),
                    timed_out: AtomicBool::new(false),
                });
                unsafe {
                    role::enter(ctx);
                    for _ in 0..7 {
                        let stack = Stack::new();
                        let ctx_child = stack.context();
                        ctx_child.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                        let recording = child(ctx).unwrap();
                        let path = recording.path().to_vec();
                        (*ctx_child).recording = Some(Box::new(recording));
                        let s = scheduler(ctx);
                        let id = s.tasks.len() as u32;
                        (*ctx_child).task_id = id;
                        (*ctx_child).task_sp = stack.prepare(
                            segment,
                            ctx_child.cast(),
                            Box::into_raw(Box::new(race.clone())).cast(),
                        );
                        let mut row = crate::tasks::Task::new(1, ctx_child, None, path, None);
                        row.stack = Some(stack);
                        s.tasks.push(Some(row));
                        crate::pool::free::schedule_saved(
                            (*ctx).run_ptr(),
                            crate::tasks::scheduler(ctx).key(id),
                            Owner::Any,
                        );
                    }
                    role::release(ctx);
                }
                started.send(race.clone()).unwrap();
                exercise(ctx, &race);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while race.finished.load(Ordering::Acquire) != 8
                    && !race.timed_out.load(Ordering::Acquire)
                {
                    if std::time::Instant::now() >= until {
                        race.timed_out.store(true, Ordering::Release);
                        break;
                    }
                    std::thread::yield_now();
                }
                STATUS_OK
            },
        );
        let race = cutter.join().unwrap();
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert!(!race.timed_out.load(Ordering::Acquire));
        assert_eq!(race.finished.load(Ordering::Acquire), 8);
        let cut = shared.recorder.stop("returned", None).unwrap().unwrap().0;
        let trace = shared.trace();
        let live = Projection::new("returned", &cut.progress, &trace);
        assert_eq!(live.observed.len(), 8 * OPS as usize);
        let marked = Arc::try_unwrap(shared).ok().unwrap().recorder.into_sink();
        // The same backend-neutral session drives the interpreter replay.
        // Replay every completed flush and terminal prefix, not just the end.
        for end in marked.marks {
            let bytes = &marked.bytes[..end];
            if hirsel_free::tape::parse(bytes).unwrap().cut.is_empty() {
                continue; // a durable header barrier has no admitted phases
            }
            let mut replay = Session::replay(bytes).unwrap();
            assert!(replay.start(&[]));
            let terminal = end == marked.bytes.len();
            let mut paths = vec![Vec::new()];
            paths.extend((0..7).map(|id| vec![id]));
            for path in paths {
                if !path.is_empty() && !replay.start(&path) {
                    continue;
                }
                let spawn_ops = if path.is_empty() { 8 } else { 0 };
                for op in 1..=spawn_ops + 2 * OPS {
                    replay.begin(&path);
                    if !replay.admits(&path, Phase::Done) {
                        break;
                    }
                    if op > spawn_ops && (op - spawn_ops) % 2 == 0 {
                        replay.load(&path, 0).unwrap();
                    }
                    replay.publish(&path, Phase::Done).unwrap();
                }
            }
            let projection = replay.finish().unwrap();
            if terminal {
                assert_eq!(projection, live);
            } else {
                assert_eq!(
                    projection,
                    Projection::new("truncated", &projection.cut, &trace)
                );
            }
        }
    }

    #[test]
    fn a_native_atomic_action_cannot_pass_a_terminal_cut() {
        let shared = Recording::new(b"native stop", false);
        let install = shared.clone();
        let (ready, wait_ready) = std::sync::mpsc::sync_channel(1);
        let (stopped, wait_stopped) = std::sync::mpsc::sync_channel(1);
        let cutter = std::thread::spawn({
            let shared = shared.clone();
            move || {
                wait_ready.recv().unwrap();
                let cut = shared.recorder.stop("stopped", None).unwrap().unwrap().0;
                stopped.send(()).unwrap();
                cut
            }
        });
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let acted = Arc::new(AtomicBool::new(false));
        let observed = acted.clone();
        let outcome = run.run(
            move |ctx| unsafe {
                crate::tasks::free::enable(ctx);
                install.root(ctx);
            },
            move |ctx| {
                let mut word = 0;
                let value = 7u64;
                unsafe {
                    hirsel_rt_v2_atomic_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut word,
                        (&raw const value).cast(),
                        8,
                        0,
                        8,
                    );
                }
                ready.send(()).unwrap();
                wait_stopped.recv().unwrap();
                // Call the boundary helper so the test can inspect rejection
                // before raising the stop; this is the same helper called by
                // AtomicStore's ABI entry. No effect or progress may escape.
                unsafe {
                    let _role = role::Guard::new(ctx);
                    let replacement = 9u64;
                    assert_eq!(
                        atomic_store(ctx, word, (&raw const replacement).cast(), 8),
                        Err(Stop::Requested)
                    );
                    let mut after = 0u64;
                    scheduler(ctx).sync.read(word, (&raw mut after).cast(), 8);
                    assert_eq!(after, 7);
                }
                acted.store(true, Ordering::Release);
                STATUS_OK
            },
        );
        let cut = cutter.join().unwrap();
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert!(observed.load(Ordering::Acquire));
        assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Done));
        let live = Projection::new("stopped", &cut.progress, &shared.trace());
        let bytes = Arc::try_unwrap(shared)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let mut replay = Session::replay(&bytes).unwrap();
        assert!(replay.start(&[]));
        replay.begin(&[]);
        replay.publish(&[], Phase::Done).unwrap();
        assert_eq!(replay.finish().unwrap(), live);
    }
}

#[cfg(test)]
mod root_tests;

#[cfg(test)]
#[path = "record/executor_tests.rs"]
mod executor_tests;
