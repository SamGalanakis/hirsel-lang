//! The recording side of a free-mode run: what an engine calls at each op.
//!
//! An engine stages a phase's effects (creations, object sub-events, observed outcomes, host
//! positions) as the op runs, then calls [`Recorder::publish`] for the phase: one section
//! appends the staged records to the task's log and publishes its progress. Gaps come from
//! per-object versions and each task's last version on the object. A flush cuts, writes a
//! flush chunk and makes it durable, which advances the causal durable watermark; the stop
//! cuts for good, sweeps unclaimed requests and writes the stop chunk. Every staged effect is
//! also a stamped [`Event`], from which [`Recorder::projection`] builds the live projection at
//! any cut the tape names.
//!
//! The recorder serialises its callers (`&mut self`): it is the width-1 executor's, and its
//! gate, logs and claim words are the concurrent protocol F1 runs at width N.

use crate::claim::{Arbiter, Request, Requests, Sweep};
use crate::keys::KeyEpochs;
use crate::phase::{Phase, Progress};
use crate::projection::{Event, Observed, Projection, Stamped};
use crate::record::{CheckIndex, Encoder, ObjectId, Record};
use crate::section::{Scanned, Stopped, TaskLog};
use crate::sync::Arc;
use crate::tape::{self, End, TaskChunk};
use std::collections::{BTreeMap, HashMap};
use std::io;

/// Where tape bytes go.
pub trait TapeSink {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Make everything appended durable.
    fn barrier(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl TapeSink for Vec<u8> {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// A position a request holds on a host key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    pub key: u32,
    pub epoch: u64,
}

struct Live {
    log: Arc<TaskLog>,
    encoder: Encoder,
    staged: Vec<u8>,
    events: Vec<Event>,
    /// The version this task's previous sub-event left on each object.
    last: HashMap<ObjectId, u64>,
    /// This task's previous epoch on each key.
    epochs: HashMap<u32, u64>,
    /// The cancellation its previous unshielded check observed.
    canceled: Option<u32>,
    cancel_checks: CheckIndex,
}

/// One recording run at width 1 (slot 0).
pub struct Recorder<S: TapeSink> {
    arbiter: Arbiter,
    requests: Requests<()>,
    sink: S,
    durable: bool,
    tasks: HashMap<Vec<u32>, Live>,
    objects: HashMap<ObjectId, u64>,
    keys: Vec<(String, KeyEpochs)>,
    key_index: HashMap<String, u32>,
    /// Keys not yet named in a chunk.
    unnamed: usize,
    /// The durable watermark: each task's progress at the last durable cut.
    watermark: HashMap<Vec<u32>, Progress>,
    /// Every cut written, in order: flushes, then the stop.
    cuts: Vec<BTreeMap<Vec<u32>, Progress>>,
    events: Vec<Stamped>,
    end: Option<End>,
}

impl<S: TapeSink> Recorder<S> {
    /// A recording into `sink`, bound to the engine's `binding`; `durable` makes every
    /// `Prepared` durable before its claim and every completion before its delivery.
    pub fn new(mut sink: S, binding: &[u8], durable: bool) -> io::Result<Recorder<S>> {
        sink.append(&tape::header(binding))?;
        if durable {
            sink.barrier()?;
        }
        Ok(Recorder {
            arbiter: Arbiter::new(true, 1),
            requests: Requests::default(),
            sink,
            durable,
            tasks: HashMap::new(),
            objects: HashMap::new(),
            keys: Vec::new(),
            key_index: HashMap::new(),
            unnamed: 0,
            watermark: HashMap::new(),
            cuts: Vec::new(),
            events: Vec::new(),
            end: None,
        })
    }

    fn live(&mut self, task: &[u32]) -> &mut Live {
        self.tasks
            .get_mut(task)
            .unwrap_or_else(|| panic!("task {task:?} was never started"))
    }

    fn stage(&mut self, task: &[u32], record: Option<Record>, event: Event) {
        let live = self.live(task);
        if let Some(record) = record {
            live.encoder.encode(&record, &mut live.staged);
        }
        live.events.push(event);
    }

    /// Register a task: the root (no parent), or a child in its parent's spawn section.
    pub fn start(&mut self, child: Vec<u32>) -> Result<(), Stopped> {
        let log = TaskLog::new(&child);
        let gate = self.arbiter.gate().expect("a recording has a gate");
        gate.enter(0)?.register(&log);
        self.tasks.insert(
            child,
            Live {
                log,
                encoder: Encoder::default(),
                staged: Vec::new(),
                events: Vec::new(),
                last: HashMap::new(),
                epochs: HashMap::new(),
                canceled: None,
                cancel_checks: CheckIndex::default(),
            },
        );
        Ok(())
    }

    /// A serial package gate exists from run admission, at version 1. It has
    /// no creating task or op; lazy registration must not emit a create event.
    pub fn gate(&mut self, package: &str) {
        self.objects
            .entry(ObjectId::Gate(package.into()))
            .or_insert(1);
    }

    /// Op `op` of `task` created `object` (version 1).
    pub fn create(&mut self, task: &[u32], object: ObjectId) {
        self.objects.insert(object.clone(), 1);
        self.stage(
            task,
            None,
            Event::Versioned {
                object,
                version: 1,
                sub: 0,
                outcome: "create".into(),
            },
        );
    }

    /// Sub-event `sub` of op `op` touched shared `object`: its gap.
    pub fn touch(
        &mut self,
        task: &[u32],
        op: u64,
        sub: u32,
        object: &ObjectId,
        outcome: &str,
    ) -> u64 {
        let version = *self
            .objects
            .get(object)
            .unwrap_or_else(|| panic!("{object} was never created"));
        let live = self.live(task);
        let previous = live.last.get(object).copied().unwrap_or(1);
        let gap = version - previous;
        live.last.insert(object.clone(), version + 1);
        self.objects.insert(object.clone(), version + 1);
        let record = (gap > 0).then_some(Record::Gap { op, sub, gap });
        let event = Event::Versioned {
            object: object.clone(),
            version: version + 1,
            sub,
            outcome: outcome.into(),
        };
        self.stage(task, record, event);
        gap
    }

    /// A receive on `object`: no record, no version.
    pub fn receive(&mut self, task: &[u32], object: &ObjectId, outcome: &str) {
        self.stage(
            task,
            None,
            Event::Received {
                object: object.clone(),
                outcome: outcome.into(),
            },
        );
    }

    pub fn load(&mut self, task: &[u32], op: u64, bits: u128) {
        self.stage(
            task,
            Some(Record::Load { op, bits }),
            Event::Observed(Observed::Load(bits)),
        );
    }

    /// CPU control retains the existing observation/load wire representation.
    pub fn cpu_control(
        &mut self,
        task: &[u32],
        op: u64,
        width: u32,
        action: u32,
        count: u64,
        outcome: u32,
    ) {
        self.observation(
            task,
            op,
            &crate::projection::cpu_request(width, action, count),
        );
        self.stage(
            task,
            Some(Record::Load {
                op,
                bits: u128::from(outcome),
            }),
            Event::Observed(Observed::CpuControl {
                width,
                action,
                count,
                outcome,
            }),
        );
    }

    pub fn await_any(&mut self, task: &[u32], op: u64, index: u64) {
        self.stage(
            task,
            Some(Record::AwaitAny { op, index }),
            Event::Observed(Observed::AwaitAny(index)),
        );
    }

    pub fn thaw(&mut self, task: &[u32], op: u64, ok: bool) {
        self.stage(
            task,
            Some(Record::Thaw { op, ok }),
            Event::Observed(Observed::Thaw(ok)),
        );
    }

    /// A `scope_until` outcome: only an expiry is recorded.
    pub fn expired(&mut self, task: &[u32], op: u64, expired: bool) {
        let record = expired.then_some(Record::Expired { op });
        self.stage(task, record, Event::Observed(Observed::Expired(expired)));
    }

    /// An unshielded cancellation check: only a change of the observed value is recorded.
    pub fn cancel_check(&mut self, task: &[u32], op: u64, observed: Option<u32>) {
        let live = self.live(task);
        let check = live.cancel_checks.take(op);
        let changed = std::mem::replace(&mut live.canceled, observed) != observed;
        let record = changed.then_some(Record::Cancel {
            op,
            check,
            cause: observed,
        });
        self.stage(task, record, Event::Observed(Observed::Cancel(observed)));
    }

    pub fn blocked(&mut self, task: &[u32], op: u64) {
        let live = self.live(task);
        live.encoder
            .encode(&Record::Blocked { op }, &mut live.staged);
    }

    pub fn ready(&mut self, task: &[u32], op: u64) {
        let live = self.live(task);
        live.encoder.encode(&Record::Ready { op }, &mut live.staged);
    }

    fn key(&mut self, name: &str) -> u32 {
        if let Some(&index) = self.key_index.get(name) {
            return index;
        }
        let index = self.keys.len() as u32;
        self.keys.push((name.into(), KeyEpochs::default()));
        self.key_index.insert(name.into(), index);
        index
    }

    /// Take one position per key, in key order: a request's issue (`index` 0, in its entered
    /// section) or attempt `index`.
    pub fn positions(
        &mut self,
        task: &[u32],
        op: u64,
        index: u32,
        keys: &[(&str, bool)],
    ) -> Vec<Position> {
        let mut keys: Vec<(u32, bool)> = keys
            .iter()
            .map(|&(name, shared)| (self.key(name), shared))
            .collect();
        keys.sort_unstable();
        keys.iter()
            .map(|&(key, shared)| {
                let epoch = self.keys[key as usize].1.take(shared);
                let live = self.live(task);
                let gap = epoch - live.epochs.insert(key, epoch).unwrap_or(0);
                let name = self.keys[key as usize].0.clone();
                self.stage(
                    task,
                    Some(Record::Key {
                        op,
                        key,
                        shared,
                        gap,
                    }),
                    Event::Key {
                        key: name,
                        epoch,
                        index,
                    },
                );
                Position { key, epoch }
            })
            .collect()
    }

    /// Whether every position's dependencies have ended, so its host call may run.
    pub fn positions_ready(&self, positions: &[Position]) -> bool {
        positions
            .iter()
            .all(|p| self.keys[p.key as usize].1.ready(p.epoch))
    }

    /// The bounded host call of these positions returned or registered its wait.
    pub fn end_positions(&mut self, positions: &[Position]) {
        for p in positions {
            self.keys[p.key as usize].1.end(p.epoch);
        }
    }

    /// A host request's entered phase: `Prepared` as request `id`.
    pub fn prepare(&mut self, task: &[u32], op: u64, id: u64) -> Arc<Request<()>> {
        let live = self.live(task);
        live.encoder
            .encode(&Record::Prepared { op, id }, &mut live.staged);
        self.requests.prepare(id, ())
    }

    /// The claimed section: whether this request may call the adapter. It publishes
    /// (op, claimed) when the claim wins.
    pub fn claim(&mut self, task: &[u32], op: u64, request: &Request<()>) -> Result<bool, Stopped> {
        let live = self.tasks.get(task).expect("a started task");
        let gate = self.arbiter.gate().expect("a recording has a gate");
        let section = gate.enter(0)?;
        if !request.claim() {
            return Ok(false);
        }
        section.publish(&live.log, Progress::new(op, Phase::Claimed));
        Ok(true)
    }

    /// Bytes the claimed host call wrote to `sink` in its exclusive epoch on the sink's key.
    pub fn output(&mut self, task: &[u32], sink: &str, epoch: u64, bytes: &[u8]) {
        self.stage(
            task,
            None,
            Event::Output {
                sink: sink.into(),
                epoch,
                bytes: bytes.to_vec(),
            },
        );
    }

    pub fn observation(&mut self, task: &[u32], op: u64, bytes: &[u8]) {
        let live = self.live(task);
        live.encoder.encode(
            &Record::Observation {
                op,
                bytes: bytes.to_vec(),
            },
            &mut live.staged,
        );
    }

    /// The done section's completion; the request is finished when the section publishes.
    pub fn complete(
        &mut self,
        task: &[u32],
        op: u64,
        request: &Request<()>,
        bytes: &[u8],
    ) -> Result<(), Stopped> {
        let live = self.live(task);
        live.encoder.encode(
            &Record::Completion {
                op,
                bytes: bytes.to_vec(),
            },
            &mut live.staged,
        );
        self.publish_with(task, Progress::new(op, Phase::Done), || request.finish())?;
        self.requests.retire(request.id);
        Ok(())
    }

    /// Close the section of `task`'s phase: append its staged records, publish `progress`.
    /// After the stop the section cannot open; the staged effects are dropped (quarantined).
    pub fn publish(&mut self, task: &[u32], progress: Progress) -> Result<(), Stopped> {
        self.publish_with(task, progress, || ())
    }

    fn publish_with(
        &mut self,
        task: &[u32],
        progress: Progress,
        act: impl FnOnce(),
    ) -> Result<(), Stopped> {
        let gate = self.arbiter.gate().expect("a recording has a gate");
        let live = self.tasks.get_mut(task).expect("a started task");
        // SAFETY: this width-1 recorder alone writes the task's log, outside any section.
        unsafe { live.log.reserve(live.staged.len()) };
        let section = match gate.enter(0) {
            Ok(section) => section,
            Err(stopped) => {
                live.staged.clear();
                live.events.clear();
                return Err(stopped);
            }
        };
        act();
        section.append(&live.log, &live.staged);
        section.publish(&live.log, progress);
        drop(section);
        live.staged.clear();
        for event in live.events.drain(..) {
            self.events.push(Stamped {
                task: task.to_vec(),
                progress,
                event,
            });
        }
        Ok(())
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

    /// The keys first named since the previous chunk.
    fn new_keys(keys: &[(String, KeyEpochs)], unnamed: &mut usize) -> Vec<String> {
        let names = keys[*unnamed..]
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        *unnamed = keys.len();
        names
    }

    /// Cut, write a flush chunk and make it durable: the watermark becomes that cut.
    pub fn flush(&mut self) -> io::Result<()> {
        let gate = self.arbiter.gate().expect("a recording has a gate");
        let Some(mut cut) = gate.flush_cut() else {
            return Ok(());
        };
        let scanned = cut.scan();
        let progress = cut.progress();
        drop(cut);
        let keys = Self::new_keys(&self.keys, &mut self.unnamed);
        self.sink
            .append(&tape::flush(&keys, &Self::chunks(scanned)))?;
        self.sink.barrier()?;
        self.watermark = progress.iter().cloned().collect();
        self.cuts.push(progress.into_iter().collect());
        Ok(())
    }

    /// Wait (by flushing) until `task`'s `progress` is under the durable watermark. A no-op
    /// for a run that is not durable.
    pub fn durable_through(&mut self, task: &[u32], progress: Progress) -> io::Result<()> {
        if self.durable && !self.watermark.get(task).is_some_and(|w| w.admits(progress)) {
            self.flush()?;
        }
        Ok(())
    }

    /// Stop the run: the terminal cut, the request sweep and the stop chunk. `None` when the
    /// run already stopped.
    pub fn stop(&mut self, outcome: &str, task: Option<&[u32]>) -> io::Result<Option<Sweep>> {
        let Some(terminal) = self.arbiter.stop(0, &[&self.requests]) else {
            return Ok(None);
        };
        let sweep = terminal.sweep;
        let mut cut = terminal.cut.expect("a recording cuts");
        let scanned = cut.scan();
        let progress = cut.progress();
        drop(cut);
        let mut bytes: HashMap<Vec<u32>, Vec<u8>> =
            scanned.into_iter().map(|s| (s.path, s.bytes)).collect();
        let chunks: Vec<TaskChunk> = progress
            .iter()
            .map(|(path, progress)| TaskChunk {
                path: path.clone(),
                progress: *progress,
                bytes: bytes.remove(path).unwrap_or_default(),
            })
            .collect();
        let end = End {
            outcome: outcome.into(),
            task: task.map(<[u32]>::to_vec),
        };
        let keys = Self::new_keys(&self.keys, &mut self.unnamed);
        self.sink.append(&tape::stop(&keys, &chunks, &end))?;
        self.sink.barrier()?;
        self.cuts.push(progress.into_iter().collect());
        self.end = Some(end);
        Ok(Some(sweep))
    }

    /// Every cut the tape holds, flushes first and the terminal cut last.
    pub fn cuts(&self) -> &[BTreeMap<Vec<u32>, Progress>] {
        &self.cuts
    }

    /// The live projection at `cut`.
    pub fn projection(&self, outcome: &str, cut: &BTreeMap<Vec<u32>, Progress>) -> Projection {
        Projection::new(outcome, cut, &self.events)
    }

    /// The live projection at the terminal cut.
    pub fn final_projection(&self) -> Projection {
        let end = self.end.as_ref().expect("the run stopped");
        self.projection(&end.outcome, self.cuts.last().expect("a terminal cut"))
    }

    pub fn into_sink(self) -> S {
        self.sink
    }
}
