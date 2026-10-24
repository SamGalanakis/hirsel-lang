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
//! deferred child, replay and every creation, close or failure keep the role
//! path. This follows Tokio's reserve-oldest then unlock-before-wake order
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
    unsafe { (*ctx).free_ops.mutexes.keep(word, || cell.clone()) };
}

pub(super) fn keep_image(ctx: *mut TaskCtx, word: u64, image: &Arc<crate::sync::AtomicImage>) {
    unsafe { (*ctx).free_ops.images.keep(word, || image.clone()) };
}

pub(super) fn keep_endpoint(
    ctx: *mut TaskCtx,
    word: u64,
    channel: u32,
    sender: bool,
    cell: &Arc<crate::channels::ChannelCell>,
) {
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
        && !task.run_ref().free_replay
        && task.role_depth == 0
        && task.free_ops.deferred.is_none()
        && !task.roots.has_pending()
        && unsafe { crate::pool::settled(run) }
        && crate::stop::observe(ctx).is_ok()
}

/// The end of an immediate operation: its quantum continues here, or the
/// role path gives up the worker.
unsafe fn tail(ctx: *mut TaskCtx) -> Stopped<()> {
    if unsafe { crate::pool::free::proceed(ctx) } {
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
    let me = unsafe { (*ctx).task_id };
    let mut state = cell.lock();
    // A reserved front waiter owns the next permit; never overtake it.
    if !state.permit.ready_for(me) {
        return None;
    }
    super::record::begin(ctx);
    let mut result = Acquired::Pending;
    let publication = super::mutex::touched(ctx, Phase::Done, cell, &mut state, None, |state| {
        result = state.permit.acquire(me, &mut false);
        if result == Acquired::Payload {
            unsafe { cell.take(state, payload) };
        }
        "take"
    });
    drop(state);
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
    super::record::begin(ctx);
    let mut state = cell.lock();
    let mut next = None;
    let mut token = None;
    let publication = super::mutex::touched(ctx, Phase::Done, cell, &mut state, None, |state| {
        next = unsafe { cell.put(state, payload) };
        token = next.and_then(|next| state.take_token(next));
        "put"
    });
    drop(state);
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
    super::record::begin(ctx);
    Some(
        unsafe { super::record::load_image(ctx, image, slot, size, None) }
            .and_then(|()| unsafe { scheduling_op(ctx) }),
    )
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
    super::record::begin(ctx);
    Some(
        unsafe { super::record::store_image(ctx, image, value, None) }
            .and_then(|()| unsafe { scheduling_op(ctx) }),
    )
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
    super::record::begin(ctx);
    let mut state = cell.lock();
    cell.prepare_send(&mut state);
    // Send deliberately does not observe cancellation (CONC-3).
    let publication = super::channel::sent(ctx, cell, &mut state, message, out, None);
    // A receiver registers before its first poll takes this latch, so reading
    // under the latch cannot miss one that will not see this message.
    let parked = cell.parked();
    drop(state);
    Some(publication.and_then(|status| {
        if status == crate::channels::DONE && parked {
            let _role = unsafe { role::Guard::new(ctx) };
            scheduler(ctx).channels.touch(channel);
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
    // Receive is a cancellation point: act only on an observation that no
    // cancellation, scope change or shield change has invalidated since.
    let known = ops.uncanceled.as_ref()?;
    if known.shield != shield || known.epoch.load() != known.seen {
        return None;
    }
    let endpoint = ops.endpoints.get(receiver)?;
    if endpoint.sender {
        return None;
    }
    let cell = unsafe { &*Arc::as_ptr(&endpoint.cell) };
    let mut state = cell.lock();
    if !cell.ready(&state) {
        return None;
    }
    super::record::begin(ctx);
    let publication = super::record::publish_with_replay(ctx, Phase::Done, None, |writer| {
        let (status, outcome) = match unsafe { cell.receive(&mut state, message) } {
            Ok(true) => (crate::channels::DONE, "msg"),
            Err(status) => (status, "closed"),
            Ok(false) => unreachable!("the receiver owns this channel's ready message"),
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
        }
        unsafe { out.write(status) };
    });
    drop(state);
    Some(publication.and_then(|()| unsafe { tail(ctx) }))
}
