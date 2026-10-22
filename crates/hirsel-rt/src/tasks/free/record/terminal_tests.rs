use super::*;
use crate::io::{Completion, Request};
use crate::tape_wire::{Wire, decode};
use crate::tasks::{free::host, role};
use hirsel_free::claim::Sweep;
use hirsel_free::projection::Projection;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant};

fn request() -> Request {
    Request::Print {
        stdout: 17,
        bytes: vec![42; 4096],
    }
}

fn stop_here(ready: &SyncSender<()>, stopped: &mpsc::Receiver<()>) {
    ready.send(()).unwrap();
    stopped.recv_timeout(Duration::from_secs(5)).unwrap();
}

#[test]
fn embedding_stop_cuts_native_requests_before_publishing_the_run_outcome() {
    for width in [1, 8] {
        for durable in [false, true] {
            for point in 0..4 {
                let recording = Recording::new(b"embedding stop", durable);
                let install = recording.clone();
                let terminal = Terminal::new(recording.clone());
                let admitted = terminal.clone();
                let (ready, reached) = mpsc::sync_channel(0);
                let (release, stopped) = mpsc::sync_channel(0);
                let issued = Arc::new(AtomicBool::new(false));
                let observed = issued.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let control = run.control();
                let stop_control = control.clone();
                let stopper = std::thread::spawn(move || {
                    reached.recv_timeout(Duration::from_secs(5)).unwrap();
                    stop_control.stop();
                    // A second request returns the same winner and must not
                    // run the cutter or append a second terminal frame.
                    stop_control.stop();
                    release.send(()).unwrap();
                });
                let outcome = run.run(
                    move |ctx| unsafe {
                        crate::tasks::free::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        // The BodyRun harness replaces its initial control
                        // after setup; admit against the actual hosted one.
                        // No semantic op or stop request has run yet.
                        (*ctx).run_ref().control.terminal(admitted.clone());
                        crate::stop::check(ctx, host::prepare(ctx, request()));
                        if point == 0 {
                            stop_here(&ready, &stopped);
                        }
                        assert!(crate::stop::check(ctx, host::claim(ctx)));
                        if point == 1 {
                            stop_here(&ready, &stopped);
                        }
                        crate::stop::check(
                            ctx,
                            host::invoke(ctx, |request| {
                                observed.store(true, Ordering::Release);
                                let Request::Print { bytes, .. } = request else {
                                    unreachable!()
                                };
                                if point == 2 {
                                    stop_here(&ready, &stopped);
                                }
                                // The cutter never frees a claimed call's
                                // buffers, even after it reports InDoubt.
                                assert_eq!(bytes.len(), 4096);
                                assert_eq!(bytes[4095], 42);
                                Completion::Unit(Ok(()))
                            }),
                        );
                        crate::stop::check(ctx, host::ended(ctx));
                        if point == 3 {
                            stop_here(&ready, &stopped);
                        }
                        crate::stop::check(ctx, host::complete(ctx));
                        panic!("a stopped request delivered its completion");
                    },
                );
                stopper.join().unwrap();
                assert_eq!(outcome.ended, Err(Stop::Requested));
                // A claimed call may be leaving on another worker. Reclaim
                // before inspecting encoder traces or unwrapping the owner.
                let deadline = Instant::now() + Duration::from_secs(5);
                while control.status().reclamation_pending != 0 {
                    assert!(Instant::now() < deadline, "stopped worker did not reclaim");
                    std::thread::yield_now();
                }
                assert_eq!(issued.load(Ordering::Acquire), point >= 2);
                let (cut, sweep) = terminal.receipt.lock().unwrap().take().unwrap();
                assert_eq!(
                    sweep,
                    Sweep {
                        withdrawn: if point == 0 { vec![1] } else { vec![] },
                        in_doubt: if point == 0 { vec![] } else { vec![1] },
                    }
                );
                assert_eq!(
                    cut.progress[&Vec::new()],
                    Progress::new(
                        1,
                        match point {
                            0 => Phase::Entered,
                            1 | 2 => Phase::Claimed,
                            3 => Phase::Ended(0),
                            _ => unreachable!(),
                        }
                    )
                );
                let mut encoded = Vec::new();
                Stop::Requested.encode(&mut encoded);
                assert_eq!(decode::<Stop>(&encoded).unwrap(), Stop::Requested);
                let end = format!("native-stop-v1/{}", hirsel_run_schema::hex(&encoded));
                let projection = Projection::new(&end, &cut.progress, &recording.trace());
                drop(control);
                drop(terminal);
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .expect("no retained terminal or request owners")
                    .recorder
                    .into_sink()
                    .bytes;
                let tape = hirsel_free::tape::parse(&bytes).unwrap();
                assert_eq!(tape.end.as_ref().unwrap().outcome, end);
                assert_eq!(tape.cut, cut.progress);

                for replay_width in [1, 8] {
                    let replay = crate::tasks::free::replay::Replay::new(&bytes).unwrap();
                    let install = replay.clone();
                    let mut run = crate::host::BodyRun::new(replay_width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let replayed = run.run(
                        move |ctx| unsafe { install.root(ctx) },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            crate::stop::check(
                                ctx,
                                host::replay_request(
                                    ctx,
                                    request(),
                                    crate::io_keys::KeySet::coupled(),
                                ),
                            );
                            panic!("replay delivered a request beyond its terminal cut");
                        },
                    );
                    assert!(matches!(replayed.ended, Err(Stop::Blocked(_))));
                    assert_eq!(replayed.pending, 0);
                    assert_eq!(replay.finish().unwrap(), projection);
                }
            }
        }
    }
}

struct Broken(std::sync::atomic::AtomicU32);
impl crate::stop::Terminal for Broken {
    fn stop(&self, _: &Stop) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Err(std::io::Error::other("terminal barrier failed"))
    }
}

#[test]
fn failed_terminal_recording_is_reported_once_before_a_stop_returns() {
    let control = crate::stop::Control::default();
    let broken = Arc::new(Broken(0.into()));
    control.terminal(broken.clone());
    let expected = Stop::Environmental(hirsel_run_schema::Environmental::Recording(
        "terminal barrier failed".into(),
    ));
    assert_eq!(control.request(Stop::Requested), expected);
    assert_eq!(control.request(Stop::Failed), expected);
    assert_eq!(control.stopped(), Some(expected));
    assert_eq!(broken.0.load(Ordering::Relaxed), 1);
}

#[test]
fn terminal_binding_cuts_a_pre_stopped_run_and_releases_its_owner_at_finish() {
    let recording = Recording::new(b"stop before entry", true);
    let mut ctx = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
            0
        );
        recording.root(ctx);
        let control = (*ctx).run_ref().control.clone();
        assert_eq!(control.request(Stop::Requested), Stop::Requested);
        bind_terminal(&*ctx);
        assert!(
            recording.recorder.flush().unwrap().is_none(),
            "admission closed the gate"
        );
        crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        control.finish(0);
        // Control remains alive while the recording's final owner can leave.
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let tape = hirsel_free::tape::parse(&bytes).unwrap();
        assert_eq!(tape.cut[&Vec::new()], Progress::START);
        let mut expected = Vec::new();
        Stop::Requested.encode(&mut expected);
        assert_eq!(
            tape.end.unwrap().outcome,
            format!("native-stop-v1/{}", hirsel_run_schema::hex(&expected))
        );
        assert_eq!(control.request(Stop::Failed), Stop::Requested);
    }
}
