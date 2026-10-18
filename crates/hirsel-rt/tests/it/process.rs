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
        command: command(program, args),
        stdin: stdin.to_vec(),
        max_output: max,
        deadline: None,
    }
}

/// A Command that starts in `/` and inherits its rule's environment names.
fn command(program: &str, args: &[&str]) -> Command {
    Command {
        program: program.into(),
        args: args.iter().map(|a| (*a).into()).collect(),
        directory: None,
        env: None,
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
    let Request::ProcessRun { command, .. } = &mut request else {
        unreachable!()
    };
    command.directory = Some(dir);
    let expected = format!("{}\n", std::fs::canonicalize(&temp.0).unwrap().display());
    assert_eq!(
        io.call(request),
        Ok(output(ExitStatus::Exited(0), expected.as_bytes(), b""))
    );
}

/// Go os/exec's TestDedupEnvEcho and TestEnvNULCharacter (exec_test.go, adapted;
/// third-party/NOTICES.md): a repeated name or a NUL is invalid input rather than
/// deduplicated or a start failure.
#[test]
fn an_exact_environment_replaces_inheritance_and_names_only_granted_variables() {
    let (_temp, mut io, p) = host(&[("/usr/bin/env", &["HIRSEL_SET", "PATH"])]);
    let exact = |vars: &[(&str, &str)]| {
        let mut request = run(p, "/usr/bin/env", &[], b"", 4096);
        let Request::ProcessRun { command, .. } = &mut request else {
            unreachable!()
        };
        command.env = Some(
            vars.iter()
                .map(|(n, v)| ((*n).into(), (*v).into()))
                .collect(),
        );
        request
    };
    // Nothing is inherited: PATH, granted and set in the launcher, is absent.
    assert_eq!(
        io.call(exact(&[("HIRSEL_SET", "a b=c")])),
        Ok(output(ExitStatus::Exited(0), b"HIRSEL_SET=a b=c\n", b""))
    );
    assert_eq!(
        io.call(exact(&[])),
        Ok(output(ExitStatus::Exited(0), b"", b""))
    );
    // A name the rule does not list is not granted; malformed names and values, and a
    // repeated name, are invalid before that.
    for (vars, failure) in [
        (&[("HOME", "/")][..], ProcessFailure::NotGranted),
        (
            &[("HIRSEL_SET", "x"), ("LD_PRELOAD", "/x.so")][..],
            ProcessFailure::NotGranted,
        ),
        (&[("", "x")][..], ProcessFailure::InvalidInput),
        (&[("A=B", "x")][..], ProcessFailure::InvalidInput),
        (&[("HIRSEL_SET", "x\0")][..], ProcessFailure::InvalidInput),
        (
            &[("HIRSEL_SET", "x"), ("HIRSEL_SET", "y")][..],
            ProcessFailure::InvalidInput,
        ),
        (
            &[("LD_PRELOAD", "x"), ("A=B", "y")][..],
            ProcessFailure::InvalidInput,
        ),
    ] {
        assert_eq!(
            io.call(exact(vars)),
            Ok(Completion::Process(ProcessOutcome::Error(failure))),
            "{vars:?}"
        );
    }
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

fn pipeline(process: u64, stages: &[(&str, &[&str])], stdin: &[u8], max: u64) -> Request {
    Request::ProcessPipeline {
        process,
        stages: stages
            .iter()
            .map(|(program, args)| command(program, args))
            .collect(),
        stdin: stdin.to_vec(),
        max_output: max,
        deadline: None,
    }
}

fn piped(statuses: &[ExitStatus], stdout: &[u8], stderr: &[u8]) -> Completion {
    Completion::Pipeline(PipelineOutcome::Output {
        statuses: statuses.to_vec(),
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    })
}

/// `printf | sort | uniq -c` on the host: each stage's output feeds the next without
/// passing through the program, every status is reported, and every stage shares one
/// standard error. Go os/exec's TestIgnorePipeErrorOnSuccess (exec_test.go, adapted;
/// third-party/NOTICES.md) is the early-exit case: a stage that stops reading ends its
/// upstream with SIGPIPE, which the statuses report rather than hide.
#[test]
fn host_pipelines_connect_stages_and_report_every_status() {
    let (_temp, mut io, p) = host(&[
        ("/usr/bin/printf", &[]),
        ("/usr/bin/sort", &[]),
        ("/usr/bin/uniq", &[]),
        ("/usr/bin/yes", &[]),
        ("/usr/bin/head", &[]),
        (SH, &[]),
        ("/bin/sleep", &[]),
        ("/bin/cat", &[]),
        ("/no/such/program", &[]),
    ]);
    let counted = pipeline(
        p,
        &[
            ("/usr/bin/printf", &["b\\na\\nb\\n"]),
            ("/usr/bin/sort", &[]),
            ("/usr/bin/uniq", &["-c"]),
        ],
        b"",
        4096,
    );
    let ok = [ExitStatus::Exited(0); 3];
    assert_eq!(
        io.call(counted),
        Ok(piped(&ok, b"      1 a\n      2 b\n", b""))
    );
    // The program's input feeds the first stage.
    assert_eq!(
        io.call(pipeline(
            p,
            &[("/usr/bin/sort", &[]), ("/bin/cat", &[])],
            b"b\na\n",
            64
        )),
        Ok(piped(&ok[..2], b"a\nb\n", b""))
    );
    assert_eq!(
        io.call(pipeline(
            p,
            &[("/usr/bin/yes", &[]), ("/usr/bin/head", &["-n1"])],
            b"",
            64
        )),
        Ok(piped(
            &[ExitStatus::Signaled(13), ExitStatus::Exited(0)],
            b"y\n",
            b""
        ))
    );
    // Both stages write to the one shared standard error, here in a fixed order.
    assert_eq!(
        io.call(pipeline(
            p,
            &[
                (SH, &["-c", "echo one >&2; echo data"]),
                (SH, &["-c", "cat; echo two >&2; exit 3"]),
            ],
            b"",
            64
        )),
        Ok(piped(
            &[ExitStatus::Exited(0), ExitStatus::Exited(3)],
            b"data\n",
            b"one\ntwo\n"
        ))
    );
    // The bound covers the captured streams; an unbounded producer is killed.
    assert_eq!(
        io.call(pipeline(
            p,
            &[("/usr/bin/yes", &[]), ("/bin/cat", &[])],
            b"",
            4096
        )),
        Ok(Completion::Pipeline(PipelineOutcome::Error(
            ProcessFailure::OutputLimit
        )))
    );
    // A stage that cannot start kills and reaps the stages started before it.
    let begun = std::time::Instant::now();
    assert_eq!(
        io.call(pipeline(
            p,
            &[("/bin/sleep", &["30"]), ("/no/such/program", &[])],
            b"",
            64
        )),
        Ok(Completion::Pipeline(PipelineOutcome::Error(
            ProcessFailure::NotFound
        )))
    );
    assert!(begun.elapsed() < std::time::Duration::from_secs(10));
    // A deadline ends every stage.
    let (clock, now) = clock(&mut io);
    let mut timed = pipeline(p, &[("/bin/sleep", &["30"]), ("/bin/cat", &[])], b"", 64);
    let Request::ProcessPipeline { deadline, .. } = &mut timed else {
        unreachable!()
    };
    *deadline = Some(Deadline {
        clock,
        ticks: now + 100_000_000,
    });
    assert_eq!(
        io.call(timed),
        Ok(Completion::Pipeline(PipelineOutcome::Error(
            ProcessFailure::TimedOut
        )))
    );
    assert!(begun.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn host_pipelines_record_and_replay_without_spawning() {
    use hirsel_rt::launch::{parse_tape, render_tape};
    let rules: &[(&str, &[&str])] = &[("/usr/bin/sort", &[]), ("/bin/cat", &[])];
    let (_temp, mut io, p) = host(rules);
    io.call(pipeline(
        p,
        &[("/usr/bin/sort", &[]), ("/bin/cat", &[])],
        b"b\na\n",
        64,
    ))
    .unwrap();
    io.call(pipeline(
        p,
        &[("/usr/bin/sort", &[]), ("/bin/sh", &[])],
        b"",
        64,
    ))
    .unwrap();
    io.call(pipeline(p, &[], b"", 64)).unwrap();
    let tape = io.tape().to_vec();
    assert_eq!(tape.len(), 4);
    assert_eq!(parse_tape(&render_tape(&tape).unwrap()).unwrap(), tape);
    let replay = |tape: Vec<Event>| {
        let mut sim = SimIo::new(Tree::default(), tape.clone());
        sim.set_process_grant(&grant(rules));
        tape.iter()
            .map(|event| sim.call(event.request.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        replay(tape.clone()),
        tape.iter()
            .map(|e| Ok(e.completion.clone()))
            .collect::<Vec<_>>()
    );
    // A recorded completion with the wrong number of statuses, or past the bound,
    // contradicts the request.
    for forged in [
        piped(&[ExitStatus::Exited(0)], b"a\nb\n", b""),
        piped(&[ExitStatus::Exited(0); 2], &[0; 65], b""),
    ] {
        let mut tape = tape.clone();
        tape[1].completion = forged;
        assert!(replay(tape)[1].is_err());
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
        deadline, command, ..
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
    command.directory = Some(dir);
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
    let Request::ProcessRun { command, .. } = &mut request else {
        unreachable!()
    };
    command.directory = Some(dir);
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

fn spawn(process: u64, program: &str, args: &[&str]) -> Request {
    Request::ProcessSpawn {
        process,
        command: command(program, args),
    }
}

fn spawned(io: &mut RealIo, request: Request) -> u64 {
    match io.call(request) {
        Ok(Completion::Spawned(Ok(child))) => child,
        other => panic!("spawn: {other:?}"),
    }
}

fn write(process: u64, child: u64, bytes: &[u8]) -> Request {
    Request::ChildWrite {
        process,
        child,
        bytes: bytes.to_vec(),
    }
}

fn read(process: u64, child: u64, max: u64) -> Request {
    Request::ChildRead {
        process,
        child,
        stream: ChildStream::Stdout,
        max,
    }
}

const UNIT: Completion = Completion::ChildUnit(Ok(()));

/// Read stdout until `want` bytes arrived or end of stream.
fn read_line(io: &mut RealIo, p: u64, c: u64, want: usize) -> Vec<u8> {
    let mut got = vec![];
    while got.len() < want {
        let Ok(Completion::ChildBytes(Ok(bytes))) = io.call(read(p, c, 64)) else {
            panic!("child read")
        };
        if bytes.is_empty() {
            break;
        }
        got.extend(bytes);
    }
    got
}

#[test]
fn a_cat_child_streams_a_line_protocol_until_its_input_closes() {
    let (_temp, mut io, p) = host(&[("/bin/cat", &[])]);
    let c = spawned(&mut io, spawn(p, "/bin/cat", &[]));
    for line in [&b"ping\n"[..], b"pong\n", b""] {
        assert_eq!(io.call(write(p, c, line)), Ok(UNIT));
        assert_eq!(read_line(&mut io, p, c, line.len()), line);
    }
    assert_eq!(
        io.call(read(p, c, 0)),
        Ok(Completion::ChildBytes(Err(ProcessFailure::InvalidInput)))
    );
    let close = Request::ChildCloseStdin {
        process: p,
        child: c,
    };
    assert_eq!(io.call(close.clone()), Ok(UNIT));
    assert_eq!(io.call(close), Ok(UNIT));
    assert_eq!(
        io.call(write(p, c, b"late")),
        Ok(Completion::ChildUnit(Err(ProcessFailure::InvalidInput)))
    );
    // End of input ends cat: its output reaches end of stream, and the wait reaps it.
    assert_eq!(
        io.call(read(p, c, 64)),
        Ok(Completion::ChildBytes(Ok(vec![])))
    );
    let wait = Request::ChildWait {
        process: p,
        child: c,
        deadline: None,
    };
    assert_eq!(
        io.call(wait.clone()),
        Ok(Completion::ChildExit(Ok(ExitStatus::Exited(0))))
    );
    // The wait consumed the Child.
    assert_eq!(io.call(wait), Err(AdapterError::InvalidHandle(c)));
}

#[test]
fn spawns_make_the_run_checks_and_child_operations_need_their_own_proc() {
    let (_temp, mut io, p) = host(&[("/bin/cat", &[]), ("/no/such/program", &[])]);
    for (program, failure) in [
        ("/bin/./cat", ProcessFailure::InvalidInput),
        ("/bin/sh", ProcessFailure::NotGranted),
        ("/no/such/program", ProcessFailure::NotFound),
    ] {
        assert_eq!(
            io.call(spawn(p, program, &[])),
            Ok(Completion::Spawned(Err(failure)))
        );
    }
    let c = spawned(&mut io, spawn(p, "/bin/cat", &[]));
    let Ok(Completion::Handle(Ok(other))) = io.call(Request::Process { io: 1 }) else {
        panic!("second Proc")
    };
    assert_eq!(
        io.call(write(other, c, b"x")),
        Err(AdapterError::InvalidHandle(c))
    );
    assert_eq!(io.call(write(p, c, b"x")), Ok(UNIT));
}

/// Go os/exec's graceful-stop tests (exec_test.go `TestWaitInterrupt`, adapted;
/// third-party/NOTICES.md): a child that handles SIGTERM exits on its own terms, one that
/// ignores it is killed once the wait's deadline and grace pass.
#[test]
fn a_chosen_signal_stops_a_child_and_a_wait_deadline_terminates_one_that_ignores_it() {
    let (_temp, mut io, p) = host(&[(SH, &[]), ("/bin/sleep", &[]), ("/bin/true", &[])]);
    let signal = |io: &mut RealIo, c, signal| {
        io.call(Request::ChildKill {
            process: p,
            child: c,
            signal,
        })
    };
    let wait = |io: &mut RealIo, c, deadline| {
        io.call(Request::ChildWait {
            process: p,
            child: c,
            deadline,
        })
    };
    let handles = spawned(
        &mut io,
        spawn(
            p,
            SH,
            &[
                "-c",
                "trap 'exit 7' TERM; echo ready; while :; do sleep 0.05; done",
            ],
        ),
    );
    assert_eq!(read_line(&mut io, p, handles, 6), b"ready\n");
    assert_eq!(signal(&mut io, handles, Signal::Terminate), Ok(UNIT));
    assert_eq!(
        wait(&mut io, handles, None),
        Ok(Completion::ChildExit(Ok(ExitStatus::Exited(7))))
    );
    let sleeper = spawned(&mut io, spawn(p, "/bin/sleep", &["5"]));
    assert_eq!(signal(&mut io, sleeper, Signal::Interrupt), Ok(UNIT));
    assert_eq!(
        wait(&mut io, sleeper, None),
        Ok(Completion::ChildExit(Ok(ExitStatus::Signaled(2))))
    );
    // A child that ends before its deadline returns its status.
    let (clock, now) = clock(&mut io);
    let soon = |ns: u64| {
        Some(Deadline {
            clock,
            ticks: now + ns,
        })
    };
    let quick = spawned(&mut io, spawn(p, "/bin/true", &[]));
    assert_eq!(
        wait(&mut io, quick, soon(10_000_000_000)),
        Ok(Completion::ChildExit(Ok(ExitStatus::Exited(0))))
    );
    // One that ignores SIGTERM is killed one grace period after its deadline.
    let stubborn = spawned(
        &mut io,
        spawn(p, SH, &["-c", "trap '' TERM; echo ready; exec sleep 30"]),
    );
    assert_eq!(read_line(&mut io, p, stubborn, 6), b"ready\n");
    let begun = std::time::Instant::now();
    assert_eq!(
        wait(&mut io, stubborn, soon(100_000_000)),
        Ok(Completion::ChildExit(Err(ProcessFailure::TimedOut)))
    );
    assert!(begun.elapsed() >= TERMINATION_GRACE);
    assert!(begun.elapsed() < std::time::Duration::from_secs(10));
    // A deadline the clock already reached terminates at once, and its wait consumes the
    // Child.
    let late = spawned(&mut io, spawn(p, "/bin/sleep", &["30"]));
    assert_eq!(
        wait(&mut io, late, Some(Deadline { clock, ticks: now })),
        Ok(Completion::ChildExit(Err(ProcessFailure::TimedOut)))
    );
    assert_eq!(
        wait(&mut io, late, None),
        Err(AdapterError::InvalidHandle(late))
    );
}

#[test]
fn kill_signals_the_child_writes_after_exit_are_input_closed_and_a_drop_kills() {
    let (temp, mut io, p) = host(&[(SH, &[]), ("/bin/true", &[])]);
    let c = spawned(&mut io, spawn(p, SH, &["-c", "sleep 5"]));
    let kill = Request::ChildKill {
        process: p,
        child: c,
        signal: Signal::Kill,
    };
    assert_eq!(io.call(kill.clone()), Ok(UNIT));
    assert_eq!(io.call(kill), Ok(UNIT));
    assert_eq!(
        io.call(Request::ChildWait {
            process: p,
            child: c,
            deadline: None,
        }),
        Ok(Completion::ChildExit(Ok(ExitStatus::Signaled(9))))
    );
    // Once true's output ends it has exited, and its input is closed.
    let c = spawned(&mut io, spawn(p, "/bin/true", &[]));
    assert_eq!(read_line(&mut io, p, c, 1), b"");
    assert_eq!(
        io.call(write(p, c, b"x")),
        Ok(Completion::ChildUnit(Err(ProcessFailure::InputClosed)))
    );
    let Ok(Completion::Handle(Ok(dir))) = io.call(Request::Directory {
        io: 1,
        path: "".into(),
    }) else {
        panic!("root Dir")
    };
    let c = spawned(
        &mut io,
        Request::ProcessSpawn {
            process: p,
            command: Command {
                directory: Some(dir),
                ..command(SH, &["-c", "sleep 0.3; touch survived"])
            },
        },
    );
    let begun = std::time::Instant::now();
    assert_eq!(
        io.call(Request::Close { handle: c }),
        Ok(Completion::Unit(Ok(())))
    );
    assert!(begun.elapsed() < std::time::Duration::from_millis(250));
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!temp.0.join("survived").exists());
}

#[test]
fn children_record_and_replay_without_spawning() {
    use hirsel_rt::launch::{parse_tape, render_tape};
    let (_temp, mut io, p) = host(&[("/bin/cat", &[])]);
    let c = spawned(&mut io, spawn(p, "/bin/cat", &["-u"]));
    io.call(write(p, c, b"echo\n")).unwrap();
    assert_eq!(read_line(&mut io, p, c, 5), b"echo\n");
    io.call(Request::ChildKill {
        process: p,
        child: c,
        signal: Signal::Kill,
    })
    .unwrap();
    io.call(Request::ChildWait {
        process: p,
        child: c,
        deadline: None,
    })
    .unwrap();
    let c = spawned(&mut io, spawn(p, "/bin/cat", &[]));
    io.call(Request::Close { handle: c }).unwrap();
    io.call(spawn(p, "/bin/sh", &[])).unwrap();
    let tape = io.tape().to_vec();
    assert_eq!(parse_tape(&render_tape(&tape).unwrap()).unwrap(), tape);
    let replay = |tape: Vec<Event>| {
        let mut sim = SimIo::new(Tree::default(), tape.clone());
        sim.set_process_grant(&grant(&[("/bin/cat", &[])]));
        tape.iter()
            .map(|event| sim.call(event.request.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        replay(tape.clone()),
        tape.iter()
            .map(|e| Ok(e.completion.clone()))
            .collect::<Vec<_>>()
    );
    let spawn_at = |program: &str| {
        tape.iter()
            .position(
                |e| matches!(&e.request, Request::ProcessSpawn { command, .. } if command.program == program),
            )
            .unwrap()
    };
    let read_at = tape
        .iter()
        .position(|e| matches!(e.request, Request::ChildRead { .. }))
        .unwrap();
    let Request::ChildRead { max, .. } = tape[read_at].request else {
        unreachable!()
    };
    // Replay re-derives the checks: a spawned ungranted program, a refused granted one
    // and a read longer than its bound contradict the bound configuration.
    for (index, forged) in [
        (spawn_at("/bin/sh"), Completion::Spawned(Ok(99))),
        (
            spawn_at("/bin/cat"),
            Completion::Spawned(Err(ProcessFailure::NotGranted)),
        ),
        (
            read_at,
            Completion::ChildBytes(Ok(vec![b'x'; max as usize + 1])),
        ),
    ] {
        let mut tape = tape.clone();
        tape[index].completion = forged;
        assert!(replay(tape)[index].is_err());
    }
}

#[test]
fn cooperative_reads_and_waits_park_and_a_cancelled_wait_kills_its_child() {
    let (temp, mut io, p) = host(&[(SH, &[])]);
    io.set_cooperative(hirsel_rt::readiness::Readiness::default());
    let c = spawned(&mut io, spawn(p, SH, &["-c", "sleep 0.1; echo late"]));
    io.enter(5);
    assert_eq!(io.call(read(p, c, 64)), Err(AdapterError::Blocked));
    assert!(io.cancellable(5));
    while !io.ready(5).unwrap() {
        assert!(io.wait().unwrap());
    }
    io.enter(5);
    assert_eq!(
        io.call(read(p, c, 64)),
        Ok(Completion::ChildBytes(Ok(b"late\n".to_vec())))
    );
    io.finish(5);
    let wait = Request::ChildWait {
        process: p,
        child: c,
        deadline: None,
    };
    io.enter(6);
    let status = loop {
        match io.call(wait.clone()) {
            Err(AdapterError::Blocked) => {
                while !io.ready(6).unwrap() {
                    assert!(io.wait().unwrap());
                }
                io.enter(6);
            }
            other => break other,
        }
    };
    io.finish(6);
    assert_eq!(status, Ok(Completion::ChildExit(Ok(ExitStatus::Exited(0)))));
    let Ok(Completion::Handle(Ok(dir))) = io.call(Request::Directory {
        io: 1,
        path: "".into(),
    }) else {
        panic!("root Dir")
    };
    let c = spawned(
        &mut io,
        Request::ProcessSpawn {
            process: p,
            command: Command {
                directory: Some(dir),
                ..command(SH, &["-c", "sleep 0.3; touch survived"])
            },
        },
    );
    io.enter(7);
    assert_eq!(
        io.call(Request::ChildWait {
            process: p,
            child: c,
            deadline: None,
        }),
        Err(AdapterError::Blocked)
    );
    // Cancelling finishes the token, which kills the child the wait took; its drop
    // then closes the Child.
    let begun = std::time::Instant::now();
    io.finish(7);
    assert_eq!(
        io.call(Request::Close { handle: c }),
        Ok(Completion::Unit(Ok(())))
    );
    assert!(begun.elapsed() < std::time::Duration::from_millis(250));
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!temp.0.join("survived").exists());
    assert!(!io.wait().unwrap());
}

#[test]
fn scripted_children_play_steps_wait_at_an_expect_and_fail_a_contradiction() {
    let child = |steps| ChildRun {
        program: "/usr/bin/bc".into(),
        args: vec![],
        steps,
    };
    let temp = Disposable::new();
    let mut io = RealIo::grant(&temp.0).unwrap();
    io.test_context(0);
    let Ok(Completion::Handle(Ok(p))) = io.call(Request::TestProcess {
        test: 1,
        runs: vec![],
        children: vec![
            child(vec![
                ChildStep::Print(b"ready\n".to_vec()),
                ChildStep::Expect(b"1+1\n".to_vec()),
                ChildStep::Print(b"2\n".to_vec()),
                ChildStep::Exit(ExitStatus::Exited(4)),
            ]),
            child(vec![ChildStep::Expect(b"quit\n".to_vec())]),
        ],
    }) else {
        panic!("scripted Proc")
    };
    let c = spawned(&mut io, spawn(p, "/usr/bin/bc", &[]));
    assert_eq!(
        io.call(read(p, c, 64)),
        Ok(Completion::ChildBytes(Ok(b"ready\n".to_vec())))
    );
    assert_eq!(io.call(read(p, c, 64)), Err(AdapterError::Blocked));
    assert_eq!(io.call(write(p, c, b"1+1\n")), Ok(UNIT));
    assert_eq!(
        io.call(read(p, c, 1)),
        Ok(Completion::ChildBytes(Ok(b"2".to_vec())))
    );
    assert_eq!(
        io.call(write(p, c, b"more")),
        Ok(Completion::ChildUnit(Err(ProcessFailure::InputClosed)))
    );
    assert_eq!(
        io.call(Request::ChildWait {
            process: p,
            child: c,
            deadline: None,
        }),
        Ok(Completion::ChildExit(Ok(ExitStatus::Exited(4))))
    );
    let c = spawned(&mut io, spawn(p, "/usr/bin/bc", &[]));
    assert_eq!(
        io.call(spawn(p, "/usr/bin/bc", &[])),
        Ok(Completion::Spawned(Err(ProcessFailure::NotFound)))
    );
    assert_eq!(
        io.call(write(p, c, b"exit\n")),
        Ok(Completion::Checked(false))
    );
}

fn spawn_pipeline(process: u64, stages: &[(&str, &[&str])], max_stderr: u64) -> Request {
    Request::ProcessSpawnPipeline {
        process,
        stages: stages.iter().map(|(p, a)| command(p, a)).collect(),
        max_stderr,
    }
}

fn wait_group(process: u64, group: u64, deadline: Option<Deadline>) -> Request {
    Request::GroupWait {
        process,
        group,
        deadline,
    }
}

fn grouped(statuses: &[ExitStatus], stderr: &[&[u8]]) -> Completion {
    Completion::GroupExit(Ok(GroupOutput {
        statuses: statuses.to_vec(),
        stderr: stderr.iter().map(|e| e.to_vec()).collect(),
    }))
}

/// Wait out a cooperative request under `token`, re-issuing it whenever it is ready.
fn parked(io: &mut RealIo, token: u64, request: Request) -> Result<Completion, AdapterError> {
    io.enter(token);
    let completion = loop {
        match io.call(request.clone()) {
            Err(AdapterError::Blocked) => {
                while !io.ready(token).unwrap() {
                    assert!(io.wait().unwrap());
                }
                io.enter(token);
            }
            other => break other,
        }
    };
    io.finish(token);
    completion
}

#[test]
fn a_cat_sort_group_streams_and_waits_under_a_deadline() {
    let (_temp, mut io, p) = host(&[("/bin/cat", &[]), ("/usr/bin/sort", &[])]);
    let cat_sort: &[(&str, &[&str])] = &[("/bin/cat", &[]), ("/usr/bin/sort", &[])];
    assert_eq!(
        io.call(spawn_pipeline(p, &[], 64)),
        Ok(Completion::Spawned(Err(ProcessFailure::InvalidInput)))
    );
    assert_eq!(
        io.call(spawn_pipeline(
            p,
            &[("/bin/cat", &[]), ("/bin/sh", &[])],
            64
        )),
        Ok(Completion::Spawned(Err(ProcessFailure::NotGranted)))
    );
    let g = spawned(&mut io, spawn_pipeline(p, cat_sort, 64));
    for line in [&b"b\n"[..], b"c\n", b"a\n"] {
        assert_eq!(io.call(write(p, g, line)), Ok(UNIT));
    }
    // A group has no standard error to read, and a Child's wait does not take it.
    assert_eq!(
        io.call(Request::ChildRead {
            process: p,
            child: g,
            stream: ChildStream::Stderr,
            max: 64,
        }),
        Err(AdapterError::InvalidHandle(g))
    );
    assert_eq!(
        io.call(Request::ChildWait {
            process: p,
            child: g,
            deadline: None,
        }),
        Err(AdapterError::InvalidHandle(g))
    );
    // sort answers once its input ends.
    assert_eq!(
        io.call(Request::ChildCloseStdin {
            process: p,
            child: g,
        }),
        Ok(UNIT)
    );
    assert_eq!(read_line(&mut io, p, g, 7), b"a\nb\nc\n");
    assert_eq!(read_line(&mut io, p, g, 1), b"");
    let (clock, now) = clock(&mut io);
    let deadline = Some(Deadline {
        clock,
        ticks: now + 10_000_000_000,
    });
    assert_eq!(
        io.call(wait_group(p, g, deadline)),
        Ok(grouped(&[ExitStatus::Exited(0); 2], &[b"", b""]))
    );
    // The wait consumed the ChildGroup, and a Child is not a group.
    assert_eq!(
        io.call(wait_group(p, g, None)),
        Err(AdapterError::InvalidHandle(g))
    );
    let c = spawned(&mut io, spawn(p, "/bin/cat", &[]));
    assert_eq!(
        io.call(wait_group(p, c, None)),
        Err(AdapterError::InvalidHandle(c))
    );
}

#[test]
fn each_stage_keeps_its_own_stderr_and_the_bound_kills_every_stage() {
    let (_temp, mut io, p) = host(&[(SH, &[])]);
    let g = spawned(
        &mut io,
        spawn_pipeline(
            p,
            &[
                (SH, &["-c", "echo one >&2; cat; echo three >&2"]),
                (SH, &["-c", "echo two >&2; tr a-z A-Z"]),
            ],
            64,
        ),
    );
    assert_eq!(io.call(write(p, g, b"up\n")), Ok(UNIT));
    // tr writes once its input ends.
    assert_eq!(
        io.call(Request::ChildCloseStdin {
            process: p,
            child: g,
        }),
        Ok(UNIT)
    );
    assert_eq!(read_line(&mut io, p, g, 4), b"UP\n");
    assert_eq!(
        io.call(wait_group(p, g, None)),
        Ok(grouped(
            &[ExitStatus::Exited(0); 2],
            &[b"one\nthree\n", b"two\n"]
        ))
    );
    // A stage that floods its standard error past the bound gets every stage killed, even
    // one that would run for a minute.
    let begun = std::time::Instant::now();
    let g = spawned(
        &mut io,
        spawn_pipeline(
            p,
            &[(SH, &["-c", "yes >&2"]), (SH, &["-c", "sleep 60"])],
            4096,
        ),
    );
    assert_eq!(
        io.call(wait_group(p, g, None)),
        Ok(Completion::GroupExit(Err(ProcessFailure::OutputLimit)))
    );
    assert!(begun.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn a_group_kill_and_a_wait_deadline_stop_every_stage() {
    let (temp, mut io, p) = host(&[(SH, &[]), ("/bin/cat", &[])]);
    let g = spawned(
        &mut io,
        spawn_pipeline(p, &[("/bin/cat", &[]), ("/bin/cat", &[])], 64),
    );
    assert_eq!(
        io.call(Request::ChildKill {
            process: p,
            child: g,
            signal: Signal::Kill,
        }),
        Ok(UNIT)
    );
    assert_eq!(
        io.call(wait_group(p, g, None)),
        Ok(grouped(&[ExitStatus::Signaled(9); 2], &[b"", b""]))
    );
    // At the deadline every stage gets SIGTERM, and one that ignores it SIGKILL once the
    // grace passes: neither lives to touch its file.
    let Ok(Completion::Handle(Ok(dir))) = io.call(Request::Directory {
        io: 1,
        path: "".into(),
    }) else {
        panic!("root Dir")
    };
    let stage = |script: &str| Command {
        directory: Some(dir),
        ..command(SH, &["-c", script])
    };
    let Ok(Completion::Spawned(Ok(g))) = io.call(Request::ProcessSpawnPipeline {
        process: p,
        stages: vec![
            stage("sleep 2; touch first"),
            stage("trap '' TERM; sleep 2; touch second"),
        ],
        max_stderr: 64,
    }) else {
        panic!("spawn_pipeline")
    };
    let (clock, now) = clock(&mut io);
    let begun = std::time::Instant::now();
    assert_eq!(
        io.call(wait_group(
            p,
            g,
            Some(Deadline {
                clock,
                ticks: now + 100_000_000,
            })
        )),
        Ok(Completion::GroupExit(Err(ProcessFailure::TimedOut)))
    );
    assert!(begun.elapsed() >= TERMINATION_GRACE);
    assert!(begun.elapsed() < std::time::Duration::from_secs(2));
    std::thread::sleep(std::time::Duration::from_millis(2500) - begun.elapsed());
    assert!(!temp.0.join("first").exists());
    assert!(!temp.0.join("second").exists());
    // A dropped group kills every stage at once.
    let g = spawned(
        &mut io,
        spawn_pipeline(
            p,
            &[(SH, &["-c", "sleep 30"]), (SH, &["-c", "sleep 30"])],
            64,
        ),
    );
    let begun = std::time::Instant::now();
    assert_eq!(
        io.call(Request::Close { handle: g }),
        Ok(Completion::Unit(Ok(())))
    );
    assert!(begun.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn a_cooperative_group_parks_its_reads_and_wait() {
    let (_temp, mut io, p) = host(&[(SH, &[]), ("/bin/cat", &[])]);
    io.set_cooperative(hirsel_rt::readiness::Readiness::default());
    let g = spawned(
        &mut io,
        spawn_pipeline(
            p,
            &[
                (SH, &["-c", "sleep 0.1; echo late; echo err >&2"]),
                ("/bin/cat", &[]),
            ],
            64,
        ),
    );
    io.enter(5);
    assert_eq!(io.call(read(p, g, 64)), Err(AdapterError::Blocked));
    io.finish(5);
    assert_eq!(
        parked(&mut io, 6, read(p, g, 64)),
        Ok(Completion::ChildBytes(Ok(b"late\n".to_vec())))
    );
    assert_eq!(
        parked(&mut io, 7, wait_group(p, g, None)),
        Ok(grouped(&[ExitStatus::Exited(0); 2], &[b"err\n", b""]))
    );
}

#[test]
fn groups_record_and_replay_without_spawning() {
    use hirsel_rt::launch::{parse_tape, render_tape};
    let (_temp, mut io, p) = host(&[(SH, &[]), ("/bin/cat", &[])]);
    let g = spawned(
        &mut io,
        spawn_pipeline(
            p,
            &[(SH, &["-c", "echo e >&2; cat"]), ("/bin/cat", &[])],
            64,
        ),
    );
    io.call(write(p, g, b"echo\n")).unwrap();
    assert_eq!(read_line(&mut io, p, g, 5), b"echo\n");
    io.call(Request::ChildCloseStdin {
        process: p,
        child: g,
    })
    .unwrap();
    io.call(wait_group(p, g, None)).unwrap();
    let g = spawned(
        &mut io,
        spawn_pipeline(p, &[("/bin/cat", &[]), ("/bin/cat", &[])], 64),
    );
    io.call(Request::ChildKill {
        process: p,
        child: g,
        signal: Signal::Terminate,
    })
    .unwrap();
    io.call(Request::Close { handle: g }).unwrap();
    let tape = io.tape().to_vec();
    assert_eq!(parse_tape(&render_tape(&tape).unwrap()).unwrap(), tape);
    let replay = |tape: Vec<Event>| {
        let mut sim = SimIo::new(Tree::default(), tape.clone());
        sim.set_process_grant(&grant(&[(SH, &[]), ("/bin/cat", &[])]));
        tape.iter()
            .map(|event| sim.call(event.request.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        replay(tape.clone()),
        tape.iter()
            .map(|e| Ok(e.completion.clone()))
            .collect::<Vec<_>>()
    );
    let wait_at = tape
        .iter()
        .position(|e| matches!(e.request, Request::GroupWait { .. }))
        .unwrap();
    assert_eq!(
        tape[wait_at].completion,
        grouped(&[ExitStatus::Exited(0); 2], &[b"e\n", b""])
    );
    // Replay re-derives the shape: a wait with a missing stage or an over-bound stderr
    // contradicts the spawn.
    for forged in [
        grouped(&[ExitStatus::Exited(0)], &[b""]),
        grouped(&[ExitStatus::Exited(0); 2], &[&[b'x'; 65], b""]),
        Completion::GroupExit(Err(ProcessFailure::TimedOut)),
    ] {
        let mut tape = tape.clone();
        tape[wait_at].completion = forged;
        assert!(replay(tape)[wait_at].is_err());
    }
}
