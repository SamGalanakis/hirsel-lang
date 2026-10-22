use super::*;
use crate::host::{Host, Selection, Sink, Sinks};
use crate::io::{ClockResult, GrantResult, UnitResult};
use crate::program::{Code, Program};
use hirsel_run_config::{ProgramFacts, Root, RunConfig};
use hirsel_run_schema::{CodeReach, Environmental, IoGrants, LaunchKind, RecordingLevel, Returned};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

static RECORDING: AtomicBool = AtomicBool::new(false);
static ALTER_REQUEST: AtomicBool = AtomicBool::new(false);
static ENDING: AtomicU8 = AtomicU8::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
static NOTICE: Mutex<Option<mpsc::Sender<Arc<crate::tasks::free::host::concurrent::Service>>>> =
    Mutex::new(None);

unsafe extern "C" fn entry(
    ctx: *mut crate::TaskCtx,
    _: *mut u8,
    out: *mut u8,
    io: *const u64,
    _: *const u64,
) -> u8 {
    unsafe {
        assert_ne!(ENDING.load(Ordering::Relaxed), 4, "pre-stopped entry ran");
        assert!((*ctx).run_ref().free_production);
        assert!(crate::pool::free::enabled((*ctx).run_ref()));
        assert!(crate::tasks::free::host::native::enabled(ctx));
        if !(*ctx).run_ref().free_replay {
            assert_eq!(
                crate::tasks::free::record::on(ctx),
                RECORDING.load(Ordering::Relaxed)
            );
        }
        let mut tasks = GrantResult::default();
        assert_eq!(
            crate::io::hirsel_rt_v2_io_tasks(ctx, std::ptr::null_mut(), &mut tasks, io),
            crate::STATUS_OK
        );
        assert_eq!(tasks.tag, 0);
        let mut stdout = GrantResult::default();
        crate::io::hirsel_rt_v2_io_stdout(ctx, std::ptr::null_mut(), &mut stdout, io);
        assert_eq!(stdout.tag, 0);
        let bytes: &[u8] = if ALTER_REQUEST.load(Ordering::Relaxed) {
            b"changed-payload"
        } else {
            b"production-free"
        };
        let text = crate::text::allocate_bytes(ctx, bytes);
        let mut printed = UnitResult::of(Ok(()));
        crate::io::hirsel_rt_v2_out_print(
            ctx,
            std::ptr::null_mut(),
            &mut printed,
            &stdout.value,
            text.owner,
            text.start,
            text.length,
        );
        assert_eq!(printed.tag, 0);
        crate::release::hirsel_rt_v2_release(
            ctx,
            text.owner,
            (*text.owner).index(),
            std::ptr::null(),
            0,
            0,
        );
        let mut clock = ClockResult {
            tag: 99,
            padding: 0,
            value: 0,
        };
        crate::io::hirsel_rt_v2_io_monotonic_clock(ctx, std::ptr::null_mut(), &mut clock, io);
        assert_eq!(clock.tag, 0);
        let clock = clock.value;
        let mut now = ClockResult {
            tag: 99,
            padding: 0,
            value: 0,
        };
        crate::io::hirsel_rt_v2_clock_monotonic(ctx, std::ptr::null_mut(), &mut now, &clock);
        assert_eq!(now.tag, 0);
        let stopping = STOP.load(Ordering::Relaxed);
        if stopping && !(*ctx).run_ref().free_replay {
            let notice = NOTICE.lock().unwrap().take().unwrap();
            notice
                .send((*ctx).run_ref().free_io.get().unwrap().clone())
                .unwrap();
        }
        let deadline = now.value + if stopping { 30_000_000_000 } else { 1_000_000 };
        let mut slept = UnitResult::of(Ok(()));
        crate::io::hirsel_rt_v2_clock_sleep_until(
            ctx,
            std::ptr::null_mut(),
            &mut slept,
            &clock,
            &deadline,
        );
        assert!(!stopping, "stopped host wait returned to user code");
        assert_eq!(slept.tag, 0);
        match ENDING.load(Ordering::Relaxed) {
            2 => crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 1, 1, 0),
            3 => crate::stop::raise(ctx, Stop::Refused("fixture internal".into())),
            6 => {
                crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 1, 1, 0);
                crate::stop::raise(ctx, Stop::Failed)
            }
            5 => {
                out.cast::<u32>().write(1);
                crate::STATUS_OK
            }
            _ => {
                out.cast::<u32>().write(0);
                crate::STATUS_OK
            }
        }
    }
}

fn sinks() -> (Sinks, crate::host::Captured) {
    let (stdout, captured) = Sink::capture();
    (
        Sinks {
            stdout,
            stderr: Sink::capture().0,
            report: Sink::capture().0,
        },
        captured,
    )
}

fn facts() -> ProgramFacts {
    ProgramFacts {
        program: [1; 32],
        entry: "free_production@1/main".into(),
        launch: LaunchKind::Main,
        roots: vec![Root::Io, Root::Args],
        code: CodeReach::Safe,
        fueled: false,
    }
}

#[test]
fn ordinary_free_admission_records_real_wait_stops_and_replays_bound_terminal_outcomes() {
    let scratch = tempfile::tempdir().unwrap();
    let table = failure_table();
    for width in [1, 8] {
        for level in [
            RecordingLevel::None,
            RecordingLevel::Record,
            RecordingLevel::Durable,
        ] {
            for ending in 0..7 {
                let stopping = ending == 1;
                ENDING.store(ending, Ordering::Relaxed);
                RECORDING.store(level != RecordingLevel::None, Ordering::Relaxed);
                STOP.store(stopping, Ordering::Relaxed);
                let mut config =
                    RunConfig::empty([1; 32], "free_production@1/main".into(), LaunchKind::Main);
                config.args = Some(vec![]);
                config.io = Some(IoGrants {
                    tasks: Some("tasks.free@1".into()),
                    stdout: Some("stdout.bytes@1".into()),
                    monotonic_clock: Some("clock.monotonic.ns@1".into()),
                    ..Default::default()
                });
                config.placement = Some(
                    hirsel_run_config::Placement::from_json(
                        format!("{{\"cores\":{width}}}").as_bytes(),
                    )
                    .unwrap(),
                );
                config.recording = Some(level);
                let validated = config.validated().unwrap();
                let bound = validated.clone().bind(&facts()).unwrap();
                let launch_config = crate::launch::Config {
                    run_identity: validated.identity(),
                    hash_seed: 0,
                    random_seed: 0,
                    test_seed: 0,
                };
                let mut program =
                    Program::harness(Code::Main(entry), 4, std::ptr::null(), 0, launch_config);
                program.digest = [1; 32];
                program.entry_name = facts().entry;
                program.failure_table = Some(table);
                let host = Host::with_clock(|| Some(0));
                let (output, captured) = sinks();
                let (admitted, io) = host.admit(&bound, None, &output).unwrap();
                let path = scratch
                    .path()
                    .join(format!("{width}-{level:?}-{ending}.tape"));
                let handle = host.start(
                    &program,
                    launch_config,
                    Selection::Live {
                        admitted,
                        io,
                        tape: (level != RecordingLevel::None).then(|| path.clone()),
                    },
                    vec![],
                    output,
                );
                let control = handle.control();
                if ending == 4 {
                    control.stop();
                }
                let (notice, parked) = mpsc::channel();
                *NOTICE.lock().unwrap() = Some(notice);
                let stopper = stopping.then(|| {
                    let control = control.clone();
                    std::thread::spawn(move || {
                        let service = parked.recv_timeout(Duration::from_secs(5)).unwrap();
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !service.test_waiting() {
                            assert!(Instant::now() < deadline, "ordinary host wait did not park");
                            std::thread::yield_now();
                        }
                        control.stop();
                    })
                });
                let outcome = handle.wait();
                if let Some(stopper) = stopper {
                    stopper.join().unwrap();
                }
                let expected = match ending {
                    1 | 4 => RunOutcome::Environmental(Environmental::Stopped),
                    2 | 6 => RunOutcome::Failed {
                        primary: hirsel_run_schema::Failure {
                            rule: "FREE-TEST".into(),
                            site: hirsel_run_schema::Site {
                                definition: facts().entry,
                                file: "fixture.hir".into(),
                                line: 7,
                                column: 9,
                            },
                            operation: None,
                        },
                        causes: vec![],
                    },
                    3 => RunOutcome::Refused("fixture internal".into()),
                    5 => RunOutcome::Returned(Returned::Error),
                    _ => RunOutcome::Returned(Returned::Ok),
                };
                assert_eq!(outcome, expected);
                assert_eq!(
                    captured.bytes(),
                    if ending == 4 {
                        &b""[..]
                    } else {
                        &b"production-free"[..]
                    }
                );
                assert!(control.worker_width() <= width);
                if level == RecordingLevel::None {
                    assert!(!path.exists());
                    continue;
                }
                let bytes = std::fs::read(&path).unwrap();
                let tape = hirsel_free::tape::parse(&bytes).unwrap();
                let header = header(&bytes).unwrap();
                assert_eq!(header.run_identity, validated.identity());
                assert!(tape.end.is_some());
                if stopping {
                    assert!(
                        tape.cut
                            .values()
                            .any(|p| matches!(p.phase, hirsel_free::phase::Phase::Ended(_)))
                    );
                }
                let end = tape.end.as_ref().unwrap();
                assert!(end.outcome.starts_with(if matches!(ending, 1 | 3 | 4) {
                    "native-stop-v2/"
                } else if ending == 6 {
                    "native-failure-v2/"
                } else {
                    "native-outcome-v2/"
                }));
                assert_eq!(
                    end.task.as_deref(),
                    if matches!(ending, 3 | 6) {
                        Some(&[][..])
                    } else {
                        None
                    }
                );
                validate_receipt(&tape, &end.outcome).unwrap();
                let forged = hirsel_free::claim::Sweep {
                    withdrawn: vec![],
                    in_doubt: vec![u64::MAX],
                };
                let corrupt = match terminal(&end.outcome).unwrap() {
                    Terminal::Stop(cause) => terminal_bytes("native-stop-v2", &cause, &forged),
                    Terminal::Outcome(cause) => {
                        terminal_bytes("native-outcome-v2", &cause, &forged)
                    }
                    Terminal::Failed(cause) => terminal_bytes("native-failure-v2", &cause, &forged),
                };
                assert!(validate_receipt(&tape, &corrupt).is_err());
                for replay_width in [1, 8] {
                    let (io, run) = crate::host_launch::replay(&path).unwrap();
                    let run = run.bind(&facts()).unwrap();
                    let (output, captured) = sinks();
                    let no_clock = Host::with_clock(|| panic!("replay read admission clock"));
                    let handle = no_clock.start(
                        &program,
                        launch_config,
                        Selection::Replay { run, io },
                        vec![],
                        output,
                    );
                    handle.control().resize(replay_width).unwrap();
                    assert_eq!(handle.wait(), expected);
                    assert_eq!(
                        captured.bytes(),
                        if ending == 4 {
                            &b""[..]
                        } else {
                            &b"production-free"[..]
                        }
                    );
                }
                if ending == 4 {
                    continue;
                }
                // Same resource and result shape, different output bytes:
                // reject the initialized request before substituting success.
                ALTER_REQUEST.store(true, Ordering::Relaxed);
                let (io, run) = crate::host_launch::replay(&path).unwrap();
                let (output, captured) = sinks();
                let outcome = Host::with_clock(|| panic!("replay admission clock"))
                    .start(
                        &program,
                        launch_config,
                        Selection::Replay {
                            run: run.bind(&facts()).unwrap(),
                            io,
                        },
                        vec![],
                        output,
                    )
                    .wait();
                assert!(matches!(outcome, RunOutcome::Diverged(_)), "{outcome:?}");
                assert!(captured.bytes().is_empty());
                ALTER_REQUEST.store(false, Ordering::Relaxed);
                if ending == 0 {
                    ENDING.store(5, Ordering::Relaxed);
                    let (io, run) = crate::host_launch::replay(&path).unwrap();
                    let (output, captured) = sinks();
                    let outcome = Host::with_clock(|| panic!("replay admission clock"))
                        .start(
                            &program,
                            launch_config,
                            Selection::Replay {
                                run: run.bind(&facts()).unwrap(),
                                io,
                            },
                            vec![],
                            output,
                        )
                        .wait();
                    assert!(matches!(outcome, RunOutcome::Diverged(_)), "{outcome:?}");
                    assert_eq!(captured.bytes(), b"production-free");
                    ENDING.store(ending, Ordering::Relaxed);
                }
            }
        }
    }
}

pub(super) fn failure_table() -> crate::isolate::FailureTable {
    let definition = "free_production@1/main";
    let file = "fixture.hir";
    let rule = "FREE-TEST";
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u32.to_le_bytes()); // sites
    bytes.extend_from_slice(&1u32.to_le_bytes()); // rules
    bytes.extend_from_slice(&1u64.to_le_bytes()); // site code
    for word in [
        7,
        9,
        0,
        definition.len() as u32,
        definition.len() as u32,
        file.len() as u32,
        (definition.len() + file.len()) as u32,
        rule.len() as u32,
    ] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(definition.as_bytes());
    bytes.extend_from_slice(file.as_bytes());
    bytes.extend_from_slice(rule.as_bytes());
    crate::isolate::FailureTable::new(Box::leak(bytes.into_boxed_slice())).unwrap()
}
