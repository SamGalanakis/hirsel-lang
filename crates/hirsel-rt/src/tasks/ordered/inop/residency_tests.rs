//! A latch opens only after every host operation has parked from its frame.
use super::super::*;
use crate::io::{AdapterError, Completion, Ready, Request};
use crate::io_concurrent::ConcurrentAdapter;
use crate::tasks::*;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Latch {
    released: AtomicBool,
    calls: Mutex<BTreeMap<u64, usize>>,
    finishes: AtomicUsize,
    peak_parked: AtomicUsize,
    peak_stacks: AtomicUsize,
}
impl ConcurrentAdapter for Latch {
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
        *self.calls.lock().unwrap().entry(token).or_default() += 1;
        if self.released.load(Ordering::Acquire) {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn ready(&self) -> Result<Ready, AdapterError> {
        Ok(if self.released.load(Ordering::Acquire) {
            Ready::Every
        } else {
            Ready::Tokens(vec![])
        })
    }
    fn finish(&self, _: u64) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn eight_sixteen_and_thirty_two_entered_host_waits_release_their_stacks_before_readiness() {
    for width in [1, 4] {
        for count in [8, 16, 32] {
            let environment = super::tests::layout();
            let result = super::tests::layout();
            let layouts = [&environment as *const crate::Layout, &result];
            let host = Arc::new(Latch::default());
            let observed = host.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 2;
            run.concurrent_adapter = Some(host.clone());
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(move |ctx| unsafe {
                engine::enable(ctx, crate::ordered::Policy::admit(&Default::default(), None));
                engine::driver(ctx).after_save = Some(Box::new(move |s, _, _| {
                    let Logical::Ordered(driver) = &s.logical else { unreachable!() };
                    let mut parked = 0;
                    let mut stacks = 0;
                    for (index, row) in s.tasks.iter().enumerate() {
                        let Some(row) = row else { continue };
                        if matches!(row.wait, Some(Wait::Host { .. })) {
                            let key = handle(index as u32, row.generation);
                            if matches!(driver.in_operations.get(&key), Some(Operation::Host(h)) if h.binding.is_some()) {
                                parked += 1;
                                stacks += usize::from(row.stack.is_some());
                                if row.stack.is_none() { assert!(row.context_home.is_some()); }
                            }
                        }
                    }
                    observed.peak_parked.fetch_max(parked, Ordering::Relaxed);
                    observed.peak_stacks.fetch_max(stacks, Ordering::Relaxed);
                    if parked == count && stacks == 0 {
                        observed.released.store(true, Ordering::Release);
                    }
                }));
            }, move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0), STATUS_OK);
                let mut children = Vec::new();
                for _ in 0..count {
                    let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                    crate::alloc::payload(env).cast::<[u64;2]>().write([0,0]);
                    let mut child = 0;
                    assert_eq!(hirsel_rt_v2_task_spawn(ctx, 0, 0, std::ptr::null_mut(), &mut child, &scope, super::tests::compact_sleeper, env, 0), STATUS_OK);
                    children.push(child);
                }
                for mut child in children {
                    let mut result = [0u64;2];
                    assert_eq!(hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut child, result.as_mut_ptr().cast(), 16), STATUS_OK);
                    assert_eq!(result, [1,7]);
                }
                assert!(engine::driver(ctx).in_operations.is_empty());
                assert!(engine::driver(ctx).host_ops.is_empty());
                assert_eq!(hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), STATUS_OK);
                STATUS_OK
            });
            assert_eq!(outcome.ended, Ok(STATUS_OK), "width={width} count={count}");
            assert_eq!(outcome.pending, 0);
            assert!(host.released.load(Ordering::Acquire));
            assert_eq!(host.peak_parked.load(Ordering::Relaxed), count);
            assert!(host.peak_stacks.load(Ordering::Relaxed) <= width as usize);
            assert_eq!(host.finishes.load(Ordering::Relaxed), count);
            let calls = host.calls.lock().unwrap();
            assert_eq!(calls.len(), count);
            assert!(calls.values().all(|&calls| calls == 2), "{calls:?}");
            eprintln!(
                "width={width} entered parked={count} peak parked stacks={}",
                host.peak_stacks.load(Ordering::Relaxed)
            );
        }
    }
}
