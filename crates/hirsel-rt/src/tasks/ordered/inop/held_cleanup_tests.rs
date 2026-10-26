//! Exercise the runtime's Holding/rest contract directly: source task.main
//! deliberately drops its callable before TaskFinish (CONC-2), so a captured
//! resource cannot express failing post-finish rest in a source fixture.
use super::super::*;
use crate::tasks::ordered::engine;
use crate::tasks::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Probe {
    events: Vec<u64>,
    saved_some: usize,
    released: usize,
}

type Log = Mutex<Probe>;

unsafe fn log(env: *mut Header) -> &'static Log {
    let words = unsafe { crate::alloc::payload(env).cast::<u64>() };
    unsafe { &*(words.add(2).read() as *const Log) }
}

unsafe extern "C" fn drop_result(_: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    let log = unsafe { &*(value.cast::<u64>().read() as *const Log) };
    log.lock().unwrap().events.push(3);
    crate::STATUS_OK
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let log = unsafe { log(env) };
    let block =
        unsafe { crate::alloc::hirsel_rt_v2_alloc(ctx, 4, 8, 8, crate::alloc::DOMAIN_HEAP, 0) };
    unsafe {
        crate::alloc::payload(block)
            .cast::<u64>()
            .write(log as *const Log as u64)
    };
    let mut result = [1u64, block as u64];
    assert_eq!(
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                u32::MAX,
            )
        },
        crate::STATUS_OK
    );
    assert_eq!(result, [0, 0], "delivery consumes the child's owning Some");
    // TaskFinish returned into the child's rest, with the parent now owning
    // the result. Both cleanup operations yield before failing, like drops.
    for site in [11, 12] {
        unsafe { crate::finalize::hirsel_rt_v2_finalize_begin(ctx, 0) };
        let mut canceled = u32::MAX;
        assert_eq!(
            unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
            crate::STATUS_OK
        );
        assert_eq!(canceled, 0);
        log.lock().unwrap().events.push(site - 10);
        unsafe {
            crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 0, site, 0);
            crate::finalize::hirsel_rt_v2_finalize_end(ctx, 0);
        }
    }
    crate::STATUS_FAILURE
}

unsafe fn joined(ctx: *mut TaskCtx, result: *mut u64, log: &Log) -> u8 {
    assert_eq!(
        unsafe { result.read() },
        1,
        "failure must retain initialized Some"
    );
    let chain = unsafe { (*ctx).failure.take_chain(&[]) };
    assert_eq!(
        chain.iter().map(|(r, _)| r.site).collect::<Vec<_>>(),
        [11, 12]
    );
    assert!(chain.iter().all(|(_, spawn)| !spawn.is_empty()));
    assert_eq!(log.lock().unwrap().events, [1, 2]);
    let block = unsafe { result.add(1).read() as *mut Header };
    assert_eq!(
        unsafe { crate::release::hirsel_rt_v2_release(ctx, block, 4, std::ptr::null(), 0, 0) },
        crate::STATUS_OK
    );
    unsafe {
        result.write(0);
        result.add(1).write(0)
    };
    let mut value = [1u64, 7];
    unsafe {
        hirsel_rt_v2_task_finish(
            ctx,
            value.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            u32::MAX,
        )
    }
}

unsafe extern "C" fn stackful(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let [scope, mut child, _] = unsafe { crate::alloc::payload(env).cast::<[u64; 3]>().read() };
    let mut result = [0u64; 2];
    assert_eq!(
        unsafe {
            hirsel_rt_v2_task_join(
                ctx,
                0,
                0,
                &scope,
                &mut child,
                result.as_mut_ptr().cast(),
                16,
            )
        },
        crate::STATUS_FAILURE
    );
    unsafe { joined(ctx, result.as_mut_ptr(), log(env)) }
}

unsafe extern "C" fn compact(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 48) }
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    env: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    let words = frame.cast::<u64>();
    if unsafe { words.read() } == 0 {
        let [scope, child, _] = unsafe { crate::alloc::payload(env).cast::<[u64; 3]>().read() };
        unsafe {
            words.add(1).write(scope);
            words.add(2).write(child);
            words.add(3).write(0);
            words.add(4).write(0);
            words.write(1)
        };
    }
    unsafe { crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 32, 48, 0, 0) };
    let status = unsafe {
        hirsel_rt_v2_ordered_join(
            ctx,
            0,
            0,
            words.add(1),
            words.add(2),
            words.add(3).cast(),
            16,
            8,
        )
    };
    if status == crate::STATUS_SUSPENDED {
        return status;
    }
    assert_eq!(status, crate::STATUS_FAILURE);
    unsafe { joined(ctx, words.add(3), log(env)) }
}

#[test]
fn held_owning_result_survives_two_yielding_rest_failures_and_is_dropped_once() {
    for width in [1, 4] {
        for compact_parent in [false, true] {
            let environment = crate::Layout {
                size: 24,
                ..super::tests::layout()
            };
            let result = super::tests::layout();
            let owned = super::super::super::drops::tests::finalized(
                crate::Layout {
                    size: 8,
                    finalizer: Some(drop_result),
                    ..result
                },
                &[],
            );
            let layouts = [&environment as *const crate::Layout, &result, owned];
            let log = Arc::new(Log::default());
            let observe = log.clone();
            let release = log.clone();
            let address = Arc::as_ptr(&log) as u64;
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 3;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                    engine::driver(ctx).after_save = Some(Box::new(move |s, task, completed| {
                        if completed {
                            return;
                        }
                        let Logical::Ordered(d) = &s.logical else {
                            unreachable!()
                        };
                        let row = s.tasks[task as usize].as_ref().unwrap();
                        let id = handle(task, row.generation);
                        let Some(Operation::Join(j)) = d.in_operations.get(&id) else {
                            return;
                        };
                        if j.phase != JoinPhase::Retire {
                            return;
                        }
                        let Destination::Retained { site, offset, .. } = j.destination else {
                            return;
                        };
                        assert!(row.context_home.is_some());
                        assert!(row.stack.is_none());
                        let child = j.child.unwrap();
                        assert_ne!(s.tasks[child as usize].as_ref().unwrap().state, State::Done);
                        assert!(s.tasks[child as usize].as_ref().unwrap().slot.is_none());
                        assert_eq!(((site.frame + offset) as *const u64).read(), 1);
                        observe.lock().unwrap().saved_some += 1;
                    }));
                },
                move |ctx| unsafe {
                    let release = release.clone();
                    crate::pool::free::test_after_compact(ctx, move |_, _, pooled| {
                        assert!(pooled);
                        release.lock().unwrap().released += 1;
                    });
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        crate::STATUS_OK
                    );
                    let mut child_handle = 0;
                    {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            24,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<[u64; 3]>()
                            .write([scope, 0, address]);
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child_handle,
                                &scope,
                                child,
                                env,
                                0
                            ),
                            crate::STATUS_OK
                        );
                    }
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        24,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env).cast::<[u64; 3]>().write([
                        scope,
                        child_handle,
                        address,
                    ]);
                    let mut parent = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut parent,
                            &scope,
                            if compact_parent { compact } else { stackful },
                            env,
                            0
                        ),
                        crate::STATUS_OK
                    );
                    let mut result = [0u64; 2];
                    assert_eq!(
                        hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            &mut parent,
                            result.as_mut_ptr().cast(),
                            16
                        ),
                        crate::STATUS_OK
                    );
                    assert_eq!(result, [1, 7]);
                    assert!(engine::driver(ctx).in_operations.is_empty());
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        crate::STATUS_OK
                    );
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let log = log.lock().unwrap();
            assert_eq!(log.events, [1, 2, 3]);
            if compact_parent {
                assert!(log.saved_some > 0);
                assert!(log.released > 0);
            }
        }
    }
}
