//! Free mode (`tasks.free@1`) on the cooperative runtime at width 1 and one permit (design
//! §5, lane F0), over a [`hirsel_free::session::Session`]: op phases, recording, and replay
//! with cut admission, enablement and substitution.
//!
//! - **Ops.** The free ops are the task runtime's synchronising entries: channel open, dup,
//!   send, receive and an endpoint's close, mutex creation, take, put and kill, atomic
//!   creation, loads and stores, `thaw_turn`, spawn, join, `await_any`, scope entry and exit,
//!   the other task and set entries, `task.check_canceled`, the standard-stream prints (host
//!   writes on the `stdout`/`stderr` keys) and each task's end. Each yields, so a seeded
//!   schedule interleaves tasks at every op as simultaneous events at width N would.
//! - **Phases.** An op that cannot wait publishes (k, done); a waiting op (receive, take,
//!   join, `await_any`, `cancel_all`, scope exit) publishes (k, entered) before it parks and
//!   (k, done) once it acts; a print runs entered, claimed, ended 0 and done.
//! - **Releases.** A `Shared` release that does not find its root's count at 1 joins the
//!   releasing task's pending list ([`crate::roots`]), which the entered phase of its next op
//!   applies, walking the roots it frees and applying those walks' nested releases at once
//!   (Amendment F0-1). A task's end applies the rest, so no cleanup phase remains.
//! - **Replay.** A phase the cut does not admit parks its task for good, as does reaching the
//!   cut. A phase whose record names a predecessor waits replay-pending: it parks until a
//!   publish, then checks again. A run whose every task waits ends Blocked, and the replay's
//!   projection is checked then.
//!
//! The session arrives through F0's test channel, [`hirsel_free::session::TestHook`].

use crate::context::TaskCtx;
use hirsel_free::phase::Phase;
use hirsel_free::replay::Diverged;
use hirsel_free::session::{Advance, Claim, Handle, Session, TestHook};

pub(crate) const CHANNEL: u8 = 1;
pub(crate) const MUTEX: u8 = 2;
const GATE: u8 = 3;

/// A run's free-mode state.
pub(crate) struct Free {
    hook: TestHook,
    pub(crate) session: Session,
}

/// The run's free-mode state, when it has one.
fn free(ctx: *mut TaskCtx) -> Option<&'static mut Free> {
    // SAFETY: the context is live for the whole run and reached by its running task only.
    unsafe { (*(*(*ctx).run_ptr()).free.get()).as_deref_mut() }
}

/// Install F0's test channel on `ctx` when the environment sets it: the session, and a seeded
/// schedule that interleaves the tasks at every op.
pub(crate) fn install(ctx: &mut TaskCtx, binding: &[u8]) -> Result<(), String> {
    let Some(hook) = TestHook::from_env()? else {
        return Ok(());
    };
    let mut session = hook.session(binding)?;
    session.start(&[]);
    ctx.run().schedule = hirsel_kernels::schedule::Schedule::new(
        hirsel_kernels::schedule::Algorithm::Random,
        hook.seed,
    );
    unsafe { (*(*ctx.run_ptr()).free.get()) = Some(Box::new(Free { hook, session })) };
    Ok(())
}

/// End the run: a recording stops with `outcome` and writes its tape and report; a replay
/// checks and writes its projection.
pub(crate) fn finish(ctx: &mut TaskCtx, outcome: &hirsel_run_schema::RunOutcome) {
    use hirsel_run_schema::RunOutcome as O;
    let Some(free) = unsafe { &mut (*(*ctx.run_ptr()).free.get()) }.take() else {
        return;
    };
    let name = match outcome {
        O::Returned(_) => "returned",
        O::Failed { .. } => "failed",
        O::BudgetExceeded(kind) => kind.name(),
        O::Blocked(_) => "blocked",
        _ => "stopped",
    };
    if let Err(error) = free.hook.finish(free.session, name) {
        crate::fault(&format!("free mode: {error}"));
    }
}

/// A replay that cannot follow its tape: report it, then stop the run Diverged.
fn diverged(ctx: *mut TaskCtx, error: Diverged) -> ! {
    // SAFETY: the context is live.
    if let Some(free) = unsafe { (*(*(*ctx).run_ptr()).free.get()).take() } {
        let _ = std::fs::write(&free.hook.report, format!("diverged {}\n", error.0));
    }
    // SAFETY: the context is live; a test-channel divergence may leave this op's locals.
    unsafe {
        crate::stop::raise(
            ctx,
            crate::stop::Stop::Diverged(format!("free-mode {error}")),
        )
    }
}

/// Whether free mode is on.
pub(crate) fn on(ctx: *mut TaskCtx) -> bool {
    // SAFETY: the context is live; policy is fixed before any worker starts.
    unsafe { (*ctx).run_ref().free_mode || (*ctx).run_ref().free.is_some() }
}

fn path(ctx: *mut TaskCtx) -> Vec<u32> {
    crate::tasks::current_path(ctx)
}

/// Begin an op of the running task whose first phase is `first` (entered for a waiting op or
/// a print, done for an op that cannot wait): whether free mode is on. A phase the cut does
/// not admit parks the task for good here; a waiting op whose done phase it does not admit
/// runs its entry and then waits for good.
pub(crate) fn enter(ctx: *mut TaskCtx, first: Phase) -> bool {
    let Some(free) = free(ctx) else {
        if on(ctx) {
            crate::roots::apply_pending(ctx);
            return true;
        }
        return false;
    };
    let path = path(ctx);
    free.session.begin(&path);
    if !free.session.admits(&path, first) {
        crate::tasks::park_final(ctx);
    }
    crate::roots::apply_pending(ctx);
    if first == Phase::Entered && !free.session.admits(&path, Phase::Done) {
        publish(ctx, Phase::Entered);
        crate::tasks::park_final(ctx);
    }
    true
}

/// Close the section of `phase` of the running task's current op; tasks waiting
/// replay-pending check again.
pub(crate) fn publish(ctx: *mut TaskCtx, phase: Phase) {
    let Some(free) = free(ctx) else {
        if on(ctx) {
            crate::tasks::wake_retrying(ctx);
        }
        return;
    };
    let path = path(ctx);
    if let Err(error) = free.session.publish(&path, phase) {
        diverged(ctx, error);
    }
    crate::tasks::wake_retrying(ctx);
}

/// Publish (k, done); a replayed task at its cut parks for good before the next segment.
pub(crate) fn done(ctx: *mut TaskCtx) {
    if !on(ctx) {
        return;
    }
    publish(ctx, Phase::Done);
    if free(ctx).is_some_and(|free| free.session.at_cut(&path(ctx))) {
        crate::tasks::park_final(ctx);
    }
}

/// Wait replay-pending until `ready` holds, parking until the next publish each time it does
/// not.
pub(crate) fn wait_until(ctx: *mut TaskCtx, mut ready: impl FnMut(*mut TaskCtx) -> bool) {
    while !ready(ctx) {
        crate::tasks::park_retry(ctx, false);
    }
}

fn check<T>(ctx: *mut TaskCtx, result: Result<T, Diverged>) -> T {
    result.unwrap_or_else(|error| diverged(ctx, error))
}

pub(crate) fn create(ctx: *mut TaskCtx, handle: Handle) {
    if let Some(free) = free(ctx) {
        let result = free.session.create(&path(ctx), handle);
        check(ctx, result);
    }
}

/// A serial package's logical gate: no creating op, version 1 at admission.
pub(crate) fn gate(ctx: *mut TaskCtx, gate: u32) -> Handle {
    let handle = (GATE, u64::from(gate));
    if let Some(free) = free(ctx) {
        free.session.gate(handle, &format!("package:{gate}"));
    }
    handle
}

/// Whether sub-event 0 of the running op on `handle` is enabled (always, live).
pub(crate) fn enabled(ctx: *mut TaskCtx, handle: Handle) -> bool {
    match free(ctx) {
        None => true,
        Some(free) => {
            let result = free.session.enabled(&path(ctx), 0, handle);
            check(ctx, result)
        }
    }
}

/// Wait until sub-event 0 on `handle` is enabled, then apply it with `outcome`.
pub(crate) fn touch(ctx: *mut TaskCtx, handle: Handle, outcome: &str) {
    if free(ctx).is_none() {
        return;
    }
    wait_until(ctx, |ctx| enabled(ctx, handle));
    let free = free(ctx).unwrap();
    let result = free.session.touch(&path(ctx), 0, handle, outcome);
    check(ctx, result);
}

pub(crate) fn receive(ctx: *mut TaskCtx, channel: u32, outcome: &str) {
    if let Some(free) = free(ctx) {
        let result = free
            .session
            .receive(&path(ctx), (CHANNEL, u64::from(channel)), outcome);
        check(ctx, result);
    }
}

/// An atomic load's bits: live recorded, replayed substituted.
pub(crate) fn load(ctx: *mut TaskCtx, live: u128) -> u128 {
    match free(ctx) {
        None => live,
        Some(free) => {
            let result = free.session.load(&path(ctx), live);
            check(ctx, result)
        }
    }
}

/// `thaw_turn`'s answer: whether the root is the last, with a replayed `Ok` waiting for count
/// 1 and a replayed `Err` forced.
pub(crate) fn thaw(ctx: *mut TaskCtx, last: impl Fn() -> bool) -> bool {
    if free(ctx).is_none() {
        return last();
    }
    let at = path(ctx);
    wait_until(ctx, |ctx| {
        free(ctx).unwrap().session.thaw_ready(&at, last())
    });
    let result = free(ctx).unwrap().session.thaw(&at, last());
    check(ctx, result)
}

/// The cancellation an unshielded check observes: live its own, replayed the recorded one.
pub(crate) fn cancel_check(ctx: *mut TaskCtx, live: Option<u32>) -> Option<u32> {
    match free(ctx) {
        None => live,
        Some(free) => {
            let result = free.session.cancel_check(&path(ctx), live);
            check(ctx, result)
        }
    }
}

/// Whether the running task's next check observes a cancellation, without checking.
pub(crate) fn cancel_peek(ctx: *mut TaskCtx, live: Option<u32>) -> Option<u32> {
    match free(ctx) {
        None => live,
        Some(free) => free.session.cancel_peek(&path(ctx), live),
    }
}

/// `await_any`'s member: live the first completed (`live`), replayed the recorded one once
/// `finished` holds for it. `None` while it waits.
pub(crate) fn await_any(
    ctx: *mut TaskCtx,
    live: Option<u64>,
    finished: impl Fn(u64) -> bool,
) -> Option<u64> {
    match free(ctx) {
        None => live,
        Some(free) => {
            let result = free.session.await_any(&path(ctx), live, finished);
            check(ctx, result)
        }
    }
}

/// `task.await_any`'s recorded member in a replay, which its wait waits for; `None` live.
pub(crate) fn await_any_member(ctx: *mut TaskCtx) -> Option<u64> {
    let free = free(ctx)?;
    free.session.await_any_member(&path(ctx))
}

/// An endpoint's close (its drop) as an op: `None` when free mode is off. It waits until its
/// version is reached, records `outcome` and publishes (k, done); `Some(false)` when the cut
/// does not admit it, and the endpoint stays open. A drop never parks for good: the task's
/// next op does.
pub(crate) fn close(ctx: *mut TaskCtx, handle: Handle, outcome: &str) -> Option<bool> {
    let Some(free) = free(ctx) else {
        if on(ctx) {
            crate::roots::apply_pending(ctx);
            crate::tasks::wake_retrying(ctx);
            return Some(true);
        }
        return None;
    };
    let at = path(ctx);
    free.session.begin(&at);
    if !free.session.admits(&at, Phase::Done) {
        return Some(false);
    }
    crate::roots::apply_pending(ctx);
    touch(ctx, handle, outcome);
    let free = self::free(ctx).unwrap();
    if let Err(error) = free.session.publish(&at, Phase::Done) {
        diverged(ctx, error);
    }
    crate::tasks::wake_retrying(ctx);
    Some(true)
}

/// Register a spawned child; whether it runs (a replay never starts one its cut lacks).
pub(crate) fn start(ctx: *mut TaskCtx, child: &[u32]) -> bool {
    free(ctx).is_none_or(|free| free.session.start(child))
}

/// A standard-stream print in free mode: `None` when free mode is off. Otherwise the print's
/// phases run here: `write` makes the host call (live only) and its completion is the op's,
/// or `None` when the task parked for good in flight.
pub(crate) fn print<T>(
    ctx: *mut TaskCtx,
    stderr: bool,
    bytes: &[u8],
    canceled: Option<u32>,
    write: impl FnOnce() -> T,
    replayed: impl FnOnce() -> T,
    canceled_result: impl FnOnce(u32) -> T,
) -> Option<T> {
    let free = free(ctx)?;
    // A request's cut may be its claimed or ended phase, so only its entry is checked here.
    let at = path(ctx);
    free.session.begin(&at);
    if !free.session.admits(&at, Phase::Entered) {
        crate::tasks::park_final(ctx);
    }
    crate::roots::apply_pending(ctx);
    // SAFETY: the context is live.
    let shielded = unsafe { (*ctx).shield } != 0;
    let cause = if shielded {
        canceled
    } else {
        cancel_check(ctx, canceled)
    };
    if let Some(cause) = cause {
        done(ctx);
        return Some(canceled_result(cause));
    }
    let sink = if stderr { "stderr" } else { "stdout" };
    let free = self::free(ctx).unwrap();
    let mut request = check(ctx, free.session.host_begin(&at, sink));
    crate::tasks::wake_retrying(ctx);
    loop {
        let free = self::free(ctx).unwrap();
        match check(ctx, free.session.host_claim(&at, &mut request)) {
            Claim::Waiting => crate::tasks::park_retry(ctx, false),
            Claim::Final => crate::tasks::park_final(ctx),
            Claim::Call => break,
        }
    }
    let result = if self::free(ctx).unwrap().session.is_replay() {
        replayed()
    } else {
        write()
    };
    let free = self::free(ctx).unwrap();
    match check(ctx, free.session.host_end(&at, &request, sink, bytes)) {
        Advance::Final => crate::tasks::park_final(ctx),
        Advance::Done => {
            crate::tasks::wake_retrying(ctx);
            if free.session.at_cut(&at) {
                crate::tasks::park_final(ctx);
            }
        }
    }
    Some(result)
}

/// A task's end op: entered, whose entry applies the task's pending releases (walking the
/// roots they free in the phase, so no cleanup phase remains), then done.
pub(crate) fn end(ctx: *mut TaskCtx, task: &[u32]) {
    let Some(free) = self::free(ctx) else {
        if on(ctx) {
            crate::roots::apply_pending(ctx);
            crate::tasks::wake_retrying(ctx);
        }
        return;
    };
    free.session.begin(task);
    for phase in [Phase::Entered, Phase::Done] {
        let free = self::free(ctx).unwrap();
        if !free.session.admits(task, phase) {
            return;
        }
        if phase == Phase::Entered {
            crate::roots::apply_pending(ctx);
        }
        let free = self::free(ctx).unwrap();
        if let Err(error) = free.session.publish(task, phase) {
            diverged(ctx, error);
        }
    }
    let free = self::free(ctx).unwrap();
    crate::tasks::wake_retrying(ctx);
}

/// The root task's entry returned: its end op, and in a replay a wait until every other task
/// is at its cut, since the recording's stop followed all their published phases.
pub(crate) fn root_end(ctx: *mut TaskCtx) {
    if !on(ctx) {
        return;
    }
    end(ctx, &[]);
    if free(ctx).is_some_and(|free| free.session.is_replay()) {
        wait_until(ctx, |ctx| {
            free(ctx).is_none_or(|free| free.session.others_at_cut(&[]))
        });
    }
}
