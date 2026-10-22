//! A `task.select` race on the host's monotonic clock (std task.select over core
//! task.await_any, CONC-2): a clock reading decides which of two sleeping children completes
//! first, and the other is canceled. Each live run is recorded in both engines, and every
//! tape replays the same winner interpreted and compiled, without sleeping.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PROGRAM: &str = r#"// A child's result must be sendable, so it reports whether its sleep completed. The loser is
// canceled in its sleep, so it never prints.
fn sleeper(out: Stdout, c: MonotonicClock, child: u64, ns: u64) -> bool {
  match clock.monotonic(c) {
    Ok(now) => match instant.checked_add(c, now, ns) {
      Some(at) => match clock.sleep_until(c, at) {
        Ok(()) => {
          let _ = out.print(out, f"child {child} woke\n")
          true
        },
        Error(_) => false,
      },
      None() => false,
    },
    Error(_) => false,
  }
}

pub fn main(rt: Io) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let tasks = io.tasks(rt)?
  let c = io.monotonic_clock(rt)?
  // The host's clock decides the race: at an even reading child 0 is fast, at an odd one
  // child 1. The other sleeps an hour, so however loaded the host, only the fast one wakes.
  let fast = match clock.monotonic(c) {
    Ok(now) => instant.ticks(now) % 2,
    Error(_) => return Ok(()),
  }
  let hour = 3600000000000u64
  let short = 10000000u64
  let (index, slept) = task.scope(
    tasks,
    fn(s: Scope) -> (u64, bool) {
      let mut set: TaskSet[bool] of s = task.set(s)
      let first = if fast == 0 { short } else { hour }
      let second = if fast == 1 { short } else { hour }
      task.add(s, mut set, task.async(s, fn() -> bool { sleeper(out, c, 0, first) }))
      task.add(s, mut set, task.async(s, fn() -> bool { sleeper(out, c, 1, second) }))
      task.select(s, take set)
    }
  )
  let _ = out.print(out, f"winner {index} slept {slept}\n")
  Ok(())
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root_scratch = tempfile::Builder::new()
            .prefix(&format!("hirsel-select-{name}-"))
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
            serde_json::json!({"schema":1,"package":{"name":"select","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("grants.json"),
            serde_json::json!({"io": {
                "stdout": "stdout.bytes@1",
                "tasks": "tasks.cooperative@1",
                "monotonic_clock": "clock.monotonic.ns@1",
                "dirs": [],
            }})
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

fn stdout(output: &std::process::Output, what: &str) -> String {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{what}: {output:?}"
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// A live race recorded by `engine`: its tape and its output, either winner's.
fn record(fixture: &Fixture, engine: Engine, name: &str) -> (PathBuf, String) {
    let grants = fixture.0.join("grants.json");
    let tape = fixture.0.join(name);
    let live = stdout(
        &fixture.run(
            engine,
            &[
                "--grants".as_ref(),
                grants.as_os_str(),
                "--record".as_ref(),
                tape.as_os_str(),
            ],
        ),
        &format!("live {engine:?}"),
    );
    assert!(WINNERS.contains(&live.as_str()), "live {engine:?}: {live}");
    let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
    assert!(text.contains("sleep_until"), "{text}");
    (tape, live)
}

const WINNERS: [&str; 2] = [
    "child 0 woke\nwinner 0 slept true\n",
    "child 1 woke\nwinner 1 slept true\n",
];

/// Every tape replays its own winner interpreted and compiled.
fn replay(fixture: &Fixture, recorded: &[(PathBuf, String)]) {
    for (tape, live) in recorded {
        for engine in [Engine::Interpreted, Engine::Llvm] {
            assert_eq!(
                &stdout(
                    &fixture.run(engine, &["--replay".as_ref(), tape.as_os_str()]),
                    &format!("{tape:?} replayed {engine:?}"),
                ),
                live,
                "{tape:?} replayed {engine:?}"
            );
        }
    }
}

/// Interpreted races are cheap: record until each child has won once (each run's reading
/// decides, so 32 runs all agreeing has probability 2^-31), then one compiled race.
#[test]
fn a_select_race_replays_its_winner_in_either_engine() {
    let fixture = Fixture::new("llvm");
    let mut recorded: Vec<(PathBuf, String)> = Vec::new();
    for run in 0..32 {
        let (tape, live) = record(&fixture, Engine::Interpreted, &format!("tape-{run}"));
        if recorded.iter().all(|(_, seen)| *seen != live) {
            recorded.push((tape, live));
        }
        if recorded.len() == 2 {
            break;
        }
    }
    assert_eq!(recorded.len(), 2, "one child won all 32 races");
    recorded.push(record(&fixture, Engine::Llvm, "tape-llvm"));
    replay(&fixture, &recorded);
}

#[test]
#[ignore = "full gate"]
fn full_gate_cranelift_select_race_replays_its_winner_in_either_engine() {
    let fixture = Fixture::new("cranelift");
    let recorded = [record(&fixture, Engine::Cranelift, "tape-cranelift")];
    replay(&fixture, &recorded);
}
