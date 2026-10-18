//! Cooperative host waits of a compiled run (`tasks.cooperative@1`), with the interpreter's
//! schedule (`hirsel-driver`'s HostLibrary).
//!
//! Every capability request goes through [`issue`]. Under a cancellation (CONC-3) a blocking
//! request (stdin, print, and the network operations that can wait) completes `Canceled` with
//! no host request and no tape event. Otherwise it runs under a fresh token
//! ([`Adapter::enter`](crate::io::Adapter::enter)). An adapter that cannot complete it yet
//! answers `AdapterError::Blocked`: under cooperative admission a host provider recorded its
//! descriptor interest or deadline under the token, and in replay the tape recorded the park.
//! The task then parks ([`crate::tasks::park_host`]): a network request yields first and parks
//! when it is next at the front, while a stdin read parks at once, as the interpreter parks
//! them. The scheduler polls timer waits by deadline (ties by suspension order), and other
//! waits in park order ([`poll`]). When the task is
//! canceled while its request has committed no effect, or the adapter finds the token ready,
//! the poll re-issues the request, so the effect and its tape event happen at the poll; the
//! completion waits here for the task to resume. With no runnable task the scheduler sleeps
//! in [`wait`] on every interest and the earliest deadline. Without cooperative admission a
//! host request blocks the thread, as it always did.
use std::collections::BTreeMap;

use crate::RunCtx;
use crate::io::{AdapterError, Binding, Completion, InputError, IoError, Request, WaitError};
use crate::net::NetError;

/// The parked requests of one run.
#[derive(Default)]
pub(crate) struct Waits {
    next: u64,
    parked: BTreeMap<u64, Parked>,
}
struct Parked {
    request: Request,
    /// The re-issued request's outcome, from the poll that ended the wait.
    completion: Option<Result<Completion, AdapterError>>,
}

/// The completion a canceled `request` returns without reaching the host, or `None` when it
/// is not a cancellation point. A canceled `tls.connect` or `tls.close` still closes its
/// stream, which the call consumes.
///
/// # Safety
/// `ctx` is live and its Binding bound.
unsafe fn canceled(ctx: *mut RunCtx, request: &Request, cause: u32) -> Option<Completion> {
    let e = NetError::Canceled(cause);
    Some(match request {
        // A zero-byte read is invalid before it is a cancellation point.
        Request::ReadStdin { max: 0, .. } => Completion::Input(Err(InputError::InvalidInput)),
        Request::ReadStdin { .. } => Completion::Input(Err(InputError::Canceled(cause))),
        Request::Print { .. } | Request::PrintStderr { .. } => {
            Completion::Unit(Err(IoError::Canceled(cause)))
        }
        Request::SleepUntil { .. } => Completion::Wait(Err(WaitError::Canceled(cause))),
        Request::Resolve { .. } => Completion::Addresses(Err(e)),
        Request::Connect { .. } | Request::ConnectAddr { .. } => Completion::NetHandle(Err(e)),
        Request::Accept { .. } => Completion::Accepted(Err(e)),
        Request::NetRead { .. } => Completion::NetBytes(Err(e)),
        Request::NetWrite { .. } => Completion::Count(Err(e)),
        Request::NetWriteAll { .. } => Completion::NetUnit(Err(e)),
        Request::TlsConnect { stream, .. } | Request::TlsClose { stream, .. } => {
            let binding = unsafe { Binding::get(ctx) };
            let close = Request::Close { handle: *stream };
            match binding.adapter.call(close) {
                Ok(Completion::Unit(Ok(()))) => {}
                Ok(_) => crate::fault("a canceled TLS stream's close failed"),
                Err(error) => crate::io::adapter_refusal(binding, error),
            }
            Completion::NetUnit(Err(e))
        }
        _ => return None,
    })
}

/// Whether `request` can wait for the host and so park its task: a stdin read, a timer
/// wait or a network request.
pub(crate) fn may_park(request: &Request) -> bool {
    matches!(
        request,
        Request::ReadStdin { .. } | Request::SleepUntil { .. }
    ) || crate::net::is_net_request(request)
}

/// Perform `request` for the running task: canceled, completed, or parked until a poll
/// completes it. True with a parked request's outcome: its task already yielded or parked
/// there, so the operation's own yield is spent.
///
/// # Safety
/// `ctx` is live, its Binding bound, and no borrow of the Binding or scheduler is held.
pub(crate) unsafe fn issue(
    ctx: *mut RunCtx,
    request: Request,
) -> (Result<Completion, AdapterError>, bool) {
    if let Some(cause) = unsafe { crate::tasks::canceled(ctx) }
        && let Some(completion) = unsafe { canceled(ctx, &request, cause) }
    {
        return (Ok(completion), false);
    }
    let binding = unsafe { Binding::get(ctx) };
    if !may_park(&request) {
        return (binding.adapter.call(request), false);
    }
    let token = binding.waits.next;
    binding.waits.next += 1;
    binding.adapter.enter(token);
    let yielded = !matches!(request, Request::ReadStdin { .. });
    let retry = request.clone();
    match binding.adapter.call(request) {
        Err(AdapterError::Blocked) => {}
        result => {
            binding.adapter.finish(token);
            return (result, false);
        }
    }
    binding.waits.parked.insert(
        token,
        Parked {
            request: retry,
            completion: None,
        },
    );
    // SAFETY: the context is live and nothing is borrowed across the park.
    unsafe { crate::tasks::park_host(ctx, token, yielded) };
    let binding = unsafe { Binding::get(ctx) };
    let completion = binding
        .waits
        .parked
        .remove(&token)
        .and_then(|parked| parked.completion)
        .unwrap_or_else(|| crate::fault("a host wait resumed without its completion"));
    (completion, true)
}

/// Original absolute timer ticks for scheduler ordering, alike in live runs and replay.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` parked.
pub(crate) unsafe fn deadline(ctx: *mut RunCtx, token: u64) -> Option<u64> {
    let binding = unsafe { Binding::get(ctx) };
    match &binding.waits.parked.get(&token)?.request {
        Request::SleepUntil { deadline, .. } => Some(*deadline),
        _ => None,
    }
}

/// The scheduler's poll of a task parked on `token`, `cause` its cancellation: whether the
/// wait is over, after re-issuing its request when it is.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` parked.
pub(crate) unsafe fn poll(ctx: *mut RunCtx, token: u64, cause: Option<u32>) -> bool {
    let binding = unsafe { Binding::get(ctx) };
    let request = match binding.waits.parked.get(&token) {
        Some(parked) => parked.request.clone(),
        None => crate::fault("a poll of an unknown host wait"),
    };
    // A request that already committed part of its effect completes with its own outcome.
    let completion = match cause.filter(|_| binding.adapter.cancellable(token)) {
        Some(cause) => {
            binding.adapter.finish(token);
            // SAFETY: no borrow of the Binding is held across this call.
            match unsafe { canceled(ctx, &request, cause) } {
                Some(completion) => Ok(completion),
                None => crate::fault("a parked request that is no cancellation point"),
            }
        }
        None => {
            match binding.adapter.ready(token) {
                Ok(true) => {}
                Ok(false) => return false,
                Err(error) => crate::io::adapter_refusal(binding, error),
            }
            binding.adapter.enter(token);
            match binding.adapter.call(request) {
                Err(AdapterError::Blocked) => return false,
                result => {
                    binding.adapter.finish(token);
                    result
                }
            }
        }
    };
    let binding = unsafe { Binding::get(ctx) };
    if let Some(parked) = binding.waits.parked.get_mut(&token) {
        parked.completion = Some(completion);
    }
    true
}

/// With no runnable task: sleep until some parked request's descriptor is ready or the
/// earliest deadline passes (in replay, until the recording's next ready poll). False when
/// nothing can wake a task.
///
/// # Safety
/// `ctx` is live and its Binding bound.
pub(crate) unsafe fn wait(ctx: *mut RunCtx) -> bool {
    let binding = unsafe { Binding::get(ctx) };
    if binding.waits.parked.is_empty() {
        return false;
    }
    match binding.adapter.wait() {
        Ok(woke) => woke,
        Err(error) => crate::io::adapter_refusal(binding, error),
    }
}
