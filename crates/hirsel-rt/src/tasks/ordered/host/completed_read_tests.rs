use super::*;
use crate::io::{Adapter, AdapterError, Completion, Ready, Request};
use crate::io_concurrent::ConcurrentAdapter;
use crate::ordered::Policy;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Probe {
    run: AtomicPtr<crate::context::RunShared>,
    ready: AtomicBool,
    parked: AtomicBool,
    computed: AtomicBool,
    attempts: AtomicUsize,
    finishes: AtomicUsize,
    concurrent: bool,
    value: u8,
    post_read: bool,
}
impl Probe {
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        match request {
            Request::NetRead { .. } => {
                self.attempts.fetch_add(1, Ordering::Relaxed);
                if !self.ready.load(Ordering::Acquire) {
                    return Err(AdapterError::Blocked);
                }
                Ok(Completion::NetBytes(match self.value {
                    0 => Ok(vec![71]),
                    1 => Ok(vec![]),
                    _ => Err(crate::net::NetError::TimedOut),
                }))
            }
            _ => panic!("unexpected {request:?}"),
        }
    }
}
struct Home(Arc<Probe>, std::thread::ThreadId);
impl Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().id(), self.1);
        self.0.call(request)
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(if self.0.ready.load(Ordering::Acquire) {
            Ready::Every
        } else {
            Ready::Tokens(vec![])
        })
    }
    fn parked(&mut self, _: u64) {
        self.0.parked.store(true, Ordering::Release);
    }
    fn finish(&mut self, _: u64) {
        self.0.finishes.fetch_add(1, Ordering::Relaxed);
    }
}
impl ConcurrentAdapter for Probe {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        Probe::call(self, request)
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        Ok(if self.ready.load(Ordering::Acquire) {
            Ready::Every
        } else {
            Ready::Tokens(vec![])
        })
    }
    fn parked(&self, _: u64) {
        self.parked.store(true, Ordering::Release);
    }
    fn finish(&self, _: u64) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "C" fn reader(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let probe = unsafe { &*crate::alloc::payload(env).cast::<*const Probe>().read() };
    unsafe {
        (*ctx).test_host_tail = probe.post_read;
    }
    let request = Request::NetRead {
        net: 11,
        stream: 12,
        max: 1,
    };
    let role = unsafe { role::Guard::new(ctx) };
    let result = if probe.concurrent {
        let _role = unsafe { role::Guard::new(ctx) };
        unsafe { crate::stop::check(ctx, concurrent::request(ctx, probe, request)) }.unwrap()
    } else {
        unsafe { crate::io::capability_call(ctx, request) }.unwrap()
    };
    drop(role);
    let expected = Completion::NetBytes(match probe.value {
        0 => Ok(vec![71]),
        1 => Ok(vec![]),
        _ => Err(crate::net::NetError::TimedOut),
    });
    assert_eq!(result, expected);
    // Scalar-only useful work. This flag is test instrumentation, not a
    // language operation; the next actual operation must still wait for head.
    let mut x = 7u64;
    for i in 0..1000 {
        x = x.wrapping_mul(13).wrapping_add(i);
    }
    assert_ne!(std::hint::black_box(x), 0);
    probe.computed.store(true, Ordering::Release);
    STATUS_OK
}

// Its check wakes the reader while the already queued parent precedes that
// wake. It then saves at the existing strict gate, releasing compute capacity.
unsafe extern "C" fn poller(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let probe = unsafe { &*crate::alloc::payload(env).cast::<*const Probe>().read() };
    probe.ready.store(true, Ordering::Release);
    let mut cause = 0;
    assert_eq!(
        unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
        STATUS_OK
    );
    STATUS_OK
}

fn witness(concurrent: bool, value: u8, stop: bool, cancel: bool, bounded: bool, certified: bool) {
    witness_with_authority(
        concurrent, value, stop, cancel, bounded, certified, certified,
    );
}

fn witness_with_authority(
    concurrent: bool,
    value: u8,
    stop: bool,
    cancel: bool,
    bounded: bool,
    certified: bool,
    post_read: bool,
) {
    let probe = Arc::new(Probe {
        concurrent,
        value,
        post_read,
        ready: AtomicBool::new(bounded),
        ..Default::default()
    });
    let setup = probe.clone();
    let seen = probe.clone();
    let layout = super::tests::layout();
    let layouts = [&layout as *const crate::Layout];
    let mut run = crate::host::BodyRun::new(2, true);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    if concurrent {
        run.concurrent_adapter = Some(probe.clone());
    } else {
        run.adapter = Some(Box::new(Home(probe.clone(), std::thread::current().id())));
    }
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        move |ctx| unsafe {
            engine::enable(ctx, Policy::admit(&Default::default(), None));
            setup.run.store((*ctx).run_ptr(), Ordering::Release);
        },
        move |ctx| unsafe {
            let mut scope = 0;
            assert_eq!(
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                STATUS_OK
            );
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<*const Probe>()
                .write(Arc::as_ptr(&seen));
            let mut child = 0;
            // This native-only alternate entry has an explicitly finite
            // scalar tail (1000 iterations). It is not a compiler proof.
            let info = super::super::ahead::SpawnAheadInfo {
                version: 1,
                flags: 0,
                entry: Some(reader),
            };
            let status = if certified {
                super::super::ahead::hirsel_rt_v2_task_spawn_ahead(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut child,
                    &scope,
                    reader,
                    env,
                    0,
                    &info,
                )
            } else {
                hirsel_rt_v2_task_spawn(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut child,
                    &scope,
                    reader,
                    env,
                    0,
                )
            };
            assert_eq!(status, STATUS_OK);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            if bounded {
                let _role = role::Guard::new(ctx);
                engine::at_head(ctx).unwrap();
            } else {
                loop {
                    let saved = {
                        let _role = role::Guard::new(ctx);
                        let s = scheduler(ctx);
                        matches!(s.task(child as u32).wait, Some(Wait::Host { .. }))
                            && readiness::saved(s, child as u32)
                    };
                    if saved && seen.parked.load(Ordering::Acquire) {
                        break;
                    }
                    assert!(std::time::Instant::now() < until, "read never parked");
                    let mut cause = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_canceled(ctx, &mut cause, &scope),
                        STATUS_OK
                    );
                }
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<*const Probe>()
                    .write(Arc::as_ptr(&seen));
                let mut polling = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut polling,
                        &scope,
                        poller,
                        env,
                        0
                    ),
                    STATUS_OK
                );
                let _role = role::Guard::new(ctx);
                engine::at_head(ctx).unwrap();
            }
            // Root was already queued before the poller wakes the reader.
            if post_read {
                while !seen.computed.load(Ordering::Acquire) {
                    assert!(
                        std::time::Instant::now() < until,
                        "completed read retained strict return gate"
                    );
                    std::thread::yield_now();
                }
            } else {
                loop {
                    let _role = role::Guard::new(ctx);
                    if crate::tasks::ordered::engine::with_driver(ctx, |d| d.head_paused(child)) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < until,
                        "legacy read never saved at strict gate"
                    );
                    drop(_role);
                    std::thread::yield_now();
                }
                assert!(!seen.computed.load(Ordering::Acquire));
            }
            {
                let _role = role::Guard::new(ctx);
                let me = engine::current(ctx);
                assert!(crate::tasks::ordered::engine::with_driver(ctx, |d| d.head_ready(me)));
                assert_eq!(
                    crate::tasks::ordered::engine::with_driver(ctx, |d| d
                        .host_returns
                        .contains_key(&child)),
                    !bounded && post_read
                );
                assert_eq!(
                    crate::tasks::ordered::engine::with_driver(ctx, |d| d
                        .host_ops
                        .contains_key(&child)),
                    !bounded
                );
                if !bounded {
                    if concurrent {
                        assert!(
                            concurrent::pending((*ctx).run_ptr()),
                            "canonical wait still owns pending accounting"
                        );
                    } else {
                        assert_eq!(
                            (*ctx).run_ref().host_waits.load(Ordering::Acquire),
                            1,
                            "canonical wait still owns home readiness accounting"
                        );
                    }
                }
                if cancel {
                    crate::tasks::role::with_role(ctx, |s| {
                        s.cancel(child as u32, CANCEL_REQUESTED)
                    });
                }
                if stop {
                    (*ctx)
                        .run_ref()
                        .control
                        .request_inside(crate::stop::Stop::Requested);
                }
            }
            assert_eq!(
                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                STATUS_OK
            );
            assert!(crate::tasks::ordered::engine::with_driver(ctx, |d| d
                .host_returns
                .is_empty()));
            STATUS_OK
        },
    );
    assert_eq!(
        outcome.ended,
        if stop {
            Err(crate::stop::Stop::Requested)
        } else {
            Ok(STATUS_OK)
        }
    );
    assert_eq!(
        probe.attempts.load(Ordering::Acquire),
        if bounded { 1 } else { 2 }
    );
    assert_eq!(probe.finishes.load(Ordering::Acquire), 1);
    assert!(probe.computed.load(Ordering::Acquire));
}

#[test]
fn completed_reads_release_pure_tails_in_both_routes() {
    for concurrent in [false, true] {
        witness(concurrent, 0, false, false, false, true);
    }
}
#[test]
fn completed_short_and_failed_reads_keep_the_selected_outcome() {
    for concurrent in [false, true] {
        for value in [1, 2] {
            witness(concurrent, value, false, false, false, true);
        }
    }
}
#[test]
fn cancel_after_completion_preserves_the_return_before_canonical_finish() {
    for concurrent in [false, true] {
        witness(concurrent, 0, false, true, false, true);
    }
}
#[test]
fn stop_after_completion_withdraws_the_unfinished_return() {
    for concurrent in [false, true] {
        witness(concurrent, 0, true, false, false, true);
    }
}
#[test]
fn a_certificate_requires_a_completed_owned_read_with_a_valid_extent() {
    let read = Request::NetRead {
        net: 11,
        stream: 12,
        max: 1,
    };
    assert!(CompletedRead::new(7, &read, &Err(AdapterError::Blocked)).is_none());
    assert!(CompletedRead::new(7, &read, &Ok(Completion::NetBytes(Ok(vec![1, 2])))).is_none());
    assert!(
        CompletedRead::new(
            7,
            &Request::SleepUntil {
                clock: 11,
                deadline: 1
            },
            &Ok(Completion::Wait(Ok(())))
        )
        .is_none()
    );
    assert!(CompletedRead::new(7, &read, &Ok(Completion::NetBytes(Ok(vec![])))).is_some());
}

#[test]
fn bounded_completed_reads_release_pure_tails_in_both_routes() {
    for concurrent in [false, true] {
        witness(concurrent, 0, false, false, true, true);
    }
}

#[test]
fn legacy_read_entries_keep_the_strict_return_gate() {
    for concurrent in [false, true] {
        witness(concurrent, 0, false, false, false, false);
    }
}

#[test]
fn scalar_spawn_metadata_does_not_authorize_a_completed_host_tail() {
    for concurrent in [false, true] {
        witness_with_authority(concurrent, 0, false, false, false, true, false);
    }
}
