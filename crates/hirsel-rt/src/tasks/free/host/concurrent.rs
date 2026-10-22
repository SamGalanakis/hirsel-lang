//! Concurrent readiness keeps only owned requests and runtime row identities.
//! Like the home service, it latches notifications before physical save; only
//! the owner executes the next recorded bounded attempt.
use super::*;
use crate::context::RunShared;
use crate::io_concurrent::ConcurrentAdapter;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const CONCURRENT: u64 = 1 << 63;

pub(crate) struct Service {
    pub(super) adapter: Arc<dyn ConcurrentAdapter>,
    pub(super) live: Arc<super::live::State>,
    driver: Option<crate::readiness::Readiness>,
    rows: Mutex<BTreeMap<u64, Row>>,
    pending: AtomicUsize,
}

struct Row {
    request: Request,
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
        assert!(
            run.free_io
                .set(Arc::new(Self {
                    adapter,
                    live,
                    driver,
                    rows: Default::default(),
                    pending: AtomicUsize::new(0),
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
        self.pending.load(Ordering::Acquire) != 0
    }

    #[cfg(test)]
    pub(crate) fn test_waiting(&self) -> bool {
        self.rows
            .lock()
            .unwrap()
            .values()
            .any(|row| row.task.is_some())
    }

    pub(super) fn register(&self, token: u64, request: Request) -> u64 {
        assert_eq!(token >> 48, 0, "request token below driver tag bits");
        let wait = token | CONCURRENT;
        assert!(
            self.rows
                .lock()
                .unwrap()
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
        let Some(row) = rows.get_mut(&token) else {
            return false;
        };
        row.task = Some((task, generation));
        true
    }

    pub(super) fn retry(&self, token: u64) {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.get_mut(&token).unwrap();
        row.task = None;
        row.ready = false;
        row.notified = false;
    }

    pub(super) fn finish(&self, token: u64) {
        assert!(self.rows.lock().unwrap().remove(&token).is_some());
        self.pending.fetch_sub(1, Ordering::Release);
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // The last physical continuation released the run. No issuer or
        // home poll can still use these request-owned registrations.
        for token in self.rows.get_mut().unwrap().keys() {
            self.adapter.finish(token & !CONCURRENT);
        }
    }
}

/// Run a bounded provider setup outside the scheduler role. No adapter loan
/// escapes this call, and no facade borrow is retained while the role is free.
/// Replay consumes only the resulting request digest and invokes no provider.
pub(crate) fn with_current(action: impl FnOnce(&dyn ConcurrentAdapter)) -> bool {
    let ctx = crate::stop::running();
    if ctx.is_null() || !super::native::enabled(ctx) {
        return false;
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
    // Clone only from that stable owner after preparation has returned.
    unsafe { prepare_with_keys(ctx, request, keys) }?;
    let (token, request) = {
        let pending = unsafe { (*ctx).pending_host.as_ref().unwrap() };
        (pending.request.id, pending.request.buffers().clone())
    };
    let wait = io.register(token, request);
    if !unsafe { claim(ctx) }? {
        return Err(Stop::Requested);
    }
    unsafe { invoke_concurrent(ctx, &*io.adapter) }?;
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
        unsafe { invoke_concurrent(ctx, &*io.adapter) }?;
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
/// # Safety
/// The owner holds the role and its request buffers; the adapter is admitted.
pub(super) unsafe fn native_keys(ctx: *mut TaskCtx, request: &Request) -> Stopped<KeySet> {
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
    Ok(expected)
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
        let rows = io.rows.lock().unwrap();
        let canceled = rows
            .iter()
            .filter_map(|(&token, row)| {
                let (task, generation) = row.task?;
                let task_row = s.tasks.get(task as usize)?.as_ref()?;
                (task_row.generation == generation
                    && s.canceled_task(task, task_row.shield()).is_some()
                    && crate::io_checks::cancellation_result(&row.request, 0).is_some())
                .then_some(token & !CONCURRENT)
            })
            .collect::<Vec<_>>();
        (idle, canceled)
    };
    let result = io.adapter.ready();
    let canceled = canceled
        .into_iter()
        .filter(|&t| io.adapter.cancellable(t))
        .collect::<Vec<_>>();
    let _guard = unsafe { role::IdleGuard::new(run) };
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
    let mut changed = false;
    let mut waiting = false;
    {
        let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
        for (&token, row) in io.rows.lock().unwrap().iter_mut() {
            row.ready |=
                ready.contains(token & !CONCURRENT) || canceled.contains(&(token & !CONCURRENT));
            let Some((task, generation)) = row.task else {
                continue;
            };
            if row.notified {
                continue;
            }
            waiting = true;
            let valid = s
                .tasks
                .get(task as usize)
                .and_then(Option::as_ref)
                .is_some_and(|r| {
                    r.generation == generation
                        && matches!(r.wait, Some(Wait::Host { token: t }) if t == token)
                        && super::super::super::readiness::saved(s, task)
                });
            if valid && row.ready {
                row.notified = true;
                s.unpark(task);
                changed = true;
            }
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
    if !matches!(sources, Ok(true)) {
        let _guard = unsafe { role::IdleGuard::new(run) };
        match sources {
            Ok(false) => unsafe { super::super::check_quiet_without_sources(run) },
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
    let concurrent = io.adapter.wait_sources().map_err(Stop::refusal)?;
    let home = if unsafe { (*run).host_waits.load(Ordering::Acquire) != 0 } {
        unsafe { crate::host_waits::wait_sources_home(run) }?
            .ok_or_else(|| Stop::Refused("mixed wait needs home adapter source exposure".into()))?
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
