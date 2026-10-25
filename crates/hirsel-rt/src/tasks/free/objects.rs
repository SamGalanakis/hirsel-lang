//! Free object custody without the scheduler role (r6 S3).
//!
//! A running task resolves a mutex, atomic image or channel endpoint to its
//! stable cell once, under the role, and keeps that lease in its own context.
//! Later immediate operations take only the cell latch and the task's own
//! phase: no role acquisition, task-table lookup or readiness poll. A lease
//! keeps cell storage, never a closed object: sync words are never reissued
//! (cells are append-only) and endpoint words carry their generation.
//!
//! An operation that cannot complete at once leaves before its ordinal
//! begins, and the role path registers its wait. A completed put or send whose
//! successor is registered unlocks the cell, then notifies it under the role.
//! The quantum's end, a placement correction, pending root releases, a
//! deferred child and every creation, close or failure keep the role path.
//! Replay admits an immediate op through the task's own token, captured once
//! under the role: the cut, the op's Done and its object's version order are
//! the task's cursor and the shared version cells, never the scheduler. A
//! replayed receive reads the tape's cancellation before it begins; with no
//! cause and nothing ready it stays unbegun, and the role path registers, then
//! rechecks, as Tokio's `recv` (`tokio/src/sync/mpsc/chan.rs:304-344`). This
//! follows Tokio's reserve-oldest then unlock-before-wake order
//! (`tokio/src/sync/batch_semaphore.rs:334-402`, ce685ee6) and Verona's
//! single-resource successor handoff (`src/rt/boc/behaviourcore.h:648-677`).

use super::super::*;
use hirsel_free::mutex::Acquired;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[cfg(test)]
mod tests;

/// Leases one task keeps per kind; the oldest is dropped first.
const LEASES: usize = 8;

/// Operation state only the running task touches. It moves with the task's
/// context, never through the scheduler's rows.
#[derive(Default)]
pub(crate) struct OpLocal {
    /// A compiler-proved adjacent spawn/await, not published to a worker yet.
    pub(crate) deferred: Option<u32>,
    mutexes: Leases<Arc<crate::sync::MutexCell>>,
    images: Leases<Arc<crate::sync::AtomicImage>>,
    endpoints: Leases<Endpoint>,
    /// The last role observation that this task was not canceled.
    uncanceled: Option<Uncanceled>,
    /// This task's replay reservation; set once, under the role.
    replay: Option<super::replay::Token>,
}

struct Endpoint {
    channel: u32,
    sender: bool,
    cell: Arc<crate::channels::ChannelCell>,
}

struct Uncanceled {
    epoch: Arc<hirsel_free::cancel::Epoch>,
    seen: u64,
    shield: u32,
}

struct Leases<T>(Vec<(u64, T)>);

impl<T> Default for Leases<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> Leases<T> {
    fn get(&self, word: u64) -> Option<&T> {
        self.0
            .iter()
            .find_map(|(key, lease)| (*key == word).then_some(lease))
    }

    fn keep(&mut self, word: u64, lease: impl FnOnce() -> T) {
        if self.get(word).is_some() {
            return;
        }
        if self.0.len() == LEASES {
            self.0.remove(0);
        }
        self.0.push((word, lease()));
    }
}

/// Resolution under the role: remember a mutex cell for later operations.
pub(super) fn keep_mutex(ctx: *mut TaskCtx, word: u64, cell: &Arc<crate::sync::MutexCell>) {
    keep_replay(ctx);
    unsafe { (*ctx).free_ops.mutexes.keep(word, || cell.clone()) };
}

pub(super) fn keep_image(ctx: *mut TaskCtx, word: u64, image: &Arc<crate::sync::AtomicImage>) {
    keep_replay(ctx);
    unsafe { (*ctx).free_ops.images.keep(word, || image.clone()) };
}

/// A lease is useless in replay without the token, so take both together.
fn keep_replay(ctx: *mut TaskCtx) {
    if super::replay::on(ctx) && unsafe { (*ctx).free_ops.replay.is_none() } {
        let token = super::replay::token(ctx);
        unsafe { (*ctx).free_ops.replay = token };
    }
}

pub(super) fn keep_endpoint(
    ctx: *mut TaskCtx,
    word: u64,
    channel: u32,
    sender: bool,
    cell: &Arc<crate::channels::ChannelCell>,
) {
    keep_replay(ctx);
    unsafe {
        (*ctx).free_ops.endpoints.keep(word, || Endpoint {
            channel,
            sender,
            cell: cell.clone(),
        })
    };
}

/// The role observed this task uncanceled at `seen` under `shield`.
pub(super) fn keep_uncanceled(ctx: *mut TaskCtx, s: &Scheduler, shield: u32) {
    let seen = s.cancel_epoch.load();
    let ops = unsafe { &mut (*ctx).free_ops };
    match &mut ops.uncanceled {
        Some(known) => {
            known.seen = seen;
            known.shield = shield;
        }
        None => {
            ops.uncanceled = Some(Uncanceled {
                epoch: s.cancel_epoch.clone(),
                seen,
                shield,
            })
        }
    }
}

/// Whether this entry may act on leases alone. Every condition is the running
/// task's own state or immutable admission data.
fn admitted(ctx: *mut TaskCtx) -> bool {
    let run = unsafe { (*ctx).run_ptr() };
    let task = unsafe { &*ctx };
    #[cfg(feature = "measurement")]
    if task.run_ref().mc_profile.is_some() {
        return false;
    }
    #[cfg(feature = "diagnostic")]
    if crate::root_trace::enabled() {
        return false;
    }
    task.run_ref().free_objects.load(Ordering::Acquire)
        && (!task.run_ref().free_replay || task.free_ops.replay.is_some())
        && task.role_depth == 0
        && task.free_ops.deferred.is_none()
        && !task.roots.has_pending()
        && unsafe { crate::pool::settled(run) }
        && crate::stop::observe(ctx).is_ok()
}

/// The replay binding of an op from its task's token, `Some(None)` outside
/// replay. `None` leaves the op unbegun for the role path.
fn replayed(
    ctx: *mut TaskCtx,
    handle: Option<hirsel_free::session::Handle>,
) -> Option<Option<super::replay::Token>> {
    match unsafe { &(*ctx).free_ops.replay } {
        None => Some(None),
        Some(token) => token.immediate(handle).then(|| Some(token.clone())),
    }
}

/// Begin an admitted immediate op: its replay token already passed the cut.
fn begin(ctx: *mut TaskCtx, replay: Option<&super::replay::Token>) {
    match replay {
        None => super::record::begin(ctx),
        Some(token) => super::record::begin_replayed(ctx, token),
    }
}

/// The end of an immediate operation: its quantum continues here, or the
/// role path gives up the worker. A replayed task that reached its cut parks
/// at the role path's boundary, as its role-held op would.
unsafe fn tail(ctx: *mut TaskCtx) -> Stopped<()> {
    let cut = unsafe { (*ctx).free_ops.replay.as_ref() }.is_some_and(|token| token.at_cut());
    if !cut && unsafe { crate::pool::free::proceed(ctx) } {
        if let Some(metrics) = &unsafe { (*ctx).run_ref() }.worker_metrics {
            metrics.elision();
        }
        return crate::stop::observe(ctx);
    }
    let _role = unsafe { role::Guard::new(ctx) };
    unsafe { super::boundary(&mut *ctx, false) }
}

/// Raise an immediate operation's stop as its role-held entry would.
/// # Safety
/// `ctx` is the running task's, at its ABI entry, holding nothing to drop.
pub(crate) unsafe fn finish(ctx: *mut TaskCtx, result: Stopped<()>) -> u8 {
    if let Err(stop) = result {
        let _role = unsafe { role::Guard::new(ctx) };
        unsafe { crate::stop::check(ctx, Err::<(), _>(stop)) };
    }
    STATUS_OK
}

/// `None` leaves the operation unbegun for the role path.
/// # Safety
/// As for hirsel_rt_v2_mutex_take.
pub(crate) unsafe fn mutex_take(
    ctx: *mut TaskCtx,
    word: u64,
    payload: *mut u8,
    out: *mut u32,
) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    // The lease outlives this op: only the role path adds or drops leases.
    let cell = Arc::as_ptr(unsafe { (*ctx).free_ops.mutexes.get(word) }?);
    let cell = unsafe { &*cell };
    let replay = replayed(ctx, Some((crate::free::MUTEX, word)))?;
    let replay = replay.map(|token| token.bind((crate::free::MUTEX, word)));
    let me = unsafe { (*ctx).task_id };
    let mut state = cell.lock();
    // A reserved front waiter owns the next permit; never overtake it.
    if !state.permit.ready_for(me) {
        return None;
    }
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let mut result = Acquired::Pending;
    let publication = super::mutex::touched(
        ctx,
        Phase::Done,
        cell,
        &mut state,
        replay.as_ref(),
        |state| {
            result = state.permit.acquire(me, &mut false);
            if result == Acquired::Payload {
                unsafe { cell.take(state, payload) };
            }
            "take"
        },
    );
    drop(state);
    drop(replay);
    Some(publication.and_then(|()| {
        unsafe { out.write(u32::from(result == Acquired::Dead)) };
        unsafe { tail(ctx) }
    }))
}

/// # Safety
/// As for hirsel_rt_v2_mutex_put.
pub(crate) unsafe fn mutex_put(
    ctx: *mut TaskCtx,
    word: u64,
    payload: *mut u8,
) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    let cell = Arc::as_ptr(unsafe { (*ctx).free_ops.mutexes.get(word) }?);
    let cell = unsafe { &*cell };
    let replay = replayed(ctx, Some((crate::free::MUTEX, word)))?;
    let replay = replay.map(|token| token.bind((crate::free::MUTEX, word)));
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let mut state = cell.lock();
    let mut next = None;
    let mut token = None;
    let publication = super::mutex::touched(
        ctx,
        Phase::Done,
        cell,
        &mut state,
        replay.as_ref(),
        |state| {
            next = unsafe { cell.put(state, payload) };
            token = next.and_then(|next| state.take_token(next));
            "put"
        },
    );
    drop(state);
    drop(replay);
    Some(publication.and_then(|()| match (next, token) {
        (None, _) => unsafe { tail(ctx) },
        // The permit is reserved for next before its wake, after the unlock.
        (Some(_), Some(token)) => {
            unsafe { crate::pool::free::notify((*ctx).run_ptr(), &token) };
            unsafe { tail(ctx) }
        }
        (Some(next), None) => {
            let _role = unsafe { role::Guard::new(ctx) };
            super::mutex::notify(ctx, next);
            unsafe { super::try_yield(&mut *ctx) }
        }
    }))
}

/// # Safety
/// As for hirsel_rt_v2_atomic_load.
pub(crate) unsafe fn atomic_load(
    ctx: *mut TaskCtx,
    word: u64,
    slot: *mut u8,
    size: u64,
) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    let image = unsafe { &*Arc::as_ptr((*ctx).free_ops.images.get(word)?) };
    if image.size() as u64 != size {
        return None;
    }
    let replay = replayed(ctx, None)?;
    begin(ctx, replay.as_ref());
    let result = unsafe { super::record::load_image(ctx, image, slot, size, replay.as_ref()) };
    drop(replay);
    Some(result.and_then(|()| unsafe { scheduling_op(ctx) }))
}

/// # Safety
/// As for hirsel_rt_v2_atomic_store.
pub(crate) unsafe fn atomic_store(
    ctx: *mut TaskCtx,
    word: u64,
    value: *const u8,
    size: u64,
) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    let image = unsafe { &*Arc::as_ptr((*ctx).free_ops.images.get(word)?) };
    if image.size() as u64 != size {
        return None;
    }
    let replay = replayed(ctx, None)?;
    begin(ctx, replay.as_ref());
    let result = unsafe { super::record::store_image(ctx, image, value, replay.as_ref()) };
    drop(replay);
    Some(result.and_then(|()| unsafe { scheduling_op(ctx) }))
}

/// Atomic entries end at yield_now's scheduling point, which counts its op.
unsafe fn scheduling_op(ctx: *mut TaskCtx) -> Stopped<()> {
    if let Some(metrics) = &unsafe { (*ctx).run_ref() }.worker_metrics {
        metrics.op();
    }
    unsafe { tail(ctx) }
}

/// # Safety
/// As for hirsel_rt_v2_channel_send.
pub(crate) unsafe fn channel_send(
    ctx: *mut TaskCtx,
    sender: u64,
    message: *mut u8,
    out: *mut u32,
) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    let endpoint = unsafe { (*ctx).free_ops.endpoints.get(sender) }?;
    if !endpoint.sender {
        return None;
    }
    let channel = endpoint.channel;
    let cell = unsafe { &*Arc::as_ptr(&endpoint.cell) };
    let handle = (crate::free::CHANNEL, u64::from(channel));
    let replay = replayed(ctx, Some(handle))?.map(|token| token.bind(handle));
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let mut state = cell.lock();
    cell.prepare_send(&mut state);
    // Send deliberately does not observe cancellation (CONC-3).
    let publication = super::channel::sent(ctx, cell, &mut state, message, out, replay.as_ref());
    // A receiver registers before its first poll takes this latch, so reading
    // under the latch cannot miss one that will not see this message. The
    // oldest receiver that parked with a token is woken after the unlock.
    let done = publication
        .as_ref()
        .is_ok_and(|&status| status == crate::channels::DONE);
    let token = if done { state.next_token() } else { None };
    let parked = cell.parked();
    drop(state);
    drop(replay);
    Some(publication.and_then(|status| {
        if let Some(token) = token {
            unsafe { crate::pool::free::notify((*ctx).run_ptr(), &token) };
            unsafe { tail(ctx) }
        } else if status == crate::channels::DONE && parked {
            let _role = unsafe { role::Guard::new(ctx) };
            role::with_role(ctx, |s| s.channels.touch(channel));
            unsafe { super::try_yield(&mut *ctx) }
        } else {
            unsafe { tail(ctx) }
        }
    }))
}

/// # Safety
/// As for hirsel_rt_v2_channel_receive.
pub(crate) unsafe fn channel_receive(
    ctx: *mut TaskCtx,
    receiver: u64,
    message: *mut u8,
    out: *mut u32,
) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    let shield = unsafe { (*ctx).shield };
    let ops = unsafe { &(*ctx).free_ops };
    let endpoint = ops.endpoints.get(receiver)?;
    if endpoint.sender {
        return None;
    }
    let channel = endpoint.channel;
    let cell = unsafe { &*Arc::as_ptr(&endpoint.cell) };
    let handle = (crate::free::CHANNEL, u64::from(channel));
    // Receive is a cancellation point. Replay reads its recorded cause before
    // the op begins; a receive orders against no version, only the cut.
    let (replay, cause) = match replayed(ctx, None)? {
        Some(token) => {
            let cause = if shield == 0 {
                token.next_cancel()
            } else {
                None
            };
            (Some(token.bind(handle)), cause)
        }
        // Live, act only on an observation that no cancellation, scope change
        // or shield change has invalidated since.
        None => {
            let known = ops.uncanceled.as_ref()?;
            if known.shield != shield || known.epoch.load() != known.seen {
                return None;
            }
            (None, None)
        }
    };
    let canceled = cause.map(|cause| format!("canceled {cause}"));
    let mut state = cell.lock();
    // Without a message or close, the role path enters and registers the wait.
    if cause.is_none() && !cell.ready(&state) {
        return None;
    }
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let publication = super::record::publish_with_replay(
        ctx,
        Phase::Done,
        replay.as_ref().map(|object| object.token()),
        |writer| {
            let (status, outcome) = if let Some(cause) = cause {
                (
                    crate::channels::CANCELED + cause,
                    canceled.as_deref().unwrap(),
                )
            } else {
                match unsafe { cell.receive(&mut state, message) } {
                    Ok(true) => (crate::channels::DONE, "msg"),
                    Err(status) => (status, "closed"),
                    Ok(false) => unreachable!("the receiver owns this channel's ready message"),
                }
            };
            if let Some(writer) = writer {
                if shield == 0 {
                    writer.cancel(None);
                }
                writer.receive(
                    cell.object
                        .get()
                        .expect("a recorded channel identity")
                        .clone(),
                    outcome,
                );
            } else if let Some(replay) = &replay {
                if shield == 0 {
                    assert_eq!(replay.token().cancel(cause)?, cause);
                }
                replay.receive(outcome)?;
            }
            unsafe { out.write(status) };
            Ok(())
        },
    );
    let parked = cell.parked();
    drop(state);
    drop(canceled);
    drop(replay);
    Some(publication.and_then(|result| result).and_then(|()| {
        if cause.is_some() && parked {
            // A canceled receive takes no message: the policy serves the next
            // parked receiver, as on the role path.
            let _role = unsafe { role::Guard::new(ctx) };
            role::with_role(ctx, |s| s.channels.touch(channel));
            unsafe { super::try_yield(&mut *ctx) }
        } else {
            unsafe { tail(ctx) }
        }
    }))
}
