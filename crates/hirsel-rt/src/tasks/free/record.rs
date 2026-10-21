//! Native recorder admission at concrete runtime boundaries. The encoder is
//! task-local; only stable logs are shared with a cutter. Effects and their
//! records execute inside the same production recorder phase, before its
//! progress publication. No section or recorder borrow crosses a suspension.
//!
//! Admission is still limited to explicit runtime tests: atomic, mutex and
//! channel/task boundaries, root entry tails, thaw, mutex closes and foreign
//! callbacks on compute, home and pool threads, without host requests.
//! General activation requires every boundary and stop to use this protocol.

use crate::TaskCtx;
use crate::stop::{Stop, Stopped};
use hirsel_free::concurrent::{Recorder, Writer};
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::projection::Stamped;
use hirsel_free::session::Marked;
use std::sync::{Arc, Mutex};

pub(crate) struct Recording {
    pub(crate) recorder: Recorder<Marked>,
    trace: Mutex<Vec<Stamped>>,
}

pub(crate) struct Task {
    shared: Arc<Recording>,
    log: hirsel_free::concurrent::Task,
    ordinal: u64,
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

/// Publish another phase of the current ordinal; no section escapes action.
pub(super) fn publish<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    let replay = super::replay::token(ctx);
    publish_with_replay(ctx, phase, replay.as_ref(), action)
}

fn publish_with_replay<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    replay: Option<&super::replay::Token>,
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
        publish_phase(ctx, Phase::Entered, |_| ())?;
    }
    publish_phase(ctx, phase, action)
}

fn publish_phase<T>(
    ctx: *mut TaskCtx,
    phase: Phase,
    action: impl FnOnce(Option<&mut Writer<'_, '_>>) -> T,
) -> Stopped<T> {
    let mut roots = crate::roots::PhaseRoots::prepare(ctx);
    let mut task = unsafe { (*ctx).recording.take() }.expect("an admitted native recorder");
    let shared = task.shared.clone();
    let mut worker = loop {
        if let Some(worker) = shared
            .recorder
            .worker_available(crate::pool::recorder_slot())
        {
            break worker;
        }
        // No section is open. Retiring execs may temporarily outnumber the
        // active topology; another bounded section will release a token.
        std::thread::yield_now();
    };
    let progress = Progress::new(task.ordinal, phase);
    let mut action = Some(action);
    let result = loop {
        let attempt = shared.recorder.phase_if(
            &mut worker,
            &mut task.log,
            progress,
            || roots.apply(ctx),
            |writer| action.take().expect("one admitted action")(Some(writer)),
        );
        match attempt {
            Ok(Some(result)) => break Ok(result),
            Err(stop) => break Err(stop),
            Ok(None) => crate::roots::wait_phase(ctx),
        }
    };
    drop(worker);
    if result.is_ok() {
        // The entry is now published and no section/writer survives. A cut
        // admitting it also admits its full deterministic frozen-graph tail.
        roots.finish(ctx);
    }
    unsafe { (*ctx).recording = Some(task) };
    result.map_err(|_| Stop::Requested)
}

pub(super) fn object(ctx: *mut TaskCtx) -> Option<hirsel_free::record::ObjectId> {
    unsafe { (*ctx).recording.as_ref() }.map(|task| hirsel_free::record::ObjectId::Made {
        path: task.log.path().to_vec(),
        op: task.ordinal,
    })
}

pub(super) fn end<T>(ctx: *mut TaskCtx, action: impl FnOnce() -> T) -> Stopped<T> {
    begin(ctx);
    publish(ctx, Phase::Entered, |_| ())?;
    publish(ctx, Phase::Done, |_| action())
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
) -> Stopped<()> {
    entry(ctx);
    // scheduler() observes a runtime stop and can abandon its calling fiber.
    // Resolve it before opening the section, whose RAII owner must be dropped
    // normally before any stop/suspension. The role keeps this pointer stable.
    let scheduler = crate::tasks::scheduler(ctx) as *mut crate::tasks::Scheduler;
    phase(ctx, |_| unsafe {
        out.write((*scheduler).sync.create_image(value, size));
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
    let size_bytes = usize::try_from(size).expect("an atomic image fits memory");
    if on(ctx) || super::replay::on(ctx) {
        assert!(size_bytes <= 16, "a recorded atomic image fits 128 bits");
    }
    let image = crate::tasks::scheduler(ctx).sync.image(word, size);
    unsafe { crate::tasks::role::release(ctx) };
    // The wide-image stripe is acquired outside the section. Retain it until
    // progress is published, so its next accessor cannot observe an action
    // that the cutter cannot yet see. No scheduler borrow remains here.
    let access = image.access();
    let result = publish_with_replay(ctx, Phase::Done, replay.as_ref(), |writer| unsafe {
        let out = std::slice::from_raw_parts_mut(slot, size_bytes);
        access.load(out);
        if writer.is_some() || replay.is_some() {
            let mut bits = [0; 16];
            bits[..size_bytes].copy_from_slice(out);
            if let Some(writer) = writer {
                writer.load(u128::from_le_bytes(bits));
            } else {
                let bits = replay
                    .as_ref()
                    .unwrap()
                    .load(u128::from_le_bytes(bits))?
                    .to_le_bytes();
                out.copy_from_slice(&bits[..size_bytes]);
            }
        }
        Ok(())
    })
    .and_then(|result| result);
    drop(access);
    drop(image);
    unsafe { crate::tasks::role::enter(ctx) };
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
    let image = crate::tasks::scheduler(ctx).sync.image(word, size);
    let value = unsafe { std::slice::from_raw_parts(value, image.size()) };
    unsafe { crate::tasks::role::release(ctx) };
    let access = image.access();
    let result = publish_with_replay(ctx, Phase::Done, replay.as_ref(), |_| access.store(value));
    drop(access);
    drop(image);
    unsafe { crate::tasks::role::enter(ctx) };
    result
}

#[cfg(test)]
impl Recording {
    pub(crate) fn new(binding: &[u8], durable: bool) -> Arc<Self> {
        Arc::new(Self {
            recorder: Recorder::new(
                Marked::default(),
                binding,
                crate::pool::MAX_WIDTH as usize + crate::pool::POOL_THREADS + 1,
                durable,
                true,
                None,
            )
            .unwrap(),
            trace: Mutex::new(Vec::new()),
        })
    }

    /// # Safety
    /// Admission only, before any task runs; no F0 session is installed.
    pub(crate) unsafe fn root(self: &Arc<Self>, ctx: *mut TaskCtx) {
        assert!(unsafe { (*ctx).recording.is_none() && (*ctx).run_ref().free.is_none() });
        let mut worker = self.recorder.worker(0).unwrap();
        let log = self.recorder.root(&mut worker).unwrap();
        unsafe {
            (*ctx).recording = Some(Box::new(Task {
                shared: self.clone(),
                log,
                ordinal: 0,
            }))
        };
    }

    pub(crate) fn trace(&self) -> Vec<Stamped> {
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
                        crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
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
