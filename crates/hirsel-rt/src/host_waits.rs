//! Cooperative host waits of a compiled run (`tasks.cooperative@1`), with the interpreter's
//! schedule (`hirsel-driver`'s HostLibrary).
//!
//! Every capability request goes through [`issue`]. Under a cancellation (CONC-3) a blocking
//! request first checks program validity and authority through `io_checks`, then completes
//! `Canceled` with no provider request and no tape event. Otherwise it runs under a fresh token
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
use crate::io::{AdapterError, Binding, Completion, Request};

/// The parked requests of one run.
#[derive(Default)]
pub(crate) struct Waits {
    next: u64,
    parked: BTreeMap<u64, Parked>,
    /// Armed scope deadlines (CONC-3) by token: the scope and its timer request.
    armed: BTreeMap<u64, (u32, Request)>,
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
    let binding = unsafe { Binding::get(ctx) };
    crate::io_checks::cancel(binding.adapter.as_mut(), request, cause)
        .unwrap_or_else(|error| crate::io::adapter_refusal(binding, error))
}

/// Whether `request` can wait for the host and so park its task: a stdin read, a timer
/// wait, a process run, a child write, read or wait, or a network request.
pub(crate) fn may_park(request: &Request) -> bool {
    matches!(
        request,
        Request::ReadStdin { .. }
            | Request::SleepUntil { .. }
            | Request::ProcessRun { .. }
            | Request::ChildWrite { .. }
            | Request::ChildRead { .. }
            | Request::ChildWait { .. }
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

/// Arm scope `scope`'s deadline: a timer wait for `at` on `clock` under a token of its own,
/// issued whatever the running task's cancellation. The token, or `None` when the deadline
/// is already reached or its clock cannot be read, which expires the scope at once.
///
/// # Safety
/// `ctx` is live, its Binding bound, and no borrow of the Binding or scheduler is held.
pub(crate) unsafe fn arm(ctx: *mut RunCtx, scope: u32, clock: u64, at: u64) -> Option<u64> {
    let binding = unsafe { Binding::get(ctx) };
    let token = binding.waits.next;
    binding.waits.next += 1;
    binding.adapter.enter(token);
    let request = Request::SleepUntil {
        clock,
        deadline: at,
    };
    match binding.adapter.call(request.clone()) {
        Err(AdapterError::Blocked) => {
            binding.waits.armed.insert(token, (scope, request));
            Some(token)
        }
        Ok(Completion::Wait(_)) => {
            binding.adapter.finish(token);
            None
        }
        Ok(_) => crate::fault("a scope deadline's timer completed as another request"),
        Err(error) => crate::io::adapter_refusal(binding, error),
    }
}

/// The scopes whose deadline a poll finds due, by deadline with ties in arming order; each
/// leaves the armed set.
///
/// # Safety
/// `ctx` is live; with no Binding bound nothing is armed.
pub(crate) unsafe fn expired(ctx: *mut RunCtx) -> Vec<u32> {
    // SAFETY: the context is live.
    if unsafe { (*ctx).io_data() }.is_null() {
        return Vec::new();
    }
    let binding = unsafe { Binding::get(ctx) };
    if binding.waits.armed.is_empty() {
        return Vec::new();
    }
    let mut armed: Vec<(u64, u64)> = binding
        .waits
        .armed
        .iter()
        .map(|(token, (_, request))| match request {
            Request::SleepUntil { deadline, .. } => (*deadline, *token),
            _ => crate::fault("an armed deadline that is not a timer"),
        })
        .collect();
    armed.sort_unstable();
    let mut due = Vec::new();
    for (_, token) in armed {
        match binding.adapter.ready(token) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => crate::io::adapter_refusal(binding, error),
        }
        let (scope, request) = binding.waits.armed[&token].clone();
        binding.adapter.enter(token);
        match binding.adapter.call(request) {
            Err(AdapterError::Blocked) => continue,
            Ok(Completion::Wait(_)) => {}
            Ok(_) => crate::fault("a scope deadline's timer completed as another request"),
            Err(error) => crate::io::adapter_refusal(binding, error),
        }
        binding.adapter.finish(token);
        binding.waits.armed.remove(&token);
        due.push(scope);
    }
    due
}

/// A scope exited before its deadline: remove its timer, recording nothing.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` armed.
pub(crate) unsafe fn disarm(ctx: *mut RunCtx, token: u64) {
    let binding = unsafe { Binding::get(ctx) };
    binding.adapter.finish(token);
    binding.waits.armed.remove(&token);
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
    if binding.waits.parked.is_empty() && binding.waits.armed.is_empty() {
        return false;
    }
    match binding.adapter.wait() {
        Ok(woke) => woke,
        Err(error) => crate::io::adapter_refusal(binding, error),
    }
}
