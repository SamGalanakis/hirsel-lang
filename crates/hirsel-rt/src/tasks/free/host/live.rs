//! Recording-off host ownership. Reuse F0's request claim/withdraw CAS and the
//! same bounded key transitions/readiness service, with no recorder, encoder,
//! operation ordinal, section gate or tape allocation.
use super::*;
use hirsel_free::claim::{Requests, Sweep};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct State {
    requests: Requests<Request>,
    keys: crate::io_keys::KeyTable,
    next: AtomicU64,
    terminal: Mutex<Option<Sweep>>,
}

impl crate::stop::Terminal for State {
    fn stop(&self, _: &Stop) -> std::io::Result<()> {
        let mut terminal = self.terminal.lock().unwrap();
        if terminal.is_none() {
            *terminal = Some(self.requests.sweep());
        }
        Ok(())
    }
}

pub(crate) struct Pending {
    state: Arc<State>,
    request: Arc<hirsel_free::claim::Request<Request>>,
    keys: KeySet,
    positions: Option<Positions>,
    completion: Option<Result<Completion, AdapterError>>,
    job: Option<super::jobs::Owned>,
}

/// # Safety
/// Native live Free owner holds the role. The service owns all adapter state;
/// task-owned Pending retains every result/buffer through suspension or stop.
pub(super) unsafe fn request(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
) -> Stopped<Result<Completion, AdapterError>> {
    let io = unsafe { &**(*ctx).run_ref().free_io.get().expect("live Free service") };
    let state = io.live.clone();
    let token = state.next.fetch_add(1, Ordering::Relaxed);
    assert_ne!(token, u64::MAX, "live host token overflow");
    let request = state.requests.prepare(token, request);
    assert!(unsafe { (*ctx).live_host.is_none() });
    unsafe {
        (*ctx).live_host = Some(Box::new(Pending {
            state,
            request,
            keys,
            positions: None,
            completion: None,
            job: None,
        }));
    }
    super::super::record::begin(ctx);
    let pending = unsafe { (*ctx).live_host.as_deref_mut().unwrap() } as *mut Pending;
    let wait = io.register(token, unsafe { (*pending).request.buffers().clone() });
    let blocking = {
        let _segment = unsafe { role::Segment::new(ctx) };
        io.adapter.blocking(unsafe { (*pending).request.buffers() })
    };
    if blocking {
        unsafe { (*pending).job = Some(super::jobs::Owned::new(ctx)) };
    }
    let mut first = true;
    loop {
        unsafe {
            (*pending).positions = Some((&(*pending).state).keys.take(&(*pending).keys));
        }
        while !unsafe { (*pending).positions.as_ref().unwrap().ready() } {
            // The predecessor ends under the same role and wakes this list.
            // Register before saving so its end cannot strand a live waiter.
            let s = scheduler(ctx);
            let me = s.current;
            if !s.retrying.contains(&me) {
                s.retrying.push(me);
            }
            unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
        }
        crate::stop::observe(ctx)?;
        if let Some(owned) = unsafe { (*pending).job.as_mut() } {
            // Positions are ready: reserve job credit before claim.
            unsafe { blocking::reserve(ctx, io, wait, owned) }?;
            crate::stop::observe(ctx)?;
        }
        if first && !unsafe { (*pending).request.claim() } {
            return Err(Stop::Requested);
        }
        first = false;
        let cause =
            if crate::io_checks::cancellation_result(unsafe { (*pending).request.buffers() }, 0)
                .is_some()
            {
                scheduler(ctx).canceled(unsafe { (*ctx).shield })
            } else {
                None
            };
        if let Some(owned) = unsafe { (*pending).job.as_mut() } {
            let request = unsafe { &(*pending).request };
            let (result, _) = unsafe { blocking::call(ctx, io, wait, owned, request, cause) }?;
            unsafe { (*pending).completion = Some(result) };
        } else {
            let _segment = unsafe { role::Segment::new(ctx) };
            let request = unsafe { (*pending).request.buffers() };
            let result = if let Some(cause) = cause
                && io.adapter.cancellable(token)
            {
                match crate::io_checks::cancel_concurrent(&*io.adapter, token, request, cause) {
                    Ok(Some(result)) => Ok(result),
                    Err(error) => Err(error),
                    Ok(None) => io.adapter.call_at(token, request.clone()),
                }
            } else {
                io.adapter.call_at(token, request.clone())
            };
            unsafe {
                (*pending).completion = Some(result);
            }
        }
        unsafe {
            (*pending).positions.take().unwrap().end();
        }
        wake_retrying(ctx);
        crate::stop::observe(ctx)?;
        if !matches!(
            unsafe { &(*pending).completion },
            Some(Err(AdapterError::Blocked))
        ) {
            break;
        }
        {
            let _segment = unsafe { role::Segment::new(ctx) };
            io.adapter.parked(token);
        }
        unsafe { try_block(&mut *ctx, Wait::Host { token: wait }, false, 0) }?;
        io.retry(wait);
    }
    {
        let _segment = unsafe { role::Segment::new(ctx) };
        io.adapter.finish(token);
    }
    io.finish(wait);
    // Stop and completion arbitrate here without a section gate. A completion
    // that loses remains task-owned and is quarantined until reclamation.
    {
        let state = unsafe { &(*pending).state };
        let terminal = state.terminal.lock().unwrap();
        if terminal.is_some() {
            return Err(Stop::Requested);
        }
        unsafe {
            (*pending).request.finish();
        }
        state.requests.retire(token);
    }
    let mut pending = unsafe { (*ctx).live_host.take().unwrap() };
    let result = pending.completion.take().unwrap();
    native::observe_span(ctx, pending.request.buffers(), &result)?;
    Ok(result)
}
