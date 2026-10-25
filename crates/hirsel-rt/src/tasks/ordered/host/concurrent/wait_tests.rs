use super::*;
use crate::io::{Ready, WaitError, WaitSources};
use crate::io_keys::{Key, KeySet, Kind, Mode};
use crate::ordered::Policy;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicPtr, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Host {
    run: AtomicPtr<crate::context::RunShared>,
    attempts: Mutex<BTreeMap<u64, usize>>,
    finished: Mutex<Vec<u64>>,
    parked: AtomicUsize,
    cancel: bool,
    never_ready: bool,
    stop_barrier: Option<Arc<std::sync::Barrier>>,
    stop_completed: bool,
    driver: crate::readiness::Readiness,
}
impl Host {
    fn role(&self) -> role::IdleGuard {
        unsafe { role::IdleGuard::new(self.run.load(Ordering::Acquire)) }
    }
}
impl ConcurrentAdapter for Host {
    fn keys(&self, request: &Request) -> KeySet {
        let _role = self.role();
        match request {
            Request::SleepUntil { clock, .. } | Request::ReadStdin { stdin: clock, .. } => {
                KeySet::new(vec![(Key(*clock as u32), Mode::Exclusive)])
            }
            _ => KeySet::coupled(),
        }
    }
    fn identity(&self, word: u64) -> Option<(Key, Kind)> {
        let _role = self.role();
        Some((Key(word as u32), Kind::Other))
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        let _role = self.role();
        match request {
            Request::SleepUntil { clock: 11, .. } | Request::ReadStdin { stdin: 12, .. } => {
                Ok(None)
            }
            _ => Err(AdapterError::InvalidHandle(99)),
        }
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        let _role = self.role();
        match request {
            Request::MonotonicClock { .. } => Ok(Completion::ClockGrant(Ok(11))),
            Request::Stdin { .. } => Ok(Completion::Handle(Ok(12))),
            Request::SleepUntil { .. } | Request::ReadStdin { .. } => {
                let mut attempts = self.attempts.lock().unwrap();
                let count = attempts.entry(token).or_default();
                *count += 1;
                if *count < 3 || self.never_ready {
                    if self.never_ready {
                        self.driver.enter(token);
                        self.driver.pending().deadline =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
                    }
                    return Err(AdapterError::Blocked);
                }
                Ok(match request {
                    Request::SleepUntil { .. } => Completion::Wait(Ok(())),
                    _ => Completion::Input(Ok(vec![0, 255, b'x'])),
                })
            }
            _ => panic!("unexpected {request:?}"),
        }
    }
    fn parked(&self, token: u64) {
        let _role = self.role();
        self.parked.fetch_add(1, Ordering::Release);
        if self.never_ready {
            self.driver.parked(token);
        }
        if self.cancel {
            let run = self.run.load(Ordering::Acquire);
            unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() }
                .cancel(0, crate::tasks::CANCEL_REQUESTED);
        }
    }
    fn finish(&self, token: u64) {
        let _role = self.role();
        self.finished.lock().unwrap().push(token);
        self.driver.finish(token);
        if self.stop_completed && token == 2 {
            unsafe {
                (*self.run.load(Ordering::Acquire))
                    .control
                    .request_inside(Stop::Requested)
            };
        }
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        let _role = self.role();
        Ok(if self.never_ready || self.cancel {
            Ready::Tokens(vec![])
        } else {
            Ready::Every
        })
    }
    fn readiness(&self) -> Option<crate::readiness::Readiness> {
        Some(self.driver.clone())
    }
    fn wait_sources(&self) -> Result<WaitSources, AdapterError> {
        {
            let _role = self.role();
        }
        if let Some(barrier) = &self.stop_barrier {
            // Home reached the source wait after the owner physically saved.
            // No role or adapter lock is held while the external host stops it.
            barrier.wait();
            barrier.wait();
        }
        Ok(if self.never_ready {
            WaitSources::Drivers(vec![self.driver.clone()])
        } else {
            WaitSources::Ready
        })
    }
}

fn init(ctx: *mut TaskCtx, host: &Host, reorder: hirsel_run_config::Reorder, trace: bool) {
    unsafe {
        let mut policy = Policy::admit(&Default::default(), None);
        policy.reorder = reorder;
        engine::enable(ctx, policy);
        if trace {
            super::super::super::events::enable(ctx);
            super::super::super::events::initial_host(ctx, Some(1), None);
        }
        host.run.store((*ctx).run_ptr(), Ordering::Release);
    }
}

#[test]
fn ordered_concurrent_saved_waits_keep_spurious_attempts_owned_bytes_and_canonical_ordinals() {
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let host = Arc::new(Host::default());
            let setup = host.clone();
            let calls = host.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.concurrent_adapter = Some(host.clone());
            unsafe { run.use_physical_dispatch() };
            let (outcome, trace) = run.run_traced(
                move |ctx| init(ctx, &setup, reorder, true),
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    assert_eq!(
                        request(ctx, &*calls, Request::MonotonicClock { io: 1 }).unwrap(),
                        Ok(Completion::ClockGrant(Ok(11)))
                    );
                    assert_eq!(
                        request(ctx, &*calls, Request::Stdin { io: 1 }).unwrap(),
                        Ok(Completion::Handle(Ok(12)))
                    );
                    for (request_value, expected) in [
                        (
                            Request::SleepUntil {
                                clock: 11,
                                deadline: 10,
                            },
                            Completion::Wait(Ok(())),
                        ),
                        (
                            Request::ReadStdin { stdin: 12, max: 3 },
                            Completion::Input(Ok(vec![0, 255, b'x'])),
                        ),
                    ] {
                        assert_eq!(request(ctx, &*calls, request_value).unwrap(), Ok(expected));
                    }
                    assert!((*ctx).pending_ordered_host.is_none());
                    assert!(!pending((*ctx).run_ptr()));
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(log.host_attempts.len(), 4);
                    assert_eq!(log.host_polls.len(), 2);
                    assert_eq!(log.host_completions.len(), 4);
                    for (i, ordinal) in [2, 3].into_iter().enumerate() {
                        assert_eq!(
                            log.host_initials[&(vec![], ordinal)].result,
                            Err(AdapterError::Blocked)
                        );
                        assert_eq!(
                            log.host_attempts[2 * i].completion.result,
                            Err(AdapterError::Blocked)
                        );
                        assert_eq!(log.host_attempts[2 * i + 1].completion.ordinal, ordinal);
                        assert_eq!(
                            log.host_attempts[2 * i + 1].completion,
                            log.host_polls[i].completion
                        );
                        assert_eq!(log.host_polls[i].completion, log.host_completions[i + 2]);
                    }
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(*host.finished.lock().unwrap(), [1, 2, 3, 4]);
            assert_eq!(host.parked.load(Ordering::Acquire), 2);
            let bytes = trace
                .expect("complete concurrent parked phases")
                .bytes()
                .to_vec();
            if let Some(reference) = &reference {
                assert_eq!(&bytes, reference);
            } else {
                reference = Some(bytes);
            }
        }
    }
}

#[test]
fn ordered_concurrent_saved_cancellation_validates_authority_without_retrying_the_effect() {
    for width in [1, 8] {
        let host = Arc::new(Host {
            cancel: true,
            ..Default::default()
        });
        let setup = host.clone();
        let calls = host.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.concurrent_adapter = Some(host.clone());
        unsafe { run.use_physical_dispatch() };
        let (outcome, trace) = run.run_traced(
            move |ctx| init(ctx, &setup, hirsel_run_config::Reorder::Auto, true),
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                request(ctx, &*calls, Request::MonotonicClock { io: 1 })
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    request(
                        ctx,
                        &*calls,
                        Request::SleepUntil {
                            clock: 11,
                            deadline: 10
                        }
                    )
                    .unwrap(),
                    Ok(Completion::Wait(Err(WaitError::Canceled(
                        crate::tasks::CANCEL_REQUESTED
                    ))))
                );
                assert_eq!(
                    request(
                        ctx,
                        &*calls,
                        Request::SleepUntil {
                            clock: 99,
                            deadline: 10
                        }
                    )
                    .unwrap(),
                    Err(AdapterError::InvalidHandle(99))
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(host.attempts.lock().unwrap().values().sum::<usize>(), 1);
        assert_eq!(*host.finished.lock().unwrap(), [1, 2, 3]);
        // The unknown authority preserves the live refusal and cannot certify a trace.
        assert!(trace.is_err());
    }
}

#[test]
fn ordered_concurrent_external_stop_wakes_the_driver_and_reclaims_the_saved_request_once() {
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let host = Arc::new(Host {
        never_ready: true,
        stop_barrier: Some(barrier.clone()),
        ..Default::default()
    });
    let setup = host.clone();
    let calls = host.clone();
    let mut run = crate::host::BodyRun::new(2, true);
    run.concurrent_adapter = Some(host.clone());
    unsafe { run.use_physical_dispatch() };
    let control = run.control();
    let stopper = std::thread::spawn(move || {
        // Registration precedes physical save. Stop only once the home wait
        // owns a saved request, so the zero-pending assertion has its premise.
        barrier.wait();
        control.stop();
        barrier.wait();
    });
    let outcome = run.run(
        move |ctx| init(ctx, &setup, hirsel_run_config::Reorder::Auto, false),
        move |ctx| unsafe {
            let _role = role::Guard::new(ctx);
            let result = request(ctx, &*calls, Request::ReadStdin { stdin: 12, max: 3 });
            crate::stop::check(ctx, result);
            panic!("stopped wait resumed")
        },
    );
    stopper.join().unwrap();
    assert_eq!(outcome.ended, Err(Stop::Requested));
    assert_eq!(outcome.pending, 0);
    assert_eq!(*host.finished.lock().unwrap(), [1]);
}

#[test]
fn ordered_host_terminal_external_stop_certifies_saved_wait_and_finishes_registration_once() {
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let host = Arc::new(Host {
                never_ready: true,
                stop_barrier: Some(barrier.clone()),
                ..Default::default()
            });
            let setup = host.clone();
            let calls = host.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.concurrent_adapter = Some(host.clone());
            unsafe { run.use_physical_dispatch() };
            let control = run.control();
            let stopper = std::thread::spawn(move || {
                barrier.wait();
                control.stop();
                barrier.wait();
            });
            let (outcome, trace) = run.run_traced(
                move |ctx| init(ctx, &setup, reorder, true),
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    request(ctx, &*calls, Request::Stdin { io: 1 })
                        .unwrap()
                        .unwrap();
                    let result = request(ctx, &*calls, Request::ReadStdin { stdin: 12, max: 3 });
                    crate::stop::check(ctx, result);
                    panic!("stopped wait resumed")
                },
            );
            stopper.join().unwrap();
            assert_eq!(outcome.ended, Err(Stop::Requested));
            assert_eq!(outcome.pending, 0);
            assert_eq!(*host.finished.lock().unwrap(), [1, 2]);
            assert_eq!(host.attempts.lock().unwrap().values().sum::<usize>(), 1);
            let trace = trace.expect("quiescent saved host terminal cut");
            let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(rows.last().unwrap()["item"]["kind"], "terminal");
            assert_eq!(rows.last().unwrap()["outcome"]["kind"], "stopped");
            assert!(
                rows.iter()
                    .any(|row| row["outcome"]["kind"] == "host_waiting")
            );
            assert_eq!(
                rows.iter()
                    .filter(|row| row["outcome"]["kind"] == "host_result")
                    .count(),
                1
            );
            if let Some(reference) = &reference {
                assert_eq!(trace.bytes(), reference);
            } else {
                reference = Some(trace.bytes().to_vec());
            }
        }
    }
}

#[test]
fn ordered_host_terminal_saved_completion_cut_keeps_bytes_before_owner_resume() {
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let host = Arc::new(Host {
                stop_completed: true,
                ..Default::default()
            });
            let setup = host.clone();
            let calls = host.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.concurrent_adapter = Some(host.clone());
            unsafe { run.use_physical_dispatch() };
            let (outcome, trace) = run.run_traced_after_bounded_stop(
                move |ctx| init(ctx, &setup, reorder, true),
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    request(ctx, &*calls, Request::Stdin { io: 1 })
                        .unwrap()
                        .unwrap();
                    let result = request(ctx, &*calls, Request::ReadStdin { stdin: 12, max: 3 });
                    crate::stop::check(ctx, result);
                    panic!("stopped saved completion returned to code")
                },
            );
            assert_eq!(outcome.ended, Err(Stop::Requested));
            assert!(outcome.pending <= 1);
            assert_eq!(*host.finished.lock().unwrap(), [1, 2]);
            assert_eq!(host.attempts.lock().unwrap().values().sum::<usize>(), 3);
            let trace = trace.expect("completed saved request remains owned at cut");
            let rows: Vec<serde_json::Value> = std::str::from_utf8(trace.bytes())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let results: Vec<_> = rows
                .iter()
                .filter(|row| row["outcome"]["kind"] == "host_result")
                .collect();
            assert_eq!(results.len(), 2);
            let payload = results[1]["outcome"]["result"]["payload"].as_str().unwrap();
            let mut expected = Vec::new();
            crate::tape_wire::Wire::encode(
                &Ok::<Completion, AdapterError>(Completion::Input(Ok(vec![0, 255, b'x']))),
                &mut expected,
            );
            assert_eq!(payload, hirsel_run_schema::hex(&expected));
            assert!(
                rows.iter()
                    .any(|row| row["outcome"]["kind"] == "host_abandoned"
                        && row["outcome"]["phase"] == "result")
            );
            assert_eq!(
                rows.iter()
                    .filter(|row| row["outcome"]["kind"] == "host_finished")
                    .count(),
                1
            );
            if let Some(reference) = &reference {
                assert_eq!(trace.bytes(), reference);
            } else {
                reference = Some(trace.bytes().to_vec());
            }
        }
    }
}
