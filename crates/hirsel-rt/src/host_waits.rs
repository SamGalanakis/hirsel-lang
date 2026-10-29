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
use crate::tasks::ordered::host::concurrent::{self as ordered_concurrent, CONCURRENT};

/// Home publishes a lower bound on readiness, only for an exclusive real timer
/// driver. Ordered's poll owner counts the empty points it can answer locally;
/// home applies them before its next adapter operation. No adapter or saved
/// task frame is reachable through this snapshot.
pub(crate) struct TimerHint {
    origin: std::time::Instant,
    next: std::sync::atomic::AtomicU64,
    points: std::sync::atomic::AtomicU64,
    scope_only: std::sync::atomic::AtomicBool,
}

impl Default for TimerHint {
    fn default() -> Self {
        Self {
            origin: std::time::Instant::now(),
            next: Default::default(),
            points: Default::default(),
            scope_only: Default::default(),
        }
    }
}

impl TimerHint {
    /// Called only by Ordered's exclusive poll owner, with no concurrent host
    /// request. Home cannot mutate the adapter until this owner relinquishes
    /// the role or synchronously calls it.
    fn empty_point(&self) -> bool {
        use std::sync::atomic::Ordering;
        let next = self.next.load(Ordering::Acquire);
        if next == 0 || self.origin.elapsed().as_nanos() >= u128::from(next) {
            return false;
        }
        self.points.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub(crate) fn publish(&self, next: Option<std::time::Instant>) {
        // A derived watcher may have no driver Pending row at all. Only the
        // run's scope-only wait-table mirror can authorize this publication.
        let next = next.filter(|_| self.scope_only.load(std::sync::atomic::Ordering::Acquire));
        // Round down, never extending the driver's deadline. Zero declines.
        let next = next
            .and_then(|next| next.checked_duration_since(self.origin))
            .and_then(|left| u64::try_from(left.as_nanos()).ok())
            .unwrap_or(0);
        self.next.store(next, std::sync::atomic::Ordering::Release);
    }

    fn scope_only(&self, only: bool) {
        self.scope_only
            .store(only, std::sync::atomic::Ordering::Release);
        if !only {
            self.publish(None);
        }
    }

    pub(crate) fn take_points(&self) -> u64 {
        self.points.swap(0, std::sync::atomic::Ordering::AcqRel)
    }
}

/// Run `f` on the run's home thread, which holds the Io binding: the scheduling points of a
/// width > 1 run run on compute workers (`crate::pool`).
///
/// # Safety
/// `ctx` is live; no Binding or scheduler loan crosses the call. `f` uses
/// only home-owned state and copied inputs, never the issuer's scheduler.
unsafe fn at_home<T>(ctx: *mut TaskCtx, f: impl FnOnce() -> T) -> Stopped<T> {
    // Every wait-table bridge can return a compute permit before waiting.
    // Release the role until that permit is reacquired; a worker holding the
    // last permit may itself be waiting to enter the scheduler.
    let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
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
        let (live, scope_only) = if (*ctx).io_data().is_null() {
            (0, false)
        } else {
            let binding = Binding::raw(ctx);
            let waits = &binding.waits;
            (
                waits.parked.len()
                    + waits.native.len()
                    + waits.armed.len()
                    + usize::from(binding.adapter.process_sources()),
                waits.scope_only(),
            )
        };
        (*ctx).run_ref().timer_hint.scope_only(scope_only);
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
    parked_ready: crate::id_map::IdSet,
    parked_injected: crate::id_map::IdSet,
    /// Free requests retry on their owning fiber, inside recorded phases.
    /// Home observes readiness only; it never calls these requests at a poll.
    native: IdMap<Native>,
    native_ready: crate::id_map::IdSet,
    native_injected: crate::id_map::IdSet,
    /// Armed scope deadlines (CONC-3) by token: the scope and its timer request.
    armed: IdMap<(u32, u32, Request)>,
    armed_due: std::collections::BTreeSet<(u64, u64)>,
}
impl Waits {
    fn remove_parked(&mut self, token: u64) -> Option<Parked> {
        self.parked_ready.remove(&token);
        self.parked_injected.remove(&token);
        self.parked.remove(&token)
    }
    fn scope_only(&self) -> bool {
        self.parked.is_empty() && self.native.is_empty() && !self.armed.is_empty()
    }

    fn every_tokens(&self) -> Vec<u64> {
        self.parked
            .iter_unordered()
            .map(|(token, _)| token)
            .chain(self.armed.iter_unordered().map(|(token, _)| token))
            .chain(self.native.iter_unordered().map(|(token, _)| token))
            .collect()
    }
}

struct Parked {
    request: Request,
    /// The task that issued it, once it parked.
    task: Option<u32>,
    generation: u32,
    /// The re-issued request's outcome, from the poll that ended the wait.
    completion: Option<Result<Completion, AdapterError>>,
    ready: bool,
}

struct Native {
    request: Request,
    task: Option<u32>,
    generation: u32,
    notified: bool,
    ready: bool,
}

/// Reserve a normal adapter token for an explicitly admitted Free request.
/// # Safety
/// Home only; the task owns the request through the synchronous bridge.
pub(crate) unsafe fn native_begin_home(
    run: *mut crate::context::RunShared,
    request: &Request,
) -> u64 {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    let token = binding.waits.next;
    binding.waits.next += 1;
    if binding.adapter.injected_wait(request) {
        binding.waits.native_injected.insert(token);
    }
    binding.waits.native.insert(
        token,
        Native {
            request: request.clone(),
            task: None,
            generation: 0,
            notified: false,
            ready: false,
        },
    );
    unsafe { native_count_home(run, binding) };
    token
}

unsafe fn native_count_home(run: *mut crate::context::RunShared, binding: &Binding) {
    unsafe {
        (*run).timer_hint.scope_only(binding.waits.scope_only());
        (*run).host_waits.store(
            (binding.waits.parked.len()
                + binding.waits.native.len()
                + binding.waits.armed.len()
                + usize::from(binding.adapter.process_sources())) as u32,
            std::sync::atomic::Ordering::Release,
        )
    };
}

/// One bounded attempt, called by its owner after claim/key admission.
/// # Safety
/// Home only; token belongs to the owner's live native request.
pub(crate) unsafe fn native_call_home(
    run: *mut crate::context::RunShared,
    token: u64,
    cause: Option<u32>,
) -> Result<Completion, AdapterError> {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    let row = binding
        .waits
        .native
        .get_mut(&token)
        .expect("a native attempt");
    row.task = None;
    row.ready = false;
    binding.waits.native_ready.remove(&token);
    let request = row.request.clone();
    binding.adapter.enter(token);
    if let Some(cause) = cause
        && binding.adapter.cancellable(token)
        && let Some(completion) =
            crate::io_checks::cancel(binding.adapter.as_mut(), &request, cause)?
    {
        return Ok(completion);
    }
    binding.adapter.call(request)
}

/// Retire readiness storage after the final bounded attempt ended.
/// # Safety
/// Home only, under the owner's synchronous bridge.
pub(crate) unsafe fn native_finish_home(run: *mut crate::context::RunShared, token: u64) {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    binding.adapter.finish(token);
    assert!(binding.waits.native.remove(&token).is_some());
    binding.waits.native_ready.remove(&token);
    binding.waits.native_injected.remove(&token);
    unsafe { native_count_home(run, binding) };
}

/// Home copied readiness into saved runtime metadata. Suppress repeated
/// notifications until another Blocked attempt parks this token again.
/// # Safety
/// Home only; no adapter borrow or scheduler role is retained.
pub(crate) unsafe fn native_notified_home(run: *mut crate::context::RunShared, tokens: &[u64]) {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    for token in tokens {
        binding
            .waits
            .native
            .get_mut(token)
            .expect("a native readiness owner")
            .notified = true;
        binding.waits.native_ready.remove(token);
    }
}

/// Home only; no Binding loan survives this source-accounting query.
pub(crate) unsafe fn scope_pending_home(run: *mut crate::context::RunShared) -> bool {
    let binding = unsafe { &*(*run).io_binding().cast::<Binding>() };
    !binding.waits.armed.is_empty()
}

/// Free polls use the existing driver/token readiness surface, but only
/// return native token notifications. No request is reissued on this stack.
/// # Safety
/// Home only; no scheduler role or parked context is borrowed.
pub(crate) unsafe fn native_ready_home(
    run: *mut crate::context::RunShared,
    canceled: &[u64],
) -> Stopped<HomeReady> {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    native_ready_binding(binding, canceled)
}

fn native_ready_binding(binding: &mut Binding, canceled: &[u64]) -> Stopped<HomeReady> {
    assert!(
        binding.waits.parked.is_empty(),
        "Free native requests use owner-side attempts"
    );
    let ready = binding.adapter.ready().map_err(Stop::refusal)?;
    let expired = expired_binding(binding, &ready)?;
    // Adapter readiness may consume an edge before an owner is fully saved
    // (or even before its parked notification reaches home). Retain it until
    // that owner's next bounded attempt, independently of task registration.
    let mut available = crate::id_map::IdSet::default();
    match ready {
        Ready::Tokens(tokens) => {
            for token in tokens {
                if binding.waits.native.contains_key(&token) {
                    available.insert(token);
                }
            }
        }
        Ready::Every => {
            for (token, _) in binding.waits.native.iter_unordered() {
                available.insert(token);
            }
        }
    }
    for token in binding.waits.native_injected.members_unordered() {
        available.insert(token);
    }
    for token in available.members_unordered() {
        let row = binding.waits.native.get_mut(&token).unwrap();
        if !row.notified {
            row.ready = true;
            binding.waits.native_ready.insert(token);
        }
    }
    native_cancel_ready(binding, canceled);
    Ok(HomeReady {
        expired,
        hosts: native_ready_hosts(binding),
    })
}

fn native_cancel_ready(binding: &mut Binding, canceled: &[u64]) {
    for &token in canceled {
        if let Some(row) = binding.waits.native.get_mut(&token)
            && !row.notified
            && binding.adapter.cancellable(token)
            && crate::io_checks::cancellation_result(&row.request, 0).is_some()
        {
            row.ready = true;
            binding.waits.native_ready.insert(token);
        }
    }
}

fn native_ready_hosts(binding: &Binding) -> Vec<ReadyHost> {
    binding
        .waits
        .native_ready
        .members_unordered()
        .filter_map(|token| {
            let row = binding.waits.native.get(&token)?;
            let task = row.task?;
            if row.notified {
                return None;
            }
            let deadline = if let Request::SleepUntil { deadline, .. } = row.request {
                Some(deadline)
            } else {
                None
            };
            Some(ReadyHost {
                generation: row.generation,
                task,
                token,
                deadline,
                ready: row.ready,
            })
        })
        .collect()
}

/// Complete provider phases outside the scheduler role, then collect all
/// latched tokens, including candidates first published by this batch.
pub(crate) unsafe fn native_batch_home(
    run: *mut crate::context::RunShared,
    canceled: &[u64],
) -> Stopped<Vec<ReadyHost>> {
    unsafe { process_batch_home(run, canceled) }?;
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    // Cancellation can arrive after the initial adapter readiness poll.
    // Latch it before consuming its subscription notice, even when this
    // provider has no physical source and process_batch returns no tokens.
    native_cancel_ready(binding, canceled);
    Ok(native_ready_hosts(binding))
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
    binding
        .waits
        .parked
        .get(&token)
        .map(|p| &p.request)
        .or_else(|| binding.waits.native.get(&token).map(|p| &p.request))
        .map_or("provider request", request_kind)
}

/// Pure diagnostic classification, shared with owned canonical wait snapshots.
pub(crate) fn request_kind(request: &Request) -> &'static str {
    match request {
        Request::SleepUntil { .. } => "clock.sleep_until",
        Request::ReadStdin { .. } => "stdin.read",
        Request::Accept { .. } => "net.accept",
        Request::NetRead { .. } => "net.read",
        Request::Resolve { .. } => "net.resolve",
        Request::Connect { .. } => "net.connect",
        Request::ConnectAddr { .. } => "net.connect_addr",
        Request::NetWrite { .. } => "net.write",
        Request::NetWriteAll { .. } => "net.write_all",
        Request::TlsConnect { .. } => "tls.connect",
        Request::TlsAccept { .. } => "tls.accept",
        Request::UdpRecv { .. } => "udp.recv_from",
        Request::UdpSend { .. } => "udp.send_to",
        Request::ProcessRun { .. } => "proc.run",
        Request::ProcessPipeline { .. } => "proc.pipeline",
        Request::ChildWrite { .. } => "child.write",
        Request::ChildRead {
            stream: crate::process::ChildStream::Stdout,
            ..
        } => "child.read_stdout",
        Request::ChildRead { .. } => "child.read_stderr",
        Request::ChildWait { .. } => "child.wait",
        Request::GroupWait { .. } => "proc.wait_group",
        Request::WatchNext { .. } => "watcher.next",
        Request::RunnerCall(_) => "runner.call",
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
            | Request::FileRead { .. }
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
    if matches!(request, Request::Tasks { .. } | Request::TestTasks { .. })
        && unsafe {
            (*ctx).run_ref().cpu_grant.is_some() && (*ctx).run_ref().deferred_ordered.is_some()
        }
    {
        unsafe { crate::tasks::ordered::cpu::serial::enable_meter(ctx) };
    }
    if (matches!(request, Request::Tasks { .. } | Request::TestTasks { .. })
        && unsafe { (*ctx).run_ref().cpu_grant.is_none() })
        || (may_park(&request) && unsafe { crate::pool::wide((*ctx).run_ref()) })
    {
        // A deferred run must use its admitted wait/retirement protocol at
        // the first parkable host effect, even before it projects Tasks.
        unsafe { crate::tasks::ordered::admission::Admission::activate(ctx) };
    }
    if crate::tasks::free::host::native::enabled(ctx) {
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        let provider = crate::runtime_roots::free()
            .unwrap_or_else(|| crate::fault("native Free Io without its provider"));
        return unsafe { (provider.issue)(ctx, request) };
    }
    let ordered = crate::tasks::ordered::engine::on(ctx);
    if ordered && let Some(io) = unsafe { (*ctx).run_ref().ordered_io.get() } {
        // Live Ordered: every request enters the run's concurrent service, so a
        // blocking built-in call runs as an owned job, never on a compute worker
        // or home. The service spends the operation's yield or parks.
        let adapter = io.adapter.clone();
        return unsafe { ordered_concurrent::request(ctx, &*adapter, request) }
            .map(|result| (result, true));
    }
    if ordered {
        return unsafe { crate::tasks::ordered::host::home::request(ctx, request) };
    }
    let mut request = ordered_concurrent::HomeRequest::local(request);
    let issued = unsafe { issue_here(ctx, &mut request) };
    unsafe { count(ctx) };
    issued
}

pub(crate) unsafe fn observe_keys(ctx: *mut TaskCtx, request: &Request) {
    let (keys, domains) = {
        let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
        let adapter = &*unsafe { Binding::get(ctx) }.adapter;
        let keys = adapter.keys(request);
        let domains = if keys.is_some() {
            adapter.domains(request)
        } else {
            Vec::new()
        };
        (keys, domains)
    };
    crate::tasks::ordered::events::request_domains(ctx, domains);
    crate::tasks::ordered::events::host_phases::request_keys(ctx, keys);
}

pub(crate) type Discovery =
    std::collections::BTreeMap<u64, (crate::io_keys::Key, crate::io_keys::Kind)>;

pub(crate) fn snapshot(
    adapter: &dyn crate::io::Adapter,
    result: &Result<Completion, AdapterError>,
) -> Result<Discovery, &'static str> {
    if matches!(result, Err(AdapterError::InvalidHandle(_))) {
        return Ok(Default::default());
    }
    let mut image = result.clone();
    crate::io_observation::capture(&mut image).map_err(|_| "invalid home mapping")?;
    crate::io_observation::fields(&mut image)?
        .into_iter()
        .flatten()
        .map(|word| {
            adapter
                .identity(*word)
                .map(|identity| (*word, identity))
                .ok_or("created home handle has no discovered resource")
        })
        .collect()
}

pub(crate) unsafe fn observe_result(ctx: *mut TaskCtx, result: &Result<Completion, AdapterError>) {
    if !crate::tasks::ordered::engine::on(ctx)
        || !crate::tasks::ordered::events::observing(ctx)
        || !crate::tasks::ordered::events::home_declared(ctx)
    {
        return;
    }
    let discovered = {
        let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
        snapshot(&*unsafe { Binding::get(ctx) }.adapter, result)
    };
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    crate::tasks::ordered::events::home_discovery(s, unsafe { (*ctx).task_id }, result, discovered);
}

pub(crate) enum HomeOutcome {
    Complete(Result<Completion, AdapterError>),
    Wait { token: u64, yielded: bool },
}

/// Bounded home attempt. Its borrowed view ends before an owned drive can leave.
pub(crate) unsafe fn home_initial(ctx: *mut TaskCtx) -> Stopped<HomeOutcome> {
    let mut request = unsafe { ordered_concurrent::HomeRequest::retained(ctx) };
    let result = unsafe { attempt_here(ctx, &mut request) };
    unsafe { count(ctx) };
    result
}

/// Take the published completion once, without introducing a stop after selection.
pub(crate) unsafe fn home_completion(
    ctx: *mut TaskCtx,
    token: u64,
) -> Result<Completion, AdapterError> {
    let binding = unsafe { Binding::raw(ctx) };
    let completion = if crate::tasks::ordered::host::return_pending(ctx, token) {
        binding
            .waits
            .parked
            .get(&token)
            .and_then(|row| row.completion.clone())
    } else {
        binding
            .waits
            .remove_parked(token)
            .and_then(|row| row.completion)
    }
    .unwrap_or_else(|| crate::fault("a host wait resumed without its completion"));
    unsafe { count(ctx) };
    completion
}

unsafe fn issue_here(
    ctx: *mut TaskCtx,
    request: &mut ordered_concurrent::HomeRequest,
) -> Stopped<(Result<Completion, AdapterError>, bool)> {
    match unsafe { attempt_here(ctx, request) }? {
        HomeOutcome::Complete(result) => Ok((result, false)),
        HomeOutcome::Wait { token, yielded } => {
            unsafe { crate::tasks::park_host(&mut *ctx, token, yielded) }?;
            Ok((unsafe { home_completion(ctx, token) }, true))
        }
    }
}

unsafe fn attempt_here(
    ctx: *mut TaskCtx,
    request: &mut ordered_concurrent::HomeRequest,
) -> Stopped<HomeOutcome> {
    if let Some(cause) = unsafe { crate::tasks::canceled(ctx) }
        && let Some(completion) = unsafe { canceled(ctx, request.get(), cause) }?
    {
        request.save_result(Ok(completion));
        unsafe { observe_result(ctx, request.result()) };
        crate::tasks::ordered::events::record_host_initial(ctx, request.result());
        if crate::tasks::ordered::engine::on(ctx) && crate::tasks::ordered::events::observing(ctx) {
            match request.get() {
                Request::Print { bytes, .. } | Request::PrintStderr { bytes, .. } => {
                    crate::tasks::ordered::events::print_output(
                        ctx,
                        bytes,
                        request.result(),
                        Some(0),
                    )
                }
                _ => {}
            }
        }
        return Ok(HomeOutcome::Complete(request.take_result()));
    }
    let binding = unsafe { Binding::get(ctx) };
    let batch = may_park(request.get()) && binding.adapter.batches_issue();
    if !batch && binding.adapter.wants_issuer() {
        let task = crate::tasks::current_path(ctx);
        let binding = unsafe { Binding::get(ctx) };
        binding.adapter.issuer(&task);
    }
    let binding = unsafe { Binding::get(ctx) };
    if !may_park(request.get()) {
        let observing =
            crate::tasks::ordered::engine::on(ctx) && crate::tasks::ordered::events::observing(ctx);
        if observing
            && let Request::Print {
                stdout: handle,
                bytes,
            }
            | Request::PrintStderr {
                stderr: handle,
                bytes,
            } = request.get()
        {
            let stderr = matches!(request.get(), Request::PrintStderr { .. });
            let (result, count) = {
                let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
                binding.adapter.print_observed(*handle, stderr, bytes)
            };
            request.save_result(result);
            unsafe { observe_result(ctx, request.result()) };
            crate::tasks::ordered::events::record_host_initial(ctx, request.result());
            let bytes = match request.get() {
                Request::Print { bytes, .. } | Request::PrintStderr { bytes, .. } => bytes,
                _ => unreachable!(),
            };
            crate::tasks::ordered::events::print_output(ctx, bytes, request.result(), count);
            return Ok(HomeOutcome::Complete(request.take_result()));
        }
        let result = {
            let _segment = observing.then(|| unsafe { crate::tasks::role::Segment::new(ctx) });
            binding.adapter.call(request.take(false))
        };
        request.save_result(result);
        unsafe { observe_result(ctx, request.result()) };
        crate::tasks::ordered::events::record_host_initial(ctx, request.result());
        return Ok(HomeOutcome::Complete(request.take_result()));
    }
    let token = binding.waits.next;
    binding.waits.next += 1;
    if !batch {
        binding.adapter.enter(token);
    }
    let yielded = !matches!(
        request.get(),
        Request::ReadStdin { .. } | Request::WatchNext { .. }
    );
    let observing =
        crate::tasks::ordered::engine::on(ctx) && crate::tasks::ordered::events::observing(ctx);
    // The current request owns the role until the optional Segment below.
    let issuer = batch.then(|| unsafe { crate::tasks::issuer_path(ctx) });
    let result = {
        let _segment = observing.then(|| unsafe { crate::tasks::role::Segment::new(ctx) });
        if let Some(issuer) = &issuer {
            binding
                .adapter
                .issue_batch(request.take(true), token, issuer)
        } else {
            binding.adapter.call(request.take(true))
        }
    };
    request.save_result(result);
    unsafe { observe_result(ctx, request.result()) };
    crate::tasks::ordered::events::record_host_initial(ctx, request.result());
    // The snapshot owns the first attempt before this frame may be saved.
    if request.result() != &Err(AdapterError::Blocked) {
        binding.adapter.finish(token);
        return Ok(HomeOutcome::Complete(request.take_result()));
    }
    drop(request.take_result());
    if binding.adapter.injected_wait(request.get()) {
        binding.waits.parked_injected.insert(token);
    }
    binding.waits.parked.insert(
        token,
        Parked {
            request: request.take(false),
            task: None,
            generation: 0,
            completion: None,
            ready: false,
        },
    );
    // Invalidate timer-only publication before park can retire a boundary.
    unsafe { count(ctx) };
    Ok(HomeOutcome::Wait { token, yielded })
}

/// Inspect only an already-completed row, after the original poll and wake.
/// This neither retries a request nor changes token/row/evidence ownership.
/// # Safety
/// The issuer has resumed its saved frame and exclusively owns this wait.
pub(crate) unsafe fn completed_read(
    ctx: *mut TaskCtx,
    token: u64,
) -> Option<crate::tasks::ordered::host::CompletedRead> {
    if token & CONCURRENT != 0 {
        return unsafe { (*ctx).run_ref().ordered_io.get().unwrap() }.completed_read(token);
    }
    // The facade's owned wait rows are role-protected; no adapter callback or
    // home loan is needed to inspect a completed saved owner's immutable bytes.
    let binding = unsafe { Binding::raw(ctx) };
    let row = binding.waits.parked.get(&token).expect("saved host wait");
    crate::tasks::ordered::host::CompletedRead::new(
        token,
        &row.request,
        row.completion.as_ref().expect("completed wait"),
    )
}

/// Remove the original wait only at its canonical return, with the role held
/// and the poll finished. No provider callback or new stop observation occurs.
/// # Safety
/// The caller is the generation-qualified certificate's canonical head owner.
pub(crate) unsafe fn retire_completed_read(ctx: *mut TaskCtx, token: u64, task: u64) {
    assert_eq!(task as u32, unsafe { (*ctx).task_id });
    if token & CONCURRENT != 0 {
        unsafe { (*ctx).run_ref().ordered_io.get().unwrap() }.retire_read(
            token,
            task as u32,
            (task >> 32) as u32,
        );
    } else {
        let binding = unsafe { Binding::raw(ctx) };
        let row = binding
            .waits
            .remove_parked(token)
            .expect("certified read row");
        assert_eq!(row.task, Some(unsafe { (*ctx).task_id }));
        assert!(row.completion.is_some());
        unsafe { count(ctx) };
    }
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
    if binding.adapter.injected_wait(&retry) {
        binding.waits.parked_injected.insert(token);
    }
    binding.waits.parked.insert(
        token,
        Parked {
            request: retry,
            task: None,
            generation: 0,
            completion: None,
            ready: false,
        },
    );
    // Invalidate timer-only publication before park can retire a boundary.
    unsafe { count(ctx) };
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
        .remove_parked(token)
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

/// Execute borrowed live network storage only when it completes without a wait,
/// or decline before any effect; a wait takes the owned path, so no parked row
/// retains the borrowed buffer.
/// # Safety
/// Same context/binding contract as issue; buffer's storage lives through the call.
pub(crate) unsafe fn issue_net(
    ctx: *mut TaskCtx,
    net: u64,
    stream: u64,
    buffer: &mut crate::io::NetBuffer<'_>,
) -> Stopped<Option<(Result<crate::io::NetResult, AdapterError>, bool)>> {
    // Native Free records owned payloads and retries through its claim/epoch
    // service. Decline the borrowed shortcut before touching the home Binding.
    if crate::tasks::free::host::native::enabled(ctx) {
        return Ok(None);
    }
    // Canonical capture needs owned request/result discovery. Decline before
    // touching a borrowed provider buffer or performing its fast-path effect.
    if crate::tasks::ordered::engine::on(ctx) && crate::tasks::ordered::events::observing(ctx) {
        return Ok(None);
    }

    // Ordered host waits retain owned request/completion buffers. Borrowed
    // socket storage remains on the owned fallback until its boundary audit.
    if crate::tasks::ordered::engine::on(ctx) {
        return Ok(None);
    }
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
        None => Ok(None),
        // A wait takes the owned path: no parked row borrows fiber storage, and
        // the declined attempt had no effect.
        Some(Err(AdapterError::Blocked)) => {
            binding.adapter.finish(token);
            Ok(None)
        }
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
            Ok(Some((result, inline)))
        }
    }
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
    generation: u32,
    clock: u64,
    at: u64,
) -> Stopped<Option<u64>> {
    // The adapter and wait table are home-affine at width > 1. The bridge
    // also refreshes the run's armed/parked count before returning.
    // Both policies copy the issuer before the home callback leaves the role.
    let task = crate::tasks::current_path(ctx);
    let owner = crate::tasks::role::with_role(ctx, |s| s.scope_key(scope));
    unsafe {
        at_home(ctx, || {
            arm_here(ctx, scope, generation, owner, clock, at, &task)
        })
    }?
}

unsafe fn arm_here(
    ctx: *mut TaskCtx,
    scope: u32,
    generation: u32,
    owner: Option<crate::tasks::registry::ScopeKey>,
    clock: u64,
    at: u64,
    issuer: &[u32],
) -> Stopped<Option<u64>> {
    // Production admission moves every host grant into this service. Explicit
    // harness mixtures can instead retain their scope clock in the home binding.
    if unsafe { (*ctx).run_ref().free_production } {
        let io = unsafe {
            (*ctx)
                .run_ref()
                .free_io
                .get()
                .expect("production Free service")
        };
        return io.arm(owner.expect("live scope timer owner"), clock, at);
    }
    let binding = unsafe { Binding::get(ctx) };
    if binding.adapter.wants_issuer() {
        binding.adapter.issuer(issuer);
    }
    let binding = unsafe { Binding::get(ctx) };
    let token = binding.waits.next;
    binding.waits.next += 1;
    binding.adapter.enter(token);
    let request = Request::SleepUntil {
        clock,
        deadline: at,
    };
    match binding.adapter.scope_timer(request.clone()) {
        Err(AdapterError::Blocked) => {
            binding.adapter.parked(token);
            binding
                .waits
                .armed
                .insert(token, (scope, generation, request));
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
    let expired = expired_binding(binding, ready)?;
    acknowledge_expired(
        binding,
        &expired.iter().map(|e| e.token).collect::<Vec<_>>(),
    );
    Ok(expired.into_iter().map(|e| e.scope).collect())
}

fn expired_binding(binding: &mut Binding, ready: &Ready) -> Stopped<Vec<ExpiredScope>> {
    let mut armed = std::collections::BTreeSet::new();
    let mut select = |token| {
        if let Some((_, _, Request::SleepUntil { deadline, .. })) = binding.waits.armed.get(&token)
        {
            armed.insert((*deadline, token));
        }
    };
    match ready {
        Ready::Tokens(tokens) => tokens.iter().copied().for_each(&mut select),
        Ready::Every => binding
            .waits
            .armed
            .iter_unordered()
            .map(|(t, _)| t)
            .for_each(&mut select),
    }
    for (at, token) in armed {
        if binding.waits.armed_due.contains(&(at, token)) {
            continue;
        }
        let (_, _, request) = binding.waits.armed[&token].clone();
        binding.adapter.enter(token);
        match binding.adapter.scope_timer(request) {
            Err(AdapterError::Blocked) => continue,
            Ok(Completion::Wait(_)) => {}
            Ok(_) => crate::fault("a scope deadline's timer completed as another request"),
            Err(error) => return Err(Stop::refusal(error)),
        }
        binding.adapter.finish(token);
        binding.waits.armed_due.insert((at, token));
    }
    Ok(binding
        .waits
        .armed_due
        .iter()
        .map(|&(_, token)| {
            let (scope, generation, _) = &binding.waits.armed[&token];
            ExpiredScope {
                scope: *scope,
                generation: *generation,
                token,
            }
        })
        .collect())
}

fn acknowledge_expired(binding: &mut Binding, tokens: &[u64]) {
    for token in tokens {
        if let Some((_, _, Request::SleepUntil { deadline, .. })) =
            binding.waits.armed.remove(token)
        {
            binding.waits.armed_due.remove(&(deadline, *token));
        }
    }
}

/// Home only, after the generation-qualified role merge, outside the role.
/// Tokens are unique throughout a run; a delayed acknowledgement cannot name
/// another scope's timer. Unpublished expiries remain latched in armed_due.
pub(crate) unsafe fn expired_ack_home(run: *mut crate::context::RunShared, tokens: &[u64]) {
    if tokens.is_empty() {
        return;
    }
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    acknowledge_expired(binding, tokens);
    unsafe { native_count_home(run, binding) };
}

/// Compatibility entry for a timer-only logical policy.
/// # Safety
/// Home only, with a reserved poll and no parked host requests.
pub(crate) unsafe fn timers_home(
    run: *mut crate::context::RunShared,
    idle: bool,
) -> Stopped<(Vec<u32>, bool)> {
    loop {
        let HomeReady { expired, hosts } = unsafe { ready_home(run) }?;
        assert!(
            hosts.is_empty(),
            "timer-only policy has parked host requests"
        );
        if !idle || !expired.is_empty() {
            return Ok((expired.into_iter().map(|e| e.scope).collect(), true));
        }
        if !unsafe { wait_home(run) }? {
            return Ok((Vec::new(), false));
        }
    }
}

pub(crate) struct HomeReady {
    pub(crate) expired: Vec<ExpiredScope>,
    pub(crate) hosts: Vec<ReadyHost>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ExpiredScope {
    pub(crate) scope: u32,
    pub(crate) generation: u32,
    pub(crate) token: u64,
}

pub(crate) struct ReadyHost {
    pub(crate) generation: u32,
    pub(crate) task: u32,
    pub(crate) token: u64,
    pub(crate) deadline: Option<u64>,
    pub(crate) ready: bool,
}

/// Context-free readiness for the home-loop's reserved retirement poll.
/// # Safety
/// Home only; the policy reserves all host effects and retains no role borrow.
pub(crate) unsafe fn ready_home(run: *mut crate::context::RunShared) -> Stopped<HomeReady> {
    unsafe { ready_home_canceled(run, &[]) }
}

/// Home only; the affected cancellation registrations were validated under R.
pub(crate) unsafe fn ready_home_canceled(
    run: *mut crate::context::RunShared,
    canceled: &[u64],
) -> Stopped<HomeReady> {
    let (mut ready, polled) =
        if unsafe { (*run).host_waits.load(std::sync::atomic::Ordering::Acquire) } != 0 {
            let (ready, polled) = unsafe { ready_binding_home(run, canceled) }?;
            (ready, Some(polled))
        } else {
            let ready = HomeReady {
                expired: Vec::new(),
                hosts: Vec::new(),
            };
            (ready, None)
        };
    if unsafe { ordered_concurrent::pending(run) } {
        ready
            .hosts
            .extend(unsafe { (*run).ordered_io.get().unwrap() }.ready(polled.as_ref())?);
    }
    Ok(ready)
}

unsafe fn ready_binding_home(
    run: *mut crate::context::RunShared,
    canceled: &[u64],
) -> Stopped<(HomeReady, Ready)> {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    let ready = binding.adapter.ready().map_err(Stop::refusal)?;
    let expired = expired_binding(binding, &ready)?;
    if !unsafe { (*run).free_mode } {
        acknowledge_expired(
            binding,
            &expired.iter().map(|e| e.token).collect::<Vec<_>>(),
        );
    }
    let hosts = if unsafe { (*run).free_mode } {
        let mut available = crate::id_map::IdSet::default();
        match &ready {
            Ready::Tokens(tokens) => {
                for &token in tokens {
                    if binding.waits.parked.contains_key(&token) {
                        available.insert(token);
                    }
                }
            }
            Ready::Every => {
                for (token, _) in binding.waits.parked.iter_unordered() {
                    available.insert(token);
                }
            }
        }
        for token in binding.waits.parked_injected.members_unordered() {
            available.insert(token);
        }
        for &token in canceled {
            if let Some(row) = binding.waits.parked.get(&token)
                && binding.adapter.cancellable(token)
                && crate::io_checks::cancellation_result(&row.request, 0).is_some()
            {
                available.insert(token);
            }
        }
        for token in available.members_unordered() {
            let row = binding.waits.parked.get_mut(&token).unwrap();
            if row.completion.is_none() {
                row.ready = true;
                binding.waits.parked_ready.insert(token);
            }
        }
        binding
            .waits
            .parked_ready
            .members_unordered()
            .filter_map(|token| {
                let row = binding.waits.parked.get(&token)?;
                Some(ReadyHost {
                    task: row.task?,
                    generation: row.generation,
                    token,
                    deadline: if let Request::SleepUntil { deadline, .. } = row.request {
                        Some(deadline)
                    } else {
                        None
                    },
                    ready: row.ready,
                })
            })
            .collect()
    } else {
        binding
            .waits
            .parked
            .iter_unordered()
            .filter_map(|(token, parked)| {
                let task = parked.task?;
                let deadline = if let Request::SleepUntil { deadline, .. } = &parked.request {
                    Some(*deadline)
                } else {
                    None
                };
                let ready = ready.contains(token) || binding.adapter.injected_wait(&parked.request);
                Some(ReadyHost {
                    generation: parked.generation,
                    task,
                    token,
                    deadline,
                    ready,
                })
            })
            .collect()
    };
    unsafe {
        (*run).timer_hint.scope_only(binding.waits.scope_only());
        (*run).host_waits.store(
            (binding.waits.parked.len()
                + binding.waits.native.len()
                + binding.waits.armed.len()
                + usize::from(binding.adapter.process_sources())) as u32,
            std::sync::atomic::Ordering::Release,
        )
    };
    Ok((HomeReady { expired, hosts }, ready))
}

/// # Safety
/// As for ready_home; the saved request owns all completion storage.
pub(crate) unsafe fn poll_home(
    run: *mut crate::context::RunShared,
    token: u64,
    cause: Option<u32>,
    ready: bool,
) -> Stopped<bool> {
    if token & CONCURRENT != 0 {
        return Ok(unsafe { (*run).ordered_io.get().unwrap() }
            .poll(token, cause, ready, false)?
            .0);
    }
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    unsafe { poll_binding(binding, token, cause, ready) }
}

/// One reserved Ordered poll, with evidence only when it actually attempted
/// the request. A not-ready/noncanceled probe returns None; Blocked is Some.
/// # Safety
/// As for poll_home. The returned evidence owns all completion storage and
/// names the saved scheduler row without borrowing its TaskCtx.
pub(crate) unsafe fn poll_observed_home(
    run: *mut crate::context::RunShared,
    token: u64,
    cause: Option<u32>,
    ready: bool,
) -> Stopped<(bool, Option<CompletedHost>)> {
    if token & CONCURRENT != 0 {
        let io = unsafe { (*run).ordered_io.get().unwrap() };
        let (done, attempt) = io.poll(token, cause, ready, true)?;
        let task = io.task(token);
        return Ok((
            done,
            attempt.map(|attempt| CompletedHost {
                task,
                result: attempt.result,
                discovery: Some(attempt.discovery),
                keys: None,
            }),
        ));
    }
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    let task = binding
        .waits
        .parked
        .get(&token)
        .and_then(|row| row.task)
        .expect("a saved host owner");
    let mut attempt = None;
    let done = unsafe { poll_binding_observed(binding, token, cause, ready, Some(&mut attempt)) }?;
    Ok((
        done,
        attempt.map(|result| {
            let request = &binding.waits.parked.get(&token).unwrap().request;
            let keys = binding.adapter.keys(request);
            let discovery = Some(if keys.is_some() {
                snapshot(&*binding.adapter, &result)
            } else {
                Err("home retry has no declared footprint")
            });
            CompletedHost {
                task,
                result,
                discovery,
                keys,
            }
        }),
    ))
}

/// Owned evidence of a request whose poll just completed. The caller must
/// still publish its wake before another request can remove this wait row.
/// # Safety
/// Home only, under the same reserved poll as poll_home; no Binding borrow
/// overlaps this access. No borrowed request buffer or native mapping escapes.
pub(crate) unsafe fn completed_home(
    run: *mut crate::context::RunShared,
    token: u64,
) -> CompletedHost {
    if token & CONCURRENT != 0 {
        return unsafe { (*run).ordered_io.get().unwrap() }.completed(token);
    }
    let binding = unsafe { &*(*run).io_binding().cast::<Binding>() };
    let row = binding
        .waits
        .parked
        .get(&token)
        .expect("a reserved host wait");
    CompletedHost {
        task: row.task.expect("a parked host owner"),
        discovery: None,
        keys: None,
        result: crate::tasks::ordered::events::owned_host_result(
            row.completion
                .as_ref()
                .expect("a completed reserved host poll"),
        ),
    }
}

pub(crate) struct CompletedHost {
    pub(crate) keys: Option<crate::io_keys::KeySet>,
    pub(crate) discovery: Option<
        Result<
            std::collections::BTreeMap<u64, (crate::io_keys::Key, crate::io_keys::Kind)>,
            &'static str,
        >,
    >,
    pub(crate) task: u32,
    pub(crate) result: Result<Completion, AdapterError>,
}

/// # Safety
/// Home only, outside the scheduler role and all adapter loans.
pub(crate) unsafe fn wait_sources_home(
    run: *mut crate::context::RunShared,
) -> Stopped<Option<crate::io::WaitSources>> {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    binding.adapter.wait_sources().map_err(Stop::refusal)
}

/// An opaque Free home provider keeps its blocking wait on home. Every
/// concurrent/stop wake descriptor is stable until run reclamation. A provider
/// that cannot watch one declines before entering an uninterruptible wait.
/// # Safety
/// Home only, idle, with no scheduler role or outstanding Binding loan.
pub(crate) unsafe fn wait_opaque_home(
    run: *mut crate::context::RunShared,
    fds: &[std::os::fd::RawFd],
) -> Stopped<bool> {
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    for &fd in fds {
        if !binding.adapter.wake_on(fd) {
            return Err(Stop::Refused(
                "opaque mixed wait needs interruptible home wait".into(),
            ));
        }
    }
    binding.adapter.wait().map_err(Stop::refusal)
}

/// # Safety
/// Home only, after no running segment or runnable turn remains. No role is held.
pub(crate) unsafe fn wait_home(run: *mut crate::context::RunShared) -> Stopped<bool> {
    if unsafe { ordered_concurrent::pending(run) } {
        return unsafe { (*run).ordered_io.get().unwrap().wait(run) };
    }
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    let waited = binding.adapter.wait().map_err(Stop::refusal)?;
    if let Some(stop) = unsafe { (*run).control.stopped() } {
        return Err(stop);
    }
    Ok(waited)
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
    if unsafe { (*ctx).run_ref().free_production } {
        let io = unsafe {
            (*ctx)
                .run_ref()
                .free_io
                .get()
                .expect("production Free service")
        };
        io.disarm(token);
        return;
    }
    let binding = unsafe { Binding::get(ctx) };
    if let Some((_, _, Request::SleepUntil { deadline, .. })) = binding.waits.armed.remove(&token) {
        if !binding.waits.armed_due.remove(&(deadline, token)) {
            binding.adapter.finish(token);
        }
    }
}

/// Original absolute timer ticks for scheduler ordering, alike in live runs and replay.
///
/// # Safety
/// `ctx` is live, its Binding bound, and `token` parked.
pub(crate) unsafe fn deadline(ctx: *mut TaskCtx, token: u64) -> Option<u64> {
    if token & CONCURRENT != 0 && unsafe { (*ctx).run_ref().ordered_io.get().is_some() } {
        return unsafe { (*ctx).run_ref().ordered_io.get().unwrap() }.deadline(token);
    }
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
    if token & CONCURRENT != 0 && crate::tasks::ordered::engine::on(ctx) {
        unsafe { ordered_concurrent::park(ctx, token, task) };
        return;
    }
    // SAFETY: caller contract.
    let (owner, generation) = crate::tasks::free::home::runner_waiter(ctx);
    assert_eq!(owner, task);
    unsafe {
        crate::stop::check(
            ctx,
            at_home(ctx, || park_here(ctx, token, task, generation)),
        )
    }
}

unsafe fn park_here(ctx: *mut TaskCtx, token: u64, task: u32, generation: u32) {
    let binding = unsafe { Binding::get(ctx) };
    if let Some(native) = binding.waits.native.get_mut(&token) {
        native.task = Some(task);
        native.generation = generation;
        native.notified = false;
        if native.ready {
            binding.waits.native_ready.insert(token);
        }
    } else {
        match binding.waits.parked.get_mut(&token) {
            Some(parked) => {
                parked.task = Some(task);
                parked.generation = generation;
            }
            None => crate::fault("a park of an unknown host wait"),
        }
    }
    binding.adapter.parked(token);
}

/// Whether a scheduling point can observe an external wait or scope deadline.
/// # Safety
/// The context is live; its binding, if any, has exclusive access.
pub(crate) unsafe fn idle(ctx: *mut TaskCtx) -> bool {
    if unsafe { ordered_concurrent::pending((*ctx).run_ptr()) } {
        return false;
    }
    if unsafe { (*ctx).io_data() }.is_null() {
        return true;
    }
    let binding = unsafe { Binding::get(ctx) };
    binding.waits.parked.is_empty()
        && binding.waits.armed.is_empty()
        && !binding.adapter.process_sources()
}

/// A scheduling point: the parked tokens to re-issue, asked of the adapter once while a host
/// request is parked or a scope deadline armed.
///
/// # Safety
/// `ctx` is live and no Binding or scheduler borrow crosses this call.
pub(crate) unsafe fn ready(ctx: *mut TaskCtx) -> Stopped<Ready> {
    // SAFETY: caller contract.
    if unsafe { quiet(ctx) && !ordered_concurrent::pending((*ctx).run_ptr()) } {
        return Ok(Ready::Tokens(Vec::new()));
    }
    let concurrent = unsafe { ordered_concurrent::pending((*ctx).run_ptr()) };
    if !concurrent
        && crate::tasks::ordered::engine::on(ctx)
        && unsafe { (*ctx).run_ref().timer_hint.empty_point() }
    {
        return Ok(Ready::Tokens(Vec::new()));
    }
    // Ordered retains its host_poll retirement gate throughout this bridge.
    unsafe { at_home(ctx, || ready_here(ctx)) }?
}

unsafe fn ready_here(ctx: *mut TaskCtx) -> Stopped<Ready> {
    let (home, polled) = unsafe { ready_binding(ctx) }?;
    if !unsafe { ordered_concurrent::pending((*ctx).run_ptr()) } {
        return Ok(home);
    }
    let home_tokens = if matches!(home, Ready::Every) {
        unsafe { Binding::get(ctx) }.waits.every_tokens()
    } else {
        Vec::new()
    };
    let concurrent =
        unsafe { (*ctx).run_ref().ordered_io.get().unwrap() }.ready(polled.then_some(&home))?;
    Ok(ordered_ready_union(home, home_tokens, concurrent))
}

fn ordered_ready_union(home: Ready, every: Vec<u64>, concurrent: Vec<ReadyHost>) -> Ready {
    let mut tokens = match home {
        Ready::Tokens(tokens) => tokens,
        Ready::Every => every,
    };
    tokens.extend(concurrent.into_iter().filter(|r| r.ready).map(|r| r.token));
    tokens.sort_unstable();
    tokens.dedup();
    Ready::Tokens(tokens)
}

/// The home binding's ready tokens, and whether it polled its adapter.
unsafe fn ready_binding(ctx: *mut TaskCtx) -> Stopped<(Ready, bool)> {
    // SAFETY: the context is live.
    if unsafe { (*ctx).io_data() }.is_null() {
        return Ok((Ready::Tokens(Vec::new()), false));
    }
    let binding = unsafe { Binding::get(ctx) };
    if binding.waits.parked.is_empty()
        && binding.waits.armed.is_empty()
        && !binding.adapter.process_sources()
    {
        return Ok((Ready::Tokens(Vec::new()), false));
    }
    let ready = binding.adapter.ready().map_err(Stop::refusal)?;
    // Injected waits are derived, never recorded: each point re-issues them.
    let Ready::Tokens(mut tokens) = ready else {
        return Ok((ready, true));
    };
    tokens.extend(binding.waits.parked_injected.members_unordered());
    tokens.sort_unstable();
    tokens.dedup();
    Ok((Ready::Tokens(tokens), true))
}

/// Phase B/C on home's binding, after the scheduler has applied scope expiry.
/// The adapter receives only canceled consuming children; borrowing reads retain gates.
unsafe fn process_batch_binding(
    run: *mut crate::context::RunShared,
    binding: &mut Binding,
    canceled: &[u64],
) -> Stopped<Ready> {
    let mut children: Vec<u64> = canceled
        .iter()
        .filter_map(|token| {
            let request = binding
                .waits
                .parked
                .get(token)
                .map(|p| &p.request)
                .or_else(|| binding.waits.native.get(token).map(|p| &p.request))?;
            match request {
                Request::ChildWait { child, .. } | Request::GroupWait { group: child, .. } => {
                    Some(*child)
                }
                _ => None,
            }
        })
        .collect();
    children.sort_unstable();
    children.dedup();
    let mut ready = binding
        .adapter
        .process_batch(&children)
        .map_err(|error| match error {
            AdapterError::ProcessFailed {
                site,
                operation_site,
                path,
                ..
            } => {
                let record = crate::failure::FailureRecord {
                    kind: 1,
                    rule: hirsel_core_contract::runtime_rule_code("LANG-9").unwrap(),
                    site,
                    operation_site,
                };
                unsafe {
                    (*(*run).root_context())
                        .failure
                        .append_chain(vec![(record, path)])
                };
                Stop::Failed
            }
            other => Stop::refusal(other),
        })?;
    if binding.adapter.isolated_clock() {
        let canceled = canceled
            .iter()
            .copied()
            .filter(|token| binding.adapter.cancellable(*token))
            .filter(|token| {
                binding.waits.native.get(token).is_some_and(|row| {
                    crate::io_checks::cancellation_result(&row.request, 0).is_some()
                })
            })
            .collect();
        ready = merge_ready(ready, Ready::Tokens(canceled));
    }
    let tokens: Vec<u64> = match &ready {
        Ready::Tokens(tokens) => tokens.clone(),
        Ready::Every => binding
            .waits
            .native
            .iter_unordered()
            .map(|(token, _)| token)
            .collect(),
    };
    for token in tokens {
        if let Some(row) = binding.waits.native.get_mut(&token)
            && !row.notified
        {
            row.ready = true;
            binding.waits.native_ready.insert(token);
        }
    }
    Ok(ready)
}

pub(crate) unsafe fn process_batch_home(
    run: *mut crate::context::RunShared,
    canceled: &[u64],
) -> Stopped<Ready> {
    if unsafe { (*run).io_binding() }.is_null() {
        return Ok(Ready::Tokens(Vec::new()));
    }
    let binding = unsafe { &mut *(*run).io_binding().cast::<Binding>() };
    let ready = unsafe { process_batch_binding(run, binding, canceled) };
    unsafe { native_count_home(run, binding) };
    ready
}

pub(crate) unsafe fn process_batch(ctx: *mut TaskCtx, canceled: &[u64]) -> Stopped<Ready> {
    unsafe { at_home(ctx, || process_batch_home((*ctx).run_ptr(), canceled)) }?
}

pub(crate) fn merge_ready(a: Ready, b: Ready) -> Ready {
    match (a, b) {
        (Ready::Every, _) | (_, Ready::Every) => Ready::Every,
        (Ready::Tokens(mut a), Ready::Tokens(b)) => {
            a.extend(b);
            a.sort_unstable();
            a.dedup();
            Ready::Tokens(a)
        }
    }
}
/// The parked tasks whose request `ready` names. Their order is immaterial: the scheduler
/// polls the woken tasks by deadline, then park order.
///
/// # Safety
/// `ctx` is live; with no Binding bound nothing is parked.
pub(crate) unsafe fn woken(ctx: *mut TaskCtx, ready: &Ready) -> Vec<u32> {
    if matches!(ready, Ready::Tokens(tokens) if tokens.is_empty()) {
        return Vec::new();
    }
    // SAFETY: caller contract.
    let mut woken = if unsafe { quiet(ctx) } {
        Vec::new()
    } else {
        unsafe { crate::stop::check(ctx, at_home(ctx, || woken_here(ctx, ready))) }
    };
    if let Some(io) = unsafe { (*ctx).run_ref().ordered_io.get() } {
        woken.extend(crate::tasks::role::with_role(ctx, |s| io.woken(s, ready)));
    }
    woken
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
    let observing =
        crate::tasks::ordered::engine::on(ctx) && crate::tasks::ordered::events::observing(ctx);
    if token & CONCURRENT != 0 {
        let io = unsafe { (*ctx).run_ref().ordered_io.get().unwrap() };
        let (done, attempt) = {
            let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
            io.poll(token, cause, ready, observing)
        }?;
        if let Some(attempt) = attempt {
            let result = attempt.result;
            let task = io.task(token);
            let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
            let discovery = attempt.discovery.and_then(|d| {
                crate::tasks::ordered::events::discover_host_for(s, task, &result, &d)
            });
            crate::tasks::ordered::events::record_host_attempt(s, task, &result, false);
            if done {
                crate::tasks::ordered::events::record_host_poll(s, task, result, false);
            }
            discovery.map_err(|e| Stop::Refused(e.into()))?;
        }
        return Ok(done);
    }
    if !observing {
        // SAFETY: caller contract; ordinary path retains no evidence.
        return unsafe { at_home(ctx, || poll_here(ctx, token, cause, ready)) }?;
    }
    // Borrow only the home service on this bridge; copy the completion while
    // its wait row still owns it, then record after the caller regains role.
    let (done, attempt) = {
        let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
        unsafe {
            at_home(ctx, || {
                poll_observed_home((*ctx).run_ptr(), token, cause, ready)
            })
        }
    }??;
    if let Some(completion) = attempt {
        // SAFETY: the bridge returned the role; do not observe a stop while
        // this owned completion has yet to be installed in the log.
        let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
        crate::tasks::ordered::events::home_retry_keys(
            s,
            completion.task,
            completion.keys.as_ref(),
        );
        if let Some(discovered) = completion.discovery {
            crate::tasks::ordered::events::home_discovery(
                s,
                completion.task,
                &completion.result,
                discovered,
            );
        }
        crate::tasks::ordered::events::record_host_attempt(
            s,
            completion.task,
            &completion.result,
            false,
        );
        if done {
            crate::tasks::ordered::events::record_host_poll(
                s,
                completion.task,
                completion.result,
                false,
            );
        }
    }
    Ok(done)
}

unsafe fn poll_here(
    ctx: *mut TaskCtx,
    token: u64,
    cause: Option<u32>,
    ready: bool,
) -> Stopped<bool> {
    if token & CONCURRENT != 0 {
        return Ok(unsafe { (*ctx).run_ref().ordered_io.get().unwrap() }
            .poll(token, cause, ready, false)?
            .0);
    }
    let binding = unsafe { Binding::get(ctx) };
    unsafe { poll_binding(binding, token, cause, ready) }
}

unsafe fn poll_binding(
    binding: &mut Binding,
    token: u64,
    cause: Option<u32>,
    ready: bool,
) -> Stopped<bool> {
    unsafe { poll_binding_observed(binding, token, cause, ready, None) }
}

unsafe fn poll_binding_observed(
    binding: &mut Binding,
    token: u64,
    cause: Option<u32>,
    ready: bool,
    mut observed: Option<&mut Option<Result<Completion, AdapterError>>>,
) -> Stopped<bool> {
    let _poll = crate::io_home::Poll::new();
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
    binding.waits.parked_ready.remove(&token);
    binding.waits.parked.get_mut(&token).unwrap().ready = false;
    let completion = match cause {
        Some(cause) => {
            binding.adapter.finish(token);
            // SAFETY: no borrow of the Binding is held across this call.
            match crate::io_checks::cancel(binding.adapter.as_mut(), &request, cause)
                .map_err(Stop::refusal)?
            {
                Some(completion) => Ok(completion),
                None => crate::fault("a parked request that is no cancellation point"),
            }
        }
        None => {
            binding.adapter.enter(token);
            let result = binding.adapter.call(request);
            if let Some(observed) = observed.as_mut() {
                **observed = Some(crate::tasks::ordered::events::owned_host_result(&result));
            }
            match result {
                Err(AdapterError::Blocked) => return Ok(false),
                result => {
                    binding.adapter.finish(token);
                    result
                }
            }
        }
    };
    if let Some(observed) = observed
        && observed.is_none()
    {
        *observed = Some(crate::tasks::ordered::events::owned_host_result(
            &completion,
        ));
    }
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
    let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
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
#[cfg(test)]
mod native_tests {
    use super::*;
    #[test]
    fn first_blocked_host_call_creates_the_root_scheduler_before_polling() {
        struct FirstWait(bool, Completion);
        impl crate::io::Adapter for FirstWait {
            fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
                Ok(None)
            }
            fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
                if std::mem::take(&mut self.0) {
                    Err(AdapterError::Blocked)
                } else {
                    Ok(self.1.clone())
                }
            }
        }
        for (request, completion) in [
            (
                Request::SleepUntil {
                    clock: 17,
                    deadline: 100,
                },
                Completion::Wait(Ok(())),
            ),
            (
                Request::ReadStdin { stdin: 17, max: 1 },
                Completion::Input(Ok(vec![42])),
            ),
        ] {
            let mut run = crate::host::BodyRun::new(1, false);
            run.adapter = Some(Box::new(FirstWait(true, completion.clone())));
            let outcome = run.run(
                |_| {},
                move |ctx| unsafe {
                    let _role = crate::tasks::role::Guard::new(ctx);
                    assert!((*(*ctx).run_ref().tasks.get()).is_none());
                    let (result, parked) = issue(ctx, request.clone()).unwrap();
                    assert_eq!(result, Ok(completion.clone()));
                    assert!(parked);
                    assert!((*(*ctx).run_ref().tasks.get()).is_some());
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }

    struct Edge(bool);
    impl crate::io::Adapter for Edge {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            unreachable!()
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            Ok(Ready::Tokens(if std::mem::take(&mut self.0) {
                vec![7]
            } else {
                vec![]
            }))
        }
    }
    #[test]
    fn native_edge_survives_registration_and_save_until_owner_notification() {
        let mut binding = Binding::new(Edge(true));
        binding.waits.native.insert(
            7,
            Native {
                request: Request::SleepUntil {
                    clock: 17,
                    deadline: 100,
                },
                task: None,
                generation: 0,
                notified: false,
                ready: false,
            },
        );
        assert!(
            native_ready_binding(&mut binding, &[])
                .unwrap()
                .hosts
                .is_empty()
        );
        binding.waits.native.get_mut(&7).unwrap().task = Some(3);
        // The adapter has already consumed its one ready edge.
        for _ in 0..2 {
            let snapshot = native_ready_binding(&mut binding, &[]).unwrap();
            assert_eq!(snapshot.hosts.len(), 1);
            assert_eq!(snapshot.hosts[0].task, 3);
            assert!(snapshot.hosts[0].ready);
        }
        binding.waits.native.get_mut(&7).unwrap().notified = true;
        assert!(
            native_ready_binding(&mut binding, &[])
                .unwrap()
                .hosts
                .is_empty()
        );
    }
}

#[cfg(test)]
mod ordered_poll_tests;

#[cfg(test)]
#[path = "host_waits/modules.rs"]
mod runtime50;
