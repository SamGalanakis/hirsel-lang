//! Request ownership and native recorded host phases. This foundation does
//! not admit general host operations: key transitions, logical handle mapping
//! and completion replay must wrap it before production activation.
use super::super::*;
use crate::io::{Completion, Request};
use crate::tape_wire::Wire;
use hirsel_free::claim::Claim;
use hirsel_free::phase::Progress;
use std::sync::Arc;

pub(crate) struct Pending {
    shared: Arc<super::record::Recording>,
    request: Arc<hirsel_free::claim::Request<Request>>,
    path: Vec<u32>,
    entered: Progress,
    completion: Option<Completion>,
    encoded: Vec<u8>,
}

/// # Safety
/// Recorded Free admission, scheduler role held, and no outstanding request.
pub(crate) unsafe fn prepare(ctx: *mut TaskCtx, request: Request) -> Stopped<()> {
    let shared = super::record::owner(ctx).expect("recorded host-phase admission");
    assert!(unsafe { (*ctx).pending_host.is_none() });
    let request = shared.prepare_request(request);
    // The task owns all buffers before begin can reject a replay cut or
    // another runtime entry can observe an external stop.
    unsafe {
        (*ctx).pending_host = Some(Box::new(Pending {
            shared,
            request,
            path: Vec::new(),
            entered: Progress::START,
            completion: None,
            encoded: Vec::new(),
        }));
    }
    super::record::begin(ctx);
    let (path, entered) = super::record::position(ctx);
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() };
    pending.path = path;
    pending.entered = entered;
    let pending = pending as *const Pending;
    super::record::publish(ctx, Phase::Entered, |writer| {
        writer
            .expect("a recorded host entry")
            .prepare(unsafe { &(*pending).request });
    })
}

/// # Safety
/// The owner holds the role and has published its prepared request.
pub(crate) unsafe fn claim(ctx: *mut TaskCtx) -> Stopped<bool> {
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    let durable = unsafe {
        (&(*pending).shared)
            .recorder
            .durable_through(&(*pending).path, (*pending).entered)
    }
    .map_err(|error| {
        Stop::Environmental(hirsel_run_schema::Environmental::Recording(
            error.to_string(),
        ))
    })?;
    if !durable {
        return Err(Stop::Requested);
    }
    super::record::publish(ctx, Phase::Claimed, |writer| unsafe {
        (&(*pending).shared)
            .recorder
            .claim(writer.expect("a recorded claim"), &(*pending).request)
    })
}

/// # Safety
/// The owner holds the role and has claimed its request. The bounded host
/// call neither suspends Hirsel nor retains its borrowed request buffers.
pub(crate) unsafe fn invoke(
    ctx: *mut TaskCtx,
    call: impl FnOnce(&Request) -> Completion,
) -> Stopped<()> {
    crate::stop::observe(ctx)?;
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    assert_eq!(
        unsafe { (*pending).request.state() },
        Claim::ClaimedForIssue
    );
    let completion = {
        // No role, section, writer or session lock crosses the host call.
        let _segment = unsafe { role::Segment::new(ctx) };
        call(unsafe { (*pending).request.buffers() })
    };
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() };
    completion.encode(&mut pending.encoded);
    pending.completion = Some(completion);
    Ok(())
}

/// # Safety
/// The owner holds the role; the bounded call returned or registered a wait.
pub(crate) unsafe fn ended(ctx: *mut TaskCtx) -> Stopped<()> {
    super::record::publish(ctx, Phase::Ended(0), |_| ())
}

/// # Safety
/// The owner holds the role; its call returned and Ended was published.
pub(crate) unsafe fn complete(ctx: *mut TaskCtx) -> Stopped<Completion> {
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    super::record::publish(ctx, Phase::Done, |writer| unsafe {
        writer
            .expect("a recorded completion")
            .complete(&(*pending).request, &(*pending).encoded);
    })?;
    let durable = unsafe {
        (&(*pending).shared).recorder.durable_through(
            &(*pending).path,
            Progress::new((*pending).entered.op, Phase::Done),
        )
    }
    .map_err(|error| {
        Stop::Environmental(hirsel_run_schema::Environmental::Recording(
            error.to_string(),
        ))
    })?;
    if !durable {
        return Err(Stop::Requested);
    }
    // A terminal cut rejects this publication, so no late completion reaches
    // its caller. The task retains it until teardown on the error path.
    let mut pending = unsafe { (*ctx).pending_host.take().unwrap() };
    pending.shared.recorder.retire(pending.request.id);
    Ok(pending
        .completion
        .take()
        .expect("one returned host completion"))
}

#[cfg(test)]
mod tests;
