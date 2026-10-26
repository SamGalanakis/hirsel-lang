//! Tokio fs_file_join_error.rs:21-68 repeated-failure shape, using reported
//! adapter failure rather than Tokio's panic/canceled-job policy.
use super::super::*;
use crate::io::UnitResult;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct FailedJobs {
    tokens: Mutex<Vec<u64>>,
    finished: AtomicUsize,
}
impl ConcurrentAdapter for FailedJobs {
    fn blocking(&self, _: &Request) -> bool {
        true
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().name(), Some("hirsel-host-job"));
        self.tokens.lock().unwrap().push(token);
        match request {
            Request::SleepUntil { clock: 99, .. } => Err(AdapterError::InvalidHandle(99)),
            Request::SleepUntil { clock: 11, .. } => Ok(Completion::Wait(Ok(()))),
            _ => unreachable!(),
        }
    }
    fn finish(&self, _: u64) {
        self.finished.fetch_add(1, Ordering::Release);
    }
}

#[test]
fn failed_real_jobs_convert_errors_without_poisoning_repeated_or_independent_operations() {
    for width in [1, 4] {
        let adapter = Arc::new(FailedJobs::default());
        let mut run = crate::host::BodyRun::new(width, true);
        run.concurrent_adapter = Some(adapter.clone());
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                )
            },
            |ctx| unsafe {
                for (clock, expected) in [
                    (99, crate::STATUS_FAILURE),
                    (99, crate::STATUS_FAILURE),
                    (11, crate::STATUS_OK),
                ] {
                    let mut output = UnitResult {
                        tag: 99,
                        error: 99,
                        code: 99,
                    };
                    assert_eq!(
                        crate::io::hirsel_rt_v2_clock_sleep_until(
                            ctx,
                            std::ptr::null_mut(),
                            &mut output,
                            &clock,
                            &10
                        ),
                        expected
                    );
                    assert_eq!(
                        output.tag,
                        if expected == crate::STATUS_OK { 0 } else { 99 }
                    );
                    let _role = role::Guard::new(ctx);
                    assert!((*ctx).pending_ordered_host.is_none());
                    assert!(engine::driver(ctx).in_operations.is_empty());
                    assert!(engine::driver(ctx).host_ops.is_empty());
                    assert!(engine::driver(ctx).head_job.is_none());
                    assert!(!super::super::super::pending((*ctx).run_ptr()));
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(*adapter.tokens.lock().unwrap(), [1, 2, 3]);
        assert_eq!(adapter.finished.load(Ordering::Acquire), 3);
    }
}

#[derive(Default)]
struct StopAfterInitial {
    run: std::sync::atomic::AtomicPtr<crate::context::RunShared>,
}
impl ConcurrentAdapter for StopAfterInitial {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn keys(&self, request: &Request) -> crate::io_keys::KeySet {
        use crate::io_keys::{Key, KeySet, Mode, ROOT};
        KeySet::new(vec![match request {
            Request::MonotonicClock { .. } => (ROOT, Mode::Shared),
            Request::SleepUntil { .. } => (Key(11), Mode::Exclusive),
            _ => unreachable!(),
        }])
    }
    fn identity(&self, word: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        (word == 11).then_some((crate::io_keys::Key(11), crate::io_keys::Kind::Other))
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, _: u64, request: Request) -> Result<Completion, AdapterError> {
        Ok(match request {
            Request::MonotonicClock { .. } => Completion::ClockGrant(Ok(11)),
            Request::SleepUntil { .. } => Completion::Wait(Ok(())),
            _ => unreachable!(),
        })
    }
    fn finish(&self, token: u64) {
        if token == 2 {
            unsafe {
                (*self.run.load(Ordering::Acquire))
                    .control
                    .request_inside(Stop::Requested)
            };
        }
    }
}

#[test]
fn an_initial_effect_keeps_its_selected_evidence_when_finish_requests_a_terminal_cut() {
    for width in [1, 4] {
        let adapter = Arc::new(StopAfterInitial::default());
        let setup = adapter.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.concurrent_adapter = Some(adapter);
        unsafe { run.use_physical_dispatch() };
        let (outcome, trace) = run.run_traced_after_bounded_stop(
            move |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
                crate::tasks::ordered::events::enable(ctx);
                crate::tasks::ordered::events::initial_host(ctx, Some(1), None);
                setup.run.store((*ctx).run_ptr(), Ordering::Release);
            },
            |ctx| unsafe {
                assert_eq!(
                    crate::io::capability_call(ctx, Request::MonotonicClock { io: 1 }),
                    Ok(Completion::ClockGrant(Ok(11)))
                );
                let _ = crate::io::capability_call(
                    ctx,
                    Request::SleepUntil {
                        clock: 11,
                        deadline: 10,
                    },
                );
                panic!("a selected initial effect returned after the terminal cut")
            },
        );
        assert_eq!(outcome.ended, Err(Stop::Requested));
        let trace = trace.expect("selected initial outcome retained before stop observation");
        let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            rows.iter()
                .filter(|row| row["outcome"]["kind"] == "host_result")
                .count(),
            2
        );
    }
}

struct CutBeforeAttempt {
    run: std::sync::atomic::AtomicPtr<crate::context::RunShared>,
    blocking: bool,
    calls: AtomicUsize,
    classifications: AtomicUsize,
    at_classification: bool,
}
impl ConcurrentAdapter for CutBeforeAttempt {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn blocking(&self, _: &Request) -> bool {
        self.classifications.fetch_add(1, Ordering::Relaxed);
        if self.at_classification {
            unsafe {
                (*self.run.load(Ordering::Acquire))
                    .control
                    .request_inside(Stop::Requested)
            };
        }
        self.blocking
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Completion::Wait(Ok(())))
    }
}

unsafe extern "C" fn cut_child(
    ctx: *mut TaskCtx,
    env: *mut crate::Header,
    caller: u64,
    site: u64,
) -> u8 {
    unsafe {
        crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, cut_dispatch, 48)
    }
}

unsafe extern "C" fn cut_dispatch(
    ctx: *mut TaskCtx,
    _: *mut crate::Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    let words = frame.cast::<u64>();
    unsafe {
        words.write(1);
        words.add(1).write(11);
        words.add(2).write(10);
        words.add(3).cast::<UnitResult>().write(UnitResult {
            tag: 99,
            error: 99,
            code: 99,
        });
        crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 33, 48, 0, 0);
        crate::tasks::ordered::host::entered::hirsel_rt_v2_ordered_sleep_until(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            words.add(3).cast(),
            words.add(1),
            words.add(2),
        )
    }
}

#[test]
fn entered_cut_during_classification_prevents_both_inline_and_job_attempts() {
    use crate::tasks::*;
    for width in [1, 4] {
        for blocking in [false, true] {
            let layout = crate::tasks::ordered::host::tests::layout();
            let layouts = [&layout as *const crate::Layout];
            let adapter = Arc::new(CutBeforeAttempt {
                run: Default::default(),
                blocking,
                calls: AtomicUsize::new(0),
                classifications: AtomicUsize::new(0),
                at_classification: true,
            });
            let setup = adapter.clone();
            let jobs = Arc::new(Mutex::new(None));
            let probe = jobs.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            run.concurrent_adapter = Some(adapter.clone());
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                    setup.run.store((*ctx).run_ptr(), Ordering::Release);
                },
                move |ctx| unsafe {
                    *probe.lock().unwrap() =
                        Some((*ctx).run_ref().ordered_io.get().unwrap().job_probe());
                    let mut scope = 0;
                    let mut child = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        crate::STATUS_OK
                    );
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        16,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut child,
                            &scope,
                            cut_child,
                            env,
                            0
                        ),
                        crate::STATUS_OK
                    );
                    let mut result = [0u64; 2];
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child,
                        result.as_mut_ptr().cast(),
                        16,
                    );
                    panic!("entered cut returned to its source suffix")
                },
            );
            assert_eq!(outcome.ended, Err(Stop::Requested));
            assert_eq!(adapter.classifications.load(Ordering::Relaxed), 1);
            assert_eq!(
                adapter.calls.load(Ordering::Relaxed),
                0,
                "{width}/{blocking}"
            );
            let jobs = jobs.lock().unwrap();
            let jobs = jobs.as_ref().unwrap();
            assert_eq!(jobs.stats().submitted.load(Ordering::Acquire), 0);
            assert_eq!(jobs.held(), 0);
        }
    }
}

#[test]
fn entered_cut_with_a_newly_granted_unsubmitted_job_withdraws_its_credit() {
    use crate::tasks::*;
    for width in [1, 4] {
        let layout = crate::tasks::ordered::host::tests::layout();
        let layouts = [&layout as *const crate::Layout];
        let adapter = Arc::new(CutBeforeAttempt {
            run: Default::default(),
            blocking: true,
            calls: AtomicUsize::new(0),
            classifications: AtomicUsize::new(0),
            at_classification: false,
        });
        let setup = adapter.clone();
        let jobs = Arc::new(Mutex::new(None));
        let probe = jobs.clone();
        let held = Arc::new(Mutex::new(Vec::new()));
        let release = held.clone();
        let saved = Arc::new(AtomicUsize::new(0));
        let observe = saved.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        run.concurrent_adapter = Some(adapter.clone());
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
                setup.run.store((*ctx).run_ptr(), Ordering::Release);
                let address = (*ctx).run_ptr() as usize;
                engine::driver(ctx).after_save = Some(Box::new(move |s, task, completed| {
                    let Logical::Ordered(d) = &s.logical else {
                        unreachable!()
                    };
                    let row = s.tasks[task as usize].as_ref().unwrap();
                    let id = handle(task, row.generation);
                    let Some((owner, token)) = d.head_job else {
                        return;
                    };
                    if completed || owner != id {
                        return;
                    }
                    assert!(row.context_home.is_some());
                    assert!(row.stack.is_none());
                    assert_eq!(observe.fetch_add(1, Ordering::Relaxed), 0);
                    let run = &*(address as *const crate::context::RunShared);
                    let io = run.ordered_io.get().unwrap();
                    assert_eq!(io.jobs.stats().submitted.load(Ordering::Acquire), 0);
                    // A real reservation existed while the entered owner saved.
                    // Release capacity, then force the same home-poll entry after
                    // the terminal cut, before any submission can be accepted.
                    release.lock().unwrap().clear();
                    assert!(
                        io.rows
                            .lock()
                            .unwrap()
                            .get_mut(&token)
                            .unwrap()
                            .job
                            .as_mut()
                            .unwrap()
                            .reservation
                            .as_mut()
                            .unwrap()
                            .granted()
                    );
                    run.control.request_inside(Stop::Requested);
                    assert!(!io.poll_head_job(token, None));
                    assert_eq!(io.jobs.stats().submitted.load(Ordering::Acquire), 0);
                    assert_eq!(
                        io.jobs.held(),
                        1,
                        "unsubmitted credit remains owned until teardown"
                    );
                }));
            },
            move |ctx| unsafe {
                let io = (*ctx).run_ref().ordered_io.get().unwrap();
                *probe.lock().unwrap() = Some(io.job_probe());
                *held.lock().unwrap() = (0..crate::tasks::free::host::jobs::LIMIT)
                    .map(|i| io.jobs.reserve(100 + i as u64))
                    .collect();
                let mut scope = 0;
                let mut child = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    crate::STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        cut_child,
                        env,
                        0
                    ),
                    crate::STATUS_OK
                );
                let mut result = [0u64; 2];
                hirsel_rt_v2_task_join(
                    ctx,
                    0,
                    0,
                    &scope,
                    &mut child,
                    result.as_mut_ptr().cast(),
                    16,
                );
                panic!("a cut job returned to its source suffix")
            },
        );
        assert_eq!(outcome.ended, Err(Stop::Requested));
        assert_eq!(saved.load(Ordering::Relaxed), 1);
        assert_eq!(adapter.calls.load(Ordering::Relaxed), 0);
        let jobs = jobs.lock().unwrap();
        let jobs = jobs.as_ref().unwrap();
        assert_eq!(jobs.stats().submitted.load(Ordering::Acquire), 0);
        assert_eq!(jobs.stats().queued_credits.load(Ordering::Acquire), 1);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while jobs.held() != 0 {
            assert!(
                std::time::Instant::now() < until,
                "unsubmitted reservation was not withdrawn"
            );
            std::thread::yield_now();
        }
        assert_eq!(jobs.held(), 0);
    }
}
