use super::super::*;
use crate::tasks::ordered::events;
use crate::tasks::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug)]
enum Case {
    Holding,
    Done,
    HoldingDuringYield,
    DoneDuringYield,
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let [failed, during, _] = unsafe { crate::alloc::payload(env).cast::<[u64; 3]>().read() };
    if during != 0 {
        for _ in 0..2 {
            let mut cause = 0;
            assert_eq!(
                unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                crate::STATUS_OK
            );
        }
    }
    if failed != 0 {
        unsafe { crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 0, 33, 0) };
        crate::STATUS_FAILURE
    } else {
        unsafe { super::tests::finish_value(ctx, 7) }
    }
}

unsafe fn deliver(ctx: *mut TaskCtx, env: *mut Header, slot: *mut u64, status: u8) -> u8 {
    let failed = unsafe { crate::alloc::payload(env).cast::<u64>().add(2).read() } != 0;
    assert_eq!(
        status,
        if failed {
            crate::STATUS_FAILURE
        } else {
            crate::STATUS_OK
        }
    );
    if failed {
        assert_eq!(unsafe { slot.cast::<[u64; 2]>().read() }, [0, 0]);
        let chain = unsafe { (*ctx).failure.take_chain(&[]) };
        assert_eq!(chain.iter().map(|(r, _)| r.site).collect::<Vec<_>>(), [33]);
    } else {
        assert_eq!(unsafe { slot.cast::<[u64; 2]>().read() }, [1, 7]);
    }
    unsafe { super::tests::finish_value(ctx, 7) }
}

unsafe extern "C" fn stackful(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let [scope, mut child, _] = unsafe { crate::alloc::payload(env).cast::<[u64; 3]>().read() };
    let mut slot = [0u64; 2];
    let status = unsafe {
        hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut child, slot.as_mut_ptr().cast(), 16)
    };
    unsafe { deliver(ctx, env, slot.as_mut_ptr(), status) }
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
            words.write(1);
        }
        unsafe { refuse_unmovable(ctx, frame) };
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
    unsafe { deliver(ctx, env, words.add(3), status) }
}

unsafe fn refuse_unmovable(ctx: *mut TaskCtx, frame: *mut u8) {
    use crate::continuation::binding;
    unsafe { binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 32, 48, 0, 0) };
    let _role = unsafe { role::Guard::new(ctx) };
    let site = unsafe { binding::take_bound(ctx, 32).unwrap() };
    assert!(unsafe { binding::can_leave(ctx, site) });
    for owner in [Owner::Home, Owner::Pool(0), Owner::Compute(1)] {
        let original = role::with_role(ctx, |s| {
            let me = s.current;
            std::mem::replace(&mut s.task(me).owner, owner)
        });
        assert!(!unsafe { binding::can_leave(ctx, site) });
        role::with_role(ctx, |s| {
            let me = s.current;
            s.task(me).owner = original;
        });
    }
    for pinned in [true, false] {
        role::with_role(ctx, |s| {
            let me = s.current;
            if pinned {
                s.task(me).pinned = true;
            } else {
                s.task(me).inline_running = true;
            }
        });
        assert!(!unsafe { binding::can_leave(ctx, site) });
        role::with_role(ctx, |s| {
            let me = s.current;
            s.task(me).pinned = false;
            s.task(me).inline_running = false;
        });
    }
    // A parent-lent activation has no stack owned by this task.
    let stack = role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me).stack.take()
    });
    assert!(!unsafe { binding::can_leave(ctx, site) });
    role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me).stack = stack;
    });
    unsafe { (*ctx).callback_depth += 1 };
    assert!(!unsafe { binding::can_leave(ctx, site) });
    unsafe { (*ctx).callback_depth -= 1 };
    assert!(unsafe { binding::can_leave(ctx, site) });
    unsafe { binding::complete_bound(ctx, site) };
}

unsafe fn spawn(ctx: *mut TaskCtx, scope: u64, code: TaskCode, env: *mut Header) -> u64 {
    let mut child = 0;
    assert_eq!(
        unsafe {
            hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                &mut child,
                &scope,
                code,
                env,
                0,
            )
        },
        crate::STATUS_OK
    );
    child
}

#[test]
fn holding_done_and_completion_during_first_yield_preserve_the_original_join_branch() {
    for case in [
        Case::Holding,
        Case::Done,
        Case::HoldingDuringYield,
        Case::DoneDuringYield,
    ] {
        let during = matches!(case, Case::HoldingDuringYield | Case::DoneDuringYield);
        let failed = matches!(case, Case::Done | Case::DoneDuringYield);
        let mut baseline = None;
        for width in [1, 4] {
            for retained in [false, true] {
                let layout = crate::Layout {
                    size: 24,
                    ..super::tests::layout()
                };
                let layouts = [&layout as *const crate::Layout, &layout];
                let saved = Arc::new(Mutex::new(Vec::new()));
                let observe = saved.clone();
                let rows = Arc::new(Mutex::new(Vec::new()));
                let capture = rows.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        engine::enable(
                            ctx,
                            crate::ordered::Policy::admit(&Default::default(), None),
                        );
                        events::enable(ctx);
                        engine::driver(ctx).after_save = Some(Box::new(move |s, task, _| {
                            let Logical::Ordered(d) = &s.logical else {
                                unreachable!()
                            };
                            let row = s.tasks[task as usize].as_ref().unwrap();
                            if let Some(Operation::Join(j)) =
                                d.in_operations.get(&handle(task, row.generation))
                            {
                                if matches!(j.phase, JoinPhase::Prepared | JoinPhase::Delivered) {
                                    return;
                                }
                                if matches!(j.destination, Destination::Retained { .. }) {
                                    assert!(row.context_home.is_some());
                                    assert!(row.stack.is_none());
                                    observe.lock().unwrap().push((
                                        j.phase,
                                        s.tasks[j.child.unwrap() as usize].as_ref().unwrap().state,
                                    ));
                                }
                            }
                        }));
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
                            crate::STATUS_OK
                        );
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
                            0,
                            u64::from(failed),
                        ]);
                        let child_env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            24,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(child_env).cast::<[u64; 3]>().write([
                            u64::from(failed),
                            u64::from(during),
                            0,
                        ]);
                        let child_id = spawn(ctx, scope, child, child_env);
                        if !during {
                            let mut cause = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_canceled(ctx, &mut cause, &0),
                                crate::STATUS_OK
                            );
                            role::with_role(ctx, |s| {
                                assert_eq!(
                                    s.task(child_id as u32).state,
                                    if failed { State::Done } else { State::Holding }
                                )
                            });
                        }
                        crate::alloc::payload(env)
                            .cast::<u64>()
                            .add(1)
                            .write(child_id);
                        let mut joining =
                            spawn(ctx, scope, if retained { compact } else { stackful }, env);
                        let mut result = [0u64; 2];
                        assert_eq!(
                            hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                &mut joining,
                                result.as_mut_ptr().cast(),
                                16
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(result, [1, 7]);
                        let _role = role::Guard::new(ctx);
                        let d = engine::driver(ctx);
                        assert!(d.in_operations.is_empty());
                        *capture.lock().unwrap() = d
                            .events
                            .as_ref()
                            .unwrap()
                            .boundaries
                            .iter()
                            .filter(|b| b.task == vec![1])
                            .cloned()
                            .collect();
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                            crate::STATUS_OK
                        );
                        crate::STATUS_OK
                    },
                );
                assert_eq!(
                    outcome.ended,
                    Ok(crate::STATUS_OK),
                    "{case:?}/{width}/{retained}"
                );
                assert_eq!(outcome.pending, 0);
                let rows = rows.lock().unwrap();
                assert_eq!(rows.iter().filter(|b| b.op == Some("task.join")).count(), 1);
                assert_eq!(
                    rows.iter()
                        .filter(|b| b.transition == events::Transition::Park)
                        .count(),
                    usize::from(during)
                );
                if retained && during {
                    let saved = saved.lock().unwrap();
                    assert!(
                        saved.contains(&(
                            JoinPhase::Child,
                            if failed { State::Done } else { State::Holding }
                        )),
                        "mandatory park was not physically saved: {case:?} {:?}",
                        *saved
                    );
                }
                if let Some(expected) = &baseline {
                    assert_eq!(&*rows, expected, "{case:?}/{width}/{retained}");
                } else {
                    baseline = Some(rows.clone());
                }
            }
        }
    }
}
