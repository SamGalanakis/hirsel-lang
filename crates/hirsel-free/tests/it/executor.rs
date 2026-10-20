//! A reference cooperative executor for the protocol: tasks run small programs at width 1 and
//! one permit, one phase per step, with a seeded choice of the next task at every step (so
//! phases of different tasks interleave as simultaneous events would), injected flushes and an
//! injected stop. Live runs record through [`Recorder`]; replays follow a tape through
//! [`Replayer`] under any seed. Both emit the events the projection compares.
//!
//! Object ops follow the design's op table: creations, gapped sub-events (send, sender close,
//! mutex take and put), receives (no version), atomic loads (recorded values) and stores
//! (nothing), thaws (a bit), spawns and joins (nothing), and a durable host write on a sink
//! key with entered, claimed, ended and done phases. `Shared` releases inside segments are
//! deferred unless the releaser is the sole holder, applied at the next op's entry and drained
//! by the end op's cleanup phases.

use hirsel_free::claim::Request;
use hirsel_free::pending::Pending;
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::projection::Projection;
use hirsel_free::record::ObjectId;
use hirsel_free::recorder::{Position, Recorder, TapeSink};
use hirsel_free::replay::{Diverged, Replayer};
use hirsel_free::tape;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

pub type Name = &'static str;

/// One instruction. Releases, dups and freezes are segment work (no op); every other
/// instruction is one op.
#[derive(Clone, Debug)]
pub enum Ins {
    /// `channel.open` with this many Sender endpoints.
    Open(Name, u32),
    Send(Name, u32),
    Recv(Name),
    /// One Sender endpoint's close.
    Close(Name),
    Mutex(Name),
    Take(Name),
    Put(Name),
    Atomic(Name),
    Store(Name, u64),
    Load(Name),
    /// A new root holding one hold of each nested root (moved in from this task).
    Freeze(Name, Vec<Name>),
    Dup(Name),
    Release(Name),
    Thaw(Name),
    /// Spawn program `usize`, moving one hold of each named root into it.
    Spawn(usize, Vec<Name>),
    Join(usize),
    /// A durable host write of the bytes to a sink.
    Write(Name, Name),
    /// A pure loop: the task never reaches another op.
    Loop,
}

/// Program 0 is the root task's.
#[derive(Clone, Debug)]
pub struct Program(pub Vec<Vec<Ins>>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    Ready,
    Looping,
    Done,
    /// Replay: parked for good at its cut.
    Final,
    Unstarted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Host {
    Claim,
    Call,
    Done,
}

#[derive(Clone, Debug)]
enum Stage {
    Next,
    /// A waiting op published (k, entered); its done phase retries.
    Waiting,
    Host {
        step: Host,
        request: u64,
        live: Option<Arc<Request<()>>>,
        positions: Vec<Position>,
    },
    End {
        cleanup: u32,
    },
    EndDone,
}

struct Task {
    program: usize,
    path: Vec<u32>,
    pc: usize,
    op: u64,
    stage: Stage,
    pending: Pending<Name>,
    holds: Vec<Name>,
    status: Status,
    progress: Progress,
    spawned: u32,
}

struct Chan {
    id: ObjectId,
    queue: VecDeque<u32>,
    senders: u32,
}

struct Root {
    count: u32,
    nested: Vec<Name>,
}

/// A tape sink that remembers where each barrier fell.
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

enum Side {
    Live(Box<Recorder<Marked>>),
    Replay(Box<Replayer>),
}

/// How a run is driven.
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    pub seed: u64,
    pub durable: bool,
    /// Live: flush at a step with probability 1/n (0: never).
    pub flush_one_in: u64,
    /// Live: stop the run (a fuel stop by the chosen task) at this step.
    pub stop_at: Option<u64>,
    /// The mutation that removes deferral: every release applies at once.
    pub immediate_releases: bool,
    /// The design's r5 rule: a walk after a phase defers nested non-last releases to the
    /// walker's pending list (which this executor shows can strand them).
    pub deferred_walks: bool,
}

impl Drive {
    pub fn new(seed: u64) -> Drive {
        Drive {
            seed,
            durable: false,
            flush_one_in: 0,
            stop_at: None,
            immediate_releases: false,
            deferred_walks: false,
        }
    }
}

struct Exec<'p> {
    program: &'p Program,
    drive: Drive,
    rng: u64,
    side: Side,
    tasks: Vec<Task>,
    chans: HashMap<Name, Chan>,
    mutexes: HashMap<Name, (ObjectId, bool)>,
    atomics: HashMap<Name, u64>,
    roots: HashMap<Name, Root>,
    requests: u64,
    stopped: bool,
}

/// What a step did.
enum Step {
    Progress,
    /// The task cannot move now (a wait or an enablement).
    Blocked,
}

impl<'p> Exec<'p> {
    fn new(program: &'p Program, drive: Drive, side: Side) -> Exec<'p> {
        let root = Task {
            program: 0,
            path: vec![],
            pc: 0,
            op: 0,
            stage: Stage::Next,
            pending: Pending::default(),
            holds: vec![],
            status: Status::Ready,
            progress: Progress::START,
            spawned: 0,
        };
        let mut exec = Exec {
            program,
            drive,
            rng: drive.seed,
            side,
            tasks: vec![root],
            chans: HashMap::new(),
            mutexes: HashMap::new(),
            atomics: HashMap::new(),
            roots: HashMap::new(),
            requests: 0,
            stopped: false,
        };
        match &mut exec.side {
            Side::Live(recorder) => recorder.start(vec![]).unwrap(),
            Side::Replay(replayer) => {
                if !replayer.starts(&[]) {
                    exec.tasks[0].status = Status::Unstarted;
                }
            }
        }
        exec
    }

    fn word(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    fn path(&self, t: usize) -> Vec<u32> {
        self.tasks[t].path.clone()
    }

    /// Whether the replay admits `t`'s phase of its current op; parks it for good otherwise.
    fn admits(&mut self, t: usize, phase: Phase) -> bool {
        let progress = Progress::new(self.tasks[t].op, phase);
        let admitted = match &self.side {
            Side::Live(_) => true,
            Side::Replay(replayer) => replayer.admits(&self.tasks[t].path, progress),
        };
        if !admitted {
            self.tasks[t].status = Status::Final;
        }
        admitted
    }

    fn publish(&mut self, t: usize, phase: Phase) -> Result<(), Diverged> {
        let progress = Progress::new(self.tasks[t].op, phase);
        let path = self.path(t);
        match &mut self.side {
            Side::Live(recorder) => recorder
                .publish(&path, progress)
                .expect("no section opens after the stop"),
            Side::Replay(replayer) => replayer.publish(&path, progress)?,
        }
        self.tasks[t].progress = progress;
        self.reach_cut(t);
        Ok(())
    }

    /// A replayed task at its cut parks for good, before the segment that follows.
    fn reach_cut(&mut self, t: usize) {
        if let Side::Replay(replayer) = &self.side
            && replayer.cut(&self.tasks[t].path) == Some(self.tasks[t].progress)
        {
            self.tasks[t].status = Status::Final;
        }
    }

    /// A release in a segment, or of a nested root by a freeing walk.
    fn release(&mut self, t: usize, root: Name) {
        let count = self.roots[root].count;
        if count == 1 || self.drive.immediate_releases {
            self.roots.get_mut(root).unwrap().count -= 1;
            if count == 1 {
                self.walk(t, root, false);
            }
        } else {
            self.tasks[t].pending.defer(root);
        }
    }

    /// A freed root's frozen graph releases the roots it holds. A walk in a segment defers
    /// them like any segment release; a walk that ends a phase (its entry applied the last
    /// release) applies them at once, as the tail of that phase.
    fn walk(&mut self, t: usize, root: Name, in_phase: bool) {
        for nested in std::mem::take(&mut self.roots.get_mut(root).unwrap().nested) {
            if in_phase && !self.drive.deferred_walks {
                let entry = self.roots.get_mut(nested).unwrap();
                entry.count -= 1;
                if entry.count == 0 {
                    self.walk(t, nested, true);
                }
            } else {
                self.release(t, nested);
            }
        }
    }

    /// An entered or cleanup phase applies the pending list; the roots that reach zero are
    /// walked after the section.
    fn apply_pending(&mut self, t: usize) -> Vec<Name> {
        let mut freed = Vec::new();
        for (root, n) in self.tasks[t].pending.take() {
            let entry = self.roots.get_mut(root).unwrap();
            entry.count -= n;
            if entry.count == 0 {
                freed.push(root);
            }
        }
        freed
    }

    fn walk_all(&mut self, t: usize, freed: Vec<Name>) {
        for root in freed {
            self.walk(t, root, true);
        }
    }

    /// Segment work up to the next op.
    fn segment(&mut self, t: usize) {
        let code = &self.program.0[self.tasks[t].program];
        while let Some(ins) = code.get(self.tasks[t].pc) {
            match ins {
                Ins::Freeze(root, nested) => {
                    for n in nested {
                        let at = self.tasks[t]
                            .holds
                            .iter()
                            .position(|h| h == n)
                            .expect("a held nested root");
                        self.tasks[t].holds.remove(at);
                    }
                    self.roots.insert(
                        root,
                        Root {
                            count: 1,
                            nested: nested.clone(),
                        },
                    );
                    self.tasks[t].holds.push(root);
                }
                Ins::Dup(root) => {
                    if !self.tasks[t].pending.dup(root) {
                        self.roots.get_mut(root).unwrap().count += 1;
                    }
                    self.tasks[t].holds.push(root);
                }
                Ins::Release(root) => {
                    if let Some(at) = self.tasks[t].holds.iter().position(|h| h == root) {
                        self.tasks[t].holds.remove(at);
                        self.release(t, root);
                    }
                }
                Ins::Loop => {
                    self.tasks[t].status = Status::Looping;
                    return;
                }
                _ => return,
            }
            self.tasks[t].pc += 1;
        }
    }

    fn create(&mut self, t: usize, object: ObjectId) -> Result<(), Diverged> {
        let path = self.path(t);
        match &mut self.side {
            Side::Live(recorder) => recorder.create(&path, object),
            Side::Replay(replayer) => replayer.create(&path, object)?,
        }
        Ok(())
    }

    fn enabled(&self, t: usize, object: &ObjectId) -> Result<bool, Diverged> {
        match &self.side {
            Side::Live(_) => Ok(true),
            Side::Replay(replayer) => {
                replayer.enabled(&self.tasks[t].path, self.tasks[t].op, 0, object)
            }
        }
    }

    fn touch(&mut self, t: usize, object: &ObjectId, outcome: &str) -> Result<(), Diverged> {
        let (path, op) = (self.path(t), self.tasks[t].op);
        match &mut self.side {
            Side::Live(recorder) => {
                recorder.touch(&path, op, 0, object, outcome);
            }
            Side::Replay(replayer) => replayer.touch(&path, op, 0, object, outcome)?,
        }
        Ok(())
    }

    fn made(&self, t: usize) -> ObjectId {
        ObjectId::Made {
            path: self.tasks[t].path.clone(),
            op: self.tasks[t].op,
        }
    }

    /// Whether a single-phase op can act now, and its gapped object, if any.
    fn single_ready(&self, t: usize, ins: &Ins) -> Result<bool, Diverged> {
        Ok(match ins {
            Ins::Send(c, _) | Ins::Close(c) => self.enabled(t, &self.chans[c].id)?,
            Ins::Put(m) => self.enabled(t, &self.mutexes[m].0)?,
            Ins::Thaw(root) => match &self.side {
                Side::Live(_) => true,
                Side::Replay(replayer) => {
                    let ok = replayer.thaw_outcome(&self.tasks[t].path, self.tasks[t].op);
                    ok != Some(true) || self.roots[root].count == 1
                }
            },
            _ => true,
        })
    }

    /// A single-phase op's action, in its section.
    fn single(&mut self, t: usize, ins: &Ins) -> Result<(), Diverged> {
        let (path, op) = (self.path(t), self.tasks[t].op);
        match ins {
            Ins::Open(c, senders) => {
                let id = self.made(t);
                self.create(t, id.clone())?;
                self.chans.insert(
                    c,
                    Chan {
                        id,
                        queue: VecDeque::new(),
                        senders: *senders,
                    },
                );
            }
            Ins::Send(c, value) => {
                let id = self.chans[c].id.clone();
                self.touch(t, &id, &format!("send {value}"))?;
                self.chans.get_mut(c).unwrap().queue.push_back(*value);
            }
            Ins::Close(c) => {
                let id = self.chans[c].id.clone();
                self.touch(t, &id, "close")?;
                self.chans.get_mut(c).unwrap().senders -= 1;
            }
            Ins::Mutex(m) => {
                let id = self.made(t);
                self.create(t, id.clone())?;
                self.mutexes.insert(m, (id, false));
            }
            Ins::Put(m) => {
                let id = self.mutexes[m].0.clone();
                self.touch(t, &id, "put")?;
                self.mutexes.get_mut(m).unwrap().1 = false;
            }
            Ins::Atomic(a) => {
                self.atomics.insert(a, 0);
            }
            Ins::Store(a, value) => {
                self.atomics.insert(a, *value);
            }
            Ins::Load(a) => {
                let value = self.atomics[a];
                match &mut self.side {
                    Side::Live(recorder) => recorder.load(&path, op, u128::from(value)),
                    // The recorded value is substituted, whatever the replay's cell holds.
                    Side::Replay(replayer) => drop(replayer.load(&path, op)?),
                }
            }
            Ins::Thaw(root) => {
                let ok = match &mut self.side {
                    Side::Live(recorder) => {
                        let ok = self.roots[root].count == 1;
                        recorder.thaw(&path, op, ok);
                        ok
                    }
                    Side::Replay(replayer) => replayer.thaw(&path, op)?,
                };
                if ok {
                    let at = self.tasks[t]
                        .holds
                        .iter()
                        .position(|h| h == root)
                        .expect("a held root");
                    self.tasks[t].holds.remove(at);
                    let entry = self.roots.get_mut(root).unwrap();
                    entry.count = 0;
                    let nested = std::mem::take(&mut entry.nested);
                    self.tasks[t].holds.extend(nested);
                }
            }
            Ins::Spawn(program, moved) => {
                let mut child_path = path.clone();
                child_path.push(self.tasks[t].spawned);
                self.tasks[t].spawned += 1;
                let mut holds = Vec::new();
                for root in moved {
                    let at = self.tasks[t]
                        .holds
                        .iter()
                        .position(|h| h == root)
                        .expect("a held root to move");
                    holds.push(self.tasks[t].holds.remove(at));
                }
                let status = match &mut self.side {
                    Side::Live(recorder) => {
                        recorder.start(child_path.clone()).unwrap();
                        Status::Ready
                    }
                    Side::Replay(replayer) if replayer.starts(&child_path) => Status::Ready,
                    Side::Replay(_) => Status::Unstarted,
                };
                self.tasks.push(Task {
                    program: *program,
                    path: child_path,
                    pc: 0,
                    op: 0,
                    stage: Stage::Next,
                    pending: Pending::default(),
                    holds,
                    status,
                    progress: Progress::START,
                    spawned: 0,
                });
            }
            other => unreachable!("{other:?} is not a single-phase op"),
        }
        Ok(())
    }

    /// Whether a waiting op's done phase can act now.
    fn wait_ready(&self, t: usize, ins: &Ins) -> Result<bool, Diverged> {
        Ok(match ins {
            Ins::Recv(c) => !self.chans[c].queue.is_empty() || self.chans[c].senders == 0,
            Ins::Take(m) => !self.mutexes[m].1 && self.enabled(t, &self.mutexes[m].0)?,
            Ins::Join(program) => self.tasks.iter().any(|child| {
                child.program == *program
                    && child.path.starts_with(&self.tasks[t].path)
                    && child.status == Status::Done
            }),
            other => unreachable!("{other:?} is not a waiting op"),
        })
    }

    fn wait_act(&mut self, t: usize, ins: &Ins) -> Result<(), Diverged> {
        let path = self.path(t);
        match ins {
            Ins::Recv(c) => {
                let chan = self.chans.get_mut(c).unwrap();
                let outcome = match chan.queue.pop_front() {
                    Some(value) => format!("msg {value}"),
                    None => "closed".into(),
                };
                let id = chan.id.clone();
                match &mut self.side {
                    Side::Live(recorder) => recorder.receive(&path, &id, &outcome),
                    Side::Replay(replayer) => replayer.receive(&path, &id, &outcome)?,
                }
            }
            Ins::Take(m) => {
                let id = self.mutexes[m].0.clone();
                self.touch(t, &id, "take")?;
                self.mutexes.get_mut(m).unwrap().1 = true;
            }
            Ins::Join(_) => {}
            _ => unreachable!(),
        }
        Ok(())
    }

    fn step(&mut self, t: usize) -> Result<Step, Diverged> {
        if matches!(self.tasks[t].stage, Stage::Next) {
            // Segment work is its own step, so other tasks' phases can fall between a release
            // and the entry that applies it, as they do at width N.
            let pc = self.tasks[t].pc;
            self.segment(t);
            if self.tasks[t].status != Status::Ready || self.tasks[t].pc != pc {
                return Ok(Step::Progress);
            }
        }
        let code = &self.program.0[self.tasks[t].program];
        let stage = self.tasks[t].stage.clone();
        match stage {
            Stage::Next => {
                let Some(ins) = code.get(self.tasks[t].pc).cloned() else {
                    // The task's own holds are released by its cleanup code, then the end op.
                    for root in std::mem::take(&mut self.tasks[t].holds) {
                        self.release(t, root);
                    }
                    self.tasks[t].op += 1;
                    self.tasks[t].stage = Stage::End { cleanup: 0 };
                    return Ok(Step::Progress);
                };
                match ins {
                    Ins::Recv(_) | Ins::Take(_) | Ins::Join(_) => {
                        self.tasks[t].op += 1;
                        if !self.admits(t, Phase::Entered) {
                            return Ok(Step::Progress);
                        }
                        let freed = self.apply_pending(t);
                        let done =
                            self.admits_quietly(t, Phase::Done) && self.wait_ready(t, &ins)?;
                        if done {
                            self.wait_act(t, &ins)?;
                            self.publish(t, Phase::Done)?;
                            self.tasks[t].pc += 1;
                        } else {
                            self.publish(t, Phase::Entered)?;
                            self.tasks[t].stage = Stage::Waiting;
                        }
                        self.walk_all(t, freed);
                    }
                    Ins::Write(sink, bytes) => {
                        self.tasks[t].op += 1;
                        if !self.admits(t, Phase::Entered) {
                            return Ok(Step::Progress);
                        }
                        let (path, op) = (self.path(t), self.tasks[t].op);
                        let freed = self.apply_pending(t);
                        let _ = bytes;
                        let (request, live, positions) = match &mut self.side {
                            Side::Live(recorder) => {
                                self.requests += 1;
                                let live = recorder.prepare(&path, op, self.requests);
                                (
                                    self.requests,
                                    Some(live),
                                    recorder.positions(&path, op, 0, &[(sink, false)]),
                                )
                            }
                            Side::Replay(replayer) => (
                                replayer.prepare(&path, op)?,
                                None,
                                replayer.positions(&path, op, 0, &[(sink, false)])?,
                            ),
                        };
                        self.publish(t, Phase::Entered)?;
                        if let Side::Live(recorder) = &mut self.side {
                            recorder
                                .durable_through(&path, Progress::new(op, Phase::Entered))
                                .unwrap();
                        }
                        self.tasks[t].stage = Stage::Host {
                            step: Host::Claim,
                            request,
                            live,
                            positions,
                        };
                        self.walk_all(t, freed);
                    }
                    ins => {
                        self.tasks[t].op += 1;
                        if !self.admits(t, Phase::Done) {
                            return Ok(Step::Progress);
                        }
                        if !self.single_ready(t, &ins)? {
                            self.tasks[t].op -= 1;
                            return Ok(Step::Blocked);
                        }
                        let freed = self.apply_pending(t);
                        self.single(t, &ins)?;
                        self.publish(t, Phase::Done)?;
                        self.tasks[t].pc += 1;
                        self.walk_all(t, freed);
                    }
                }
            }
            Stage::Waiting => {
                let ins = code[self.tasks[t].pc].clone();
                if !self.admits(t, Phase::Done) {
                    return Ok(Step::Progress);
                }
                if !self.wait_ready(t, &ins)? {
                    return Ok(Step::Blocked);
                }
                self.wait_act(t, &ins)?;
                self.publish(t, Phase::Done)?;
                self.tasks[t].pc += 1;
                self.tasks[t].stage = Stage::Next;
            }
            Stage::Host {
                step,
                request,
                live,
                positions,
            } => {
                let Ins::Write(sink, bytes) = code[self.tasks[t].pc] else {
                    unreachable!()
                };
                let (path, op) = (self.path(t), self.tasks[t].op);
                match step {
                    Host::Claim => {
                        let ready = match &self.side {
                            Side::Live(recorder) => recorder.positions_ready(&positions),
                            Side::Replay(replayer) => replayer.positions_ready(&positions),
                        };
                        if !ready {
                            return Ok(Step::Blocked);
                        }
                        if !self.admits(t, Phase::Claimed) {
                            return Ok(Step::Progress);
                        }
                        match &mut self.side {
                            Side::Live(recorder) => {
                                let live = live.as_ref().expect("a live request");
                                assert!(
                                    recorder.claim(&path, op, live).unwrap(),
                                    "a running request is never withdrawn"
                                );
                                self.tasks[t].progress = Progress::new(op, Phase::Claimed);
                            }
                            Side::Replay(_) => self.publish(t, Phase::Claimed)?,
                        }
                        self.tasks[t].stage = Stage::Host {
                            step: Host::Call,
                            request,
                            live,
                            positions,
                        };
                    }
                    Host::Call => {
                        // The host call, outside sections, then its ended section.
                        if !self.admits(t, Phase::Ended(0)) {
                            return Ok(Step::Progress);
                        }
                        let epoch = positions[0].epoch;
                        match &mut self.side {
                            Side::Live(recorder) => {
                                recorder.output(&path, sink, epoch, bytes.as_bytes());
                                recorder.end_positions(&positions);
                            }
                            Side::Replay(replayer) => {
                                replayer.output(&path, sink, epoch, bytes.as_bytes())?;
                                replayer.end_positions(&positions);
                            }
                        }
                        self.publish(t, Phase::Ended(0))?;
                        self.tasks[t].stage = Stage::Host {
                            step: Host::Done,
                            request,
                            live,
                            positions,
                        };
                    }
                    Host::Done => {
                        if !self.admits(t, Phase::Done) {
                            return Ok(Step::Progress);
                        }
                        match &mut self.side {
                            Side::Live(recorder) => {
                                let live = live.as_ref().expect("a live request");
                                recorder.complete(&path, op, live, b"ok").unwrap();
                                recorder
                                    .durable_through(&path, Progress::new(op, Phase::Done))
                                    .unwrap();
                                self.tasks[t].progress = Progress::new(op, Phase::Done);
                            }
                            Side::Replay(replayer) => {
                                assert_eq!(replayer.completion(&path, op)?, b"ok");
                                self.publish(t, Phase::Done)?;
                            }
                        }
                        self.tasks[t].pc += 1;
                        self.tasks[t].stage = Stage::Next;
                    }
                }
            }
            Stage::End { cleanup } => {
                let phase = if cleanup == 0 {
                    Phase::Entered
                } else {
                    Phase::Cleanup(cleanup)
                };
                if !self.admits(t, phase) {
                    return Ok(Step::Progress);
                }
                let freed = self.apply_pending(t);
                self.publish(t, phase)?;
                self.walk_all(t, freed);
                self.tasks[t].stage = if self.tasks[t].pending.is_empty() {
                    Stage::EndDone
                } else {
                    Stage::End {
                        cleanup: cleanup + 1,
                    }
                };
            }
            Stage::EndDone => {
                if !self.admits(t, Phase::Done) {
                    return Ok(Step::Progress);
                }
                self.publish(t, Phase::Done)?;
                self.tasks[t].status = Status::Done;
            }
        }
        Ok(Step::Progress)
    }

    fn admits_quietly(&self, t: usize, phase: Phase) -> bool {
        match &self.side {
            Side::Live(_) => true,
            Side::Replay(replayer) => {
                replayer.admits(&self.tasks[t].path, Progress::new(self.tasks[t].op, phase))
            }
        }
    }
}

/// A live run's tape, its live projections at each cut it wrote, and the request sweep.
pub struct Recorded {
    pub tape: Vec<u8>,
    pub marks: Vec<usize>,
    pub cuts: Vec<BTreeMap<Vec<u32>, Progress>>,
    pub live: Projection,
    pub at_cut: Vec<Projection>,
    pub withdrawn: usize,
    pub in_doubt: usize,
}

/// Run `program` live under `drive`, recording.
pub fn record(program: &Program, drive: Drive) -> Recorded {
    let recorder = Recorder::new(Marked::default(), b"reference executor", drive.durable).unwrap();
    let mut exec = Exec::new(program, drive, Side::Live(Box::new(recorder)));
    let mut steps = 0u64;
    let mut outcome = None;
    loop {
        if drive.stop_at == Some(steps) {
            let running: Vec<usize> = (0..exec.tasks.len())
                .filter(|&t| exec.tasks[t].status == Status::Ready)
                .collect();
            if !running.is_empty() {
                let t = running[(exec.word() % running.len() as u64) as usize];
                outcome = Some(("fuel".to_string(), Some(exec.path(t))));
                break;
            }
        }
        if drive.flush_one_in > 0
            && exec.word().is_multiple_of(drive.flush_one_in)
            && let Side::Live(recorder) = &mut exec.side
        {
            recorder.flush().unwrap();
        }
        let mut order: Vec<usize> = (0..exec.tasks.len())
            .filter(|&t| exec.tasks[t].status == Status::Ready)
            .collect();
        let mut moved = false;
        while !order.is_empty() {
            let t = order.remove((exec.word() % order.len() as u64) as usize);
            if let Step::Progress = exec.step(t).expect("a live run never diverges") {
                moved = true;
                break;
            }
        }
        steps += 1;
        if !moved {
            break;
        }
    }
    let (outcome, task) = outcome.unwrap_or_else(|| {
        if let Some(t) = exec
            .tasks
            .iter()
            .position(|task| task.status == Status::Looping)
        {
            ("fuel".to_string(), Some(exec.path(t)))
        } else if exec.tasks[0].status == Status::Done {
            ("returned".to_string(), None)
        } else {
            ("blocked".to_string(), None)
        }
    });
    exec.stopped = true;
    let Side::Live(mut recorder) = exec.side else {
        unreachable!()
    };
    let sweep = recorder
        .stop(&outcome, task.as_deref())
        .unwrap()
        .expect("the first stop wins");
    let live = recorder.final_projection();
    let cuts = recorder.cuts().to_vec();
    let at_cut = cuts[..cuts.len() - 1]
        .iter()
        .map(|cut| recorder.projection("truncated", cut))
        .collect();
    let sink = recorder.into_sink();
    Recorded {
        tape: sink.bytes,
        marks: sink.marks,
        cuts,
        live,
        at_cut,
        withdrawn: sweep.withdrawn.len(),
        in_doubt: sweep.in_doubt.len(),
    }
}

/// Replay `tape` under `seed`; the replayed projection.
pub fn replay(program: &Program, tape_bytes: &[u8], seed: u64) -> Result<Projection, Diverged> {
    replay_with(program, tape_bytes, Drive::new(seed))
}

/// Replay `tape` with `drive`'s seed and release rules.
pub fn replay_with(
    program: &Program,
    tape_bytes: &[u8],
    drive: Drive,
) -> Result<Projection, Diverged> {
    let tape = tape::parse(tape_bytes).map_err(Diverged)?;
    let replayer = Replayer::new(&tape)?;
    let mut exec = Exec::new(program, drive, Side::Replay(Box::new(replayer)));
    loop {
        let mut order: Vec<usize> = (0..exec.tasks.len())
            .filter(|&t| exec.tasks[t].status == Status::Ready)
            .collect();
        let mut moved = false;
        while !order.is_empty() {
            let t = order.remove((exec.word() % order.len() as u64) as usize);
            if let Step::Progress = exec.step(t)? {
                moved = true;
                break;
            }
        }
        if !moved {
            break;
        }
    }
    if let Some(task) = exec.tasks.iter().find(|task| task.status == Status::Ready) {
        return Err(Diverged(format!(
            "replay is stuck: task {:?} waits at {:?}",
            task.path, task.stage
        )));
    }
    let at: BTreeMap<Vec<u32>, Progress> = exec
        .tasks
        .iter()
        .filter(|t| t.status != Status::Unstarted)
        .map(|t| (t.path.clone(), t.progress))
        .collect();
    let Side::Replay(replayer) = exec.side else {
        unreachable!()
    };
    replayer.finish(&at)
}
