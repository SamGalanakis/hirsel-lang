//! The engine-facing side of a free-mode run at width 1: one [`Session`] per run, which the
//! interpreter's task library and the runtime's scheduler drive at each op.
//!
//! A session either records (a [`Recorder`] into memory, with seeded injected flushes) or
//! replays a tape (a [`Replayer`]). It keeps each task's op ordinal and published progress,
//! maps the engine's object handles to [`ObjectId`]s, answers admission and enablement, and
//! runs a host write's phases. Pending `Shared` releases stay the engine's ([`Pending`]),
//! since only the engine knows its roots.
//!
//! Engines call it in this order for every op of a task:
//!
//! 1. [`Session::begin`] (the next ordinal), then [`Session::admits`] for the op's first phase;
//!    a phase the cut does not admit parks the task for good;
//! 2. in replay, [`Session::enabled`] (or the outcome peeks) until the phase can act; the task
//!    waits replay-pending meanwhile;
//! 3. the action, the staged effects (`create`, `touch`, `load`, ...), then
//!    [`Session::publish`], after which [`Session::at_cut`] parks a replayed task for good.
//!
//! [`Pending`]: crate::pending::Pending

use crate::phase::{Phase, Progress};
use crate::projection::Projection;
use crate::record::ObjectId;
use crate::recorder::{Position, Recorder, TapeSink};
use crate::replay::{Diverged, Replayer};
use crate::tape;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// A tape held in memory, with the offset of each barrier (the end of each durable chunk).
#[derive(Default)]
pub struct Marked {
    pub bytes: Vec<u8>,
    pub marks: Vec<usize>,
}

impl TapeSink for Marked {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn barrier(&mut self) -> std::io::Result<()> {
        self.marks.push(self.bytes.len());
        Ok(())
    }
}

/// How a recording session runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Recording {
    /// Every `Prepared` durable before its claim, every completion before its delivery.
    pub durable: bool,
    /// Flush after a publish with probability 1/n (0: never), drawn from `seed`.
    pub flush_one_in: u64,
    pub seed: u64,
}

/// An engine's object kinds, so handles of different kinds never collide.
pub type Handle = (u8, u64);

enum Side {
    Live(Box<Recorder<Sink>>),
    Replay(Box<Replayer>),
}

// Production streams directly to its admitted destination. Only the test channel
// retains bytes and barrier offsets for rendering every intermediate cut.
enum Sink {
    Memory(Marked),
    Stream(Box<dyn TapeSink + Send>),
}
impl TapeSink for Sink {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Memory(sink) => sink.append(bytes),
            Self::Stream(sink) => sink.append(bytes),
        }
    }
    fn barrier(&mut self) -> std::io::Result<()> {
        match self {
            Self::Memory(sink) => sink.barrier(),
            Self::Stream(sink) => sink.barrier(),
        }
    }
}

#[derive(Clone, Copy)]
struct Task {
    op: u64,
    progress: Progress,
}

/// A host write in flight: its request, positions and next step.
pub struct HostWrite {
    op: u64,
    live: Option<Arc<crate::claim::Request<()>>>,
    positions: Vec<Position>,
}

/// What [`Session::host_claim`] decided.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Claim {
    /// Claimed: make the host call (live), then [`Session::host_end`].
    Call,
    /// It waits for its key's earlier epochs to end (replay-pending).
    Waiting,
    /// The cut admits no further phase of it, or the stop withdrew it: the task parks for
    /// good, in flight.
    Final,
}

/// What [`Session::host_end`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Advance {
    /// The write is done; its op published (k, done).
    Done,
    /// The cut admits no further phase of it: the task parks for good, in flight.
    Final,
}

/// What a live session leaves: the tape, each barrier's offset, and the live projection at
/// every cut the tape names (each flush, then the terminal cut).
pub struct Report {
    pub tape: Vec<u8>,
    pub cuts: Vec<(usize, Projection)>,
}

impl Report {
    /// One line per cut: `cut <tape offset> <projection>`; the terminal cut is last.
    pub fn render(&self) -> String {
        self.cuts
            .iter()
            .map(|(offset, projection)| format!("cut {offset} {projection:?}\n"))
            .collect()
    }
}

/// One run's free-mode session.
pub struct Session {
    side: Side,
    tasks: HashMap<Vec<u32>, Task>,
    objects: HashMap<Handle, ObjectId>,
    recording: Recording,
    rng: u64,
    requests: u64,
    stopped: bool,
}

impl Session {
    /// A recording bound to `binding`.
    pub fn record(binding: &[u8], recording: Recording) -> Session {
        let recorder = Recorder::new(Sink::Memory(Marked::default()), binding, recording.durable)
            .expect("an in-memory tape");
        Session::new(Side::Live(Box::new(recorder)), recording)
    }

    /// Stream a production recording to an already admitted destination.
    pub fn record_to(
        binding: &[u8],
        recording: Recording,
        sink: Box<dyn TapeSink + Send>,
    ) -> std::io::Result<Session> {
        let recorder = Recorder::new(Sink::Stream(sink), binding, recording.durable)?;
        Ok(Session::new(Side::Live(Box::new(recorder)), recording))
    }

    /// A replay of `tape`.
    pub fn replay(tape: &[u8]) -> Result<Session, Diverged> {
        let tape = tape::parse(tape).map_err(Diverged)?;
        let replayer = Replayer::new(&tape)?;
        Ok(Session::new(
            Side::Replay(Box::new(replayer)),
            Recording::default(),
        ))
    }

    fn new(side: Side, recording: Recording) -> Session {
        Session {
            side,
            tasks: HashMap::new(),
            objects: HashMap::new(),
            recording,
            rng: recording.seed,
            requests: 0,
            stopped: false,
        }
    }

    pub fn is_replay(&self) -> bool {
        matches!(self.side, Side::Replay(_))
    }

    /// Whether the live run stopped: no section opens again.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    fn word(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn task(&self, path: &[u32]) -> Task {
        *self
            .tasks
            .get(path)
            .unwrap_or_else(|| panic!("free-mode task {path:?} was never started"))
    }

    /// Register a task, the root first; a child in its parent's spawn op. Whether it runs:
    /// a replay never starts a child its cut does not hold.
    pub fn start(&mut self, path: &[u32]) -> bool {
        let starts = match &mut self.side {
            Side::Live(recorder) => recorder.start(path.to_vec()).is_ok(),
            Side::Replay(replayer) => replayer.starts(path),
        };
        self.tasks.insert(
            path.to_vec(),
            Task {
                op: 0,
                progress: Progress::START,
            },
        );
        starts
    }

    /// Begin the task's next op: its ordinal.
    pub fn begin(&mut self, path: &[u32]) -> u64 {
        let task = self.tasks.get_mut(path).expect("a started task");
        task.op += 1;
        task.op
    }

    /// The ordinal of the task's current op.
    pub fn op(&self, path: &[u32]) -> u64 {
        self.task(path).op
    }

    /// The task's published progress.
    pub fn progress(&self, path: &[u32]) -> Progress {
        self.task(path).progress
    }

    /// Whether the cut admits `phase` of the task's current op (always, live).
    pub fn admits(&self, path: &[u32], phase: Phase) -> bool {
        match &self.side {
            Side::Live(_) => !self.stopped,
            Side::Replay(replayer) => replayer.admits(path, Progress::new(self.op(path), phase)),
        }
    }

    /// Whether a replayed task reached its cut: it parks for good before the next segment.
    pub fn at_cut(&self, path: &[u32]) -> bool {
        match &self.side {
            Side::Live(_) => false,
            Side::Replay(replayer) => {
                !replayer.starts(path) || replayer.cut(path) == Some(self.task(path).progress)
            }
        }
    }

    /// Whether every replayed task other than `except` is at its cut (or never starts).
    pub fn others_at_cut(&self, except: &[u32]) -> bool {
        match &self.side {
            Side::Live(_) => true,
            Side::Replay(replayer) => self.tasks.iter().all(|(path, task)| {
                path == except
                    || !replayer.starts(path)
                    || replayer.cut(path) == Some(task.progress)
            }),
        }
    }

    /// Close the section of `phase` of the task's current op.
    pub fn publish(&mut self, path: &[u32], phase: Phase) -> Result<(), Diverged> {
        let progress = Progress::new(self.op(path), phase);
        match &mut self.side {
            Side::Live(recorder) => {
                if self.stopped || recorder.publish(path, progress).is_err() {
                    return Ok(());
                }
            }
            Side::Replay(replayer) => replayer.publish(path, progress)?,
        }
        self.tasks.get_mut(path).unwrap().progress = progress;
        self.maybe_flush()
    }

    fn maybe_flush(&mut self) -> Result<(), Diverged> {
        let n = self.recording.flush_one_in;
        if n > 0
            && self.word().is_multiple_of(n)
            && let Side::Live(recorder) = &mut self.side
        {
            recorder.flush().map_err(|e| Diverged(e.to_string()))?;
        }
        Ok(())
    }

    fn object(&self, handle: Handle) -> ObjectId {
        self.objects
            .get(&handle)
            .unwrap_or_else(|| panic!("free-mode object {handle:?} was never created"))
            .clone()
    }

    /// Bind an engine's gate handle to its stable package identity. Unlike
    /// ordinary objects, gates exist before any task runs (version 1).
    pub fn gate(&mut self, handle: Handle, package: &str) {
        let id = ObjectId::Gate(package.into());
        if let Some(previous) = self.objects.insert(handle, id.clone()) {
            assert_eq!(previous, id, "a gate handle changed package identity");
        }
        match &mut self.side {
            Side::Live(recorder) => recorder.gate(package),
            Side::Replay(replayer) => replayer.gate(package),
        }
    }

    /// The task's current op created `handle`'s object (version 1).
    pub fn create(&mut self, path: &[u32], handle: Handle) -> Result<(), Diverged> {
        let id = ObjectId::Made {
            path: path.to_vec(),
            op: self.op(path),
        };
        self.objects.insert(handle, id.clone());
        match &mut self.side {
            Side::Live(recorder) => recorder.create(path, id),
            Side::Replay(replayer) => replayer.create(path, id)?,
        }
        Ok(())
    }

    /// Whether sub-event `sub` of the task's current op on `handle` is enabled (always, live).
    pub fn enabled(&self, path: &[u32], sub: u32, handle: Handle) -> Result<bool, Diverged> {
        match &self.side {
            Side::Live(_) => Ok(true),
            Side::Replay(replayer) => {
                replayer.enabled(path, self.op(path), sub, &self.object(handle))
            }
        }
    }

    /// An enabled sub-event on `handle`'s object, with its outcome.
    pub fn touch(
        &mut self,
        path: &[u32],
        sub: u32,
        handle: Handle,
        outcome: &str,
    ) -> Result<(), Diverged> {
        let (op, id) = (self.op(path), self.object(handle));
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.touch(path, op, sub, &id, outcome);
            }
            Side::Replay(replayer) => replayer.touch(path, op, sub, &id, outcome)?,
        }
        Ok(())
    }

    /// Replay: whether sub-event `sub` of the task's earlier op `op` on `handle` is enabled.
    /// An engine whose op cannot wait (an endpoint closed by a drop) applies it later, when
    /// it is, with [`Session::touch_at`].
    pub fn enabled_at(
        &self,
        path: &[u32],
        op: u64,
        sub: u32,
        handle: Handle,
    ) -> Result<bool, Diverged> {
        match &self.side {
            Side::Live(_) => Ok(true),
            Side::Replay(replayer) => replayer.enabled(path, op, sub, &self.object(handle)),
        }
    }

    /// Replay: apply sub-event `sub` of the task's earlier op `op`, which published (op,
    /// done) when it ran, now that it is enabled.
    pub fn touch_at(
        &mut self,
        path: &[u32],
        op: u64,
        sub: u32,
        handle: Handle,
        outcome: &str,
    ) -> Result<(), Diverged> {
        let id = self.object(handle);
        match &mut self.side {
            Side::Live(_) => panic!("a live sub-event applies in its own section"),
            Side::Replay(replayer) => replayer.touch_stamped(path, op, sub, &id, outcome),
        }
    }

    /// A receive on `handle`'s channel: no record, no version.
    pub fn receive(&mut self, path: &[u32], handle: Handle, outcome: &str) -> Result<(), Diverged> {
        let id = self.object(handle);
        match &mut self.side {
            Side::Live(recorder) => recorder.receive(path, &id, outcome),
            Side::Replay(replayer) => replayer.receive(path, &id, outcome)?,
        }
        Ok(())
    }

    /// An atomic load: the live value recorded, or the recorded value substituted.
    pub fn load(&mut self, path: &[u32], live: u128) -> Result<u128, Diverged> {
        let op = self.op(path);
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.load(path, op, live);
                Ok(live)
            }
            Side::Replay(replayer) => replayer.load(path, op),
        }
    }

    /// The admitted CPU request and its scalar outcome, without host placement.
    pub fn cpu_control(
        &mut self,
        path: &[u32],
        width: u32,
        action: u32,
        count: u64,
        live: u32,
    ) -> Result<u32, Diverged> {
        let op = self.op(path);
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.cpu_control(path, op, width, action, count, live);
                Ok(live)
            }
            Side::Replay(replayer) => replayer.cpu_control(path, op, width, action, count),
        }
    }

    /// A deadline scope's exit outcome: record only Expired, or substitute it.
    pub fn expired(&mut self, path: &[u32], live: bool) -> Result<bool, Diverged> {
        let op = self.op(path);
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.expired(path, op, live);
                Ok(live)
            }
            Side::Replay(replayer) => replayer.expired(path, op),
        }
    }

    /// Whether a thaw can act now: a replayed `Ok` waits for count 1.
    pub fn thaw_ready(&self, path: &[u32], last: bool) -> bool {
        match &self.side {
            Side::Live(_) => true,
            Side::Replay(replayer) => {
                replayer.thaw_outcome(path, self.op(path)) != Some(true) || last
            }
        }
    }

    /// `shared.thaw`'s outcome: live, whether the root is the last; replayed, the bit.
    pub fn thaw(&mut self, path: &[u32], last: bool) -> Result<bool, Diverged> {
        let op = self.op(path);
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.thaw(path, op, last);
                Ok(last)
            }
            Side::Replay(replayer) => {
                let ok = replayer.thaw(path, op)?;
                if ok && !last {
                    return Err(Diverged(format!(
                        "{path:?} op {op}: a recorded Ok thaw of a root that is not the last"
                    )));
                }
                Ok(ok)
            }
        }
    }

    /// `task.await_any`'s member: live, the first completed one; replayed, the recorded one,
    /// once `done` (whether a member index completed) holds for it.
    pub fn await_any(
        &mut self,
        path: &[u32],
        live: Option<u64>,
        done: impl Fn(u64) -> bool,
    ) -> Result<Option<u64>, Diverged> {
        let op = self.op(path);
        match &mut self.side {
            Side::Live(recorder) => {
                if let Some(index) = live {
                    recorder.await_any(path, op, index);
                }
                Ok(live)
            }
            Side::Replay(replayer) => match replayer.await_any_member(path, op) {
                Some(index) if done(index) => replayer.await_any(path, op).map(Some),
                Some(_) => Ok(None),
                None => Err(Diverged(format!(
                    "{path:?} op {op}: no recorded await_any member"
                ))),
            },
        }
    }

    /// Replay: the recorded `await_any` member of the task's current op, which its wait waits
    /// for. Live: `None`.
    pub fn await_any_member(&self, path: &[u32]) -> Option<u64> {
        match &self.side {
            Side::Live(_) => None,
            Side::Replay(replayer) => replayer.await_any_member(path, self.op(path)),
        }
    }

    /// Replay: the cancellation the current op's check will observe, without consuming it
    /// (a wait that a cancellation ends is ready when this is `Some`). Live: `live`.
    pub fn cancel_peek(&self, path: &[u32], live: Option<u32>) -> Option<u32> {
        match &self.side {
            Side::Live(_) => live,
            Side::Replay(replayer) => replayer.cancel_outcome(path, self.op(path)),
        }
    }

    /// An unshielded cancellation check: live, the observed cause; replayed, forced.
    pub fn cancel_check(
        &mut self,
        path: &[u32],
        live: Option<u32>,
    ) -> Result<Option<u32>, Diverged> {
        let op = self.op(path);
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.cancel_check(path, op, live);
                Ok(live)
            }
            Side::Replay(replayer) => replayer.cancel_check(path, op),
        }
    }

    /// Native replay consumes a host transition using the same epoch decoder
    /// as host_write. The engine owns phases, waits and typed payloads.
    /// Index zero consumes Prepared; later indices keep the current ordinal.
    pub fn replay_host_positions(
        &mut self,
        path: &[u32],
        index: u32,
        keys: &[(&str, bool)],
    ) -> Result<Vec<Position>, Diverged> {
        let op = self.op(path);
        let Side::Replay(replayer) = &mut self.side else {
            return Err(Diverged(
                "native host replay requires a replay session".into(),
            ));
        };
        if index == 0 {
            replayer.prepare(path, op)?;
        }
        replayer.positions(path, op, index, keys)
    }

    /// Compare the engine's canonical request before its claim can replay.
    /// The engine appends this observation in Entered after key positions.
    pub fn replay_host_request(&mut self, path: &[u32], expected: &[u8]) -> Result<(), Diverged> {
        let op = self.op(path);
        let Side::Replay(replayer) = &mut self.side else {
            return Err(Diverged("host request needs replay".into()));
        };
        if replayer.observation(path, op)? != expected {
            return Err(Diverged(format!(
                "host request differs at {path:?} operation {op}"
            )));
        }
        Ok(())
    }

    /// Check without advancing or retaining a session borrow across a save.
    pub fn replay_host_ready(&self, positions: &[Position]) -> bool {
        let Side::Replay(replayer) = &self.side else {
            return false;
        };
        replayer.positions_ready(positions)
    }

    /// An admitted Ended phase substitutes the bounded-call observation and
    /// ends its key positions before later transitions become enabled.
    pub fn replay_host_observation(
        &mut self,
        path: &[u32],
        positions: &[Position],
    ) -> Result<Vec<u8>, Diverged> {
        let op = self.op(path);
        let Side::Replay(replayer) = &mut self.side else {
            return Err(Diverged(
                "native host replay requires a replay session".into(),
            ));
        };
        let bytes = replayer.observation(path, op)?;
        replayer.end_positions(positions);
        Ok(bytes)
    }

    /// Reconstruct external output from an admitted completion's written prefix.
    pub fn replay_host_output(
        &mut self,
        path: &[u32],
        positions: &[Position],
        sink: &str,
        bytes: &[u8],
    ) -> Result<(), Diverged> {
        let Side::Replay(replayer) = &mut self.side else {
            return Err(Diverged("native output requires replay".into()));
        };
        replayer.output_at_position(path, positions, sink, bytes)
    }

    /// Existing blocked/ready markers surrounding an owner-side host retry.
    pub fn replay_host_readiness(&mut self, path: &[u32], ready: bool) -> Result<(), Diverged> {
        let op = self.op(path);
        let Side::Replay(replayer) = &mut self.side else {
            return Err(Diverged(
                "native host replay requires a replay session".into(),
            ));
        };
        if ready {
            replayer.ready(path, op)
        } else {
            replayer.blocked(path, op)
        }
    }

    /// Typed completion bytes at an admitted Done phase. Host handles, if
    /// present, still need the engine's persisted logical mapping.
    pub fn replay_host_completion(&mut self, path: &[u32]) -> Result<Vec<u8>, Diverged> {
        let op = self.op(path);
        let Side::Replay(replayer) = &mut self.side else {
            return Err(Diverged(
                "native host replay requires a replay session".into(),
            ));
        };
        replayer.completion(path, op)
    }

    /// Prepare an owned typed host call. The caller stages cancellation and the
    /// canonical request, then publishes and persists Entered before claim.
    pub fn record_host_prepare(
        &mut self,
        path: &[u32],
        keys: &[(&str, bool)],
    ) -> Result<HostWrite, Diverged> {
        let op = self.op(path);
        let Side::Live(recorder) = &mut self.side else {
            return Err(Diverged("host preparation requires recording".into()));
        };
        self.requests = self
            .requests
            .checked_add(1)
            .ok_or_else(|| Diverged("host request overflow".into()))?;
        let id = self.requests;
        Ok(HostWrite {
            op,
            live: Some(recorder.prepare(path, op, id)),
            positions: recorder.positions(path, op, 0, keys),
        })
    }

    pub fn record_host_id(request: &HostWrite) -> u64 {
        request.live.as_ref().expect("a live host call").id
    }

    pub fn record_host_positions(
        &mut self,
        path: &[u32],
        request: &mut HostWrite,
        index: u32,
        keys: &[(&str, bool)],
    ) -> Result<(), Diverged> {
        let Side::Live(recorder) = &mut self.side else {
            return Err(Diverged("host retry requires recording".into()));
        };
        recorder.ready(path, request.op);
        request.positions = recorder.positions(path, request.op, index, keys);
        Ok(())
    }

    pub fn record_host_ready(&self, request: &HostWrite) -> bool {
        match &self.side {
            Side::Live(recorder) => recorder.positions_ready(&request.positions),
            Side::Replay(_) => false,
        }
    }

    pub fn record_host_observation(&mut self, path: &[u32], bytes: &[u8]) -> Result<(), Diverged> {
        let op = self.op(path);
        let Side::Live(recorder) = &mut self.side else {
            return Err(Diverged("host observation requires recording".into()));
        };
        recorder.observation(path, op, bytes);
        Ok(())
    }

    /// Publish Entered and make Prepared durable before the claim CAS.
    pub fn record_host_entered(&mut self, path: &[u32]) -> Result<(), Diverged> {
        self.publish(path, Phase::Entered)?;
        self.record_host_durable(path, Phase::Entered)
    }

    fn record_host_durable(&mut self, path: &[u32], phase: Phase) -> Result<(), Diverged> {
        let progress = Progress::new(self.op(path), phase);
        let Side::Live(recorder) = &mut self.side else {
            return Err(Diverged("host durability requires recording".into()));
        };
        if let Err(error) = recorder.durable_through(path, progress) {
            self.stopped = true;
            return Err(Diverged(error.to_string()));
        }
        Ok(())
    }

    pub fn record_host_ended(
        &mut self,
        path: &[u32],
        request: &HostWrite,
        index: u32,
        bytes: &[u8],
        output: Option<(&str, &[u8])>,
        blocked: bool,
    ) -> Result<(), Diverged> {
        let Side::Live(recorder) = &mut self.side else {
            return Err(Diverged("host result requires recording".into()));
        };
        recorder.observation(path, request.op, bytes);
        if let Some((sink, bytes)) = output {
            let key = recorder
                .key_index(sink)
                .ok_or_else(|| Diverged("unknown output key".into()))?;
            let position = request
                .positions
                .iter()
                .find(|position| position.key == key)
                .ok_or_else(|| Diverged("output has no key position".into()))?;
            recorder.output(path, sink, position.epoch, bytes);
        }
        recorder.end_positions(&request.positions);
        if blocked {
            recorder.blocked(path, request.op);
        }
        self.publish(path, Phase::Ended(index))
    }

    /// Persist completion before returning any initialized result to the engine.
    pub fn record_host_complete(
        &mut self,
        path: &[u32],
        request: &HostWrite,
        bytes: &[u8],
    ) -> Result<(), Diverged> {
        let Side::Live(recorder) = &mut self.side else {
            return Err(Diverged("host completion requires recording".into()));
        };
        recorder
            .complete(
                path,
                request.op,
                request.live.as_ref().expect("live request"),
                bytes,
            )
            .map_err(|e| Diverged(format!("host completion: {e:?}")))?;
        self.tasks.get_mut(path).unwrap().progress = Progress::new(request.op, Phase::Done);
        self.record_host_durable(path, Phase::Done)
    }

    /// A host write's entered phase, once the engine began the op, the cut admitted its entry
    /// and the engine applied the task's pending releases: `Prepared`, its exclusive position
    /// on `sink`'s key, then (durable) the wait until the entry is under the watermark.
    pub fn host_begin(&mut self, path: &[u32], sink: &str) -> Result<HostWrite, Diverged> {
        let op = self.op(path);
        let (live, positions) = match &mut self.side {
            Side::Live(recorder) => {
                self.requests += 1;
                let live = recorder.prepare(path, op, self.requests);
                (
                    Some(live),
                    recorder.positions(path, op, 0, &[(sink, false)]),
                )
            }
            Side::Replay(replayer) => {
                replayer.prepare(path, op)?;
                (None, replayer.positions(path, op, 0, &[(sink, false)])?)
            }
        };
        self.publish(path, Phase::Entered)?;
        if let Side::Live(recorder) = &mut self.side
            && !self.stopped
        {
            recorder
                .durable_through(path, Progress::new(op, Phase::Entered))
                .map_err(|e| Diverged(e.to_string()))?;
        }
        Ok(HostWrite {
            op,
            live,
            positions,
        })
    }

    /// Claim a host write once its key's earlier epochs ended. [`Claim::Call`]: live, the
    /// engine makes the host call now, outside sections; a replay makes none. Then
    /// [`Session::host_end`].
    pub fn host_claim(&mut self, path: &[u32], request: &mut HostWrite) -> Result<Claim, Diverged> {
        let op = request.op;
        let ready = match &self.side {
            Side::Live(recorder) => recorder.positions_ready(&request.positions),
            Side::Replay(replayer) => replayer.positions_ready(&request.positions),
        };
        if !ready {
            return Ok(Claim::Waiting);
        }
        if !self.admits(path, Phase::Claimed) {
            return Ok(Claim::Final);
        }
        match &mut self.side {
            Side::Live(recorder) => {
                let live = request.live.as_ref().expect("a live request");
                if self.stopped || !recorder.claim(path, op, live).unwrap_or(false) {
                    // Withdrawn by the stop: it never issues.
                    return Ok(Claim::Final);
                }
                self.tasks.get_mut(path).unwrap().progress = Progress::new(op, Phase::Claimed);
            }
            Side::Replay(_) => self.publish(path, Phase::Claimed)?,
        }
        Ok(Claim::Call)
    }

    /// A claimed host write's call returned: its ended section (the output, its position
    /// ended) and its done section (the completion).
    pub fn host_end(
        &mut self,
        path: &[u32],
        request: &HostWrite,
        sink: &str,
        bytes: &[u8],
    ) -> Result<Advance, Diverged> {
        let op = request.op;
        if !self.admits(path, Phase::Ended(0)) {
            return Ok(Advance::Final);
        }
        let epoch = request.positions[0].epoch;
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.output(path, sink, epoch, bytes);
                recorder.end_positions(&request.positions);
            }
            Side::Replay(replayer) => {
                replayer.output(path, sink, epoch, bytes)?;
                replayer.end_positions(&request.positions);
            }
        }
        self.publish(path, Phase::Ended(0))?;
        if !self.admits(path, Phase::Done) {
            return Ok(Advance::Final);
        }
        match &mut self.side {
            Side::Live(recorder) => {
                let live = request.live.as_ref().expect("a live request");
                if self.stopped || recorder.complete(path, op, live, b"ok").is_err() {
                    return Ok(Advance::Final);
                }
                recorder
                    .durable_through(path, Progress::new(op, Phase::Done))
                    .map_err(|e| Diverged(e.to_string()))?;
                self.tasks.get_mut(path).unwrap().progress = Progress::new(op, Phase::Done);
            }
            Side::Replay(replayer) => {
                let completion = replayer.completion(path, op)?;
                if completion != b"ok" {
                    return Err(Diverged(format!(
                        "{path:?} op {op}: a write completed {completion:?}"
                    )));
                }
                self.publish(path, Phase::Done)?;
            }
        }
        Ok(Advance::Done)
    }

    /// Stop a live run: the terminal cut and the stop chunk. `task` is the stopping task.
    pub fn stop(&mut self, outcome: &str, task: Option<&[u32]>) {
        if let Side::Live(recorder) = &mut self.side
            && !self.stopped
        {
            self.stopped = true;
            recorder.stop(outcome, task).expect("an in-memory tape");
        }
    }

    /// Close a streaming recording with its canonical terminal and request receipt.
    pub fn finish_recording(
        mut self,
        outcome: impl FnOnce(&crate::claim::Sweep) -> String,
    ) -> Result<Projection, Diverged> {
        let Side::Live(recorder) = &mut self.side else {
            return Err(Diverged("a replay is not a recording".into()));
        };
        recorder
            .stop_with(outcome, None)
            .map_err(|e| Diverged(e.to_string()))?;
        Ok(recorder.final_projection())
    }

    /// A stopped live run's tape and projections.
    pub fn report(self) -> Report {
        let Side::Live(recorder) = self.side else {
            panic!("a replay has no recording report");
        };
        let cuts = recorder.cuts().to_vec();
        let mut projections: Vec<Projection> = cuts[..cuts.len().saturating_sub(1)]
            .iter()
            .map(|cut| recorder.projection("truncated", cut))
            .collect();
        projections.push(recorder.final_projection());
        let Sink::Memory(sink) = recorder.into_sink() else {
            panic!("a streaming recording finishes with its projection");
        };
        // Each cut's chunk ends at a barrier; the header's (durable runs) comes first.
        let first = sink.marks.len() - cuts.len();
        let cuts = projections
            .into_iter()
            .enumerate()
            .map(|(i, projection)| (sink.marks[first + i], projection))
            .collect();
        Report {
            tape: sink.bytes,
            cuts,
        }
    }

    /// Finish a replay: every record consumed and every started task at its cut.
    pub fn finish(self) -> Result<Projection, Diverged> {
        let Side::Replay(replayer) = self.side else {
            panic!("a recording finishes with its report");
        };
        let at: BTreeMap<Vec<u32>, Progress> = self
            .tasks
            .into_iter()
            .filter(|(path, _)| replayer.starts(path))
            .map(|(path, task)| (path, task.progress))
            .collect();
        replayer.finish(&at)
    }
}

/// F0's test channel into an engine: the `HIRSEL_RT_FREE_TEST` variable, a comma-separated
/// list `mode=record|replay,tape=PATH,report=PATH[,seed=N][,flush=N][,durable=1]`. A
/// recording runs under seeded schedule `seed` with injected flushes (one publish in
/// `flush`), writes its tape to `tape` and its live projection at every cut to `report`
/// ([`Report::render`]); a replay reads `tape`, runs under `seed`, and writes `replayed
/// <projection>` or `diverged <reason>` to `report`. F1 replaces it with the run record's
/// recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestHook {
    pub replay: bool,
    pub tape: std::path::PathBuf,
    pub report: std::path::PathBuf,
    pub seed: u64,
    pub flush_one_in: u64,
    pub durable: bool,
}

impl TestHook {
    pub const VAR: &'static str = "HIRSEL_RT_FREE_TEST";

    /// The hook the environment sets, if any.
    pub fn from_env() -> Result<Option<TestHook>, String> {
        match std::env::var(Self::VAR) {
            Ok(text) => Self::parse(&text).map(Some),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(e) => Err(format!("{}: {e}", Self::VAR)),
        }
    }

    pub fn parse(text: &str) -> Result<TestHook, String> {
        let bad = |what: &str| format!("{}: {what} in {text:?}", Self::VAR);
        let (mut mode, mut tape, mut report) = (None, None, None);
        let (mut seed, mut flush_one_in, mut durable) = (0, 0, false);
        for item in text.split(',') {
            let (key, value) = item
                .split_once('=')
                .ok_or_else(|| bad("an item without ="))?;
            let number = || value.parse::<u64>().map_err(|_| bad("a bad number"));
            match key {
                "mode" => mode = Some(value == "replay"),
                "tape" => tape = Some(value.into()),
                "report" => report = Some(value.into()),
                "seed" => seed = number()?,
                "flush" => flush_one_in = number()?,
                "durable" => durable = value == "1",
                _ => return Err(bad("an unknown key")),
            }
        }
        Ok(TestHook {
            replay: mode.ok_or_else(|| bad("no mode"))?,
            tape: tape.ok_or_else(|| bad("no tape"))?,
            report: report.ok_or_else(|| bad("no report"))?,
            seed,
            flush_one_in,
            durable,
        })
    }

    /// The run's session: a recording bound to `binding`, or a replay of the tape.
    pub fn session(&self, binding: &[u8]) -> Result<Session, String> {
        if self.replay {
            let tape = std::fs::read(&self.tape).map_err(|e| format!("free-mode tape: {e}"))?;
            Session::replay(&tape).map_err(|e| e.to_string())
        } else {
            Ok(Session::record(
                binding,
                Recording {
                    durable: self.durable,
                    flush_one_in: self.flush_one_in,
                    seed: self.seed,
                },
            ))
        }
    }

    /// End the run: a recording stops with `outcome` and writes its tape and report; a
    /// replay writes its projection, or why it diverged (also the error).
    pub fn finish(&self, mut session: Session, outcome: &str) -> Result<(), String> {
        let write = |path: &std::path::Path, bytes: &[u8]| {
            std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
        };
        if session.is_replay() {
            match session.finish() {
                Ok(projection) => write(
                    &self.report,
                    format!("replayed {projection:?}\n").as_bytes(),
                ),
                Err(diverged) => {
                    write(
                        &self.report,
                        format!("diverged {}\n", diverged.0).as_bytes(),
                    )?;
                    Err(diverged.to_string())
                }
            }
        } else {
            session.stop(outcome, None);
            let report = session.report();
            write(&self.tape, &report.tape)?;
            write(&self.report, report.render().as_bytes())
        }
    }
}
