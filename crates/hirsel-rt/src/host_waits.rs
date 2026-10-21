//! Cooperative host waits of a compiled run (`tasks.cooperative@1`), with the interpreter's
//! schedule (`hirsel-driver`'s HostLibrary).
//!
//! Capability requests go through [`issue`] or the borrowed live network entry [`issue_net`]. Under a cancellation (CONC-3) a blocking
//! request first checks program validity and authority through `io_checks`, then completes
//! `Canceled` without the requested effect. Consumed-resource cleanup may issue Close and
//! report a scripted failure. Otherwise it runs under a fresh token
//! ([`Adapter::enter`](crate::io::Adapter::enter)). An adapter that cannot complete it yet
//! answers `AdapterError::Blocked`: under cooperative admission a host provider recorded its
//! descriptor interest or deadline under the token, and in replay the tape recorded the park.
//! The task then parks ([`crate::tasks::park_host`]): a network request yields first and parks
//! when it is next at the front, while a stdin read or a watcher's next event parks
//! at once, as the interpreter parks
//! them; the park tells the adapter ([`parked`]). Each scheduling point asks the adapter once
//! for the parked tokens that are ready ([`ready`]), so its cost does not grow with the parked
//! count, and the scheduler polls only those tasks and the canceled ones: timer waits by
//! deadline (ties by suspension order), other waits in park order ([`poll`]). When the task
//! is canceled while its request has committed no effect, or the token is ready, the poll
//! re-issues the request, so the effect and its tape event happen at the poll; the
//! completion waits here for the task to resume. With no runnable task the scheduler sleeps
//! in [`wait`] on every interest and the earliest deadline. Without cooperative admission a
//! host request blocks the thread, as it always did.
use crate::TaskCtx;
use crate::id_map::IdMap;
use crate::io::{AdapterError, Binding, Completion, Ready, Request};
use crate::stop::{Stop, Stopped};

/// Run `f` on the run's home thread, which holds the Io binding: the scheduling points of a
/// width > 1 run run on compute workers (`crate::pool`).
///
/// # Safety
/// `ctx` is live; `f` does not suspend.
unsafe fn at_home<T>(ctx: *mut TaskCtx, f: impl FnOnce() -> T) -> Stopped<T> {
    let (mut f, mut out) = (Some(f), None);
    // SAFETY: caller contract.
    unsafe {
        crate::pool::call_home(ctx, &mut || {
            out = f.take().map(|f| f());
            count(ctx);
        })
    }?;
    Ok(out.unwrap_or_else(|| crate::fault("a home call that did not run")))
}

/// Mirror the binding's parked and armed counts into the run ([`quiet`]), on the thread that
/// holds the binding.
///
/// # Safety
/// `ctx` is live.
unsafe fn count(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    unsafe {
        let live = if (*ctx).io_data().is_null() {
            0
        } else {
            let waits = &Binding::raw(ctx).waits;
            waits.parked.len() + waits.armed.len()
        };
        (*ctx)
            .run_ref()
            .host_waits
            .store(live as u32, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Whether the run parks no request and arms no deadline: a scheduling point asks its home
/// thread nothing. It reads the run's mirror of the binding's counts ([`count`]), never the
/// binding, which only its home thread reaches at width > 1.
///
/// # Safety
/// `ctx` is live.
unsafe fn quiet(ctx: *mut TaskCtx) -> bool {
    // SAFETY: caller contract.
    unsafe { (*ctx).run_ref() }
        .host_waits
        .load(std::sync::atomic::Ordering::Relaxed)
        == 0
}

/// The parked requests of one run.
#[derive(Default)]
pub(crate) struct Waits {
    next: u64,
    parked: IdMap<Parked>,
    /// Armed scope deadlines (CONC-3) by token: the scope and its timer request.
    armed: IdMap<(u32, Request)>,
}
struct Parked {
    request: Request,
    borrowed: Option<*mut crate::io::NetBuffer<'static>>,
    /// The task that issued it, once it parked.
    task: Option<u32>,
    /// The re-issued request's outcome, from the poll that ended the wait.
    completion: Option<Result<Completion, AdapterError>>,
}

/// The completion a canceled `request` returns without its requested effect, or `None` when it
/// is not a cancellation point. A canceled `tls.connect`, `tls.accept` or `tls.close` still closes its
/// stream, which the call consumes.
///
/// # Safety
/// `ctx` is live and its Binding bound.
unsafe fn canceled(
    ctx: *mut TaskCtx,
    request: &Request,
    cause: u32,
) -> Stopped<Option<Completion>> {
    let binding = unsafe { Binding::get(ctx) };
    crate::io_checks::cancel(binding.adapter.as_mut(), request, cause).map_err(Stop::refusal)
}

/// The name of a parked request for host test reporting after the run stopped.
/// This reads only the run-owned wait table; the home bridge is already closed,
/// and no adapter method or stop-aware entry may run during reporting.
/// # Safety
/// The context and its binding are live, and every worker has gone idle.
pub(crate) unsafe fn kind(ctx: *mut TaskCtx, token: u64) -> &'static str {
    // SAFETY: admission reports only after quiescence, with exclusive access.
    let binding = unsafe { Binding::raw(ctx) };
    match binding.waits.parked.get(&token).map(|p| &p.request) {
        Some(Request::SleepUntil { .. }) => "clock.sleep_until",
        Some(Request::ReadStdin { .. }) => "stdin.read",
        Some(Request::Accept { .. }) => "net.accept",
        Some(Request::NetRead { .. }) => "net.read",
        Some(Request::Resolve { .. }) => "net.resolve",
        Some(Request::Connect { .. }) => "net.connect",
        Some(Request::ConnectAddr { .. }) => "net.connect_addr",
        Some(Request::NetWrite { .. }) => "net.write",
        Some(Request::NetWriteAll { .. }) => "net.write_all",
        Some(Request::TlsConnect { .. }) => "tls.connect",
        Some(Request::TlsAccept { .. }) => "tls.accept",
        Some(Request::UdpRecv { .. }) => "udp.recv_from",
        Some(Request::UdpSend { .. }) => "udp.send_to",
        Some(Request::ProcessRun { .. }) => "proc.run",
        Some(Request::ProcessPipeline { .. }) => "proc.pipeline",
        Some(Request::ChildWrite { .. }) => "child.write",
        Some(Request::ChildRead {
            stream: crate::process::ChildStream::Stdout,
            ..
        }) => "child.read_stdout",
        Some(Request::ChildRead { .. }) => "child.read_stderr",
        Some(Request::ChildWait { .. }) => "child.wait",
        Some(Request::GroupWait { .. }) => "proc.wait_group",
        Some(Request::WatchNext { .. }) => "watcher.next",
        Some(Request::RunnerCall(_)) => "runner.call",
        _ => "provider request",
    }
}

/// Whether `request` can wait for the host and so park its task: a stdin read, a timer
/// wait, a signal wait, a process run or pipeline, a child or group write, read or wait, a
/// runner call, a watcher's next event, or a network request.
pub(crate) fn may_park(request: &Request) -> bool {
    matches!(
        request,
        Request::ReadStdin { .. }
            | Request::SleepUntil { .. }
            | Request::SignalWait { .. }
            | Request::ProcessRun { .. }
            | Request::ProcessPipeline { .. }
            | Request::ChildWrite { .. }
            | Request::ChildRead { .. }
            | Request::ChildWait { .. }
            | Request::GroupWait { .. }
            | Request::RunnerCall(_)
            | Request::WatchNext { .. }
    ) || crate::net::is_net_request(request)
}

/// Perform `request` for the running task: canceled, completed, or parked until a poll
/// completes it. True with a parked request's outcome: its task already yielded or parked
/// there, so the operation's own yield is spent. A stop is the run's while it waited.
///
/// # Safety
/// `ctx` is live, its Binding bound, and no borrow of the Binding or scheduler is held.
pub(crate) unsafe fn issue(
    ctx: *mut TaskCtx,
    request: Request,
) -> Stopped<(Result<Completion, AdapterError>, bool)> {
    // SAFETY: caller contract.
    let issued = unsafe { issue_here(ctx, request) };
    // SAFETY: as above.
    unsafe { count(ctx) };
    issued
}

unsafe fn issue_here(
    ctx: *mut TaskCtx,
    request: Request,
) -> Stopped<(Result<Completion, AdapterError>, bool)> {
    if let Some(cause) = unsafe { crate::tasks::canceled(ctx) }
        && let Some(completion) = unsafe { canceled(ctx, &request, cause) }?
    {
        return Ok((Ok(completion), false));
    }
    let binding = unsafe { Binding::get(ctx) };
    if binding.adapter.wants_issuer() {
        let task = crate::tasks::current_path(ctx);
        let binding = unsafe { Binding::get(ctx) };
        binding.adapter.issuer(&task);
    }
    let binding = unsafe { Binding::get(ctx) };
    if !may_park(&request) {
        return Ok((binding.adapter.call(request), false));
    }
    let token = binding.waits.next;
    binding.waits.next += 1;
    binding.adapter.enter(token);
    let yielded = !matches!(
        request,
        Request::ReadStdin { .. } | Request::WatchNext { .. }
    );
    let retry = request.clone();
    match binding.adapter.call(request) {
        Err(AdapterError::Blocked) => {}
        result => {
            binding.adapter.finish(token);
            return Ok((result, false));
        }
    }
    binding.waits.parked.insert(
        token,
        Parked {
            request: retry,
            borrowed: None,
            task: None,
            completion: None,
        },
    );
    // SAFETY: the context is live and nothing is borrowed across the park.
    unsafe { crate::tasks::park_host(&mut *ctx, token, yielded) }?;
    let binding = unsafe { Binding::get(ctx) };
    let completion = binding
        .waits
        .parked
        .remove(&token)
        .and_then(|parked| parked.completion)
        .unwrap_or_else(|| crate::fault("a host wait resumed without its completion"));
    Ok((completion, true))
}

/// The token the next parkable request is issued under.
///
/// # Safety
/// `ctx` is live and its Binding bound.
pub(crate) unsafe fn next_token(ctx: *mut TaskCtx) -> u64 {
    unsafe { Binding::get(ctx) }.waits.next
}

/// Send a runner call's `request` for the running task without parking it (RUN-11): its
/// completion when it completed at once (a refused call, or replay), otherwise its token,
/// under which a task later waits for it ([`await_sent`]).
///
/// # Safety
/// `ctx` is live, its Binding bound, and no borrow of the Binding or scheduler is held.
pub(crate) unsafe fn send(ctx: *mut TaskCtx, request: Request) -> Stopped<Result<Completion, u64>> {
    let binding = unsafe { Binding::get(ctx) };
    let token = binding.waits.next;
    binding.waits.next += 1;
    binding.adapter.enter(token);
    let retry = request.clone();
    match binding.adapter.call(request) {
        Err(AdapterError::Blocked) => {}
        Ok(completion) => {
            binding.adapter.finish(token);
            return Ok(Ok(completion));
        }
        Err(error) => return Err(Stop::refusal(error)),
    }
    binding.waits.parked.insert(
        token,
        Parked {
            request: retry,
            borrowed: None,
            task: None,
            completion: None,
        },
    );
    Ok(Err(token))
}

/// Park the running task on the runner call sent under `token` until its outcome arrives:
/// its completion. A sent call is no cancellation point (RUN-9).
///
/// # Safety
/// As for [`send`]; `token` was sent and no task waits on it yet.
pub(crate) unsafe fn await_sent(ctx: *mut TaskCtx, token: u64) -> Stopped<Completion> {
    // SAFETY: the context is live and nothing is borrowed across the park.
    unsafe { crate::tasks::park_host(&mut *ctx, token, false) }?;
    let binding = unsafe { Binding::get(ctx) };
    match binding
        .waits
        .parked
        .remove(&token)
        .and_then(|parked| parked.completion)
    {
        Some(Ok(completion)) => Ok(completion),
        Some(Err(error)) => Err(Stop::refusal(error)),
        None => crate::fault("a runner wait resumed without its completion"),
    }
}

/// Whether the parked request under `token` is a runner call.
///
/// # Safety
/// The context and its binding are live.
pub(crate) unsafe fn is_runner_call(ctx: *mut TaskCtx, token: u64) -> bool {
    let binding = unsafe { Binding::get(ctx) };
    matches!(
        binding.waits.parked.get(&token).map(|p| &p.request),
        Some(Request::RunnerCall(_))
    )
}

/// Execute borrowed live network storage, or decline before any effect. The buffer
/// stays exclusively borrowed on this fiber until its parked row is removed.
/// # Safety
/// Same context/binding contract as issue; buffer's storage lives through the call.
pub(crate) unsafe fn issue_net(
    ctx: *mut TaskCtx,
    net: u64,
    stream: u64,
    buffer: &mut crate::io::NetBuffer<'_>,
) -> Stopped<Option<(Result<crate::io::NetResult, AdapterError>, bool)>> {
    // Pool callbacks use the owned protocol's bridge. Never touch the home
    // Binding on a blocking-pool thread; home-thread callbacks may borrow.
    if crate::pool::on_pool() {
        return Ok(None);
    }
    // Use the ordinary cancellation path, including its preflight checks.
    if unsafe { crate::tasks::canceled(ctx) }.is_some() {
        return Ok(None);
    }
    let inline = unsafe { crate::tasks::sole_root(ctx) && idle(ctx) };
    let binding = unsafe { Binding::get(ctx) };
    let token = binding.waits.next;
    binding.waits.next += 1;
    match binding.adapter.net_io(token, net, stream, inline, buffer) {
        None => return Ok(None),
        Some(Err(AdapterError::Blocked)) => {}
        Some(result) => {
            // The root already proved no peer, host wait, deadline or free-mode
            // point needs a yield. Keep the diagnostic point without redoing it.
            #[cfg(feature = "diagnostic")]
            if inline {
                unsafe { crate::root_trace::point(&mut *ctx) };
            }
            // The spent yield still observes a stop, as `try_yield` does at its entry.
            if inline {
                crate::stop::observe(ctx)?;
            }
            return Ok(Some((result, inline)));
        }
    }
    let request = buffer.request(net, stream);
    binding.waits.parked.insert(
        token,
        Parked {
            request: request.clone(),
            // SAFETY: issue_net cannot return until the row is removed. A stop abandons
            // the run, including its wait table; no pointer escapes the run.
            borrowed: Some((buffer as *mut crate::io::NetBuffer<'_>).cast()),
            task: None,
            completion: None,
        },
    );
    unsafe { crate::tasks::park_host(&mut *ctx, token, true) }?;
    let binding = unsafe { Binding::get(ctx) };
    let completion = binding
        .waits
        .parked
        .remove(&token)
        .and_then(|p| p.completion)
        .unwrap_or_else(|| crate::fault("a borrowed host wait resumed without completion"));
    Ok(Some((completion.map(|c| net_result(&request, c)), true)))
}

/// Arm scope `scope`'s deadline: a timer wait for `at` on `clock` under a token of its own,
/// issued whatever the running task's cancellation. The token, or `None` when the deadline
/// is already reached or its clock cannot be read, which expires the scope at once.
///
/// # Safety
/// `ctx` is live, its Binding bound, and no borrow of the Binding or scheduler is held.
pub(crate) unsafe fn arm(
    ctx: *mut TaskCtx,
    scope: u32,
    clock: u64,
    at: u64,
) -> Stopped<Option<u64>> {
    // The adapter and wait table are home-affine at width > 1. The bridge
    // also refreshes the run's armed/parked count before returning.
    unsafe { at_home(ctx, || arm_here(ctx, scope, clock, at)) }?
}

unsafe fn arm_here(ctx: *mut TaskCtx, scope: u32, clock: u64, at: u64) -> Stopped<Option<u64>> {
    let binding = unsafe { Binding::get(ctx) };
    if binding.adapter.wants_issuer() {
        let task = crate::tasks::current_path(ctx);
        let binding = unsafe { Binding::get(ctx) };
        binding.adapter.issuer(&task);
    }
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
            binding.adapter.parked(token);
            binding.waits.armed.insert(token, (scope, request));
            Ok(Some(token))
        }
        Ok(Completion::Wait(_)) => {
            binding.adapter.finish(token);
            Ok(None)
        }
        Ok(_) => crate::fault("a scope deadline's timer completed as another request"),
        Err(error) => Err(Stop::refusal(error)),
    }
}

/// The scopes whose deadline is due at this scheduling point (`ready`), by deadline with ties
/// in arming order; each leaves the armed set.
///
/// # Safety
/// `ctx` is live; with no Binding bound nothing is armed.
pub(crate) unsafe fn expired(ctx: *mut TaskCtx, ready: &Ready) -> Stopped<Vec<u32>> {
    // SAFETY: caller contract.
    if unsafe { quiet(ctx) } {
        return Ok(Vec::new());
    }
    // SAFETY: caller contract.
    unsafe { at_home(ctx, || expired_here(ctx, ready)) }?
}

unsafe fn expired_here(ctx: *mut TaskCtx, ready: &Ready) -> Stopped<Vec<u32>> {
    // SAFETY: the context is live.
    if unsafe { (*ctx).io_data() }.is_null() {
        return Ok(Vec::new());
    }
    let binding = unsafe { Binding::get(ctx) };
    if binding.waits.armed.is_empty() {
        return Ok(Vec::new());
    }
    expired_binding(binding, ready)
}

fn expired_binding(binding: &mut Binding, ready: &Ready) -> Stopped<Vec<u32>> {
    let mut armed: Vec<(u64, u64)> = binding
        .waits
        .armed
        .iter_unordered()
        .filter(|(token, _)| ready.contains(*token))
        .map(|(token, (_, request))| match request {
            Request::SleepUntil { deadline, .. } => (*deadline, token),
            _ => crate::fault("an armed deadline that is not a timer"),
        })
        .collect();
    // By deadline, ties in arming (token) order.
    armed.sort_unstable();
    let mut due = Vec::new();
    for (_, token) in armed {
        let (scope, request) = binding.waits.armed[&token].clone();
        binding.adapter.enter(token);
        match binding.adapter.call(request) {
            Err(AdapterError::Blocked) => continue,
            Ok(Completion::Wait(_)) => {}
            Ok(_) => crate::fault("a scope deadline's timer completed as another request"),
            Err(error) => return Err(Stop::refusal(error)),
        }
        binding.adapter.finish(token);
        binding.waits.armed.remove(&token);
        due.push(scope);
    }
    Ok(due)
}

/// A context-free timer readiness step on the admission thread. The logical
/// policy reserves its poll before calling this and holds no scheduler role.
/// `idle` permits waiting only after every physical segment is saved.
/// # Safety
/// Home only; no other adapter/wait-table call may overlap this reservation.
pub(crate) unsafe fn timers_home(
    run: *mut crate::context::RunShared,
    idle: bool,
) -> Stopped<(Vec<u32>, bool)> {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    assert!(
        binding.waits.parked.is_empty(),
        "parked host requests require their completion adapter"
    );
    loop {
        let ready = binding.adapter.ready().map_err(Stop::refusal)?;
        let expired = expired_binding(binding, &ready)?;
        unsafe {
            (*run).host_waits.store(
                binding.waits.armed.len() as u32,
                std::sync::atomic::Ordering::Release,
            )
        };
        if !idle || !expired.is_empty() {
            return Ok((expired, true));
        }
        if !binding.adapter.wait().map_err(Stop::refusal)? {
            return Ok((Vec::new(), false));
        }
        if let Some(stop) = unsafe { (*run).control.stopped() } {
            return Err(stop);
        }
    }
}

/// A scope exited before its deadline: remove its timer, recording nothing.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` armed.
pub(crate) unsafe fn disarm(ctx: *mut TaskCtx, token: u64) {
    // SAFETY: caller contract.
    unsafe { crate::stop::check(ctx, at_home(ctx, || disarm_here(ctx, token))) }
}

unsafe fn disarm_here(ctx: *mut TaskCtx, token: u64) {
    let binding = unsafe { Binding::get(ctx) };
    binding.adapter.finish(token);
    binding.waits.armed.remove(&token);
}

/// Original absolute timer ticks for scheduler ordering, alike in live runs and replay.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` parked.
pub(crate) unsafe fn deadline(ctx: *mut TaskCtx, token: u64) -> Option<u64> {
    // SAFETY: caller contract.
    unsafe { crate::stop::check(ctx, at_home(ctx, || deadline_here(ctx, token))) }
}

unsafe fn deadline_here(ctx: *mut TaskCtx, token: u64) -> Option<u64> {
    let binding = unsafe { Binding::get(ctx) };
    match &binding.waits.parked.get(&token)?.request {
        Request::SleepUntil { deadline, .. } => Some(*deadline),
        _ => None,
    }
}

/// Task `task` parked on the request blocked under `token`.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` blocked.
pub(crate) unsafe fn park(ctx: *mut TaskCtx, token: u64, task: u32) {
    // SAFETY: caller contract.
    unsafe { crate::stop::check(ctx, at_home(ctx, || park_here(ctx, token, task))) }
}

unsafe fn park_here(ctx: *mut TaskCtx, token: u64, task: u32) {
    let binding = unsafe { Binding::get(ctx) };
    match binding.waits.parked.get_mut(&token) {
        Some(parked) => parked.task = Some(task),
        None => crate::fault("a park of an unknown host wait"),
    }
    binding.adapter.parked(token);
}

/// Whether a scheduling point can observe an external wait or scope deadline.
/// # Safety
/// The context is live; its binding, if any, has exclusive access.
pub(crate) unsafe fn idle(ctx: *mut TaskCtx) -> bool {
    if unsafe { (*ctx).io_data() }.is_null() {
        return true;
    }
    let waits = &unsafe { Binding::get(ctx) }.waits;
    waits.parked.is_empty() && waits.armed.is_empty()
}

/// A scheduling point: the parked tokens to re-issue, asked of the adapter once while a host
/// request is parked or a scope deadline armed.
///
/// # Safety
/// `ctx` is live and no borrow of the Binding is held.
pub(crate) unsafe fn ready(ctx: *mut TaskCtx) -> Stopped<Ready> {
    // SAFETY: caller contract.
    if unsafe { quiet(ctx) } {
        return Ok(Ready::Tokens(Vec::new()));
    }
    // SAFETY: caller contract.
    unsafe { at_home(ctx, || ready_here(ctx)) }?
}

unsafe fn ready_here(ctx: *mut TaskCtx) -> Stopped<Ready> {
    // SAFETY: the context is live.
    if unsafe { (*ctx).io_data() }.is_null() {
        return Ok(Ready::Tokens(Vec::new()));
    }
    let binding = unsafe { Binding::get(ctx) };
    if binding.waits.parked.is_empty() && binding.waits.armed.is_empty() {
        return Ok(Ready::Tokens(Vec::new()));
    }
    let ready = binding.adapter.ready().map_err(Stop::refusal)?;
    // Injected waits are derived, never recorded: each point re-issues them.
    let Ready::Tokens(mut tokens) = ready else {
        return Ok(ready);
    };
    tokens.extend(
        binding
            .waits
            .parked
            .iter_unordered()
            .filter(|(_, parked)| binding.adapter.injected_wait(&parked.request))
            .map(|(token, _)| token),
    );
    tokens.sort_unstable();
    tokens.dedup();
    Ok(Ready::Tokens(tokens))
}

/// The parked tasks whose request `ready` names. Their order is immaterial: the scheduler
/// polls the woken tasks by deadline, then park order.
///
/// # Safety
/// `ctx` is live; with no Binding bound nothing is parked.
pub(crate) unsafe fn woken(ctx: *mut TaskCtx, ready: &Ready) -> Vec<u32> {
    // SAFETY: caller contract.
    if unsafe { quiet(ctx) } {
        return Vec::new();
    }
    // SAFETY: caller contract.
    unsafe { crate::stop::check(ctx, at_home(ctx, || woken_here(ctx, ready))) }
}

unsafe fn woken_here(ctx: *mut TaskCtx, ready: &Ready) -> Vec<u32> {
    // SAFETY: the context is live.
    if unsafe { (*ctx).io_data() }.is_null() {
        return Vec::new();
    }
    let waits = &unsafe { Binding::get(ctx) }.waits;
    match ready {
        Ready::Every => waits
            .parked
            .values_unordered()
            .filter_map(|p| p.task)
            .collect(),
        Ready::Tokens(tokens) => tokens
            .iter()
            .filter_map(|token| waits.parked.get(token)?.task)
            .collect(),
    }
}

/// The scheduler's poll of a task parked on `token`, `cause` its cancellation and `ready`
/// whether this scheduling point found the token ready: whether the wait is over, after
/// re-issuing its request when it is.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` parked.
pub(crate) unsafe fn poll(
    ctx: *mut TaskCtx,
    token: u64,
    cause: Option<u32>,
    ready: bool,
) -> Stopped<bool> {
    // SAFETY: caller contract.
    unsafe { at_home(ctx, || poll_here(ctx, token, cause, ready)) }?
}

unsafe fn poll_here(
    ctx: *mut TaskCtx,
    token: u64,
    cause: Option<u32>,
    ready: bool,
) -> Stopped<bool> {
    let binding = unsafe { Binding::get(ctx) };
    let request = match binding.waits.parked.get(&token) {
        Some(parked) => parked.request.clone(),
        None => crate::fault("a poll of an unknown host wait"),
    };
    // A request that already committed part of its effect completes with its own outcome;
    // a sent runner call waits for its callee's, a cancellation being forwarded (RUN-9).
    let cause = cause.filter(|_| {
        binding.adapter.cancellable(token) && !matches!(request, Request::RunnerCall(_))
    });
    if cause.is_none() && !ready {
        return Ok(false);
    }
    let completion = match cause {
        Some(cause) => {
            binding.adapter.finish(token);
            // SAFETY: no borrow of the Binding is held across this call.
            match unsafe { canceled(ctx, &request, cause) }? {
                Some(completion) => Ok(completion),
                None => crate::fault("a parked request that is no cancellation point"),
            }
        }
        None => {
            let borrowed = binding.waits.parked.get(&token).and_then(|p| p.borrowed);
            let result = match borrowed {
                // SAFETY: issue_net holds this stack buffer exclusively until removal.
                Some(buffer) => binding
                    .adapter
                    .net_io(
                        token,
                        match &request {
                            Request::NetRead { net, .. }
                            | Request::NetWrite { net, .. }
                            | Request::NetWriteAll { net, .. } => *net,
                            _ => unreachable!(),
                        },
                        match &request {
                            Request::NetRead { stream, .. }
                            | Request::NetWrite { stream, .. }
                            | Request::NetWriteAll { stream, .. } => *stream,
                            _ => unreachable!(),
                        },
                        false,
                        unsafe { &mut *buffer },
                    )
                    .unwrap_or_else(|| crate::fault("borrowed network adapter changed"))
                    .map(|value| net_completion(&request, value)),
                None => {
                    binding.adapter.enter(token);
                    binding.adapter.call(request)
                }
            };
            match result {
                Err(AdapterError::Blocked) => return Ok(false),
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
    Ok(true)
}

/// With no runnable task: deliver a script's next signal, or sleep until some parked
/// request's descriptor is ready or the earliest deadline passes (in replay, until the
/// recording's next `ready`). False when nothing can wake a task.
///
/// # Safety
/// `ctx` is live and its Binding bound.
pub(crate) unsafe fn wait(ctx: *mut TaskCtx) -> Stopped<bool> {
    // The last task can free its request/response buffers after its last worker collection.
    // With no runnable task, nothing else will enforce the idle cap until new work arrives.
    // SAFETY: the scheduler is idle on this worker; no heap borrow crosses the host wait.
    let heap = unsafe { &mut *ctx }.heap();
    // Most waits are unrelated to a completed request. Avoid collecting/scanning the
    // live board unless already-empty slabs actually exceed the cap.
    if heap.view().idle_slabs > heap.idle_cap / crate::alloc::SLAB_SIZE as u64 {
        heap.trim();
    }
    #[cfg(feature = "measurement")]
    crate::measurement::idle_snapshot(unsafe { &mut *ctx });
    // The code's caller owns this permit. Home must never reacquire it on
    // behalf of a waiting compute worker: at one process permit that would
    // prevent the caller from resuming after the home completion.
    crate::pool::release_permit(ctx);
    let waited = unsafe { at_home(ctx, || wait_here(ctx)) }??;
    crate::pool::acquire_permit(ctx);
    Ok(waited)
}

unsafe fn wait_here(ctx: *mut TaskCtx) -> Stopped<bool> {
    let binding = unsafe { Binding::get(ctx) };
    // A script's undelivered signals are an event source even when nothing is parked.
    if binding.waits.parked.is_empty()
        && binding.waits.armed.is_empty()
        && !binding.adapter.scripted_input()
    {
        return Ok(false);
    }
    binding.adapter.wait().map_err(Stop::refusal)
}

// Only parked calls need the generic protocol's result storage.
fn net_completion(request: &Request, value: Result<u64, crate::net::NetError>) -> Completion {
    match request {
        Request::NetWriteAll { .. } => Completion::NetUnit(value.map(|_| ())),
        _ => Completion::Count(value),
    }
}
fn net_result(request: &Request, value: Completion) -> Result<u64, crate::net::NetError> {
    match value {
        Completion::Count(value) => value,
        Completion::NetBytes(Err(error)) => Err(error),
        Completion::NetUnit(value) if matches!(request, Request::NetWriteAll { .. }) => {
            value.map(|()| 0)
        }
        _ => crate::fault("borrowed network result changed"),
    }
}
