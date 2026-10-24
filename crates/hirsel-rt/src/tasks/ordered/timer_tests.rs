//! Home-affine timer admission and retirement observations.
use super::super::*;
use super::engine;
use crate::io::{Adapter, AdapterError, Completion, Ready, Request};
use crate::ordered::Policy;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct Clock {
    home: std::thread::ThreadId,
    now: Arc<AtomicU64>,
    calls: Arc<AtomicU64>,
    finishes: Arc<AtomicU64>,
    advance: bool,
}
impl Adapter for Clock {
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        assert!(matches!(request, Request::SleepUntil { .. }));
        Ok(None)
    }

    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        let Request::SleepUntil { clock, deadline } = request else {
            panic!("unexpected {request:?}")
        };
        assert_eq!(clock, 11);
        self.calls.fetch_add(1, Ordering::Relaxed);
        if deadline <= self.now.load(Ordering::Acquire) {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        Ok(if self.now.load(Ordering::Acquire) >= 100 {
            Ready::Every
        } else {
            Ready::Tokens(Vec::new())
        })
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        if !self.advance {
            return Ok(false);
        }
        self.now.store(100, Ordering::Release);
        Ok(true)
    }
    fn finish(&mut self, _: u64) {
        assert_eq!(std::thread::current().id(), self.home);
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn ordered_scope_timers_arm_expire_and_disarm_only_on_the_home_thread() {
    for width in [2, 4, 8] {
        for mode in 0..3 {
            let now = Arc::new(AtomicU64::new(0));
            let calls = Arc::new(AtomicU64::new(0));
            let finishes = Arc::new(AtomicU64::new(0));
            let seen = now.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.adapter = Some(Box::new(Clock {
                home: std::thread::current().id(),
                now,
                calls: calls.clone(),
                finishes: finishes.clone(),
                advance: true,
            }));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
                move |ctx| unsafe {
                    let mut scope = 0;
                    let deadline = [11, if mode == 0 { 0 } else { 100 }];
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter_until(
                            ctx,
                            std::ptr::null_mut(),
                            &mut scope,
                            &0,
                            &deadline
                        ),
                        STATUS_OK
                    );
                    let mut canceled = u32::MAX;
                    if mode == 2 {
                        seen.store(100, Ordering::Release);
                        // A keep segment alone cannot observe expiration. The
                        // next logical scheduling point polls the armed timer.
                        let _role = role::Guard::new(ctx);
                        assert!(
                            !scheduler(ctx)
                                .scope(scope as u32)
                                .deadline
                                .as_ref()
                                .unwrap()
                                .expired
                        );
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                        STATUS_OK
                    );
                    assert_eq!(
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                        STATUS_OK
                    );
                    assert_eq!(canceled, if mode == 1 { 0 } else { 1 + CANCEL_DEADLINE });
                    if mode != 1 {
                        crate::finalize::hirsel_rt_v2_finalize_begin(ctx, 0);
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                            STATUS_OK
                        );
                        assert_eq!(canceled, 0, "finalizers shield deadline cancellation");
                        crate::finalize::hirsel_rt_v2_finalize_end(ctx, 0);
                    }
                    let mut expired = 99;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut expired, &scope, 0),
                        STATUS_OK
                    );
                    assert_eq!(expired, u8::from(mode != 1));
                    assert_eq!((*ctx).run_ref().host_waits.load(Ordering::Acquire), 0);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(calls.load(Ordering::Relaxed), if mode == 2 { 2 } else { 1 });
            assert_eq!(
                finishes.load(Ordering::Relaxed),
                1,
                "one timer token retired exactly once"
            );
        }
    }
}

#[test]
fn deadline_events_keep_scope_identity_arm_outcome_and_expiry_before_cancellation_observation() {
    use super::events::{CanonicalItem, CanonicalOutcome};
    use std::sync::Mutex;
    for mode in 0..3 {
        for known in [false, true] {
            let mut reference = None;
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    let now = Arc::new(AtomicU64::new(0));
                    let seen_now = now.clone();
                    let calls = Arc::new(AtomicU64::new(0));
                    let finishes = Arc::new(AtomicU64::new(0));
                    let output = Arc::new(Mutex::new(Vec::new()));
                    let seen = output.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.adapter = Some(Box::new(Clock {
                        home: std::thread::current().id(),
                        now,
                        calls: calls.clone(),
                        finishes: finishes.clone(),
                        advance: true,
                    }));
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            engine::enable(ctx, policy);
                            super::events::enable(ctx);
                            if known {
                                // An explicitly admitted fixture clock. Real host
                                // creator/key registration remains a separate gate.
                                engine::driver(ctx).objects.bind_erased(
                                    crate::ordered::objects::Kind::Capability,
                                    11,
                                    crate::ordered::footprint::ObjectId {
                                        creator: vec![99],
                                        ordinal: 0,
                                    },
                                    Default::default(),
                                );
                            }
                        },
                        move |ctx| unsafe {
                            let mut scope = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter_until(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut scope,
                                    &0,
                                    &[11, if mode == 0 { 0 } else { 100 }]
                                ),
                                STATUS_OK
                            );
                            if mode == 2 {
                                seen_now.store(100, Ordering::Release);
                            }
                            let mut canceled = 99;
                            for _ in 0..2 {
                                assert_eq!(
                                    hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                                    STATUS_OK
                                );
                            }
                            assert_eq!(canceled, if mode == 1 { 0 } else { 1 + CANCEL_DEADLINE });
                            let mut expired = 99;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit_until(
                                    ctx,
                                    0,
                                    0,
                                    &mut expired,
                                    &scope,
                                    0
                                ),
                                STATUS_OK
                            );
                            assert_eq!(expired, u8::from(mode != 1));
                            let _role = role::Guard::new(ctx);
                            let log = engine::driver(ctx).events.as_ref().unwrap();
                            if known {
                                assert!(log.canonical().is_ok());
                            } else {
                                assert!(
                                    log.canonical().is_err(),
                                    "an unknown clock cannot certify a host effect"
                                );
                            }
                            let rows: Vec<_> = log
                                .canonical
                                .iter()
                                .filter(|row| row.op == "task.scope_enter")
                                .collect();
                            let id = crate::ordered::footprint::ObjectId {
                                creator: vec![],
                                ordinal: 0,
                            };
                            assert_eq!(rows[0].outcome, CanonicalOutcome::Created(id.clone()));
                            assert!(
                                rows.iter()
                                    .all(|row| row.ordinal == 0 && row.task.is_empty())
                            );
                            let arms: Vec<_> = rows
                                .iter()
                                .filter(|row| {
                                    matches!(row.outcome, CanonicalOutcome::DeadlineArmed { .. })
                                })
                                .collect();
                            assert_eq!(arms.len(), usize::from(known));
                            if known {
                                assert_eq!(
                                    arms[0].item,
                                    CanonicalItem::HostKey(crate::ordered::footprint::ObjectId {
                                        creator: vec![99],
                                        ordinal: 0
                                    })
                                );
                                assert_eq!(
                                    arms[0].outcome,
                                    CanonicalOutcome::DeadlineArmed {
                                        deadline: if mode == 0 { 0 } else { 100 },
                                        armed: mode != 0
                                    }
                                );
                            }
                            let expiry = rows
                                .iter()
                                .filter(|row| row.outcome == CanonicalOutcome::DeadlineExpired)
                                .count();
                            assert_eq!(expiry, usize::from(mode != 1));
                            if mode != 1 {
                                let expired_at = log
                                    .canonical
                                    .iter()
                                    .position(|row| {
                                        row.outcome == CanonicalOutcome::DeadlineExpired
                                    })
                                    .unwrap();
                                assert!(log.canonical[expired_at + 1..].iter().any(|row| row.item
                                    == CanonicalItem::Cancellation(vec![])
                                    && row.outcome
                                        == CanonicalOutcome::Scalar(
                                            (1 + CANCEL_DEADLINE).to_le_bytes().to_vec()
                                        )));
                                assert_eq!(
                                    log.canonical[expired_at].item,
                                    CanonicalItem::Object(id)
                                );
                            }
                            *seen.lock().unwrap() = log.canonical.clone();
                            STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(STATUS_OK));
                    assert_eq!(calls.load(Ordering::Relaxed), if mode == 2 { 2 } else { 1 });
                    assert_eq!(finishes.load(Ordering::Relaxed), 1);
                    let rows = output.lock().unwrap().clone();
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
    }
}

#[test]
fn an_armed_timer_wakes_the_last_saved_receiver_without_a_runnable_turn() {
    timer_wait_run(false, true);
}

#[test]
fn deadline_expiry_wakes_scope_exit_waiting_for_a_saved_child_receiver() {
    timer_wait_run(true, true);
}

#[test]
fn an_armed_timer_with_no_wake_source_reports_blocked_after_all_frames_save() {
    timer_wait_run(false, false);
}

unsafe extern "C" fn timer_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let mut receiver = unsafe { crate::alloc::payload(env).cast::<u64>().read() };
    let mut message = [0u64; 2];
    let mut status = u32::MAX;
    assert_eq!(
        unsafe {
            hirsel_rt_v2_channel_receive(
                ctx,
                0,
                0,
                &mut status,
                &mut receiver,
                message.as_mut_ptr().cast(),
            )
        },
        STATUS_OK
    );
    assert_eq!(status, crate::channels::CANCELED + CANCEL_DEADLINE);
    assert_eq!(
        unsafe { crate::finalize::hirsel_rt_v2_close_handle(ctx, receiver, 0, 0) },
        STATUS_OK
    );
    STATUS_OK
}

fn timer_wait_run(child: bool, advance: bool) {
    for width in [2, 4, 8] {
        let mut run = crate::host::BodyRun::new(width, true);
        run.adapter = Some(Box::new(Clock {
            home: std::thread::current().id(),
            now: Arc::new(AtomicU64::new(0)),
            calls: Arc::new(AtomicU64::new(0)),
            finishes: Arc::new(AtomicU64::new(0)),
            advance,
        }));
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome =
            run.run(
                |ctx| unsafe {
                    engine::enable(ctx, Policy::admit(&Default::default(), None));
                    super::events::enable(ctx);
                    engine::driver(ctx).objects.bind_erased(
                        crate::ordered::objects::Kind::Capability,
                        11,
                        crate::ordered::footprint::ObjectId {
                            creator: vec![99],
                            ordinal: 0,
                        },
                        Default::default(),
                    );
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter_until(
                            ctx,
                            std::ptr::null_mut(),
                            &mut scope,
                            &0,
                            &[11, 100]
                        ),
                        STATUS_OK
                    );
                    let mut endpoints = [0; 2];
                    let mut message = [0u64; 2];
                    assert_eq!(
                        hirsel_rt_v2_channel_open(
                            ctx,
                            std::ptr::null_mut(),
                            endpoints.as_mut_ptr(),
                            &0,
                            message.as_ptr().cast(),
                            2
                        ),
                        STATUS_OK
                    );
                    if child {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env).cast::<u64>().write(endpoints[1]);
                        let mut task = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut task,
                                &scope,
                                timer_child,
                                env,
                                0
                            ),
                            STATUS_OK
                        );
                    } else {
                        let mut status = u32::MAX;
                        assert_eq!(
                            hirsel_rt_v2_channel_receive(
                                ctx,
                                0,
                                0,
                                &mut status,
                                &mut endpoints[1],
                                message.as_mut_ptr().cast()
                            ),
                            STATUS_OK
                        );
                        assert_eq!(status, crate::channels::CANCELED + CANCEL_DEADLINE);
                        assert_eq!(message, [0, 0]);
                    }
                    let mut expired = 99;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut expired, &scope, 0),
                        STATUS_OK
                    );
                    assert_eq!(expired, 1);
                    for endpoint in if child {
                        vec![endpoints[0]]
                    } else {
                        endpoints.to_vec()
                    } {
                        assert_eq!(
                            crate::finalize::hirsel_rt_v2_close_handle(ctx, endpoint, 0, 0),
                            STATUS_OK
                        );
                    }
                    {
                        let _role = role::Guard::new(ctx);
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        let expiries: Vec<_> = log
                            .canonical
                            .iter()
                            .enumerate()
                            .filter(|(_, row)| {
                                row.outcome == super::events::CanonicalOutcome::DeadlineExpired
                            })
                            .collect();
                        assert_eq!(expiries.len(), 1);
                        assert_eq!(expiries[0].1.ordinal, 0);
                        let receiver_path = if child { vec![0] } else { vec![] };
                        let received = log
                            .canonical
                            .iter()
                            .position(|row| {
                                row.task == receiver_path
                                    && matches!(row.outcome,
                            super::events::CanonicalOutcome::Received { status, payload: None }
                            if status == crate::channels::CANCELED + CANCEL_DEADLINE)
                            })
                            .unwrap();
                        assert!(
                            expiries[0].0 < received,
                            "idle expiry is recorded before receive completion/wake"
                        );
                        assert!(log.canonical[expiries[0].0 + 1..received].iter().any(
                            |row| row.item
                                == super::events::CanonicalItem::Cancellation(
                                    receiver_path.clone()
                                )
                                && row.outcome
                                    == super::events::CanonicalOutcome::Scalar(
                                        (1 + CANCEL_DEADLINE).to_le_bytes().to_vec()
                                    )
                        ));
                    }
                    STATUS_OK
                },
            );
        if advance {
            assert_eq!(outcome.ended, Ok(STATUS_OK));
        } else {
            let Err(crate::stop::Stop::Blocked(waits)) = outcome.ended else {
                panic!("expected Blocked")
            };
            assert_eq!(waits.len(), 1);
            assert_eq!(waits[0].kind, "channel.receive");
        }
        assert_eq!(outcome.pending, 0);
    }
}

#[test]
fn an_unsaved_receiver_releases_the_home_poll_reservation_then_retries_after_save() {
    for width in [2, 4, 8] {
        let now = Arc::new(AtomicU64::new(0));
        let mut run = crate::host::BodyRun::new(width, true);
        run.adapter = Some(Box::new(Clock {
            home: std::thread::current().id(),
            now: now.clone(),
            calls: Arc::new(AtomicU64::new(0)),
            finishes: Arc::new(AtomicU64::new(0)),
            advance: true,
        }));
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: std::ptr::null(),
            scan_words: 0,
        };
        let layouts = [&layout as *const crate::Layout];
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter_until(
                        ctx,
                        std::ptr::null_mut(),
                        &mut scope,
                        &0,
                        &[11, 100]
                    ),
                    STATUS_OK
                );
                let mut endpoints = [0; 2];
                let mut image = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        endpoints.as_mut_ptr(),
                        &0,
                        image.as_ptr().cast(),
                        2
                    ),
                    STATUS_OK
                );
                let task = engine::current(ctx);
                let (_, progress) = engine::head_effect(ctx, "channel.receive", || {
                    let s = scheduler(ctx);
                    let channel = s.channels.channel(endpoints[1], false);
                    s.park(Wait::Receive {
                        channel,
                        slot: image.as_mut_ptr().cast(),
                    });
                    ((), crate::ordered::Next::Park)
                })
                .unwrap();
                crate::tasks::ordered::engine::with_driver(ctx, |d| d.defer_poll_until_saved(task));
                engine::driver(ctx).host_poll = true;
                assert!(!readiness::saved(scheduler(ctx), task as u32));
                role::release(ctx);
                let run_ptr = (*ctx).run_ptr();
                let mut completed = true;
                crate::pool::call_home(ctx, &mut || {
                    completed = super::home::poll(run_ptr, true, false).unwrap();
                })
                .unwrap();
                role::enter(ctx);
                assert!(!completed, "unsaved frame must defer this poll");
                assert!(
                    !crate::tasks::ordered::engine::with_driver(ctx, |d| d.host_poll),
                    "home reservation must be retryable"
                );
                assert!(
                    engine::driver(ctx)
                        .frontier
                        .as_ref()
                        .unwrap()
                        .retiring_task()
                        .is_some()
                );
                now.store(100, Ordering::Release);
                crate::tasks::ordered::engine::with_driver(ctx, |d| d.suspending(task, false));
                crate::pool::free::suspend_current(ctx, false);
                engine::at_head(ctx).unwrap();
                assert_eq!(
                    crate::tasks::role::with_role(ctx, |s| s.task(task as u32).status),
                    crate::channels::CANCELED + CANCEL_DEADLINE
                );
                assert!(
                    engine::driver(ctx)
                        .frontier
                        .as_ref()
                        .unwrap()
                        .retiring_task()
                        .is_none()
                );
                assert_eq!(progress, crate::ordered::Progress::Poll);
                let mut expired = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut expired, &scope, 0),
                    STATUS_OK
                );
                assert_eq!(expired, 1);
                for endpoint in endpoints {
                    crate::finalize::hirsel_rt_v2_close_handle(ctx, endpoint, 0, 0);
                }
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
}

/// The retirement precheck must preserve the number and position of logical
/// polls even when no worker asks home to query the timer driver.
#[test]
fn timer_readiness_defers_exact_poll_points_until_before_the_next_adapter_operation() {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    struct Timed {
        home: std::thread::ThreadId,
        hint: bool,
        armed: bool,
        end: Instant,
        points: u64,
        queries: Arc<AtomicU64>,
        boundaries: Arc<Mutex<Vec<u64>>>,
    }
    impl Adapter for Timed {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            self.boundaries.lock().unwrap().push(self.points);
            self.armed = true;
            Err(AdapterError::Blocked)
        }
        fn finish(&mut self, _: u64) {
            assert_eq!(std::thread::current().id(), self.home);
            self.boundaries.lock().unwrap().push(self.points);
            self.armed = false;
        }
        fn ready(&mut self) -> Result<Ready, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            self.points += 1;
            self.queries.fetch_add(1, Ordering::Relaxed);
            Ok(Ready::Tokens(Vec::new()))
        }
        fn ready_deadline(&self) -> Option<Instant> {
            (self.hint && self.armed).then_some(self.end)
        }
        fn advance_ready_points(&mut self, points: u64) {
            assert_eq!(std::thread::current().id(), self.home);
            self.points += points;
        }
    }
    for width in [1, 2, 4] {
        let mut reference = None;
        for hint in [false, true] {
            let queries = Arc::new(AtomicU64::new(0));
            let boundaries = Arc::new(Mutex::new(Vec::new()));
            let mut run = crate::host::BodyRun::new(width, true);
            // Production retains its adapter behind this launcher wrapper.
            let adapter = Timed {
                home: std::thread::current().id(),
                hint,
                armed: false,
                end: Instant::now() + Duration::from_secs(60),
                points: 0,
                queries: queries.clone(),
                boundaries: boundaries.clone(),
            };
            run.adapter = Some(Box::new(crate::launch::Shared(std::rc::Rc::new(
                std::cell::RefCell::new(adapter),
            ))));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
                |ctx| unsafe {
                    let mut scope = 0;
                    let deadline = [11, 100];
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter_until(
                            ctx,
                            std::ptr::null_mut(),
                            &mut scope,
                            &0,
                            &deadline
                        ),
                        STATUS_OK
                    );
                    for _ in 0..32 {
                        let mut canceled = 99;
                        assert_eq!(
                            hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                            STATUS_OK
                        );
                        assert_eq!(canceled, 0);
                    }
                    let mut expired = 99;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut expired, &scope, 0),
                        STATUS_OK
                    );
                    assert_eq!(expired, 0);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            let boundaries = boundaries.lock().unwrap().clone();
            assert!(boundaries.last().copied().unwrap() >= 32);
            if let Some((points, raw_queries)) = &reference {
                assert_eq!(&boundaries, points, "same polls before issue and finish");
                assert!(queries.load(Ordering::Relaxed) < *raw_queries);
            } else {
                reference = Some((boundaries, queries.load(Ordering::Relaxed)));
            }
        }
    }
}

#[test]
fn an_armed_future_timer_fast_path_observes_an_external_stop_at_the_next_entry() {
    struct FutureTimer(std::time::Instant);
    impl Adapter for FutureTimer {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            Err(AdapterError::Blocked)
        }
        fn ready_deadline(&self) -> Option<std::time::Instant> {
            Some(self.0)
        }
        fn advance_ready_points(&mut self, _: u64) {}
    }
    for width in [1, 2, 4] {
        let mut run = crate::host::BodyRun::new(width, true);
        run.adapter = Some(Box::new(FutureTimer(
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        )));
        unsafe { run.use_physical_dispatch() };
        let control = run.control();
        let (begun, receive) = std::sync::mpsc::sync_channel(0);
        let (stopped, await_stop) = std::sync::mpsc::sync_channel(0);
        let stopper = std::thread::spawn(move || {
            receive.recv().unwrap();
            control.stop();
            stopped.send(()).unwrap();
        });
        let outcome = run.run(
            |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter_until(
                        ctx,
                        std::ptr::null_mut(),
                        &mut scope,
                        &0,
                        &[11, 100]
                    ),
                    STATUS_OK
                );
                for _ in 0..16 {
                    let mut canceled = 99;
                    assert_eq!(
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0),
                        STATUS_OK
                    );
                    assert_eq!(canceled, 0);
                }
                begun.send(()).unwrap();
                await_stop.recv().unwrap();
                let mut canceled = 99;
                hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                panic!("the next runtime entry must observe the external stop");
            },
        );
        stopper.join().unwrap();
        assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
    }
}
