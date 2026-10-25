//! An Ordered file job physically saves while retaining its logical head;
//! the job keeps its credit through a stop until the call actually returns.
use super::super::*;
use crate::io::{AdapterError, Completion, Request};
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Key, Kind};
use crate::ordered::Policy;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
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
    private: bool,
    refused: bool,
    cancel: bool,
    parked: AtomicU32,
    run_ptr: AtomicPtr<crate::context::RunShared>,
    computed: AtomicBool,
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
        if self.private {
            assert!(
                until(|| self.computed.load(Ordering::Acquire)),
                "a younger private computation cannot use the saved job's worker"
            );
        }
        if self.stop {
            // Stop the run while this call still holds its job capacity.
            let control = self.run.lock().unwrap().clone().unwrap();
            control.request(crate::stop::Stop::Requested);
            assert!(until(|| self.release.load(Ordering::Acquire)));
        }
        if self.cancel {
            return Err(AdapterError::Blocked);
        }
        Ok(Completion::Handle(if self.refused {
            Err(crate::io::IoError::PermissionDenied)
        } else {
            Ok(1000)
        }))
    }
    fn finish(&self, _: u64) {
        self.finishes.fetch_add(1, Ordering::AcqRel);
    }
    fn parked(&self, _: u64) {
        self.parked.fetch_add(1, Ordering::AcqRel);
        if self.cancel {
            let run = self.run_ptr.load(Ordering::Acquire);
            let _role = unsafe { role::IdleGuard::new(run) };
            unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() }.cancel(0, CANCEL_REQUESTED);
        }
    }
}

unsafe extern "C" fn opener(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let disk = unsafe { &*crate::alloc::payload(env).cast::<*const Disk>().read() };
    let mut scope = 0;
    if disk.private {
        // Let the parent publish the younger child before Open owns the head.
        unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0) };
    }
    let _role = unsafe { role::Guard::new(ctx) };
    let request = Request::Open {
        directory: 1,
        path: "data".into(),
    };
    let result = unsafe { crate::stop::check(ctx, concurrent::request(ctx, disk, request)) };
    assert_eq!(
        result,
        Ok(Completion::Handle(if disk.cancel {
            Err(crate::io::IoError::Canceled(CANCEL_PARENT))
        } else if disk.refused {
            Err(crate::io::IoError::PermissionDenied)
        } else {
            Ok(1000)
        }))
    );
    drop(_role);
    if disk.private {
        unsafe { hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0) };
    }
    STATUS_OK
}

#[test]
fn runtime50_ordered_blocking_file_runs_as_owned_job() {
    for (stop, refused, cancel) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let disk = Arc::new(Disk {
            stop,
            refused,
            cancel,
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
                seen.run_ptr.store((*ctx).run_ptr(), Ordering::Release);
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
        assert_eq!(
            disk.on_job.load(Ordering::Acquire),
            1,
            "stop {stop}: {outcome:?}"
        );
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
            assert_eq!(disk.parked.load(Ordering::Acquire), u32::from(cancel));
        }
    }
}

unsafe extern "C" fn private_compute(_: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let disk = unsafe { &*crate::alloc::payload(env).cast::<*const Disk>().read() };
    assert!(until(|| disk.on_job.load(Ordering::Acquire) == 1));
    let mut value = 7u64;
    for i in 0..1000 {
        value = value.wrapping_mul(13).wrapping_add(i);
    }
    assert_ne!(std::hint::black_box(value), 0);
    disk.computed.store(true, Ordering::Release);
    STATUS_OK
}

#[test]
fn ordered_file_job_keeps_head_while_younger_private_compute_progresses() {
    let disk = Arc::new(Disk {
        private: true,
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
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
            for code in [
                opener as crate::tasks::TaskCode,
                private_compute as crate::tasks::TaskCode,
            ] {
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<*const Disk>()
                    .write(Arc::as_ptr(&seen));
                let mut child = 0;
                // Publish both logical children before the private body waits
                // for the older job. A speculative reservation would require
                // the younger frame to save before its spawn could retire.
                hirsel_rt_v2_task_spawn(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut child,
                    &scope,
                    code,
                    env,
                    0,
                );
            }
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(STATUS_OK));
    assert!(disk.computed.load(Ordering::Acquire));
    assert_eq!(disk.on_job.load(Ordering::Acquire), 1);
    assert_eq!(disk.off_job.load(Ordering::Acquire), 0);
    assert_eq!(disk.finishes.load(Ordering::Acquire), 1);
}
