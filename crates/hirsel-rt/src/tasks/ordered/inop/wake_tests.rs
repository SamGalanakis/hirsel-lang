//! Durable provider readiness at the three registration/save windows. Polling
//! owns the retry; duplicate and retired tokens cannot deliver twice.
use super::super::*;
use crate::io::{AdapterError, Completion, Ready, Request};
use crate::io_concurrent::ConcurrentAdapter;
use crate::tasks::*;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug)]
enum Window {
    BeforeRegistration,
    DuringRegistration,
    AfterSave,
}
struct Host {
    window: Window,
    run: AtomicPtr<crate::context::RunShared>,
    current: AtomicU64,
    retired: AtomicU64,
    ready: AtomicBool,
    driver: crate::readiness::Readiness,
    attempts: Mutex<Vec<u64>>,
    finishes: AtomicUsize,
    parked: AtomicUsize,
    saved: Mutex<BTreeMap<u64, u32>>,
}
impl Host {
    fn signal(&self) {
        self.ready.store(true, Ordering::Release);
        self.driver.notify().unwrap();
        self.driver.notify().unwrap();
    }
}
impl ConcurrentAdapter for Host {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        assert!(matches!(
            request,
            Request::SleepUntil {
                clock: 11,
                deadline: 10
            }
        ));
        let mut attempts = self.attempts.lock().unwrap();
        let first = !attempts.contains(&token);
        attempts.push(token);
        if first {
            self.current.store(token, Ordering::Release);
            self.ready.store(false, Ordering::Release);
            if matches!(self.window, Window::BeforeRegistration) {
                self.signal();
            }
            return Err(AdapterError::Blocked);
        }
        assert!(self.ready.load(Ordering::Acquire));
        let run = self.run.load(Ordering::Acquire);
        let _role = unsafe { role::IdleGuard::new(run) };
        let s = unsafe { (*(*run).tasks.get()).as_deref().unwrap() };
        let row = s
            .tasks
            .iter()
            .flatten()
            .find(|row| matches!(row.wait, Some(Wait::Host {token:t}) if t & !(1u64<<63) == token))
            .expect("registered owner");
        assert!(
            row.context_home.is_some(),
            "retry before first compact save"
        );
        assert!(row.stack.is_none(), "retry while owner still has its stack");
        Ok(Completion::Wait(Ok(())))
    }
    fn parked(&self, token: u64) {
        assert_eq!(token, self.current.load(Ordering::Acquire));
        self.parked.fetch_add(1, Ordering::Relaxed);
        if matches!(self.window, Window::DuringRegistration) {
            self.signal();
        }
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        let old = self.retired.load(Ordering::Acquire);
        Ok(if self.ready.load(Ordering::Acquire) {
            let token = self.current.load(Ordering::Acquire);
            Ready::Tokens(vec![old, old, token, token])
        } else {
            Ready::Tokens(vec![old, old])
        })
    }
    fn readiness(&self) -> Option<crate::readiness::Readiness> {
        Some(self.driver.clone())
    }
    fn finish(&self, token: u64) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
        self.retired.store(token, Ordering::Release);
    }
}

#[test]
fn readiness_before_registration_during_registration_and_after_save_keeps_one_original_completion()
{
    for width in [1, 4] {
        for window in [
            Window::BeforeRegistration,
            Window::DuringRegistration,
            Window::AfterSave,
        ] {
            let environment = super::tests::layout();
            let result = super::tests::layout();
            let layouts = [&environment as *const crate::Layout, &result];
            let host = Arc::new(Host {
                window,
                run: AtomicPtr::default(),
                current: AtomicU64::new(0),
                retired: AtomicU64::new(0),
                ready: AtomicBool::new(false),
                driver: Default::default(),
                attempts: Default::default(),
                finishes: AtomicUsize::new(0),
                parked: AtomicUsize::new(0),
                saved: Default::default(),
            });
            let setup = host.clone();
            let observe = host.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 2;
            run.concurrent_adapter = Some(host.clone());
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                    setup.run.store((*ctx).run_ptr(), Ordering::Release);
                    engine::driver(ctx).after_save = Some(Box::new(move |s, task, completed| {
                        if completed {
                            return;
                        }
                        let row = s.tasks[task as usize].as_ref().unwrap();
                        if let Some(Wait::Host { token }) = row.wait {
                            assert!(row.context_home.is_some());
                            assert!(row.stack.is_none());
                            observe
                                .saved
                                .lock()
                                .unwrap()
                                .insert(token & !(1u64 << 63), row.generation);
                            if matches!(window, Window::AfterSave) {
                                observe.signal();
                            }
                        }
                    }));
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
                    );
                    let mut prior = None;
                    for _ in 0..2 {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env).cast::<[u64; 2]>().write([0, 0]);
                        let mut child = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                super::tests::compact_sleeper,
                                env,
                                0
                            ),
                            STATUS_OK
                        );
                        let key = TaskKey::from_word(child);
                        if let Some(old) = prior {
                            let old: TaskKey = old;
                            assert_eq!(key.slot, old.slot);
                            assert!(key.generation > old.generation);
                        }
                        prior = Some(key);
                        let mut result = [0u64; 2];
                        assert_eq!(
                            hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                &mut child,
                                result.as_mut_ptr().cast(),
                                16
                            ),
                            STATUS_OK
                        );
                        assert_eq!(result, [1, 7]);
                    }
                    assert!(engine::driver(ctx).in_operations.is_empty());
                    assert!(engine::driver(ctx).host_ops.is_empty());
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    STATUS_OK
                },
            );
            assert_eq!(
                outcome.ended,
                Ok(STATUS_OK),
                "{window:?} width={width} attempts={:?} parked={} ready={} saved={:?}",
                host.attempts.lock().unwrap(),
                host.parked.load(Ordering::Relaxed),
                host.ready.load(Ordering::Acquire),
                host.saved.lock().unwrap()
            );
            assert_eq!(outcome.pending, 0);
            assert_eq!(*host.attempts.lock().unwrap(), [1, 1, 2, 2]);
            assert_eq!(host.finishes.load(Ordering::Relaxed), 2);
            assert_eq!(host.parked.load(Ordering::Relaxed), 2);
            // Saved publication is observed even when readiness predates it.
            assert_eq!(host.saved.lock().unwrap().len(), 2);
        }
    }
}
