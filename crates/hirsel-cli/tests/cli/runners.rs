//! Runners on the host: a caller recorded with `--record T` journals each callee run it starts
//! to `T.call-<ordinal>`. Every tape replays alone, the caller's without its callees, and
//! `hirsel tapes check` joins the set: each link it holds is checked, a link with one end
//! missing is reported unchecked, and a callee outcome that differs from the caller's completion
//! refuses the set.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Two runners: a call with io, computes that succeed, fail and fault, and a spawned call.
const PROGRAM: &str = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

fn report(rt: Io, line: String) -> Result[u64, String] {
  match io.stdout(rt) {
    Ok(o) => {
      let _ = out.print(o, line)
      Ok(1u64)
    },
    Error(_) => Error("no stdout"),
  }
}

fn half(n: u64) -> Result[u64, String] {
  failure.check(n < 100u64, Arithmetic())
  if n % 2u64 == 1u64 {
    return Error("odd")
  }
  Ok(n / 2u64)
}

fn shown(outcome: Result[u64, RunError[String]]) -> String {
  match outcome {
    Ok(n) => if n == 4u64 {
      "ok 4\n"
    } else {
      "ok\n"
    },
    Error(Failed(e)) => if e == "odd" {
      "failed odd\n"
    } else {
      "failed\n"
    },
    Error(Faulted(_)) => "faulted\n",
    Error(_) => "refused\n",
  }
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let left = io.runner(rt, "left") ? Grant
  let right = io.runner(rt, "right") ? Grant
  let called = shown(runner.call(left, report, "left\n"))
  let even = shown(runner.compute(right, half, 8u64))
  let odd = shown(runner.compute(right, half, 3u64))
  let large = shown(runner.compute(right, half, 200u64))
  let spawned = task.scope(
    tasks,
    fn(s: Scope) -> String {
      let h = runner.spawn(s, left, report, "spawned\n")
      shown(task.await(s, take h))
    }
  )
  out.print(output, called) ? Output
  out.print(output, even) ? Output
  out.print(output, odd) ? Output
  out.print(output, large) ? Output
  out.print(output, spawned) ? Output
  Ok(())
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hirsel-runners-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"runners","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("grants.json"), GRANTS).unwrap();
        Self(root)
    }
    fn tape(&self, ordinal: Option<u32>) -> PathBuf {
        match ordinal {
            None => self.0.join("T"),
            Some(n) => self.0.join(format!("T.call-{n}")),
        }
    }
    fn hirsel(&self, args: &[&std::ffi::OsStr]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(args)
            .output()
            .unwrap()
    }
    fn record(&self) -> Output {
        let manifest = self.0.join("main.hir.json");
        let grants = self.0.join("grants.json");
        let tape = self.tape(None);
        self.hirsel(&[
            "run".as_ref(),
            manifest.as_os_str(),
            "--grants".as_ref(),
            grants.as_os_str(),
            "--record".as_ref(),
            tape.as_os_str(),
        ])
    }
    fn replay(&self, tape: &Path) -> Output {
        let manifest = self.0.join("main.hir.json");
        self.hirsel(&[
            "run".as_ref(),
            manifest.as_os_str(),
            "--replay".as_ref(),
            tape.as_os_str(),
        ])
    }
    fn check(&self, ordinals: &[Option<u32>]) -> Output {
        let tapes: Vec<PathBuf> = ordinals.iter().map(|&n| self.tape(n)).collect();
        let mut args = vec!["tapes".as_ref(), "check".as_ref()];
        args.extend(tapes.iter().map(|tape| tape.as_os_str()));
        self.hirsel(&args)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Left grants its callees stdout; right runs two at once with no io.
const GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[],"runners":[{"name":"left","provider":"local.threads@1","grants":{"stdout":"stdout.bytes@1","dirs":[]},"definitions":null},{"name":"right","provider":"local.threads@1","threads":2,"grants":null,"definitions":null}]}}"#;

const ALL: [Option<u32>; 6] = [None, Some(0), Some(1), Some(2), Some(3), Some(4)];

#[test]
fn every_tape_replays_alone_and_the_set_checks() {
    let fixture = Fixture::new("linked");
    let live = fixture.record();
    assert!(live.status.success(), "{}", stderr(&live));
    // The callees print as they run; the caller prints once every call has answered.
    assert_eq!(
        stdout(&live),
        "left\nspawned\nok\nok 4\nfailed odd\nfaulted\nok\n"
    );
    let caller = fixture.replay(&fixture.tape(None));
    assert!(caller.status.success(), "{}", stderr(&caller));
    assert_eq!(stdout(&caller), "ok\nok 4\nfailed odd\nfaulted\nok\n");
    // Each callee replays as its own run: request 0 printed, 1 answered Ok, 2 returned its
    // error, 3 faulted, 4 (spawned) printed.
    let expected: [(&str, bool); 5] = [
        ("left\n", true),
        ("", true),
        ("", false),
        ("", false),
        ("spawned\n", true),
    ];
    for (ordinal, (out, ok)) in expected.into_iter().enumerate() {
        let replay = fixture.replay(&fixture.tape(Some(ordinal as u32)));
        assert_eq!(
            stdout(&replay),
            out,
            "callee {ordinal}: {}",
            stderr(&replay)
        );
        assert_eq!(
            replay.status.success(),
            ok,
            "callee {ordinal}: {}",
            stderr(&replay)
        );
        assert!(
            !stderr(&replay).contains("replay"),
            "callee {ordinal}: {}",
            stderr(&replay)
        );
    }
    let check = fixture.check(&ALL);
    assert!(check.status.success(), "{}", stderr(&check));
    assert!(
        stdout(&check).ends_with("tapes: 5 links checked, 0 unchecked\n"),
        "{}",
        stdout(&check)
    );
    assert_eq!(stdout(&check).matches("checked: ").count(), 5);
}

#[test]
fn a_partial_set_reports_the_links_it_cannot_check() {
    let fixture = Fixture::new("partial");
    let live = fixture.record();
    assert!(live.status.success(), "{}", stderr(&live));
    let check = fixture.check(&[None, Some(1)]);
    assert!(check.status.success(), "{}", stderr(&check));
    assert!(
        stdout(&check).ends_with("tapes: 1 links checked, 4 unchecked\n"),
        "{}",
        stdout(&check)
    );
    // A callee without its caller is unchecked too.
    let check = fixture.check(&[Some(2)]);
    assert!(check.status.success(), "{}", stderr(&check));
    assert!(
        stdout(&check).contains("the caller's tape is not in the set"),
        "{}",
        stdout(&check)
    );
    assert!(
        stdout(&check).ends_with("tapes: 0 links checked, 1 unchecked\n"),
        "{}",
        stdout(&check)
    );
}

#[test]
fn a_callee_outcome_that_differs_refuses_the_set() {
    let fixture = Fixture::new("tampered");
    let live = fixture.record();
    assert!(live.status.success(), "{}", stderr(&live));
    // Request 1 answered Ok(4); its callee's tape now says it returned Ok(5).
    let tape = fixture.tape(Some(1));
    let text = std::fs::read_to_string(&tape).unwrap();
    let line = text
        .lines()
        .find(|line| line.starts_with("call_return ok "))
        .expect("the callee records its outcome");
    let tampered = line.replace("0400000000000000 -> ", "0500000000000000 -> ");
    assert_ne!(line, tampered, "{line}");
    std::fs::write(&tape, text.replace(line, &tampered)).unwrap();
    let check = fixture.check(&ALL);
    assert_eq!(check.status.code(), Some(1), "{}", stdout(&check));
    assert!(
        stderr(&check).contains("the outcome bytes differ"),
        "{}",
        stderr(&check)
    );
    // The other links are untouched.
    let check = fixture.check(&[None, Some(0), Some(2), Some(3), Some(4)]);
    assert!(check.status.success(), "{}", stderr(&check));
    // Replay of the tampered callee is refused: its run no longer returns what it recorded.
    let replay = fixture.replay(&tape);
    assert!(!replay.status.success());
    assert!(stderr(&replay).contains("replay"), "{}", stderr(&replay));
}
