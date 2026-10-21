use super::*;
use crate::foreign::*;
use hirsel_free::{phase::Progress, projection::Projection, session::Session};
use std::sync::Arc;

#[test]
fn native_serial_gate_outer_phases_record_while_nested_callbacks_keep_depth_only() {
    for width in [2, 8] {
        for durable in [false, true] {
            let recording = super::super::record::Recording::new(b"native serial gate", durable);
            let install = recording.clone();
            let inspect = recording.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let dispatch = FOREIGN_THREADS_SERIAL | (7 << FOREIGN_PACKAGE_SHIFT);
                    let mut outer = [0u64; 5];
                    hirsel_rt_v2_foreign_enter(ctx, 0, outer.as_mut_ptr().cast(), dispatch);
                    let cut = inspect.recorder.flush().unwrap().unwrap();
                    assert_eq!(
                        cut.progress[&Vec::new()],
                        Progress::new(1, Phase::Done),
                        "gate acquisition is recorded before C may call back"
                    );
                    let callback = hirsel_rt_v2_callback_context();
                    assert_eq!(callback, ctx);
                    let mut nested = [0u64; 5];
                    hirsel_rt_v2_foreign_enter(ctx, 0, nested.as_mut_ptr().cast(), dispatch);
                    assert_eq!(crate::foreign::gate_version(ctx, 7), 1);
                    hirsel_rt_v2_foreign_leave(nested.as_ptr().cast());
                    let cut = inspect.recorder.flush().unwrap().unwrap();
                    assert_eq!(
                        cut.progress[&Vec::new()],
                        Progress::new(1, Phase::Done),
                        "nested reentry consumes no ordinal"
                    );
                    hirsel_rt_v2_callback_leave(ctx);
                    hirsel_rt_v2_foreign_leave(outer.as_ptr().cast());
                    assert_eq!(crate::foreign::gate_version(ctx, 7), 2);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            assert_eq!(cut.progress[&Vec::new()], Progress::new(2, Phase::Done));
            let trace = recording.trace();
            let marked = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink();
            let mut replay = Session::replay(&marked.bytes).unwrap();
            assert!(replay.start(&[]));
            replay.gate((3, 7), "package:7");
            for outcome in ["gate enter", "gate leave"] {
                replay.begin(&[]);
                replay.touch(&[], 0, (3, 7), outcome).unwrap();
                replay.publish(&[], Phase::Done).unwrap();
            }
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

unsafe extern "C" fn gate_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let flag = unsafe {
        crate::alloc::payload(env)
            .cast::<*const std::sync::atomic::AtomicBool>()
            .read()
    };
    let mut interval = [0u64; 5];
    unsafe {
        hirsel_rt_v2_foreign_enter(
            ctx,
            0,
            interval.as_mut_ptr().cast(),
            FOREIGN_THREADS_SERIAL | (7 << FOREIGN_PACKAGE_SHIFT),
        );
        assert_eq!(hirsel_rt_v2_callback_context(), ctx);
        (*flag).store(true, std::sync::atomic::Ordering::Release);
        hirsel_rt_v2_callback_leave(ctx);
        hirsel_rt_v2_foreign_leave(interval.as_ptr().cast());
    }
    STATUS_OK
}

#[test]
fn a_contended_gate_retries_done_under_one_ordinal_and_wakes_before_the_owner_next_op() {
    contended_gate(false);
}

#[test]
fn two_native_serial_callbacks_complete_with_one_compute_permit() {
    const CHILD: &str = "HIRSEL_F1_GATE_ONE_PERMIT";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::lifecycle::gate_tests::two_native_serial_callbacks_complete_with_one_compute_permit", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    contended_gate(true);
}

fn contended_gate(one_permit: bool) {
    for width in [2, 8] {
        for durable in [false, true] {
            let recording = super::super::record::Recording::new(b"native contended gate", durable);
            let install = recording.clone();
            let inspect = recording.clone();
            let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            let layout = crate::Layout {
                magic: crate::LAYOUT_MAGIC,
                version: crate::LAYOUT_VERSION,
                size: 8,
                align: 8,
                finalizer: None,
                steps: std::ptr::null(),
                step_count: 0,
                field_count: 1,
                flags: 0,
                logical: 0,
                logical_count: std::ptr::null(),
                scan: std::ptr::null(),
                scan_words: 0,
            };
            let layouts = [&raw const layout];
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let mut outer = [0u64; 5];
                    if one_permit {
                        hirsel_rt_v2_foreign_enter(
                            ctx,
                            0,
                            outer.as_mut_ptr().cast(),
                            FOREIGN_THREADS_SERIAL | (7 << FOREIGN_PACKAGE_SHIFT),
                        );
                        assert_eq!(hirsel_rt_v2_callback_context(), ctx);
                    } else {
                        let _role = role::Guard::new(ctx);
                        assert!(crate::tasks::gate_enter(&mut *ctx, 7, 0));
                    }
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<*const std::sync::atomic::AtomicBool>()
                        .write(Arc::as_ptr(&flag));
                    let mut child = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        gate_child,
                        env,
                        0,
                    );
                    if one_permit {
                        hirsel_rt_v2_callback_leave(ctx);
                    }
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        let saved = {
                            let _role = role::Guard::new(ctx);
                            let s = scheduler(ctx);
                            s.task(child as u32).state == State::Parked
                                && readiness::saved(s, child as u32)
                        };
                        if saved {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "gate waiter saves its frame"
                        );
                        std::thread::yield_now();
                    }
                    let cut = inspect.recorder.flush().unwrap().unwrap();
                    assert_eq!(cut.progress[&vec![0]], Progress::new(1, Phase::Entered));
                    assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
                    if one_permit {
                        hirsel_rt_v2_foreign_leave(outer.as_ptr().cast());
                    } else {
                        let _role = role::Guard::new(ctx);
                        crate::tasks::gate_leave(ctx, 7, true);
                    }
                    // The releasing task can remain in a pure segment forever.
                    // Gate leave must publish runnable work before returning.
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    while !one_permit && !flag.load(std::sync::atomic::Ordering::Acquire) {
                        assert!(
                            std::time::Instant::now() < until,
                            "gate release publishes its parked waiter"
                        );
                        std::thread::yield_now();
                    }
                    let mut image = [0u64; 2];
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child,
                        image.as_mut_ptr().cast(),
                        16,
                    );
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            assert_eq!(cut.progress[&Vec::new()], Progress::new(6, Phase::Done));
            assert_eq!(cut.progress[&vec![0]], Progress::new(3, Phase::Done));
            let trace = recording.trace();
            let marked = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink();
            let mut replay = Session::replay(&marked.bytes).unwrap();
            assert!(replay.start(&[]));
            assert!(replay.start(&[0]));
            replay.gate((3, 7), "package:7");
            for op in 1..=4 {
                replay.begin(&[]);
                if op == 2 || op == 4 {
                    replay
                        .touch(
                            &[],
                            0,
                            (3, 7),
                            if op == 2 { "gate enter" } else { "gate leave" },
                        )
                        .unwrap();
                }
                replay.publish(&[], Phase::Done).unwrap();
            }
            replay.begin(&[0]);
            replay.publish(&[0], Phase::Entered).unwrap();
            for op in 1..=3 {
                if op > 1 {
                    replay.begin(&[0]);
                }
                if op <= 2 {
                    replay
                        .touch(
                            &[0],
                            0,
                            (3, 7),
                            if op == 1 { "gate enter" } else { "gate leave" },
                        )
                        .unwrap();
                }
                if op == 3 {
                    replay.publish(&[0], Phase::Entered).unwrap();
                }
                replay.publish(&[0], Phase::Done).unwrap();
            }
            for _ in 5..=6 {
                replay.begin(&[]);
                replay.publish(&[], Phase::Entered).unwrap();
                replay.publish(&[], Phase::Done).unwrap();
            }
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

#[test]
fn a_terminal_cut_rejects_release_while_the_saved_gate_waiter_remains_entered() {
    stopped_gate(false);
}

#[test]
fn a_host_stopped_gate_waiter_reclaims_its_recording_after_prompt_return() {
    stopped_gate(true);
}

fn stopped_gate(host_stop: bool) {
    struct Flags {
        ready: std::sync::atomic::AtomicBool,
        stopped: std::sync::atomic::AtomicBool,
        acquired: std::sync::atomic::AtomicBool,
    }
    let flags = Box::new(Flags {
        ready: false.into(),
        stopped: false.into(),
        acquired: false.into(),
    });
    let raw = (&*flags as *const Flags) as usize;
    let recording = super::super::record::Recording::new(b"native gate stop", false);
    let install = recording.clone();
    let cutter = recording.clone();
    let mut run = crate::host::BodyRun::new(2, true);
    run.free = true;
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 1,
        flags: 0,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    };
    let layouts = [&raw const layout];
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    unsafe { run.use_physical_dispatch() };
    let control = run.control();
    let stop_control = control.clone();
    let stop = std::thread::spawn(move || {
        let flags = unsafe { &*(raw as *const Flags) };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !flags.ready.load(std::sync::atomic::Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < until,
                "gate waiter reaches idle before stop"
            );
            std::thread::yield_now();
        }
        let cut = cutter.recorder.stop("stopped", None).unwrap().unwrap().0;
        assert_eq!(cut.progress[&vec![0]], Progress::new(1, Phase::Entered));
        if host_stop {
            stop_control.stop();
        }
        flags
            .stopped
            .store(true, std::sync::atomic::Ordering::Release);
    });
    let outcome = run.run(
        move |ctx| unsafe {
            super::super::enable(ctx);
            install.root(ctx);
        },
        move |ctx| unsafe {
            let flags = &*(raw as *const Flags);
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
            {
                let _role = role::Guard::new(ctx);
                assert!(crate::tasks::gate_enter(&mut *ctx, 7, 0));
            }
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<*const std::sync::atomic::AtomicBool>()
                .write(&raw const flags.acquired);
            let mut child = 0;
            hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                &mut child,
                &scope,
                gate_child,
                env,
                0,
            );
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let saved = {
                    let _role = role::Guard::new(ctx);
                    let s = scheduler(ctx);
                    s.task(child as u32).state == State::Parked && readiness::saved(s, child as u32)
                };
                if saved {
                    break;
                }
                assert!(std::time::Instant::now() < until, "gate waiter saves");
                std::thread::yield_now();
            }
            flags
                .ready
                .store(true, std::sync::atomic::Ordering::Release);
            while !flags.stopped.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::yield_now();
            }
            if host_stop {
                let mut canceled = 0;
                hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                panic!("a stopped task cannot resume");
            }
            let _role = role::Guard::new(ctx);
            assert_eq!(
                super::super::gate::leave(ctx, 7, true),
                Err(crate::stop::Stop::Requested)
            );
            let me = scheduler(ctx).current;
            assert_eq!(scheduler(ctx).gate(7).owner, Some(me));
            assert_eq!(scheduler(ctx).gate(7).version, 1);
            STATUS_OK
        },
    );
    stop.join().unwrap();
    if host_stop {
        assert!(
            matches!(outcome.ended, Err(crate::stop::Stop::Requested)),
            "{outcome:?}"
        );
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while control.status().reclamation_pending != 0 {
            assert!(
                std::time::Instant::now() < until,
                "stopped gate frames finish reclaiming"
            );
            std::thread::yield_now();
        }
        assert!(control.status().finished);
    } else {
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
    assert!(!flags.acquired.load(std::sync::atomic::Ordering::Acquire));
    let trace = recording.trace();
    let cut = recording.recorder.stop("stopped", None).unwrap();
    assert!(cut.is_none(), "the cutter already closed the recording");
    let marked = Arc::try_unwrap(recording)
        .ok()
        .unwrap()
        .recorder
        .into_sink();
    let tape = hirsel_free::tape::parse(&marked.bytes).unwrap();
    let mut replay = Session::replay(&marked.bytes).unwrap();
    assert!(replay.start(&[]));
    assert!(replay.start(&[0]));
    replay.gate((3, 7), "package:7");
    for op in 1..=3 {
        replay.begin(&[]);
        if op == 2 {
            replay.touch(&[], 0, (3, 7), "gate enter").unwrap();
        }
        replay.publish(&[], Phase::Done).unwrap();
    }
    replay.begin(&[0]);
    replay.publish(&[0], Phase::Entered).unwrap();
    assert_eq!(
        replay.finish().unwrap(),
        Projection::new("stopped", &tape.cut, &trace)
    );
}
