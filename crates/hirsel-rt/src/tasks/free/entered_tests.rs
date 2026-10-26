//! Entered publication follows the existing recorder terminal-prefix vectors;
//! physical Pending follows Tokio's task harness completion-before-idle rule.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

#[derive(Default)]
struct Seen {
    effects: AtomicU32,
    suffix: AtomicU32,
    leaves: AtomicU32,
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    env: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        let seen = &**crate::alloc::payload(env).cast::<*const Seen>();
        if frame.cast::<u32>().read() == 0 {
            frame.cast::<u32>().write(1);
            assert_eq!(
                crate::continuation::hirsel_rt_v2_resume_point(ctx, frame, 1),
                0
            );
        }
        crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 1, 16, 32, 100, 101);
        let site = crate::continuation::binding::take_bound(ctx, 16).unwrap();
        if frame.add(8).read() == 0 {
            assert!(record::begin_entered(ctx));
            assert_eq!(
                record::poll_publish(ctx, Phase::Done, |_| {
                    seen.effects.fetch_add(1, Ordering::Relaxed);
                    frame.add(8).write(1);
                })
                .unwrap(),
                Some(())
            );
            if let Some(recording) = record::owner(ctx) {
                recording.recorder.flush().unwrap().unwrap();
            }
        }
        let leave = crate::continuation::binding::can_leave(ctx, site);
        assert!(leave);
        let pending = try_yield_entered(&mut *ctx, leave).unwrap();
        frame.add(8).write(2);
        if pending {
            return crate::STATUS_SUSPENDED;
        }
        crate::continuation::binding::complete_bound(ctx, site);
        seen.suffix.fetch_add(1, Ordering::Relaxed);
        STATUS_OK
    }
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 32) }
}

fn run(
    width: u32,
    recording: Option<Arc<record::Recording>>,
    replay: Option<Arc<replay::Replay>>,
) -> Arc<Seen> {
    let seen = Arc::new(Seen::default());
    let observe = seen.clone();
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
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
    let layouts = [&raw const layout];
    let mut run = crate::host::BodyRun::new(width, false);
    run.free = true;
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    unsafe { run.use_physical_dispatch() };
    let replayed = replay.is_some();
    let outcome = run.run(
        move |ctx| unsafe {
            if let Some(replay) = &replay {
                replay.root(ctx)
            } else {
                enable(ctx)
            }
            if let Some(recording) = &recording {
                recording.root(ctx)
            }
        },
        move |ctx| unsafe {
            let hook = seen.clone();
            crate::pool::free::test_after_compact(ctx, move |_, _, pooled| {
                assert!(pooled);
                hook.leaves.fetch_add(1, Ordering::Relaxed);
            });
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<*const Seen>()
                .write(Arc::as_ptr(&seen));
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
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            STATUS_OK
        },
    );
    assert_eq!(outcome.pending, 0);
    if replayed {
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
fn entered_final_yield_at_a_replay_cut_leaves_compact_without_running_its_suffix() {
    let recording = record::Recording::new(b"entered final cut", false);
    let observed = run(1, Some(recording.clone()), None);
    assert_eq!(observed.effects.load(Ordering::Acquire), 1);
    assert_eq!(observed.suffix.load(Ordering::Acquire), 1);
    recording.recorder.stop("returned", None).unwrap().unwrap();
    let trace = recording.trace();
    let marked = Arc::try_unwrap(recording)
        .ok()
        .unwrap()
        .recorder
        .into_sink();
    let end = marked
        .marks
        .iter()
        .copied()
        .find(|&end| {
            hirsel_free::tape::parse(&marked.bytes[..end]).is_ok_and(|t| {
                t.cut.get(&vec![0]) == Some(&hirsel_free::phase::Progress::new(1, Phase::Done))
            })
        })
        .unwrap();
    let bytes = &marked.bytes[..end];
    let tape = hirsel_free::tape::parse(bytes).unwrap();
    let expected = hirsel_free::projection::Projection::new("truncated", &tape.cut, &trace);
    for width in [1, 4] {
        let replay = replay::Replay::new(bytes).unwrap();
        let observed = run(width, None, Some(replay.clone()));
        assert_eq!(observed.effects.load(Ordering::Acquire), 1);
        assert_eq!(observed.suffix.load(Ordering::Acquire), 0);
        assert_eq!(
            observed.leaves.load(Ordering::Acquire),
            1,
            "cut retains compact Final, no stackful start"
        );
        assert_eq!(replay.finish().unwrap(), expected);
    }
}
