//! Deadline calls stay outside recorder sections and on their home adapter.
use super::*;
use crate::io::{Adapter, AdapterError, Completion, Request};
use crate::tasks::free::replay::Replay;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::replay::Replayer;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct Clock {
    home: std::thread::ThreadId,
    recording: Arc<super::super::record::Recording>,
    calls: Arc<AtomicU64>,
    finishes: Arc<AtomicU64>,
    exit_op: u64,
}

struct NoReplayClock;
impl Adapter for NoReplayClock {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        panic!("native replay consulted a live deadline adapter")
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        panic!("native replay issued a live deadline")
    }
    fn ready(&mut self) -> Result<crate::io::Ready, AdapterError> {
        panic!("native replay polled a live deadline")
    }
    fn finish(&mut self, _: u64) {
        panic!("native replay finished a live deadline")
    }
}

#[test]
fn native_deadline_replay_substitutes_expiry_without_calling_live_timers() {
    for durable in [false, true] {
        for expired in [false, true] {
            let recording =
                super::super::record::Recording::new(b"native deadline replay", durable);
            let install = recording.clone();
            let calls = Arc::new(AtomicU64::new(0));
            let finishes = Arc::new(AtomicU64::new(0));
            let mut run = crate::host::BodyRun::new(8, true);
            run.free = true;
            run.adapter = Some(Box::new(Clock {
                home: std::thread::current().id(),
                recording: recording.clone(),
                calls: calls.clone(),
                finishes: finishes.clone(),
                exit_op: 3,
            }));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    let deadline = [11, if expired { 0 } else { 100 }];
                    hirsel_rt_v2_task_scope_enter_until(
                        ctx,
                        std::ptr::null_mut(),
                        &mut scope,
                        &0,
                        &deadline,
                    );
                    let mut cause = 99;
                    hirsel_rt_v2_task_canceled(ctx, &mut cause, &0);
                    assert_eq!(cause, if expired { CANCEL_DEADLINE + 1 } else { 0 });
                    let mut observed = 99;
                    hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut observed, &scope, 0);
                    assert_eq!(observed, u8::from(expired));
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(calls.load(Ordering::Acquire), 1);
            assert_eq!(finishes.load(Ordering::Acquire), 1);
            recording.recorder.stop("returned", None).unwrap().unwrap();
            let trace = recording.trace();
            let bytes = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes;
            let entered = (1..bytes.len())
                .find(|&end| {
                    hirsel_free::tape::parse(&bytes[..end]).is_ok_and(|tape| {
                        tape.cut.get(&Vec::new()) == Some(&Progress::new(1, Phase::Entered))
                    })
                })
                .unwrap();
            for end in [entered, entered + 1, bytes.len()] {
                let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
                let full = end == bytes.len();
                let expected = Projection::new(
                    if full { "returned" } else { "truncated" },
                    &parsed.cut,
                    &trace,
                );
                for width in [2, 8] {
                    let replay = Replay::new(&bytes[..end]).unwrap();
                    let install = replay.clone();
                    let outputs = Arc::new((
                        std::sync::atomic::AtomicU32::new(99),
                        std::sync::atomic::AtomicU8::new(99),
                    ));
                    let observed = outputs.clone();
                    let mut run = crate::host::BodyRun::new(width, true);
                    run.free = true;
                    run.adapter = Some(Box::new(NoReplayClock));
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe { install.root(ctx) },
                        move |ctx| unsafe {
                            let mut scope = 0;
                            // The adapter must never interpret this different
                            // deadline, even when the recording expired.
                            hirsel_rt_v2_task_scope_enter_until(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0,
                                &[999, u64::MAX],
                            );
                            hirsel_rt_v2_task_canceled(ctx, observed.0.as_ptr(), &0);
                            hirsel_rt_v2_task_scope_exit_until(
                                ctx,
                                0,
                                0,
                                observed.1.as_ptr(),
                                &scope,
                                0,
                            );
                            panic!("native deadline replay passed its cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(
                        outputs.0.load(Ordering::Acquire),
                        if full {
                            if expired { CANCEL_DEADLINE + 1 } else { 0 }
                        } else {
                            99
                        }
                    );
                    assert_eq!(
                        outputs.1.load(Ordering::Acquire),
                        if full { u8::from(expired) } else { 99 }
                    );
                    assert_eq!(replay.finish().unwrap(), expected);
                }
            }
        }
    }
}
impl Adapter for Clock {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        // A synchronous cut here also proves the timer call holds no section.
        let cut = self.recording.recorder.flush().unwrap().unwrap();
        assert_eq!(
            cut.progress[&Vec::new()],
            Progress::new(1, Phase::Entered),
            "timer issue follows its recorded scope entry"
        );
        self.calls.fetch_add(1, Ordering::Relaxed);
        let Request::SleepUntil {
            clock: 11,
            deadline,
        } = request
        else {
            panic!("unexpected {request:?}")
        };
        if deadline == 0 {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn ready(&mut self) -> Result<crate::io::Ready, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        // This fixture's nonzero timer stays pending until disarm. The
        // scripted default Ready::Every would reissue it during cleanup.
        Ok(crate::io::Ready::Tokens(vec![]))
    }
    fn finish(&mut self, _: u64) {
        assert_eq!(std::thread::current().id(), self.home);
        let cut = self.recording.recorder.flush().unwrap().unwrap();
        let progress = cut.progress[&Vec::new()];
        assert!(
            progress == Progress::new(1, Phase::Entered)
                || progress == Progress::new(self.exit_op, Phase::Entered),
            "timer cleanup is outside the entry/exit section: {progress:?}"
        );
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn native_deadline_entry_records_before_home_arm_and_exit_records_expired() {
    for width in [2, 8] {
        for durable in [false, true] {
            for expired in [false, true] {
                let recording = super::super::record::Recording::new(b"native deadline", durable);
                let install = recording.clone();
                let calls = Arc::new(AtomicU64::new(0));
                let finishes = Arc::new(AtomicU64::new(0));
                let mut run = crate::host::BodyRun::new(width, true);
                run.free = true;
                run.adapter = Some(Box::new(Clock {
                    home: std::thread::current().id(),
                    recording: recording.clone(),
                    calls: calls.clone(),
                    finishes: finishes.clone(),
                    exit_op: 4,
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        let at = [11, if expired { 0 } else { 100 }];
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter_until(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0,
                                &at
                            ),
                            STATUS_OK
                        );
                        let mut canceled = 99;
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                        assert_eq!(canceled, if expired { 1 + CANCEL_DEADLINE } else { 0 });
                        crate::finalize::hirsel_rt_v2_finalize_begin(ctx, 0);
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                        assert_eq!(canceled, 0, "finalizers shield deadline cancellation");
                        crate::finalize::hirsel_rt_v2_finalize_end(ctx, 0);
                        let mut observed = 99;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut observed, &scope, 0),
                            STATUS_OK
                        );
                        assert_eq!(observed, u8::from(expired));
                        assert_eq!((*ctx).run_ref().host_waits.load(Ordering::Acquire), 0);
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(calls.load(Ordering::Relaxed), 1);
                assert_eq!(finishes.load(Ordering::Relaxed), 1);
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                assert_eq!(cut.progress[&Vec::new()], Progress::new(4, Phase::Done));
                let trace = recording.trace();
                let marked = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink();
                let tape = hirsel_free::tape::parse(&marked.bytes).unwrap();
                let mut replay = Replayer::new(&tape).unwrap();
                for op in 1..=4 {
                    if op == 2 {
                        assert_eq!(
                            replay.cancel_check(&[], op).unwrap(),
                            expired.then_some(CANCEL_DEADLINE)
                        );
                    }
                    if op == 4 {
                        assert_eq!(replay.expired(&[], op).unwrap(), expired);
                    }
                    replay.publish(&[], Progress::new(op, Phase::Done)).unwrap();
                }
                assert_eq!(
                    replay.finish(&cut.progress).unwrap(),
                    Projection::new("returned", &cut.progress, &trace)
                );
            }
        }
    }
}

unsafe extern "C" fn held_result(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let scope = unsafe { crate::alloc::payload(env).cast::<u64>().read() };
    let mut image = [1u64, 7];
    let none = [0u64; 2];
    unsafe { hirsel_rt_v2_task_finish(ctx, image.as_mut_ptr().cast(), none.as_ptr().cast(), 16) };
    assert_eq!(image, [1, 7], "scope exit discards in the child's frame");
    // Force the home readiness probe while the parent is at exit/Entered,
    // rather than relying on an incidental idle-loop scheduling window.
    let run = unsafe { (*ctx).run_ptr() };
    let mut ready = None;
    unsafe {
        crate::pool::call_home(ctx, &mut || {
            ready = Some(crate::host_waits::timers_home(run, false));
        })
        .unwrap();
    }
    assert!(ready.unwrap().unwrap().0.is_empty());
    for _ in 0..130 {
        let mut canceled = 99;
        unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) };
        assert_eq!(canceled, 0);
        let _role = unsafe { role::Guard::new(ctx) };
        let deadline = scheduler(ctx)
            .scope(scope as u32)
            .deadline
            .as_ref()
            .unwrap();
        assert!(
            deadline.token.is_some(),
            "the scope retains its armed timer through cleanup"
        );
        assert!(!deadline.expired);
    }
    STATUS_OK
}

#[test]
fn native_scope_exit_keeps_its_timer_until_the_saved_child_rest_returns() {
    for width in [2, 8] {
        for durable in [false, true] {
            let recording = super::super::record::Recording::new(b"native deadline rest", durable);
            let install = recording.clone();
            let calls = Arc::new(AtomicU64::new(0));
            let finishes = Arc::new(AtomicU64::new(0));
            let mut run = crate::host::BodyRun::new(width, true);
            run.free = true;
            run.adapter = Some(Box::new(Clock {
                home: std::thread::current().id(),
                recording: recording.clone(),
                calls: calls.clone(),
                finishes: finishes.clone(),
                exit_op: 3,
            }));
            let layout = crate::Layout {
                magic: crate::LAYOUT_MAGIC,
                version: crate::LAYOUT_VERSION,
                size: 8,
                align: 8,
                finalizer: None,
                steps: std::ptr::null(),
                step_count: 0,
                field_count: 1,
                flags: 0,
                logical: 0,
                logical_count: std::ptr::null(),
                scan: std::ptr::null(),
                scan_words: 0,
            };
            let layouts = [&raw const layout];
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter_until(
                        ctx,
                        std::ptr::null_mut(),
                        &mut scope,
                        &0,
                        &[11, 100],
                    );
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env).cast::<u64>().write(scope);
                    let mut child = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        held_result,
                        env,
                        0,
                    );
                    let mut expired = 99;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut expired, &scope, 0),
                        STATUS_OK
                    );
                    assert_eq!(expired, 0);
                    assert_eq!((*ctx).run_ref().host_waits.load(Ordering::Acquire), 0);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            assert_eq!(finishes.load(Ordering::Relaxed), 1);
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            assert_eq!(cut.progress[&Vec::new()], Progress::new(3, Phase::Done));
            assert_eq!(cut.progress[&vec![0]], Progress::new(132, Phase::Done));
        }
    }
}

struct ParkedClock {
    home: std::thread::ThreadId,
    now: bool,
    advance: bool,
    run: Arc<std::sync::atomic::AtomicPtr<crate::context::RunShared>>,
}
impl Adapter for ParkedClock {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        assert!(matches!(
            request,
            Request::SleepUntil {
                clock: 11,
                deadline: 100
            }
        ));
        if self.now {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn ready(&mut self) -> Result<crate::io::Ready, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        let run = self.run.load(Ordering::Acquire);
        assert!(!run.is_null());
        let guard = unsafe { role::IdleGuard::try_new(run) };
        assert!(guard.is_some(), "readiness probes hold no scheduler role");
        drop(guard);
        Ok(if self.now {
            crate::io::Ready::Every
        } else {
            crate::io::Ready::Tokens(vec![])
        })
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        self.now = self.advance;
        Ok(self.advance)
    }
}

unsafe extern "C" fn receiving_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let mut receiver = unsafe { crate::alloc::payload(env).cast::<u64>().read() };
    let mut image = [0u64; 2];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut receiver,
            image.as_mut_ptr().cast(),
        )
    };
    assert_eq!(status, crate::channels::CANCELED + CANCEL_DEADLINE);
    unsafe { crate::finalize::hirsel_rt_v2_close_handle(ctx, receiver, 0, 0) };
    STATUS_OK
}

fn parked_deadline(child: bool, advance: bool) {
    for width in [2, 8] {
        for durable in [false, true] {
            let recording =
                super::super::record::Recording::new(b"native parked deadline", durable);
            let install = recording.clone();
            let run_ptr = Arc::new(std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()));
            let install_run = run_ptr.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.free = true;
            run.adapter = Some(Box::new(ParkedClock {
                home: std::thread::current().id(),
                now: false,
                advance,
                run: run_ptr,
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
            let layouts = [&raw const layout];
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install_run.store((*ctx).run_ptr(), Ordering::Release);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter_until(
                        ctx,
                        std::ptr::null_mut(),
                        &mut scope,
                        &0,
                        &[11, 100],
                    );
                    let mut endpoints = [0u64; 2];
                    let mut image = [0u64; 2];
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        endpoints.as_mut_ptr(),
                        &0,
                        image.as_ptr().cast(),
                        2,
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
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut task,
                            &scope,
                            receiving_child,
                            env,
                            0,
                        );
                    } else {
                        let mut status = 99;
                        hirsel_rt_v2_channel_receive(
                            ctx,
                            0,
                            0,
                            &mut status,
                            &mut endpoints[1],
                            image.as_mut_ptr().cast(),
                        );
                        assert_eq!(status, crate::channels::CANCELED + CANCEL_DEADLINE);
                    }
                    let mut expired = 99;
                    hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut expired, &scope, 0);
                    assert_eq!(expired, 1);
                    for endpoint in if child {
                        vec![endpoints[0]]
                    } else {
                        endpoints.to_vec()
                    } {
                        crate::finalize::hirsel_rt_v2_close_handle(ctx, endpoint, 0, 0);
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
}

#[test]
fn a_deadline_wakes_the_last_saved_free_receiver() {
    parked_deadline(false, true);
}

#[test]
fn a_deadline_wakes_scope_exit_waiting_for_a_saved_free_child() {
    parked_deadline(true, true);
}

#[test]
fn an_armed_free_deadline_without_a_source_reports_blocked() {
    parked_deadline(false, false);
}
