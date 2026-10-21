//! The replay side of a free-mode tape (design §5 Replay protocol), backend-neutral.
//!
//! - **Admission.** A task runs only up to its progress in the tape's cut (the terminal cut,
//!   or a truncated tape's last flush). The engine asks [`Replayer::admits`] before each phase
//!   and parks the task for good when it is not admitted: at (k, done) it parks once op k
//!   completes; at (k, entered) it runs op k's entry, releases included, and waits for good; a
//!   request parks in flight at its claimed, attempt or ended phase; at (k, cleanup i) it
//!   applies cleanup batches 1..i. A child absent from the cut never starts.
//! - **Enablement.** A gapped sub-event is enabled when its object's version equals p + g (p:
//!   the version this task's previous sub-event on it left, 1 for its first); a host
//!   transition when every transition of its key's previous epoch ended. A task waiting for
//!   enablement is replay-pending: it holds nothing and releases its worker.
//! - **Substitution.** Loads, `await_any`, thaws, cancellation checks, `scope_until`, host
//!   observations and completions take their recorded outcomes; no host call is made.
//! - **Validation.** At the end every record is consumed, or the replay [`Diverged`].
//!
//! Progress over the admitted prefix holds at any width and permit count: order the admitted
//! phases by the real time of their sections in the recording; the earliest one not yet
//! replayed has its task's earlier phases replayed, and each predecessor it names acted in an
//! earlier section, which the closed cut admits, so it is enabled.

use crate::keys::KeyEpochs;
use crate::phase::Progress;
use crate::projection::{Event, Observed, Projection, Stamped};
use crate::record::{ObjectId, Record, decode};
use crate::tape::{End, Tape};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The replay cannot follow its tape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diverged(pub String);

impl std::fmt::Display for Diverged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "replay diverged: {}", self.0)
    }
}

#[derive(Default)]
struct Script {
    cut: Option<Progress>,
    records: VecDeque<Record>,
    last: HashMap<ObjectId, u64>,
    /// This task's previous epoch on each key, as its records advance.
    epochs: HashMap<u32, u64>,
    canceled: Option<u32>,
    events: Vec<Event>,
}

impl Script {
    /// Take the first unconsumed record of op `op` that `pick` accepts.
    fn take<T>(&mut self, op: u64, mut pick: impl FnMut(&Record) -> Option<T>) -> Option<T> {
        let at = self
            .records
            .iter()
            .take_while(|r| r.op() <= op)
            .position(|r| r.op() == op && pick(r).is_some())?;
        let record = self.records.remove(at).unwrap();
        pick(&record)
    }
    fn peek<T>(&self, op: u64, pick: impl Fn(&Record) -> Option<T>) -> Option<T> {
        self.records
            .iter()
            .take_while(|r| r.op() <= op)
            .filter(|r| r.op() == op)
            .find_map(pick)
    }
}

/// A replay of one tape.
pub struct Replayer {
    end: Option<End>,
    cut: BTreeMap<Vec<u32>, Progress>,
    keys: Vec<String>,
    scripts: HashMap<Vec<u32>, Script>,
    objects: HashMap<ObjectId, u64>,
    epochs: Vec<KeyEpochs>,
    events: Vec<Stamped>,
}

impl Replayer {
    pub fn new(tape: &Tape) -> Result<Replayer, Diverged> {
        let mut epochs = vec![KeyEpochs::default(); tape.keys.len()];
        let mut scripts = HashMap::new();
        for (path, bytes) in &tape.logs {
            let records = decode(bytes).map_err(|e| Diverged(format!("task {path:?}: {e}")))?;
            let mut last: HashMap<u32, u64> = HashMap::new();
            for record in &records {
                if let Record::Key {
                    key, shared, gap, ..
                } = *record
                {
                    let epochs = epochs
                        .get_mut(key as usize)
                        .ok_or(Diverged(format!("unnamed key {key}")))?;
                    let epoch = last.get(&key).copied().unwrap_or(0) + gap;
                    last.insert(key, epoch);
                    epochs.member(epoch, shared).map_err(Diverged)?;
                }
            }
            let script = Script {
                records: records.into(),
                ..Script::default()
            };
            scripts.insert(path.clone(), script);
        }
        for (path, progress) in &tape.cut {
            scripts.entry(path.clone()).or_default().cut = Some(*progress);
        }
        Ok(Replayer {
            end: tape.end.clone(),
            cut: tape.cut.clone(),
            keys: tape.keys.clone(),
            scripts,
            objects: HashMap::new(),
            epochs,
            events: Vec::new(),
        })
    }

    fn script(&mut self, task: &[u32]) -> Result<&mut Script, Diverged> {
        self.scripts
            .get_mut(task)
            .ok_or_else(|| Diverged(format!("task {task:?} is not on the tape")))
    }

    /// Whether the cut admits `task`'s phase at `progress`.
    pub fn admits(&self, task: &[u32], progress: Progress) -> bool {
        self.cut.get(task).is_some_and(|cut| cut.admits(progress))
    }

    /// `task`'s progress in the cut.
    pub fn cut(&self, task: &[u32]) -> Option<Progress> {
        self.cut.get(task).copied()
    }

    /// Whether a spawned child starts: it is in the cut, past its start.
    pub fn starts(&self, task: &[u32]) -> bool {
        self.cut.get(task).is_some_and(|cut| *cut > Progress::START)
    }

    /// The recorded outcome and stopping task; `None` for a truncated tape.
    pub fn end(&self) -> Option<&End> {
        self.end.as_ref()
    }

    /// Gates exist at admission, independently of which replayed task first
    /// reaches one. Registration does not consume an ordinal or a gap.
    pub fn gate(&mut self, package: &str) {
        self.objects
            .entry(ObjectId::Gate(package.into()))
            .or_insert(1);
    }

    pub fn create(&mut self, task: &[u32], object: ObjectId) -> Result<(), Diverged> {
        self.objects.insert(object.clone(), 1);
        self.script(task)?.events.push(Event::Versioned {
            object,
            version: 1,
            sub: 0,
            outcome: "create".into(),
        });
        Ok(())
    }

    fn required(
        &self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
    ) -> Result<u64, Diverged> {
        let script = self
            .scripts
            .get(task)
            .ok_or_else(|| Diverged(format!("task {task:?} is not on the tape")))?;
        let gap = script.peek(op, |r| match *r {
            Record::Gap { sub: s, gap, .. } if s == sub => Some(gap),
            _ => None,
        });
        Ok(script.last.get(object).copied().unwrap_or(1) + gap.unwrap_or(0))
    }

    /// Whether sub-event `sub` of `task`'s op `op` on `object` is enabled now.
    pub fn enabled(
        &self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
    ) -> Result<bool, Diverged> {
        let version = *self
            .objects
            .get(object)
            .ok_or_else(|| Diverged(format!("{object} was never created")))?;
        let required = self.required(task, op, sub, object)?;
        if version > required {
            return Err(Diverged(format!(
                "{object} is at version {version}, past the {required} that {task:?} op {op} needs"
            )));
        }
        Ok(version == required)
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
        let event = self.apply(task, op, sub, object, outcome)?;
        self.script(task)?.events.push(event);
        Ok(())
    }

    /// Apply an enabled sub-event of an op that already published (op, done): an engine
    /// that cannot hold the op until it is enabled applies it later, stamped with that phase.
    pub fn touch_stamped(
        &mut self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> Result<(), Diverged> {
        let event = self.apply(task, op, sub, object, outcome)?;
        self.events.push(Stamped {
            task: task.to_vec(),
            progress: Progress::new(op, crate::phase::Phase::Done),
            event,
        });
        Ok(())
    }

    fn apply(
        &mut self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> Result<Event, Diverged> {
        if !self.enabled(task, op, sub, object)? {
            return Err(Diverged(format!(
                "{task:?} op {op} touched {object} before it was enabled"
            )));
        }
        let version = self.objects[object] + 1;
        self.objects.insert(object.clone(), version);
        let script = self.script(task)?;
        script.take(op, |r| {
            matches!(*r, Record::Gap { sub: s, .. } if s == sub).then_some(())
        });
        script.last.insert(object.clone(), version);
        Ok(Event::Versioned {
            object: object.clone(),
            version,
            sub,
            outcome: outcome.into(),
        })
    }

    pub fn receive(
        &mut self,
        task: &[u32],
        object: &ObjectId,
        outcome: &str,
    ) -> Result<(), Diverged> {
        self.script(task)?.events.push(Event::Received {
            object: object.clone(),
            outcome: outcome.into(),
        });
        Ok(())
    }

    fn substitute<T>(
        &mut self,
        task: &[u32],
        op: u64,
        what: &str,
        pick: impl FnMut(&Record) -> Option<T>,
    ) -> Result<T, Diverged> {
        self.script(task)?
            .take(op, pick)
            .ok_or_else(|| Diverged(format!("{task:?} op {op}: no recorded {what}")))
    }

    pub fn load(&mut self, task: &[u32], op: u64) -> Result<u128, Diverged> {
        let bits = self.substitute(task, op, "load", |r| match *r {
            Record::Load { bits, .. } => Some(bits),
            _ => None,
        })?;
        self.script(task)?
            .events
            .push(Event::Observed(Observed::Load(bits)));
        Ok(bits)
    }

    pub fn await_any(&mut self, task: &[u32], op: u64) -> Result<u64, Diverged> {
        let index = self.substitute(task, op, "await_any member", |r| match *r {
            Record::AwaitAny { index, .. } => Some(index),
            _ => None,
        })?;
        self.script(task)?
            .events
            .push(Event::Observed(Observed::AwaitAny(index)));
        Ok(index)
    }

    /// Peek ``task.await_any``'s recorded member without consuming it (to wait for it).
    pub fn await_any_member(&self, task: &[u32], op: u64) -> Option<u64> {
        self.scripts.get(task)?.peek(op, |r| match *r {
            Record::AwaitAny { index, .. } => Some(index),
            _ => None,
        })
    }

    /// `shared.thaw`'s outcome: `Err` is forced; `Ok` waits for count 1.
    pub fn thaw(&mut self, task: &[u32], op: u64) -> Result<bool, Diverged> {
        let ok = self.substitute(task, op, "thaw", |r| match *r {
            Record::Thaw { ok, .. } => Some(ok),
            _ => None,
        })?;
        self.script(task)?
            .events
            .push(Event::Observed(Observed::Thaw(ok)));
        Ok(ok)
    }

    /// Peek a thaw's outcome without consuming it (to wait for count 1 before applying it).
    pub fn thaw_outcome(&self, task: &[u32], op: u64) -> Option<bool> {
        self.scripts.get(task)?.peek(op, |r| match *r {
            Record::Thaw { ok, .. } => Some(ok),
            _ => None,
        })
    }

    pub fn expired(&mut self, task: &[u32], op: u64) -> Result<bool, Diverged> {
        let script = self.script(task)?;
        let expired = script
            .take(op, |r| matches!(r, Record::Expired { .. }).then_some(()))
            .is_some();
        script
            .events
            .push(Event::Observed(Observed::Expired(expired)));
        Ok(expired)
    }

    /// The cancellation an unshielded check of op `op` will observe, without consuming it.
    pub fn cancel_outcome(&self, task: &[u32], op: u64) -> Option<u32> {
        let script = self.scripts.get(task)?;
        script
            .peek(op, |r| match *r {
                Record::Cancel { cause, .. } => Some(cause),
                _ => None,
            })
            .unwrap_or(script.canceled)
    }

    /// The cancellation an unshielded check of op `op` observes.
    pub fn cancel_check(&mut self, task: &[u32], op: u64) -> Result<Option<u32>, Diverged> {
        let script = self.script(task)?;
        if let Some(cause) = script.take(op, |r| match *r {
            Record::Cancel { cause, .. } => Some(cause),
            _ => None,
        }) {
            script.canceled = cause;
        }
        let observed = script.canceled;
        script
            .events
            .push(Event::Observed(Observed::Cancel(observed)));
        Ok(observed)
    }

    pub fn blocked(&mut self, task: &[u32], op: u64) -> Result<(), Diverged> {
        self.substitute(task, op, "blocked", |r| {
            matches!(r, Record::Blocked { .. }).then_some(())
        })
    }

    pub fn ready(&mut self, task: &[u32], op: u64) -> Result<(), Diverged> {
        self.substitute(task, op, "ready", |r| {
            matches!(r, Record::Ready { .. }).then_some(())
        })
    }

    /// A host request's entered phase: its recorded request id.
    pub fn prepare(&mut self, task: &[u32], op: u64) -> Result<u64, Diverged> {
        self.substitute(task, op, "Prepared", |r| match *r {
            Record::Prepared { id, .. } => Some(id),
            _ => None,
        })
    }

    /// The positions a request's issue (`index` 0) or attempt `index` takes, in key order.
    pub fn positions(
        &mut self,
        task: &[u32],
        op: u64,
        index: u32,
        keys: &[(&str, bool)],
    ) -> Result<Vec<crate::recorder::Position>, Diverged> {
        let mut wanted: Vec<(u32, bool)> = Vec::new();
        for &(name, shared) in keys {
            let key = self
                .keys
                .iter()
                .position(|k| k == name)
                .ok_or_else(|| Diverged(format!("key {name} is not on the tape")))?;
            wanted.push((key as u32, shared));
        }
        wanted.sort_unstable();
        let mut positions = Vec::new();
        for (key, shared) in wanted {
            let script = self.script(task)?;
            let gap = script
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
                        "{task:?} op {op}: no recorded position on key {key}"
                    ))
                })?;
            let epoch = script.epochs.get(&key).copied().unwrap_or(0) + gap;
            script.epochs.insert(key, epoch);
            let name = self.keys[key as usize].clone();
            self.script(task)?.events.push(Event::Key {
                key: name,
                epoch,
                index,
            });
            positions.push(crate::recorder::Position { key, epoch });
        }
        Ok(positions)
    }

    pub fn positions_ready(&self, positions: &[crate::recorder::Position]) -> bool {
        positions
            .iter()
            .all(|p| self.epochs[p.key as usize].ready(p.epoch))
    }

    pub fn end_positions(&mut self, positions: &[crate::recorder::Position]) {
        for p in positions {
            self.epochs[p.key as usize].end(p.epoch);
        }
    }

    pub fn output(
        &mut self,
        task: &[u32],
        sink: &str,
        epoch: u64,
        bytes: &[u8],
    ) -> Result<(), Diverged> {
        self.script(task)?.events.push(Event::Output {
            sink: sink.into(),
            epoch,
            bytes: bytes.to_vec(),
        });
        Ok(())
    }

    pub fn observation(&mut self, task: &[u32], op: u64) -> Result<Vec<u8>, Diverged> {
        self.substitute(task, op, "host observation", |r| match r {
            Record::Observation { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
    }

    pub fn completion(&mut self, task: &[u32], op: u64) -> Result<Vec<u8>, Diverged> {
        self.substitute(task, op, "completion", |r| match r {
            Record::Completion { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
    }

    /// A phase of `task` replayed: stamp its effects.
    pub fn publish(&mut self, task: &[u32], progress: Progress) -> Result<(), Diverged> {
        if !self.admits(task, progress) {
            return Err(Diverged(format!(
                "{task:?} replayed {progress}, past its cut"
            )));
        }
        let events = std::mem::take(&mut self.script(task)?.events);
        self.events.extend(events.into_iter().map(|event| Stamped {
            task: task.to_vec(),
            progress,
            event,
        }));
        Ok(())
    }

    /// Finish: every record consumed and every task at its cut. `at` is each task's progress
    /// when the replay ended.
    pub fn finish(self, at: &BTreeMap<Vec<u32>, Progress>) -> Result<Projection, Diverged> {
        for (path, script) in &self.scripts {
            if let Some(record) = script.records.front() {
                return Err(Diverged(format!(
                    "task {path:?} left {record:?} and {} more unconsumed",
                    script.records.len() - 1
                )));
            }
        }
        for (path, cut) in &self.cut {
            let reached = at.get(path).copied().unwrap_or(Progress::START);
            if reached != *cut {
                return Err(Diverged(format!(
                    "task {path:?} reached {reached}, not its cut {cut}"
                )));
            }
        }
        let outcome = self.end.as_ref().map_or("truncated", |end| &end.outcome);
        Ok(Projection::new(outcome, &self.cut, &self.events))
    }
}
