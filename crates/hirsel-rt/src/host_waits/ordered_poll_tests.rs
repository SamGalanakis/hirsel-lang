use super::*;
use crate::io::Adapter;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

struct Attempts {
    calls: Arc<AtomicU32>,
    finishes: Arc<AtomicU32>,
    cancellable: bool,
}
impl Adapter for Attempts {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            Err(AdapterError::Blocked)
        } else {
            Ok(Completion::Wait(Ok(())))
        }
    }
    fn finish(&mut self, _: u64) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
    fn cancellable(&self, _: u64) -> bool {
        self.cancellable
    }
}
fn binding(cancellable: bool) -> (Binding, Arc<AtomicU32>, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let finishes = Arc::new(AtomicU32::new(0));
    let mut binding = Binding::new(Attempts {
        calls: calls.clone(),
        finishes: finishes.clone(),
        cancellable,
    });
    binding.waits.parked.insert(
        7,
        Parked {
            request: Request::SleepUntil {
                clock: 99,
                deadline: 100,
            },
            borrowed: None,
            task: Some(3),
            generation: 0,
            completion: None,
        },
    );
    (binding, calls, finishes)
}

#[test]
fn ordered_poll_observation_distinguishes_skipped_blocked_and_completed_attempts() {
    let (mut binding, calls, finishes) = binding(true);
    let mut snapshot = None;
    assert!(
        !unsafe { poll_binding_observed(&mut binding, 7, None, false, Some(&mut snapshot)) }
            .unwrap()
    );
    assert!(snapshot.is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(
        !unsafe { poll_binding_observed(&mut binding, 7, None, true, Some(&mut snapshot)) }
            .unwrap()
    );
    assert_eq!(snapshot, Some(Err(AdapterError::Blocked)));
    assert!(binding.waits.parked[&7].completion.is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(finishes.load(Ordering::Relaxed), 0);
    snapshot = None;
    assert!(
        unsafe { poll_binding_observed(&mut binding, 7, None, true, Some(&mut snapshot)) }.unwrap()
    );
    assert_eq!(snapshot, Some(Ok(Completion::Wait(Ok(())))));
    assert_eq!(binding.waits.parked[&7].completion, snapshot);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(finishes.load(Ordering::Relaxed), 1);
    binding.waits.parked.remove(&7);
    assert_eq!(snapshot, Some(Ok(Completion::Wait(Ok(())))));
}

#[test]
fn ordered_poll_observation_records_cancellation_only_when_adapter_allows_it() {
    for allowed in [false, true] {
        let (mut binding, calls, finishes) = binding(allowed);
        let mut snapshot = None;
        let done =
            unsafe { poll_binding_observed(&mut binding, 7, Some(5), false, Some(&mut snapshot)) }
                .unwrap();
        assert_eq!(done, allowed);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(finishes.load(Ordering::Relaxed), u32::from(allowed));
        assert_eq!(
            snapshot,
            allowed.then_some(Ok(Completion::Wait(Err(crate::io::WaitError::Canceled(5)))))
        );
    }
    // A no-observer caller retains the same skip/attempt/finish behavior.
    let (mut binding, calls, finishes) = binding(true);
    assert!(!unsafe { poll_binding(&mut binding, 7, None, false) }.unwrap());
    assert!(!unsafe { poll_binding(&mut binding, 7, None, true) }.unwrap());
    assert!(unsafe { poll_binding(&mut binding, 7, None, true) }.unwrap());
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(finishes.load(Ordering::Relaxed), 1);
}

#[test]
fn ordered_mixed_home_every_and_concurrent_tokens_support_binary_search_for_every_ready_owner() {
    let ready = ordered_ready_union(
        Ready::Every,
        vec![9, 1, 7, 9],
        vec![
            ReadyHost {
                generation: 0,
                task: 3,
                token: CONCURRENT | 2,
                deadline: None,
                ready: true,
            },
            ReadyHost {
                generation: 0,
                task: 4,
                token: CONCURRENT | 1,
                deadline: None,
                ready: false,
            },
            ReadyHost {
                generation: 0,
                task: 5,
                token: CONCURRENT | 2,
                deadline: None,
                ready: true,
            },
        ],
    );
    assert_eq!(ready, Ready::Tokens(vec![1, 7, 9, CONCURRENT | 2]));
    for token in [1, 7, 9, CONCURRENT | 2] {
        assert!(ready.contains(token));
    }
    for token in [0, 2, 8, CONCURRENT | 1] {
        assert!(!ready.contains(token));
    }
}

#[test]
fn ordered_mixed_home_every_keeps_armed_deadlines_for_expiry_with_concurrent_waits() {
    let (mut binding, calls, _) = binding(true);
    binding.waits.armed.insert(
        3,
        (
            42,
            0,
            Request::SleepUntil {
                clock: 99,
                deadline: 5,
            },
        ),
    );
    binding.waits.native.insert(
        1,
        Native {
            request: Request::ReadStdin { stdin: 2, max: 1 },
            task: Some(6),
            generation: 0,
            notified: false,
            ready: false,
        },
    );
    let ready = ordered_ready_union(
        Ready::Every,
        binding.waits.every_tokens(),
        vec![ReadyHost {
            generation: 0,
            task: 4,
            token: CONCURRENT | 1,
            deadline: None,
            ready: true,
        }],
    );
    assert_eq!(ready, Ready::Tokens(vec![1, 3, 7, CONCURRENT | 1]));
    // The timer is selected and attempted now, rather than being erased by
    // conversion of the home adapter's Every snapshot.
    assert!(expired_binding(&mut binding, &ready).unwrap().is_empty());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        expired_binding(&mut binding, &ready).unwrap(),
        [ExpiredScope {
            scope: 42,
            generation: 0,
            token: 3
        }]
    );
    acknowledge_expired(&mut binding, &[3]);
    assert!(binding.waits.armed.is_empty());
    assert!(binding.waits.armed_due.is_empty());
}

#[test]
fn ordered_home_poll_does_not_retain_another_tasks_result_under_the_running_caller() {
    struct Home;
    impl Adapter for Home {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            match request {
                Request::ReadStdin { .. } => Ok(Completion::Input(Ok(vec![1, 2, 3]))),
                Request::Close { .. } => Ok(Completion::Unit(Ok(()))),
                _ => panic!("unexpected request {request:?}"),
            }
        }
    }
    let mut run = crate::host::BodyRun::new(1, false);
    run.adapter = Some(Box::new(Home));
    unsafe { run.use_ordered_home_dispatch() };
    let outcome = run.run(
        |ctx| unsafe {
            crate::tasks::ordered::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            crate::tasks::ordered::events::enable(ctx);
            crate::tasks::ordered::events::initial_host(ctx, Some(1), None);
        },
        |ctx| unsafe {
            let _role = crate::tasks::role::Guard::new(ctx);
            let run = (*ctx).run_ptr();
            // A saved peer's poll runs synchronously on width-one home, while
            // the caller's TLS task remains installed. Its owned completion
            // belongs to the peer, never to the caller's initial-result slot.
            Binding::get(ctx).waits.parked.insert(
                7,
                Parked {
                    request: Request::ReadStdin { stdin: 12, max: 3 },
                    borrowed: None,
                    task: Some(42),
                    generation: 0,
                    completion: None,
                },
            );
            let (done, completed) = poll_observed_home(run, 7, None, true).unwrap();
            let completed = completed.unwrap();
            assert!(done);
            assert_eq!(completed.task, 42);
            assert_eq!(completed.result, Ok(Completion::Input(Ok(vec![1, 2, 3]))));
            assert!(
                (*run).ordered_home_results.lock().unwrap().is_empty(),
                "a scheduler poll must not create an issuing-task snapshot"
            );
            Binding::get(ctx).waits.parked.remove(&7);
            // A subsequent real cleanup must still be observed and taken
            // normally, retaining the strict duplicate-snapshot assertion.
            assert_eq!(
                crate::io::hirsel_rt_v2_io_close(ctx, 12, 0, 0),
                crate::STATUS_OK
            );
            assert!((*run).ordered_home_results.lock().unwrap().is_empty());
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}

#[test]
fn an_injected_parked_wait_is_polled_even_with_a_published_future_scope_timer() {
    struct Injected(std::time::Instant);
    impl Adapter for Injected {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            Err(AdapterError::Blocked)
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            Ok(Ready::Tokens(Vec::new()))
        }
        fn ready_deadline(&self) -> Option<std::time::Instant> {
            Some(self.0)
        }
        fn advance_ready_points(&mut self, _: u64) {}
        fn injected_wait(&self, request: &Request) -> bool {
            matches!(request, Request::WatchNext { .. })
        }
    }
    for width in [1, 2, 4] {
        let mut run = crate::host::BodyRun::new(width, true);
        run.adapter = Some(Box::new(Injected(
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )));
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                crate::tasks::ordered::engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                )
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                let timer = arm(ctx, 0, 1, 11, 100).unwrap().unwrap();
                // Derived watcher readiness has no driver Pending row. Mirror
                // the normal parked insertion without inventing a task frame.
                Binding::raw(ctx).waits.parked.insert(
                    77,
                    Parked {
                        request: Request::WatchNext {
                            directory: 1,
                            watcher: 2,
                        },
                        borrowed: None,
                        task: None,
                        generation: 0,
                        completion: None,
                    },
                );
                count(ctx);
                let found = ready(ctx).unwrap();
                assert!(found.contains(77), "derived readiness must reach this poll");
                assert!(!found.contains(timer));
                Binding::raw(ctx).waits.parked.remove(&77);
                count(ctx);
                disarm(ctx, timer);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    }
}

#[test]
fn home_readiness_releases_the_role_before_reacquiring_a_compute_permit() {
    use std::sync::atomic::{AtomicBool, AtomicPtr};
    struct HomeReady {
        run: Arc<AtomicPtr<crate::context::RunShared>>,
        released: Arc<AtomicBool>,
    }
    impl Adapter for HomeReady {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            unreachable!()
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            let run = self.run.load(Ordering::Acquire);
            // Model the real permit waiter: the home bridge cannot return its
            // permit while another worker holds capacity waiting for this role.
            // The readiness callback borrows only the home binding, so it must
            // leave the scheduler available throughout the synchronous bridge.
            let guard = unsafe { crate::tasks::role::IdleGuard::try_new(run) };
            self.released.store(guard.is_some(), Ordering::Release);
            drop(guard);
            Ok(Ready::Every)
        }
    }
    for width in [1, 4] {
        let shared = Arc::new(AtomicPtr::default());
        let released = Arc::new(AtomicBool::new(false));
        let mut run = crate::host::BodyRun::new(width, true);
        run.adapter = Some(Box::new(HomeReady {
            run: shared.clone(),
            released: released.clone(),
        }));
        unsafe { run.use_ordered_home_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                shared.store((*ctx).run_ptr(), Ordering::Release);
                crate::tasks::ordered::engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                at_home(ctx, || {
                    Binding::get(ctx).waits.parked.insert(
                        7,
                        Parked {
                            request: Request::ReadStdin { stdin: 12, max: 1 },
                            borrowed: None,
                            task: None,
                            generation: 0,
                            completion: None,
                        },
                    );
                    count(ctx);
                })
                .unwrap();
                assert_eq!(ready(ctx).unwrap(), Ready::Every);
                assert!(
                    crate::tasks::role::IdleGuard::try_new((*ctx).run_ptr()).is_none(),
                    "the issuer must regain its role before touching the scheduler"
                );
                at_home(ctx, || {
                    Binding::get(ctx).waits.parked.remove(&7);
                    count(ctx);
                })
                .unwrap();
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert!(
            released.load(Ordering::Acquire),
            "home readiness retained the role across compute-permit reacquisition at width {width}"
        );
    }
}

#[test]
fn home_timer_arming_lends_the_role_and_preserves_the_copied_issuer() {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicPtr};
    struct HomeTimer {
        run: Arc<AtomicPtr<crate::context::RunShared>>,
        released: Arc<AtomicBool>,
        issuer: Arc<Mutex<Option<Vec<u32>>>>,
    }
    impl Adapter for HomeTimer {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            unreachable!()
        }
        fn wants_issuer(&self) -> bool {
            true
        }
        fn issuer(&mut self, issuer: &[u32]) {
            *self.issuer.lock().unwrap() = Some(issuer.to_vec());
        }
        fn scope_timer(&mut self, _: Request) -> Result<Completion, AdapterError> {
            let guard =
                unsafe { crate::tasks::role::IdleGuard::try_new(self.run.load(Ordering::Acquire)) };
            self.released.store(guard.is_some(), Ordering::Release);
            drop(guard);
            Ok(Completion::Wait(Ok(())))
        }
    }
    for width in [1, 4] {
        let shared = Arc::new(AtomicPtr::default());
        let released = Arc::new(AtomicBool::new(false));
        let issuer = Arc::new(Mutex::new(None));
        let mut run = crate::host::BodyRun::new(width, true);
        run.adapter = Some(Box::new(HomeTimer {
            run: shared.clone(),
            released: released.clone(),
            issuer: issuer.clone(),
        }));
        unsafe { run.use_ordered_home_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                shared.store((*ctx).run_ptr(), Ordering::Release);
                crate::tasks::ordered::engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                assert_eq!(arm(ctx, 0, 1, 11, 100).unwrap(), None);
                assert!(
                    crate::tasks::role::IdleGuard::try_new((*ctx).run_ptr()).is_none(),
                    "issuer role restored after timer bridge"
                );
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(*issuer.lock().unwrap(), Some(vec![]));
        assert!(
            released.load(Ordering::Acquire),
            "home wait-table bridge retained role at width {width}"
        );
    }
}
