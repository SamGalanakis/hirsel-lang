//! Owner side of a blocking built-in call (S5/S9). The owner reserves job
//! credit after its key positions are ready and before claim, so no credit is
//! ever held by a task still waiting on a key. It then parks its saved
//! continuation, holding no role, section or compute worker, while the job
//! performs the one bounded call. A parked owner keeps its key positions:
//! they cover exactly that in-flight call and end at Ended, as before.
use super::concurrent::Service;
use super::jobs::{Output, Owned, Reservation};
use super::*;

/// Park on the request's row until `ready` holds. Stops propagate; the
/// request-owned job state is reclaimed or quarantined at teardown.
unsafe fn park_until(
    ctx: *mut TaskCtx,
    io: &Service,
    wait: u64,
    mut ready: impl FnMut() -> bool,
) -> Stopped<()> {
    while !ready() {
        unsafe { try_block(&mut *ctx, Wait::Host { token: wait }, false, 0) }?;
        io.retry(wait);
    }
    Ok(())
}

/// Reserve one job credit for `owned`, parking FIFO when the pool is full.
/// # Safety
/// The owner holds the role; `owned` is request-owned (it outlives a stop).
pub(super) unsafe fn reserve(
    ctx: *mut TaskCtx,
    io: &Service,
    wait: u64,
    owned: *mut Owned,
) -> Stopped<()> {
    let owned = unsafe { &mut *owned };
    if owned.reservation.is_none() {
        owned.reservation = Some(io.jobs.reserve(wait));
    }
    let reservation = owned.reservation.as_mut().unwrap() as *mut Reservation;
    unsafe { park_until(ctx, io, wait, || (*reservation).granted()) }
}

/// Submit the claimed attempt with its reserved credit and park until the
/// call actually returned; the owner is then the single delivery winner.
/// # Safety
/// As for reserve; the request is claimed and its positions are ready.
pub(super) unsafe fn call(
    ctx: *mut TaskCtx,
    io: &Service,
    wait: u64,
    owned: *mut Owned,
    request: &Arc<hirsel_free::claim::Request<Request>>,
    cause: Option<u32>,
) -> Stopped<Output> {
    unsafe { reserve(ctx, io, wait, owned) }?;
    let owned = unsafe { &mut *owned };
    let credit = owned
        .reservation
        .take()
        .and_then(Reservation::take)
        .expect("a granted job credit");
    let (adapter, lease, token) = (io.adapter.clone(), request.clone(), request.id);
    let slot = io.jobs.submit(credit, wait, move || {
        // The job owns the request lease: buffers outlive any abandoned owner.
        let request = lease.buffers();
        if let Some(cause) = cause
            && adapter.cancellable(token)
        {
            match crate::io_checks::cancel_concurrent(&*adapter, token, request, cause) {
                Ok(Some(completion)) => return (Ok(completion), None),
                Err(error) => return (Err(error), None),
                Ok(None) => {}
            }
        }
        adapter.call_observed_at(token, request.clone())
    });
    // An abandoned frame never runs Drop: only the request owns the slot.
    owned.slot = Some(slot);
    let owned = owned as *mut Owned;
    unsafe { park_until(ctx, io, wait, || (*owned).slot.as_ref().unwrap().returned()) }?;
    let owned = unsafe { &mut *owned };
    Ok(owned
        .slot
        .take()
        .unwrap()
        .deliver()
        .expect("one owner delivery"))
}

/// Recorded attempt: the job result enters the same resolution, discovery
/// and encoding as a synchronous bounded call.
/// # Safety
/// As for invoke_concurrent; Pending's job state was reserved before claim.
pub(super) unsafe fn invoke(ctx: *mut TaskCtx, io: &Service, wait: u64) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    let Some(owned) = (unsafe { (*pending).job.as_mut() }) else {
        return unsafe { invoke_concurrent(ctx, &*io.adapter) };
    };
    let owned = owned as *mut Owned;
    let cause = unsafe { (*pending).canceled };
    let (completion, count) = unsafe { call(ctx, io, wait, owned, &(*pending).request, cause) }?;
    // Task-owned from here: an observed stop leaves it for teardown.
    unsafe { (*pending).completion = Some(completion) };
    let adapter = &*io.adapter;
    unsafe {
        invoke_discovered_observed(
            ctx,
            |_| ((*pending).completion.take().unwrap(), count),
            |handle| adapter.identity(handle),
            |handle| adapter.process_keys(handle),
            |handle| adapter.net_ports(handle),
        )
    }
}

/// # Safety
/// The owner holds the role, its positions are ready and it has not claimed.
pub(super) unsafe fn before_claim(ctx: *mut TaskCtx, io: &Service, wait: u64) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    let blocking = {
        let _segment = unsafe { role::Segment::new(ctx) };
        io.adapter.blocking(unsafe { (*pending).request.buffers() })
    };
    if !blocking {
        return Ok(());
    }
    // Wait for key predecessors first: a credit never waits behind a key.
    unsafe { wait_keys(ctx) }?;
    let owned = unsafe { (*pending).job.insert(Owned::new(ctx)) } as *mut Owned;
    unsafe { reserve(ctx, io, wait, owned) }
}
