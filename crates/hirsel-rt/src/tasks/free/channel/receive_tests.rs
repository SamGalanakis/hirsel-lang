//! Real saved receives publish Entered, then owner-side Done at the same op.
use super::*;
use crate::tasks::free::record;
use crate::tasks::free::replay::Replay;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

struct ReplayReceive {
    recording: Option<Arc<record::Recording>>,
}

unsafe extern "C" fn replay_sender(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let (mut sender, probe) = unsafe {
        crate::alloc::payload(env)
            .cast::<(u64, *const ReplayReceive)>()
            .read()
    };
    let probe = unsafe { &*probe };
    if let Some(recording) = &probe.recording {
        let cut = recording.recorder.flush().unwrap().unwrap();
        assert_eq!(cut.progress[&Vec::new()], Progress::new(4, Phase::Entered));
        assert_eq!(cut.progress[&vec![0]], Progress::START);
    }
    let mut message = [1u64, 7];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_channel_send(ctx, &mut status, &mut sender, message.as_mut_ptr().cast())
    };
    assert_eq!(status, crate::channels::DONE);
    STATUS_OK
}

unsafe fn replay_receive_body(
    ctx: *mut TaskCtx,
    probe: &ReplayReceive,
    out: *mut u8,
    replay: bool,
) {
    unsafe {
        let mut endpoints = [0; 2];
        let none = [0u64; 2];
        hirsel_rt_v2_channel_open(
            ctx,
            std::ptr::null_mut(),
            endpoints.as_mut_ptr(),
            &0,
            none.as_ptr().cast(),
            2,
        );
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<(u64, *const ReplayReceive)>()
            .write((endpoints[0], probe));
        let mut child = 0;
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut child,
            &scope,
            replay_sender,
            env,
            0,
        );
        let parks = {
            let _role = role::Guard::new(ctx);
            if replay {
                // No tape cancellation was observed. The live flag must not
                // cancel the receive or make its saved wait spin ready.
                scheduler(ctx).cancel(ROOT, CANCEL_PARENT);
            }
            scheduler(ctx).parks
        };
        let mut status = 99;
        hirsel_rt_v2_channel_receive(ctx, 0, 0, &mut status, &mut endpoints[1], out);
        assert_eq!(status, crate::channels::DONE);
        let _role = role::Guard::new(ctx);
        assert!(
            scheduler(ctx).parks > parks,
            "the empty receive saved its wait at one permit"
        );
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}

#[test]
fn native_receive_replay_saves_before_send_and_admits_entered_only_cuts() {
    const CHILD: &str = "HIRSEL_F1_CHANNEL_REPLAY_WAIT";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::channel::receive_tests::native_receive_replay_saves_before_send_and_admits_entered_only_cuts", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    let layout = tests::layout();
    let layouts = [&raw const layout];
    for durable in [false, true] {
        let recording = record::Recording::new(b"native receive replay", durable);
        let install = recording.clone();
        let probe = Arc::new(ReplayReceive {
            recording: Some(recording.clone()),
        });
        let observed = probe.clone();
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
                let mut image = [0u64; 2];
                replay_receive_body(ctx, &observed, image.as_mut_ptr().cast(), false);
                assert_eq!(image, [1, 7]);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        drop(probe);
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let trace = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let flush = (1..bytes.len())
            .find(|&end| {
                hirsel_free::tape::parse(&bytes[..end]).is_ok_and(|tape| {
                    tape.cut.get(&Vec::new()) == Some(&Progress::new(4, Phase::Entered))
                })
            })
            .unwrap();
        for end in [bytes.len(), flush, flush + 1] {
            let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
            let full = end == bytes.len();
            let expected = Projection::new(
                if full { "returned" } else { "truncated" },
                &parsed.cut,
                &trace,
            );
            for width in [2, 8] {
                let replay = Replay::new(&bytes[..end]).unwrap();
                let install = replay.clone();
                let probe = Arc::new(ReplayReceive { recording: None });
                let observed = probe.clone();
                let outputs = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
                let out = outputs.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe { install.root(ctx) },
                    move |ctx| unsafe {
                        replay_receive_body(ctx, &observed, out[0].as_ptr().cast(), true);
                        panic!("receive replay passed scope exit cut");
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(outputs[0].load(Ordering::Acquire), u64::from(full));
                assert_eq!(outputs[1].load(Ordering::Acquire), if full { 7 } else { 0 });
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}

struct Result {
    receiver: u64,
    finished: AtomicBool,
    status: AtomicU32,
    value: AtomicU64,
}

#[test]
fn immediate_receive_checks_cancellation_before_messages_and_shields_closed_reads() {
    let mut run = crate::host::BodyRun::new(2, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let layout = tests::layout();
    let layouts = [&raw const layout];
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    let outcome = run.run(
        |ctx| unsafe { super::super::enable(ctx) },
        |ctx| {
            let mut endpoints = [0u64; 2];
            let none = [0u64; 2];
            let mut image = [1u64, 99];
            let mut status = 99;
            unsafe {
                hirsel_rt_v2_channel_open(
                    ctx,
                    std::ptr::null_mut(),
                    endpoints.as_mut_ptr(),
                    &0,
                    none.as_ptr().cast(),
                    2,
                );
                hirsel_rt_v2_channel_send(
                    ctx,
                    &mut status,
                    &mut endpoints[0],
                    image.as_mut_ptr().cast(),
                );
                assert_eq!(status, crate::channels::DONE);
                {
                    let _role = role::Guard::new(ctx);
                    scheduler(ctx).cancel(ROOT, CANCEL_REQUESTED);
                }
                hirsel_rt_v2_channel_receive(
                    ctx,
                    0,
                    0,
                    &mut status,
                    &mut endpoints[1],
                    image.as_mut_ptr().cast(),
                );
                assert_eq!(status, crate::channels::CANCELED + CANCEL_REQUESTED);
                assert_eq!(image, none);
                {
                    let _role = role::Guard::new(ctx);
                    (*ctx).shield = 1;
                }
                hirsel_rt_v2_channel_receive(
                    ctx,
                    0,
                    0,
                    &mut status,
                    &mut endpoints[1],
                    image.as_mut_ptr().cast(),
                );
                assert_eq!(status, crate::channels::DONE);
                assert_eq!(image, [1, 99]);
                image = none;
                {
                    let _role = role::Guard::new(ctx);
                    assert_eq!(close_endpoint(ctx, endpoints[0], 0, 0), STATUS_OK);
                }
                hirsel_rt_v2_channel_receive(
                    ctx,
                    0,
                    0,
                    &mut status,
                    &mut endpoints[1],
                    image.as_mut_ptr().cast(),
                );
                assert_eq!(status, crate::channels::CLOSED);
                assert_eq!(image, none);
                let _role = role::Guard::new(ctx);
                (*ctx).shield = 0;
            }
            STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(STATUS_OK));
    assert_eq!(outcome.pending, 0);
}

unsafe extern "C" fn child(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let shared = unsafe { Box::from_raw(data.cast::<Arc<Result>>()) };
    let mut receiver = shared.receiver;
    let mut image = [0u64; 2];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut receiver,
            image.as_mut_ptr().cast(),
        );
    }
    shared.status.store(status, Ordering::Relaxed);
    shared.value.store(image[1], Ordering::Relaxed);
    assert_eq!(image[0], u64::from(status == crate::channels::DONE));
    shared.finished.store(true, Ordering::Release);
    drop(shared);
    unsafe {
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}

#[test]
fn a_saved_receive_replays_its_entered_cut_and_its_message_or_cancellation() {
    const CHILD: &str = "HIRSEL_F1_RECEIVE_RECORDING";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::channel::receive_tests::a_saved_receive_replays_its_entered_cut_and_its_message_or_cancellation", "--nocapture"])
            .env(CHILD, "1")
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(2);
    for durable in [false, true] {
        for case in ["msg", "canceled 0", "closed"] {
            let canceled = case == "canceled 0";
            let closed = case == "closed";
            let recording = record::Recording::new(b"native waiting receive", durable);
            let install = recording.clone();
            let cutter = recording.clone();
            let mut run = crate::host::BodyRun::new(2, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let layout = tests::layout();
            let layouts = [&raw const layout];
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| {
                    let mut endpoints = [0u64; 2];
                    let none = [0u64; 2];
                    unsafe {
                        hirsel_rt_v2_channel_open(
                            ctx,
                            std::ptr::null_mut(),
                            endpoints.as_mut_ptr(),
                            &0,
                            none.as_ptr().cast(),
                            2,
                        );
                    }
                    let result = Arc::new(Result {
                        receiver: endpoints[1],
                        finished: AtomicBool::new(false),
                        status: AtomicU32::new(99),
                        value: AtomicU64::new(99),
                    });
                    let (id, channel) = unsafe {
                        let _role = role::Guard::new(ctx);
                        let stack = Stack::new();
                        let child_ctx = stack.context();
                        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                        let log = record::child(ctx).unwrap();
                        (*child_ctx).recording = Some(Box::new(log));
                        let s = scheduler(ctx);
                        let id = s.tasks.len() as u32;
                        let channel = s.channels.channel(endpoints[1], false);
                        (*child_ctx).task_id = id;
                        (*child_ctx).task_sp = stack.prepare(
                            child,
                            child_ctx.cast(),
                            Box::into_raw(Box::new(result.clone())).cast(),
                        );
                        let mut row = Task::new(1, child_ctx, None, vec![0], None);
                        row.stack = Some(stack);
                        s.tasks.push(Some(row));
                        crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
                        (id, channel)
                    };
                    tests::wait_until(|| unsafe {
                        let _role = role::Guard::new(ctx);
                        scheduler(ctx)
                            .channels
                            .waiting(channel)
                            .is_some_and(|(_, _, waiting)| !waiting.is_empty())
                    });
                    let cut = cutter.recorder.flush().unwrap().unwrap();
                    assert_eq!(cut.progress[&vec![0]], Progress::new(1, Phase::Entered));
                    assert!(!result.finished.load(Ordering::Acquire));
                    if canceled {
                        unsafe {
                            let _role = role::Guard::new(ctx);
                            scheduler(ctx).cancel(id, CANCEL_REQUESTED);
                            // Publish the notification to the saved owner.
                            super::super::try_yield(&mut *ctx).unwrap();
                        }
                    } else if closed {
                        unsafe {
                            let _role = role::Guard::new(ctx);
                            assert_eq!(close_endpoint(ctx, endpoints[0], 0, 0), STATUS_OK);
                        }
                    } else {
                        let mut image = [1u64, 77];
                        let mut status = 99;
                        unsafe {
                            hirsel_rt_v2_channel_send(
                                ctx,
                                &mut status,
                                &mut endpoints[0],
                                image.as_mut_ptr().cast(),
                            );
                        }
                        assert_eq!(status, crate::channels::DONE);
                        assert_eq!(image, none);
                    }
                    tests::wait_until(|| result.finished.load(Ordering::Acquire));
                    assert_eq!(
                        result.status.load(Ordering::Relaxed),
                        if canceled {
                            crate::channels::CANCELED + CANCEL_REQUESTED
                        } else if closed {
                            crate::channels::CLOSED
                        } else {
                            crate::channels::DONE
                        }
                    );
                    assert_eq!(
                        result.value.load(Ordering::Relaxed),
                        if canceled || closed { 0 } else { 77 }
                    );
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
            assert_eq!(cut.progress[&vec![0]], Progress::new(1, Phase::Done));
            let trace = recording.trace();
            let marked = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink();
            for end in marked.marks {
                let bytes = &marked.bytes[..end];
                let tape = hirsel_free::tape::parse(bytes).unwrap();
                if tape.cut.is_empty() {
                    continue;
                }
                let terminal = end == marked.bytes.len();
                let mut replay = Session::replay(bytes).unwrap();
                assert!(replay.start(&[]));
                assert!(replay.start(&[0]));
                let handle = (crate::free::CHANNEL, 1);
                replay.begin(&[]);
                replay.create(&[], handle).unwrap();
                replay.publish(&[], Phase::Done).unwrap();
                replay.begin(&[]);
                replay.publish(&[], Phase::Done).unwrap();
                replay.begin(&[0]);
                replay.publish(&[0], Phase::Entered).unwrap();
                if terminal {
                    if !canceled {
                        replay.begin(&[]);
                        replay
                            .touch(&[], 0, handle, if closed { "close sender" } else { "send" })
                            .unwrap();
                        replay.publish(&[], Phase::Done).unwrap();
                    }
                    let cause = replay.cancel_check(&[0], None).unwrap();
                    assert_eq!(cause, canceled.then_some(CANCEL_REQUESTED));
                    replay.receive(&[0], handle, case).unwrap();
                    replay.publish(&[0], Phase::Done).unwrap();
                } else {
                    assert!(!replay.admits(&[0], Phase::Done));
                }
                assert_eq!(
                    replay.finish().unwrap(),
                    Projection::new(
                        if terminal { "returned" } else { "truncated" },
                        &tape.cut,
                        &trace
                    )
                );
            }
        }
    }
}
