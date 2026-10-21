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
