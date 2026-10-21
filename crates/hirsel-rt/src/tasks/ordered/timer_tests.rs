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
