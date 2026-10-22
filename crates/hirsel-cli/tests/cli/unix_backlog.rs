//! A cooperative Unix connect never blocks the task scheduler (core net.unix_connect,
//! CONC-3), in both engines, against a host listener that never accepts: tasks fill its
//! backlog, a connect that must wait parks only its own task while another keeps napping,
//! a scope deadline and a cancellation each end such a wait, and accepting one connection
//! lets the waiting filler connect. Each recording replays in either engine without sockets.
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

fn nap(c: MonotonicClock) -> () {
  let _ = match clock.monotonic(c) {
    Ok(now) => match instant.checked_add(c, now, 1000000u64) {
      Some(at) => clock.sleep_until(c, at),
      None() => Ok(()),
    },
    Error(_) => Ok(()),
  }
}

// A filler connects, reports it, and holds its stream in a read nothing answers.
fn hold(tasks: Tasks, n: Net, take p: Path, take tx: Sender[bool]) -> Option[CancelCause] {
  match net.unix_connect(n, p) {
    Ok(stream) => {
      let mut st = take stream
      let mut t = take tx
      let _ = channel.send(tasks, mut t, true)
      let mut buffer: List[u8] = []
      match net.read(n, mut st, mut buffer, 1u64) {
        Error(Canceled(e)) => Some(e),
        _ => None(),
      }
    },
    Error(Canceled(e)) => Some(e),
    Error(_) => None(),
  }
}

// A child's result must be sendable: its connect's cancellation, or None.
fn canceled_connect(n: Net, take p: Path) -> Option[CancelCause] {
  match net.unix_connect(n, p) {
    Error(Canceled(e)) => Some(e),
    _ => None(),
  }
}

fn outcome(c: Option[CancelCause]) -> String {
  match c {
    Some(e) => f"canceled {cause(e)}",
    None() => "not canceled",
  }
}

fn show(out: Stdout, name: String, text: String) -> () {
  let _ = out.print(out, f"{name}: {text}\n")
}

pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let tasks = io.tasks(rt)?
  let c = io.monotonic_clock(rt)?
  let n = io.net(rt)?
  let p = match arg.path(args, 0) {
    Ok(p) => p,
    Error(_) => return Ok(()),
  }
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let listener = match net.unix_listen(n, p) {
    Ok(l) => take l,
    Error(_) => return Ok(()),
  }
  let summary = task.scope(
    tasks,
    fn(s: Scope) -> String {
      // Nothing accepts, so fillers fill the backlog: each connects at once until one
      // waits, and the bounded wait for its report expires, whatever the kernel's bound.
      let (tx, receiver): (Sender[bool], Receiver[bool]) = channel.open(tasks)
      let mut rx = take receiver
      let mut fillers: TaskSet[Option[CancelCause]] of s = task.set(s)
      let mut full = false
      let mut count = 0u64
      while !full && count < 1000u64 {
        count += 1u64
        let mine = path.of(path.text(p))
        let sender = sender.dup(tasks, tx)
        task.add(
          s,
          mut fillers,
          task.async(s, fn() -> Option[CancelCause] { hold(tasks, n, take mine, take sender) })
        )
        let current = take rx
        let (connected, back) = match task.scope_until(
          tasks,
          after(c, start, 1000000000u64),
          fn(t: Scope) -> (bool, Receiver[bool]) {
            let mut r = take current
            let got = match channel.receive(tasks, mut r) {
              Ok(_) => true,
              Error(_) => false,
            }
            (got, take r)
          }
        ) {
          Finished(pair) => take pair,
          Expired(pair) => take pair,
        }
        rx = take back
        full = !connected
      }
      // A waiting connect parks only its own task: this one keeps napping meanwhile.
      let mine = path.of(path.text(p))
      let waiting = task.async(s, fn() -> Option[CancelCause] { canceled_connect(n, take mine) })
      let theirs = path.of(path.text(p))
      let probe = task.async(
        s,
        fn() -> bool {
          let q = take theirs
          match task.scope_until(
            tasks,
            after(c, start, 50000000u64),
            fn(t: Scope) -> Option[CancelCause] { canceled_connect(n, take q) }
          ) {
            Expired(Some(DeadlineExceeded())) => true,
            _ => false,
          }
        }
      )
      let mut naps = 0u64
      while naps < 3u64 {
        nap(c)
        naps += 1u64
      }
      let probed = task.await(s, take probe)
      task.cancel(s, waiting)
      let canceled = task.await(s, take waiting)
      // Accepting one makes room, so the waiting filler connects.
      let mut l = take listener
      let accepted = match net.accept(n, mut l) {
        Ok(_) => "accepted",
        Error(_) => "failed",
      }
      let filled = match channel.receive(tasks, mut rx) {
        Ok(_) => "connected",
        Error(_) => "failed",
      }
      task.cancel_all(s, take fillers)
      f"full {full}, napped {naps}, probe expired {probed}, waiting {outcome(canceled)}, {accepted}, filler {filled}"
    }
  )
  show(out, "backlog", summary)
  Ok(())
}
"#;

const EXPECTED: &str = "backlog: full true, napped 3, probe expired true, waiting canceled requested, accepted, filler connected\n";

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root_scratch = tempfile::Builder::new()
            .prefix(&format!("hirsel-unix-{name}-"))
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"unix_backlog","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        let socket = root.join("app.sock");
        let socket = socket.to_str().unwrap();
        assert!(socket.len() <= 107, "socket path too long: {socket}");
        std::fs::write(
            root.join("grants.json"),
            serde_json::json!({"io": {
                "stdout": "stdout.bytes@1",
                "tasks": "tasks.cooperative@1",
                "monotonic_clock": "clock.monotonic.ns@1",
                "dirs": [],
                "net": {
                    "provider": "net.stream@1",
                    "rules": [
                        {"address": socket, "service": "0", "operation": "unix_connect"},
                        {"address": socket, "service": "0", "operation": "unix_listen"}
                    ]
                },
            }, "args": [socket]})
            .to_string(),
        )
        .unwrap();
        let _ = root_scratch.keep();
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
    let text = hirsel_rt::launch::read_recording_text(&tapes[0]).unwrap();
    assert!(text.contains("unix_connect"), "{text}");
    for tape in &tapes {
        for engine in [Engine::Interpreted, Engine::Llvm] {
            assert_output(
                &fixture.run(engine, &["--replay".as_ref(), tape.as_os_str()]),
                &format!("{} replayed {engine:?}", tape.display()),
            );
        }
    }
}

#[test]
fn unix_connects_to_a_full_backlog_park_their_task_and_replay_in_either_engine() {
    round_trip(&[Engine::Interpreted, Engine::Llvm]);
}

#[test]
#[ignore = "full gate"]
fn full_gate_cranelift_unix_connects_to_a_full_backlog_park_their_task() {
    round_trip(&[Engine::Cranelift]);
}
