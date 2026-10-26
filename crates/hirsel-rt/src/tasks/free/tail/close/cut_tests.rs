//! A real close Done cut retains detached owners without entering their drops.
use super::*;
use crate::tasks::free::objects::tests::{isolated_at, plain};
use crate::tasks::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

static DROPS: AtomicU32 = AtomicU32::new(0);
static RECORDING: Mutex<Option<Arc<record::Recording>>> = Mutex::new(None);
pub(super) fn flush_done() {
    // Runs outside the admitted section and only observes its completed prefix.
    if let Some(recording) = RECORDING.lock().unwrap().as_ref() {
        recording.recorder.flush().unwrap().unwrap();
    }
}
unsafe extern "C" fn leaf(_: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    assert_eq!(unsafe { value.cast::<[u64; 2]>().read() }, [1, 7]);
    DROPS.fetch_add(1, Ordering::AcqRel);
    STATUS_OK
}
struct Shared {
    receiver: AtomicU64,
    task: AtomicU32,
    leaves: AtomicU32,
    suffix: AtomicU32,
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
        let result = frame.add(16).cast::<u64>();
        if frame.cast::<u32>().read() == 2 {
            return hirsel_rt_v2_task_finish(
                ctx,
                result.cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                u32::MAX,
            );
        }
        if frame.cast::<u32>().read() == 0 {
            shared.task.store((*ctx).task_id, Ordering::Release);
            frame
                .add(8)
                .cast::<u64>()
                .write(shared.receiver.load(Ordering::Acquire));
            frame.cast::<u32>().write(1);
        }
        crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 50, 32, 31, 41);
        let status = crate::finalize::hirsel_rt_v2_finalize(
            ctx,
            4,
            frame.add(8),
            ptr::null(),
            ptr::null_mut(),
            31,
            41,
        );
        if status == crate::STATUS_SUSPENDED {
            return status;
        }
        assert_eq!(status, STATUS_OK);
        shared.suffix.fetch_add(1, Ordering::AcqRel);
        result.write(1);
        result.add(1).write(108);
        frame.cast::<u32>().write(2);
        hirsel_rt_v2_task_finish(ctx, result.cast(), [0u64; 2].as_ptr().cast(), 16, u32::MAX)
    }
}
unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 32) }
}
fn run(
    width: u32,
    recording: Option<Arc<record::Recording>>,
    replay: Option<Arc<replay::Replay>>,
) -> Arc<Shared> {
    DROPS.store(0, Ordering::Release);
    *RECORDING.lock().unwrap() = recording.clone();
    let is_replay = replay.is_some();
    let shared = Arc::new(Shared {
        receiver: AtomicU64::new(0),
        task: AtomicU32::new(u32::MAX),
        leaves: AtomicU32::new(0),
        suffix: AtomicU32::new(0),
    });
    let observe = shared.clone();
    let leaf = crate::tasks::free::finalized(crate::Layout {
        finalizer: Some(leaf),
        ..plain(16)
    });
    let close = Box::leak(Box::new(crate::Step {
        kind: crate::STEP_CLOSE_HANDLE,
        offset: 0,
        field: crate::WHOLE_VALUE,
        variant: 0,
        span: 0,
        stride: 0,
        count: 0,
        count_offset: crate::NO_OFFSET,
        layout: ptr::null(),
    }));
    let endpoint = crate::Layout {
        steps: close,
        step_count: 1,
        ..plain(8)
    };
    let image_step = Box::leak(Box::new(crate::Step {
        offset: 8,
        ..*close
    }));
    let image = crate::Layout {
        steps: image_step,
        step_count: 1,
        ..plain(16)
    };
    let env = plain(8);
    let layouts = [leaf as *const crate::Layout, &image, &endpoint, &env];
    let mut run = crate::host::BodyRun::new(width, false);
    run.free = true;
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 4;
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
            let mut outer = none;
            let mut inner = none;
            hirsel_rt_v2_channel_open(
                ctx,
                ptr::null_mut(),
                outer.as_mut_ptr(),
                &0,
                none.as_ptr().cast(),
                3,
            );
            hirsel_rt_v2_channel_open(
                ctx,
                ptr::null_mut(),
                inner.as_mut_ptr(),
                &0,
                none.as_ptr().cast(),
                2,
            );
            let mut value = [1u64, 7];
            let mut status = 99;
            hirsel_rt_v2_channel_send(ctx, &mut status, &mut inner[0], value.as_mut_ptr().cast());
            assert_eq!(status, crate::channels::DONE);
            let mut value = [1u64, inner[1]];
            hirsel_rt_v2_channel_send(ctx, &mut status, &mut outer[0], value.as_mut_ptr().cast());
            assert_eq!(status, crate::channels::DONE);
            {
                let _role = role::Guard::new(ctx);
                assert_eq!(close_endpoint(ctx, inner[0], 0, 0), STATUS_OK);
                assert_eq!(close_endpoint(ctx, outer[0], 0, 0), STATUS_OK);
            }
            shared.receiver.store(outer[1], Ordering::Release);
            let hook = shared.clone();
            crate::pool::free::test_after_compact(ctx, move |task, _, pooled| {
                if task == hook.task.load(Ordering::Acquire) {
                    assert!(pooled);
                    hook.leaves.fetch_add(1, Ordering::AcqRel);
                }
            });
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, ptr::null_mut(), &mut scope, &0);
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 5, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<*const Shared>()
                .write(Arc::as_ptr(&shared));
            let mut task = 0;
            hirsel_rt_v2_task_spawn(ctx, 0, 0, ptr::null_mut(), &mut task, &scope, child, env, 0);
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            STATUS_OK
        },
    );
    *RECORDING.lock().unwrap() = None;
    assert_eq!(outcome.pending, 0);
    if is_replay {
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
fn close_done_cuts_leave_compact_before_detached_finalizers_or_source_suffix() {
    if !isolated_at(
        "tasks::free::tail::close::cut_tests::close_done_cuts_leave_compact_before_detached_finalizers_or_source_suffix",
        "HIRSEL_OWNED_CLOSE_DONE_CUT",
    ) {
        return;
    }
    for durable in [false, true] {
        let recording = record::Recording::new(b"owned close Done cuts", durable);
        let observed = run(1, Some(recording.clone()), None);
        assert_eq!(observed.suffix.load(Ordering::Acquire), 1);
        assert_eq!(DROPS.load(Ordering::Acquire), 1);
        recording.recorder.stop("returned", None).unwrap();
        let trace = recording.trace();
        let sink = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        for ordinal in [1, 2] {
            let end = sink
                .marks
                .iter()
                .copied()
                .find(|&end| {
                    hirsel_free::tape::parse(&sink.bytes[..end]).is_ok_and(|t| {
                        t.cut.get(&vec![0])
                            == Some(&hirsel_free::phase::Progress::new(
                                ordinal,
                                hirsel_free::phase::Phase::Done,
                            ))
                    })
                })
                .expect("completed close prefix");
            let bytes = &sink.bytes[..end];
            let tape = hirsel_free::tape::parse(bytes).unwrap();
            let expected = hirsel_free::projection::Projection::new("truncated", &tape.cut, &trace);
            for width in [1, 4] {
                let replay = replay::Replay::new(bytes).unwrap();
                let observed = run(width, None, Some(replay.clone()));
                assert_eq!(observed.leaves.load(Ordering::Acquire), 1);
                assert_eq!(observed.suffix.load(Ordering::Acquire), 0);
                assert_eq!(
                    DROPS.load(Ordering::Acquire),
                    0,
                    "cut must precede detached user cleanup"
                );
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}
