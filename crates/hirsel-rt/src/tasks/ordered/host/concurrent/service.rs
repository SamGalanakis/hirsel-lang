//! Owned wait rows follow Free's concurrent service registration and generation
//! checks. Ordered differs by retrying effects in the existing retirement poll,
//! before publishing any wake, rather than on the resumed request owner.
use super::*;
use crate::context::RunShared;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

pub(crate) const CONCURRENT: u64 = 1 << 63;

pub(crate) struct Attempt {
    pub(crate) result: Result<Completion, AdapterError>,
    pub(crate) discovery: Discovered,
}

pub(crate) struct Service {
    pub(crate) adapter: Arc<dyn ConcurrentAdapter>,
    driver: Option<crate::readiness::Readiness>,
    rows: Mutex<BTreeMap<u64, Row>>,
    pending: AtomicUsize,
}

struct Row {
    request: Request,
    task: Option<(u32, u32)>,
    completion: Option<Result<Completion, AdapterError>>,
}

impl Service {
    /// Sole-root setup. No role is held while adapter callbacks run.
    pub(crate) unsafe fn install(
        ctx: *mut TaskCtx,
        adapter: Arc<dyn ConcurrentAdapter>,
    ) -> Stopped<()> {
        let run = unsafe { (*ctx).run_ref() };
        let driver = adapter.readiness();
        if let Some(driver) = &driver {
            driver.notify().map_err(Stop::refusal)?;
            if let Some(fd) = run.control.wake_fd() {
                driver.wake_on(fd).map_err(Stop::refusal)?;
            }
        }
        assert!(
            run.ordered_io
                .set(Arc::new(Self {
                    adapter,
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
            driver.notify().expect("admitted Ordered wake descriptor");
        }
    }

    pub(crate) fn park(&self, token: u64, task: u32, generation: u32) {
        assert!(
            self.rows
                .lock()
                .unwrap()
                .get_mut(&token)
                .unwrap()
                .task
                .replace((task, generation))
                .is_none()
        );
        self.adapter.parked(token & !CONCURRENT);
    }

    pub(crate) fn deadline(&self, token: u64) -> Option<u64> {
        match &self.rows.lock().unwrap().get(&token)?.request {
            Request::SleepUntil { deadline, .. } => Some(*deadline),
            _ => None,
        }
    }

    pub(crate) fn ready(&self) -> Stopped<Vec<crate::host_waits::ReadyHost>> {
        let ready = self.adapter.ready().map_err(Stop::refusal)?;
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(&token, row)| {
                let (task, _) = row.task?;
                let deadline = match row.request {
                    Request::SleepUntil { deadline, .. } => Some(deadline),
                    _ => None,
                };
                Some(crate::host_waits::ReadyHost {
                    task,
                    token,
                    deadline,
                    ready: ready.contains(token & !CONCURRENT),
                })
            })
            .collect())
    }

    pub(crate) fn valid(&self, token: u64, task: u32, generation: u32) -> bool {
        self.rows
            .lock()
            .unwrap()
            .get(&token)
            .is_some_and(|r| r.task == Some((task, generation)))
    }

    pub(crate) fn woken(&self, s: &crate::tasks::Scheduler, ready: &crate::io::Ready) -> Vec<u32> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(&token, row)| {
                let (task, generation) = row.task?;
                let valid = s.tasks.get(task as usize).and_then(Option::as_ref).is_some_and(|r| {
                r.generation == generation
                    && matches!(r.wait, Some(crate::tasks::Wait::Host { token: t }) if t == token)
            });
                (ready.contains(token) && valid).then_some(task)
            })
            .collect()
    }

    /// One reserved bounded attempt. No row, scheduler or context loan crosses
    /// a callback. Completion storage is published before any scheduler wake.
    pub(crate) fn poll(
        &self,
        token: u64,
        cause: Option<u32>,
        ready: bool,
        observing: bool,
    ) -> Stopped<(bool, Option<Attempt>)> {
        let request = self
            .rows
            .lock()
            .unwrap()
            .get(&token)
            .unwrap()
            .request
            .clone();
        let cause = cause.filter(|_| self.adapter.cancellable(token & !CONCURRENT));
        if cause.is_none() && !ready {
            return Ok((false, None));
        }
        let (result, discovered, _) = invoke_attempt(
            &*self.adapter,
            token & !CONCURRENT,
            cause,
            request,
            observing,
        );
        let done = result != Err(AdapterError::Blocked);
        let evidence = observing.then(|| Attempt {
            result: super::super::super::events::owned_host_result(&result),
            discovery: discovered,
        });
        if done {
            self.adapter.finish(token & !CONCURRENT);
            self.rows
                .lock()
                .unwrap()
                .get_mut(&token)
                .unwrap()
                .completion = Some(result);
        }
        Ok((done, evidence))
    }

    pub(crate) fn task(&self, token: u64) -> u32 {
        self.rows
            .lock()
            .unwrap()
            .get(&token)
            .unwrap()
            .task
            .unwrap()
            .0
    }

    pub(crate) unsafe fn wait(&self, run: *mut RunShared) -> Stopped<bool> {
        if let (Some(driver), Some(fd)) = (&self.driver, unsafe { (*run).control.wake_fd() }) {
            driver.wake_on(fd).map_err(Stop::refusal)?;
        }
        let concurrent = self.adapter.wait_sources().map_err(Stop::refusal)?;
        let home = if unsafe { (*run).host_waits.load(Ordering::Acquire) } != 0 {
            unsafe { crate::host_waits::wait_sources_home(run) }?.ok_or_else(|| {
                Stop::Refused("mixed Ordered wait requires home source exposure".into())
            })?
        } else {
            crate::io::WaitSources::Drivers(Vec::new())
        };
        let sources = match (concurrent, home) {
            (crate::io::WaitSources::Ready, _) | (_, crate::io::WaitSources::Ready) => {
                crate::io::WaitSources::Ready
            }
            (crate::io::WaitSources::Drivers(mut a), crate::io::WaitSources::Drivers(b)) => {
                a.extend(b);
                crate::io::WaitSources::Drivers(a)
            }
        };
        let waited = sources.wait().map_err(Stop::refusal)?;
        if let Some(stop) = unsafe { (*run).control.stopped() } {
            return Err(stop);
        }
        Ok(waited)
    }

    pub(crate) fn completed(&self, token: u64) -> crate::host_waits::CompletedHost {
        let rows = self.rows.lock().unwrap();
        let row = rows.get(&token).unwrap();
        crate::host_waits::CompletedHost {
            task: row.task.unwrap().0,
            discovery: None,
            keys: None,
            result: super::super::super::events::owned_host_result(
                row.completion.as_ref().unwrap(),
            ),
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        for (&token, row) in self.rows.get_mut().unwrap().iter() {
            if row.completion.is_none() {
                self.adapter.finish(token & !CONCURRENT);
            }
        }
    }
}

pub(crate) unsafe fn pending(run: *mut RunShared) -> bool {
    unsafe { (*run).ordered_io.get() }.is_some_and(|s| s.pending.load(Ordering::Acquire) != 0)
}

pub(super) unsafe fn request(
    ctx: *mut TaskCtx,
    adapter: &dyn ConcurrentAdapter,
    request: Request,
) -> Stopped<Result<Completion, AdapterError>> {
    let io = unsafe { (*ctx).run_ref().ordered_io.get() }.ok_or_else(|| {
        Stop::Refused("Ordered concurrent wait requires run-owned admission".into())
    })?;
    if !std::ptr::addr_eq(adapter, &*io.adapter) {
        return Err(Stop::Refused(
            "Ordered concurrent wait adapter differs from admission".into(),
        ));
    }
    assert!(unsafe { (*ctx).pending_ordered_host.is_none() });
    unsafe {
        (*ctx).pending_ordered_host = Some(Box::new(Pending {
            request: Some(request),
            result: None,
        }));
    }
    let _owner = Owner(ctx);
    super::super::begin(ctx, false)?;
    let token = engine::driver(ctx).next_host_token;
    assert_eq!(token >> 48, 0);
    engine::driver(ctx).next_host_token = token.checked_add(1).unwrap();
    let observing = engine::driver(ctx).events.is_some();
    let request = unsafe {
        (*ctx)
            .pending_ordered_host
            .as_ref()
            .unwrap()
            .request
            .as_ref()
            .unwrap()
    };
    super::super::super::events::request_host(ctx, request);
    if observing {
        let (keys, domains) = {
            let _segment = unsafe { role::Segment::new(ctx) };
            (adapter.keys(request), adapter.domains(request))
        };
        super::super::super::events::request_domains(ctx, domains);
        super::super::super::events::request_keys(ctx, &keys);
        super::super::super::events::concurrent_phases(ctx);
    }
    let cause = unsafe { crate::tasks::canceled(ctx) };
    let (result, discovered, _) = {
        let _segment = unsafe { role::Segment::new(ctx) };
        invoke_attempt(adapter, token, cause, request.clone(), observing)
    };
    if result != Err(AdapterError::Blocked) || discovered.is_err() {
        let _segment = unsafe { role::Segment::new(ctx) };
        adapter.finish(token);
    }
    let discovery =
        discovered.and_then(|d| super::super::super::events::discover_host(ctx, &result, &d));
    super::super::super::events::record_host_initial(ctx, &result);
    if let Err(error) = discovery {
        if result != Err(AdapterError::Blocked) {
            super::super::super::events::complete_host(ctx, false, &result);
        }
        return Err(Stop::Refused(error.into()));
    }
    if result == Err(AdapterError::Blocked) {
        let wait = token | CONCURRENT;
        let request = unsafe {
            (*ctx)
                .pending_ordered_host
                .as_mut()
                .unwrap()
                .request
                .take()
                .unwrap()
        };
        let yielded = !matches!(
            request,
            Request::ReadStdin { .. } | Request::WatchNext { .. }
        );
        assert!(
            io.rows
                .lock()
                .unwrap()
                .insert(
                    wait,
                    Row {
                        request,
                        task: None,
                        completion: None
                    }
                )
                .is_none()
        );
        io.pending.fetch_add(1, Ordering::Release);
        unsafe { super::super::park(ctx, wait, yielded) }?;
        let result = io
            .rows
            .lock()
            .unwrap()
            .remove(&wait)
            .unwrap()
            .completion
            .expect("completed saved wait");
        io.pending.fetch_sub(1, Ordering::Release);
        super::super::super::events::complete_host(ctx, false, &result);
        Ok(result)
    } else {
        super::super::super::events::complete_host(ctx, false, &result);
        unsafe {
            (*ctx).pending_ordered_host.as_mut().unwrap().result = Some(result);
        }
        super::super::finish(ctx, Next::Yield)?;
        Ok(unsafe {
            (*ctx)
                .pending_ordered_host
                .as_mut()
                .unwrap()
                .result
                .take()
                .unwrap()
        })
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    #[test]
    fn ordered_concurrent_readiness_rejects_reused_rows_and_unrelated_wait_tokens() {
        let mut run = crate::host::BodyRun::new(1, false);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            |ctx| unsafe {
                let _guard = role::Guard::new(ctx);
                struct Adapter;
                impl ConcurrentAdapter for Adapter {
                    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
                        Ok(None)
                    }
                    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
                        unreachable!()
                    }
                }
                let io = Service {
                    adapter: Arc::new(Adapter),
                    driver: None,
                    rows: Default::default(),
                    pending: AtomicUsize::new(0),
                };
                let s = crate::tasks::scheduler(ctx);
                let generation = s.task(0).generation;
                let token = CONCURRENT | 3;
                io.rows.lock().unwrap().insert(
                    token,
                    Row {
                        request: Request::SleepUntil {
                            clock: 11,
                            deadline: 10,
                        },
                        task: Some((0, generation.wrapping_add(1))),
                        completion: None,
                    },
                );
                s.task(0).wait = Some(crate::tasks::Wait::Host { token });
                assert!(io.woken(s, &crate::io::Ready::Every).is_empty());
                assert!(!io.valid(token, 0, generation));
                io.rows.lock().unwrap().get_mut(&token).unwrap().task = Some((0, generation));
                assert_eq!(io.woken(s, &crate::io::Ready::Every), [0]);
                s.task(0).wait = Some(crate::tasks::Wait::Host { token: token + 1 });
                assert!(io.woken(s, &crate::io::Ready::Every).is_empty());
                s.task(0).wait = None;
                io.rows.lock().unwrap().clear();
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
    }
}
