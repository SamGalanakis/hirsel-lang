use super::*;
use crate::tasks::free::{record::Recording, replay::Replay};
use hirsel_free::projection::Projection;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

unsafe fn endpoints(ctx: *mut TaskCtx, cutter: Option<&Recording>, stage: &AtomicU32) {
    let mut pair = [0; 2];
    let none = [0u64; 2];
    unsafe {
        hirsel_rt_v2_channel_open(
            ctx,
            std::ptr::null_mut(),
            pair.as_mut_ptr(),
            &0,
            none.as_ptr().cast(),
            2,
        );
        stage.store(1, Ordering::Release);
        let mut alias = 0;
        hirsel_rt_v2_sender_dup(ctx, std::ptr::null_mut(), &mut alias, &0, &pair[0]);
        stage.store(2, Ordering::Release);
        if let Some(cutter) = cutter {
            cutter.recorder.flush().unwrap().unwrap();
        }
        let mut message = [1u64, 7];
        let mut status = 99;
        hirsel_rt_v2_channel_send(ctx, &mut status, &mut alias, message.as_mut_ptr().cast());
        assert_eq!(status, crate::channels::DONE);
        stage.store(3, Ordering::Release);
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[0], 0, 0), STATUS_OK);
        }
        stage.store(4, Ordering::Release);
        if let Some(cutter) = cutter {
            cutter.recorder.flush().unwrap().unwrap();
        }
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut pair[1],
            message.as_mut_ptr().cast(),
        );
        assert_eq!(status, crate::channels::DONE);
        assert_eq!(message, [1, 7]);
        message = none;
        stage.store(5, Ordering::Release);
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, alias, 0, 0), STATUS_OK);
        }
        stage.store(6, Ordering::Release);
        if let Some(cutter) = cutter {
            cutter.recorder.flush().unwrap().unwrap();
        }
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut pair[1],
            message.as_mut_ptr().cast(),
        );
        assert_eq!(status, crate::channels::CLOSED);
        assert_eq!(message, none);
        stage.store(7, Ordering::Release);
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[1], 0, 0), STATUS_OK);
        }
        stage.store(8, Ordering::Release);
        hirsel_rt_v2_channel_open(
            ctx,
            std::ptr::null_mut(),
            pair.as_mut_ptr(),
            &0,
            none.as_ptr().cast(),
            2,
        );
        stage.store(9, Ordering::Release);
        message = [1, 9];
        hirsel_rt_v2_channel_send(ctx, &mut status, &mut pair[0], message.as_mut_ptr().cast());
        assert_eq!(status, crate::channels::DONE);
        stage.store(10, Ordering::Release);
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[1], 0, 0), STATUS_OK);
        }
        stage.store(11, Ordering::Release);
        if let Some(cutter) = cutter {
            cutter.recorder.flush().unwrap().unwrap();
        }
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[0], 0, 0), STATUS_OK);
        }
        stage.store(12, Ordering::Release);
    }
}

#[test]
fn native_endpoint_aliases_and_closes_replay_one_history_and_stop_at_keep_cuts() {
    let layout = tests::layout();
    let layouts = [&raw const layout];
    for durable in [false, true] {
        let recording = Recording::new(b"native endpoint replay", durable);
        let install = recording.clone();
        let cutter = recording.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let stage = AtomicU32::new(0);
                endpoints(ctx, Some(&cutter), &stage);
                assert_eq!(stage.load(Ordering::Acquire), 12);
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
        for ordinal in [2, 4, 6, 11] {
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
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe { install.root(ctx) },
                    move |ctx| unsafe {
                        endpoints(ctx, None, &observed);
                        panic!("endpoint replay passed its cut");
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

unsafe extern "C" fn endpoint_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    unsafe {
        let (sender, checked) = crate::alloc::payload(env)
            .cast::<(u64, *const std::sync::atomic::AtomicBool)>()
            .read();
        let mut alias = 0;
        hirsel_rt_v2_sender_dup(ctx, std::ptr::null_mut(), &mut alias, &0, &sender);
        let mut message = [1u64, 42];
        let mut status = 99;
        hirsel_rt_v2_channel_send(ctx, &mut status, &mut alias, message.as_mut_ptr().cast());
        assert_eq!(status, crate::channels::DONE);
        let _role = role::Guard::new(ctx);
        assert_eq!(close_endpoint(ctx, sender, 0, 0), STATUS_OK);
        (*checked).store(true, Ordering::Release);
        assert_eq!(close_endpoint(ctx, alias, 0, 0), STATUS_OK);
    }
    STATUS_OK
}

unsafe fn gapped_endpoints(
    ctx: *mut TaskCtx,
    checked: &std::sync::atomic::AtomicBool,
    replay: bool,
    close_first: bool,
) {
    unsafe {
        let mut pair = [0; 2];
        let none = [0u64; 2];
        hirsel_rt_v2_channel_open(
            ctx,
            std::ptr::null_mut(),
            pair.as_mut_ptr(),
            &0,
            none.as_ptr().cast(),
            2,
        );
        let mut child_sender = 0;
        hirsel_rt_v2_sender_dup(ctx, std::ptr::null_mut(), &mut child_sender, &0, &pair[0]);
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<(u64, *const std::sync::atomic::AtomicBool)>()
            .write((child_sender, checked));
        let mut child = 0;
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut child,
            &scope,
            endpoint_child,
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
        let mut alias = 0;
        if close_first {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[0], 0, 0), STATUS_OK);
        } else {
            hirsel_rt_v2_sender_dup(ctx, std::ptr::null_mut(), &mut alias, &0, &pair[0]);
        }
        assert!(
            checked.load(Ordering::Acquire),
            "the child supplied the recorded predecessors"
        );
        if replay {
            let _role = role::Guard::new(ctx);
            assert!(
                scheduler(ctx).parks > parks,
                "endpoint version wait released the sole permit"
            );
        }
        if !close_first {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[0], 0, 0), STATUS_OK);
            assert_eq!(close_endpoint(ctx, alias, 0, 0), STATUS_OK);
        }
        let mut message = none;
        let mut status = 99;
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut pair[1],
            message.as_mut_ptr().cast(),
        );
        assert_eq!(status, crate::channels::DONE);
        assert_eq!(message, [1, 42]);
        message = none;
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut pair[1],
            message.as_mut_ptr().cast(),
        );
        assert_eq!(status, crate::channels::CLOSED);
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, pair[1], 0, 0), STATUS_OK);
        }
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}

#[test]
fn native_alias_and_close_replay_save_for_other_endpoint_versions_with_one_permit() {
    const CHILD: &str = "HIRSEL_F1_ENDPOINT_REPLAY_GAPS";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::channel::endpoint_tests::native_alias_and_close_replay_save_for_other_endpoint_versions_with_one_permit", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    let layout = tests::layout();
    let layouts = [&raw const layout];
    for (durable, close_first) in [(false, false), (true, false), (false, true), (true, true)] {
        let recording = Recording::new(b"native endpoint gap replay", durable);
        let install = recording.clone();
        let checked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = checked.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                gapped_endpoints(ctx, &observed, false, close_first);
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
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    gapped_endpoints(ctx, &observed, true, close_first);
                    panic!("endpoint gap replay passed scope exit");
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
