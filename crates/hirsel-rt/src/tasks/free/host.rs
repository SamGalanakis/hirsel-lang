//! Request ownership and native recorded host phases. This foundation does
//! not admit general host operations: logical handle mapping, adapter routing
//! and completion replay must wrap it before production activation.
use super::super::*;
use crate::io::{Completion, Request};
use crate::io_keys::{KeySet, Mode, Positions};
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
    keys: KeySet,
    names: Vec<(u32, String)>,
    positions: Option<Positions>,
    attempt: u32,
}

/// # Safety
/// Recorded Free admission, scheduler role held, and no outstanding request.
pub(crate) unsafe fn prepare(ctx: *mut TaskCtx, request: Request) -> Stopped<()> {
    unsafe { prepare_with_keys(ctx, request, KeySet::coupled()) }
}

/// # Safety
/// As for prepare; keys are logical resources declared by the admitted adapter.
pub(crate) unsafe fn prepare_with_keys(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
) -> Stopped<()> {
    let shared = super::record::owner(ctx).expect("recorded host-phase admission");
    let names = keys
        .keys()
        .iter()
        .map(|(key, _)| {
            let name = format!("host-resource/{}", key.0);
            (shared.recorder.key(&name), name)
        })
        .collect();
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
            keys,
            names,
            positions: None,
            attempt: 0,
        }));
    }
    super::record::begin(ctx);
    let (path, entered) = super::record::position(ctx);
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() };
    pending.path = path;
    pending.entered = entered;
    let pending = pending as *mut Pending;
    super::record::publish(ctx, Phase::Entered, |writer| unsafe {
        let writer = writer.expect("a recorded host entry");
        writer.prepare(&(*pending).request);
        take_positions(&mut *pending, writer);
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
    unsafe { wait_keys(ctx) }?;
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
    assert!(unsafe {
        (*pending)
            .positions
            .as_ref()
            .expect("an active host transition")
            .ready()
    });
    let completion = {
        // No role, section, writer or session lock crosses the host call.
        let _segment = unsafe { role::Segment::new(ctx) };
        call(unsafe { (*pending).request.buffers() })
    };
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() };
    pending.encoded.clear();
    completion.encode(&mut pending.encoded);
    pending.completion = Some(completion);
    Ok(())
}

/// # Safety
/// The owner holds the role; the bounded call returned or registered a wait.
pub(crate) unsafe fn ended(ctx: *mut TaskCtx) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    let index = unsafe { (*pending).attempt };
    super::record::publish(ctx, Phase::Ended(index), |_| unsafe {
        (*pending)
            .positions
            .take()
            .expect("an active host transition")
            .end();
    })?;
    // Key positions ended in the same section as the publication. Wake
    // after leaving it, under scheduler ownership, with no key lock held.
    wake_retrying(ctx);
    Ok(())
}

fn take_positions(pending: &mut Pending, writer: &mut hirsel_free::concurrent::Writer<'_, '_>) {
    assert!(
        pending.positions.is_none(),
        "no key survives a readiness wait"
    );
    let positions = pending.shared.host_keys.take(&pending.keys);
    for ((_, mode, epoch), (id, name)) in positions.epochs().zip(&pending.names) {
        writer.key(*id, name, epoch, mode == Mode::Shared, pending.attempt);
    }
    pending.positions = Some(positions);
}

/// Wait without a section, a key lock, or an owner on the saved stack. Ending
/// a predecessor and registering this wait both require the scheduler role,
/// so the readiness check cannot lose an end notification before the save.
unsafe fn wait_keys(ctx: *mut TaskCtx) -> Stopped<()> {
    loop {
        crate::stop::observe(ctx)?;
        let ready = unsafe { (*ctx).pending_host.as_deref().unwrap() }
            .positions
            .as_ref()
            .expect("a host transition")
            .ready();
        if ready {
            return Ok(());
        }
        let s = scheduler(ctx);
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
    }
}

/// # Safety
/// The owner holds the role; the previous bounded call ended and readiness
/// permits one more bounded attempt of this same semantic operation.
pub(crate) unsafe fn attempt(ctx: *mut TaskCtx) -> Stopped<()> {
    let pending = unsafe { (*ctx).pending_host.as_deref_mut().unwrap() } as *mut Pending;
    let index = unsafe {
        (*pending)
            .attempt
            .checked_add(1)
            .expect("host attempt overflow")
    };
    super::record::publish(ctx, Phase::Attempt(index), |writer| unsafe {
        (*pending).attempt = index;
        take_positions(&mut *pending, writer.expect("a recorded host attempt"));
    })?;
    unsafe { wait_keys(ctx) }
}

/// # Safety
/// The owner holds the role; its call returned and Ended was published.
pub(crate) unsafe fn complete(ctx: *mut TaskCtx) -> Stopped<Completion> {
    let pending = unsafe { (*ctx).pending_host.as_deref().unwrap() } as *const Pending;
    assert!(
        unsafe { (*pending).positions.is_none() },
        "end positions before completion"
    );
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
