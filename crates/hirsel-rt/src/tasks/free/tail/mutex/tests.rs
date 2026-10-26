use super::*;
use crate::tasks::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const NONE: [u64; 2] = [0, 0];

struct Shared {
    handle: u64,
    kind: Kind,
    task: AtomicU32,
    callback: AtomicU32,
    polls: AtomicU32,
    go: AtomicBool,
    saved: AtomicBool,
    force_yield: bool,
    yields: AtomicU32,
    ticket: Mutex<Option<Ticket>>,
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    env: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        let shared = &**crate::alloc::payload(env).cast::<*const Shared>();
        let payload = frame.add(16).cast::<u64>();
        let state = frame.cast::<u32>().read();
        if state == 2 {
            return hirsel_rt_v2_task_finish(
                ctx,
                payload.cast(),
                NONE.as_ptr().cast(),
                16,
                u32::MAX,
            );
        }
        let _role = role::Guard::new(ctx);
        if state == 0 {
            shared.task.store((*ctx).task_id, Ordering::Release);
            let mut dead = 99;
            assert_eq!(
                hirsel_rt_v2_mutex_take(ctx, &mut dead, &shared.handle, payload.cast()),
                STATUS_OK
            );
            match shared.kind {
                Kind::Put => {
                    assert_eq!((dead, payload.add(1).read()), (0, 7));
                    shared.callback.fetch_add(1, Ordering::Relaxed);
                    payload.add(1).write(8);
                }
                Kind::Kill => {
                    assert_eq!(dead, 0);
                    shared.callback.fetch_add(1, Ordering::Relaxed);
                    crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 11, 101, 201);
                }
                Kind::Cause => {
                    assert_eq!(dead, 1);
                    crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 13, 103, 203);
                }
                _ => unreachable!(),
            }
            frame.cast::<u32>().write(1);
            let ticket = prepare_mutex(
                ctx,
                Site {
                    object: (crate::free::MUTEX, mutex_word(&shared.handle)),
                    kind: shared.kind,
                    anchor: 31,
                    operation: 41,
                },
                if shared.kind == Kind::Put {
                    payload.cast()
                } else {
                    std::ptr::null_mut()
                },
            );
            *shared.ticket.lock().unwrap() = Some(ticket);
            if shared.kind == Kind::Put {
                assert_eq!(std::slice::from_raw_parts(payload, 2), NONE);
            }
        }
        let ticket = shared.ticket.lock().unwrap().unwrap();
        if shared.force_yield
            && shared.go.load(Ordering::Acquire)
            && !(*ctx)
                .free_ops
                .tails
                .active(crate::tasks::running_key(ctx), ticket)
                .suffix_done
        {
            crate::pool::free::renew(ctx);
            for _ in 1..hirsel_free::schedule::OP_BUDGET {
                assert!(crate::pool::free::proceed(ctx));
            }
        }
        let result = poll_mutex_with(ctx, ticket, shared.force_yield, |_, entered| {
            shared.polls.fetch_add(1, Ordering::Release);
            if !*entered {
                record::publish(ctx, Phase::Entered, |_| ())?;
                *entered = true;
            }
            Ok(shared.go.load(Ordering::Acquire))
        })
        .unwrap();
        if result.is_pending() {
            if (*ctx)
                .free_ops
                .tails
                .active(crate::tasks::running_key(ctx), ticket)
                .yielded
            {
                shared.yields.fetch_add(1, Ordering::Relaxed);
                return crate::STATUS_SUSPENDED;
            }
            crate::tasks::park_left(ctx, Wait::Retry { cancelable: false }).unwrap();
            crate::tasks::compact::wait(ctx);
            return crate::STATUS_SUSPENDED;
        }
        assert_eq!(result, Poll::Ready(STATUS_OK));
        if shared.kind == Kind::Cause {
            assert_eq!(
                (*ctx)
                    .failure
                    .chain(&[])
                    .iter()
                    .map(|(r, _)| r.rule)
                    .collect::<Vec<_>>(),
                [13, 11]
            );
        }
        if shared.kind != Kind::Put {
            crate::failure::hirsel_rt_v2_failure_clear(ctx);
        }
        payload.write(1);
        payload.add(1).write(108);
        frame.cast::<u32>().write(2);
        hirsel_rt_v2_task_finish(ctx, payload.cast(), NONE.as_ptr().cast(), 16, u32::MAX)
    }
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 48) }
}

#[test]
fn owned_put_kill_cause_keep_original_effect_and_failure_order_off_released_stacks() {
    controls(false);
}

#[test]
fn owned_completed_put_and_kill_resume_the_final_yield_without_reapplying_the_effect() {
    controls(true);
}

fn controls(force_yield: bool) {
    use crate::tasks::free::objects::tests::{
        Layouts, isolated_at, mutex as make_mutex, plain, wait_until, wrapper,
    };
    let name = if force_yield {
        "tasks::free::tail::mutex::tests::owned_completed_put_and_kill_resume_the_final_yield_without_reapplying_the_effect"
    } else {
        "tasks::free::tail::mutex::tests::owned_put_kill_cause_keep_original_effect_and_failure_order_off_released_stacks"
    };
    if !isolated_at(name, "HIRSEL_OWNED_MUTEX_TAILS") {
        return;
    }
    let layouts = Layouts::new(Box::leak(Box::new(plain(16))), wrapper());
    for kind in [Kind::Put, Kind::Kill, Kind::Cause] {
        if force_yield && kind == Kind::Cause {
            continue;
        }
        for durable in [false, true] {
            let recording = record::Recording::new(b"owned mutex tail controls", durable);
            let install = recording.clone();
            let mut run = crate::host::BodyRun::new(4, false);
            run.free = true;
            let mut pointers = Vec::new();
            layouts.install(&mut run, &mut pointers);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    crate::tasks::free::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let handle = make_mutex(ctx, 7);
                    if kind == Kind::Cause {
                        let mut value = NONE;
                        let mut status = 99;
                        hirsel_rt_v2_mutex_take(
                            ctx,
                            &mut status,
                            &handle,
                            value.as_mut_ptr().cast(),
                        );
                        crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 11, 101, 201);
                        hirsel_rt_v2_mutex_kill(ctx, &handle);
                        crate::failure::hirsel_rt_v2_failure_clear(ctx);
                    }
                    let shared = Arc::new(Shared {
                        handle,
                        kind,
                        task: AtomicU32::new(u32::MAX),
                        callback: AtomicU32::new(0),
                        polls: AtomicU32::new(0),
                        go: AtomicBool::new(false),
                        saved: AtomicBool::new(false),
                        force_yield,
                        yields: AtomicU32::new(0),
                        ticket: Mutex::default(),
                    });
                    let hook = shared.clone();
                    crate::pool::free::test_after_compact(ctx, move |task, _, pooled| {
                        if task == hook.task.load(Ordering::Acquire) {
                            assert!(pooled);
                            hook.saved.store(true, Ordering::Release);
                        }
                    });
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        4,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<*const Shared>()
                        .write(Arc::as_ptr(&shared));
                    let mut task = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut task,
                        &scope,
                        child,
                        env,
                        0,
                    );
                    wait_until("owned tail physically saved", || {
                        shared.saved.load(Ordering::Acquire)
                    });
                    let token = {
                        let _role = role::Guard::new(ctx);
                        role::with_held(ctx, |s| {
                            let me = shared.task.load(Ordering::Acquire);
                            assert!(s.task(me).stack.is_none(), "a tail wait holds no stack");
                            crate::pool::free::token((*ctx).run_ptr(), s.key(me), Owner::Any)
                                .unwrap()
                        })
                    };
                    crate::pool::free::notify((*ctx).run_ptr(), &token);
                    crate::pool::free::notify((*ctx).run_ptr(), &token);
                    wait_until("spurious tail wake rechecks", || {
                        shared.polls.load(Ordering::Acquire) >= 2
                    });
                    assert_eq!(
                        shared.callback.load(Ordering::Relaxed),
                        u32::from(kind != Kind::Cause)
                    );
                    shared.go.store(true, Ordering::Release);
                    crate::pool::free::notify((*ctx).run_ptr(), &token);
                    drop(token);
                    let mut value = NONE;
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        value.as_mut_ptr().cast(),
                        16,
                    );
                    assert_eq!(value, [1, 108]);
                    assert_eq!(
                        shared.yields.load(Ordering::Relaxed),
                        u32::from(force_yield),
                        "the completed effect leaves exactly once for its final yield"
                    );
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                    if kind == Kind::Put {
                        let mut status = 99;
                        hirsel_rt_v2_mutex_take(
                            ctx,
                            &mut status,
                            &handle,
                            value.as_mut_ptr().cast(),
                        );
                        assert_eq!((status, value), (0, [1, 8]));
                        hirsel_rt_v2_mutex_put(ctx, &handle, value.as_mut_ptr().cast());
                    }
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK), "{kind:?} durable={durable}");
            recording.recorder.stop("returned", None).unwrap().unwrap();
            let name = match kind {
                Kind::Put => "put",
                Kind::Kill => "kill",
                Kind::Cause => "cause",
                _ => unreachable!(),
            };
            let ops: Vec<_> = recording
                .trace()
                .iter()
                .filter(|s| s.task == [0])
                .filter_map(|s| match &s.event {
                    hirsel_free::projection::Event::Versioned { outcome, .. }
                        if outcome == name =>
                    {
                        Some(s.progress.op)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(ops, [2], "one Done at the original tail ordinal");
        }
    }
}
