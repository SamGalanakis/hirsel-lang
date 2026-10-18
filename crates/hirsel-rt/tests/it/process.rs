//! The host process provider and its policy, recording and replay. The host cases
//! port Rust library/std/src/process/tests.rs (f6c1107f; third-party/NOTICES.md):
//! `smoke`, `smoke_failure`, `exit_reported_right`, `signal_reported_right`,
//! `stdout_works`, `stdin_works`, `test_process_output_fail_to_start`,
//! `test_process_output_output` and `env_empty`, `test_add_to_env`. Deadlines and
//! cancellation follow docs/spec/core-library.md#processes.
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::process::*;
use std::path::PathBuf;

struct Disposable(PathBuf);
impl Disposable {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "hirsel-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Disposable {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn grant(rules: &[(&str, &[&str])]) -> hirsel_run_config::ProcessGrant {
    hirsel_run_config::ProcessGrant {
        provider: "process.spawn@1".into(),
        rules: rules
            .iter()
            .map(|(program, env)| ProcessRule {
                program: (*program).into(),
                env: env.iter().map(|n| (*n).into()).collect(),
            })
            .collect(),
        runs: None,
        children: None,
    }
}

/// A RealIo over a fresh directory with the root Proc and a monotonic clock granted.
fn host(rules: &[(&str, &[&str])]) -> (Disposable, RealIo, u64) {
    let temp = Disposable::new();
    let mut io = RealIo::grant(&temp.0).unwrap();
    io.set_process_grant(&grant(rules));
    io.clock_grants(true, false);
    let Ok(Completion::Handle(Ok(proc))) = io.call(Request::Process { io: 1 }) else {
        panic!("root Proc")
    };
    (temp, io, proc)
}

fn run(process: u64, program: &str, args: &[&str], stdin: &[u8], max: u64) -> Request {
    Request::ProcessRun {
        process,
        program: program.into(),
        args: args.iter().map(|a| (*a).into()).collect(),
        stdin: stdin.to_vec(),
        directory: None,
        max_output: max,
        deadline: None,
    }
}

/// The MonotonicClock handle of `io` and its reading now.
fn clock(io: &mut RealIo) -> (u64, u64) {
    let Ok(Completion::ClockGrant(Ok(clock))) = io.call(Request::MonotonicClock { io: 1 }) else {
        panic!("monotonic clock")
    };
    let Ok(Completion::Monotonic(Ok(now))) = io.call(Request::Monotonic { clock }) else {
        panic!("monotonic sample")
    };
    (clock, now)
}

/// `request` bounded by a deadline `nanoseconds` after `now` on `clock`.
fn until(mut request: Request, (clock, now): (u64, u64), nanoseconds: u64) -> Request {
    let Request::ProcessRun { deadline, .. } = &mut request else {
        unreachable!()
    };
    *deadline = Some(Deadline {
        clock,
        ticks: now + nanoseconds,
    });
    request
}

const TIMED_OUT: Completion = Completion::Process(ProcessOutcome::Error(ProcessFailure::TimedOut));

fn output(status: ExitStatus, stdout: &[u8], stderr: &[u8]) -> Completion {
    Completion::Process(ProcessOutcome::Output {
        status,
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    })
}

const SH: &str = "/bin/sh";

#[test]
fn smoke_echo_and_false_report_their_exit_status_and_output() {
    let (_temp, mut io, p) = host(&[("/bin/echo", &[]), ("/bin/false", &[]), (SH, &[])]);
    assert_eq!(
        io.call(run(p, "/bin/echo", &["hello", "world"], b"", 64)),
        Ok(output(ExitStatus::Exited(0), b"hello world\n", b""))
    );
    // A nonzero status is an ordinary completion, not an error.
    assert_eq!(
        io.call(run(p, "/bin/false", &[], b"", 64)),
        Ok(output(ExitStatus::Exited(1), b"", b""))
    );
    assert_eq!(
        io.call(run(
            p,
            SH,
            &["-c", "echo out; echo err >&2; exit 7"],
            b"",
            64
        )),
        Ok(output(ExitStatus::Exited(7), b"out\n", b"err\n"))
    );
    assert_eq!(
        io.call(run(p, SH, &["-c", "kill -9 $$"], b"", 64)),
        Ok(output(ExitStatus::Signaled(9), b"", b""))
    );
}

#[test]
fn stdin_is_written_then_closed_and_a_child_may_ignore_it() {
    let (_temp, mut io, p) = host(&[("/bin/cat", &[]), ("/bin/true", &[])]);
    assert_eq!(
        io.call(run(p, "/bin/cat", &[], b"foobar\n", 64)),
        Ok(output(ExitStatus::Exited(0), b"foobar\n", b""))
    );
    // Far beyond a pipe buffer: the unread rest ends the writing, never this program.
    let big = vec![b'x'; 4 << 20];
    assert_eq!(
        io.call(run(p, "/bin/true", &[], &big, 64)),
        Ok(output(ExitStatus::Exited(0), b"", b""))
    );
    let echoed = io.call(run(p, "/bin/cat", &[], &big, 4 << 20)).unwrap();
    assert_eq!(echoed, output(ExitStatus::Exited(0), &big, b""));
}

#[test]
fn both_streams_drain_concurrently_and_the_bound_kills_a_flood() {
    let (_temp, mut io, p) = host(&[(SH, &[])]);
    let script = "head -c 300000 /dev/zero; head -c 300000 /dev/zero >&2; head -c 300000 /dev/zero";
    assert_eq!(
        io.call(run(p, SH, &["-c", script], b"", 600000)),
        Ok(output(ExitStatus::Exited(0), &[0; 600000], &[0; 300000]))
    );
    assert_eq!(
        io.call(run(p, SH, &["-c", script], b"", 599999)),
        Ok(Completion::Process(ProcessOutcome::Error(
            ProcessFailure::OutputLimit
        )))
    );
    // An endless writer is stopped, not waited for.
    assert_eq!(
        io.call(run(p, SH, &["-c", "yes"], b"", 4096)),
        Ok(Completion::Process(ProcessOutcome::Error(
            ProcessFailure::OutputLimit
        )))
    );
    assert_eq!(
        io.call(run(p, SH, &["-c", "printf x"], b"", 0)),
        Ok(Completion::Process(ProcessOutcome::Error(
            ProcessFailure::OutputLimit
        )))
    );
}

#[test]
fn the_environment_holds_only_the_rule_names_and_the_directory_is_explicit() {
    let (temp, mut io, p) = host(&[("/usr/bin/env", &[]), (SH, &["PATH", "HIRSEL_UNSET_NAME"])]);
    assert_eq!(
        io.call(run(p, "/usr/bin/env", &[], b"", 4096)),
        Ok(output(ExitStatus::Exited(0), b"", b""))
    );
    let path = std::env::var("PATH").unwrap();
    assert_eq!(
        io.call(run(
            p,
            SH,
            &["-c", "echo \"$PATH|${HIRSEL_UNSET_NAME-unset}|$HOME\"; pwd"],
            b"",
            4096
        )),
        Ok(output(
            ExitStatus::Exited(0),
            format!("{path}|unset|\n/\n").as_bytes(),
            b""
        ))
    );
    let Ok(Completion::Handle(Ok(dir))) = io.call(Request::Directory {
        io: 1,
        path: "".into(),
    }) else {
        panic!("root Dir")
    };
    let mut request = run(p, SH, &["-c", "pwd -P"], b"", 4096);
    let Request::ProcessRun { directory, .. } = &mut request else {
        unreachable!()
    };
    *directory = Some(dir);
    let expected = format!("{}\n", std::fs::canonicalize(&temp.0).unwrap().display());
    assert_eq!(
        io.call(request),
        Ok(output(ExitStatus::Exited(0), expected.as_bytes(), b""))
    );
}

#[test]
fn checks_precede_every_effect_and_spawn_failures_are_typed() {
    let (temp, mut io, p) = host(&[
        ("/bin/echo", &[]),
        ("/nonexistent/hirsel-program", &[]),
        ("/etc/passwd", &[]),
    ]);
    let marker = temp.0.join("spawned");
    for (program, args, failure) in [
        ("echo", vec![], ProcessFailure::InvalidInput),
        ("/bin/../bin/echo", vec![], ProcessFailure::InvalidInput),
        ("/bin/echo/", vec![], ProcessFailure::InvalidInput),
        ("/bin/echo", vec!["a\0b"], ProcessFailure::InvalidInput),
        // Granting /bin/echo grants no other spelling or program.
        ("/usr/bin/echo", vec![], ProcessFailure::NotGranted),
        (SH, vec!["-c", "touch spawned"], ProcessFailure::NotGranted),
        (
            "/nonexistent/hirsel-program",
            vec![],
            ProcessFailure::NotFound,
        ),
        ("/etc/passwd", vec![], ProcessFailure::PermissionDenied),
    ] {
        assert_eq!(
            io.call(run(p, program, &args, b"", 64)),
            Ok(Completion::Process(ProcessOutcome::Error(failure))),
            "{program}"
        );
    }
    assert!(!marker.exists());
    // Without a projection there is no Proc at all.
    let mut denied = RealIo::grant(&temp.0).unwrap();
    denied.set_projections(Default::default());
    assert_eq!(
        denied.call(Request::Process { io: 1 }),
        Ok(Completion::Handle(Err(IoError::NotGranted)))
    );
}

#[test]
fn host_runs_record_and_replay_without_spawning() {
    use hirsel_rt::launch::{parse_tape, render_tape};
    let (_temp, mut io, p) = host(&[("/bin/echo", &[]), ("/bin/false", &[])]);
    io.call(run(p, "/bin/echo", &["é", ""], b"in", 64)).unwrap();
    io.call(run(p, "/bin/false", &[], b"", 64)).unwrap();
    io.call(run(p, "/bin/cat", &[], b"", 64)).unwrap();
    let tape = io.tape().to_vec();
    assert_eq!(tape.len(), 4);
    assert_eq!(parse_tape(&render_tape(&tape).unwrap()).unwrap(), tape);
    let replay = |tape: Vec<Event>| {
        let mut sim = SimIo::new(Tree::default(), tape.clone());
        sim.set_process_grant(&grant(&[("/bin/echo", &[]), ("/bin/false", &[])]));
        tape.iter()
            .map(|event| sim.call(event.request.clone()))
            .collect::<Vec<_>>()
    };
    let replayed = replay(tape.clone());
    assert_eq!(
        replayed,
        tape.iter()
            .map(|e| Ok(e.completion.clone()))
            .collect::<Vec<_>>()
    );
    // Replay re-derives the grant: a recorded success for an ungranted program, or a
    // recorded refusal for a granted one, contradicts the bound configuration.
    for (index, forged) in [
        (3, output(ExitStatus::Exited(0), b"", b"")),
        (
            2,
            Completion::Process(ProcessOutcome::Error(ProcessFailure::NotGranted)),
        ),
        (2, output(ExitStatus::Exited(1), &[0; 65], b"")),
    ] {
        let mut tape = tape.clone();
        tape[index].completion = forged;
        assert!(replay(tape)[index].is_err());
    }
}

#[test]
fn scripted_worlds_serve_matching_entries_in_order() {
    let runs = vec![
        ProcessRun {
            program: "/usr/bin/git".into(),
            args: vec!["status".into()],
            outcome: ProcessOutcome::Output {
                status: ExitStatus::Exited(0),
                stdout: b"clean".to_vec(),
                stderr: vec![],
            },
        },
        ProcessRun {
            program: "/usr/bin/git".into(),
            args: vec!["log".into()],
            outcome: ProcessOutcome::Error(ProcessFailure::Fault(3)),
        },
    ];
    let temp = Disposable::new();
    let mut io = RealIo::grant(&temp.0).unwrap();
    io.test_context(0);
    let Ok(Completion::Handle(Ok(p))) = io.call(Request::TestProcess {
        test: 1,
        runs: runs.clone(),
        children: vec![],
    }) else {
        panic!("scripted Proc")
    };
    for (program, args, max, expected) in [
        (
            "/usr/bin/git",
            "log",
            64,
            ProcessOutcome::Error(ProcessFailure::Fault(3)),
        ),
        (
            "/usr/bin/git",
            "log",
            64,
            ProcessOutcome::Error(ProcessFailure::NotFound),
        ),
        (
            "/usr/bin/git",
            "status",
            4,
            ProcessOutcome::Error(ProcessFailure::OutputLimit),
        ),
        (
            "/usr/bin/git",
            "status",
            64,
            ProcessOutcome::Error(ProcessFailure::NotFound),
        ),
        (
            "/usr/bin/gti",
            "status",
            64,
            ProcessOutcome::Error(ProcessFailure::NotGranted),
        ),
    ] {
        assert_eq!(
            io.call(run(p, program, &[args], b"", max)),
            Ok(Completion::Process(expected))
        );
    }
}

#[test]
fn a_deadline_times_out_a_running_child_and_a_reached_one_spawns_nothing() {
    let (temp, mut io, p) = host(&[("/bin/sleep", &[]), (SH, &[])]);
    let now = clock(&mut io);
    let begun = std::time::Instant::now();
    assert_eq!(
        io.call(until(
            run(p, "/bin/sleep", &["10"], b"", 64),
            now,
            200_000_000
        )),
        Ok(TIMED_OUT)
    );
    // SIGTERM ends sleep at once: no grace was needed.
    assert!(begun.elapsed() < std::time::Duration::from_secs(5));
    // A run that ends first returns as without a deadline.
    let now = clock(&mut io);
    assert_eq!(
        io.call(until(
            run(p, SH, &["-c", "echo done"], b"", 64),
            now,
            60_000_000_000
        )),
        Ok(output(ExitStatus::Exited(0), b"done\n", b""))
    );
    // A child that ignores SIGTERM is killed once the grace passes.
    let now = clock(&mut io);
    let begun = std::time::Instant::now();
    assert_eq!(
        io.call(until(
            run(
                p,
                SH,
                &["-c", "trap '' TERM; while :; do sleep 0.05; done"],
                b"",
                64
            ),
            now,
            100_000_000
        )),
        Ok(TIMED_OUT)
    );
    assert!(begun.elapsed() >= TERMINATION_GRACE);
    // A deadline the clock already reached is checked before spawning.
    let marker = temp.0.join("spawned");
    let (clock, now) = clock(&mut io);
    let mut reached = run(p, SH, &["-c", "touch spawned"], b"", 64);
    let Request::ProcessRun {
        deadline,
        directory,
        ..
    } = &mut reached
    else {
        unreachable!()
    };
    *deadline = Some(Deadline { clock, ticks: now });
    let Ok(Completion::Handle(Ok(dir))) = io.call(Request::Directory {
        io: 1,
        path: "".into(),
    }) else {
        panic!("root Dir")
    };
    *directory = Some(dir);
    assert_eq!(io.call(reached.clone()), Ok(TIMED_OUT));
    assert!(!marker.exists());
    // Its accepted twin, with no deadline, spawns.
    let Request::ProcessRun { deadline, .. } = &mut reached else {
        unreachable!()
    };
    *deadline = None;
    assert_eq!(
        io.call(reached),
        Ok(output(ExitStatus::Exited(0), b"", b""))
    );
    assert!(marker.exists());
    // A deadline names a MonotonicClock handle, never another grant.
    assert_eq!(
        io.call(until(run(p, "/bin/sleep", &["0"], b"", 64), (p, now), 0)),
        Err(AdapterError::InvalidHandle(p))
    );
}

#[test]
fn timeouts_record_and_replay_only_under_a_deadline() {
    use hirsel_rt::launch::{parse_tape, render_tape};
    let (_temp, mut io, p) = host(&[("/bin/sleep", &[])]);
    let now = clock(&mut io);
    let request = until(run(p, "/bin/sleep", &["10"], b"", 64), now, 50_000_000);
    assert_eq!(io.call(request.clone()), Ok(TIMED_OUT));
    let tape = io.tape().to_vec();
    assert_eq!(parse_tape(&render_tape(&tape).unwrap()).unwrap(), tape);
    let replay = |tape: &[Event]| {
        let mut sim = SimIo::new(Tree::default(), tape.to_vec());
        sim.set_process_grant(&grant(&[("/bin/sleep", &[])]));
        sim.clock_grants(true, false);
        tape.iter()
            .map(|event| sim.call(event.request.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        replay(&tape),
        tape.iter()
            .map(|e| Ok(e.completion.clone()))
            .collect::<Vec<_>>()
    );
    // A recorded timeout of a run without a deadline contradicts the request.
    let mut forged = tape.clone();
    let last = forged.len() - 1;
    let Request::ProcessRun { deadline, .. } = &mut forged[last].request else {
        unreachable!()
    };
    *deadline = None;
    assert_eq!(replay(&forged)[last], Err(AdapterError::InvalidCompletion));
    // A completed timeout proves the clock reached the deadline: a later sample below it
    // is not a monotonic reading.
    let mut below = tape.clone();
    below.push(Event {
        request: Request::Monotonic { clock: now.0 },
        completion: Completion::Monotonic(Ok(now.1 + 1)),
        fault: None,
    });
    assert_eq!(
        replay(&below).last().unwrap(),
        &Err(AdapterError::InvalidCompletion)
    );
}

#[test]
fn a_cooperative_run_parks_until_its_child_exits() {
    let (_temp, mut io, p) = host(&[(SH, &[])]);
    io.set_cooperative(hirsel_rt::readiness::Readiness::default());
    io.enter(3);
    let request = run(p, SH, &["-c", "sleep 0.1; echo late"], b"", 64);
    let parked = io.tape().len();
    assert_eq!(io.call(request.clone()), Err(AdapterError::Blocked));
    assert_eq!(
        io.tape()[parked].completion,
        Completion::Blocked { committed: false }
    );
    assert!(io.cancellable(3));
    while !io.ready(3).unwrap() {
        assert!(io.wait().unwrap());
    }
    io.enter(3);
    assert_eq!(
        io.call(request),
        Ok(output(ExitStatus::Exited(0), b"late\n", b""))
    );
    io.finish(3);
    assert!(!io.wait().unwrap());
}

#[test]
fn cancelling_a_parked_run_kills_its_child_at_once() {
    let (temp, mut io, p) = host(&[(SH, &[])]);
    io.set_cooperative(hirsel_rt::readiness::Readiness::default());
    let mut request = run(p, SH, &["-c", "sleep 0.5; touch survived"], b"", 64);
    let Ok(Completion::Handle(Ok(dir))) = io.call(Request::Directory {
        io: 1,
        path: "".into(),
    }) else {
        panic!("root Dir")
    };
    let Request::ProcessRun { directory, .. } = &mut request else {
        unreachable!()
    };
    *directory = Some(dir);
    io.enter(4);
    assert_eq!(io.call(request), Err(AdapterError::Blocked));
    // The scheduler finishes a cancelled token: the child is killed and reaped then.
    let begun = std::time::Instant::now();
    io.finish(4);
    assert!(begun.elapsed() < std::time::Duration::from_millis(400));
    std::thread::sleep(std::time::Duration::from_millis(700));
    assert!(!temp.0.join("survived").exists());
    assert!(!io.wait().unwrap());
}
