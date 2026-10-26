//! Owned wait rows follow Free's concurrent service registration and generation
//! checks. Ordered differs by retrying effects in the existing retirement poll,
//! before publishing any wake, rather than on the resumed request owner.
use super::*;
use crate::context::RunShared;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

pub(super) mod drive;

pub(crate) const CONCURRENT: u64 = 1 << 63;

pub(crate) struct Attempt {
    pub(crate) result: Result<Completion, AdapterError>,
    pub(crate) discovery: Discovered,
}

pub(crate) struct Service {
    pub(crate) adapter: Arc<dyn ConcurrentAdapter>,
    /// The adapter's driver, or one of the service's own when the adapter has
    /// none: a running job is an event source, and its publication and a stop
    /// must end the same blocking wait.
    driver: crate::readiness::Readiness,
    rows: Mutex<BTreeMap<u64, Row>>,
    pending: AtomicUsize,
    jobs: Jobs,
}

struct Row {
    request: Request,
    task: Option<(u32, u32)>,
    completion: Option<Result<Completion, AdapterError>>,
    written: Option<u64>,
    /// A blocking built-in call owned by a bounded job, as in Free (S5).
    job: Option<Owned>,
}

use crate::tasks::free::host::jobs::{Jobs, Owned};

impl Row {
    fn new(request: Request, job: Option<Owned>) -> Row {
        Row {
            request,
            task: None,
            completion: None,
            written: None,
            job,
        }
    }
    /// A job row is ready when its credit was granted or its call returned.
    fn job_ready(&mut self) -> Option<bool> {
        let job = self.job.as_mut()?;
        Some(match &job.slot {
            Some(slot) => slot.returned(),
            None => job.reservation.as_mut().is_some_and(|r| r.granted()),
        })
    }
}

impl Service {
    #[cfg(test)]
    pub(in crate::tasks::ordered::host) fn job_probe(
        &self,
    ) -> crate::tasks::free::host::jobs::Probe {
        self.jobs.probe()
    }

    /// Sole-root setup. No role is held while adapter callbacks run.
    pub(crate) unsafe fn install(
        ctx: *mut TaskCtx,
        adapter: Arc<dyn ConcurrentAdapter>,
    ) -> Stopped<()> {
        let run = unsafe { (*ctx).run_ref() };
        let driver = adapter.readiness().unwrap_or_default();
        driver.notify().map_err(Stop::refusal)?;
        if let Some(fd) = run.control.wake_fd() {
            driver.wake_on(fd).map_err(Stop::refusal)?;
        }
        let jobs = Jobs::default();
        let (ready, home) = (driver.clone(), unsafe {
            crate::pool::home_wake((*ctx).run_ptr())
        });
        jobs.set_waker(Arc::new(move || {
            let _ = ready.notify();
            if let Some(home) = &home {
                home();
            }
        }));
        assert!(
            run.ordered_io
                .set(Arc::new(Self {
                    adapter,
                    driver,
                    rows: Default::default(),
                    pending: AtomicUsize::new(0),
                    jobs,
                }))
                .is_ok()
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn jobs_probe(&self) -> crate::tasks::free::host::jobs::Probe {
        self.jobs.probe()
    }

    pub(crate) fn notify(&self) {
        self.driver
            .notify()
            .expect("admitted Ordered wake descriptor");
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

    /// `home` is what the home binding's poll found at this scheduling point,
    /// if it polled: the service then takes its share instead of polling again.
    pub(crate) fn ready(
        &self,
        home: Option<&crate::io::Ready>,
    ) -> Stopped<Vec<crate::host_waits::ReadyHost>> {
        let ready = match home {
            Some(home) => self.adapter.ready_after_home(home),
            None => self.adapter.ready(),
        }
        .map_err(Stop::refusal)?;
        // Job completions publish through rows; their tokens need no merge.
        self.jobs.drain();
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter_mut()
            .filter_map(|(&token, row)| {
                let job = row.job_ready();
                let (task, generation) = row.task?;
                let deadline = match row.request {
                    Request::SleepUntil { deadline, .. } => Some(deadline),
                    _ => None,
                };
                Some(crate::host_waits::ReadyHost {
                    generation,
                    task,
                    token,
                    deadline,
                    ready: job.unwrap_or_else(|| ready.contains(token & !CONCURRENT)),
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
        if let Some(polled) = self.poll_job(token, cause, observing) {
            return Ok(polled);
        }
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

    /// Submit a granted job, or complete a returned one. Discovery and the
    /// adapter's finish run here, on the poller, exactly as for a retry.
    fn poll_job(
        &self,
        token: u64,
        cause: Option<u32>,
        observing: bool,
    ) -> Option<(bool, Option<Attempt>)> {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.get_mut(&token)?;
        let job = row.job.as_mut()?;
        let Some(slot) = &job.slot else {
            if job.reservation.as_mut().is_some_and(|r| r.granted()) {
                let credit = job.reservation.take().and_then(|r| r.take()).unwrap();
                let (adapter, request) = (self.adapter.clone(), row.request.clone());
                let cause = cause.filter(|_| adapter.cancellable(token & !CONCURRENT));
                job.slot = Some(self.jobs.submit(credit, token, move || {
                    call_attempt(&*adapter, token & !CONCURRENT, cause, request)
                }));
            }
            return Some((false, None));
        };
        if !slot.returned() {
            return Some((false, None));
        }
        let (mut result, written) = job
            .slot
            .take()
            .unwrap()
            .deliver()
            .expect("one owner delivery");
        row.written = written;
        row.job = None;
        drop(rows);
        let discovered = if observing {
            discover(&mut result, &*self.adapter)
        } else {
            Ok(BTreeMap::new())
        };
        if result != Err(AdapterError::Blocked) {
            self.adapter.finish(token & !CONCURRENT);
        }
        let evidence = observing.then(|| Attempt {
            result: super::super::super::events::owned_host_result(&result),
            discovery: discovered,
        });
        self.rows
            .lock()
            .unwrap()
            .get_mut(&token)
            .unwrap()
            .completion = Some(result);
        Some((true, evidence))
    }

    /// Delivery of a physical job does not poll the adapter or advance its
    /// logical WaitLog. The issuer publishes the ordinary initial outcome.
    pub(crate) fn poll_head_job(&self, token: u64, cause: Option<u32>) -> bool {
        self.jobs.drain();
        self.poll_job(token, cause, false)
            .expect("owned head job")
            .0
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
        if let Some(fd) = unsafe { (*run).control.wake_fd() } {
            self.driver.wake_on(fd).map_err(Stop::refusal)?;
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
        let mut waited = sources.wait().map_err(Stop::refusal)?;
        // An in-flight job is an event source: with nothing else to wait on,
        // block until its publication or a stop notifies the driver, instead
        // of declaring the run quiet.
        if !waited && self.jobs.busy() {
            waited = self.driver.wait_woken().map_err(Stop::refusal)?;
        }
        if let Some(stop) = unsafe { (*run).control.stopped() } {
            return Err(stop);
        }
        Ok(waited)
    }

    pub(crate) fn retire_read(&self, token: u64, task: u32, generation: u32) {
        let row = self
            .rows
            .lock()
            .unwrap()
            .remove(&token)
            .expect("certified read row");
        assert_eq!(row.task, Some((task, generation)));
        assert!(row.completion.is_some());
        assert!(self.pending.fetch_sub(1, Ordering::Release) > 0);
    }

    pub(crate) fn completed_read(&self, token: u64) -> Option<super::super::CompletedRead> {
        let rows = self.rows.lock().unwrap();
        let row = rows.get(&token).expect("saved concurrent wait");
        super::super::CompletedRead::new(
            token,
            &row.request,
            row.completion.as_ref().expect("completed wait"),
        )
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
            // A running job still uses its token; quarantine owns its result.
            if row.completion.is_none() && row.job.as_ref().is_none_or(|job| job.slot.is_none()) {
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
    unsafe { drive::request(ctx, request) }
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
                    driver: Default::default(),
                    rows: Default::default(),
                    pending: AtomicUsize::new(0),
                    jobs: Default::default(),
                };
                let s = crate::tasks::scheduler(ctx);
                let generation = s.task(0).generation;
                let token = CONCURRENT | 3;
                io.rows.lock().unwrap().insert(
                    token,
                    Row {
                        task: Some((0, generation.wrapping_add(1))),
                        ..Row::new(
                            Request::SleepUntil {
                                clock: 11,
                                deadline: 10,
                            },
                            None,
                        )
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

// The service still owns request/job/wait custody; only its caller's physical
// suspension representation differs from the existing stackful request loop.
pub(super) unsafe fn start_entered(ctx: *mut TaskCtx, request: Request) {
    unsafe { drive::start(ctx, request) };
}
pub(super) unsafe fn drive_entered(
    ctx: *mut TaskCtx,
) -> Stopped<super::super::super::inop::Drive<Result<Completion, AdapterError>>> {
    unsafe { drive::drive(ctx) }
}
