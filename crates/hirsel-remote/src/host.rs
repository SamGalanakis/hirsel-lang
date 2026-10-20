//! A runner host's protocol (RUN-8, RUN-9, RUN-11, RUN-13), with no I/O: sessions,
//! per-caller call streams, arrival, queueing, cancellation and status. The transport
//! feeds it connections and frames, the executor feeds it callee ends, and it answers with
//! [`Action`]s.
//!
//! **Streams.** Each caller run opens one ordered stream per runner; the host keys it by the
//! caller's authenticated name and the stream bytes. Positions are decided strictly in
//! order, and every decided position keeps its request digest and, once known, its
//! outcome until the caller acknowledges it, so many calls may be in flight:
//! - the next position (`applied + 1`) is decided: refused with `NotRun(..)`, or admitted
//!   to run;
//! - a decided position with the same digest is a duplicate: it gets the stored outcome
//!   (or, while its callee runs, the outcome when it ends) and never runs again; with a
//!   different digest it is `NotRun(Rejected())`;
//! - a position beyond the next is a gap, refused with `NotRun(Rejected())`, and that
//!   refusal is sticky: when the gap closes the position is decided `Rejected` too, so the
//!   host never contradicts an answer it gave;
//! - a position at or below the acknowledged one is stale and refused.
//!
//! The stream position is decided before the code and authority checks of RUN-8 (whose
//! order the spec gives for a single request), so every decision, refusals included, is
//! recorded at its position and a duplicate is answered without being checked again.
//!
//! **Sessions.** A session is one connection's view of a stream at an epoch. Opening at a
//! higher epoch fences the previous session: its connection is closed and frames carrying
//! an older epoch are discarded, so after a caller reconnects nothing it sent before can
//! still arrive and be decided. Gap refusals belong to the session that saw them.
//!
//! **Callees.** An admitted call queues for the executor, which runs at most `runs` callees
//! at once, with at most `queue` waiting; beyond that the call is `NotRun(Overloaded())`.
//! A cancellation of a queued call decides it `Canceled(cause)`, proved never started; of a
//! running one it is forwarded once and acknowledged. A caller whose session is lost has
//! its running callees cancelled with cause `Parent` and its queued ones decided
//! `Canceled(Parent())` (RUN-9); the caller learns those outcomes through status queries.
use crate::frame::{Call, CallRef, Frame, Outcome};
use crate::journal::{Entry, Journal, StreamKey};
use hirsel_run_schema::{CallOutcome, IoGrants, call_key};
use std::collections::{BTreeMap, VecDeque};

pub type SessionId = u64;
pub type RunId = u64;

pub const DENIED: u32 = 0;
pub const UNREACHABLE: u32 = 2;
pub const OVERLOADED: u32 = 3;
pub const REJECTED: u32 = 4;
pub const ABANDONED: u32 = 2;
/// `CancelCause.Parent()`.
pub const PARENT: u32 = 1;

/// The caller an authenticated session belongs to: the name the host's configuration gives
/// its identity, and the grant set and allowlist its calls run within.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Caller {
    pub name: String,
    pub grants: IoGrants,
    pub definitions: Option<Vec<String>>,
}

/// Why a place refuses a request it could decide: a `NotRunReason` ordinal, and the
/// definition hashes it lacks for `VersionMismatch`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    pub reason: u32,
    pub missing: Vec<[u8; 32]>,
}

/// The place's own checks of one request (RUN-8 checks 3, 4 and 8): its code, the
/// authority the request asks for within the caller's grants and allowlists, the code
/// policy and the argument's schema. On success, the grant set the callee runs under.
pub trait Place {
    fn admit(&mut self, caller: &Caller, call: &Call) -> Result<IoGrants, Refusal>;
}

/// A request's digest as the caller computed it (RUN-11).
pub trait Digests {
    fn digest(&self, call: &Call) -> [u8; 32];
}

/// What the host asks its transport and executor to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Send(SessionId, Frame),
    /// Close the session's connection.
    Close(SessionId),
    Start(Start),
    /// Forward a cancellation to a running callee (cause ordinal).
    Cancel(RunId, u32),
}

/// A callee to run as its own run (RUN-10).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Start {
    pub run: RunId,
    pub caller: String,
    /// The caller's run instance, the request ordinal and digest: the callee's cause.
    pub instance: [u8; 16],
    pub digest: [u8; 32],
    pub call: Call,
    pub grants: IoGrants,
}

/// The host's limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Callees running at once.
    pub runs: u64,
    /// Admitted calls waiting for a run.
    pub queue: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self { runs: 1, queue: 64 }
    }
}

#[derive(Clone, Debug)]
enum State {
    Queued(RunId),
    Running {
        run: RunId,
        forwarded: bool,
    },
    Done {
        end: CallOutcome,
        callee: Option<[u8; 16]>,
        missing: Vec<[u8; 32]>,
    },
}
#[derive(Clone, Debug)]
struct Decided {
    key: [u8; 32],
    digest: [u8; 32],
    state: State,
}
struct Stream {
    instance: [u8; 16],
    epoch: u64,
    session: Option<SessionId>,
    applied: u64,
    acknowledged: u64,
    decided: BTreeMap<u64, Decided>,
    /// This session's gap refusals above `applied + 1`: key and digest.
    gaps: BTreeMap<u64, ([u8; 32], [u8; 32])>,
}
fn stream_of<'a>(streams: &'a mut BTreeMap<StreamKey, Stream>, key: &StreamKey) -> &'a mut Stream {
    streams.entry(key.clone()).or_insert_with(|| Stream {
        instance: [0; 16],
        epoch: 0,
        session: None,
        applied: 0,
        acknowledged: 0,
        decided: BTreeMap::new(),
        gaps: BTreeMap::new(),
    })
}
struct Session {
    caller: Option<Caller>,
    stream: Option<StreamKey>,
}
struct Run {
    stream: StreamKey,
    position: u64,
    start: Start,
}

/// One runner host's protocol state.
pub struct Host {
    journal: Box<dyn Journal>,
    limits: Limits,
    sessions: BTreeMap<SessionId, Session>,
    streams: BTreeMap<StreamKey, Stream>,
    runs: BTreeMap<RunId, Run>,
    queued: VecDeque<RunId>,
    running: u64,
    next_run: RunId,
}

impl Host {
    /// A host over `journal`, recovering what a previous incarnation decided: a call it
    /// admitted and never decided is decided `InDoubt(Abandoned())` now.
    pub fn new(mut journal: Box<dyn Journal>, limits: Limits) -> std::io::Result<Self> {
        let mut streams: BTreeMap<StreamKey, Stream> = BTreeMap::new();
        let mut admitted: BTreeMap<(StreamKey, u64), ([u8; 32], [u8; 32])> = BTreeMap::new();
        let mut decided = vec![];
        for entry in journal.recover()? {
            match entry {
                Entry::Stream {
                    stream: key,
                    instance,
                } => {
                    stream_of(&mut streams, &key).instance = instance;
                }
                Entry::Admitted {
                    stream: key,
                    position,
                    key: call,
                    digest,
                } => {
                    admitted.insert((key, position), (call, digest));
                }
                Entry::Decided {
                    stream: key,
                    position,
                    key: call,
                    digest,
                    end,
                    callee,
                } => {
                    admitted.remove(&(key.clone(), position));
                    decided.push((key, position, call, digest, end, callee));
                }
                Entry::Acknowledged {
                    stream: key,
                    position,
                } => {
                    let s = stream_of(&mut streams, &key);
                    s.acknowledged = s.acknowledged.max(position);
                    s.decided.retain(|p, _| *p > position);
                }
            }
            for (key, position, call, digest, end, callee) in decided.drain(..) {
                let s = stream_of(&mut streams, &key);
                s.applied = s.applied.max(position);
                if position > s.acknowledged {
                    s.decided.insert(
                        position,
                        Decided {
                            key: call,
                            digest,
                            state: State::Done {
                                end,
                                callee,
                                missing: vec![],
                            },
                        },
                    );
                }
            }
        }
        // A call admitted and never decided may have run in part: its place ended it
        // without an outcome.
        for ((key, position), (call, digest)) in admitted {
            let end = CallOutcome::InDoubt(ABANDONED);
            journal.append(&Entry::Decided {
                stream: key.clone(),
                position,
                key: call,
                digest,
                end: end.clone(),
                callee: None,
            })?;
            let s = stream_of(&mut streams, &key);
            s.applied = s.applied.max(position);
            s.decided.insert(
                position,
                Decided {
                    key: call,
                    digest,
                    state: State::Done {
                        end,
                        callee: None,
                        missing: vec![],
                    },
                },
            );
        }
        Ok(Self {
            journal,
            limits,
            sessions: BTreeMap::new(),
            streams,
            runs: BTreeMap::new(),
            queued: VecDeque::new(),
            running: 0,
            next_run: 1,
        })
    }

    /// A connection authenticated as `caller`, or as no admitted caller (None).
    pub fn connected(&mut self, session: SessionId, caller: Option<Caller>) {
        self.sessions.insert(
            session,
            Session {
                caller,
                stream: None,
            },
        );
    }

    /// A session's connection ended. When it was its stream's current session, the
    /// caller's queued calls are decided `Canceled(Parent())` and its running callees
    /// cancelled.
    pub fn disconnected(&mut self, session: SessionId) -> Vec<Action> {
        let mut actions = vec![];
        let Some(Session {
            stream: Some(key), ..
        }) = self.sessions.remove(&session)
        else {
            return actions;
        };
        let Some(stream) = self.streams.get_mut(&key) else {
            return actions;
        };
        if stream.session != Some(session) {
            return actions;
        }
        stream.session = None;
        stream.gaps.clear();
        self.cancel_stream(&key, PARENT, &mut actions);
        actions
    }

    fn cancel_stream(&mut self, key: &StreamKey, cause: u32, actions: &mut Vec<Action>) {
        let positions: Vec<u64> = self.streams[key]
            .decided
            .iter()
            .filter(|(_, d)| !matches!(d.state, State::Done { .. }))
            .map(|(p, _)| *p)
            .collect();
        for position in positions {
            self.cancel(key, position, cause, actions);
        }
    }

    /// Cancel the call at `position`: a queued one is decided `Canceled`, a running one has
    /// the cancellation forwarded once. True when the callee had started.
    fn cancel(
        &mut self,
        key: &StreamKey,
        position: u64,
        cause: u32,
        actions: &mut Vec<Action>,
    ) -> bool {
        let stream = self.streams.get_mut(key).unwrap();
        let decided = stream.decided.get_mut(&position).unwrap();
        match &mut decided.state {
            State::Queued(run) => {
                let run = *run;
                self.queued.retain(|r| *r != run);
                self.runs.remove(&run);
                self.decide_end(key, position, CallOutcome::Canceled(cause), None, actions);
                false
            }
            State::Running { run, forwarded } => {
                if !*forwarded {
                    *forwarded = true;
                    actions.push(Action::Cancel(*run, cause));
                }
                true
            }
            State::Done { .. } => false,
        }
    }

    /// One frame from a session.
    pub fn frame(
        &mut self,
        session: SessionId,
        frame: Frame,
        place: &mut dyn Place,
        digests: &dyn Digests,
    ) -> Vec<Action> {
        let mut actions = vec![];
        let Some(current) = self.sessions.get(&session) else {
            return actions;
        };
        if let Frame::Open {
            stream,
            instance,
            epoch,
        } = frame
        {
            self.open(session, stream, instance, epoch, &mut actions);
            return actions;
        }
        // Every other frame belongs to the session's stream at its current epoch.
        let Some(key) = current.stream.clone() else {
            return actions;
        };
        let caller = current.caller.clone().unwrap();
        let stream = &self.streams[&key];
        if stream.session != Some(session) || frame.epoch() != Some(stream.epoch) {
            return actions;
        }
        match frame {
            Frame::Call(call) => {
                self.call(session, &key, &caller, *call, place, digests, &mut actions)
            }
            Frame::Cancel(r, cause) => match self.streams[&key].decided.get(&r.position) {
                Some(d) if d.key == r.key => {
                    if self.cancel(&key, r.position, cause, &mut actions) {
                        actions.push(Action::Send(session, Frame::Acknowledged(r)));
                    } else {
                        self.resend(session, &key, r, &mut actions);
                    }
                }
                _ => actions.push(Action::Send(session, Frame::Unknown(r))),
            },
            Frame::Status(r) => self.status(session, &key, r, &mut actions),
            Frame::Close { acknowledged, .. } => {
                self.acknowledge(&key, acknowledged);
                let stream = self.streams.get_mut(&key).unwrap();
                stream.session = None;
                stream.gaps.clear();
                self.sessions.remove(&session);
                actions.push(Action::Close(session));
            }
            // Host-to-caller frames are never accepted from a caller.
            _ => (),
        }
        actions
    }

    fn open(
        &mut self,
        session: SessionId,
        stream_id: [u8; 32],
        instance: [u8; 16],
        epoch: u64,
        actions: &mut Vec<Action>,
    ) {
        let Some(caller) = self.sessions[&session].caller.clone() else {
            actions.push(Action::Send(session, Frame::Refused(DENIED)));
            actions.push(Action::Close(session));
            return;
        };
        let key = StreamKey {
            caller: caller.name.clone(),
            stream: stream_id,
        };
        if !self.streams.contains_key(&key) {
            if self
                .journal
                .append(&Entry::Stream {
                    stream: key.clone(),
                    instance,
                })
                .is_err()
            {
                actions.push(Action::Send(session, Frame::Refused(UNREACHABLE)));
                actions.push(Action::Close(session));
                return;
            }
            stream_of(&mut self.streams, &key).instance = instance;
        }
        let stream = self.streams.get_mut(&key).unwrap();
        // A stale epoch, or another run's stream bytes, is refused.
        if epoch <= stream.epoch || stream.instance != instance {
            actions.push(Action::Send(session, Frame::Refused(DENIED)));
            actions.push(Action::Close(session));
            return;
        }
        // The previous session is fenced.
        if let Some(old) = stream.session.replace(session)
            && old != session
        {
            self.sessions.remove(&old);
            actions.push(Action::Close(old));
        }
        stream.epoch = epoch;
        stream.gaps.clear();
        self.sessions.get_mut(&session).unwrap().stream = Some(key);
        actions.push(Action::Send(
            session,
            Frame::Opened {
                epoch,
                incarnation: self.journal.incarnation(),
                applied: stream.applied,
                reconciles: self.journal.durable(),
            },
        ));
    }

    fn acknowledge(&mut self, key: &StreamKey, acknowledged: u64) {
        let stream = self.streams.get_mut(key).unwrap();
        // Only outcomes the caller holds can be dropped: decided, finished positions.
        let mut upto = stream.acknowledged;
        while upto < acknowledged.min(stream.applied)
            && matches!(
                stream.decided.get(&(upto + 1)).map(|d| &d.state),
                Some(State::Done { .. })
            )
        {
            upto += 1;
        }
        if upto > stream.acknowledged {
            stream.acknowledged = upto;
            stream.decided.retain(|p, _| *p > upto);
            // A lost acknowledgement only keeps outcomes longer.
            let _ = self.journal.append(&Entry::Acknowledged {
                stream: key.clone(),
                position: upto,
            });
        }
    }

    fn outcome_frame(stream: &Stream, position: u64, decided: &Decided) -> Option<Frame> {
        let State::Done {
            end,
            callee,
            missing,
        } = &decided.state
        else {
            return None;
        };
        Some(Frame::Outcome(Box::new(Outcome {
            epoch: stream.epoch,
            position,
            key: decided.key,
            digest: decided.digest,
            end: end.clone(),
            callee: *callee,
            missing: missing.clone(),
        })))
    }

    /// Send the stored outcome of `r`'s position to `session`, if it has one.
    fn resend(&self, session: SessionId, key: &StreamKey, r: CallRef, actions: &mut Vec<Action>) {
        let stream = &self.streams[key];
        if let Some(decided) = stream.decided.get(&r.position)
            && let Some(frame) = Self::outcome_frame(stream, r.position, decided)
        {
            actions.push(Action::Send(session, frame));
        }
    }

    fn status(
        &mut self,
        session: SessionId,
        key: &StreamKey,
        r: CallRef,
        actions: &mut Vec<Action>,
    ) {
        let stream = &self.streams[key];
        if r.position <= stream.acknowledged {
            return;
        }
        match stream.decided.get(&r.position) {
            Some(d) if d.key == r.key => match d.state {
                State::Done { .. } => self.resend(session, key, r, actions),
                _ => actions.push(Action::Send(session, Frame::Running(r))),
            },
            _ => actions.push(Action::Send(session, Frame::Unknown(r))),
        }
    }

    fn refuse(
        session: SessionId,
        epoch: u64,
        call: &Call,
        digest: [u8; 32],
        actions: &mut Vec<Action>,
    ) {
        actions.push(Action::Send(
            session,
            Frame::Outcome(Box::new(Outcome {
                epoch,
                position: call.position,
                key: call.key,
                digest,
                end: CallOutcome::NotRun(REJECTED),
                callee: None,
                missing: vec![],
            })),
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        session: SessionId,
        key: &StreamKey,
        caller: &Caller,
        call: Call,
        place: &mut dyn Place,
        digests: &dyn Digests,
        actions: &mut Vec<Action>,
    ) {
        self.acknowledge(key, call.acknowledged);
        let digest = digests.digest(&call);
        let stream = self.streams.get_mut(key).unwrap();
        let epoch = stream.epoch;
        let position = call.position;
        if position <= stream.acknowledged {
            return Self::refuse(session, epoch, &call, digest, actions);
        }
        if position <= stream.applied {
            let decided = &stream.decided[&position];
            if decided.digest != digest || decided.key != call.key {
                return Self::refuse(session, epoch, &call, digest, actions);
            }
            // A duplicate: its outcome now if it has one, else when its callee ends.
            return self.resend(session, key, call.reference(), actions);
        }
        if position > stream.applied + 1 {
            match stream.gaps.get(&position) {
                Some(&(k, d)) if (k, d) != (call.key, digest) => (),
                _ => {
                    stream.gaps.insert(position, (call.key, digest));
                }
            }
            return Self::refuse(session, epoch, &call, digest, actions);
        }
        // The next position: decide it.
        if stream.gaps.remove(&position).is_some() {
            // Refused while it was a gap: that answer stands for whatever arrives here.
            self.decide_refused(
                key,
                position,
                call.key,
                digest,
                REJECTED,
                vec![],
                session,
                actions,
            );
        } else if call.key != call_key(&stream.instance, call.ordinal) {
            self.decide_refused(
                key,
                position,
                call.key,
                digest,
                REJECTED,
                vec![],
                session,
                actions,
            );
        } else {
            match place.admit(caller, &call) {
                Err(refusal) => self.decide_refused(
                    key,
                    position,
                    call.key,
                    digest,
                    refusal.reason,
                    refusal.missing,
                    session,
                    actions,
                ),
                Ok(_)
                    if self.running + self.queued.len() as u64
                        >= self.limits.runs + self.limits.queue =>
                {
                    self.decide_refused(
                        key,
                        position,
                        call.key,
                        digest,
                        OVERLOADED,
                        vec![],
                        session,
                        actions,
                    )
                }
                Ok(grants) => {
                    let admitted = Entry::Admitted {
                        stream: key.clone(),
                        position,
                        key: call.key,
                        digest,
                    };
                    if self.journal.append(&admitted).is_err() {
                        self.decide_refused(
                            key,
                            position,
                            call.key,
                            digest,
                            UNREACHABLE,
                            vec![],
                            session,
                            actions,
                        );
                    } else {
                        let run = self.next_run;
                        self.next_run += 1;
                        let stream = self.streams.get_mut(key).unwrap();
                        stream.applied = position;
                        stream.decided.insert(
                            position,
                            Decided {
                                key: call.key,
                                digest,
                                state: State::Queued(run),
                            },
                        );
                        let instance = stream.instance;
                        self.runs.insert(
                            run,
                            Run {
                                stream: key.clone(),
                                position,
                                start: Start {
                                    run,
                                    caller: caller.name.clone(),
                                    instance,
                                    digest,
                                    call,
                                    grants,
                                },
                            },
                        );
                        self.queued.push_back(run);
                        self.dispatch(actions);
                    }
                }
            }
        }
        // Gap refusals the decided position now reaches are decided too.
        loop {
            let stream = self.streams.get_mut(key).unwrap();
            let next = stream.applied + 1;
            let Some((k, d)) = stream.gaps.remove(&next) else {
                break;
            };
            self.decide_refused(key, next, k, d, REJECTED, vec![], session, actions);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn decide_refused(
        &mut self,
        key: &StreamKey,
        position: u64,
        call: [u8; 32],
        digest: [u8; 32],
        reason: u32,
        missing: Vec<[u8; 32]>,
        _session: SessionId,
        actions: &mut Vec<Action>,
    ) {
        let stream = self.streams.get_mut(key).unwrap();
        stream.applied = position;
        stream.decided.insert(
            position,
            Decided {
                key: call,
                digest,
                state: State::Queued(0),
            },
        );
        self.decide(
            key,
            position,
            CallOutcome::NotRun(reason),
            None,
            missing,
            actions,
        );
    }

    fn decide_end(
        &mut self,
        key: &StreamKey,
        position: u64,
        end: CallOutcome,
        callee: Option<[u8; 16]>,
        actions: &mut Vec<Action>,
    ) {
        self.decide(key, position, end, callee, vec![], actions)
    }

    /// Record the position's outcome and send it to the stream's current session.
    fn decide(
        &mut self,
        key: &StreamKey,
        position: u64,
        end: CallOutcome,
        callee: Option<[u8; 16]>,
        missing: Vec<[u8; 32]>,
        actions: &mut Vec<Action>,
    ) {
        let stream = self.streams.get_mut(key).unwrap();
        let decided = stream.decided.get_mut(&position).unwrap();
        // An outcome that cannot be journaled is still the outcome; a durable journal that
        // fails loses only the ability to answer a later status query from it.
        let _ = self.journal.append(&Entry::Decided {
            stream: key.clone(),
            position,
            key: decided.key,
            digest: decided.digest,
            end: end.clone(),
            callee,
        });
        decided.state = State::Done {
            end,
            callee,
            missing,
        };
        if let Some(session) = stream.session
            && let Some(frame) = Self::outcome_frame(stream, position, &stream.decided[&position])
        {
            actions.push(Action::Send(session, frame));
        }
    }

    /// Start queued callees while the executor has room.
    fn dispatch(&mut self, actions: &mut Vec<Action>) {
        while self.running < self.limits.runs {
            let Some(run) = self.queued.pop_front() else {
                break;
            };
            let Run {
                stream,
                position,
                start,
                ..
            } = &self.runs[&run];
            let decided = self
                .streams
                .get_mut(stream)
                .unwrap()
                .decided
                .get_mut(position)
                .unwrap();
            decided.state = State::Running {
                run,
                forwarded: false,
            };
            self.running += 1;
            actions.push(Action::Start(start.clone()));
        }
    }

    /// The executor reports a callee's end: its outcome, and its run instance when it ran.
    pub fn ended(&mut self, run: RunId, end: CallOutcome, callee: Option<[u8; 16]>) -> Vec<Action> {
        let mut actions = vec![];
        let Some(Run {
            stream, position, ..
        }) = self.runs.remove(&run)
        else {
            return actions;
        };
        self.running -= 1;
        self.decide_end(&stream, position, end, callee, &mut actions);
        self.dispatch(&mut actions);
        actions
    }

    /// Whether any callee is queued or running.
    pub fn busy(&self) -> bool {
        self.running > 0 || !self.queued.is_empty()
    }
}
