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
use crate::sync::{SectionWaits, sc_load, sc_store, section_open};

/// Bytes per log segment: a published record never moves.
pub const SEGMENT: usize = 4096;

/// A task's record log and published progress.
///
/// The log is a chain of fixed segments split by who may touch what. The task's owner links
/// spare segments it reserved outside any section at the tail, and writes records there,
/// inside its sections ([`Tail`]); the cutter reads from the head and frees segments it has
/// scanned, only while the gate is closed ([`Head`]). Neither side allocates while a section
/// is open: a section writes only room reserved before it opened, and the owner's spare list
/// is never read by the cutter.
pub struct TaskLog {
    path: Vec<u32>,
    progress: AtomicU64,
    tail: Cell<Tail>,
    head: Cell<Head>,
    /// Owner only: segments reserved before a section, linked by the next section that fills
    /// its tail. Boxed: linking moves only the pointer, so a segment keeps its address.
    #[allow(clippy::vec_box)]
    spare: Cell<Vec<Box<Segment>>>,
    /// On a slot's dirty list since the last scan.
    dirty: Cell<bool>,
    /// The next task on the same slot's dirty list (an intrusive list: marking allocates
    /// nothing).
    next_dirty: Cell<Option<Arc<TaskLog>>>,
    /// The log length the last scan read up to, and whether a scan has seen the task
    /// (cutter only).
    scanned: Cell<(usize, bool)>,
}

/// One fixed segment of a log.
pub struct Segment {
    bytes: Box<[u8]>,
    next: Option<Box<Segment>>,
}

impl Segment {
    fn new() -> Box<Segment> {
        Box::new(Segment {
            bytes: vec![0; SEGMENT].into_boxed_slice(),
            next: None,
        })
    }
}

/// The writer's end of a log: written inside the owner's sections, read by the owner at any
/// time and by the cutter while the gate is closed.
struct Tail {
    /// The last linked segment, owned by the chain from [`Head`]; null before the first.
    last: *mut Segment,
    /// The committed length, from the log's start.
    len: usize,
    /// The end of the linked capacity, from the log's start.
    end: usize,
}

// SAFETY: `last` points into the chain its `TaskLog` owns; the section protocol orders every
// access to it.
unsafe impl Send for Tail {}

/// The reader's end of a log: the first segment not yet freed, and its offset.
#[derive(Default)]
struct Head {
    first: Option<Box<Segment>>,
    base: usize,
}

/// A section's direct writer into one task's log: bytes land in place, linking reserved
/// spares at the tail. It never allocates: writing past the reserved room is a broken bound.
pub struct LogOut<'a> {
    tail: &'a mut Tail,
    head: &'a mut Head,
    #[allow(clippy::vec_box)]
    spare: &'a mut Vec<Box<Segment>>,
}

impl LogOut<'_> {
    fn link(&mut self) {
        let mut segment = self
            .spare
            .pop()
            .expect("a section writes only log room reserved before it opened");
        segment.next = None;
        let last: *mut Segment = &mut *segment;
        if self.tail.last.is_null() {
            self.head.first = Some(segment);
            self.head.base = self.tail.end;
        } else {
            // SAFETY: `last` is the chain's live tail; only this section touches it.
            unsafe { (*self.tail.last).next = Some(segment) };
        }
        self.tail.last = last;
        self.tail.end += SEGMENT;
    }
    /// The bytes written so far.
    pub fn len(&self) -> usize {
        self.tail.len
    }
    pub fn is_empty(&self) -> bool {
        self.tail.len == 0
    }
    pub fn push(&mut self, byte: u8) {
        if self.tail.len == self.tail.end {
            self.link();
        }
        // SAFETY: the tail segment has room at `len % SEGMENT`; only this section reaches it.
        let segment = unsafe { &mut *self.tail.last };
        segment.bytes[self.tail.len % SEGMENT] = byte;
        self.tail.len += 1;
    }
    pub fn extend(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if self.tail.len == self.tail.end {
                self.link();
            }
            let at = self.tail.len % SEGMENT;
            let n = bytes.len().min(SEGMENT - at);
            // SAFETY: as in `push`.
            let segment = unsafe { &mut *self.tail.last };
            segment.bytes[at..at + n].copy_from_slice(&bytes[..n]);
            self.tail.len += n;
            bytes = &bytes[n..];
        }
    }
}

impl Head {
    fn read(&self, mut from: usize, to: usize, out: &mut Vec<u8>) {
        let mut segment = self.first.as_deref();
        let mut base = self.base;
        while from < to {
            let current = segment.expect("a scanned range is linked");
            if from >= base + SEGMENT {
                segment = current.next.as_deref();
                base += SEGMENT;
                continue;
            }
            let at = from - base;
            let n = (to - from).min(SEGMENT - at);
            out.extend_from_slice(&current.bytes[at..at + n]);
            from += n;
        }
    }
    /// Free every segment wholly before `through`, keeping the tail.
    fn release(&mut self, through: usize) {
        while self.base + SEGMENT <= through {
            let Some(first) = self.first.as_mut() else {
                return;
            };
            let Some(next) = first.next.take() else {
                return;
            };
            self.first = Some(next);
            self.base += SEGMENT;
        }
    }
}

impl Drop for Head {
    fn drop(&mut self) {
        let mut next = self.first.take();
        while let Some(mut segment) = next {
            next = segment.next.take();
        }
    }
}

impl TaskLog {
    pub fn new(path: Vec<u32>) -> Arc<TaskLog> {
        Arc::new(TaskLog {
            path,
            progress: AtomicU64::new(Progress::START.pack()),
            tail: Cell::new(Tail {
                last: std::ptr::null_mut(),
                len: 0,
                end: 0,
            }),
            head: Cell::new(Head::default()),
            spare: Cell::new(Vec::new()),
            dirty: Cell::new(false),
            next_dirty: Cell::new(None),
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
    /// The record bytes a section can write without allocating: the tail's room and the
    /// reserved spares.
    ///
    /// # Safety
    /// The caller owns this task's writing side.
    pub unsafe fn room(&self) -> usize {
        // SAFETY: the owner alone writes the tail and the spares; the cutter only reads the tail.
        unsafe { self.tail.with(|tail| tail.end - tail.len) + self.spare.with(Vec::len) * SEGMENT }
    }
    /// Make room for `bytes` more record bytes before a section, so the section links spares
    /// instead of allocating. Returns how many segments this allocated.
    ///
    /// # Safety
    /// The caller owns this task's writing side (it alone opens sections that write this
    /// log) and holds no open section.
    pub unsafe fn reserve(&self, bytes: usize) -> usize {
        // SAFETY: the owner alone writes the tail, and not now; the cutter only reads it.
        let room = unsafe { self.tail.with(|tail| tail.end - tail.len) };
        // SAFETY: owner only.
        unsafe {
            self.spare.with_mut(|spare| {
                let have = room + spare.len() * SEGMENT;
                let want = bytes.saturating_sub(have).div_ceil(SEGMENT);
                spare.reserve(want);
                for _ in 0..want {
                    spare.push(Segment::new());
                }
                want
            })
        }
    }
}

/// A thread's slot: one per compute worker, home thread and pool thread.
struct Slot {
    active: AtomicU32,
    /// The most recently marked task; each links the one marked before it.
    dirty: Cell<Option<Arc<TaskLog>>>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        // SAFETY: dropping the gate: nothing else reaches the slot.
        let mut next = unsafe { self.dirty.with_mut(Option::take) };
        while let Some(task) = next {
            // SAFETY: as above; a task on the list is reached only through it now.
            next = unsafe { task.next_dirty.with_mut(Option::take) };
        }
    }
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
                    dirty: Cell::new(None),
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

    /// Whether no section is open on `slot`: its retiring owner's acknowledgement.
    pub fn inactive(&self, slot: usize) -> bool {
        sc_load(&self.slots[slot].active) == 0
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
                section_open(true);
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
                let list = &self.gate.slots[self.slot].dirty;
                let previous = list.with_mut(Option::take);
                task.next_dirty.with_mut(|next| *next = previous);
                list.with_mut(|head| *head = Some(task.clone()));
            }
        }
    }
    /// A spawned task's registration, in its parent's section.
    pub fn register(&self, task: &Arc<TaskLog>) {
        self.mark(task);
    }
    /// Append record bytes to the log of the task this section runs, within the room its owner
    /// reserved before the section ([`TaskLog::reserve`]).
    pub fn append(&self, task: &Arc<TaskLog>, bytes: &[u8]) {
        self.write(task, |out| out.extend(bytes));
    }
    /// Write records directly into the log of the task this section runs: into the room its
    /// owner reserved before the section, with no intermediate copy.
    pub fn write<R>(&self, task: &Arc<TaskLog>, f: impl FnOnce(&mut LogOut<'_>) -> R) -> R {
        // SAFETY: as in `mark`; the owner's spare list is the section's own.
        let result = unsafe {
            task.tail.with_mut(|tail| {
                task.head.with_mut(|head| {
                    task.spare
                        .with_mut(|spare| f(&mut LogOut { tail, head, spare }))
                })
            })
        };
        self.mark(task);
        result
    }
    /// Publish the task's progress (release): the phase's action and records are done.
    pub fn publish(&self, task: &Arc<TaskLog>, progress: Progress) {
        task.progress.store(progress.pack(), Ordering::Release);
        self.mark(task);
    }
}

impl Drop for Section<'_> {
    fn drop(&mut self) {
        section_open(false);
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
            let mut dirty = Vec::new();
            let mut next = unsafe { slot.dirty.with_mut(Option::take) };
            while let Some(task) = next {
                // SAFETY: as above.
                next = unsafe { task.next_dirty.with_mut(Option::take) };
                dirty.push(task);
            }
            for task in dirty.into_iter().rev() {
                // SAFETY: as above.
                let (seen, scanned) = unsafe {
                    task.dirty.with_mut(|dirty| *dirty = false);
                    let (from, seen) = task.scanned.with(|scanned| *scanned);
                    let mut bytes = Vec::new();
                    let to = task.tail.with(|tail| tail.len);
                    task.head.with_mut(|head| {
                        head.read(from, to, &mut bytes);
                        head.release(to);
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
