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
//!
//! A phase declares its complete [`RecordBound`] and names only objects and keys its task
//! tracked beforehand. Admission checks the reserved room first and, when it is short,
//! reserves outside any section and admits again with the same progress, before the prefix
//! or the action runs (rr `syscallbuf.c`, which checks buffer space before the syscall and
//! declines before any effect). Inside the section the writer allocates nothing, takes no
//! lock and runs no destructor; optional diagnostic notes are staged in reserved room and
//! published, stamped with the phase's task and progress, after the section closes.

use crate::claim::{Arbiter, Request, Requests, Sweep};
pub use crate::executors::{ExecutorSlot, Executors, HOME};
use crate::phase::{Phase, Progress};
use crate::projection::{Event, Observed, Projection, Stamped};
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
    /// Each tracked object's last version this task saw. The key is shared so a note can
    /// name its object without allocating.
    last: HashMap<Arc<ObjectId>, u64>,
    epochs: HashMap<u32, u64>,
    canceled: Option<u32>,
    cancel_checks: CheckIndex,
    /// Canonical test trace, absent in ordinary recording.
    trace: Option<Vec<Stamped>>,
    spawned: u32,
    observer: Option<Arc<Observer>>,
    /// One phase's staged diagnostic notes; present only when someone observes.
    notes: Option<Notes>,
}

/// The diagnostic notes of the phase in progress, in room reserved before its section.
#[derive(Default)]
struct Notes {
    list: Vec<Note>,
    /// Labels, names and output bytes the notes refer to.
    text: Vec<u8>,
}

type Span = std::ops::Range<usize>;

/// An [`Event`] whose variable parts are shared or staged: building one allocates nothing.
enum Note {
    Versioned {
        object: Arc<ObjectId>,
        version: u64,
        sub: u32,
        outcome: Span,
    },
    Received {
        object: Arc<ObjectId>,
        outcome: Span,
    },
    Observed(Observed),
    Key {
        key: Span,
        epoch: u64,
        index: u32,
    },
    Output {
        sink: Span,
        epoch: u64,
        bytes: Span,
    },
}

impl Note {
    fn event(self, text: &[u8]) -> Event {
        let label = |span: Span| {
            std::str::from_utf8(&text[span])
                .expect("a staged label is a copied str")
                .to_owned()
        };
        match self {
            Note::Versioned {
                object,
                version,
                sub,
                outcome,
            } => Event::Versioned {
                object: ObjectId::clone(&object),
                version,
                sub,
                outcome: label(outcome),
            },
            Note::Received { object, outcome } => Event::Received {
                object: ObjectId::clone(&object),
                outcome: label(outcome),
            },
            Note::Observed(observed) => Event::Observed(observed),
            Note::Key { key, epoch, index } => Event::Key {
                key: label(key),
                epoch,
                index,
            },
            Note::Output { sink, epoch, bytes } => Event::Output {
                sink: label(sink),
                epoch,
                bytes: text[bytes].to_vec(),
            },
        }
    }
}

/// An allocated child log that has not joined the recorder registry. A cut
/// before its parent's spawn section must not publish this child's START.
pub struct PreparedChild(Task);

impl Task {
    /// Allocate the next child before opening its parent's spawn section.
    /// One owner may prepare only its next spawn; committing a different
    /// child first makes this reservation stale and is rejected.
    pub fn prepare_child(&self) -> PreparedChild {
        self.prepare_spawn(0)
    }
    /// Allocate the next `n` children, for one section that spawns them in order.
    pub fn prepare_children(&self, n: u32) -> Vec<PreparedChild> {
        (0..n).map(|at| self.prepare_spawn(at)).collect()
    }
    fn prepare_spawn(&self, at: u32) -> PreparedChild {
        let ordinal = self
            .spawned
            .checked_add(at)
            .expect("spawn path ordinal overflow");
        PreparedChild(unregistered_task(
            self.identity.clone(),
            self.log.child(ordinal),
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

    /// Reserve room for the next phase's records (and notes, when observed) before its
    /// section opens: the section then encodes in place and allocates nothing. Admission
    /// reserves whatever is still short itself; reserving earlier only moves the allocation.
    pub fn reserve(&mut self, bound: RecordBound) -> RecordReservation {
        // SAFETY: `&mut self` owns this task's writing side, and a section borrows its task
        // mutably, so none is open on it now.
        let allocated = unsafe { self.log.reserve(bound.bytes) };
        if let Some(notes) = &mut self.notes {
            // Notes are published and cleared after every section, so this is total room.
            notes.list.reserve(bound.notes);
            notes.text.reserve(bound.text);
        }
        RecordReservation { bound, allocated }
    }

    /// Whether the reserved room holds everything `bound` may write.
    fn fits(&self, bound: RecordBound) -> bool {
        // SAFETY: `&self` here is the owner, outside its sections or inside its own.
        (unsafe { self.log.room() }) >= bound.bytes
            && self.notes.as_ref().is_none_or(|notes| {
                notes.list.capacity() - notes.list.len() >= bound.notes
                    && notes.text.capacity() - notes.text.len() >= bound.text
            })
    }

    /// Track `object` before the section of the first phase that names it (a touch, its
    /// creation or a receive), so no section inserts an entry. Its previous version starts at
    /// creation (1).
    pub fn track(&mut self, object: &ObjectId) {
        if !self.last.contains_key(object) {
            self.last.insert(Arc::new(object.clone()), 1);
        }
    }

    /// Track host key `key` before the section of this task's first position on it.
    pub fn track_key(&mut self, key: u32) {
        self.epochs.entry(key).or_insert(0);
    }

    /// Publish the notes a closed section staged, stamped with its task and `progress`.
    fn publish_notes(&mut self, progress: Progress) {
        let Some(notes) = &mut self.notes else {
            return;
        };
        if notes.list.is_empty() {
            return;
        }
        let path = self.log.path();
        let mut events = Vec::with_capacity(notes.list.len());
        for note in notes.list.drain(..) {
            events.push(Stamped {
                task: path.to_vec(),
                progress,
                event: note.event(&notes.text),
            });
        }
        notes.text.clear();
        if let Some(observer) = &self.observer {
            observer.events.lock().unwrap().extend_from_slice(&events);
            observer
                .in_flight
                .fetch_sub(1, std::sync::atomic::Ordering::Release);
        }
        if let Some(trace) = &mut self.trace {
            trace.append(&mut events);
        }
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
    /// The terminal outcome, once its stop chunk is durable. The cut stays in the stopped gate.
    terminal: Option<String>,
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
    executors: Executors,
    requests: Requests<B>,
    root_started: std::sync::atomic::AtomicBool,
    keys: Mutex<Vec<String>>,
    output: Mutex<Output<S>>,
    durable: bool,
    trace: bool,
    metrics: Option<Arc<Metrics>>,
    observer: Option<Arc<Observer>>,
}

/// Optional redundant live-event observer. A phase stages its events in reserved room and
/// publishes them, stamped, right after its section closes; its section counts them in
/// flight, so a projection waits for every phase a terminal cut admitted.
#[derive(Default)]
pub struct Observer {
    events: Mutex<Vec<Stamped>>,
    in_flight: std::sync::atomic::AtomicU64,
}
impl Observer {
    /// The projection of the events `cut` admits. Call it after the cut closed: no phase
    /// counts itself in flight afterwards, and those that did publish in bounded time.
    pub fn projection(&self, outcome: &str, cut: &BTreeMap<Vec<u32>, Progress>) -> Projection {
        while self.in_flight.load(std::sync::atomic::Ordering::Acquire) != 0 {
            std::thread::yield_now();
        }
        Projection::new(outcome, cut, self.events.lock().unwrap().iter())
    }
}

/// B1 recording counters. A run without metrics allocates none, reads no
/// clock and adds no metric atomics to its phase path.
#[derive(Default)]
pub struct Metrics {
    pause_ns: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
    events: std::sync::atomic::AtomicU64,
    /// Phases whose reserved room was short at admission and reserved before acting.
    short: std::sync::atomic::AtomicU64,
    /// Instrumentation: called on the section's thread as it opens (`true`) and just before
    /// it closes (`false`), so a test allocator can trap allocations inside sections.
    section: Option<fn(bool)>,
}

impl Metrics {
    /// Counters that also report each section's open window to `hook`.
    pub fn with_section_hook(hook: fn(bool)) -> Metrics {
        Metrics {
            section: Some(hook),
            ..Metrics::default()
        }
    }
    fn section(metrics: Option<&Arc<Metrics>>, open: bool) {
        if let Some(hook) = metrics.and_then(|metrics| metrics.section) {
            hook(open);
        }
    }
    pub fn values(&self) -> Vec<(&'static str, f64)> {
        use std::sync::atomic::Ordering::Relaxed;
        let mut values = vec![
            ("cut_pause_ns", self.pause_ns.load(Relaxed) as f64),
            ("short_reservations", self.short.load(Relaxed) as f64),
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
            executors: Executors::new(slots),
            requests: Requests::default(),
            root_started: std::sync::atomic::AtomicBool::new(false),
            keys: Mutex::new(Vec::new()),
            output: Mutex::new(Output {
                sink,
                named: 0,
                watermark: BTreeMap::new(),
                terminal: None,
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
        self.executors.executor(index)
    }

    /// Register an executor on the preferred section word or another free one; see
    /// [`Executors::available`].
    pub fn executor_available(&self, preferred: usize) -> Option<ExecutorSlot> {
        self.executors.available(preferred)
    }

    /// Register a starting executor once, outside any scheduler role or section, waiting for a
    /// retirement when every word is held; `None` once the run stopped. See
    /// [`Executors::register`].
    pub fn register(&self, preferred: usize) -> Option<ExecutorSlot> {
        self.executors
            .register(preferred, || self.arbiter.stopped().is_some())
    }

    /// Retire an executor's registration. Its section word was acknowledged inactive: a slot
    /// borrows mutably into each section, so none can be open on it here, and a cut never
    /// waits for it again. Only then may another executor register the word.
    pub fn retire_executor(&self, slot: ExecutorSlot) {
        self.check_worker(&slot);
        assert!(
            self.arbiter.gate().unwrap().inactive(slot.index()),
            "an executor retires with its section closed"
        );
        drop(slot);
    }

    /// The section words registered now: live, retiring or nested executors.
    pub fn registered(&self) -> Vec<usize> {
        self.executors.registered()
    }

    /// Whether `slot` was registered with this recording.
    pub fn owns(&self, slot: &ExecutorSlot) -> bool {
        slot.belongs(&self.executors)
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
        let task = unregistered_task(
            self.identity.clone(),
            TaskLog::new(&[]),
            self.trace,
            self.observer.clone(),
        );
        let section = self
            .arbiter
            .section(worker.index())?
            .expect("a recording has a gate");
        Metrics::section(self.metrics.as_ref(), true);
        section.register(&task.log);
        Metrics::section(self.metrics.as_ref(), false);
        Ok(task)
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
    /// `bound` covers everything the action writes.
    pub fn phase<T>(
        &self,
        worker: &mut ExecutorSlot,
        task: &mut Task,
        progress: Progress,
        bound: RecordBound,
        action: impl FnOnce(&mut Writer<'_, '_>) -> T,
    ) -> Result<T, Stopped> {
        self.phase_if(worker, task, progress, bound, || true, action)
            .map(|result| result.expect("an unconditional phase acts"))
    }

    /// Admit a nonblocking entry prefix before creating a writer. A false
    /// prefix leaves progress/logs untouched and releases cutter exclusion;
    /// the caller may wait for its contended runtime service only afterward.
    /// A prefix that returns false must have made no observable change. A
    /// successful prefix and the bounded action publish together. Any admitted
    /// deterministic traversal tail belongs to the engine after this returns;
    /// no writer or section can span that tail.
    ///
    /// Room short of `bound` is found before admission, so before the prefix or any effect,
    /// and is reserved outside the section; the phase keeps its progress.
    pub fn phase_if<T>(
        &self,
        worker: &mut ExecutorSlot,
        task: &mut Task,
        progress: Progress,
        bound: RecordBound,
        prefix: impl FnOnce() -> bool,
        action: impl FnOnce(&mut Writer<'_, '_>) -> T,
    ) -> Result<Option<T>, Stopped> {
        self.phase_attempt(worker, task, progress, (bound, false), prefix, action)
    }

    /// Nonblocking admission for a typed runtime continuation. None retains
    /// the same progress with no prefix effects; the caller keeps its phase.
    pub fn try_phase_if<T>(
        &self,
        worker: &mut ExecutorSlot,
        task: &mut Task,
        progress: Progress,
        bound: RecordBound,
        prefix: impl FnOnce() -> bool,
        action: impl FnOnce(&mut Writer<'_, '_>) -> T,
    ) -> Result<Option<T>, Stopped> {
        self.phase_attempt(worker, task, progress, (bound, true), prefix, action)
    }

    fn phase_attempt<T>(
        &self,
        worker: &mut ExecutorSlot,
        task: &mut Task,
        progress: Progress,
        (bound, nonblocking): (RecordBound, bool),
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
        let short = !task.fits(bound);
        if short {
            task.reserve(bound);
        }
        let gate = self.arbiter.gate().unwrap();
        let section = if nonblocking {
            let Some(section) = gate.try_enter(worker.index())? else {
                return Ok(None);
            };
            section
        } else {
            gate.enter_counted(worker.index(), self.metrics.as_ref().map(|m| &m.pause_ns))?
        };
        Metrics::section(self.metrics.as_ref(), true);
        debug_assert!(task.fits(bound), "only the owner writes its reserved room");
        if !prefix() {
            Metrics::section(self.metrics.as_ref(), false);
            return Ok(None);
        }
        let mut writer = Writer {
            section,
            task,
            progress,
            bound,
            used: RecordBound::default(),
            bytes: 0,
            events: 0,
        };
        let result = action(&mut writer);
        let Writer {
            section,
            task,
            bytes,
            events,
            ..
        } = writer;
        section.publish(&task.log, progress);
        let noted = task
            .notes
            .as_ref()
            .is_some_and(|notes| !notes.list.is_empty());
        if noted && let Some(observer) = &task.observer {
            observer
                .in_flight
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Metrics::section(self.metrics.as_ref(), false);
        drop(section);
        task.publish_notes(progress);
        if let Some(metrics) = &self.metrics {
            use std::sync::atomic::Ordering::Relaxed;
            metrics.bytes.fetch_add(bytes, Relaxed);
            metrics.events.fetch_add(events, Relaxed);
            metrics.short.fetch_add(u64::from(short), Relaxed);
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

    /// [`end_with`](Self::end_with), returning the terminal cut too.
    pub fn stop_with(
        &self,
        outcome: impl FnOnce(&Sweep) -> String,
        task: Option<&[u32]>,
    ) -> io::Result<Option<(Cut, Sweep)>> {
        let Some(sweep) = self.end_with(outcome, task)? else {
            return Ok(None);
        };
        let cut = self.terminal_cut().expect("a written stop has its cut");
        Ok(Some((cut, sweep)))
    }

    /// Build the terminal payload from the request sweep while the terminal
    /// gate is closed. The sink observes the cut, receipt and cause atomically.
    /// The stop chunk is encoded from the closed logs in place; the cut is
    /// materialized only on request ([`terminal_cut`](Self::terminal_cut)).
    pub fn end_with(
        &self,
        outcome: impl FnOnce(&Sweep) -> String,
        task: Option<&[u32]>,
    ) -> io::Result<Option<Sweep>> {
        let Some(terminal) = self.arbiter.stop(0, &[&self.requests]) else {
            return Ok(None);
        };
        // No section opens again: an executor waiting to register starts unregistered.
        self.executors.wake();
        let mut cut = terminal.cut.unwrap();
        let tasks = cut.terminal();
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
        // The gate is stopped and its cutter role free before the outcome is published,
        // so a reader of the outcome finds the terminal progress.
        drop(tasks);
        drop(cut);
        // Terminal output owns this assignment. A concurrent reporter cannot
        // observe a terminal frame before its stop chunk is written.
        output.terminal = Some(end.outcome);
        Ok(Some(terminal.sweep))
    }

    /// The terminal cut, once its stop chunk is written.
    pub fn terminal_cut(&self) -> Option<Cut> {
        self.output.lock().unwrap().terminal.as_ref()?;
        Some(Cut {
            progress: self.terminal_progress(),
            terminal: true,
        })
    }

    /// Called only after the stop chunk is written; the output lock is not held, keeping
    /// cutter order gate then output.
    fn terminal_progress(&self) -> BTreeMap<Vec<u32>, Progress> {
        let gate = self.arbiter.gate().unwrap();
        let progress = gate.terminal_progress().expect("the run stopped");
        progress.into_iter().collect()
    }

    /// The observer's projection of the terminal cut, once stopped.
    pub fn projection(&self) -> Option<Projection> {
        let observer = self.observer.as_ref()?;
        let outcome = self.output.lock().unwrap().terminal.clone()?;
        Some(observer.projection(&outcome, &self.terminal_progress()))
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
fn unregistered_task(
    identity: Arc<()>,
    log: Arc<TaskLog>,
    trace: bool,
    observer: Option<Arc<Observer>>,
) -> Task {
    Task {
        identity,
        log,
        encoder: Encoder::default(),
        last: HashMap::new(),
        epochs: HashMap::new(),
        canceled: None,
        cancel_checks: CheckIndex::default(),
        notes: (trace || observer.is_some()).then(Notes::default),
        trace: trace.then(Vec::new),
        spawned: 0,
        observer,
    }
}

/// A phase's writer: all its records are appended before its progress becomes
/// visible to a cutter. No reference escapes the action closure. It writes only
/// within its phase's declared bound, into room reserved before the section.
pub struct Writer<'g, 't> {
    section: Section<'g>,
    task: &'t mut Task,
    progress: Progress,
    bound: RecordBound,
    /// What the phase wrote so far, against `bound`.
    used: RecordBound,
    bytes: u64,
    events: u64,
}

impl Writer<'_, '_> {
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
    /// Count `n` record bytes against the phase's bound.
    fn wrote(&mut self, n: usize) {
        self.used.bytes += n;
        assert!(
            self.used.bytes <= self.bound.bytes,
            "phase {} wrote {} record bytes past its bound {:?}",
            self.progress,
            self.used.bytes,
            self.bound
        );
    }
    /// Encode `record` in place into the task's reserved log room. Returns its byte length.
    fn record(&mut self, record: Record) -> u64 {
        assert_eq!(record.op(), self.progress.op);
        let encoder = &mut self.task.encoder;
        let bytes = self.section.write(&self.task.log, |out| {
            let start = out.len();
            encoder.encode(&record, out);
            out.len() - start
        });
        self.wrote(bytes);
        bytes as u64
    }
    fn payload(&mut self, completion: bool, bytes: &[u8]) {
        let (op, encoder) = (self.progress.op, &mut self.task.encoder);
        let written = self.section.write(&self.task.log, |out| {
            let start = out.len();
            encoder.payload(completion, op, bytes, out);
            out.len() - start
        });
        self.wrote(written);
    }
    /// Whether an optional diagnostic observer or the canonical test trace wants events.
    pub fn observed(&self) -> bool {
        self.task.notes.is_some()
    }
    /// Count one note of `text` label bytes against the phase's bound, observed or not, so a
    /// short bound fails the same way with diagnostics off.
    fn noted(&mut self, text: usize) -> bool {
        self.used.notes += 1;
        self.used.text += text;
        assert!(
            self.used.notes <= self.bound.notes && self.used.text <= self.bound.text,
            "phase {} noted past its bound {:?}",
            self.progress,
            self.bound
        );
        self.observed()
    }
    /// Copy `bytes` into the phase's reserved note text.
    fn stage(&mut self, bytes: &[u8]) -> Span {
        let text = &mut self.task.notes.as_mut().unwrap().text;
        assert!(
            text.capacity() - text.len() >= bytes.len(),
            "note text is reserved"
        );
        let start = text.len();
        text.extend_from_slice(bytes);
        start..text.len()
    }
    fn note(&mut self, note: Note) {
        let list = &mut self.task.notes.as_mut().unwrap().list;
        assert!(list.len() < list.capacity(), "notes are reserved");
        list.push(note);
    }
    /// The shared identity of a tracked object.
    fn tracked(&self, object: &ObjectId) -> Arc<ObjectId> {
        let (object, _) = self
            .task
            .last
            .get_key_value(object)
            .expect("an object is tracked before the section that names it");
        object.clone()
    }
    fn observe(&mut self, observed: Observed) {
        if self.noted(0) {
            self.note(Note::Observed(observed));
        }
    }
    /// The object lock's action just produced `version`. Version is bumped
    /// and this record is written while the same object lock is held.
    pub fn touch(&mut self, object: &ObjectId, version: u64, sub: u32, outcome: &str) {
        let last = self
            .task
            .last
            .get_mut(object)
            .expect("an object is tracked before the section of its first touch");
        let previous = std::mem::replace(last, version);
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
        if self.noted(outcome.len()) {
            let object = self.tracked(object);
            let outcome = self.stage(outcome.as_bytes());
            self.note(Note::Versioned {
                object,
                version,
                sub,
                outcome,
            });
        }
    }
    pub fn create(&mut self, object: &ObjectId) {
        const CREATE: &str = "create";
        if self.noted(CREATE.len()) {
            let object = self.tracked(object);
            let outcome = self.stage(CREATE.as_bytes());
            self.note(Note::Versioned {
                object,
                version: 1,
                sub: 0,
                outcome,
            });
        }
    }
    pub fn receive(&mut self, object: &ObjectId, outcome: &str) {
        if self.noted(outcome.len()) {
            let object = self.tracked(object);
            let outcome = self.stage(outcome.as_bytes());
            self.note(Note::Received { object, outcome });
        }
    }
    pub fn load(&mut self, bits: u128) {
        self.record(Record::Load {
            op: self.progress.op,
            bits,
        });
        self.observe(Observed::Load(bits));
    }

    pub fn cpu_control(&mut self, width: u32, action: u32, count: u64, outcome: u32) {
        self.observation(&crate::projection::cpu_request(width, action, count));
        self.record(Record::Load {
            op: self.progress.op,
            bits: u128::from(outcome),
        });
        self.observe(Observed::CpuControl {
            width,
            action,
            count,
            outcome,
        });
    }
    pub fn thaw(&mut self, ok: bool) {
        self.record(Record::Thaw {
            op: self.progress.op,
            ok,
        });
        self.observe(Observed::Thaw(ok));
    }
    pub fn await_any(&mut self, index: u64) {
        self.record(Record::AwaitAny {
            op: self.progress.op,
            index,
        });
        self.observe(Observed::AwaitAny(index));
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
        self.observe(Observed::Cancel(cause));
    }
    pub fn expired(&mut self, expired: bool) {
        if expired {
            self.record(Record::Expired {
                op: self.progress.op,
            });
        }
        self.observe(Observed::Expired(expired));
    }
    /// One position on host key `key`, which the task tracked before this section.
    pub fn key(&mut self, key: u32, name: &str, epoch: u64, shared: bool, index: u32) {
        let last = self
            .task
            .epochs
            .get_mut(&key)
            .expect("a host key is tracked before the section of its first position");
        let previous = std::mem::replace(last, epoch);
        let gap = epoch
            .checked_sub(previous)
            .expect("a task's key epochs never go backwards");
        self.record(Record::Key {
            op: self.progress.op,
            key,
            shared,
            gap,
        });
        if self.noted(name.len()) {
            let key = self.stage(name.as_bytes());
            self.note(Note::Key { key, epoch, index });
        }
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
        if self.noted(sink.len() + bytes.len()) {
            let sink = self.stage(sink.as_bytes());
            let bytes = self.stage(bytes);
            self.note(Note::Output { sink, epoch, bytes });
        }
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
