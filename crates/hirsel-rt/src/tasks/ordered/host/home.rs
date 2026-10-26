//! Home/replay attempt custody. Home callbacks return before Pending; the
//! original adapter token and completed row remain the sole outcome owner.
use super::super::inop::{self, Drive};
use super::*;
use crate::io::{AdapterError, Completion, Request};

#[derive(Clone, Copy)]
enum Phase {
    Prepared,
    Attempt,
    Wait(u64),
    Complete,
    Close,
    CloseHead,
    Deliver,
    Delivered,
}

pub(super) struct Invocation {
    identity: u64,
    phase: Phase,
    close: bool,
    print: bool,
    read: Option<Request>,
    parked: bool,
    finish_in_drive: bool,
}

fn with<T>(ctx: *mut TaskCtx, f: impl FnOnce(&mut concurrent::Pending, &mut Invocation) -> T) -> T {
    let pending = unsafe { (*ctx).pending_ordered_host.as_deref_mut().unwrap() };
    let mut invocation = pending.home.take().unwrap();
    let result = f(pending, &mut invocation);
    assert!(pending.home.replace(invocation).is_none());
    result
}
fn phase(ctx: *mut TaskCtx, next: Phase) {
    with(ctx, |_, i| i.phase = next);
}

unsafe fn start(ctx: *mut TaskCtx, request: Request, finish_in_drive: bool) {
    let close = matches!(request, Request::Close { .. });
    let print = matches!(request, Request::Print { .. } | Request::PrintStderr { .. });
    let read = matches!(request, Request::NetRead { .. }).then(|| request.clone());
    let identity = engine::current(ctx);
    assert!(unsafe { (*ctx).pending_ordered_host.is_none() });
    unsafe {
        (*ctx).pending_ordered_host = Some(Box::new(concurrent::Pending::home(
            request,
            Invocation {
                identity,
                phase: Phase::Prepared,
                close,
                print,
                read,
                parked: false,
                finish_in_drive,
            },
        )));
    }
    inop::start(ctx, inop::Operation::Host(inop::Host::new(0, false)));
}

unsafe fn drive(ctx: *mut TaskCtx) -> Stopped<Drive<(Result<Completion, AdapterError>, bool)>> {
    loop {
        let (identity, state, close) = with(ctx, |_, i| (i.identity, i.phase, i.close));
        assert_eq!(identity, inop::identity(ctx), "home invocation generation");
        match state {
            Phase::Prepared => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                begin(ctx, close)?;
                let request = with(ctx, |p, _| p.request.as_ref().unwrap() as *const Request);
                super::super::events::request_host(ctx, unsafe { &*request });
                if super::super::events::observing(ctx) {
                    unsafe { crate::host_waits::observe_keys(ctx, &*request) };
                }
                phase(ctx, Phase::Attempt);
            }
            Phase::Attempt => match unsafe { crate::host_waits::home_initial(ctx) }? {
                crate::host_waits::HomeOutcome::Complete(result) => {
                    with(ctx, |p, i| {
                        assert!(p.result.replace(result).is_none());
                        i.phase = Phase::Complete;
                    });
                }
                crate::host_waits::HomeOutcome::Wait { token, yielded } => {
                    inop::set_host_wait(ctx, token, yielded);
                    with(ctx, |_, i| {
                        i.parked = true;
                        i.phase = Phase::Wait(token);
                    });
                }
            },
            Phase::Wait(token) => {
                match unsafe { inop::drive_host(ctx) }? {
                    Drive::Pending { head_only } => return Ok(Drive::Pending { head_only }),
                    Drive::Complete(()) => {}
                }
                let result = unsafe { crate::host_waits::home_completion(ctx, token) };
                with(ctx, |p, i| {
                    assert!(p.result.replace(result).is_none());
                    i.phase = Phase::Complete;
                });
            }
            Phase::Complete => {
                let (result, read, parked, print, finish_in_drive) = with(ctx, |p, i| {
                    (
                        p.result.as_ref().unwrap() as *const _,
                        i.read.take(),
                        i.parked,
                        i.print,
                        i.finish_in_drive,
                    )
                });
                let result = unsafe { &*result };
                if !parked && let Some(read) = read {
                    bounded_read(ctx, &read, result);
                }
                super::super::events::complete_host(ctx, close, result);
                if print && super::super::events::observing(ctx) {
                    let _ = super::super::events::certify_host(ctx);
                }
                let finish = finish_in_drive && !parked;
                // Capability refusal precedes the post-effect finish, exactly
                // as in capability_call. Invalid authority still spends that
                // finish and returns language failure without initializing out.
                if finish_in_drive
                    && let Err(error) = result
                    && !matches!(error, AdapterError::InvalidHandle(_))
                {
                    return Err(crate::stop::Stop::refusal(error.clone()));
                }
                phase(
                    ctx,
                    if close || finish {
                        Phase::Close
                    } else {
                        Phase::Deliver
                    },
                );
            }
            Phase::Close => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                let (progress, private) =
                    finish_start(ctx, if close { Next::Keep } else { Next::Yield })?;
                assert!(!private);
                phase(ctx, Phase::CloseHead);
                if progress == Progress::Poll {
                    engine::poll(ctx)?;
                }
            }
            Phase::CloseHead => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                phase(ctx, Phase::Deliver);
            }
            Phase::Deliver => {
                return Ok(Drive::Complete(with(ctx, |p, i| {
                    i.phase = Phase::Delivered;
                    (p.result.take().unwrap(), i.parked)
                })));
            }
            Phase::Delivered => unreachable!("home outcome delivered once"),
        }
    }
}

pub(crate) unsafe fn request(
    ctx: *mut TaskCtx,
    request: Request,
) -> Stopped<(Result<Completion, AdapterError>, bool)> {
    unsafe { start(ctx, request, false) };
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

/// Retain a converted capability result through its post-effect finish. The
/// caller's ABI writeback still runs only after this owned continuation ends.
pub(crate) unsafe fn finish_result(
    ctx: *mut TaskCtx,
    completion: Result<Completion, ()>,
    operation: bool,
    parked: bool,
) -> Result<Completion, ()> {
    assert!(unsafe { (*ctx).pending_ordered_host.is_none() });
    let identity = inop::identity(ctx);
    let invocation = Invocation {
        identity,
        phase: Phase::Close,
        close: false,
        print: false,
        read: None,
        parked,
        finish_in_drive: false,
    };
    unsafe {
        (*ctx).pending_ordered_host = Some(Box::new(concurrent::Pending::home_result(
            completion.ok(),
            invocation,
        )));
    }
    inop::start(ctx, inop::Operation::Host(inop::Host::new(0, false)));
    let finish = if operation && !parked {
        loop {
            match unsafe { drive_finish(ctx) } {
                Ok(Drive::Complete(())) => break Ok(()),
                Ok(Drive::Pending { head_only }) => {
                    engine::suspend(ctx, engine::current(ctx), head_only)
                }
                Err(stop) => break Err(stop),
            }
        }
    } else {
        Ok(())
    };
    if !operation {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    inop::finish(ctx);
    unsafe { crate::stop::check(ctx, finish) };
    let mut pending = unsafe { (*ctx).pending_ordered_host.take().unwrap() };
    match pending.result.take() {
        Some(Ok(value)) => Ok(value),
        None => Err(()),
        Some(Err(_)) => unreachable!("converted home result"),
    }
}

/// One bounded post-effect finish step. A poll runs synchronously on its
/// original owner; only the surrounding head wait can return Pending.
unsafe fn drive_finish(ctx: *mut TaskCtx) -> Stopped<Drive<()>> {
    loop {
        let (identity, state) = with(ctx, |_, i| (i.identity, i.phase));
        assert_eq!(identity, inop::identity(ctx));
        match state {
            Phase::Close => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                let (progress, private) = finish_start(ctx, Next::Yield)?;
                phase(ctx, Phase::CloseHead);
                if private {
                    engine::continue_after_read(ctx, progress)?;
                    return Ok(Drive::Complete(()));
                }
                if progress == Progress::Poll {
                    engine::poll(ctx)?;
                }
            }
            Phase::CloseHead => {
                if !engine::head_step(ctx)? {
                    return Ok(Drive::Pending { head_only: true });
                }
                return Ok(Drive::Complete(()));
            }
            _ => unreachable!("home result finish phase"),
        }
    }
}

#[cfg(test)]
#[path = "home/modules.rs"]
mod test_modules;

/// A certified capability retains its result through the original finish.
pub(super) unsafe fn start_entered(ctx: *mut TaskCtx, request: Request) {
    unsafe { start(ctx, request, true) };
}
pub(super) unsafe fn drive_entered(
    ctx: *mut TaskCtx,
) -> Stopped<Drive<Result<Completion, AdapterError>>> {
    unsafe { drive(ctx) }.map(|outcome| match outcome {
        Drive::Complete((result, _)) => Drive::Complete(result),
        Drive::Pending { head_only } => Drive::Pending { head_only },
    })
}
