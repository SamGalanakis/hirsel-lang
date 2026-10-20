//! A caller's `remote.tls@1` provider (RUN-4, RUN-6, RUN-7, RUN-9, RUN-13), with no I/O:
//! one runner's stream of calls to its host. The driver feeds it submitted calls,
//! cancellations, connection events, frames and the time, and it answers with
//! [`ClientAction`]s, among them each call's completion.
//!
//! **Commit point.** A call is sent when its `Call` frame is handed to the transport. Before
//! that, every ending proves no effect: a connection that cannot be opened is
//! `NotRun(Unreachable())`, a session the host refuses is `NotRun(reason)`, and a
//! cancellation is `Canceled(cause)`.
//!
//! **After a lost connection** the provider never sends a `Call` again. It reconnects (up
//! to `reconnect` attempts) at the next epoch, which fences everything the old session
//! carried, and resolves each sent call from the host's answer:
//! - when the host's memory of the stream is continuous (the same incarnation) or durable
//!   (it reconciles), a position above the host's applied one was never decided and never
//!   will be, so the call is `NotRun(Unreachable())`, and a position at or below it is
//!   asked by `Status`: its outcome, `Running` (keep waiting), or `Unknown`;
//! - otherwise the host forgot the stream and every sent call is `InDoubt(Lost())`.
//!
//! Positions the host never decided are reused: the next call takes `applied + 1`.
//! Without a connection and with no attempts left, every sent call is `InDoubt(Lost())`.
//!
//! **Retries.** A call the host decided `NotRun(Unreachable())` or `NotRun(Overloaded())`,
//! or that never reached it, is re-sent at a new position while the runner's `retry`
//! attempts last (RUN-7); every other ending is final.
//!
//! **Timeouts and cancellation.** With `timeout_ns`, a sent call with no outcome that long
//! after its commit point is `InDoubt(TimedOut())`. A cancelled sent call forwards
//! `Cancel` and keeps waiting for its outcome: `Canceled` when the host proves the callee
//! never started, else the callee's own outcome (RUN-9).
use crate::frame::{Call, CallRef, Frame};
use hirsel_run_schema::{CallOutcome, RunnerCompletion};
use std::collections::BTreeMap;

const UNREACHABLE: u32 = 2;
const OVERLOADED: u32 = 3;
const LOST: u32 = 0;
const TIMED_OUT: u32 = 1;

/// A runner entry's settings this provider honors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    /// Bound on the wait for an outcome after the commit point.
    pub timeout_ns: Option<u64>,
    /// Reconnection attempts after a lost or failed connection.
    pub reconnect: u32,
    /// Re-sends of a request that ended in `NotRun(Unreachable | Overloaded)`.
    pub retry: u32,
    /// The entry requires a host that records call keys durably (`reconcile`).
    pub reconcile: bool,
}

/// What the provider asks of its transport and its caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientAction {
    /// Open a connection; answer with [`Client::connected`] or [`Client::lost`].
    Connect,
    Send(Frame),
    /// Close the connection.
    Disconnect,
    /// The call submitted under the token ended.
    Complete(u64, RunnerCompletion),
}

#[derive(Clone, Debug)]
struct Pending {
    call: Call,
    digest: [u8; 32],
    /// The commit point's time, once sent.
    sent: Option<u64>,
    cancel: Option<u32>,
    retries: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Conn {
    Down,
    Connecting,
    /// Connected, `Open` sent.
    Opening,
    Open,
}

/// One runner's stream at its host.
pub struct Client {
    stream: [u8; 32],
    instance: [u8; 16],
    settings: Settings,
    conn: Conn,
    epoch: u64,
    incarnation: Option<[u8; 16]>,
    next: u64,
    /// Every outcome up to here is held, and the host was told so.
    acknowledged: u64,
    /// Positions above `acknowledged` whose outcome is held.
    held: std::collections::BTreeSet<u64>,
    attempts: u32,
    pending: BTreeMap<u64, Pending>,
    /// Submission order of the calls not yet sent.
    unsent: Vec<u64>,
    now: u64,
}

impl Client {
    pub fn new(stream: [u8; 32], instance: [u8; 16], settings: Settings) -> Self {
        Self {
            stream,
            instance,
            settings,
            conn: Conn::Down,
            epoch: 0,
            incarnation: None,
            next: 1,
            acknowledged: 0,
            held: Default::default(),
            attempts: settings.reconnect,
            pending: BTreeMap::new(),
            unsent: vec![],
            now: 0,
        }
    }

    /// Whether any call is unfinished.
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Submit a call under `token`: `call` carries the request (its epoch, position and
    /// acknowledgement are the provider's), `digest` its request digest.
    pub fn submit(&mut self, token: u64, call: Call, digest: [u8; 32]) -> Vec<ClientAction> {
        let mut actions = vec![];
        self.pending.insert(
            token,
            Pending {
                call,
                digest,
                sent: None,
                cancel: None,
                retries: self.settings.retry,
            },
        );
        self.unsent.push(token);
        self.pump(&mut actions);
        actions
    }

    /// Start a connection when one is needed, or send what waits on an open one.
    fn pump(&mut self, actions: &mut Vec<ClientAction>) {
        match self.conn {
            Conn::Down if !self.unsent.is_empty() => {
                self.conn = Conn::Connecting;
                actions.push(ClientAction::Connect);
            }
            Conn::Open => {
                for token in std::mem::take(&mut self.unsent) {
                    let position = self.next;
                    self.next += 1;
                    let pending = self.pending.get_mut(&token).unwrap();
                    pending.call.epoch = self.epoch;
                    pending.call.position = position;
                    pending.call.acknowledged = self.acknowledged;
                    // The commit point.
                    pending.sent = Some(self.now);
                    actions.push(ClientAction::Send(Frame::Call(Box::new(
                        pending.call.clone(),
                    ))));
                }
            }
            _ => (),
        }
    }

    fn complete(
        &mut self,
        token: u64,
        outcome: CallOutcome,
        callee: Option<[u8; 16]>,
        actions: &mut Vec<ClientAction>,
    ) {
        let Some(pending) = self.pending.remove(&token) else {
            return;
        };
        self.unsent.retain(|t| *t != token);
        if pending.sent.is_some() {
            self.hold(pending.call.position);
        }
        actions.push(ClientAction::Complete(
            token,
            RunnerCompletion { outcome, callee },
        ));
    }

    fn hold(&mut self, position: u64) {
        if position > self.acknowledged {
            self.held.insert(position);
        }
        while self.held.remove(&(self.acknowledged + 1)) {
            self.acknowledged += 1;
        }
    }

    /// A call ended without running: re-send it while its retries last.
    fn not_run(&mut self, token: u64, reason: u32, actions: &mut Vec<ClientAction>) {
        let retry = matches!(reason, UNREACHABLE | OVERLOADED)
            && self.pending[&token].retries > 0
            && self.pending[&token].cancel.is_none();
        if retry {
            let pending = self.pending.get_mut(&token).unwrap();
            pending.retries -= 1;
            if pending.sent.take().is_some() {
                let position = pending.call.position;
                self.hold(position);
            }
            self.unsent.push(token);
            self.pump(actions);
        } else {
            self.complete(token, CallOutcome::NotRun(reason), None, actions);
        }
    }

    fn token_at(&self, r: &CallRef) -> Option<u64> {
        self.pending
            .iter()
            .find(|(_, p)| p.sent.is_some() && p.call.position == r.position && p.call.key == r.key)
            .map(|(t, _)| *t)
    }

    /// The transport opened a connection.
    pub fn connected(&mut self) -> Vec<ClientAction> {
        self.conn = Conn::Opening;
        self.epoch += 1;
        vec![ClientAction::Send(Frame::Open {
            stream: self.stream,
            instance: self.instance,
            epoch: self.epoch,
        })]
    }

    /// The connection failed or was lost.
    pub fn lost(&mut self) -> Vec<ClientAction> {
        let mut actions = vec![];
        if self.conn == Conn::Down {
            return actions;
        }
        self.conn = Conn::Down;
        if self.attempts > 0 && self.busy() {
            self.attempts -= 1;
            self.conn = Conn::Connecting;
            actions.push(ClientAction::Connect);
            return actions;
        }
        self.fail(&mut actions);
        actions
    }

    /// No connection is to be had: unsent calls never ran, sent ones may have.
    fn fail(&mut self, actions: &mut Vec<ClientAction>) {
        self.attempts = self.settings.reconnect;
        let tokens: Vec<u64> = self.pending.keys().copied().collect();
        for token in tokens {
            match self.pending[&token].sent {
                None => {
                    // A retry needs a connection, which there is none of.
                    self.pending.get_mut(&token).unwrap().retries = 0;
                    self.not_run(token, UNREACHABLE, actions);
                }
                Some(_) => self.complete(token, CallOutcome::InDoubt(LOST), None, actions),
            }
        }
    }

    /// Cancel the call submitted under `token` with cause `cause` (RUN-9).
    pub fn cancel(&mut self, token: u64, cause: u32) -> Vec<ClientAction> {
        let mut actions = vec![];
        let Some(pending) = self.pending.get_mut(&token) else {
            return actions;
        };
        if pending.cancel.is_some() {
            return actions;
        }
        pending.cancel = Some(cause);
        match pending.sent {
            // Not yet sent: nothing ran.
            None => self.complete(token, CallOutcome::Canceled(cause), None, &mut actions),
            Some(_) if self.conn == Conn::Open => actions.push(ClientAction::Send(Frame::Cancel(
                pending.call.reference(),
                cause,
            ))),
            // Forwarded once the session reopens and the call is still running.
            Some(_) => (),
        }
        actions
    }

    /// The time advanced to `now` (nanoseconds on the provider's clock).
    pub fn tick(&mut self, now: u64) -> Vec<ClientAction> {
        self.now = now;
        let mut actions = vec![];
        let Some(timeout) = self.settings.timeout_ns else {
            return actions;
        };
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.sent.is_some_and(|at| now.saturating_sub(at) >= timeout))
            .map(|(t, _)| *t)
            .collect();
        for token in expired {
            self.complete(token, CallOutcome::InDoubt(TIMED_OUT), None, &mut actions);
        }
        actions
    }

    /// The earliest time a sent call times out.
    pub fn deadline(&self) -> Option<u64> {
        let timeout = self.settings.timeout_ns?;
        self.pending
            .values()
            .filter_map(|p| p.sent.map(|at| at.saturating_add(timeout)))
            .min()
    }

    /// End the stream's session: the caller holds everything it will.
    pub fn close(&mut self) -> Vec<ClientAction> {
        if self.conn != Conn::Open {
            return vec![];
        }
        self.conn = Conn::Down;
        vec![
            ClientAction::Send(Frame::Close {
                epoch: self.epoch,
                acknowledged: self.acknowledged,
            }),
            ClientAction::Disconnect,
        ]
    }

    /// One frame from the host.
    pub fn frame(&mut self, frame: Frame) -> Vec<ClientAction> {
        let mut actions = vec![];
        // A frame of an older session is discarded.
        if frame.epoch().is_some_and(|e| e != self.epoch) {
            return actions;
        }
        match frame {
            Frame::Opened {
                incarnation,
                applied,
                reconciles,
                ..
            } if self.conn == Conn::Opening => {
                self.opened(incarnation, applied, reconciles, &mut actions)
            }
            Frame::Refused(reason) if matches!(self.conn, Conn::Opening | Conn::Open) => {
                self.conn = Conn::Down;
                actions.push(ClientAction::Disconnect);
                let tokens: Vec<u64> = self.pending.keys().copied().collect();
                for token in tokens {
                    match self.pending[&token].sent {
                        None => {
                            self.pending.get_mut(&token).unwrap().retries = 0;
                            self.not_run(token, reason, &mut actions)
                        }
                        Some(_) => {
                            self.complete(token, CallOutcome::InDoubt(LOST), None, &mut actions)
                        }
                    }
                }
            }
            Frame::Outcome(outcome) if self.conn == Conn::Open => {
                let r = CallRef {
                    epoch: outcome.epoch,
                    position: outcome.position,
                    key: outcome.key,
                };
                let Some(token) = self.token_at(&r) else {
                    return actions;
                };
                if self.pending[&token].digest != outcome.digest {
                    return actions;
                }
                match outcome.end {
                    CallOutcome::NotRun(reason) => self.not_run(token, reason, &mut actions),
                    end => self.complete(token, end, outcome.callee, &mut actions),
                }
            }
            Frame::Running(r) if self.conn == Conn::Open => {
                if let Some(token) = self.token_at(&r)
                    && let Some(cause) = self.pending[&token].cancel
                {
                    actions.push(ClientAction::Send(Frame::Cancel(r, cause)));
                }
            }
            Frame::Unknown(r) if self.conn == Conn::Open => {
                // The host decided this position for another key: what ran is unknown.
                if let Some(token) = self.token_at(&r) {
                    self.complete(token, CallOutcome::InDoubt(LOST), None, &mut actions);
                }
            }
            // An acknowledgement only proves the cancellation arrived; the outcome follows.
            _ => (),
        }
        actions
    }

    fn opened(
        &mut self,
        incarnation: [u8; 16],
        applied: u64,
        reconciles: bool,
        actions: &mut Vec<ClientAction>,
    ) {
        if self.settings.reconcile && !reconciles {
            // The entry promises status answers this host cannot give.
            self.conn = Conn::Down;
            actions.push(ClientAction::Disconnect);
            let tokens: Vec<u64> = self.pending.keys().copied().collect();
            for token in tokens {
                match self.pending[&token].sent {
                    None => {
                        self.pending.get_mut(&token).unwrap().retries = 0;
                        self.not_run(token, 0, actions)
                    }
                    Some(_) => self.complete(token, CallOutcome::InDoubt(LOST), None, actions),
                }
            }
            return;
        }
        let known =
            self.incarnation == Some(incarnation) || (reconciles && self.incarnation.is_some());
        self.incarnation = Some(incarnation);
        self.conn = Conn::Open;
        self.attempts = self.settings.reconnect;
        // A host claiming positions this stream never sent is not this stream's host.
        if applied >= self.next {
            self.conn = Conn::Down;
            actions.push(ClientAction::Disconnect);
            self.fail(actions);
            return;
        }
        // Positions above the host's applied one were never decided, and the new epoch
        // fences them: they are reused, and a host that forgot the stream starts over.
        self.next = applied + 1;
        self.held.retain(|p| *p <= applied);
        self.acknowledged = self.acknowledged.min(applied);
        let sent: Vec<(u64, u64)> = self
            .pending
            .iter()
            .filter_map(|(t, p)| p.sent.map(|_| (*t, p.call.position)))
            .collect();
        for (token, position) in sent {
            if !known {
                self.pending.get_mut(&token).unwrap().sent = None;
                self.complete(token, CallOutcome::InDoubt(LOST), None, actions);
            } else if position > applied {
                // Never decided: it never ran.
                self.pending.get_mut(&token).unwrap().sent = None;
                self.not_run(token, UNREACHABLE, actions);
            } else {
                // A sent call keeps its position; its references carry the new epoch.
                let pending = self.pending.get_mut(&token).unwrap();
                pending.call.epoch = self.epoch;
                actions.push(ClientAction::Send(Frame::Status(pending.call.reference())));
            }
        }
        // Calls cancelled while no session was open forward their cancellation through
        // the status answer (`Running`); the unsent ones go now.
        self.pump(actions);
    }
}
