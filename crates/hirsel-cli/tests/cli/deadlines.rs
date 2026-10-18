//! Scope deadlines on the host's monotonic clock (core task.scope_until, CONC-3), in both
//! engines: each bounded operation (a sleep, a channel receive, a stalled network read and
//! a child's read) never completes by itself, so only its deadline ends it whatever the
//! host's load, and each recording replays in either engine without waiting.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PROGRAM: &str = r#"fn cause(e: CancelCause) -> String {
  match e {
    Requested() => "requested",
    Parent() => "parent",
    ChildFailed() => "child failed",
    DeadlineExceeded() => "deadline",
  }
}

fn report(b: Bounded[String]) -> String {
  match b {
    Finished(text) => f"finished {text}",
    Expired(text) => f"expired {text}",
  }
}

// `ns` from now; a clock that cannot be read gives the already reached `start`.
fn after(c: MonotonicClock, start: Instant of c, ns: u64) -> Deadline {
  match clock.monotonic(c) {
    Ok(now) => match instant.checked_add(c, now, ns) {
      Some(at) => Deadline(clock: c, at: at),
      None() => Deadline(clock: c, at: now),
    },
    Error(_) => Deadline(clock: c, at: start),
  }
}

// A task's result must be sendable, so a child reports its cancellation, not text.
fn canceled_sleep(c: MonotonicClock) -> Option[CancelCause] {
  match clock.monotonic(c) {
    Ok(now) => match instant.checked_add(c, now, 3600000000000u64) {
      Some(at) => match clock.sleep_until(c, at) {
        Error(Canceled(e)) => Some(e),
        _ => None(),
      },
      None() => None(),
    },
    Error(_) => None(),
  }
}

fn sleep(c: MonotonicClock) -> String {
  match canceled_sleep(c) {
    Some(e) => cause(e),
    None() => "slept",
  }
}

fn show(out: Stdout, name: String, text: String) -> () {
  let _ = out.print(out, f"{name}: {text}\n")
}

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let tasks = io.tasks(rt)?
  let c = io.monotonic_clock(rt)?
  let p = io.process(rt)?
  let n = io.net(rt)?
  let hang = match arg.path(args, 0) {
    Ok(program) => program,
    Error(_) => return Ok(()),
  }
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let short = 50000000u64
  let hour = 3600000000000u64
  show(
    out,
    "sleep",
    report(task.scope_until(tasks, after(c, start, short), fn(s: Scope) -> String { sleep(c) }))
  )
  let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
  show(
    out,
    "receive",
    report(
      task.scope_until(
        tasks,
        after(c, start, short),
        fn(s: Scope) -> String {
          // No child creation races expiry: this wait always sees this scope's cause.
          let mut r = take receiver
          let received = match channel.receive(tasks, mut r) {
            Error(Canceled(e)) => cause(e),
            _ => "received",
          }
          let checked = match task.check_canceled(s) {
            Ok(()) => "running",
            Error(Canceled(e)) => cause(e),
          }
          f"{received} {checked}"
        }
      )
    )
  )
  // Deliver the success read before arming the short scope. Only the stalled
  // second read competes with its deadline, so pool load cannot change the result.
  let network = match net.connect(n, "example.com", 80u16, Unbounded()) {
    Ok(stream) => {
      let mut first_stream = take stream
      let mut first_buffer: List[u8] = []
      let first = match net.read(n, mut first_stream, mut first_buffer, 16u64) {
        Ok(count) => f"read {count}",
        Error(_) => "failed",
      }
      report(
        task.scope_until(
          tasks,
          after(c, start, short),
          fn(s: Scope) -> String {
            let mut st = take first_stream
            let mut buffer: List[u8] = []
            let second = match net.read(n, mut st, mut buffer, 16u64) {
              Ok(count) => f"read {count}",
              Error(Canceled(e)) => cause(e),
              Error(_) => "failed",
            }
            f"{first}, {second}"
          }
        )
      )
    },
    Error(_) => "unconnected",
  }
  show(out, "net", network)
  // Starting the child also precedes the short scope; it never produces stdout
  // or exits by itself, so the scope alone ends the read.
  let started = match proc.spawn(p, hang, ["infinity"], None()) {
    Ok(child) => take child,
    Error(_) => return Ok(()),
  }
  show(
    out,
    "child",
    report(
      task.scope_until(
        tasks,
        after(c, start, short),
        fn(s: Scope) -> String {
          let mut test = take started
          let mut buffer: List[u8] = []
          match child.read_stdout(p, mut test, mut buffer, 16u64) {
            Ok(count) => f"read {count}",
            Error(Canceled(e)) => cause(e),
            Error(_) => "failed",
          }
        }
      )
    )
  )
  show(
    out,
    "never",
    report(task.scope_until(tasks, after(c, start, hour), fn(s: Scope) -> String { "done" }))
  )
  let outer = task.scope_until(
    tasks,
    after(c, start, hour),
    fn(s: Scope) -> String {
      let inner = task.scope_until(
        tasks,
        after(c, start, short),
        fn(t: Scope) -> String { sleep(c) }
      )
      show(out, "inner first", f"inner {report(inner)}")
      "done"
    }
  )
  show(out, "inner first", f"outer {report(outer)}")
  show(
    out,
    "outer first",
    report(
      task.scope_until(
        tasks,
        after(c, start, short),
        fn(s: Scope) -> String {
          let inner = task.scope_until(
            tasks,
            after(c, start, hour),
            fn(t: Scope) -> String { sleep(c) }
          )
          f"inner {report(inner)}"
        }
      )
    )
  )
  show(
    out,
    "past",
    report(
      task.scope_until(
        tasks,
        after(c, start, 0u64),
        fn(s: Scope) -> String {
          let checked = match task.check_canceled(s) {
            Ok(()) => "running",
            Error(Canceled(e)) => cause(e),
          }
          let child = task.async(s, fn() -> Option[CancelCause] { canceled_sleep(c) })
          let spawned = match task.await(s, take child) {
            Some(e) => cause(e),
            None() => "slept",
          }
          f"{checked} {spawned} {sleep(c)}"
        }
      )
    )
  )
  let mut tx = take sender
  let _ = channel.send(tasks, mut tx, 1u64)
  Ok(())
}
"#;

const EXPECTED: &str = "sleep: expired deadline
receive: expired deadline deadline
net: expired read 2, deadline
child: expired deadline
never: finished done
inner first: inner expired deadline
inner first: outer finished done
outer first: expired inner finished deadline
past: expired deadline parent deadline
";

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hirsel-deadlines-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"deadlines","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        let hang = root.join("sleep");
        std::fs::copy("/bin/sleep", &hang).unwrap();
        std::fs::write(
            root.join("grants.json"),
            serde_json::json!({"io": {
                "stdout": "stdout.bytes@1",
                "tasks": "tasks.cooperative@1",
                "monotonic_clock": "clock.monotonic.ns@1",
                "dirs": [],
                "process": {"provider": "process.spawn@1", "rules": [{"program": hang}]},
                "net": {
                    "provider": "net.script@2",
                    "rules": [{"address": "example.com", "service": "80", "operation": "connect"}],
                    "peers": [{
                        "address": "example.com", "port": 80, "resolves_to": ["127.0.0.1"],
                        "tls": "plain", "connections": [{"open": [{"send": [104, 105]}, "stall"]}]
                    }]
                },
            }, "args": [hang]})
            .to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn run(&self, engine: Engine, args: &[&std::ffi::OsStr]) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .arg(self.0.join("main.hir.json"))
            .args(args);
        match engine {
            Engine::Interpreted => {}
            Engine::Llvm => {
                command.arg("--native");
            }
            Engine::Cranelift => {
                command.args(["--native", "--backend", "cranelift"]);
            }
        }
        command.stdin(Stdio::null()).output().unwrap()
    }
}
#[derive(Clone, Copy, Debug)]
enum Engine {
    Interpreted,
    Llvm,
    Cranelift,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_output(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{what}: {output:?}"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED, "{what}");
}

/// Record a live run in each of `engines`, then replay every tape interpreted and
/// compiled.
fn round_trip(engines: &[Engine]) {
    let fixture = Fixture::new(&format!("{:?}", engines[0]));
    let grants = fixture.0.join("grants.json");
    let tapes: Vec<PathBuf> = engines
        .iter()
        .map(|&engine| {
            let tape = fixture.0.join(format!("tape-{engine:?}"));
            let output = fixture.run(
                engine,
                &[
                    "--grants".as_ref(),
                    grants.as_os_str(),
                    "--record".as_ref(),
                    tape.as_os_str(),
                ],
            );
            assert_output(&output, &format!("live {engine:?}"));
            tape
        })
        .collect();
    // The deadlines are recorded as the timer waits they arm.
    let text = std::fs::read_to_string(&tapes[0]).unwrap();
    assert!(text.contains("sleep_until"), "{text}");
    // Replay spawns nothing: the granted program is gone.
    std::fs::remove_file(fixture.0.join("sleep")).unwrap();
    for tape in &tapes {
        for engine in [Engine::Interpreted, Engine::Llvm] {
            assert_output(
                &fixture.run(engine, &["--replay".as_ref(), tape.as_os_str()]),
                &format!("{} replayed {engine:?}", tape_name(tape)),
            );
        }
    }
}

fn tape_name(tape: &Path) -> String {
    tape.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn scope_deadlines_end_waits_on_the_host_clock_and_replay_in_either_engine() {
    round_trip(&[Engine::Interpreted, Engine::Llvm]);
}

#[test]
#[ignore = "full gate"]
fn full_gate_cranelift_scope_deadlines_match_and_replay_in_either_engine() {
    round_trip(&[Engine::Cranelift]);
}
