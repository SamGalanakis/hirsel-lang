//! Concurrent readiness keeps only owned requests and runtime row identities.
//! Like the home service, it latches notifications before physical save; only
//! the owner executes the next recorded bounded attempt.
use super::*;
use crate::context::RunShared;
use crate::io_concurrent::ConcurrentAdapter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const CONCURRENT: u64 = 1 << 63;

pub(crate) struct Service {
    pub(super) adapter: Arc<dyn ConcurrentAdapter>,
    pub(super) live: Arc<super::live::State>,
    pub(super) jobs: super::jobs::Jobs,
    driver: Option<crate::readiness::Readiness>,
    rows: Mutex<Rows>,
    pending: AtomicUsize,
    opaque: Mutex<Option<opaque::Wake>>,
    timers: timers::Timers,
}

#[derive(Default)]
struct Rows {
    entries: BTreeMap<u64, Row>,
    /// Ready registrations whose saved owner has not acknowledged the edge.
    /// Keep early edges here; only retry/finish or successful notification
    /// removes them. Idle registrations are never visited by the merge.
    ready: BTreeSet<u64>,
}

impl Rows {
    fn ready(&mut self, token: u64) {
        if let Some(row) = self.entries.get_mut(&token)
            && !row.notified
        {
            row.ready = true;
            self.ready.insert(token);
        }
    }
}

struct Row {
    /// The request's one immutable owner, shared with its table and Pending.
    request: Arc<hirsel_free::claim::Request<Request>>,
    task: Option<(u32, u32)>,
    ready: bool,
    notified: bool,
}

impl Service {
    /// Sole-root admission, before requests. No adapter method runs under a role.
    pub(crate) unsafe fn install(
        ctx: *mut TaskCtx,
        adapter: Arc<dyn ConcurrentAdapter>,
    ) -> Stopped<()> {
        let run = unsafe { (*ctx).run_ref() };
        assert!(run.free_mode);
        let coupled = adapter.namespace_coupled();
        if run.free_production
            && super::super::record::on(ctx)
            && run.free_namespace_coupled.load(Ordering::Relaxed) != coupled
        {
            return Err(Stop::Refused(
                "namespace policy changed after recording admission".into(),
            ));
        }
        run.free_namespace_coupled.store(coupled, Ordering::Relaxed);
        let driver = adapter.readiness();
        let live = Arc::new(super::live::State::default());
        if run.free_production && !super::super::record::on(ctx) && !super::super::replay::on(ctx) {
            run.control.terminal(live.clone());
        }
        if let Some(driver) = &driver {
            driver.notify().map_err(Stop::refusal)?;
            let fd = run
                .control
                .wake_fd()
                .ok_or_else(|| Stop::Refused("concurrent host stop descriptor".into()))?;
            driver.wake_on(fd).map_err(Stop::refusal)?;
        }
        let jobs = super::jobs::Jobs::default();
        let (ready, home) = (driver.clone(), unsafe {
            crate::pool::home_wake((*ctx).run_ptr())
        });
        jobs.set_waker(Arc::new(move || {
            if let Some(ready) = &ready {
                let _ = ready.notify();
            }
            if let Some(home) = &home {
                home();
            }
        }));
        assert!(
            run.free_io
                .set(Arc::new(Self {
                    adapter,
                    live,
                    jobs,
                    driver,
                    rows: Default::default(),
                    pending: AtomicUsize::new(0),
                    opaque: Mutex::new(None),
                    timers: Default::default(),
                }))
                .is_ok()
        );
        Ok(())
    }

    pub(crate) fn notify(&self) {
        if let Some(driver) = &self.driver {
            // Admission already created and registered this eventfd.
            driver
                .notify()
                .expect("an admitted concurrent wake descriptor");
        }
    }

    pub(crate) fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) != 0 || self.timers.pending() || self.jobs.busy()
    }

    #[cfg(test)]
    pub(crate) fn test_waiting(&self) -> bool {
        self.rows
            .lock()
            .unwrap()
            .entries
            .values()
            .any(|row| row.task.is_some())
    }

    pub(super) fn register(
        &self,
        token: u64,
        request: Arc<hirsel_free::claim::Request<Request>>,
    ) -> u64 {
        assert_eq!(token >> 48, 0, "request token below driver tag bits");
        let wait = token | CONCURRENT;
        assert!(
            self.rows
                .lock()
                .unwrap()
                .entries
                .insert(
                    wait,
                    Row {
                        request,
                        task: None,
                        ready: false,
                        notified: false,
                    }
                )
                .is_none()
        );
        self.pending.fetch_add(1, Ordering::Release);
        wait
    }

    /// Called under the role, after the logical park and before physical save.
    pub(crate) fn park(&self, token: u64, task: u32, generation: u32) -> bool {
        let mut rows = self.rows.lock().unwrap();
        let Some(row) = rows.entries.get_mut(&token) else {
            return false;
        };
        row.task = Some((task, generation));
        if row.ready {
            rows.ready.insert(token);
        }
        true
    }

    pub(super) fn retry(&self, token: u64) {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.entries.get_mut(&token).unwrap();
        row.task = None;
        row.ready = false;
        row.notified = false;
        rows.ready.remove(&token);
    }

    pub(super) fn finish(&self, token: u64) {
        let mut rows = self.rows.lock().unwrap();
        assert!(rows.entries.remove(&token).is_some());
        rows.ready.remove(&token);
        self.pending.fetch_sub(1, Ordering::Release);
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // The last physical continuation released the run. No issuer or
        // home poll can still use these request-owned registrations.
        self.timers.clear(&*self.adapter);
        for token in self.rows.get_mut().unwrap().entries.keys() {
            self.adapter.finish(token & !CONCURRENT);
        }
    }
}

/// Run a bounded provider setup outside the scheduler role. No adapter loan
/// escapes this call, and no facade borrow is retained while the role is free.
/// Replay consumes only the resulting request digest and invokes no provider.
pub(crate) fn with_current(action: impl FnOnce(&dyn ConcurrentAdapter)) -> bool {
    let ctx = crate::stop::running();
    if ctx.is_null() {
        return false;
    }
    // Live Ordered sets up its own concurrent service's providers.
    if !super::native::enabled(ctx) {
        let Some(io) = (crate::tasks::ordered::engine::on(ctx))
            .then(|| unsafe { (*ctx).run_ref().ordered_io.get() })
            .flatten()
        else {
            return false;
        };
        let _segment = unsafe { role::Segment::new(ctx) };
        action(&*io.adapter);
        return true;
    }
    if unsafe { (*ctx).run_ref().free_replay } {
        return true;
    }
    let io = unsafe { (*ctx).run_ref().free_io.get().expect("native Free service") };
    let _segment = unsafe { role::Segment::new(ctx) };
    action(&*io.adapter);
    true
}

/// # Safety
/// A live run; this query never accesses a context or scheduler.
pub(crate) unsafe fn pending(run: *mut RunShared) -> bool {
    unsafe { (*run).free_io.get() }.is_some_and(|io| io.pending())
}

/// # Safety
/// Explicit recorded Free owner, admitted concurrent adapter, role held.
pub(crate) unsafe fn request(
    ctx: *mut TaskCtx,
    request: Request,
) -> Stopped<Result<Completion, AdapterError>> {
    // The physical continuation retains RunShared, which owns this stable
    // Service. A terminal cut abandons Rust frames without running Drop, so
    // never put an owning Arc on a stack that can be saved here.
    let io = unsafe {
        &**(*ctx)
            .run_ref()
            .free_io
            .get()
            .expect("concurrent host admission")
    };
    let keys = {
        let _segment = unsafe { role::Segment::new(ctx) };
        io.adapter.keys(&request)
    };
    unsafe { request_with_keys(ctx, request, keys) }
}

/// # Safety
/// As request; the admitted key footprint was validated outside the role.
pub(super) unsafe fn request_with_keys(
    ctx: *mut TaskCtx,
    request: Request,
    keys: KeySet,
) -> Stopped<Result<Completion, AdapterError>> {
    let io = unsafe {
        &**(*ctx)
            .run_ref()
            .free_io
            .get()
            .expect("concurrent host admission")
    };
    // Move every buffer into Pending before begin may reject an admitted cut.
    // The readiness row shares that stable owner after preparation returned.
    unsafe { prepare_with_keys(ctx, request, keys) }?;
    let (token, request) = {
        let pending = unsafe { (*ctx).pending_host.as_ref().unwrap() };
        (pending.request.id, pending.request.clone())
    };
    let wait = io.register(token, request);
    unsafe { super::blocking::before_claim(ctx, io, wait) }?;
    if !unsafe { claim(ctx) }? {
        return Err(Stop::Requested);
    }
    unsafe { super::blocking::invoke(ctx, io, wait) }?;
    unsafe { ended(ctx) }?;
    while matches!(
        unsafe { &(*ctx).pending_host.as_ref().unwrap().completion },
        Some(Err(AdapterError::Blocked))
    ) {
        {
            let _segment = unsafe { role::Segment::new(ctx) };
            io.adapter.parked(token);
        }
        unsafe { try_block(&mut *ctx, Wait::Host { token: wait }, false, 0) }?;
        io.retry(wait);
        unsafe { attempt(ctx) }?;
        unsafe { super::blocking::invoke(ctx, io, wait) }?;
        unsafe { ended(ctx) }?;
    }
    {
        let _segment = unsafe { role::Segment::new(ctx) };
        io.adapter.finish(token);
    }
    io.finish(wait);
    unsafe { complete_result(ctx) }
}

/// Validate the whole supported footprint before any prepared record or effect.
/// The same released segment also asks whether the adapter runs the request as
/// a blocking job, so admission needs no second role release for it.
/// # Safety
/// The owner holds the role and its request buffers; the adapter is admitted.
pub(super) unsafe fn native_keys(ctx: *mut TaskCtx, request: &Request) -> Stopped<(KeySet, bool)> {
    let io = unsafe {
        (*ctx)
            .run_ref()
            .free_io
            .get()
            .expect("concurrent host admission")
    };
    let _segment = unsafe { role::Segment::new(ctx) };
    let expected = super::native::request_keys_with_domains(
        request,
        |word| {
            io.adapter
                .identity(word)
                .ok_or_else(|| Stop::Refused("unmapped live host resource".into()))
        },
        |word| {
            io.adapter
                .process_keys(word)
                .ok_or_else(|| Stop::Refused("unmapped process issue domains".into()))
        },
        |word| {
            io.adapter
                .net_ports(word)
                .ok_or_else(|| Stop::Refused("unmapped Net port domain".into()))
        },
        unsafe { (*ctx).run_ref().free_root_env },
    )
    .map_err(|_| Stop::Refused("native host request metadata is not admitted".into()))?;
    let coupled = unsafe {
        (*ctx)
            .run_ref()
            .free_namespace_coupled
            .load(Ordering::Relaxed)
    };
    if coupled != io.adapter.namespace_coupled() {
        return Err(Stop::Refused("namespace policy changed during run".into()));
    }
    let expected = super::native::namespace_policy(expected, coupled);
    if expected != io.adapter.keys(request) {
        return Err(Stop::Refused(
            "native host footprint has unadmitted domains".into(),
        ));
    }
    Ok((expected, io.adapter.blocking(request)))
}

/// # Safety
/// Home only. Adapter calls have no role; the merge uses saved row metadata.
pub(crate) unsafe fn service(run: *mut RunShared) -> bool {
    let Some(io) = (unsafe { (*run).free_io.get() }) else {
        return false;
    };
    if !io.pending() {
        return false;
    }
    let idle = unsafe { crate::pool::busy(&*run) } == 0;
    let (idle, canceled) = {
        let Some(_guard) = (unsafe { role::IdleGuard::try_new(run) }) else {
            return false;
        };
        let Some(s) = (unsafe { (*(*run).tasks.get()).as_deref_mut() }) else {
            return false;
        };
        let idle = idle
            && s.runnable.is_empty()
            && s.tasks.iter().flatten().all(|row| {
                row.state != State::Ready && row.dispatch.as_ref().is_none_or(|f| f.is_saved())
            });
        let notices = s.host_cancellations(CONCURRENT, CONCURRENT);
        let rows = io.rows.lock().unwrap();
        let canceled = notices
            .into_iter()
            .filter(|token| {
                rows.entries.get(token).is_some_and(|row| {
                    crate::io_checks::cancellation_result(row.request.buffers(), 0).is_some()
                })
            })
            .map(|token| token & !CONCURRENT)
            .collect::<Vec<_>>();
        (idle, canceled)
    };
    let result = io.adapter.ready().and_then(|ready| {
        io.timers.poll(&*io.adapter, &ready)?;
        Ok(ready)
    });
    let canceled = canceled
        .into_iter()
        .filter(|&t| io.adapter.cancellable(t))
        .collect::<Vec<_>>();
    let _guard = unsafe { role::IdleGuard::home(run) };
    if unsafe { (*run).control.stopped().is_some() } {
        return true;
    }
    let ready = match result {
        Ok(ready) => ready,
        Err(error) => {
            unsafe { (*run).control.request_inside(Stop::refusal(error)) };
            return true;
        }
    };
    let jobs = io.jobs.drain();
    let mut changed = false;
    let waiting = io.pending();
    {
        let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
        changed |= io.timers.expire(s);
        let mut rows = io.rows.lock().unwrap();
        match ready {
            crate::io::Ready::Tokens(tokens) => {
                for token in tokens {
                    rows.ready(token | CONCURRENT);
                }
            }
            crate::io::Ready::Every => {
                let tokens = rows.entries.keys().copied().collect::<Vec<_>>();
                for token in tokens {
                    rows.ready(token);
                }
            }
        }
        for token in canceled {
            rows.ready(token | CONCURRENT);
        }
        // Owned job completions and credit grants carry their row's wait token.
        for token in jobs {
            rows.ready(token);
        }
        let notifications = std::mem::take(&mut rows.ready);
        for token in notifications {
            let Some(row) = rows.entries.get_mut(&token) else {
                continue;
            };
            if row.notified {
                continue;
            }
            let Some((task, generation)) = row.task else {
                rows.ready.insert(token);
                continue;
            };
            let valid = s
                .tasks
                .get(task as usize)
                .and_then(Option::as_ref)
                .is_some_and(|r| {
                    r.generation == generation
                        && matches!(r.wait, Some(Wait::Host { token: t }) if t == token)
                });
            if !valid {
                continue;
            }
            if !super::super::super::readiness::saved(s, task) {
                rows.ready.insert(token);
                continue;
            }
            if valid {
                row.notified = true;
                s.unpark(task);
                changed = true;
            } else {
                rows.ready.insert(token);
            }
        }
        drop(rows);
        // Expiry also wakes runtime-only waits (channels, joins and cleanup).
        // Poll saved owners before retiring the last external event source.
        if changed {
            unsafe { super::super::super::readiness::poll_free(s) };
        }
        unsafe { super::super::publish_ready(s, run) };
    }
    let mixed = unsafe { (*run).host_waits.load(Ordering::Acquire) != 0 };
    drop(_guard);
    if mixed || changed || !idle || !waiting {
        // The home service polls its sources too, then owns the combined wait.
        return changed;
    }
    let sources = unsafe { wait(run) };
    // An in-flight job is an event source that wakes home through its inbox:
    // with nothing else to wait on, block there instead of declaring quiet.
    if matches!(sources, Ok(false)) && io.jobs.busy() {
        return false;
    }
    if !matches!(sources, Ok(true)) {
        let _guard = unsafe { role::IdleGuard::home(run) };
        match sources {
            Ok(false) => unsafe { super::super::check_quiet_without_sources(run, None) },
            Err(stop) => {
                unsafe { (*run).control.request_inside(stop) };
            }
            Ok(true) => unreachable!(),
        }
    }
    true
}

/// Prepare both source sets without role/adapter loans across the shared wait.
/// # Safety
/// Home only, with no running or runnable task and no scheduler role.
pub(crate) unsafe fn wait(run: *mut RunShared) -> Stopped<bool> {
    let io = unsafe { (*run).free_io.get().expect("concurrent admission") };
    // The final concurrent completion can leave an opaque home request alone.
    // Keep the shared stop-aware wait, without polling an idle adapter's stale
    // wake-only readiness when it has no remaining requests or timers.
    let concurrent = if io.pending() {
        io.adapter.wait_sources().map_err(Stop::refusal)?
    } else {
        crate::io::WaitSources::Drivers(Vec::new())
    };
    let home = if unsafe { (*run).host_waits.load(Ordering::Acquire) != 0 } {
        match unsafe { crate::host_waits::wait_sources_home(run) }? {
            Some(sources) => sources,
            None => return unsafe { opaque::wait(run, io, concurrent) },
        }
    } else {
        crate::io::WaitSources::Drivers(Vec::new())
    };
    let waited = match (concurrent, home) {
        (crate::io::WaitSources::Ready, _) | (_, crate::io::WaitSources::Ready) => true,
        (crate::io::WaitSources::Drivers(mut a), crate::io::WaitSources::Drivers(b)) => {
            a.extend(b);
            crate::io::WaitSources::Drivers(a)
                .wait()
                .map_err(Stop::refusal)?
        }
    };
    if let Some(stop) = unsafe { (*run).control.stopped() } {
        return Err(stop);
    }
    Ok(waited)
}

mod opaque;
mod timers;
#[cfg(test)]
pub(crate) use timers::runtime50::hooks as timer_hooks;
