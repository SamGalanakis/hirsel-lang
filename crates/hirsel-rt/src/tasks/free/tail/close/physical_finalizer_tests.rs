//! Stand-in for checked native nested finalizer emission, on real dispatch.
use super::*;
use crate::tasks::free::objects::tests::{isolated_at, plain};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

#[derive(Default)]
struct Seen {
    starts: AtomicU32,
    finishes: AtomicU32,
    leaves: AtomicU32,
    contexts: Mutex<Vec<(u32, usize)>>,
}

unsafe extern "C" fn ordinary(_: *mut TaskCtx, _: *mut u8, _: u64, _: u64) -> u8 {
    panic!("ordinary finalizer restarted instead of its retained alternate")
}

unsafe extern "C" fn finalizer(
    ctx: *mut TaskCtx,
    value: *mut u8,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        let seen = &**value.cast::<*const Seen>();
        let count = value.add(8).cast::<u64>();
        let pc = frame.cast::<u32>();
        seen.contexts
            .lock()
            .unwrap()
            .push((pc.read(), ctx as usize));
        assert_eq!((*ctx).shield, 1);
        assert_eq!((*ctx).cleanup_invocations.len(), 1);
        if pc.read() == 0 {
            assert_eq!(count.read(), 0);
            count.write(1);
            seen.starts.fetch_add(1, Ordering::Relaxed);
            let site = (*ctx).free_ops.tails.cleanup_sites.last().unwrap().0;
            assert!(crate::continuation::binding::can_leave(ctx, site));
            // Exhaust the worker's existing budget without adding language
            // effects, so this one original yield deterministically saves.
            crate::pool::free::renew(ctx);
            for _ in 0..127 {
                assert!(crate::pool::free::proceed(ctx));
            }
            let pending = crate::tasks::free::try_yield_entered(&mut *ctx, true).unwrap();
            assert!(pending);
            pc.write(1);
            return crate::STATUS_SUSPENDED;
        }
        assert_eq!(pc.read(), 1);
        assert_eq!(count.read(), 1);
        count.write(2);
        seen.finishes.fetch_add(1, Ordering::Relaxed);
        pc.write(2);
        STATUS_OK
    }
}

unsafe fn resolve(ctx: *mut TaskCtx, token: Finalizer, root: usize) -> Option<ResolvedFinalizer> {
    assert_eq!(token.layout, unsafe { (*ctx).descriptor(2) });
    Some(ResolvedFinalizer {
        dispatch: finalizer,
        frame: unsafe { (root as *mut u8).add(64) },
        bytes: 16,
    })
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 96) }
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    env: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        let pc = frame.cast::<u32>();
        let result = frame.add(16).cast::<[u64; 2]>();
        if pc.read() != 2 {
            let first = pc.read() == 0;
            if first {
                pc.write(1);
            }
            crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 50, 96, 31, 41);
            let site = crate::continuation::binding::take_bound(ctx, 50).unwrap();
            let index = if first {
                let seen = crate::alloc::payload(env).cast::<*const Seen>().read();
                let resource =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                let words = crate::alloc::payload(resource);
                words.cast::<*const Seen>().write(seen);
                words.add(8).cast::<u64>().write(0);
                let cursor = Box::new(Cursor::new(
                    (*ctx).work,
                    resource,
                    ptr::null(),
                    false,
                    31,
                    41,
                ));
                let index = attach_finalizers(
                    ctx,
                    cursor,
                    31,
                    41,
                    FinalizerResolver {
                        resolve,
                        retained: frame as usize,
                    },
                );
                crate::release::push(&mut *ctx, resource, true);
                (*ctx).free_ops.tails.cleanup_sites.push((site, index));
                index
            } else {
                let (original, index) = *(*ctx).free_ops.tails.cleanup_sites.last().unwrap();
                assert_eq!(original, site, "resumption retains the original invocation");
                index
            };
            match poll_finalizers(ctx, index).unwrap() {
                CleanupPoll::Pending(CleanupPending::Nested) => return crate::STATUS_SUSPENDED,
                CleanupPoll::Ready(status) => assert_eq!(status, STATUS_OK),
                _ => panic!("finalizer cannot replace its own pending wait"),
            }
            assert_eq!(
                (*ctx).free_ops.tails.cleanup_sites.pop(),
                Some((site, index))
            );
            crate::continuation::binding::complete_bound(ctx, site);
            assert_eq!((*ctx).shield, 0);
            assert_eq!((*ctx).cleanup_invocations.len(), 0);
            assert!((*ctx).work.is_null());
            result.write([1, 7]);
            pc.write(2);
        }
        crate::tasks::hirsel_rt_v2_task_finish(
            ctx,
            result.cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            u32::MAX,
        )
    }
}

#[test]
fn nested_finalizer_returns_its_stack_and_resumes_original_admission_after_relocation() {
    if !isolated_at(
        "tasks::free::tail::close::physical_finalizer_tests::nested_finalizer_returns_its_stack_and_resumes_original_admission_after_relocation",
        "HIRSEL_ISOLATE_FINALIZER_PHYSICAL",
    ) {
        return;
    }
    run_case(false);
}

#[test]
fn a_host_stop_after_finalizer_save_abandons_cursor_without_resuming_language_code() {
    if !isolated_at(
        "tasks::free::tail::close::physical_finalizer_tests::a_host_stop_after_finalizer_save_abandons_cursor_without_resuming_language_code",
        "HIRSEL_ISOLATE_FINALIZER_PHYSICAL_STOP",
    ) {
        return;
    }
    run_case(true);
}

fn run_case(host_stop: bool) {
    for width in [1, 4] {
        let resource = crate::tasks::free::finalized(crate::Layout {
            finalizer: Some(ordinary),
            ..plain(16)
        });
        let environment = plain(8);
        let layouts = [resource as *const crate::Layout, &environment];
        let seen = Arc::new(Seen::default());
        let observed = seen.clone();
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        unsafe { run.use_physical_dispatch() };
        let control = host_stop.then(|| run.control());
        let stopping = control.clone();
        let outcome = run.run(
            |ctx| unsafe {
                crate::tasks::free::enable(ctx);
            },
            move |ctx| unsafe {
                let hook = seen.clone();
                let stopping = stopping.clone();
                crate::pool::free::test_after_compact(ctx, move |task, _, pooled| {
                    if task != 0 {
                        assert!(pooled);
                        hook.leaves.fetch_add(1, Ordering::Relaxed);
                        if let Some(control) = &stopping {
                            // Saved ownership has left the native stack. Stop
                            // while the original finalizer and cursor are live.
                            control.stop();
                        }
                    }
                });
                let mut scope = 0;
                assert_eq!(
                    crate::tasks::hirsel_rt_v2_task_scope_enter(
                        ctx,
                        ptr::null_mut(),
                        &mut scope,
                        &0
                    ),
                    STATUS_OK
                );
                let mut children = [0u64; 4];
                for task in &mut children {
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        3,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<*const Seen>()
                        .write(Arc::as_ptr(&seen));
                    assert_eq!(
                        crate::tasks::hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            ptr::null_mut(),
                            task,
                            &scope,
                            child,
                            env,
                            0
                        ),
                        STATUS_OK
                    );
                }
                for task in &mut children {
                    let mut output = [0u64; 2];
                    assert_eq!(
                        crate::tasks::hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            task,
                            output.as_mut_ptr().cast(),
                            16
                        ),
                        STATUS_OK
                    );
                    assert_eq!(output, [1, 7]);
                }
                assert_eq!(
                    crate::tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                STATUS_OK
            },
        );
        if let Some(control) = control {
            assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while control.status().reclamation_pending != 0 {
                assert!(
                    std::time::Instant::now() < until,
                    "stopped finalizer frames reclaim"
                );
                std::thread::yield_now();
            }
            assert!(control.status().finished);
            assert_eq!(control.status().lost_permits, 0);
            assert!(observed.starts.load(Ordering::Acquire) >= 1);
            assert_eq!(observed.finishes.load(Ordering::Acquire), 0);
            assert!(observed.leaves.load(Ordering::Acquire) >= 1);
            assert!(
                observed
                    .contexts
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|(pc, _)| *pc == 0),
                "host stop cannot resume finalizer code or run ordinary destruction"
            );
            continue;
        }
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(observed.starts.load(Ordering::Acquire), 4);
        assert_eq!(observed.finishes.load(Ordering::Acquire), 4);
        assert!(observed.leaves.load(Ordering::Acquire) >= 4);
        let contexts = observed.contexts.lock().unwrap();
        let starts: std::collections::BTreeSet<_> = contexts
            .iter()
            .filter(|(pc, _)| *pc == 0)
            .map(|(_, ctx)| *ctx)
            .collect();
        assert!(
            contexts
                .iter()
                .any(|(pc, ctx)| *pc == 1 && !starts.contains(ctx)),
            "no finalizer moved off its original stack"
        );
    }
}
