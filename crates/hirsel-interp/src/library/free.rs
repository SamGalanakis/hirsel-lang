//! Free mode (`tasks.free@1`) at width 1 and one permit (design §5, lane F0): the
//! interpreter's op phases, pending `Shared` releases, recording and replay, over a
//! [`hirsel_free::session::Session`].
//!
//! - **Ops.** The free ops are the task library's synchronising operations: channel open,
//!   send, receive, dup and either endpoint's close (its drop), mutex creation, the take and
//!   put around a `mutex.with` body, atomic loads and stores, `shared.thaw`, spawn, join,
//!   `await_any`, scope entry and exit, the other task and set operations, cancellation
//!   checks, `out.print`/`err.print` (host writes on the `stdout`/`stderr` keys) and each
//!   task's end. Every one yields, so a seeded schedule interleaves tasks at every op as
//!   simultaneous events at width N would.
//! - **Phases.** An op that cannot wait runs entered and done in one section and publishes
//!   (k, done). A waiting op (receive, take, join, `await_any`, `cancel_all`, scope exit)
//!   publishes (k, entered) when it parks and (k, done) when a poll completes it. A host
//!   write runs entered, claimed, ended 0 and done. A task's end runs entered, then done.
//! - **Releases.** A `Shared` release that does not find its root's count at 1 is deferred to
//!   the releasing task's pending list, applied in its next op's entered phase. A root that an
//!   entered phase brings to 0 is walked as the tail of that phase, releasing nested roots at
//!   once (Amendment F0-1); a root a segment frees is walked by the evaluator, whose nested
//!   releases follow the segment rule. A dup never cancels a pending release here: counts are
//!   unobservable between ops, so that coalescing only saves memory.
//! - **Replay.** A phase the cut does not admit parks its task for good (a final park), as
//!   does reaching the cut. A phase whose record names a predecessor (a version, a key epoch,
//!   an `await_any` member, thaw count 1) waits replay-pending: the op parks and its poll
//!   acts once the predecessor happened. An endpoint close, which a drop performs and which
//!   cannot park, publishes (k, done) in program order and applies its sub-event when its
//!   version is reached. Outcomes the tape holds are substituted.

use super::dispatch::variant;
use super::{Library, error, ok};
use crate::value::SharedRoot;
use crate::{BlockedWait, Error, OperationContext, OperationResult, Value, invalid};
use hirsel_free::pending::Pending;
use hirsel_free::phase::Phase;
use hirsel_free::replay::Diverged;
use hirsel_free::session::{Advance, Claim, Handle, HostWrite, Session};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const CHANNEL: u8 = 1;
pub(super) const MUTEX: u8 = 2;

/// The outcome a free-mode tape names for an interpreted run's end.
pub fn outcome_name(outcome: &crate::Outcome) -> &'static str {
    match outcome {
        crate::Outcome::Returned { .. } => "returned",
        crate::Outcome::Failed(_) => "failed",
        crate::Outcome::BudgetExceeded(kind) => kind.name(),
        crate::Outcome::Blocked(_) => "blocked",
        _ => "stopped",
    }
}

/// A run's free-mode state.
pub(super) struct Free {
    pub session: Session,
    native: bool,
    root_observation: Option<Vec<u8>>,
    root_pending: bool,
    /// Pending releases by task id.
    pending: BTreeMap<u64, Pending<SharedRoot>>,
    /// Tokens of tasks parked for good.
    finals: BTreeSet<u64>,
    /// Tokens of parked waiting ops: their poll publishes (k, done).
    waits: BTreeSet<u64>,
    /// Ops waiting replay-pending, by token.
    retries: BTreeMap<u64, Retry>,
    /// Replay: endpoint closes waiting for their version, in program order.
    closes: Vec<Close>,
    /// Calling ops publish Done only after their admitted child rests return.
    after_rests: BTreeSet<u64>,
    /// A native dead take publishes before the separate cause operation.
    causes: BTreeMap<u64, u64>,
}

/// An op that waits replay-pending, with what its action needs.
pub(super) enum Retry {
    Send {
        channel: u64,
        payload: Value,
    },
    Dup {
        channel: u64,
    },
    Thaw {
        root: SharedRoot,
    },
    Write(FreeWrite),
    Cause {
        mutex: u64,
        result: Box<OperationResult>,
    },
}

/// A free-mode host write between its claim and its end.
pub struct FreeWrite {
    request: HostWrite,
    sink: &'static str,
    bytes: Vec<u8>,
}

/// How far [`Library::free_print_begin`] took a print.
pub enum FreePrint {
    /// The op's result (done, parked, or waiting replay-pending).
    Done(OperationResult),
    /// Claimed: make the host write now.
    Write(FreeWrite),
}

struct Close {
    path: Vec<u32>,
    op: u64,
    object: (u8, u64),
    kind: &'static str,
    endpoint: u64,
}

fn diverged(error: Diverged) -> Error {
    invalid(format!("free-mode {error}"))
}

/// An atomic cell's bits.
pub(super) fn scalar_bits(value: &Value) -> Result<u128, Error> {
    Ok(match value {
        Value::Bool(b) => u128::from(*b),
        Value::Integer(i) => i.bits(),
        Value::F32(bits) => u128::from(*bits),
        Value::F64(bits) => u128::from(*bits),
        _ => return Err(invalid("atomic payload must be scalar")),
    })
}

/// A value of `template`'s type with `bits`.
fn from_bits(template: &Value, bits: u128) -> Result<Value, Error> {
    let wrong = || invalid("free-mode replay: a recorded load does not fit its cell");
    Ok(match template {
        Value::Bool(_) if bits < 2 => Value::Bool(bits == 1),
        Value::Integer(i) => {
            Value::Integer(crate::value::Integer::new(i.kind(), bits).ok_or_else(wrong)?)
        }
        Value::F32(_) => Value::F32(u32::try_from(bits).map_err(|_| wrong())?),
        Value::F64(_) => Value::F64(u64::try_from(bits).map_err(|_| wrong())?),
        _ => return Err(wrong()),
    })
}

/// Walk a graph that an entered phase freed: each nested root is released at once, and one
/// that reaches 0 is walked the same way. A frozen graph holds no handle and no finalizer
/// (MEM-11), so the rest is plain data.
fn walk_in_phase(graph: Value) -> Result<(), Error> {
    let mut work = vec![graph];
    while let Some(mut value) = work.pop() {
        if let Value::Shared(root) = &value {
            if let Some(graph) = root.release()? {
                work.push(graph);
            }
            continue;
        }
        let fields = match &mut value {
            Value::Map { entries: b, .. }
            | Value::Set { entries: b, .. }
            | Value::Sequence { elements: b, .. } => b.take_unique(),
            value => value.fields_mut().map(std::mem::take),
        };
        work.extend(fields.into_iter().flatten().flatten());
    }
    Ok(())
}

impl Free {
    pub(super) fn new(session: Session) -> Free {
        Free {
            session,
            native: false,
            root_observation: None,
            root_pending: false,
            pending: BTreeMap::new(),
            finals: BTreeSet::new(),
            waits: BTreeSet::new(),
            retries: BTreeMap::new(),
            closes: Vec::new(),
            after_rests: BTreeSet::new(),
            causes: BTreeMap::new(),
        }
    }
}

/// What starting an op decided.
pub(super) enum Enter {
    /// Free mode is off: run the op as usual.
    Off,
    /// The op runs; its entry is applied.
    Run,
    /// The task parks for good instead.
    Park(OperationResult),
}

impl Library {
    /// Run this library in free mode over `session`. The scheduler interleaves tasks at every
    /// op under `seed`.
    pub fn enable_free(&mut self, session: Session, seed: u64) {
        let mut session = session;
        session.start(&[]);
        self.tasks.schedule = hirsel_kernels::schedule::Schedule::new(
            hirsel_kernels::schedule::Algorithm::Random,
            seed,
        );
        self.tasks.root_priority = self.tasks.schedule.admit();
        self.tasks.free_rests = true;
        self.free = Some(Free::new(session));
    }

    /// Native recordings carry typed host envelopes and a canonical root outcome.
    pub fn enable_native_free(&mut self, session: Session, seed: u64) {
        self.enable_free(session, seed);
        self.free.as_mut().unwrap().native = true;
    }

    /// None is a shielded request; Some is its observed cancellation check.
    pub fn host_cancellation(&self) -> Option<Option<u32>> {
        (!self.tasks.shielded).then(|| self.tasks.canceled())
    }

    pub fn native_free(&self) -> bool {
        self.free.as_ref().is_some_and(|free| free.native)
    }

    /// Short session access; the caller retains no borrow across evaluator suspension.
    pub fn with_free_session<T>(
        &mut self,
        action: impl FnOnce(&mut Session) -> Result<T, String>,
    ) -> Result<T, Error> {
        let session = &mut self
            .free
            .as_mut()
            .ok_or_else(|| invalid("no Free phase session"))?
            .session;
        action(session).map_err(|e| invalid(format!("free-mode {e}")))
    }

    /// Apply the same entered root tail as a native host request.
    pub fn begin_native_host(&mut self) -> Result<bool, Error> {
        self.free_apply_closes()?;
        let path = self.tasks.path();
        let session = &mut self.free.as_mut().unwrap().session;
        session.begin(&path);
        if !session.admits(&path, Phase::Entered) {
            return Ok(false);
        }
        self.free_apply_pending(self.tasks.current)?;
        self.tasks.yield_now = true;
        Ok(true)
    }

    /// The explicit cancellation observation preceding one bounded runner
    /// poll. Replay forces the recorded cause, as TaskCanceled does natively.
    pub fn native_runner_poll_cancellation(
        &mut self,
        context: &OperationContext<'_>,
    ) -> Result<Option<Option<u32>>, Error> {
        if let Enter::Park(_) = self.free_enter(Phase::Done, context)? {
            return Ok(None);
        }
        let cause = self.free_canceled()?;
        let result = self.free_done(OperationResult::returned(Value::Unit), context)?;
        if matches!(result, OperationResult::Park { .. }) {
            Ok(None)
        } else {
            Ok(Some(cause))
        }
    }

    pub fn native_free_cut(&mut self, context: &OperationContext<'_>) -> OperationResult {
        self.free_final(context)
    }

    pub fn native_root_pending(&self) -> bool {
        self.free.as_ref().is_some_and(|free| free.root_pending)
    }

    pub fn take_native_root_observation(&mut self) -> Option<Vec<u8>> {
        self.free
            .as_mut()
            .and_then(|free| free.root_observation.take())
    }

    /// Whether a free-mode replay drives this run.
    pub fn free_replay(&self) -> bool {
        self.free
            .as_ref()
            .is_some_and(|free| free.session.is_replay())
    }

    /// End the live free-mode recording with the run's `outcome`.
    pub fn free_stop(&mut self, outcome: &str) {
        if let Some(free) = &mut self.free {
            free.session.stop(outcome, None);
        }
    }

    /// The run's free-mode session, once the run ended.
    pub fn take_free(&mut self) -> Option<Session> {
        self.free.take().map(|free| free.session)
    }

    pub(super) fn path_of(&self, task: u64) -> Vec<u32> {
        if task == self.tasks.current {
            self.tasks.path()
        } else {
            self.tasks.child_path(task).unwrap_or_default()
        }
    }

    /// A release of `root` by the running task: deferred unless it is the last root.
    pub(super) fn free_defer_release(&mut self, root: &SharedRoot) -> bool {
        let current = self.tasks.current;
        let Some(free) = &mut self.free else {
            return false;
        };
        if root.count() <= 1 {
            return false;
        }
        free.pending.entry(current).or_default().defer(root.clone());
        true
    }

    /// An entered phase: apply the task's pending releases, walking what they free.
    fn free_apply_pending(&mut self, task: u64) -> Result<(), Error> {
        let Some(free) = &mut self.free else {
            return Ok(());
        };
        let pending = free.pending.remove(&task).unwrap_or_default().take();
        for (root, n) in pending {
            for _ in 0..n {
                if let Some(graph) = root.release()? {
                    walk_in_phase(graph)?;
                }
            }
        }
        Ok(())
    }

    /// A final park: the task waits for good.
    pub(super) fn free_final(&mut self, context: &OperationContext<'_>) -> OperationResult {
        let token = self.next_callback;
        self.next_callback += 1;
        self.free.as_mut().unwrap().finals.insert(token);
        OperationResult::Park {
            token,
            wait: BlockedWait {
                spawn_path: self.tasks.path(),
                anchor: context.anchor().unwrap().clone(),
                kind: "free.cut".into(),
                targets: vec![],
            },
        }
    }

    /// Begin an op of the running task whose first phase is `first` (entered for a waiting
    /// op or a host write, done for an op that cannot wait): admission, then its entry.
    pub(super) fn free_enter(
        &mut self,
        first: Phase,
        context: &OperationContext<'_>,
    ) -> Result<Enter, Error> {
        self.free_enter_phase(first, false, context)
    }

    pub(super) fn free_enter_call(
        &mut self,
        context: &OperationContext<'_>,
    ) -> Result<Enter, Error> {
        self.free_enter_phase(Phase::Entered, true, context)
    }

    fn free_enter_phase(
        &mut self,
        first: Phase,
        calls: bool,
        context: &OperationContext<'_>,
    ) -> Result<Enter, Error> {
        if self.free.is_none() {
            return Ok(Enter::Off);
        }
        self.free_apply_closes()?;
        self.tasks.yield_now = true;
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        free.session.begin(&path);
        if !free.session.admits(&path, first) {
            return Ok(Enter::Park(self.free_final(context)));
        }
        self.free_apply_pending(self.tasks.current)?;
        if calls {
            self.free_publish(Phase::Entered)?;
        } else if first == Phase::Entered && !self.free_admits(Phase::Done) {
            // At (k, entered) it runs op k's entry and then waits in op k for good.
            self.free_publish(Phase::Entered)?;
            return Ok(Enter::Park(self.free_final(context)));
        }
        Ok(Enter::Run)
    }

    /// Begin a host request of the running task: admission of its entry, then the entry.
    /// `None` when free mode is off.
    pub(super) fn free_enter_host(
        &mut self,
        context: &OperationContext<'_>,
    ) -> Result<Option<Result<(), OperationResult>>, Error> {
        if self.free.is_none() {
            return Ok(None);
        }
        self.free_apply_closes()?;
        self.tasks.yield_now = true;
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        free.session.begin(&path);
        if !free.session.admits(&path, Phase::Entered) {
            return Ok(Some(Err(self.free_final(context))));
        }
        self.free_apply_pending(self.tasks.current)?;
        Ok(Some(Ok(())))
    }

    pub(super) fn free_admits(&self, phase: Phase) -> bool {
        let path = self.tasks.path();
        self.free
            .as_ref()
            .is_none_or(|free| free.session.admits(&path, phase))
    }

    fn free_publish(&mut self, phase: Phase) -> Result<(), Error> {
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        free.session.publish(&path, phase).map_err(diverged)
    }

    /// Publish (k, done) with the op's `result`; a replayed task at its cut parks for good
    /// before the segment that follows.
    pub(super) fn free_done(
        &mut self,
        result: OperationResult,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if self.free.is_none() {
            return Ok(result);
        }
        if self.tasks.has_rests(self.tasks.current) {
            self.free_publish(Phase::Entered)?;
            self.free
                .as_mut()
                .unwrap()
                .after_rests
                .insert(self.tasks.current);
            return Ok(result);
        }
        if !self.free_admits(Phase::Done) {
            return Ok(self.free_final(context));
        }
        let result = self.free_deadline_done(result)?;
        self.free_publish(Phase::Done)?;
        let path = self.tasks.path();
        if self.free.as_ref().unwrap().session.at_cut(&path) {
            return Ok(self.free_final(context));
        }
        let cause = self
            .free
            .as_mut()
            .unwrap()
            .causes
            .remove(&self.tasks.current);
        if let Some(mutex) = cause {
            return match self.free_enter(Phase::Done, context)? {
                Enter::Run => self.free_cause(mutex, result, context),
                Enter::Park(park) => Ok(park),
                Enter::Off => unreachable!("a native dead take has a Free session"),
            };
        }
        Ok(result)
    }

    /// The native take returns Dead before the caller reads the immutable cause.
    pub(super) fn free_dead_mutex(&mut self, mutex: u64) {
        if let Some(free) = &mut self.free
            && free.native
        {
            free.causes.insert(self.tasks.current, mutex);
        }
    }

    fn free_cause(
        &mut self,
        mutex: u64,
        result: OperationResult,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if self.free_enabled((MUTEX, mutex))? {
            self.free_touch((MUTEX, mutex), "cause")?;
            self.free_done(result, context)
        } else {
            Ok(self.free_retry(
                Retry::Cause {
                    mutex,
                    result: Box::new(result),
                },
                "mutex.cause",
                context,
            ))
        }
    }

    pub(super) fn free_rests_completed(
        &mut self,
        parent: u64,
        result: &mut OperationResult,
        context: &OperationContext<'_>,
    ) -> Result<(), Error> {
        if self
            .free
            .as_mut()
            .is_some_and(|free| free.after_rests.remove(&parent))
        {
            self.tasks.rests_returned(parent, result);
            let completed = std::mem::replace(result, OperationResult::returned(Value::Unit));
            *result = self.free_done(completed, context)?;
        }
        Ok(())
    }

    /// A waiting op's result: a park publishes (k, entered) and registers its poll; a
    /// completion publishes (k, done).
    pub(super) fn free_waiting(
        &mut self,
        result: OperationResult,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if self.free.is_none() {
            return Ok(result);
        }
        if let OperationResult::Park { token, .. } = &result {
            let token = *token;
            self.free_publish(Phase::Entered)?;
            self.free.as_mut().unwrap().waits.insert(token);
            return Ok(result);
        }
        self.free_done(result, context)
    }

    /// Park the running op replay-pending until its predecessor happened.
    pub(super) fn free_retry(
        &mut self,
        retry: Retry,
        kind: &str,
        context: &OperationContext<'_>,
    ) -> OperationResult {
        let token = self.next_callback;
        self.next_callback += 1;
        self.free.as_mut().unwrap().retries.insert(token, retry);
        OperationResult::Park {
            token,
            wait: BlockedWait {
                spawn_path: self.tasks.path(),
                anchor: context.anchor().unwrap().clone(),
                kind: format!("free.{kind}"),
                targets: vec![],
            },
        }
    }

    /// Whether sub-event 0 of the running op on `handle` is enabled: its version is reached
    /// and the task has no earlier close on the object still waiting for its own.
    pub(super) fn free_enabled(&self, handle: Handle) -> Result<bool, Error> {
        let Some(free) = &self.free else {
            return Ok(true);
        };
        let path = self.tasks.path();
        if free
            .closes
            .iter()
            .any(|close| close.path == path && close.object == handle)
        {
            return Ok(false);
        }
        free.session.enabled(&path, 0, handle).map_err(diverged)
    }

    pub(super) fn free_touch(&mut self, handle: Handle, outcome: &str) -> Result<(), Error> {
        let path = self.tasks.path();
        if let Some(free) = &mut self.free {
            free.session
                .touch(&path, 0, handle, outcome)
                .map_err(diverged)?;
        }
        Ok(())
    }

    pub(super) fn free_create(&mut self, handle: Handle) -> Result<(), Error> {
        let path = self.tasks.path();
        if let Some(free) = &mut self.free {
            free.session.create(&path, handle).map_err(diverged)?;
        }
        Ok(())
    }

    pub(super) fn free_receive(&mut self, channel: u64, outcome: &str) -> Result<(), Error> {
        let path = self.tasks.path();
        if let Some(free) = &mut self.free {
            free.session
                .receive(&path, (CHANNEL, channel), outcome)
                .map_err(diverged)?;
        }
        Ok(())
    }

    /// The cancellation the running task observes at an unshielded check: live, its own;
    /// replayed, the recorded one. Only a done phase checks, once per op.
    pub(super) fn free_canceled(&mut self) -> Result<Option<u32>, Error> {
        let live = self.tasks.canceled();
        if self.tasks.shielded {
            return Ok(live);
        }
        let path = self.tasks.path();
        match &mut self.free {
            None => Ok(live),
            Some(free) => free.session.cancel_check(&path, live).map_err(diverged),
        }
    }

    /// Whether the running task's next check observes a cancellation, without checking.
    pub(super) fn free_cancel_peek(&self) -> Option<u32> {
        let live = self.tasks.canceled();
        if self.tasks.shielded {
            return live;
        }
        match &self.free {
            None => live,
            Some(free) => free.session.cancel_peek(&self.tasks.path(), live),
        }
    }

    pub(super) fn free_load(&mut self, cell: &Value) -> Result<Value, Error> {
        let path = self.tasks.path();
        let Some(free) = &mut self.free else {
            return Ok(cell.clone());
        };
        let bits = free
            .session
            .load(&path, scalar_bits(cell)?)
            .map_err(diverged)?;
        from_bits(cell, bits)
    }

    /// `shared.thaw` in free mode: the bit, with a replayed `Ok` waiting for count 1.
    pub(super) fn free_thaw(
        &mut self,
        root: SharedRoot,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        let last = root.count() == 1;
        if !free.session.thaw_ready(&path, last) {
            return Ok(self.free_retry(Retry::Thaw { root }, "thaw", context));
        }
        let ok_bit = free.session.thaw(&path, last).map_err(diverged)?;
        let value = if ok_bit {
            match root.thaw()? {
                Ok(value) => ok(value),
                Err(_) => return Err(invalid("a last root refused its thaw")),
            }
        } else {
            error(Value::Shared(root))
        };
        let result = OperationResult::Complete {
            value,
            arguments: vec![],
            consumed: vec![0],
            released: vec![],
        };
        self.free_done(result, context)
    }

    /// A stdout or stderr text/byte write, up to its host call: `None` when free mode
    /// is off. [`FreePrint::Write`]: make the host write (live), then
    /// [`Library::free_print_end`]; a replay never gets there, since it makes no host call.
    pub fn free_print_begin(
        &mut self,
        name: &str,
        a: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<Option<FreePrint>, Error> {
        if self.native_free() {
            return Ok(None);
        }
        let Some(entry) = self.free_enter_host(context)? else {
            return Ok(None);
        };
        if let Err(park) = entry {
            return Ok(Some(FreePrint::Done(park)));
        }
        // Its cancellation is checked once, in the entered section.
        if let Some(cause) = self.free_canceled()? {
            let result = OperationResult::returned(error(variant(1, vec![variant(cause, vec![])])));
            return self
                .free_done(result, context)
                .map(|r| Some(FreePrint::Done(r)));
        }
        let sink = if name.starts_with("err.") {
            "stderr"
        } else {
            "stdout"
        };
        let bytes = super::dispatch::output_bytes(name, super::dispatch::arg(a, 1)?)?.to_vec();
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        let request = free.session.host_begin(&path, sink).map_err(diverged)?;
        self.free_write_claim(
            FreeWrite {
                request,
                sink,
                bytes,
            },
            context,
        )
        .map(Some)
    }

    /// A free-mode print's host write returned: the library's own write (its simulated
    /// stream and supplied faults), then its ended and done sections.
    pub fn free_print_end(
        &mut self,
        write: FreeWrite,
        name: &str,
        a: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let value = self.print(name, a, None, context)?;
        self.free_write_end(write, OperationResult::returned(value), context)
    }

    fn free_write_claim(
        &mut self,
        mut write: FreeWrite,
        context: &OperationContext<'_>,
    ) -> Result<FreePrint, Error> {
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        match free
            .session
            .host_claim(&path, &mut write.request)
            .map_err(diverged)?
        {
            Claim::Waiting => Ok(FreePrint::Done(self.free_retry(
                Retry::Write(write),
                "write",
                context,
            ))),
            Claim::Final => Ok(FreePrint::Done(self.free_final(context))),
            Claim::Call if self.free_replay() => {
                let result = OperationResult::returned(ok(Value::Unit));
                self.free_write_end(write, result, context)
                    .map(FreePrint::Done)
            }
            Claim::Call => Ok(FreePrint::Write(write)),
        }
    }

    fn free_write_end(
        &mut self,
        write: FreeWrite,
        result: OperationResult,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        match free
            .session
            .host_end(&path, &write.request, write.sink, &write.bytes)
            .map_err(diverged)?
        {
            Advance::Final => Ok(self.free_final(context)),
            Advance::Done if free.session.at_cut(&path) => Ok(self.free_final(context)),
            Advance::Done => Ok(result),
        }
    }

    /// A drop of a channel endpoint: a close op. Live it closes now; a replay that does not
    /// admit it skips it, and one whose version is not reached yet applies it later.
    pub(super) fn free_close(&mut self, kind: &'static str, endpoint: u64) -> Result<bool, Error> {
        if self.free.is_none() {
            return Ok(false);
        }
        self.free_apply_closes()?;
        let object = if kind == "Mutex" {
            (MUTEX, endpoint)
        } else if let Some(channel) = self.channels.endpoint_channel(endpoint) {
            (CHANNEL, channel)
        } else {
            return Ok(false);
        };
        let path = self.tasks.path();
        let free = self.free.as_mut().unwrap();
        let op = free.session.begin(&path);
        if !free.session.admits(&path, Phase::Done) {
            return Ok(true);
        }
        self.free_apply_pending(self.tasks.current)?;
        let outcome = format!("close {}", kind.to_lowercase());
        if self.free_enabled(object)? {
            self.free_touch(object, &outcome)?;
            if kind == "Mutex" {
                let released = self.drop_mutex(endpoint);
                self.channels.released.extend(released);
            } else {
                self.channels.close(kind, endpoint)?;
            }
        } else {
            self.free.as_mut().unwrap().closes.push(Close {
                path: path.clone(),
                op,
                object,
                kind,
                endpoint,
            });
        }
        self.free_publish(Phase::Done)?;
        Ok(true)
    }

    /// Replay: apply the deferred closes whose versions are reached, in program order.
    fn free_apply_closes(&mut self) -> Result<(), Error> {
        loop {
            let Some(free) = &mut self.free else {
                return Ok(());
            };
            let mut applied = None;
            let mut blocked: BTreeSet<(Vec<u32>, (u8, u64))> = BTreeSet::new();
            for (i, close) in free.closes.iter().enumerate() {
                let key = (close.path.clone(), close.object);
                if blocked.contains(&key) {
                    continue;
                }
                if free
                    .session
                    .enabled_at(&close.path, close.op, 0, close.object)
                    .map_err(diverged)?
                {
                    applied = Some(i);
                    break;
                }
                blocked.insert(key);
            }
            let Some(i) = applied else {
                return Ok(());
            };
            let close = free.closes.remove(i);
            free.session
                .touch_at(
                    &close.path,
                    close.op,
                    0,
                    close.object,
                    &format!("close {}", close.kind.to_lowercase()),
                )
                .map_err(diverged)?;
            if close.kind == "Mutex" {
                let released = self.drop_mutex(close.endpoint);
                self.channels.released.extend(released);
            } else {
                self.channels.close(close.kind, close.endpoint)?;
            }
        }
    }

    /// Whether `token` is a free-mode wait; its poll's result when it is.
    pub(super) fn free_poll(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<Option<OperationResult>>, Error> {
        if self.free.is_none() {
            return Ok(None);
        }
        self.free_apply_closes()?;
        let free = self.free.as_mut().unwrap();
        if free.finals.contains(&token) {
            return Ok(Some(None));
        }
        let Some(retry) = free.retries.remove(&token) else {
            return Ok(None);
        };
        let result = match retry {
            Retry::Send { channel, payload } => {
                if self.free_enabled((CHANNEL, channel))? {
                    Some(self.free_send(channel, payload, context)?)
                } else {
                    self.free_requeue(token, Retry::Send { channel, payload });
                    None
                }
            }
            Retry::Dup { channel } => {
                if self.free_enabled((CHANNEL, channel))? {
                    Some(self.free_dup(channel, context)?)
                } else {
                    self.free_requeue(token, Retry::Dup { channel });
                    None
                }
            }
            Retry::Thaw { root } => {
                let path = self.tasks.path();
                let last = root.count() == 1;
                if self.free.as_ref().unwrap().session.thaw_ready(&path, last) {
                    Some(self.free_thaw(root, context)?)
                } else {
                    self.free_requeue(token, Retry::Thaw { root });
                    None
                }
            }
            Retry::Cause { mutex, result } => {
                if self.free_enabled((MUTEX, mutex))? {
                    Some(self.free_cause(mutex, *result, context)?)
                } else {
                    self.free_requeue(token, Retry::Cause { mutex, result });
                    None
                }
            }
            Retry::Write(write) => match self.free_write_claim(write, context)? {
                FreePrint::Done(OperationResult::Park { token: next, .. })
                    if self.free.as_ref().unwrap().retries.contains_key(&next) =>
                {
                    // Still waiting: keep the original token.
                    let retry = self.free.as_mut().unwrap().retries.remove(&next).unwrap();
                    self.free_requeue(token, retry);
                    None
                }
                FreePrint::Done(result) => Some(result),
                FreePrint::Write(_) => {
                    return Err(invalid("a live free-mode write waited for its key"));
                }
            },
        };
        Ok(Some(result))
    }

    fn free_requeue(&mut self, token: u64, retry: Retry) {
        self.free.as_mut().unwrap().retries.insert(token, retry);
    }

    /// After a waiting op's poll completed it: publish (k, done).
    pub(super) fn free_polled(
        &mut self,
        token: u64,
        result: Option<OperationResult>,
        context: &OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let Some(free) = &mut self.free else {
            return Ok(result);
        };
        if !free.waits.contains(&token) {
            return Ok(result);
        }
        match result {
            Some(OperationResult::Park { token: next, wait }) => {
                free.waits.remove(&token);
                free.waits.insert(next);
                Ok(Some(OperationResult::Park { token: next, wait }))
            }
            Some(result) => {
                free.waits.remove(&token);
                self.free_done(result, context).map(Some)
            }
            None => Ok(None),
        }
    }

    /// `channel.send` once enabled: the sub-event and its outcome.
    pub(super) fn free_send(
        &mut self,
        channel: u64,
        payload: Value,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let (result, outcome) = self.channels.send(channel, payload)?;
        self.free_touch((CHANNEL, channel), &outcome)?;
        self.free_done(OperationResult::returned(result).transferring([2]), context)
    }

    /// `sender.dup` once enabled.
    pub(super) fn free_dup(
        &mut self,
        channel: u64,
        context: &OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let endpoint = self.channels.dup(channel);
        self.free_touch((CHANNEL, channel), "dup")?;
        self.free_done(OperationResult::returned(endpoint), context)
    }

    /// A task's end op: its entry applies the pending list (whose freed roots are walked in
    /// the phase, so no cleanup phase remains), then the done phase publishes the end.
    pub(super) fn free_end(&mut self, task: u64) -> Result<(), Error> {
        if self.free.is_none() {
            return Ok(());
        }
        self.free_apply_closes()?;
        let path = self.path_of(task);
        let free = self.free.as_mut().unwrap();
        free.session.begin(&path);
        if !free.session.admits(&path, Phase::Entered) {
            return Ok(());
        }
        self.free_apply_pending(task)?;
        let free = self.free.as_mut().unwrap();
        free.session
            .publish(&path, Phase::Entered)
            .map_err(diverged)?;
        if free.session.admits(&path, Phase::Done) {
            if task == 0 && free.native {
                // The host projects the public outcome after the evaluator returns.
                // It writes that observation and Done together before the terminal.
                if !free.session.is_replay() {
                    free.root_pending = true;
                    return Ok(());
                }
                free.root_observation = Some(
                    free.session
                        .replay_host_observation(&path, &[])
                        .map_err(diverged)?,
                );
            }
            free.session.publish(&path, Phase::Done).map_err(diverged)?;
        }
        Ok(())
    }

    /// The `await_any` member: live the first completed, replayed the recorded one once it
    /// completed. `None` while it waits.
    pub(super) fn free_any(&mut self, live: Option<u64>, set: u64) -> Result<Option<u64>, Error> {
        let path = self.tasks.path();
        let Some(free) = &mut self.free else {
            return Ok(live);
        };
        let finished: Vec<bool> = self.tasks.member_finished(set);
        free.session
            .await_any(&path, live, |index| {
                finished.get(index as usize).copied().unwrap_or(false)
            })
            .map_err(diverged)
    }

    /// Whether a free-mode replay holds this mutex take until its version is reached.
    pub(super) fn free_take_enabled(&self, mutex: u64) -> Result<bool, Error> {
        self.free_enabled((MUTEX, mutex))
    }

    /// The canceled-receive outcome, as `receive_ready` builds it.
    pub(super) fn canceled_value(cause: u32) -> Value {
        error(variant(1, vec![variant(cause, vec![])]))
    }
}
