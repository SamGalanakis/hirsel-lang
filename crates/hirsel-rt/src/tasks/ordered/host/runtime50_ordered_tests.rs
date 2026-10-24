//! Ordered retirement parks a blocking built-in call on an owned job row;
//! the job keeps its credit through a stop until the call actually returns.
use super::super::*;
use crate::io::{AdapterError, Completion, Request};
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Key, Kind};
use crate::ordered::Policy;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn until(mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while !done() {
        if start.elapsed() > Duration::from_secs(20) {
            return false;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    true
}

#[derive(Default)]
struct Disk {
    stop: bool,
    release: AtomicBool,
    on_job: AtomicU32,
    off_job: AtomicU32,
    finishes: AtomicU32,
    run: Mutex<Option<Arc<crate::stop::Control>>>,
    jobs: Mutex<Option<crate::tasks::free::host::jobs::Probe>>,
}

impl ConcurrentAdapter for Disk {
    fn blocking(&self, request: &Request) -> bool {
        matches!(request, Request::Open { .. })
    }
    fn identity(&self, handle: u64) -> Option<(Key, Kind)> {
        (handle == 1000).then_some((Key(300), Kind::Other))
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        let job = std::thread::current().name() == Some("hirsel-host-job");
        if job { &self.on_job } else { &self.off_job }.fetch_add(1, Ordering::AcqRel);
        assert!(matches!(request, Request::Open { .. }), "{request:?}");
        if self.stop {
            // Stop the run while this call still holds its job capacity.
            let control = self.run.lock().unwrap().clone().unwrap();
            control.request(crate::stop::Stop::Requested);
            assert!(until(|| self.release.load(Ordering::Acquire)));
        }
        Ok(Completion::Handle(Ok(1000)))
    }
    fn finish(&self, _: u64) {
        self.finishes.fetch_add(1, Ordering::AcqRel);
    }
}

unsafe extern "C" fn opener(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let disk = unsafe { &*crate::alloc::payload(env).cast::<*const Disk>().read() };
    let _role = unsafe { role::Guard::new(ctx) };
    let request = Request::Open {
        directory: 1,
        path: "data".into(),
    };
    let result = unsafe { crate::stop::check(ctx, concurrent::request(ctx, disk, request)) };
    assert_eq!(result, Ok(Completion::Handle(Ok(1000))));
    STATUS_OK
}

#[test]
fn runtime50_ordered_blocking_file_runs_as_owned_job() {
    for stop in [false, true] {
        let disk = Arc::new(Disk {
            stop,
            ..Default::default()
        });
        let seen = disk.clone();
        let layout = super::super::tests::layout();
        let layouts = [&layout as *const crate::Layout];
        let mut run = crate::host::BodyRun::new(2, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        run.concurrent_adapter = Some(disk.clone());
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            move |ctx| unsafe {
                *seen.run.lock().unwrap() = Some((*ctx).run_ref().control.clone());
                *seen.jobs.lock().unwrap() =
                    Some((*ctx).run_ref().ordered_io.get().unwrap().jobs_probe());
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<*const Disk>()
                    .write(Arc::as_ptr(&seen));
                let mut child = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        opener,
                        env,
                        0,
                    ),
                    STATUS_OK
                );
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                STATUS_OK
            },
        );
        let jobs = disk.jobs.lock().unwrap().clone().unwrap();
        let stats = jobs.stats();
        assert_eq!(disk.off_job.load(Ordering::Acquire), 0, "stop {stop}");
        assert_eq!(disk.on_job.load(Ordering::Acquire), 1, "stop {stop}");
        if stop {
            assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
            assert_eq!(jobs.held(), 1, "credit released before the call returned");
            disk.release.store(true, Ordering::Release);
            assert!(until(|| jobs.held() == 0));
            assert!(until(|| stats.reclaimed.load(Ordering::Acquire) == 1));
            assert_eq!(stats.quarantined.load(Ordering::Acquire), 1);
            assert_eq!(stats.delivered.load(Ordering::Acquire), 0);
        } else {
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(stats.delivered.load(Ordering::Acquire), 1);
            assert_eq!(jobs.held(), 0);
            assert_eq!(disk.finishes.load(Ordering::Acquire), 1);
        }
    }
}
