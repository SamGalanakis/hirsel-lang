//! Owned progress of one entered ConcurrentAdapter operation. The task owns
//! this record before begin; callbacks and home loans finish before Pending.
//! Tokio's Idle/Busy file state (ce685ee6, fs/file.rs:649-712) supplies the
//! ownership shape. Ordered adds canonical retirement and actual save gates.
use super::super::super::super::inop::{self, Drive};
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Prepared,
    Begun,
    JobWaiting,
    InitialOutcome,
    Wait,
    Finish,
    FinishHead,
    Deliver,
    Delivered,
    Failed,
}

pub(in crate::tasks::ordered::host::concurrent) struct Invocation {
    identity: u64,
    token: u64,
    observing: bool,
    cause: Option<u32>,
    phase: Phase,
    job: bool,
    discovery: Option<Discovered>,
    output: Option<Result<Option<Vec<u8>>, &'static str>>,
}

/// Start reserves the record and park continuation before accepting an effect.
/// # Safety
/// The task holds its role, with no earlier outstanding host delivery.
pub(super) unsafe fn start(ctx: *mut TaskCtx, request: Request) {
    assert!(unsafe { (*ctx).pending_ordered_host.is_none() });
    let identity = engine::current(ctx);
    unsafe {
        (*ctx).pending_ordered_host = Some(Box::new(Pending {
            request: Some(request),
            result: None,
            home: None,
            invocation: Some(Invocation {
                identity,
                token: 0,
                observing: false,
                cause: None,
                phase: Phase::Prepared,
                job: false,
                discovery: None,
                output: None,
            }),
        }));
    }
    inop::start(ctx, inop::Operation::Host(inop::Host::new(0, false)));
}

// Each access ends before engine entry, callbacks or suspension. Pending's
// stable allocation owns carriers; no pointer to a movable TaskCtx is saved.
fn with<T>(ctx: *mut TaskCtx, f: impl FnOnce(&mut Pending, &mut Invocation) -> T) -> T {
    let pending = unsafe { (*ctx).pending_ordered_host.as_deref_mut().unwrap() };
    // Temporarily take only the fixed phase carrier, never the request/result.
    // This helper performs bounded field access and cannot stop or call users.
    let mut invocation = pending.invocation.take().unwrap();
    let value = f(pending, &mut invocation);
    assert!(pending.invocation.replace(invocation).is_none());
    value
}

fn phase(ctx: *mut TaskCtx, phase: Phase) {
    with(ctx, |_, i| i.phase = phase);
}

fn request_ptr(ctx: *mut TaskCtx) -> *const Request {
    (unsafe {
        (*ctx)
            .pending_ordered_host
            .as_deref()
            .unwrap()
            .request
            .as_ref()
            .unwrap()
    }) as *const Request
}
fn result_ptr(ctx: *mut TaskCtx) -> *const Result<Completion, AdapterError> {
    (unsafe {
        (*ctx)
            .pending_ordered_host
            .as_deref()
            .unwrap()
            .result
            .as_ref()
            .unwrap()
    }) as *const _
}

/// The caller reacquires context and service on every drive. The result stays
/// owned until the single delivery; errors retain a valid terminal phase.
/// # Safety
/// The current task owns the invocation and its original request/wait row.
pub(super) unsafe fn drive(ctx: *mut TaskCtx) -> Stopped<Drive<Result<Completion, AdapterError>>> {
    loop {
        let (identity, token, observing, cause, state) = with(ctx, |_, i| {
            (i.identity, i.token, i.observing, i.cause, i.phase)
        });
        assert_eq!(
            identity,
            inop::identity(ctx),
            "host invocation task generation"
        );
        let io = unsafe { (*ctx).run_ref().ordered_io.get().unwrap() };
        let adapter = &*io.adapter;
        match state {
            Phase::Prepared => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                super::super::super::begin(ctx, false)?;
                let (token, observing) = engine::with_driver(ctx, |d| {
                    let token = d.next_host_token;
                    assert_eq!(token >> 48, 0);
                    d.next_host_token = token.checked_add(1).unwrap();
                    (token, d.events.is_some())
                });
                with(ctx, |_, i| {
                    i.token = token;
                    i.observing = observing;
                });
                let request = unsafe { &*request_ptr(ctx) };
                super::super::super::super::events::request_host(ctx, request);
                if observing {
                    let (keys, domains) = {
                        let _segment = unsafe { role::Segment::new(ctx) };
                        (adapter.keys(request), adapter.domains(request))
                    };
                    super::super::super::super::events::request_domains(ctx, domains);
                    super::super::super::super::events::request_keys(ctx, &keys);
                    super::super::super::super::events::concurrent_phases(ctx);
                }
                let cause = unsafe { crate::tasks::canceled(ctx) };
                with(ctx, |_, i| {
                    i.cause = cause;
                    i.phase = Phase::Begun;
                });
            }
            Phase::Begun => {
                // A stop selected during metadata must prevent a new attempt.
                // After the attempt, InitialOutcome preserves evidence first.
                unsafe { crate::stop::observe(ctx) }?;
                let request = unsafe { &*request_ptr(ctx) };
                let blocking = adapter.blocking(request);
                crate::stop::observe(ctx)?;
                if blocking {
                    let request = with(ctx, |p, _| p.request.take().unwrap());
                    let wait = token | CONCURRENT;
                    let mut owned = unsafe { Owned::new(ctx) };
                    owned.reservation = Some(io.jobs.reserve(wait));
                    assert!(
                        io.rows
                            .lock()
                            .unwrap()
                            .insert(wait, Row::new(request, Some(owned)))
                            .is_none()
                    );
                    io.pending.fetch_add(1, Ordering::Release);
                    let _ = io.poll_job(wait, cause, false);
                    with(ctx, |_, i| {
                        i.phase = Phase::JobWaiting;
                        i.job = true;
                    });
                    engine::begin_host_job(ctx, wait);
                    return Ok(Drive::Pending { head_only: true });
                }
                let (result, discovered, output) = {
                    let _segment = unsafe { role::Segment::new(ctx) };
                    invoke_attempt(adapter, token, cause, request.clone(), observing)
                };
                let finish = result != Err(AdapterError::Blocked) || discovered.is_err();
                with(ctx, |p, i| {
                    assert!(p.result.replace(result).is_none());
                    i.discovery = Some(discovered);
                    i.output = Some(output);
                    i.phase = Phase::InitialOutcome;
                });
                if finish {
                    let _segment = unsafe { role::Segment::new(ctx) };
                    adapter.finish(token);
                }
            }
            Phase::JobWaiting => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                let Row {
                    request,
                    completion,
                    written,
                    ..
                } = io
                    .rows
                    .lock()
                    .unwrap()
                    .remove(&(token | CONCURRENT))
                    .unwrap();
                assert!(io.pending.fetch_sub(1, Ordering::Release) > 0);
                let result = completion.expect("completed head job");
                let output = if observing {
                    let bytes = match &request {
                        Request::Print { bytes, .. } | Request::PrintStderr { bytes, .. } => {
                            Some(bytes.as_slice())
                        }
                        _ => None,
                    };
                    super::super::super::super::events::host_output::project(
                        bytes, &result, written,
                    )
                } else {
                    Ok(None)
                };
                with(ctx, |p, _| {
                    assert!(p.request.replace(request).is_none());
                    assert!(p.result.replace(result).is_none());
                });
                let discovered = if observing {
                    let result = with(ctx, |p, _| p.result.as_mut().unwrap() as *mut _);
                    let _segment = unsafe { role::Segment::new(ctx) };
                    discover(unsafe { &mut *result }, adapter)
                } else {
                    Ok(BTreeMap::new())
                };
                with(ctx, |_, i| {
                    i.discovery = Some(discovered);
                    i.output = Some(output);
                    i.phase = Phase::InitialOutcome;
                });
            }
            Phase::InitialOutcome => {
                let (discovered, job) = with(ctx, |_, i| (i.discovery.take().unwrap(), i.job));
                let result = unsafe { &*result_ptr(ctx) };
                let discovery = discovered.and_then(|d| {
                    super::super::super::super::events::discover_host(ctx, result, &d)
                });
                super::super::super::super::events::record_host_initial(ctx, result);
                let output = with(ctx, |_, i| {
                    i.output.take().expect("original attempt output")
                });
                super::super::super::super::events::host_output::record(ctx, output);
                if let Err(error) = discovery {
                    if !job && result != &Err(AdapterError::Blocked) {
                        super::super::super::super::events::complete_host(ctx, false, result);
                    }
                    phase(ctx, Phase::Failed);
                    return Err(Stop::Refused(error.into()));
                }
                if result == &Err(AdapterError::Blocked) {
                    with(ctx, |p, _| {
                        p.result.take();
                    });
                    let request = with(ctx, |p, _| p.request.take().unwrap());
                    let yielded = !matches!(
                        request,
                        Request::ReadStdin { .. } | Request::WatchNext { .. }
                    );
                    let wait = token | CONCURRENT;
                    assert!(
                        io.rows
                            .lock()
                            .unwrap()
                            .insert(wait, Row::new(request, None))
                            .is_none()
                    );
                    io.pending.fetch_add(1, Ordering::Release);
                    inop::set_host_wait(ctx, wait, yielded);
                    phase(ctx, Phase::Wait);
                } else {
                    let request = unsafe { &*request_ptr(ctx) };
                    if !job && matches!(request, Request::NetRead { .. }) {
                        super::super::super::bounded_read(ctx, request, result);
                    }
                    super::super::super::super::events::complete_host(ctx, false, result);
                    if matches!(request, Request::Print { .. } | Request::PrintStderr { .. })
                        && observing
                    {
                        let _ = super::super::super::super::events::certify_host(ctx);
                    }
                    // The request's ownership stays valid across the finish wait.
                    phase(ctx, Phase::Finish);
                }
            }
            Phase::Wait => {
                match unsafe { inop::drive_host(ctx) }? {
                    Drive::Pending { head_only } => return Ok(Drive::Pending { head_only }),
                    Drive::Complete(()) => {}
                }
                let wait = token | CONCURRENT;
                let result = if super::super::super::return_pending(ctx, wait) {
                    io.rows
                        .lock()
                        .unwrap()
                        .get(&wait)
                        .unwrap()
                        .completion
                        .clone()
                } else {
                    let row = io.rows.lock().unwrap().remove(&wait).unwrap();
                    assert!(io.pending.fetch_sub(1, Ordering::Release) > 0);
                    row.completion
                }
                .expect("completed saved wait");
                with(ctx, |p, i| {
                    assert!(p.result.replace(result).is_none());
                    i.phase = Phase::Deliver;
                });
                super::super::super::super::events::complete_host(ctx, false, unsafe {
                    &*result_ptr(ctx)
                });
            }
            Phase::Finish => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                let (progress, private) = super::super::super::finish_start(ctx, Next::Yield)?;
                phase(
                    ctx,
                    if private {
                        Phase::Deliver
                    } else {
                        Phase::FinishHead
                    },
                );
                if private {
                    engine::continue_after_read(ctx, progress)?;
                } else if progress == crate::ordered::Progress::Poll {
                    engine::poll(ctx)?;
                }
            }
            Phase::FinishHead => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                phase(ctx, Phase::Deliver);
            }
            Phase::Deliver => {
                return Ok(Drive::Complete(with(ctx, |p, i| {
                    let result = p.result.take().expect("one host delivery");
                    i.phase = Phase::Delivered;
                    result
                })));
            }
            Phase::Delivered | Phase::Failed => {
                unreachable!("terminal host invocation driven again")
            }
        }
    }
}

/// Existing stackful entry consumes the same owned drive outcomes. No guard
/// on this stack is the sole owner of request, phase or completion resources.
pub(super) unsafe fn request(
    ctx: *mut TaskCtx,
    request: Request,
) -> Stopped<Result<Completion, AdapterError>> {
    unsafe { start(ctx, request) };
    let result = loop {
        match unsafe { drive(ctx) } {
            Ok(Drive::Complete(value)) => break Ok(value),
            Ok(Drive::Pending { head_only }) => {
                engine::suspend(ctx, engine::current(ctx), head_only)
            }
            Err(stop) => break Err(stop),
        }
    };
    inop::finish(ctx);
    unsafe {
        (*ctx).pending_ordered_host.take();
    }
    result
}

#[cfg(test)]
#[path = "drive/modules.rs"]
mod test_modules;
