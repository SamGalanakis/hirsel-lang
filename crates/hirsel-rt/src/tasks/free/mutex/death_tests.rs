use super::*;
use crate::tasks::free::{record::Recording, replay::Replay};
use hirsel_free::projection::Projection;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

unsafe fn dead_mutex(ctx: *mut TaskCtx, cutter: Option<&Recording>, stage: &AtomicU32, dead: bool) {
    unsafe {
        let mut handle = 0;
        let mut payload = [1u64, 7];
        let none = [0u64; 2];
        hirsel_rt_v2_mutex_create(
            ctx,
            std::ptr::null_mut(),
            &mut handle,
            payload.as_mut_ptr().cast(),
            none.as_ptr().cast(),
            2,
            3,
        );
        stage.store(1, Ordering::Release);
        if dead {
            let mut status = 99;
            hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
            assert_eq!(status, 0);
            assert_eq!(payload, [1, 7]);
            stage.store(2, Ordering::Release);
            crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 11, 101, 201);
            crate::failure::hirsel_rt_v2_failure_append(
                ctx,
                &crate::failure::FailureRecord {
                    kind: 1,
                    rule: 12,
                    site: 102,
                    operation_site: 202,
                },
            );
            hirsel_rt_v2_mutex_kill(ctx, &handle);
            stage.store(3, Ordering::Release);
            if let Some(cutter) = cutter {
                cutter.recorder.flush().unwrap().unwrap();
            }
            crate::failure::hirsel_rt_v2_failure_clear(ctx);
            payload = none;
            hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
            assert_eq!(status, 1);
            assert_eq!(payload, none);
            stage.store(4, Ordering::Release);
            crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 13, 103, 203);
            hirsel_rt_v2_mutex_cause(ctx, &handle);
            stage.store(5, Ordering::Release);
            assert_eq!(
                (*ctx)
                    .failure
                    .chain(&[])
                    .iter()
                    .map(|(record, _)| record.rule)
                    .collect::<Vec<_>>(),
                [13, 11, 12],
                "a later acquisition preserves its primary followed by the original body chain",
            );
            crate::failure::hirsel_rt_v2_failure_clear(ctx);
            if let Some(cutter) = cutter {
                cutter.recorder.flush().unwrap().unwrap();
            }
        }
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_mutex(ctx, mutex_word(&handle), 0, 0), STATUS_OK);
        }
        stage.store(if dead { 6 } else { 2 }, Ordering::Release);
    }
}

unsafe extern "C" fn query_cause(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    unsafe {
        let (handle, checked) = crate::alloc::payload(env)
            .cast::<(u64, *const std::sync::atomic::AtomicBool)>()
            .read();
        crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 14, 104, 204);
        hirsel_rt_v2_mutex_cause(ctx, &handle);
        assert_eq!((*ctx).failure.count(), 3);
        crate::failure::hirsel_rt_v2_failure_clear(ctx);
        (*checked).store(true, Ordering::Release);
    }
    STATUS_OK
}

unsafe fn cause_gap(ctx: *mut TaskCtx, checked: &std::sync::atomic::AtomicBool, replay: bool) {
    unsafe {
        let mut handle = 0;
        let mut payload = [1u64, 7];
        let none = [0u64; 2];
        hirsel_rt_v2_mutex_create(
            ctx,
            std::ptr::null_mut(),
            &mut handle,
            payload.as_mut_ptr().cast(),
            none.as_ptr().cast(),
            2,
            3,
        );
        let mut status = 99;
        hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
        assert_eq!(status, 0);
        crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 11, 101, 201);
        crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 12, 102, 202);
        hirsel_rt_v2_mutex_kill(ctx, &handle);
        crate::failure::hirsel_rt_v2_failure_clear(ctx);
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<(u64, *const std::sync::atomic::AtomicBool)>()
            .write((handle, checked));
        let mut child = 0;
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut child,
            &scope,
            query_cause,
            env,
            0,
        );
        let parks = {
            let _role = role::Guard::new(ctx);
            if !replay {
                let scope = scheduler(ctx).scope_handle(scope);
                try_block(&mut *ctx, Wait::Exit { scope }, false, 0).unwrap();
                assert!(checked.load(Ordering::Acquire));
            } else {
                assert!(!checked.load(Ordering::Acquire));
            }
            scheduler(ctx).parks
        };
        crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 13, 103, 203);
        hirsel_rt_v2_mutex_cause(ctx, &handle);
        assert!(checked.load(Ordering::Acquire));
        assert_eq!(
            (*ctx)
                .failure
                .chain(&[])
                .iter()
                .map(|(record, _)| record.rule)
                .collect::<Vec<_>>(),
            [13, 11, 12]
        );
        crate::failure::hirsel_rt_v2_failure_clear(ctx);
        if replay {
            let _role = role::Guard::new(ctx);
            assert!(
                scheduler(ctx).parks > parks,
                "cause waits for its recorded version before copying a chain"
            );
        }
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_mutex(ctx, mutex_word(&handle), 0, 0), STATUS_OK);
        }
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}

#[test]
fn native_mutex_cause_replay_saves_for_predecessors_without_owning_a_chain() {
    const CHILD: &str = "HIRSEL_F1_MUTEX_CAUSE_REPLAY_GAPS";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::mutex::death_tests::native_mutex_cause_replay_saves_for_predecessors_without_owning_a_chain", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    let image = super::tests::layout();
    let wrapper = crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..image
    };
    let layouts = [&raw const image, &raw const wrapper];
    for durable in [false, true] {
        let recording = Recording::new(b"native mutex cause gap replay", durable);
        let install = recording.clone();
        let checked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = checked.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                cause_gap(ctx, &observed, false);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        let trace = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        for width in [1, 2, 8] {
            let replay = Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let checked = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observed = checked.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 2;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    cause_gap(ctx, &observed, true);
                    panic!("cause replay passed scope exit");
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

#[test]
fn native_dead_mutex_replay_preserves_its_chain_and_inert_close_cuts() {
    let image = super::tests::layout();
    let wrapper = crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..image
    };
    let layouts = [&raw const image, &raw const wrapper];
    for (durable, dead) in [(false, false), (true, false), (false, true), (true, true)] {
        let recording = Recording::new(b"native dead mutex replay", durable);
        let install = recording.clone();
        let cutter = recording.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                dead_mutex(ctx, Some(&cutter), &AtomicU32::new(0), dead);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let trace = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let mut ends = vec![bytes.len()];
        if dead {
            for ordinal in [3, 5] {
                let end = (1..bytes.len())
                    .find(|&end| {
                        hirsel_free::tape::parse(&bytes[..end]).is_ok_and(|tape| {
                            tape.cut.get(&Vec::new())
                                == Some(&hirsel_free::phase::Progress::new(ordinal, Phase::Done))
                        })
                    })
                    .unwrap();
                ends.extend([end, end + 1]);
            }
        }
        for end in ends {
            let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
            let full = end == bytes.len();
            let ordinal = parsed.cut[&Vec::new()].op;
            let expected = Projection::new(
                if full { "returned" } else { "truncated" },
                &parsed.cut,
                &trace,
            );
            for width in [1, 2, 8] {
                let replay = Replay::new(&bytes[..end]).unwrap();
                let install = replay.clone();
                let stage = Arc::new(AtomicU32::new(0));
                let observed = stage.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe { install.root(ctx) },
                    move |ctx| unsafe {
                        dead_mutex(ctx, None, &observed, dead);
                        panic!("dead mutex replay passed its cut");
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(stage.load(Ordering::Acquire), ordinal as u32 - 1);
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}
