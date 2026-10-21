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
use crate::record::{Encoder, ObjectId, Record};
use crate::recorder::TapeSink;
use crate::section::{Gate, Scanned, Section, Stopped, TaskLog};
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
    scratch: Vec<u8>,
    /// Canonical test trace, absent in ordinary recording.
    trace: Option<Vec<Stamped>>,
    spawned: u32,
}

impl Task {
    pub fn path(&self) -> &[u32] {
        self.log.path()
    }
    pub fn progress(&self) -> Progress {
        self.log.progress()
    }
    pub fn trace(&self) -> &[Stamped] {
        self.trace.as_deref().unwrap_or_default()
    }
}

struct Output<S> {
    sink: S,
    named: usize,
    watermark: BTreeMap<Vec<u32>, Progress>,
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
    slots: Box<[std::sync::atomic::AtomicBool]>,
    requests: Requests<B>,
    root_started: std::sync::atomic::AtomicBool,
    keys: Mutex<Vec<String>>,
    output: Mutex<Output<S>>,
    durable: bool,
    trace: bool,
    metrics: Option<Arc<Metrics>>,
}

/// B1 recording counters. A run without metrics allocates none, reads no
/// clock and adds no metric atomics to its phase path.
#[derive(Default)]
pub struct Metrics {
    pause_ns: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
    events: std::sync::atomic::AtomicU64,
}

impl Metrics {
    pub fn values(&self) -> Vec<(&'static str, f64)> {
        use std::sync::atomic::Ordering::Relaxed;
        let mut values = vec![("cut_pause_ns", self.pause_ns.load(Relaxed) as f64)];
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

/// Exclusive ownership of one worker's section word. It may migrate with
/// its worker but cannot be cloned or shared. Phase entry requires a mutable
/// borrow, so safe callers cannot concurrently use one slot twice.
pub struct Worker<'r> {
    gate: &'r Gate,
    held: &'r std::sync::atomic::AtomicBool,
    index: usize,
    owner: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl Drop for Worker<'_> {
    fn drop(&mut self) {
        self.held.store(false, std::sync::atomic::Ordering::Release);
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
            slots: (0..slots)
                .map(|_| std::sync::atomic::AtomicBool::new(false))
                .collect(),
            requests: Requests::default(),
            root_started: std::sync::atomic::AtomicBool::new(false),
            keys: Mutex::new(Vec::new()),
            output: Mutex::new(Output {
                sink,
                named: 0,
                watermark: BTreeMap::new(),
            }),
            durable,
            trace,
            metrics,
        })
    }

    /// Take a worker's slot outside any section. A second owner cannot take
    /// it until the first drops its token, including during worker retirement.
    pub fn worker(&self, index: usize) -> Option<Worker<'_>> {
        use std::sync::atomic::Ordering;
        let held = self.slots.get(index)?;
        held.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        Some(Worker {
            gate: self.arbiter.gate().unwrap(),
            held,
            index,
            owner: std::marker::PhantomData,
        })
    }

    fn check_worker(&self, worker: &Worker<'_>) {
        assert!(
            std::ptr::eq(worker.gate, self.arbiter.gate().unwrap()),
            "worker belongs to another recording"
        );
    }

    /// Register the root at admission. Child registration instead belongs in
    /// the parent's spawn section (`Writer::spawn`) so a cut cannot split it
    /// from the spawning phase.
    pub fn root(&self, worker: &mut Worker<'_>) -> Result<Task, Stopped> {
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
        worker: &mut Worker<'_>,
        task: &mut Task,
        progress: Progress,
        action: impl FnOnce(&mut Writer<'_, '_>) -> T,
    ) -> Result<T, Stopped> {
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
        let mut writer = Writer {
            section,
            task,
            progress,
            bytes: 0,
            events: 0,
        };
        let result = action(&mut writer);
        writer.section.publish(&writer.task.log, progress);
        if let Some(metrics) = &self.metrics {
            use std::sync::atomic::Ordering::Relaxed;
            metrics.bytes.fetch_add(writer.bytes, Relaxed);
            metrics.events.fetch_add(writer.events, Relaxed);
        }
        Ok(result)
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
        let Some(terminal) = self.arbiter.stop(0, &[&self.requests]) else {
            return Ok(None);
        };
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
            outcome: outcome.into(),
            task: task.map(<[u32]>::to_vec),
        };
        let bytes = tape::stop(&keys[output.named..], &tasks, &end);
        output.sink.append(&bytes)?;
        output.sink.barrier()?;
        output.named = keys.len();
        Ok(Some((
            Cut {
                progress,
                terminal: true,
            },
            terminal.sweep,
        )))
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
fn new_task(section: &Section<'_>, identity: Arc<()>, path: Vec<u32>, trace: bool) -> Task {
    let log = TaskLog::new(path);
    section.register(&log);
    Task {
        identity,
        log,
        encoder: Encoder::default(),
        last: HashMap::new(),
        epochs: HashMap::new(),
        canceled: None,
        scratch: Vec::new(),
        trace: trace.then(Vec::new),
        spawned: 0,
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
}

impl Writer<'_, '_> {
    pub fn spawn(&mut self) -> Task {
        let mut path = self.task.path().to_vec();
        path.push(self.task.spawned);
        self.task.spawned = self
            .task
            .spawned
            .checked_add(1)
            .expect("spawn path ordinal overflow");
        new_task(
            &self.section,
            self.task.identity.clone(),
            path,
            self.task.trace.is_some(),
        )
    }
    pub fn prepare<B>(&mut self, request: &Request<B>) {
        assert_eq!(self.progress.phase, Phase::Entered);
        self.record(Record::Prepared {
            op: self.progress.op,
            id: request.id,
        });
    }
    fn record(&mut self, record: Record) {
        assert_eq!(record.op(), self.progress.op);
        self.task.scratch.clear();
        self.task.encoder.encode(&record, &mut self.task.scratch);
        self.section.append(&self.task.log, &self.task.scratch);
    }
    fn event(&mut self, event: Event) {
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
        let previous = self.task.last.get(object).copied().unwrap_or(1);
        let gap = version
            .checked_sub(previous + 1)
            .expect("object versions advance in the object's section order");
        self.task.last.insert(object.clone(), version);
        if gap != 0 {
            self.record(Record::Gap {
                op: self.progress.op,
                sub,
                gap,
            });
            self.bytes += self.task.scratch.len() as u64;
            self.events += 1;
        }
        self.event(Event::Versioned {
            object: object.clone(),
            version,
            sub,
            outcome: outcome.into(),
        });
    }
    pub fn create(&mut self, object: ObjectId) {
        self.event(Event::Versioned {
            object,
            version: 1,
            sub: 0,
            outcome: "create".into(),
        });
    }
    pub fn receive(&mut self, object: ObjectId, outcome: &str) {
        self.event(Event::Received {
            object,
            outcome: outcome.into(),
        });
    }
    pub fn load(&mut self, bits: u128) {
        self.record(Record::Load {
            op: self.progress.op,
            bits,
        });
        self.event(Event::Observed(Observed::Load(bits)));
    }
    pub fn thaw(&mut self, ok: bool) {
        self.record(Record::Thaw {
            op: self.progress.op,
            ok,
        });
        self.event(Event::Observed(Observed::Thaw(ok)));
    }
    pub fn await_any(&mut self, index: u64) {
        self.record(Record::AwaitAny {
            op: self.progress.op,
            index,
        });
        self.event(Event::Observed(Observed::AwaitAny(index)));
    }
    pub fn cancel(&mut self, cause: Option<u32>) {
        if std::mem::replace(&mut self.task.canceled, cause) != cause {
            self.record(Record::Cancel {
                op: self.progress.op,
                cause,
            });
        }
        self.event(Event::Observed(Observed::Cancel(cause)));
    }
    pub fn expired(&mut self, expired: bool) {
        if expired {
            self.record(Record::Expired {
                op: self.progress.op,
            });
        }
        self.event(Event::Observed(Observed::Expired(expired)));
    }
    pub fn key(&mut self, key: u32, name: &str, epoch: u64, shared: bool, index: u32) {
        let previous = self.task.epochs.insert(key, epoch).unwrap_or(0);
        let gap = epoch
            .checked_sub(previous)
            .expect("a task's key epochs never go backwards");
        self.record(Record::Key {
            op: self.progress.op,
            key,
            shared,
            gap,
        });
        self.event(Event::Key {
            key: name.into(),
            epoch,
            index,
        });
    }
    pub fn observation(&mut self, bytes: &[u8]) {
        self.record(Record::Observation {
            op: self.progress.op,
            bytes: bytes.into(),
        });
    }
    pub fn complete<B>(&mut self, request: &Request<B>, bytes: &[u8]) {
        assert_eq!(self.progress.phase, Phase::Done);
        request.finish();
        self.record(Record::Completion {
            op: self.progress.op,
            bytes: bytes.into(),
        });
    }
    pub fn output(&mut self, sink: &str, epoch: u64, bytes: &[u8]) {
        self.event(Event::Output {
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
