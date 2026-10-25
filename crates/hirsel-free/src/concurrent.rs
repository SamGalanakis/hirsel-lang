//! Concurrent recording of free phases. Unlike the cooperative [`crate::recorder`],
//! the section opens *before* the action; each task owns its encoder and gap history.
//! A worker can migrate that task without moving a published log segment. The cutter
//! reads only logs/progress, never a task's mutable encoder or its execution stack.
//!
//! The section handshake, fixed log segments, claim/withdraw and framing are the F0
//! protocol, reused here. Engine code passed to `phase` is bounded runtime work: no
//! syscall, wait, foreign call or user code. Per-object locks must be acquired before
//! opening the section, or tried without blocking inside it. No recorder-global lock
//! serialises phase actions. Recording off uses neither this type nor a section gate.

use crate::claim::{Arbiter, Request, Requests, Sweep};
use crate::phase::{Phase, Progress};
use crate::projection::{Event, Observed, Stamped};
use crate::record::{CheckIndex, Encoder, ObjectId, Record, RecordBound};
use crate::recorder::TapeSink;
use crate::section::{Scanned, Section, Stopped, TaskLog};
use crate::tape::{self, End, TaskChunk};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::{Arc, Mutex};

/// A task's owner-local recorder. Move it with the task, never share it between
/// running workers. The shared gate owns only a reference to its stable log.
pub struct Task {
    identity: Arc<()>,
    log: Arc<TaskLog>,
    encoder: Encoder,
    last: HashMap<ObjectId, u64>,
    epochs: HashMap<u32, u64>,
    canceled: Option<u32>,
    cancel_checks: CheckIndex,
    /// Canonical test trace, absent in ordinary recording.
    trace: Option<Vec<Stamped>>,
    spawned: u32,
    observer: Option<Arc<Observer>>,
}

/// An allocated child log that has not joined the recorder registry. A cut
/// before its parent's spawn section must not publish this child's START.
pub struct PreparedChild(Task);

impl Task {
    /// Allocate the next child before opening its parent's spawn section.
    /// One owner may prepare only its next spawn; committing a different
    /// child first makes this reservation stale and is rejected.
    pub fn prepare_child(&self) -> PreparedChild {
        let mut path = self.path().to_vec();
        path.push(self.spawned);
        PreparedChild(unregistered_task(
            self.identity.clone(),
            path,
            self.trace.is_some(),
            self.observer.clone(),
        ))
    }
    pub fn path(&self) -> &[u32] {
        self.log.path()
    }
    pub fn progress(&self) -> Progress {
        self.log.progress()
    }
    pub fn trace(&self) -> &[Stamped] {
        self.trace.as_deref().unwrap_or_default()
    }

    /// Reserve log room for the next phase's records before its section opens: the section
    /// then encodes in place and allocates nothing. Record bytes beyond `bound` stay correct
    /// but allocate a segment inside the section, which [`RecordReservation`] reports.
    pub fn reserve(&mut self, bound: RecordBound) -> RecordReservation {
        // SAFETY: `&mut self` owns this task's writing side, and a section borrows its task
        // mutably, so none is open on it now.
        let allocated = unsafe { self.log.reserve(bound.bytes) };
        RecordReservation { bound, allocated }
    }

    /// Track `object` before the section of its first touch, so that touch inserts no entry.
    /// Its previous version starts at creation (1), exactly as an untracked first touch.
    pub fn track(&mut self, object: &ObjectId) {
        if !self.last.contains_key(object) {
            self.last.insert(object.clone(), 1);
        }
    }

    /// Track host key `key` before the section of this task's first position on it.
    pub fn track_key(&mut self, key: u32) {
        self.epochs.entry(key).or_insert(0);
    }
}

/// Log room a task reserved for one phase, outside any section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordReservation {
    pub bound: RecordBound,
    /// Segments the reservation allocated (0 when the log had room).
    pub allocated: usize,
}

struct Output<S> {
    sink: S,
    named: usize,
    watermark: BTreeMap<Vec<u32>, Progress>,
    projection: Option<crate::projection::Projection>,
}

/// A recorder's cut and the sink offset/flush policy belong to the caller. The
/// watermark advances only after `TapeSink::barrier` succeeds.
pub struct Cut {
    pub progress: BTreeMap<Vec<u32>, Progress>,
    pub terminal: bool,
}

/// A run's recorder; `B` is the request-owned host-call buffer representation.
pub struct Recorder<S: TapeSink, B = ()> {
    arbiter: Arbiter,
    identity: Arc<()>,
    executors: Arc<Executors>,
    requests: Requests<B>,
    root_started: std::sync::atomic::AtomicBool,
    keys: Mutex<Vec<String>>,
    output: Mutex<Output<S>>,
    durable: bool,
    trace: bool,
    metrics: Option<Arc<Metrics>>,
    observer: Option<Arc<Observer>>,
}

/// Optional redundant live-event observer. Events are copied inside their
/// publishing section, so a closed terminal gate also closes this snapshot.
#[derive(Default)]
pub struct Observer(Mutex<Vec<Stamped>>);
impl Observer {
    pub fn projection(
        &self,
        outcome: &str,
        cut: &BTreeMap<Vec<u32>, Progress>,
    ) -> crate::projection::Projection {
        crate::projection::Projection::new(outcome, cut, self.0.lock().unwrap().iter())
    }
}

/// B1 recording counters. A run without metrics allocates none, reads no
/// clock and adds no metric atomics to its phase path.
#[derive(Default)]
pub struct Metrics {
    pause_ns: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
    events: std::sync::atomic::AtomicU64,
    /// Log segments sections allocated past their reservation.
    overflow: std::sync::atomic::AtomicU64,
}

impl Metrics {
    pub fn values(&self) -> Vec<(&'static str, f64)> {
        use std::sync::atomic::Ordering::Relaxed;
        let mut values = vec![
            ("cut_pause_ns", self.pause_ns.load(Relaxed) as f64),
            (
                "section_segment_allocations",
                self.overflow.load(Relaxed) as f64,
            ),
        ];
        let events = self.events.load(Relaxed);
        if events != 0 {
            values.push((
                "compressed_bytes_per_contended_event",
                self.bytes.load(Relaxed) as f64 / events as f64,
            ));
        }
        values
    }
}

/// The registered executor slots of one recording: which section words have an owner. Slot
/// [`HOME`] belongs to the run's home executor: only a caller preferring it takes it, so home
/// never waits behind another executor's registration.
struct Executors {
    held: Box<[std::sync::atomic::AtomicBool]>,
    /// Executors blocked in [`Recorder::register`]; a retirement wakes them only when nonzero.
    waiting: std::sync::atomic::AtomicUsize,
    retired: (Mutex<()>, std::sync::Condvar),
}

/// The home executor's section word.
pub const HOME: usize = 0;

impl Executors {
    fn release(&self, index: usize) {
        use std::sync::atomic::{Ordering, fence};
        self.held[index].store(false, Ordering::Release);
        // Paired with `register`'s fence: a waiter either sees this word free in its search,
        // or is counted here and woken.
        fence(Ordering::SeqCst);
        if self.waiting.load(Ordering::Relaxed) != 0 {
            self.wake();
        }
    }

    fn wake(&self) {
        drop(self.retired.0.lock().unwrap());
        self.retired.1.notify_all();
    }
}

/// One registered physical executor's section word, owned for the executor's registered
/// lifetime: a compute worker, the home thread, a blocking or pool thread. It is not `Send`,
/// so it never migrates; a migrating task moves its [`Task`] encoder, never the slot. Phase
/// entry borrows it mutably, so a slot runs one section at a time, and dropping it (the
/// executor's retirement) cannot happen while a section on it is open. A nested callback on
/// an executor whose slot is checked out registers a distinct slot instead of reusing it.
pub struct ExecutorSlot {
    executors: Arc<Executors>,
    index: usize,
    executor: std::marker::PhantomData<*const ()>,
}

impl ExecutorSlot {
    /// The section word this slot owns.
    pub fn index(&self) -> usize {
        self.index
    }
}

impl Drop for ExecutorSlot {
    fn drop(&mut self) {
        self.executors.release(self.index);
    }
}

impl<S: TapeSink, B> Recorder<S, B> {
    pub fn new(
        mut sink: S,
        binding: &[u8],
        slots: usize,
        durable: bool,
        trace: bool,
        metrics: Option<Arc<Metrics>>,
    ) -> io::Result<Self> {
        assert!(slots != 0);
        sink.append(&tape::header(binding))?;
        if durable {
            sink.barrier()?;
        }
        Ok(Self {
            arbiter: Arbiter::new(true, slots),
            identity: Arc::new(()),
            executors: Arc::new(Executors {
                held: (0..slots)
                    .map(|_| std::sync::atomic::AtomicBool::new(false))
                    .collect(),
                waiting: std::sync::atomic::AtomicUsize::new(0),
                retired: Default::default(),
            }),
            requests: Requests::default(),
            root_started: std::sync::atomic::AtomicBool::new(false),
            keys: Mutex::new(Vec::new()),
            output: Mutex::new(Output {
                sink,
                named: 0,
                watermark: BTreeMap::new(),
                projection: None,
            }),
            durable,
            trace,
            metrics,
            observer: None,
        })
    }

    /// Install before root admission. Ordinary recording allocates no observer.
    pub fn observe(&mut self, observer: Arc<Observer>) {
        assert!(!self.root_started.load(std::sync::atomic::Ordering::Relaxed));
        self.observer = Some(observer);
    }

    /// Register an executor on section word `index`, outside any section. A second owner
    /// cannot register it until the first retires (drops its slot).
    pub fn executor(&self, index: usize) -> Option<ExecutorSlot> {
        use std::sync::atomic::Ordering;
        let held = self.executors.held.get(index)?;
        held.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        Some(ExecutorSlot {
            executors: self.executors.clone(),
            index,
            executor: std::marker::PhantomData,
        })
    }

    /// Register an executor on the preferred section word, or another free one when an old
    /// retiring executor still owns it ([`HOME`] only when preferred). No section is open while
    /// searching. `None` when every word is held: a caller that may hold a scheduler role or
    /// section never waits for one, and an executor starting outside both uses [`register`].
    ///
    /// [`register`]: Self::register
    pub fn executor_available(&self, preferred: usize) -> Option<ExecutorSlot> {
        self.executor(preferred).or_else(|| {
            (0..self.executors.held.len())
                .filter(|&index| index != preferred && index != HOME)
                .find_map(|index| self.executor(index))
        })
    }

    /// Register a starting executor once, for its whole lifetime, outside any scheduler role or
    /// section. When every word is held by live or retiring executors it blocks until one
    /// retires, never spinning; `None` once the run stopped, when no section can open again.
    pub fn register(&self, preferred: usize) -> Option<ExecutorSlot> {
        use std::sync::atomic::{Ordering, fence};
        if let Some(slot) = self.executor_available(preferred) {
            return Some(slot);
        }
        let executors = &self.executors;
        let mut guard = executors.retired.0.lock().unwrap();
        executors.waiting.fetch_add(1, Ordering::Relaxed);
        // Paired with `Executors::release`; a later release wakes under this lock.
        fence(Ordering::SeqCst);
        let slot = loop {
            if self.arbiter.stopped().is_some() {
                break None;
            }
            if let Some(slot) = self.executor_available(preferred) {
                break Some(slot);
            }
            guard = executors.retired.1.wait(guard).unwrap();
        };
        executors.waiting.fetch_sub(1, Ordering::Relaxed);
        slot
    }

    /// Retire an executor's registration. Its section word was acknowledged inactive: a slot
    /// borrows mutably into each section, so none can be open on it here, and a cut never
    /// waits for it again. Only then may another executor register the word.
    pub fn retire_executor(&self, slot: ExecutorSlot) {
        self.check_worker(&slot);
        assert!(
            self.arbiter.gate().unwrap().inactive(slot.index),
            "an executor retires with its section closed"
        );
        drop(slot);
    }

    /// The section words registered now: live, retiring or nested executors.
    pub fn registered(&self) -> Vec<usize> {
        (0..self.executors.held.len())
            .filter(|&index| self.executors.held[index].load(std::sync::atomic::Ordering::Acquire))
            .collect()
    }

    /// Whether `slot` was registered with this recording.
    pub fn owns(&self, slot: &ExecutorSlot) -> bool {
        Arc::ptr_eq(&slot.executors, &self.executors)
    }

    fn check_worker(&self, worker: &ExecutorSlot) {
        assert!(
            self.owns(worker),
            "executor slot belongs to another recording"
        );
    }

    /// Register the root at admission. Child registration instead belongs in
    /// the parent's spawn section (`Writer::spawn`) so a cut cannot split it
    /// from the spawning phase.
    pub fn root(&self, worker: &mut ExecutorSlot) -> Result<Task, Stopped> {
        assert!(
            !self
                .root_started
                .swap(true, std::sync::atomic::Ordering::AcqRel),
            "a run has one root"
        );
        self.check_worker(worker);
        let section = self
            .arbiter
            .section(worker.index)?
            .expect("a recording has a gate");
        Ok(new_task(
            &section,
            self.identity.clone(),
            Vec::new(),
            self.trace,
            self.observer.clone(),
        ))
    }

    /// Declare a logical key before recording its first transition. The
    /// mapping is persisted at the next cut, never rediscovered on replay.
    pub fn key(&self, name: &str) -> u32 {
        let mut keys = self.keys.lock().unwrap();
        if let Some(at) = keys.iter().position(|key| key == name) {
            return at as u32;
        }
        let at = u32::try_from(keys.len()).expect("logical key space exhausted");
        keys.push(name.into());
        at
    }

    /// One phase: gate admission, action + records, then publication. The
    /// engine acquires no user-visible ownership before this method admits it.
    pub fn phase<T>(
        &self,
        worker: &mut ExecutorSlot,
        task: &mut Task,
        progress: Progress,
        action: impl FnOnce(&mut Writer<'_, '_>) -> T,
    ) -> Result<T, Stopped> {
        self.phase_if(worker, task, progress, || true, action)
            .map(|result| result.expect("an unconditional phase acts"))
    }

    /// Admit a nonblocking entry prefix before creating a writer. A false
    /// prefix leaves progress/logs untouched and releases cutter exclusion;
    /// the caller may wait for its contended runtime service only afterward.
    /// A prefix that returns false must have made no observable change. A
    /// successful prefix and the bounded action publish together. Any admitted
    /// deterministic traversal tail belongs to the engine after this returns;
    /// no writer or section can span that tail.
    pub fn phase_if<T>(
        &self,
        worker: &mut ExecutorSlot,
        task: &mut Task,
        progress: Progress,
        prefix: impl FnOnce() -> bool,
        action: impl FnOnce(&mut Writer<'_, '_>) -> T,
    ) -> Result<Option<T>, Stopped> {
        self.check_worker(worker);
        assert!(
            Arc::ptr_eq(&task.identity, &self.identity),
            "task belongs to another recording"
        );
        assert!(
            progress > task.progress(),
            "a task publishes each phase once, in order"
        );
        let section = self
            .arbiter
            .gate()
            .unwrap()
            .enter_counted(worker.index, self.metrics.as_ref().map(|m| &m.pause_ns))?;
        if !prefix() {
            return Ok(None);
        }
        let mut writer = Writer {
            section,
            task,
            progress,
            bytes: 0,
            events: 0,
            overflow: 0,
        };
        let result = action(&mut writer);
        writer.section.publish(&writer.task.log, progress);
        if let Some(metrics) = &self.metrics {
            use std::sync::atomic::Ordering::Relaxed;
            metrics.bytes.fetch_add(writer.bytes, Relaxed);
            metrics.events.fetch_add(writer.events, Relaxed);
            metrics.overflow.fetch_add(writer.overflow, Relaxed);
        }
        Ok(Some(result))
    }

    /// Allocate request-owned buffers outside a section. A concurrent stop
    /// may withdraw this record even before its entered phase; it then never
    /// issues. `Writer::prepare` publishes Prepared in the entered phase.
    pub fn prepare(&self, id: u64, buffers: B) -> Arc<Request<B>> {
        self.requests.prepare(id, buffers)
    }

    /// The caller checked key predecessors and, when durable, `durable_through`
    /// outside the section. The call itself happens only after this returns.
    pub fn claim(&self, writer: &Writer<'_, '_>, request: &Request<B>) -> bool {
        assert_eq!(writer.progress.phase, Phase::Claimed);
        request.claim()
    }

    pub fn retire(&self, id: u64) {
        self.requests.retire(id);
    }

    /// Flush and barrier a causally closed cut. Cutter ordering is always gate
    /// then output, including stops. No sink lock is taken by a phase.
    pub fn flush(&self) -> io::Result<Option<Cut>> {
        let gate = self.arbiter.gate().unwrap();
        let Some(mut cut) = gate.flush_cut() else {
            return Ok(None);
        };
        let scanned = cut.scan();
        let progress = cut.progress().into_iter().collect();
        let mut output = self.output.lock().unwrap();
        let keys = self.keys.lock().unwrap();
        let bytes = tape::flush(&keys[output.named..], &chunks(scanned));
        output.sink.append(&bytes)?;
        output.sink.barrier()?;
        output.named = keys.len();
        output.watermark = progress;
        Ok(Some(Cut {
            progress: output.watermark.clone(),
            terminal: false,
        }))
    }

    pub fn durable_through(&self, task: &[u32], progress: Progress) -> io::Result<bool> {
        if !self.durable {
            return Ok(true);
        }
        if self
            .output
            .lock()
            .unwrap()
            .watermark
            .get(task)
            .is_some_and(|at| at.admits(progress))
        {
            return Ok(true);
        }
        if self.flush()?.is_none() {
            return Ok(false);
        }
        Ok(self
            .output
            .lock()
            .unwrap()
            .watermark
            .get(task)
            .is_some_and(|at| at.admits(progress)))
    }

    /// A terminal cut arbitrates against claimed requests before writing. It
    /// waits for sections, never for a syscall, foreign call or whole segment.
    pub fn stop(&self, outcome: &str, task: Option<&[u32]>) -> io::Result<Option<(Cut, Sweep)>> {
        self.stop_with(|_| outcome.into(), task)
    }

    /// Build the terminal payload from the request sweep while the terminal
    /// gate is closed. The sink observes the cut, receipt and cause atomically.
    pub fn stop_with(
        &self,
        outcome: impl FnOnce(&Sweep) -> String,
        task: Option<&[u32]>,
    ) -> io::Result<Option<(Cut, Sweep)>> {
        let Some(terminal) = self.arbiter.stop(0, &[&self.requests]) else {
            return Ok(None);
        };
        // No section opens again: an executor waiting to register starts unregistered.
        self.executors.wake();
        let mut cut = terminal.cut.unwrap();
        let mut scanned: BTreeMap<_, _> =
            cut.scan().into_iter().map(|s| (s.path, s.bytes)).collect();
        let progress: BTreeMap<_, _> = cut.progress().into_iter().collect();
        let tasks: Vec<_> = progress
            .iter()
            .map(|(path, progress)| TaskChunk {
                path: path.clone(),
                progress: *progress,
                bytes: scanned.remove(path).unwrap_or_default(),
            })
            .collect();
        let mut output = self.output.lock().unwrap();
        let keys = self.keys.lock().unwrap();
        let end = End {
            outcome: outcome(&terminal.sweep),
            task: task.map(<[u32]>::to_vec),
        };
        let bytes = tape::stop(&keys[output.named..], &tasks, &end);
        output.sink.append(&bytes)?;
        output.sink.barrier()?;
        output.named = keys.len();
        // Terminal output owns this assignment. A concurrent reporter cannot
        // observe a terminal frame without its corresponding live snapshot.
        output.projection = self
            .observer
            .as_ref()
            .map(|observer| observer.projection(&end.outcome, &progress));
        Ok(Some((
            Cut {
                progress,
                terminal: true,
            },
            terminal.sweep,
        )))
    }

    pub fn projection(&self) -> Option<crate::projection::Projection> {
        self.output.lock().unwrap().projection.clone()
    }

    pub fn into_sink(self) -> S {
        self.output.into_inner().unwrap().sink
    }
}

fn chunks(scanned: Vec<Scanned>) -> Vec<TaskChunk> {
    scanned
        .into_iter()
        .map(|s| TaskChunk {
            path: s.path,
            progress: s.progress,
            bytes: s.bytes,
        })
        .collect()
}
fn new_task(
    section: &Section<'_>,
    identity: Arc<()>,
    path: Vec<u32>,
    trace: bool,
    observer: Option<Arc<Observer>>,
) -> Task {
    let task = unregistered_task(identity, path, trace, observer);
    section.register(&task.log);
    task
}

fn unregistered_task(
    identity: Arc<()>,
    path: Vec<u32>,
    trace: bool,
    observer: Option<Arc<Observer>>,
) -> Task {
    Task {
        identity,
        log: TaskLog::new(path),
        encoder: Encoder::default(),
        last: HashMap::new(),
        epochs: HashMap::new(),
        canceled: None,
        cancel_checks: CheckIndex::default(),
        trace: trace.then(Vec::new),
        spawned: 0,
        observer,
    }
}

/// A phase's writer: all its records are appended before its progress becomes
/// visible to a cutter. No reference escapes the action closure.
pub struct Writer<'g, 't> {
    section: Section<'g>,
    task: &'t mut Task,
    progress: Progress,
    bytes: u64,
    events: u64,
    overflow: u64,
}

impl Writer<'_, '_> {
    pub fn spawn(&mut self) -> Task {
        self.spawn_prepared(self.task.prepare_child())
    }
    /// Register a preallocated log as this owner's next child, in the same
    /// section as its runtime task-row and handle publication.
    pub fn spawn_prepared(&mut self, child: PreparedChild) -> Task {
        let child = child.0;
        assert!(
            Arc::ptr_eq(&child.identity, &self.task.identity),
            "a child belongs to its recorder"
        );
        let (index, parent) = child.path().split_last().expect("a child has a parent");
        assert_eq!(parent, self.task.path(), "a child belongs to its parent");
        assert_eq!(*index, self.task.spawned, "a child is the next spawn");
        self.task.spawned = self
            .task
            .spawned
            .checked_add(1)
            .expect("spawn path ordinal overflow");
        self.section.register(&child.log);
        child
    }
    pub fn prepare<B>(&mut self, request: &Request<B>) {
        assert_eq!(self.progress.phase, Phase::Entered);
        self.record(Record::Prepared {
            op: self.progress.op,
            id: request.id,
        });
    }
    /// Encode `record` in place into the task's reserved log room. Returns its byte length.
    fn record(&mut self, record: Record) -> u64 {
        assert_eq!(record.op(), self.progress.op);
        let encoder = &mut self.task.encoder;
        let (bytes, overflow) = self.section.write(&self.task.log, |out| {
            let start = out.len();
            encoder.encode(&record, out);
            (out.len() - start, out.overflow)
        });
        self.overflow += overflow as u64;
        bytes as u64
    }
    fn payload(&mut self, completion: bool, bytes: &[u8]) {
        let (op, encoder) = (self.progress.op, &mut self.task.encoder);
        let overflow = self.section.write(&self.task.log, |out| {
            encoder.payload(completion, op, bytes, out);
            out.overflow
        });
        self.overflow += overflow as u64;
    }
    /// Whether an optional diagnostic observer or the canonical test trace wants events.
    pub fn observed(&self) -> bool {
        self.task.observer.is_some() || self.task.trace.is_some()
    }
    /// Build an optional diagnostic event only when someone observes it: ordinary recording
    /// constructs no `Event` and copies no outcome.
    fn event(&mut self, event: impl FnOnce() -> Event) {
        if !self.observed() {
            return;
        }
        let event = event();
        if let Some(observer) = &self.task.observer {
            observer.0.lock().unwrap().push(Stamped {
                task: self.task.log.path().into(),
                progress: self.progress,
                event: event.clone(),
            });
        }
        if let Some(trace) = &mut self.task.trace {
            trace.push(Stamped {
                task: self.task.log.path().into(),
                progress: self.progress,
                event,
            });
        }
    }
    /// The object lock's action just produced `version`. Version is bumped
    /// and this record is written while the same object lock is held.
    pub fn touch(&mut self, object: &ObjectId, version: u64, sub: u32, outcome: &str) {
        let previous = match self.task.last.get_mut(object) {
            Some(last) => std::mem::replace(last, version),
            None => {
                // Untracked first touch: correct, but it allocates inside the section.
                self.task.last.insert(object.clone(), version);
                1
            }
        };
        let gap = version
            .checked_sub(previous + 1)
            .expect("object versions advance in the object's section order");
        if gap != 0 {
            self.bytes += self.record(Record::Gap {
                op: self.progress.op,
                sub,
                gap,
            });
            self.events += 1;
        }
        self.event(|| Event::Versioned {
            object: object.clone(),
            version,
            sub,
            outcome: outcome.into(),
        });
    }
    pub fn create(&mut self, object: ObjectId) {
        self.event(|| Event::Versioned {
            object,
            version: 1,
            sub: 0,
            outcome: "create".into(),
        });
    }
    pub fn receive(&mut self, object: ObjectId, outcome: &str) {
        self.event(|| Event::Received {
            object,
            outcome: outcome.into(),
        });
    }
    pub fn load(&mut self, bits: u128) {
        self.record(Record::Load {
            op: self.progress.op,
            bits,
        });
        self.event(|| Event::Observed(Observed::Load(bits)));
    }
    pub fn thaw(&mut self, ok: bool) {
        self.record(Record::Thaw {
            op: self.progress.op,
            ok,
        });
        self.event(|| Event::Observed(Observed::Thaw(ok)));
    }
    pub fn await_any(&mut self, index: u64) {
        self.record(Record::AwaitAny {
            op: self.progress.op,
            index,
        });
        self.event(|| Event::Observed(Observed::AwaitAny(index)));
    }
    pub fn cancel(&mut self, cause: Option<u32>) {
        let check = self.task.cancel_checks.take(self.progress.op);
        if std::mem::replace(&mut self.task.canceled, cause) != cause {
            self.record(Record::Cancel {
                op: self.progress.op,
                check,
                cause,
            });
        }
        self.event(|| Event::Observed(Observed::Cancel(cause)));
    }
    pub fn expired(&mut self, expired: bool) {
        if expired {
            self.record(Record::Expired {
                op: self.progress.op,
            });
        }
        self.event(|| Event::Observed(Observed::Expired(expired)));
    }
    pub fn key(&mut self, key: u32, name: &str, epoch: u64, shared: bool, index: u32) {
        let previous = match self.task.epochs.get_mut(&key) {
            Some(last) => std::mem::replace(last, epoch),
            None => self.task.epochs.insert(key, epoch).unwrap_or(0),
        };
        let gap = epoch
            .checked_sub(previous)
            .expect("a task's key epochs never go backwards");
        self.record(Record::Key {
            op: self.progress.op,
            key,
            shared,
            gap,
        });
        self.event(|| Event::Key {
            key: name.into(),
            epoch,
            index,
        });
    }
    pub fn observation(&mut self, bytes: &[u8]) {
        self.payload(false, bytes);
    }
    pub fn complete<B>(&mut self, request: &Request<B>, bytes: &[u8]) {
        assert_eq!(self.progress.phase, Phase::Done);
        request.finish();
        self.payload(true, bytes);
    }
    pub fn output(&mut self, sink: &str, epoch: u64, bytes: &[u8]) {
        self.event(|| Event::Output {
            sink: sink.into(),
            epoch,
            bytes: bytes.into(),
        });
    }
    pub fn blocked(&mut self) {
        self.record(Record::Blocked {
            op: self.progress.op,
        });
    }
    pub fn ready(&mut self) {
        self.record(Record::Ready {
            op: self.progress.op,
        });
    }
}
