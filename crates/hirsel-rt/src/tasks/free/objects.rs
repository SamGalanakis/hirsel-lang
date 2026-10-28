//! Free object custody without the scheduler role (r6 S3).
//!
//! A running task resolves a mutex, atomic image or channel endpoint to its
//! stable cell once, under the role, and keeps that lease in its own context.
//! Later immediate operations take only the cell latch and the task's own
//! phase: no role acquisition, task-table lookup or readiness poll. A lease
//! keeps cell storage, never a closed object: sync words are never reissued
//! (cells are append-only), and an endpoint slot's generation never wraps, so
//! a lease keyed by a closed word never matches a later endpoint.
//!
//! An operation that cannot complete at once leaves before its ordinal
//! begins, and the role path registers its wait; a live empty receive instead
//! registers its own token under the cell latch and parks without the role,
//! its save callback parking the row. A completed put, send, kill
//! or close wakes its successors with the tokens they registered, after the
//! unlock; a successor without one is notified under the role.
//! Close, kill and cause are cell-local too. A close detaches its leftovers
//! once under the latch and destroys them on the closing task at its cleanup
//! anchor; its registry rows are lifecycle administration, retired at that
//! task's next role entry. The quantum's end, a placement correction, pending
//! root releases, a deferred child and every creation keep the role path.
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
mod close_tests;
#[cfg(test)]
mod compact_mutex_tests;
#[cfg(test)]
mod receive_registration_tests;
#[cfg(test)]
pub(super) mod tests;

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
    /// This task's spawn path, for the failure chain a kill keeps.
    path: Option<Vec<u32>>,
    /// This task's own wake token, taken once under the role, which its
    /// role-free registrations keep in the cell.
    wake: Option<crate::pool::free::Token>,
    /// The wait and wait site of a role-free park, from its switch to its
    /// save callback.
    parking: Option<(Wait, u64)>,
    /// A compact take already linked in this mutex's FIFO and published
    /// Entered. Its reissue must neither begin nor register again.
    pub(super) mutex_wait: Option<u64>,
    /// Registry rows a role-free close left for this task's next role entry.
    retired: Vec<Retired>,
    /// Entered effect tails, subordinate to the task's continuation owner.
    #[allow(dead_code)] // Admission follows the checker/emitter custody proof.
    pub(super) tails: super::tail::Records,
}

/// A closed object's registry row, retired under the role.
enum Retired {
    Mutex(u64),
    Endpoint { word: u64, last: bool },
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

    fn remove(&mut self, word: u64) {
        self.0.retain(|(key, _)| *key != word);
    }
}

/// Resolution under the role: remember a mutex cell for later operations.
pub(super) fn keep_mutex(ctx: *mut TaskCtx, word: u64, cell: &Arc<crate::sync::MutexCell>) {
    keep_replay(ctx);
    if unsafe { (*ctx).free_ops.path.is_none() } {
        let me = unsafe { (*ctx).task_id };
        let path = role::with_role(ctx, |s| s.task(me).path.clone());
        unsafe { (*ctx).free_ops.path = Some(path) };
    }
    keep_wake(ctx);
    unsafe { (*ctx).free_ops.mutexes.keep(word, || cell.clone()) };
}

/// This task's own wake token, for one registration.
pub(super) fn wake(ctx: *mut TaskCtx) -> Option<crate::pool::free::Token> {
    unsafe { (*ctx).free_ops.wake.clone() }
}

/// The cell of a receiver this task resolved ([`keep_endpoint`]).
pub(super) fn receiver_cell(ctx: *mut TaskCtx, word: u64) -> Arc<crate::channels::ChannelCell> {
    match unsafe { (*ctx).free_ops.endpoints.get(word) } {
        Some(endpoint) if !endpoint.sender => endpoint.cell.clone(),
        _ => fault("a receive without its resolved receiver lease"),
    }
}

/// The wait the running task's role-free park leaves for its save callback.
pub(super) fn parking(ctx: *mut TaskCtx, wait: Wait, site: u64) {
    assert!(unsafe { (*ctx).free_ops.parking.replace((wait, site)) }.is_none());
}

/// The wait and site a role-free park left, read once by its save callback.
/// # Safety
/// The caller's idle continuation owns the suspended image `ctx`.
pub(crate) unsafe fn parked(ctx: *mut TaskCtx) -> Option<(Wait, u64)> {
    unsafe { (*ctx).free_ops.parking.take() }
}

/// Lifecycle administration a role-free close left: its closed rows retire
/// at this task's next role entry, with no latch held.
/// # Safety
/// The caller owns the run's tables: the role, W1's run token or the end.
pub(crate) unsafe fn retire(ctx: *mut TaskCtx) {
    let retired = unsafe { &mut (*ctx).free_ops.retired };
    if retired.is_empty() {
        return;
    }
    let Some(s) = (unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut() }) else {
        return;
    };
    for row in retired.drain(..) {
        match row {
            Retired::Mutex(word) => drop(s.sync.retire(word)),
            Retired::Endpoint { word, last } => drop(s.channels.retire(word, last)),
        }
    }
}

pub(super) fn keep_image(ctx: *mut TaskCtx, word: u64, image: &Arc<crate::sync::AtomicImage>) {
    keep_replay(ctx);
    unsafe { (*ctx).free_ops.images.keep(word, || image.clone()) };
}

/// Take this task's own wake token once, with its first object lease, for
/// the role-free registrations that lease's cell keeps.
fn keep_wake(ctx: *mut TaskCtx) {
    if unsafe { (*ctx).free_ops.wake.is_none() } {
        take_wake(ctx);
    }
}

/// Out of line: it runs once per task, inside role-path ops.
#[cold]
#[inline(never)]
fn take_wake(ctx: *mut TaskCtx) {
    let key = role::with_role(ctx, |s| s.key(s.current));
    unsafe { (*ctx).free_ops.wake = crate::pool::free::own_token((*ctx).run_ptr(), key) };
}

/// Drop this task's own wake token at its body's end: a fast child's
/// context outlives its row's retirement, which waits for every frame
/// reference (`registry::TaskSlots::retire`).
pub(super) fn release_wake(ctx: *mut TaskCtx) {
    drop(unsafe { (*ctx).free_ops.wake.take() });
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
    keep_wake(ctx);
    unsafe {
        (*ctx).free_ops.endpoints.keep(word, || Endpoint {
            channel,
            sender,
            cell: cell.clone(),
        })
    };
}

/// The role observed this task uncanceled at `seen` under `shield`. Scope
/// topology alone cannot introduce a cause into an empty observation.
pub(super) fn keep_uncanceled(ctx: *mut TaskCtx, s: &Scheduler, shield: u32) {
    let seen = s.cancel_causes.load();
    let ops = unsafe { &mut (*ctx).free_ops };
    match &mut ops.uncanceled {
        Some(known) => {
            known.seen = seen;
            known.shield = shield;
        }
        None => {
            ops.uncanceled = Some(Uncanceled {
                epoch: s.cancel_causes.clone(),
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

/// Raise a role-free close's stop as close's role path does, or return its
/// cleanup status.
/// # Safety
/// As for finish.
pub(crate) unsafe fn closed(ctx: *mut TaskCtx, result: Stopped<u8>) -> u8 {
    match result {
        Ok(status) => status,
        Err(stop) => {
            let _role = unsafe { role::Guard::new(ctx) };
            unsafe { crate::stop::check(ctx, Err::<u8, _>(stop)) }
        }
    }
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
        // A certified boundary can give its stack back. Resolve/register
        // under the existing role authority before beginning that path.
        if unsafe { (*ctx).point_ops == 0 } {
            return None;
        }
        // Recorded and replayed waits keep the role path, which publishes
        // Entered under the latch.
        if replay.is_some() || super::record::on(ctx) {
            return None;
        }
        return unsafe { mutex_wait(ctx, cell, state, me, payload, out) };
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

/// A live contended take on a resolved lease. With its own token, the task
/// joins the FIFO under the latch that found the permit held, as Tokio's
/// `poll_acquire` links its waiter under the wait list's lock that checked
/// the permits (`batch_semaphore.rs:432-579`, ce685ee6); a put reserves for
/// the oldest, then wakes after the unlock (`:334-402`). Tokio stores the
/// poll's waker each time; the token here is the task's own for life. Kept
/// out of line: the uncontended take is the hot path.
/// # Safety
/// As for hirsel_rt_v2_mutex_take; `state` is `cell`'s latch, live, unbegun.
#[cold]
#[inline(never)]
unsafe fn mutex_wait(
    ctx: *mut TaskCtx,
    cell: &crate::sync::MutexCell,
    mut state: std::sync::MutexGuard<'_, crate::sync::MutexImage>,
    me: u32,
    payload: *mut u8,
    out: *mut u32,
) -> Option<Stopped<()>> {
    let token = unsafe { (*ctx).free_ops.wake.as_ref() }?.clone();
    begin(ctx, None);
    state.permit.reserve_waiter();
    state.reserve_token();
    let mut queued = false;
    assert_eq!(state.permit.acquire(me, &mut queued), Acquired::Pending);
    state.keep_token(me, token);
    drop(state);
    // No latch crosses the save: a put or kill that beats it notifies the
    // running frame, whose save republishes it (`after_park`).
    let mut state = loop {
        if let Err(stop) =
            unsafe { super::park_registered(&mut *ctx, Wait::Retry { cancelable: false }, 0) }
        {
            return Some(Err(stop));
        }
        let state = cell.lock();
        if state.permit.ready_for(me) {
            break state;
        }
    };
    let mut result = Acquired::Pending;
    // A token the action takes back is dropped after the section.
    let mut stale = None;
    let publication = super::mutex::touched(ctx, Phase::Done, cell, &mut state, None, |state| {
        result = state.permit.acquire(me, &mut queued);
        if result == Acquired::Payload {
            unsafe { cell.take(state, payload) };
        }
        stale = state.take_token(me);
        "take"
    });
    drop(state);
    drop(stale);
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
    // A receiver registers its token and rechecks under this latch, so a
    // receiver this send does not wake saw its message. The oldest receiver
    // is woken after the unlock.
    let token = super::channel::sent_token(&mut state, &publication);
    drop(state);
    drop(replay);
    Some(publication.and_then(|_| {
        if let Some(token) = token {
            unsafe { crate::pool::free::notify((*ctx).run_ptr(), &token) };
        }
        unsafe { tail(ctx) }
    }))
}

/// Whether the role's last observation that this task is uncanceled, under
/// `shield`, still holds: no new cancellation cause or shield change has
/// invalidated it since.
fn uncanceled(ops: &OpLocal, shield: u32) -> bool {
    ops.uncanceled
        .as_ref()
        .is_some_and(|known| known.shield == shield && known.epoch.load() == known.seen)
}

/// A resolved receive. A ready one completes here; a live empty one
/// registers this task's own token and rechecks under the cell latch, then
/// unlocks and parks without the role, as Tokio's `recv` registers its waker
/// and tries again (`tokio/src/sync/mpsc/chan.rs:302-354`, ce685ee6). Its
/// save callback parks the row (`tasks::after_park`); a send or close that
/// makes the receive able to act takes the oldest token, and a wake that beats
/// the save is remembered by the frame. A woken receive rechecks under the
/// latch; a cancellation or a lost admission continues on the role path,
/// already entered, where the cause is read before any message.
/// # Safety
/// As for hirsel_rt_v2_channel_receive.
pub(crate) unsafe fn channel_receive(
    ctx: *mut TaskCtx,
    receiver: u64,
    message: *mut u8,
    out: *mut u32,
    caller_site: u64,
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
    // The lease outlives this op: only the role path adds or drops leases.
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
        // Live, act only on a still valid uncanceled observation.
        None if uncanceled(ops, shield) => (None, None),
        None => return None,
    };
    let canceled = cause.map(|cause| format!("canceled {cause}"));
    let me = unsafe { (*ctx).task_id };
    let mut state = cell.lock();
    let entered = cause.is_none() && !cell.ready(&state);
    if entered {
        // A replayed wait keeps the role path, which admits its Entered phase.
        if replay.is_some() {
            return None;
        }
        let token = ops.wake.clone()?;
        begin(ctx, None);
        super::record::track(ctx, cell.object.get());
        if let Err(stop) = super::record::publish_with_replay(ctx, Phase::Entered, None, |_| ()) {
            return Some(Err(stop));
        }
        state.keep_token(me, token);
        drop(state);
        state = loop {
            // No latch, owner or recorder borrow crosses the save.
            let parked = unsafe {
                super::park_registered(&mut *ctx, Wait::ReceiveReady { channel }, caller_site)
            };
            if let Err(stop) = parked {
                return Some(Err(stop));
            }
            let ops = unsafe { &(*ctx).free_ops };
            if !admitted(ctx) || !uncanceled(ops, shield) {
                let _role = unsafe { role::Guard::new(ctx) };
                return Some(unsafe {
                    super::channel::receive_begun(ctx, receiver, message, out, caller_site, true)
                });
            }
            let mut state = cell.lock();
            // A send or close took the token; any other wake left it.
            state.drop_token(me);
            if cell.ready(&state) {
                break state;
            }
            // Woken with nothing to take: register again, behind the others.
            state.keep_token(me, ops.wake.clone().expect("this task's own token"));
        };
    } else {
        begin(ctx, replay.as_ref().map(|object| object.token()));
        super::record::track(ctx, cell.object.get());
    }
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
                match unsafe {
                    #[cfg(not(feature = "ownership-trace"))]
                    {
                        cell.receive(&mut state, message)
                    }
                    #[cfg(feature = "ownership-trace")]
                    {
                        cell.receive_observed(&mut state, message, (*ctx).ownership_task)
                    }
                } {
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
                    cell.object.get().expect("a recorded channel identity"),
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
    // A canceled receive takes no message: a send may have spent its wake
    // on this receiver, so the next registered one is served.
    let next = if cause.is_some() {
        cell.pass_on(&mut state)
    } else {
        None
    };
    drop(state);
    drop(canceled);
    drop(replay);
    Some(publication.and_then(|result| result).and_then(|()| {
        if let Some(next) = next {
            unsafe { crate::pool::free::notify((*ctx).run_ptr(), &next) };
        }
        unsafe { tail(ctx) }
    }))
}

/// A role-free close's cut check: a replayed task whose cut this close
/// reached parks under the role before any unadmitted cleanup starts.
fn cut(ctx: *mut TaskCtx) {
    if unsafe { (*ctx).free_ops.replay.as_ref() }.is_some_and(|token| token.at_cut()) {
        let _role = unsafe { role::Guard::new(ctx) };
        super::replay::start(ctx);
    }
}

/// A failed body's kill of the resolved mutex it holds. Every registered
/// waiter leaves the queue in FIFO order with its token, under the latch, and
/// is woken after the unlock, as a put wakes its reserved successor.
/// # Safety
/// As for hirsel_rt_v2_mutex_kill.
pub(crate) unsafe fn mutex_kill(ctx: *mut TaskCtx, word: u64) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    let cell = Arc::as_ptr(unsafe { (*ctx).free_ops.mutexes.get(word) }?);
    let cell = unsafe { &*cell };
    let replay = replayed(ctx, Some((crate::free::MUTEX, word)))?;
    let replay = replay.map(|token| token.bind((crate::free::MUTEX, word)));
    // The chain's allocation precedes the section.
    let path = unsafe { (*ctx).free_ops.path.as_deref() }?;
    let chain = Arc::new(unsafe { (*ctx).failure.chain(path) });
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let mut state = cell.lock();
    let mut waiters = Default::default();
    let mut tokens = Vec::new();
    let publication = super::mutex::touched(
        ctx,
        Phase::Done,
        cell,
        &mut state,
        replay.as_ref(),
        |state| {
            waiters = state.kill(chain);
            tokens = state.take_tokens();
            "kill"
        },
    );
    drop(state);
    drop(replay);
    Some(publication.and_then(|()| {
        let token = |task: u32| tokens.iter().find(|&&(waiter, _)| waiter == task);
        let mut unnotified = false;
        for &task in &waiters {
            match token(task) {
                Some((_, token)) => unsafe { crate::pool::free::notify((*ctx).run_ptr(), token) },
                None => unnotified = true,
            }
        }
        if !unnotified {
            return unsafe { tail(ctx) };
        }
        let _role = unsafe { role::Guard::new(ctx) };
        for &task in waiters.iter().filter(|&&task| token(task).is_none()) {
            super::mutex::notify(ctx, task);
        }
        unsafe { super::try_yield(&mut *ctx) }
    }))
}

/// A dead resolved mutex's cause: the chain is cell state, read under the
/// latch with the cause's version event, and continued after the unlock.
/// # Safety
/// As for hirsel_rt_v2_mutex_cause.
pub(crate) unsafe fn mutex_cause(ctx: *mut TaskCtx, word: u64) -> Option<Stopped<()>> {
    if !admitted(ctx) {
        return None;
    }
    let cell = Arc::as_ptr(unsafe { (*ctx).free_ops.mutexes.get(word) }?);
    let cell = unsafe { &*cell };
    let replay = replayed(ctx, Some((crate::free::MUTEX, word)))?;
    let replay = replay.map(|token| token.bind((crate::free::MUTEX, word)));
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let mut state = cell.lock();
    let chain = state.dead().expect("a dead mutex cause").clone();
    let publication = super::mutex::touched(
        ctx,
        Phase::Done,
        cell,
        &mut state,
        replay.as_ref(),
        |state| {
            state.permit.observe();
            "cause"
        },
    );
    drop(state);
    drop(replay);
    Some(publication.map(|()| {
        unsafe { (*ctx).failure.append_chain(chain.as_ref().clone()) };
        cut(ctx);
    }))
}

/// The last wrapper's close of a resolved mutex. The payload detaches once
/// under the latch; it is destroyed after the unlock, on this task at the
/// close's anchor. The lease keeps only the closed cell's storage.
/// # Safety
/// As for close_mutex.
pub(crate) unsafe fn mutex_close(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Option<Stopped<u8>> {
    if !admitted(ctx) {
        return None;
    }
    let cell = Arc::as_ptr(unsafe { (*ctx).free_ops.mutexes.get(word) }?);
    let cell = unsafe { &*cell };
    let replay = replayed(ctx, Some((crate::free::MUTEX, word)))?;
    let replay = replay.map(|token| token.bind((crate::free::MUTEX, word)));
    let empty = crate::channels::Ring::new(cell.element.size, 1);
    unsafe { (*ctx).free_ops.retired.reserve(1) };
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let mut state = cell.lock();
    super::record::track(ctx, cell.object.get());
    let publication = super::record::publish_with_replay(
        ctx,
        Phase::Done,
        replay.as_ref().map(|object| object.token()),
        |writer| {
            let version = state.permit.observe();
            if let Some(writer) = writer {
                writer.touch(
                    cell.object.get().expect("a recorded mutex identity"),
                    version,
                    0,
                    "close mutex",
                );
            } else if let Some(replay) = &replay {
                replay.touch("close mutex")?;
            }
            Ok(cell.close_locked(&mut state, empty))
        },
    );
    drop(state);
    drop(replay);
    let closed = match publication.and_then(|closed| closed) {
        Ok(closed) => closed,
        Err(stop) => return Some(Err(stop)),
    };
    unsafe { (*ctx).free_ops.retired.push(Retired::Mutex(word)) };
    let status = unsafe { super::mutex::destroy(ctx, closed, anchor, operation_site, cut) };
    // No detached image or recorder owner survives a permanent cut save.
    cut(ctx);
    Some(Ok(status))
}

/// The close of a resolved endpoint. Counts change and a last receiver's
/// messages detach once under the latch. A last sender's close wakes every
/// receiver parked with its token after the unlock, in park order. A receiver a send
/// already woke retries and sees Closed itself. Leftover messages are
/// destroyed on this task at the close's anchor, oldest first.
/// # Safety
/// As for close_endpoint.
pub(crate) unsafe fn channel_close(
    ctx: *mut TaskCtx,
    word: u64,
    anchor: u64,
    operation_site: u64,
) -> Option<Stopped<u8>> {
    if !admitted(ctx) {
        return None;
    }
    let endpoint = unsafe { (*ctx).free_ops.endpoints.get(word) }?;
    let (channel, sender) = (endpoint.channel, endpoint.sender);
    // The endpoint's lease ends with it: hold the cell past that.
    let cell = endpoint.cell.clone();
    let handle = (crate::free::CHANNEL, u64::from(channel));
    let replay = replayed(ctx, Some(handle))?.map(|token| token.bind(handle));
    unsafe { (*ctx).free_ops.retired.reserve(1) };
    begin(ctx, replay.as_ref().map(|object| object.token()));
    let outcome = if sender {
        "close sender"
    } else {
        "close receiver"
    };
    let mut state = cell.lock();
    super::record::track(ctx, cell.object.get());
    let publication = super::record::publish_with_replay(
        ctx,
        Phase::Done,
        replay.as_ref().map(|object| object.token()),
        |writer| {
            #[cfg(not(feature = "ownership-trace"))]
            let closed = cell.close_locked(&mut state, sender);
            #[cfg(feature = "ownership-trace")]
            let closed = cell.close_observed(&mut state, sender, unsafe { (*ctx).ownership_task });
            let version = cell.next_event();
            if let Some(writer) = writer {
                writer.touch(
                    cell.object.get().expect("a recorded channel identity"),
                    version,
                    0,
                    outcome,
                );
            } else if let Some(replay) = &replay {
                replay.touch(outcome)?;
            }
            Ok(closed)
        },
    );
    let unsent = matches!(&publication, Ok(Ok(closed)) if closed.unsent);
    let tokens = if unsent {
        state.take_tokens()
    } else {
        Default::default()
    };
    drop(state);
    drop(replay);
    let closed = match publication.and_then(|closed| closed) {
        Ok(closed) => closed,
        Err(stop) => return Some(Err(stop)),
    };
    unsafe { (*ctx).free_ops.endpoints.remove(word) };
    let last = closed.last;
    for (_, token) in &tokens {
        unsafe { crate::pool::free::notify((*ctx).run_ptr(), token) };
    }
    drop(tokens);
    unsafe {
        (*ctx)
            .free_ops
            .retired
            .push(Retired::Endpoint { word, last })
    };
    let inert = cell.element.inert;
    drop(cell);
    let status = unsafe {
        super::channel::destroy(ctx, closed.into_left(), inert, anchor, operation_site, cut)
    };
    cut(ctx);
    Some(Ok(status))
}
