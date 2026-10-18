//! Real-host random.system@1 draws and timer waits: a live run draws fresh entropy and
//! really sleeps; its tape replays the same output without entropy or waiting.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const TOKEN: &str = r#"pub type AppError = Grant(GrantError) | Draw(RandomError) | Sleep(
  time.SleepError
) | Clock(ClockError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let r = io.random(rt) ? Grant
  let c = io.monotonic_clock(rt) ? Grant
  let token = random.bytes(r, 16) ? Draw
  let start = clock.monotonic(c) ? Clock
  time.sleep(c, duration.from_parts(0, 50000000)) ? Sleep
  let slept = time.elapsed(c, start) ? Clock
  out.print(output, hex.encode(token)) ? Output
  out.print(output, "\n") ? Output
  if slept.nanoseconds >= 50000000 {
    out.print(output, "slept\n") ? Output
  }
  Ok(())
}
"#;

// Register the early wait first: host preemption may expire deadlines before a
// later child starts. Deterministic overdue controls cover reversed waits and ties.
// Cancelling the hour-long sleeper removes its timer, so the short sleeps end the run.
const SLEEPERS: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let output = io.stdout(rt)?
  let tasks = io.tasks(rt)?
  let c = io.monotonic_clock(rt)?
  let start = match clock.monotonic(c) {
    Ok(t) => t,
    Error(_) => return Ok(()),
  }
  let late_deadline = match instant.checked_add(c, start, 60000000u64) {
    Some(t) => t,
    None() => return Ok(()),
  }
  let early_deadline = match instant.checked_add(c, start, 20000000u64) {
    Some(t) => t,
    None() => return Ok(()),
  }
  task.scope(
    tasks,
    fn(s: Scope) {
      let long = task.async(
        s,
        fn() -> bool {
          match time.sleep(c, duration.from_parts(3600, 0)) {
            Error(Wait(Canceled(Requested()))) => true,
            _ => false,
          }
        }
      )
      let early = task.async(
        s,
        fn() -> bool {
          let slept = match clock.sleep_until(c, early_deadline) {
            Ok(()) => true,
            Error(_) => false,
          }
          let _ = out.print(output, "early\n")
          slept
        }
      )
      let late = task.async(
        s,
        fn() -> bool {
          let slept = match clock.sleep_until(c, late_deadline) {
            Ok(()) => true,
            Error(_) => false,
          }
          let _ = out.print(output, "late\n")
          slept
        }
      )
      let both = task.await(s, take late) && task.await(s, take early)
      task.cancel(s, long)
      if both && task.await(s, take long) {
        let _ = out.print(output, "canceled\n")
      }
    }
  )
  Ok(())
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, program: &str, grants: serde_json::Value) -> Self {
        let root =
            std::env::temp_dir().join(format!("hirsel-entropy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.hir"), program).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"entropy","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("grants.json"), grants.to_string()).unwrap();
        Self(root)
    }
    fn run(&self, native: bool, args: &[&std::ffi::OsStr]) -> (Vec<u8>, Duration) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .arg(self.0.join("main.hir.json"))
            .args(args);
        if native {
            command.arg("--native");
        }
        let begun = Instant::now();
        let output = command.output().unwrap();
        let elapsed = begun.elapsed();
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{output:?}"
        );
        (output.stdout, elapsed)
    }
    fn live(&self, native: bool, tape: Option<&str>) -> (Vec<u8>, Duration) {
        let grants = self.0.join("grants.json");
        let tape = tape.map(|name| self.0.join(name));
        let mut args = vec!["--grants".as_ref(), grants.as_os_str()];
        if let Some(tape) = &tape {
            args.extend(["--record".as_ref(), tape.as_os_str()]);
        }
        self.run(native, &args)
    }
    fn replay(&self, native: bool, tape: &str) -> (Vec<u8>, Duration) {
        let tape = self.0.join(tape);
        self.run(native, &["--replay".as_ref(), tape.as_os_str()])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn token_fixture(name: &str) -> Fixture {
    Fixture::new(
        name,
        TOKEN,
        serde_json::json!({"io":{"stdout":"stdout.bytes@1","random":"random.system@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]}}),
    )
}

fn token_lines(stdout: &[u8]) -> String {
    let text = String::from_utf8(stdout.to_vec()).unwrap();
    let (token, rest) = text.split_once('\n').unwrap();
    assert_eq!(token.len(), 32, "{text}");
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()), "{text}");
    assert_eq!(rest, "slept\n", "{text}");
    token.into()
}

#[test]
fn system_entropy_and_real_sleeps_record_and_replay_without_either() {
    let fixture = token_fixture("interpreted");
    let (first, slept) = fixture.live(false, Some("first.tape"));
    assert!(slept >= Duration::from_millis(50));
    let (second, _) = fixture.live(false, None);
    // Two 128-bit host draws are equal with probability 2^-128.
    assert_ne!(token_lines(&first), token_lines(&second));
    let (replayed, _) = fixture.replay(false, "first.tape");
    assert_eq!(replayed, first);
    let tape = std::fs::read_to_string(fixture.0.join("first.tape")).unwrap();
    assert!(tape.contains("random_bytes"), "{tape}");
    assert!(tape.contains("sleep_until"), "{tape}");
}

fn sleepers_fixture(name: &str) -> Fixture {
    Fixture::new(
        name,
        SLEEPERS,
        serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]}}),
    )
}

#[test]
fn interpreted_sleepers_wake_by_deadline_and_a_cancelled_wait_drops_its_timer() {
    let fixture = sleepers_fixture("sleepers");
    let (stdout, _) = fixture.live(false, Some("sleepers.tape"));
    assert_eq!(stdout, b"early\nlate\ncanceled\n");
    // The parks and ready polls replay the schedule without waiting.
    assert_eq!(fixture.replay(false, "sleepers.tape").0, stdout);
}

#[test]
#[ignore = "full gate"]
fn full_gate_native_sleepers_park_on_timers_and_replay_in_either_engine() {
    let fixture = sleepers_fixture("native-sleepers");
    let (stdout, _) = fixture.live(true, Some("native.tape"));
    assert_eq!(stdout, b"early\nlate\ncanceled\n");
    assert_eq!(fixture.replay(true, "native.tape").0, stdout);
    assert_eq!(fixture.replay(false, "native.tape").0, stdout);
}

#[test]
#[ignore = "full gate"]
fn full_gate_native_system_entropy_and_sleeps_replay_in_either_engine() {
    let fixture = token_fixture("native");
    let (live, slept) = fixture.live(true, Some("native.tape"));
    assert!(slept >= Duration::from_millis(50));
    token_lines(&live);
    assert_eq!(fixture.replay(true, "native.tape").0, live);
    assert_eq!(fixture.replay(false, "native.tape").0, live);
}
