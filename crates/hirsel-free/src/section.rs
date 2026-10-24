//! Op sections and the cutter (design §5 Op sections and the cut).
//!
//! Each phase of an op runs in one **section** on its thread's slot: a bounded run of runtime
//! instructions with no wait, syscall, extern call or user code. Inside it the phase acts,
//! writes its records into the task's log and publishes the task's progress. A **cutter** (the
//! stop winner or the journal flusher, one at a time, a stop first) closes the gate and waits
//! until no section is open, so what it reads is a causally closed cut:
//!
//! ```text
//! section:  active := 1 (SC); if cut (SC) != 0 { active := 0; wait for cut == 0, or abandon
//!           if stopped; retry }  ... act, record, publish ...  active := 0 (release)
//! cutter:   cut := 1 (SC); for each slot: wait while active (SC) == 1; scan; then a flush
//!           reopens (cut := 0) and a stop sets stopped and leaves the cut closed
//! ```
//!
//! The `active`/`cut` pair is a Dekker handshake on SC accesses: a section either sees the cut
//! and backs off, or the cutter sees it active and waits for its release store. A phase's
//! causal predecessors acted in earlier sections, which therefore published before any cut
//! that includes it. Registration (a spawn) and a task's end are sections too, so the set of
//! tasks is stable during a scan, and a task touched since the last cut is on its slot's dirty
//! list, so a scan costs O(slots + dirty tasks).
//!
//! The logs, dirty lists and scan positions are [`Cell`]s: sections of one slot are sequential,
//! a task runs on one slot at a time, and the cutter reads only while no section is open. The
//! loom model in `proofs/free` checks exactly that exclusion.

use crate::phase::Progress;
use crate::sync::{Arc, AtomicBool, AtomicU32, AtomicU64, Cell, Mutex, MutexGuard, Ordering};
use crate::sync::{SectionWaits, sc_load, sc_store};

/// Bytes per log segment: a published record never moves.
pub const SEGMENT: usize = 4096;

/// A task's record log and published progress.
pub struct TaskLog {
    path: Vec<u32>,
    progress: AtomicU64,
    log: Cell<Segments>,
    /// On a slot's dirty list since the last scan.
    dirty: Cell<bool>,
    /// The log length the last scan read up to, and whether a scan has seen the task
    /// (cutter only).
    scanned: Cell<(usize, bool)>,
}

/// Chained fixed segments.
#[derive(Default)]
struct Segments {
    segments: Vec<Box<[u8]>>,
    len: usize,
}

impl Segments {
    fn append(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let at = self.len % SEGMENT;
            if self.len / SEGMENT == self.segments.len() {
                self.segments.push(vec![0; SEGMENT].into_boxed_slice());
            }
            let n = bytes.len().min(SEGMENT - at);
            self.segments[self.len / SEGMENT][at..at + n].copy_from_slice(&bytes[..n]);
            self.len += n;
            bytes = &bytes[n..];
        }
    }
    fn read(&self, mut from: usize, to: usize, out: &mut Vec<u8>) {
        while from < to {
            let at = from % SEGMENT;
            let n = (to - from).min(SEGMENT - at);
            out.extend_from_slice(&self.segments[from / SEGMENT][at..at + n]);
            from += n;
        }
    }
}

impl TaskLog {
    pub fn new(path: Vec<u32>) -> Arc<TaskLog> {
        Arc::new(TaskLog {
            path,
            progress: AtomicU64::new(Progress::START.pack()),
            log: Cell::new(Segments::default()),
            dirty: Cell::new(false),
            scanned: Cell::new((0, false)),
        })
    }
    pub fn path(&self) -> &[u32] {
        &self.path
    }
    /// The latest progress published (acquire).
    pub fn progress(&self) -> Progress {
        Progress::unpack(self.progress.load(Ordering::Acquire)).expect("a packed progress word")
    }
}

/// A thread's slot: one per compute worker, home thread and pool thread.
struct Slot {
    active: AtomicU32,
    dirty: Cell<Vec<Arc<TaskLog>>>,
}

/// The run is stopped: a section cannot open again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stopped;

/// The section gate of one recording run.
pub struct Gate {
    cut: AtomicU32,
    waits: SectionWaits,
    stopped: AtomicBool,
    /// A stop wants the cutter role: flushes give way.
    stopping: AtomicBool,
    slots: Box<[Slot]>,
    /// The cutter role, and every task the scans have seen, in first-seen order.
    role: Mutex<Vec<Arc<TaskLog>>>,
}

impl Gate {
    pub fn new(slots: usize) -> Gate {
        Gate {
            cut: AtomicU32::new(0),
            waits: SectionWaits::new(),
            stopped: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            slots: (0..slots)
                .map(|_| Slot {
                    active: AtomicU32::new(0),
                    dirty: Cell::new(Vec::new()),
                })
                .collect(),
            role: Mutex::new(Vec::new()),
        }
    }

    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    /// Open a section on `slot`, waiting out a flush's cut. The caller is the only thread
    /// using `slot` and is not already in a section on it.
    pub fn enter(&self, slot: usize) -> Result<Section<'_>, Stopped> {
        self.enter_counted(slot, None)
    }

    /// Optionally count only time spent waiting for a closed cut. Ordinary
    /// sections read no clock and perform no metric atomic operation.
    pub fn enter_counted(
        &self,
        slot: usize,
        pause_ns: Option<&AtomicU64>,
    ) -> Result<Section<'_>, Stopped> {
        let active = &self.slots[slot].active;
        loop {
            sc_store(active, 1);
            if sc_load(&self.cut) == 0 {
                return Ok(Section { gate: self, slot });
            }
            active.store(0, Ordering::Release);
            self.waits.wake(active);
            let waiting = pause_ns.map(|_| std::time::Instant::now());
            loop {
                if self.stopped.load(Ordering::Acquire) {
                    add_pause(pause_ns, waiting);
                    return Err(Stopped);
                }
                if sc_load(&self.cut) == 0 {
                    break;
                }
                self.waits.wait(&self.cut, Some(&self.stopped));
            }
            add_pause(pause_ns, waiting);
        }
    }

    /// Take the cutter role for a flush and close the gate; `None` once a stop wants the role
    /// or has taken it. The caller holds no open section.
    pub fn flush_cut(&self) -> Option<Cut<'_>> {
        if self.stopping.load(Ordering::Acquire) {
            return None;
        }
        let known = self.role.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return None;
        }
        Some(self.close(known, false))
    }

    /// Take the cutter role for the run's stop and close the gate for good; `None` when a stop
    /// already took it. The caller holds no open section.
    pub fn stop_cut(&self) -> Option<Cut<'_>> {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return None;
        }
        let known = self.role.lock().unwrap();
        Some(self.close(known, true))
    }

    fn close<'g>(&'g self, known: MutexGuard<'g, Vec<Arc<TaskLog>>>, stop: bool) -> Cut<'g> {
        sc_store(&self.cut, 1);
        for slot in self.slots.iter() {
            while sc_load(&slot.active) == 1 {
                self.waits.wait(&slot.active, None);
            }
        }
        Cut {
            gate: self,
            known,
            stop,
        }
    }
}

fn add_pause(counter: Option<&AtomicU64>, started: Option<std::time::Instant>) {
    if let (Some(counter), Some(started)) = (counter, started) {
        let ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        counter.fetch_add(ns, Ordering::Relaxed);
    }
}

/// An open section. Dropping it closes it.
pub struct Section<'g> {
    gate: &'g Gate,
    slot: usize,
}

impl Section<'_> {
    fn mark(&self, task: &Arc<TaskLog>) {
        // SAFETY: this section is open, so no cutter scans; `task` runs only here.
        unsafe {
            if !task.dirty.with_mut(|dirty| std::mem::replace(dirty, true)) {
                self.gate.slots[self.slot]
                    .dirty
                    .with_mut(|list| list.push(task.clone()));
            }
        }
    }
    /// A spawned task's registration, in its parent's section.
    pub fn register(&self, task: &Arc<TaskLog>) {
        self.mark(task);
    }
    /// Append record bytes to the log of the task this section runs.
    pub fn append(&self, task: &Arc<TaskLog>, bytes: &[u8]) {
        // SAFETY: as in `mark`.
        unsafe { task.log.with_mut(|log| log.append(bytes)) };
        self.mark(task);
    }
    /// Publish the task's progress (release): the phase's action and records are done.
    pub fn publish(&self, task: &Arc<TaskLog>, progress: Progress) {
        task.progress.store(progress.pack(), Ordering::Release);
        self.mark(task);
    }
}

impl Drop for Section<'_> {
    fn drop(&mut self) {
        self.gate.slots[self.slot]
            .active
            .store(0, Ordering::Release);
        self.gate.waits.wake(&self.gate.slots[self.slot].active);
    }
}

/// What a scan read of one task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scanned {
    pub path: Vec<u32>,
    pub progress: Progress,
    /// The log bytes appended since the previous scan.
    pub bytes: Vec<u8>,
}

/// A closed gate: no section is open while it lives. Dropping it reopens a flush's cut; a
/// stop's stays closed and the run is stopped.
pub struct Cut<'g> {
    gate: &'g Gate,
    known: MutexGuard<'g, Vec<Arc<TaskLog>>>,
    stop: bool,
}

impl Cut<'_> {
    pub fn is_stop(&self) -> bool {
        self.stop
    }

    /// Every task touched since the previous scan, in first-touched order per slot, with its
    /// progress and its new log bytes.
    pub fn scan(&mut self) -> Vec<Scanned> {
        let mut out = Vec::new();
        for slot in self.gate.slots.iter() {
            // SAFETY: the gate is closed and every slot was seen inactive after the close, so
            // no section touches a dirty list, a log or a dirty flag until the cut ends.
            let dirty = unsafe { slot.dirty.with_mut(std::mem::take) };
            for task in dirty {
                // SAFETY: as above.
                let (seen, scanned) = unsafe {
                    task.dirty.with_mut(|dirty| *dirty = false);
                    let (from, seen) = task.scanned.with(|scanned| *scanned);
                    let mut bytes = Vec::new();
                    let to = task.log.with(|log| {
                        log.read(from, log.len, &mut bytes);
                        log.len
                    });
                    task.scanned.with_mut(|scanned| *scanned = (to, true));
                    (
                        seen,
                        Scanned {
                            path: task.path.clone(),
                            progress: task.progress(),
                            bytes,
                        },
                    )
                };
                if !seen {
                    self.known.push(task);
                }
                out.push(scanned);
            }
        }
        out
    }

    /// Every task any scan has seen, with its progress now: a terminal cut's progress list.
    /// Call it after [`scan`](Self::scan).
    pub fn progress(&self) -> Vec<(Vec<u32>, Progress)> {
        self.known
            .iter()
            .map(|task| (task.path.clone(), task.progress()))
            .collect()
    }
}

impl Drop for Cut<'_> {
    fn drop(&mut self) {
        if self.stop {
            self.gate.stopped.store(true, Ordering::Release);
            self.gate.waits.wake(&self.gate.cut);
        } else {
            sc_store(&self.gate.cut, 0);
            self.gate.waits.wake(&self.gate.cut);
        }
    }
}
