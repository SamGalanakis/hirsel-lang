//! Flush a real completed mutex phase while its retained dispatcher is saved,
//! then replay that exact cut. No test hook changes operation admission.
use super::*;
use crate::tasks::free::objects::tests::{isolated_at, plain, wait_until, wrapper};
use crate::tasks::*;
use std::ptr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};

struct Shared {
    handle: AtomicU64,
    kind: Kind,
    task: AtomicU32,
    callback: AtomicU32,
    suffix: AtomicU32,
    saved: AtomicBool,
    go: AtomicBool,
    leaves: AtomicU32,
    replay: bool,
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
        let handle = shared.handle.load(Ordering::Acquire);
        let payload = frame.add(16).cast::<u64>();
        if frame.cast::<u32>().read() == 2 {
            return hirsel_rt_v2_task_finish(
                ctx,
                payload.cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                u32::MAX,
            );
        }
        let _role = role::Guard::new(ctx);
        if frame.cast::<u32>().read() == 0 {
            shared.task.store((*ctx).task_id, Ordering::Release);
            let mut dead = 99;
            assert_eq!(
                hirsel_rt_v2_mutex_take(ctx, &mut dead, &handle, payload.cast()),
                STATUS_OK
            );
            assert_eq!(dead, u32::from(shared.kind == Kind::Cause));
            match shared.kind {
                Kind::Put => {
                    shared.callback.fetch_add(1, Ordering::Relaxed);
                    payload.add(1).write(8);
                }
                Kind::Kill => {
                    shared.callback.fetch_add(1, Ordering::Relaxed);
                    crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 11, 101, 201);
                }
                Kind::Cause => {
                    crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 13, 103, 203);
                }
                _ => unreachable!(),
            }
            frame.cast::<u32>().write(1);
            if shared.kind != Kind::Cause {
                crate::pool::free::renew(ctx);
                for _ in 1..hirsel_free::schedule::OP_BUDGET {
                    assert!(crate::pool::free::proceed(ctx));
                }
            }
        } else if !shared.go.load(Ordering::Acquire) && !shared.replay {
            crate::tasks::park_left(ctx, Wait::Retry { cancelable: false }).unwrap();
            crate::tasks::compact::wait(ctx);
            return crate::STATUS_SUSPENDED;
        }
        if frame.cast::<u32>().read() != 3 {
            crate::continuation::binding::hirsel_rt_v2_resume_tail(
                ctx,
                frame,
                1,
                match shared.kind {
                    Kind::Put => 0,
                    Kind::Kill => 1,
                    Kind::Cause => 2,
                    _ => unreachable!(),
                },
                32,
                31,
                41,
            );
            let status = match shared.kind {
                Kind::Put => hirsel_rt_v2_mutex_put(ctx, &handle, payload.cast()),
                Kind::Kill => hirsel_rt_v2_mutex_kill(ctx, &handle),
                Kind::Cause => hirsel_rt_v2_mutex_cause(ctx, &handle),
                _ => unreachable!(),
            };
            if status == crate::STATUS_SUSPENDED {
                return status;
            }
            assert_eq!(status, STATUS_OK);
            // Cause has no live quantum yield. Force its retained completed call
            // to a saved frame before the source suffix, without issuing an op.
            if shared.kind == Kind::Cause && !shared.go.load(Ordering::Acquire) && !shared.replay {
                crate::tasks::park_left(ctx, Wait::Retry { cancelable: false }).unwrap();
                crate::tasks::compact::wait(ctx);
                frame.cast::<u32>().write(3);
                return crate::STATUS_SUSPENDED;
            }
        }
        shared.suffix.fetch_add(1, Ordering::Relaxed);
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
        crate::failure::hirsel_rt_v2_failure_clear(ctx);
        payload.write(1);
        payload.add(1).write(108);
        frame.cast::<u32>().write(2);
        hirsel_rt_v2_task_finish(ctx, payload.cast(), [0u64; 2].as_ptr().cast(), 16, u32::MAX)
    }
}
unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 32) }
}

fn run(
    width: u32,
    kind: Kind,
    recording: Option<Arc<record::Recording>>,
    replay: Option<Arc<replay::Replay>>,
) -> Arc<Shared> {
    let shared = Arc::new(Shared {
        handle: AtomicU64::new(0),
        kind,
        task: AtomicU32::new(u32::MAX),
        callback: AtomicU32::new(0),
        suffix: AtomicU32::new(0),
        saved: AtomicBool::new(false),
        go: AtomicBool::new(false),
        leaves: AtomicU32::new(0),
        replay: replay.is_some(),
    });
    let observe = shared.clone();
    let owner = recording.clone();
    let scalar = plain(16);
    let wrapper = wrapper();
    let env = plain(8);
    let layouts = [&scalar as *const crate::Layout, &wrapper, &env];
    let mut run = crate::host::BodyRun::new(width, false);
    run.free = true;
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 3;
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        move |ctx| unsafe {
            if let Some(replay) = replay {
                replay.root(ctx)
            } else {
                crate::tasks::free::enable(ctx)
            }
            if let Some(recording) = recording {
                recording.root(ctx)
            }
        },
        move |ctx| unsafe {
            let none = [0u64; 2];
            let mut payload = [1u64, 7];
            let mut handle = 0;
            hirsel_rt_v2_mutex_create(
                ctx,
                ptr::null_mut(),
                &mut handle,
                payload.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                2,
                3,
            );
            shared.handle.store(handle, Ordering::Release);
            if kind == Kind::Cause {
                let mut dead = 99;
                let mut payload = none;
                hirsel_rt_v2_mutex_take(ctx, &mut dead, &handle, payload.as_mut_ptr().cast());
                crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 11, 101, 201);
                hirsel_rt_v2_mutex_kill(ctx, &handle);
                crate::failure::hirsel_rt_v2_failure_clear(ctx);
            }
            let hook = shared.clone();
            crate::pool::free::test_after_compact(ctx, move |task, _, pooled| {
                if task == hook.task.load(Ordering::Acquire) {
                    assert!(pooled);
                    hook.leaves.fetch_add(1, Ordering::Relaxed);
                    hook.saved.store(true, Ordering::Release);
                }
            });
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, ptr::null_mut(), &mut scope, &0);
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 4, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<*const Shared>()
                .write(Arc::as_ptr(&shared));
            let mut task = 0;
            hirsel_rt_v2_task_spawn(ctx, 0, 0, ptr::null_mut(), &mut task, &scope, child, env, 0);
            if !shared.replay {
                wait_until("entered mutex yields with completed phase", || {
                    if shared.saved.load(Ordering::Acquire) {
                        return true;
                    }
                    let _role = role::Guard::new(ctx);
                    crate::tasks::free::suspend(&mut *ctx, true);
                    false
                });
                owner.as_ref().unwrap().recorder.flush().unwrap().unwrap();
                shared.go.store(true, Ordering::Release);
                let token = {
                    let _role = role::Guard::new(ctx);
                    role::with_held(ctx, |s| {
                        let task = shared.task.load(Ordering::Acquire);
                        assert!(s.task(task).stack.is_none());
                        crate::pool::free::token((*ctx).run_ptr(), s.key(task), Owner::Any).unwrap()
                    })
                };
                crate::pool::free::notify((*ctx).run_ptr(), &token);
                drop(token);
            }
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            STATUS_OK
        },
    );
    assert_eq!(outcome.pending, 0);
    if observe.replay {
        assert!(
            matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
            "{outcome:?}"
        );
    } else {
        assert_eq!(outcome.ended, Ok(STATUS_OK));
    }
    observe
}
#[test]
fn entered_mutex_done_cuts_leave_compact_without_repeating_effect_or_source_suffix() {
    if !isolated_at(
        "tasks::free::tail::mutex::cut_tests::entered_mutex_done_cuts_leave_compact_without_repeating_effect_or_source_suffix",
        "HIRSEL_ENTERED_MUTEX_CUT",
    ) {
        return;
    }
    for kind in [Kind::Put, Kind::Kill, Kind::Cause] {
        for durable in [false, true] {
            let recording = record::Recording::new(b"entered mutex Done cut", durable);
            let observed = run(1, kind, Some(recording.clone()), None);
            assert_eq!(observed.suffix.load(Ordering::Acquire), 1);
            recording.recorder.stop("returned", None).unwrap();
            let trace = recording.trace();
            let sink = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink();
            let end = sink
                .marks
                .iter()
                .copied()
                .find(|&end| {
                    hirsel_free::tape::parse(&sink.bytes[..end]).is_ok_and(|t| {
                        t.cut.get(&vec![0])
                            == Some(&hirsel_free::phase::Progress::new(2, Phase::Done))
                    })
                })
                .unwrap();
            let bytes = &sink.bytes[..end];
            let tape = hirsel_free::tape::parse(bytes).unwrap();
            let expected = hirsel_free::projection::Projection::new("truncated", &tape.cut, &trace);
            for width in [1, 4] {
                let replay = replay::Replay::new(bytes).unwrap();
                let observed = run(width, kind, None, Some(replay.clone()));
                assert_eq!(observed.suffix.load(Ordering::Acquire), 0);
                assert_eq!(
                    observed.callback.load(Ordering::Acquire),
                    u32::from(kind != Kind::Cause)
                );
                assert_eq!(observed.leaves.load(Ordering::Acquire), 1);
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}
