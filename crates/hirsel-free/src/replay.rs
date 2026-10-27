//! The replay side of a free-mode tape (design §5 Replay protocol), backend-neutral.
//!
//! - **Validation.** [`Shared::new`] decodes every task's stream once and checks it before any
//!   task runs: every record decodes, names an op the task's cut admits and a key the tape
//!   names; key epochs are consistent across streams. A malformed tape is refused here. It
//!   keeps each stream's checked bytes and record count, not its records: a [`Cursor`] decodes
//!   records only as its admitted phases ask for them and drops each once consumed, so the
//!   decoded records alive at any time are those of the tasks' active ops.
//! - **Admission.** A task runs only up to its progress in the tape's cut (the terminal cut,
//!   or a truncated tape's last flush). The engine asks [`Cursor::admits`] before each phase
//!   and parks the task for good when it is not admitted: at (k, done) it parks once op k
//!   completes; at (k, entered) it runs op k's entry, releases included, and waits for good; a
//!   request parks in flight at its claimed, attempt or ended phase; at (k, cleanup i) it
//!   applies cleanup batches 1..i. A child absent from the cut never starts.
//! - **Enablement.** A gapped sub-event is enabled when its object's version equals p + g (p:
//!   the version this task's previous sub-event on it left, 1 for its first); a host
//!   transition when every transition of its key's previous epoch ended. A task waiting for
//!   enablement is replay-pending: it holds nothing and releases its worker
//!   ([`Admission::Pending`]).
//! - **Wakes.** A pending wait registers a [`Subscriber`] on the one dependency it waits for (an
//!   object's version cell or a key's epochs) and rechecks under the registration lock, as Tokio
//!   `sync/notify.rs:1138-1197` (ce685ee6916549ed7e52dbfd5430858bbdc734d3) registers and reloads
//!   its state. A publication advances the cell first, then takes exactly the registrations it
//!   enabled, and the engine wakes them after the phase is published: a publication visits no
//!   other dependency's registrations. A wait at its cut never registers; [`Shared::finish`]
//!   drops the registrations of waits that never became ready.
//! - **Substitution.** Loads, `await_any`, thaws, cancellation checks, `scope_until`, host
//!   observations and completions take their recorded outcomes; no host call is made.
//! - **Finish.** At the end every record is consumed, or the replay [`Diverged`]. An observed
//!   replay ([`Shared::open`]) also stamps each phase's effects and returns their
//!   [`Projection`]; an unobserved one builds no events.
//!
//! Each task's [`Cursor`] is owner-local: it moves with its task and needs no lock, so an
//! ordinary phase touches only its own cursor and the atomic version cell of the object it
//! orders against. [`Shared`] holds the immutable streams and synchronises only shared
//! administration: the object directory (creation and lookup), host-key epochs and the
//! terminal check. [`Replayer`] keys the same cursors by spawn path for width-1 engines.
//!
//! Progress over the admitted prefix holds at any width and permit count: order the admitted
//! phases by the real time of their sections in the recording; the earliest one not yet
//! replayed has its task's earlier phases replayed, and each predecessor it names acted in an
//! earlier section, which the closed cut admits, so it is enabled. An engine that finds every
//! unfinished task pending reports [`Diverged`], never a hang.

use crate::keys::KeyEpochs;
use crate::phase::Progress;
use crate::projection::{Event, Observed, Projection, Stamped};
use crate::record::{CheckIndex, Decoder, ObjectId, Record, Resume};
use crate::tape::{End, Tape};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::Waker;

mod wakes;
use wakes::register;
pub use wakes::{DependencyToken, Subscriber, Version, take_ready};

/// The replay cannot follow its tape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diverged(pub String);

impl std::fmt::Display for Diverged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "replay diverged: {}", self.0)
    }
}

/// One task's immutable recorded stream: its checked bytes, how many records they hold, and
/// its cut.
#[derive(Default)]
struct Stream {
    bytes: Box<[u8]>,
    len: usize,
    cut: Option<Progress>,
}

/// Whether a gapped sub-event can act now.
#[derive(Clone, Debug)]
pub enum Admission {
    Ready,
    /// Its predecessor has not acted: wait, holding nothing, for the token.
    Pending(DependencyToken),
}

/// What every replayed task shares: the validated streams (immutable) and the synchronised
/// administration.
pub struct Shared {
    observe: bool,
    end: Option<End>,
    cut: BTreeMap<Vec<u32>, Progress>,
    keys: Vec<String>,
    streams: HashMap<Vec<u32>, Arc<Stream>>,
    objects: RwLock<HashMap<ObjectId, Arc<Version>>>,
    epochs: Mutex<Keys>,
    /// Registrations examined by publications, so tests can show a publication visits only its
    /// own dependency's waits.
    visits: AtomicU64,
}

/// Every key's epochs and the waits registered on each key.
struct Keys {
    epochs: Vec<KeyEpochs>,
    /// Per key: pending waits and the epoch each waits to start.
    waiters: Vec<Vec<(u64, Subscriber)>>,
}

impl Shared {
    /// [`Shared::open`] an observed replay.
    pub fn new(tape: &Tape) -> Result<Arc<Shared>, Diverged> {
        Shared::open(tape, true)
    }

    /// Validate `tape` and keep its checked streams; a malformed tape is refused before any
    /// task runs. Every stream is checked from the tape's borrowed bytes before any is copied.
    /// Only an `observe`d replay keeps the events its projection is built from.
    pub fn open(tape: &Tape, observe: bool) -> Result<Arc<Shared>, Diverged> {
        let mut epochs = vec![KeyEpochs::default(); tape.keys.len()];
        let mut checked = Vec::with_capacity(tape.logs.len());
        for (path, bytes) in &tape.logs {
            let cut = tape.cut.get(path).copied();
            let mut last: HashMap<u32, u64> = HashMap::new();
            // A stream that does not decode is refused as such, before a refusal of one of
            // its records.
            let mut refused = None;
            let mut len = 0;
            for record in Decoder::new(bytes) {
                let record = record.map_err(|e| Diverged(format!("task {path:?}: {e}")))?;
                len += 1;
                if refused.is_none()
                    && let Some(cut) = cut
                {
                    refused = check(path, cut, &record, &mut last, &mut epochs).err();
                }
            }
            let cut = cut.ok_or_else(|| {
                Diverged(format!(
                    "task {path:?} has records but no progress in the cut"
                ))
            })?;
            if let Some(refusal) = refused {
                return Err(refusal);
            }
            checked.push((path, bytes, len, cut));
        }
        let mut streams = HashMap::with_capacity(tape.cut.len());
        for (path, bytes, len, cut) in checked {
            let stream = Stream {
                bytes: bytes.as_slice().into(),
                len,
                cut: Some(cut),
            };
            streams.insert(path.clone(), Arc::new(stream));
        }
        for (path, progress) in &tape.cut {
            streams.entry(path.clone()).or_insert_with(|| {
                Arc::new(Stream {
                    cut: Some(*progress),
                    ..Stream::default()
                })
            });
        }
        Ok(Arc::new(Shared {
            observe,
            end: tape.end.clone(),
            cut: tape.cut.clone(),
            keys: tape.keys.clone(),
            streams,
            objects: RwLock::new(HashMap::new()),
            epochs: Mutex::new(Keys {
                waiters: epochs.iter().map(|_| Vec::new()).collect(),
                epochs,
            }),
            visits: AtomicU64::new(0),
        }))
    }

    /// The recorded outcome and stopping task; `None` for a truncated tape.
    pub fn end(&self) -> Option<&End> {
        self.end.as_ref()
    }

    /// Every task's progress in the cut.
    pub fn cut(&self) -> &BTreeMap<Vec<u32>, Progress> {
        &self.cut
    }

    /// Whether a spawned child starts: it is in the cut, past its start.
    pub fn starts(&self, task: &[u32]) -> bool {
        self.cut.get(task).is_some_and(|cut| *cut > Progress::START)
    }

    /// The owner-local cursor of `task`. A task absent from the tape gets an empty cursor,
    /// which the cut never admits.
    pub fn cursor(self: &Arc<Shared>, task: &[u32]) -> Cursor {
        let stream = self.streams.get(task).cloned().unwrap_or_default();
        Cursor {
            shared: self.clone(),
            path: task.to_vec(),
            window: RefCell::default(),
            remaining: stream.len,
            observe: self.observe,
            stream,
            last: HashMap::new(),
            epochs: HashMap::new(),
            canceled: None,
            cancel_checks: CheckIndex::default(),
            staged: Vec::new(),
            events: Vec::new(),
            wakes: Vec::new(),
        }
    }

    /// Gates exist at admission, independently of which replayed task first reaches one.
    /// Registration does not consume an ordinal or a gap.
    pub fn gate(&self, package: &str) {
        let id = ObjectId::Gate(package.into());
        if self.objects.read().unwrap().contains_key(&id) {
            return;
        }
        self.objects
            .write()
            .unwrap()
            .entry(id)
            .or_insert_with(|| Arc::new(Version::new(1)));
    }

    /// `object`'s version cell, once created.
    pub fn object(&self, object: &ObjectId) -> Result<Arc<Version>, Diverged> {
        self.objects
            .read()
            .unwrap()
            .get(object)
            .cloned()
            .ok_or_else(|| Diverged(format!("{object} was never created")))
    }

    fn key(&self, name: &str) -> Result<u32, Diverged> {
        self.keys
            .iter()
            .position(|k| k == name)
            .map(|key| key as u32)
            .ok_or_else(|| Diverged(format!("key {name} is not on the tape")))
    }

    pub fn positions_ready(&self, positions: &[crate::recorder::Position]) -> bool {
        let keys = self.epochs.lock().unwrap();
        positions
            .iter()
            .all(|p| keys.epochs[p.key as usize].ready(p.epoch))
    }

    /// Register `subscriber` on the first of `positions` whose epoch may not start yet, checked
    /// under the registration lock: `true` means every position is ready and nothing is
    /// registered. A wait for several keys registers on one at a time.
    pub fn subscribe_positions(
        &self,
        positions: &[crate::recorder::Position],
        subscriber: Subscriber,
    ) -> bool {
        let mut keys = self.epochs.lock().unwrap();
        let Some(p) = positions
            .iter()
            .find(|p| !keys.epochs[p.key as usize].ready(p.epoch))
        else {
            return true;
        };
        register(&mut keys.waiters[p.key as usize], p.epoch, subscriber);
        false
    }

    /// End `positions`' transitions; the waits whose epochs they let start.
    pub fn end_positions(&self, positions: &[crate::recorder::Position]) -> Vec<Waker> {
        let mut keys = self.epochs.lock().unwrap();
        let Keys { epochs, waiters } = &mut *keys;
        for p in positions {
            epochs[p.key as usize].end(p.epoch);
        }
        let mut wakes = Vec::new();
        for (i, p) in positions.iter().enumerate() {
            if positions[..i].iter().any(|q| q.key == p.key) {
                continue;
            }
            let epochs = &epochs[p.key as usize];
            take_ready(
                &mut waiters[p.key as usize],
                &self.visits,
                &mut wakes,
                |epoch| epochs.ready(epoch),
            );
        }
        wakes
    }

    /// Registrations examined by publications so far.
    pub fn visits(&self) -> u64 {
        self.visits.load(Ordering::Relaxed)
    }

    /// Drop the registrations of waits that never became ready: the replay ended.
    fn end_subscriptions(&self) {
        for cell in self.objects.read().unwrap().values() {
            cell.clear();
        }
        for waiters in &mut self.epochs.lock().unwrap().waiters {
            waiters.clear();
        }
    }

    /// Finish: every cursor consumed its records and reached its cut (`cursors` holds every
    /// started task's cursor; an unstarted one has nothing to consume). An observed replay
    /// returns its projection.
    pub fn finish<'c>(
        &self,
        cursors: impl IntoIterator<Item = (&'c Cursor, Progress)>,
    ) -> Result<Option<Projection>, Diverged> {
        self.end_subscriptions();
        let mut finished = Vec::new();
        let mut reached: BTreeMap<&[u32], Progress> = BTreeMap::new();
        for (cursor, at) in cursors {
            if let Some(record) = cursor.unconsumed() {
                return Err(Diverged(format!(
                    "task {:?} left {record:?} and {} more unconsumed",
                    cursor.path,
                    cursor.remaining - 1
                )));
            }
            reached.insert(&cursor.path, at);
            finished.push(cursor);
        }
        for (path, stream) in &self.streams {
            if stream.len != 0 && !reached.contains_key(path.as_slice()) {
                return Err(Diverged(format!(
                    "task {path:?} left {} records unconsumed",
                    stream.len
                )));
            }
        }
        for (path, cut) in &self.cut {
            let at = reached
                .get(path.as_slice())
                .copied()
                .unwrap_or(Progress::START);
            if at != *cut {
                return Err(Diverged(format!(
                    "task {path:?} reached {at}, not its cut {cut}"
                )));
            }
        }
        let outcome = self.end.as_ref().map_or("truncated", |end| &end.outcome);
        Ok(self.observe.then(|| {
            let events = finished.iter().flat_map(|cursor| &cursor.events);
            Projection::new(outcome, &self.cut, events)
        }))
    }
}

/// One record of a stream, checked by [`Shared::open`].
fn check(
    path: &[u32],
    cut: Progress,
    record: &Record,
    last: &mut HashMap<u32, u64>,
    epochs: &mut [KeyEpochs],
) -> Result<(), Diverged> {
    if record.op() > cut.op {
        return Err(Diverged(format!(
            "task {path:?} records op {} past its cut {cut}",
            record.op()
        )));
    }
    if let Record::Key {
        key, shared, gap, ..
    } = *record
    {
        let epochs = epochs
            .get_mut(key as usize)
            .ok_or(Diverged(format!("unnamed key {key}")))?;
        let epoch = last
            .get(&key)
            .copied()
            .unwrap_or(0)
            .checked_add(gap)
            .ok_or_else(|| Diverged(format!("key {key} epoch overflows")))?;
        last.insert(key, epoch);
        epochs.member(epoch, shared).map_err(Diverged)?;
    }
    Ok(())
}

/// A cursor's decoded records, each with whether it was consumed: from the first unconsumed
/// one through the first of an op later than any asked for, or the stream's last. A record is
/// decoded once, when an op at or past its own is asked for, and dropped once it and every
/// record before it are consumed (at the latest by the next decode).
#[derive(Default)]
struct Window {
    /// The window is `records[first..]`; the consumed records before `first` are dropped
    /// when the window empties or decodes past them.
    records: Vec<(Record, bool)>,
    first: usize,
    /// Where decoding resumes past the window.
    next: Resume,
}

impl Window {
    /// Decode every record of ops up to `op` from the stream's checked `bytes`.
    #[inline(always)]
    fn extend(&mut self, bytes: &[u8], op: u64) {
        if self.records.last().is_none_or(|(r, _)| r.op() <= op) && self.next.offset() < bytes.len()
        {
            self.decode(bytes, op);
        }
    }

    #[inline(never)]
    fn decode(&mut self, bytes: &[u8], op: u64) {
        self.records.drain(..self.first);
        self.first = 0;
        let mut decoder = Decoder::at(bytes, self.next);
        for record in decoder.by_ref() {
            let record = record.expect("a checked stream decodes");
            let past = record.op() > op;
            self.records.push((record, false));
            if past {
                break;
            }
        }
        self.next = decoder.resume();
    }

    /// The first unconsumed record.
    fn front(&self) -> Option<&Record> {
        self.records.get(self.first).map(|(r, _)| r)
    }

    /// The unconsumed records of op `op`, by index.
    #[inline]
    fn of(&self, op: u64) -> impl Iterator<Item = (usize, &Record)> {
        let first = self.first;
        self.records[first..]
            .iter()
            .enumerate()
            .take_while(move |(_, (r, _))| r.op() <= op)
            .filter(move |(_, (r, taken))| !taken && r.op() == op)
            .map(move |(i, (r, _))| (first + i, r))
    }

    /// Consume the record at `at`.
    #[inline]
    fn consume(&mut self, at: usize) {
        self.records[at].1 = true;
        while self
            .records
            .get(self.first)
            .is_some_and(|&(_, taken)| taken)
        {
            self.first += 1;
        }
        if self.first == self.records.len() {
            self.records.clear();
            self.first = 0;
        }
    }
}

/// One task's replay: its immutable stream, what it consumed, and its own object and key
/// history. It moves with its task and is used by one owner at a time.
pub struct Cursor {
    shared: Arc<Shared>,
    path: Vec<u32>,
    stream: Arc<Stream>,
    window: RefCell<Window>,
    remaining: usize,
    /// Whether this replay keeps its effects (`staged` and `events` stay empty otherwise).
    observe: bool,
    last: HashMap<ObjectId, u64>,
    /// This task's previous epoch on each key, as its records advance.
    epochs: HashMap<u32, u64>,
    canceled: Option<u32>,
    cancel_checks: CheckIndex,
    /// The current phase's effects, stamped at its publication.
    staged: Vec<Event>,
    events: Vec<Stamped>,
    /// Waits this task's sub-events and key ends enabled, woken once its phase is published.
    wakes: Vec<Waker>,
}

impl Cursor {
    pub fn path(&self) -> &[u32] {
        &self.path
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// How many records this cursor holds decoded: its window over the stream.
    pub fn decoded(&self) -> usize {
        let window = self.window.borrow();
        window.records.len() - window.first
    }

    fn unconsumed(&self) -> Option<Record> {
        if self.remaining == 0 {
            return None;
        }
        let mut window = self.window.borrow_mut();
        // The window's first record is unconsumed.
        window.extend(&self.stream.bytes, 0);
        window.front().cloned()
    }

    /// Take the first unconsumed record of op `op` that `pick` accepts.
    fn take<T>(&mut self, op: u64, mut pick: impl FnMut(&Record) -> Option<T>) -> Option<T> {
        let window = self.window.get_mut();
        window.extend(&self.stream.bytes, op);
        let (at, value) = window.of(op).find_map(|(i, r)| pick(r).map(|v| (i, v)))?;
        window.consume(at);
        self.remaining -= 1;
        Some(value)
    }

    /// The first unconsumed record of op `op` that `pick` accepts, left unconsumed.
    fn peek<T>(&self, op: u64, pick: impl Fn(&Record) -> Option<T>) -> Option<T> {
        let mut window = self.window.borrow_mut();
        window.extend(&self.stream.bytes, op);
        window.of(op).find_map(|(_, r)| pick(r))
    }

    fn substitute<T>(
        &mut self,
        op: u64,
        what: &str,
        pick: impl FnMut(&Record) -> Option<T>,
    ) -> Result<T, Diverged> {
        self.take(op, pick)
            .ok_or_else(|| Diverged(format!("{:?} op {op}: no recorded {what}", self.path)))
    }

    /// Whether the cut admits this task's phase at `progress`.
    pub fn admits(&self, progress: Progress) -> bool {
        self.stream.cut.is_some_and(|cut| cut.admits(progress))
    }

    /// This task's progress in the cut.
    pub fn cut(&self) -> Option<Progress> {
        self.stream.cut
    }

    /// Whether this task starts: it is in the cut, past its start.
    pub fn starts(&self) -> bool {
        self.stream.cut.is_some_and(|cut| cut > Progress::START)
    }

    pub fn create(&mut self, object: ObjectId) {
        self.shared
            .objects
            .write()
            .unwrap()
            .insert(object.clone(), Arc::new(Version::new(1)));
        self.stage(|| Event::Versioned {
            object,
            version: 1,
            sub: 0,
            outcome: "create".into(),
        });
    }

    /// Stage an effect of the current phase, if this replay is observed.
    fn stage(&mut self, event: impl FnOnce() -> Event) {
        if self.observe {
            self.staged.push(event());
        }
    }

    fn required(&self, op: u64, sub: u32, object: &ObjectId) -> u64 {
        let gap = self.peek(op, |r| match *r {
            Record::Gap { sub: s, gap, .. } if s == sub => Some(gap),
            _ => None,
        });
        self.last.get(object).copied().unwrap_or(1) + gap.unwrap_or(0)
    }

    /// Whether sub-event `sub` of op `op` on `object` can act now, or the token it waits for.
    pub fn admission(&self, op: u64, sub: u32, object: &ObjectId) -> Result<Admission, Diverged> {
        let cell = self.shared.object(object)?;
        let required = self.required(op, sub, object);
        let version = cell.get();
        if version > required {
            return Err(Diverged(format!(
                "{object} is at version {version}, past the {required} that {:?} op {op} needs",
                self.path
            )));
        }
        Ok(if version == required {
            Admission::Ready
        } else {
            Admission::Pending(DependencyToken::new(cell, required))
        })
    }

    /// Whether sub-event `sub` of op `op` on `object` is enabled now.
    pub fn enabled(&self, op: u64, sub: u32, object: &ObjectId) -> Result<bool, Diverged> {
        Ok(matches!(self.admission(op, sub, object)?, Admission::Ready))
    }

    fn apply(
        &mut self,
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> Result<Option<Event>, Diverged> {
        let cell = self.shared.object(object)?;
        let required = self.required(op, sub, object);
        if !cell.advance(required, &self.shared.visits, &mut self.wakes) {
            return Err(Diverged(format!(
                "{:?} op {op} touched {object} before it was enabled",
                self.path
            )));
        }
        self.take(op, |r| {
            matches!(*r, Record::Gap { sub: s, .. } if s == sub).then_some(())
        });
        let version = required + 1;
        match self.last.get_mut(object) {
            Some(last) => *last = version,
            None => {
                self.last.insert(object.clone(), version);
            }
        }
        Ok(self.observe.then(|| Event::Versioned {
            object: object.clone(),
            version,
            sub,
            outcome: outcome.into(),
        }))
    }

    /// Apply an enabled sub-event.
    pub fn touch(
        &mut self,
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> Result<(), Diverged> {
        let event = self.apply(op, sub, object, outcome)?;
        self.staged.extend(event);
        Ok(())
    }

    /// Apply an enabled sub-event of an op that already published (op, done): an engine
    /// that cannot hold the op until it is enabled applies it later, stamped with that phase.
    pub fn touch_stamped(
        &mut self,
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> Result<(), Diverged> {
        let event = self.apply(op, sub, object, outcome)?;
        self.events.extend(event.map(|event| Stamped {
            task: self.path.clone(),
            progress: Progress::new(op, crate::phase::Phase::Done),
            event,
        }));
        Ok(())
    }

    pub fn receive(&mut self, object: &ObjectId, outcome: &str) {
        self.stage(|| Event::Received {
            object: object.clone(),
            outcome: outcome.into(),
        });
    }

    pub fn load(&mut self, op: u64) -> Result<u128, Diverged> {
        let bits = self.substitute(op, "load", |r| match *r {
            Record::Load { bits, .. } => Some(bits),
            _ => None,
        })?;
        self.stage(|| Event::Observed(Observed::Load(bits)));
        Ok(bits)
    }

    pub fn cpu_control(
        &mut self,
        op: u64,
        width: u32,
        action: u32,
        count: u64,
    ) -> Result<u32, Diverged> {
        if self.observation(op)? != crate::projection::cpu_request(width, action, count) {
            return Err(Diverged("CPU control request differs".into()));
        }
        let bits = self.substitute(op, "CPU outcome", |r| match *r {
            Record::Load { bits, .. } => Some(bits),
            _ => None,
        })?;
        let outcome =
            u32::try_from(bits).map_err(|_| Diverged("CPU outcome exceeds u32".into()))?;
        self.stage(|| {
            Event::Observed(Observed::CpuControl {
                width,
                action,
                count,
                outcome,
            })
        });
        Ok(outcome)
    }

    pub fn await_any(&mut self, op: u64) -> Result<u64, Diverged> {
        let index = self.substitute(op, "await_any member", |r| match *r {
            Record::AwaitAny { index, .. } => Some(index),
            _ => None,
        })?;
        self.stage(|| Event::Observed(Observed::AwaitAny(index)));
        Ok(index)
    }

    /// Peek `task.await_any`'s recorded member without consuming it (to wait for it).
    pub fn await_any_member(&self, op: u64) -> Option<u64> {
        self.peek(op, |r| match *r {
            Record::AwaitAny { index, .. } => Some(index),
            _ => None,
        })
    }

    /// `shared.thaw`'s outcome: `Err` is forced; `Ok` waits for count 1.
    pub fn thaw(&mut self, op: u64) -> Result<bool, Diverged> {
        let ok = self.substitute(op, "thaw", |r| match *r {
            Record::Thaw { ok, .. } => Some(ok),
            _ => None,
        })?;
        self.stage(|| Event::Observed(Observed::Thaw(ok)));
        Ok(ok)
    }

    /// Peek a thaw's outcome without consuming it (to wait for count 1 before applying it).
    pub fn thaw_outcome(&self, op: u64) -> Option<bool> {
        self.peek(op, |r| match *r {
            Record::Thaw { ok, .. } => Some(ok),
            _ => None,
        })
    }

    pub fn expired(&mut self, op: u64) -> bool {
        let expired = self
            .take(op, |r| matches!(r, Record::Expired { .. }).then_some(()))
            .is_some();
        self.stage(|| Event::Observed(Observed::Expired(expired)));
        expired
    }

    /// The cancellation an unshielded check of op `op` will observe, without consuming it.
    pub fn cancel_outcome(&self, op: u64) -> Option<u32> {
        let check = self.cancel_checks.peek(op);
        self.peek(op, |r| match *r {
            Record::Cancel {
                check: index,
                cause,
                ..
            } if index == check => Some(cause),
            _ => None,
        })
        .unwrap_or(self.canceled)
    }

    /// The cancellation an unshielded check of op `op` observes.
    pub fn cancel_check(&mut self, op: u64) -> Option<u32> {
        let check = self.cancel_checks.take(op);
        if let Some(cause) = self.take(op, |r| match *r {
            Record::Cancel {
                check: index,
                cause,
                ..
            } if index == check => Some(cause),
            _ => None,
        }) {
            self.canceled = cause;
        }
        let observed = self.canceled;
        self.stage(|| Event::Observed(Observed::Cancel(observed)));
        observed
    }

    pub fn blocked(&mut self, op: u64) -> Result<(), Diverged> {
        self.substitute(op, "blocked", |r| {
            matches!(r, Record::Blocked { .. }).then_some(())
        })
    }

    pub fn ready(&mut self, op: u64) -> Result<(), Diverged> {
        self.substitute(op, "ready", |r| {
            matches!(r, Record::Ready { .. }).then_some(())
        })
    }

    /// A host request's entered phase: its recorded request id.
    pub fn prepare(&mut self, op: u64) -> Result<u64, Diverged> {
        self.substitute(op, "Prepared", |r| match *r {
            Record::Prepared { id, .. } => Some(id),
            _ => None,
        })
    }

    /// The positions a request's issue (`index` 0) or attempt `index` takes, in key order.
    pub fn positions(
        &mut self,
        op: u64,
        index: u32,
        keys: &[(&str, bool)],
    ) -> Result<Vec<crate::recorder::Position>, Diverged> {
        let mut wanted: Vec<(u32, bool)> = Vec::new();
        for &(name, shared) in keys {
            wanted.push((self.shared.key(name)?, shared));
        }
        wanted.sort_unstable();
        let mut positions = Vec::new();
        for (key, shared) in wanted {
            let gap = self
                .take(op, |r| match *r {
                    Record::Key {
                        key: k,
                        shared: s,
                        gap,
                        ..
                    } if k == key && s == shared => Some(gap),
                    _ => None,
                })
                .ok_or_else(|| {
                    Diverged(format!(
                        "{:?} op {op}: no recorded position on key {key}",
                        self.path
                    ))
                })?;
            let epoch = self.epochs.get(&key).copied().unwrap_or(0) + gap;
            self.epochs.insert(key, epoch);
            if self.observe {
                self.staged.push(Event::Key {
                    key: self.shared.keys[key as usize].clone(),
                    epoch,
                    index,
                });
            }
            positions.push(crate::recorder::Position { key, epoch });
        }
        Ok(positions)
    }

    /// End a request's key positions; the waits they enable wake with this task's phase.
    pub fn end_positions(&mut self, positions: &[crate::recorder::Position]) {
        let wakes = self.shared.end_positions(positions);
        self.wakes.extend(wakes);
    }

    /// The waits enabled since the last call, for the engine to wake after publication.
    pub fn take_wakes(&mut self) -> Vec<Waker> {
        std::mem::take(&mut self.wakes)
    }

    pub fn output(&mut self, sink: &str, epoch: u64, bytes: &[u8]) {
        self.stage(|| Event::Output {
            sink: sink.into(),
            epoch,
            bytes: bytes.to_vec(),
        });
    }

    pub fn output_at_position(
        &mut self,
        positions: &[crate::recorder::Position],
        sink: &str,
        bytes: &[u8],
    ) -> Result<(), Diverged> {
        let position = positions
            .iter()
            .find(|p| {
                self.shared
                    .keys
                    .get(p.key as usize)
                    .is_some_and(|key| key == sink)
            })
            .ok_or_else(|| Diverged("output sink is absent from host positions".into()))?;
        self.output(sink, position.epoch, bytes);
        Ok(())
    }

    pub fn observation(&mut self, op: u64) -> Result<Vec<u8>, Diverged> {
        self.substitute(op, "host observation", |r| match r {
            Record::Observation { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
    }

    pub fn completion(&mut self, op: u64) -> Result<Vec<u8>, Diverged> {
        self.substitute(op, "completion", |r| match r {
            Record::Completion { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
    }

    /// A phase of this task replayed: stamp its effects.
    pub fn publish(&mut self, progress: Progress) -> Result<(), Diverged> {
        if !self.admits(progress) {
            return Err(Diverged(format!(
                "{:?} replayed {progress}, past its cut",
                self.path
            )));
        }
        let path = &self.path;
        self.events
            .extend(self.staged.drain(..).map(|event| Stamped {
                task: path.clone(),
                progress,
                event,
            }));
        Ok(())
    }
}

/// A replay of one tape for a width-1 engine: the same cursors, keyed by spawn path.
pub struct Replayer {
    shared: Arc<Shared>,
    cursors: HashMap<Vec<u32>, Cursor>,
}

impl Replayer {
    pub fn new(tape: &Tape) -> Result<Replayer, Diverged> {
        let shared = Shared::new(tape)?;
        let cursors = shared
            .streams
            .keys()
            .map(|path| (path.clone(), shared.cursor(path)))
            .collect();
        Ok(Replayer { shared, cursors })
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    fn cursor(&mut self, task: &[u32]) -> Result<&mut Cursor, Diverged> {
        self.cursors
            .get_mut(task)
            .ok_or_else(|| Diverged(format!("task {task:?} is not on the tape")))
    }

    fn get(&self, task: &[u32]) -> Result<&Cursor, Diverged> {
        self.cursors
            .get(task)
            .ok_or_else(|| Diverged(format!("task {task:?} is not on the tape")))
    }

    /// Whether the cut admits `task`'s phase at `progress`.
    pub fn admits(&self, task: &[u32], progress: Progress) -> bool {
        self.cursors
            .get(task)
            .is_some_and(|cursor| cursor.admits(progress))
    }

    /// `task`'s progress in the cut.
    pub fn cut(&self, task: &[u32]) -> Option<Progress> {
        self.shared.cut.get(task).copied()
    }

    /// Whether a spawned child starts: it is in the cut, past its start.
    pub fn starts(&self, task: &[u32]) -> bool {
        self.shared.starts(task)
    }

    /// The recorded outcome and stopping task; `None` for a truncated tape.
    pub fn end(&self) -> Option<&End> {
        self.shared.end()
    }

    pub fn gate(&mut self, package: &str) {
        self.shared.gate(package);
    }

    pub fn create(&mut self, task: &[u32], object: ObjectId) -> Result<(), Diverged> {
        self.cursor(task)?.create(object);
        Ok(())
    }

    /// Whether sub-event `sub` of `task`'s op `op` on `object` is enabled now.
    pub fn enabled(
        &self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
    ) -> Result<bool, Diverged> {
        self.get(task)?.enabled(op, sub, object)
    }

    /// Apply an enabled sub-event.
    pub fn touch(
        &mut self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> Result<(), Diverged> {
        self.cursor(task)?.touch(op, sub, object, outcome)
    }

    /// See [`Cursor::touch_stamped`].
    pub fn touch_stamped(
        &mut self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> Result<(), Diverged> {
        self.cursor(task)?.touch_stamped(op, sub, object, outcome)
    }

    pub fn receive(
        &mut self,
        task: &[u32],
        object: &ObjectId,
        outcome: &str,
    ) -> Result<(), Diverged> {
        self.cursor(task)?.receive(object, outcome);
        Ok(())
    }

    pub fn load(&mut self, task: &[u32], op: u64) -> Result<u128, Diverged> {
        self.cursor(task)?.load(op)
    }

    pub fn cpu_control(
        &mut self,
        task: &[u32],
        op: u64,
        width: u32,
        action: u32,
        count: u64,
    ) -> Result<u32, Diverged> {
        self.cursor(task)?.cpu_control(op, width, action, count)
    }

    pub fn await_any(&mut self, task: &[u32], op: u64) -> Result<u64, Diverged> {
        self.cursor(task)?.await_any(op)
    }

    pub fn await_any_member(&self, task: &[u32], op: u64) -> Option<u64> {
        self.cursors.get(task)?.await_any_member(op)
    }

    pub fn thaw(&mut self, task: &[u32], op: u64) -> Result<bool, Diverged> {
        self.cursor(task)?.thaw(op)
    }

    pub fn thaw_outcome(&self, task: &[u32], op: u64) -> Option<bool> {
        self.cursors.get(task)?.thaw_outcome(op)
    }

    pub fn expired(&mut self, task: &[u32], op: u64) -> Result<bool, Diverged> {
        Ok(self.cursor(task)?.expired(op))
    }

    pub fn cancel_outcome(&self, task: &[u32], op: u64) -> Option<u32> {
        self.cursors.get(task)?.cancel_outcome(op)
    }

    pub fn cancel_check(&mut self, task: &[u32], op: u64) -> Result<Option<u32>, Diverged> {
        Ok(self.cursor(task)?.cancel_check(op))
    }

    pub fn blocked(&mut self, task: &[u32], op: u64) -> Result<(), Diverged> {
        self.cursor(task)?.blocked(op)
    }

    pub fn ready(&mut self, task: &[u32], op: u64) -> Result<(), Diverged> {
        self.cursor(task)?.ready(op)
    }

    pub fn prepare(&mut self, task: &[u32], op: u64) -> Result<u64, Diverged> {
        self.cursor(task)?.prepare(op)
    }

    pub fn positions(
        &mut self,
        task: &[u32],
        op: u64,
        index: u32,
        keys: &[(&str, bool)],
    ) -> Result<Vec<crate::recorder::Position>, Diverged> {
        self.cursor(task)?.positions(op, index, keys)
    }

    pub fn positions_ready(&self, positions: &[crate::recorder::Position]) -> bool {
        self.shared.positions_ready(positions)
    }

    pub fn end_positions(&mut self, positions: &[crate::recorder::Position]) {
        // A width-1 engine polls its pending tasks and registers no waits.
        drop(self.shared.end_positions(positions));
    }

    pub fn output(
        &mut self,
        task: &[u32],
        sink: &str,
        epoch: u64,
        bytes: &[u8],
    ) -> Result<(), Diverged> {
        self.cursor(task)?.output(sink, epoch, bytes);
        Ok(())
    }

    pub fn output_at_position(
        &mut self,
        task: &[u32],
        positions: &[crate::recorder::Position],
        sink: &str,
        bytes: &[u8],
    ) -> Result<(), Diverged> {
        self.cursor(task)?
            .output_at_position(positions, sink, bytes)
    }

    pub fn observation(&mut self, task: &[u32], op: u64) -> Result<Vec<u8>, Diverged> {
        self.cursor(task)?.observation(op)
    }

    pub fn completion(&mut self, task: &[u32], op: u64) -> Result<Vec<u8>, Diverged> {
        self.cursor(task)?.completion(op)
    }

    /// A phase of `task` replayed: stamp its effects.
    pub fn publish(&mut self, task: &[u32], progress: Progress) -> Result<(), Diverged> {
        self.cursor(task)?.publish(progress)
    }

    /// Finish: every record consumed and every task at its cut. `at` is each task's progress
    /// when the replay ended.
    pub fn finish(self, at: &BTreeMap<Vec<u32>, Progress>) -> Result<Projection, Diverged> {
        let mut cursors: Vec<_> = self.cursors.values().collect();
        cursors.sort_by(|a, b| a.path.cmp(&b.path));
        let projection = self.shared.finish(cursors.into_iter().map(|cursor| {
            (
                cursor,
                at.get(&cursor.path).copied().unwrap_or(Progress::START),
            )
        }))?;
        Ok(projection.expect("a Replayer observes"))
    }
}

#[cfg(test)]
mod tests;
