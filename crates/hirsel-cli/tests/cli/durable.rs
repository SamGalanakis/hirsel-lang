//! The durable recording level: a host run whose record selects `recording: durable` journals
//! every request before issue and every completion before delivery. Its journal replays in
//! either engine; a journal cut at any record boundary refuses replay and names the requests
//! in doubt; the level's destination rule refuses before entry.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Concurrent sleepers whose waits share a batch, durable puts, a write, and a cancelled
/// wait whose request is withdrawn.
const PROGRAM: &str = r#"pub type AppError = Grant(GrantError) | Files(FileError) | Output(OutputError)

fn sleeper(c: MonotonicClock, d: Dir, name: String, ms: i32) -> bool {
  let slept = match time.sleep(c, duration.from_parts(0, ms * 1000000)) {
    Ok(()) => true,
    Error(_) => false,
  }
  let put = match file.put_new(d, path.of(name), b"kept", Durable()) {
    Ok(()) => true,
    Error(_) => false,
  }
  slept && put
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let c = io.monotonic_clock(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  file.write_bytes(d, path.of("start"), b"begun") ? Files
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
      let a = task.async(s, fn() -> bool { sleeper(c, d, "a", 10) })
      let b = task.async(s, fn() -> bool { sleeper(c, d, "b", 20) })
      let both = task.await(s, take a) && task.await(s, take b)
      task.cancel(s, long)
      if both && task.await(s, take long) {
        let _ = out.print(output, "slept and put\n")
      }
    }
  )
  if file.read_bytes(d, path.of("a")) ? Files == b"kept" {
    out.print(output, "kept\n") ? Output
  }
  Ok(())
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, recording: Option<&str>) -> Self {
        let root =
            std::env::temp_dir().join(format!("hirsel-durable-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"durable","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        let mut grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[{"path":"data","host":root.join("data"),"access":"read_write"}]},"args":[]});
        if let Some(level) = recording {
            grants["recording"] = level.into();
        }
        std::fs::write(root.join("grants.json"), grants.to_string()).unwrap();
        Self(root)
    }
    fn run(&self, native: bool, args: &[&std::ffi::OsStr]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .arg(self.0.join("main.hir.json"))
            .args(args);
        if native {
            command.arg("--native");
        }
        command.output().unwrap()
    }
    fn live(&self, native: bool, journal: Option<&Path>) -> Output {
        let grants = self.0.join("grants.json");
        let mut args = vec!["--grants".as_ref(), grants.as_os_str()];
        if let Some(journal) = journal {
            args.extend(["--record".as_ref(), journal.as_os_str()]);
        }
        // Each live run starts from an empty data directory.
        let _ = std::fs::remove_dir_all(self.0.join("data"));
        std::fs::create_dir_all(self.0.join("data")).unwrap();
        self.run(native, &args)
    }
    fn replay(&self, native: bool, journal: &Path) -> Output {
        self.run(native, &["--replay".as_ref(), journal.as_os_str()])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const MAGIC: &[u8] = hirsel_rt::journal::MAGIC;

/// Each record's end offset, from the frame lengths (len, crc, seq, kind, payload).
fn boundaries(bytes: &[u8]) -> Vec<usize> {
    let mut ends = vec![];
    let mut at = MAGIC.len();
    while at < bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        at += 17 + len;
        ends.push(at);
    }
    assert_eq!(at, bytes.len());
    ends
}

fn assert_ran(output: &Output, what: &str) {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{what}: {output:?}"
    );
    assert_eq!(
        output.stdout, b"slept and put\nkept\n",
        "{what}: {output:?}"
    );
}

#[test]
fn durable_journals_replay_in_either_engine_and_a_cut_journal_names_its_requests_in_doubt() {
    let fixture = Fixture::new("journals", Some("durable"));
    for native in [false, true] {
        let journal = fixture.0.join(format!("run-{native}.journal"));
        assert_ran(&fixture.live(native, Some(&journal)), "live");
        let bytes = std::fs::read(&journal).unwrap();
        assert!(bytes.starts_with(MAGIC), "native={native}");
        // The data directory is gone: replay performs no effect.
        std::fs::remove_dir_all(fixture.0.join("data")).unwrap();
        for engine in [false, true] {
            assert_ran(
                &fixture.replay(engine, &journal),
                &format!("recorded native={native}, replayed native={engine}"),
            );
        }
        // A crash at a record boundary leaves a prefix that refuses replay: no trailer, and a
        // request prepared but not completed is named in doubt. Each replay compiles the
        // program, so this samples boundaries; the runtime's journal tests cut at every one.
        let ends = boundaries(&bytes);
        let mut doubted = 0;
        // Three samples; odd, so they fall after Prepared records and after completions alike.
        let step = (ends.len() / 3).max(1) | 1;
        for end in ends[..ends.len() - 1].iter().step_by(step) {
            let cut = fixture.0.join("cut.journal");
            std::fs::write(&cut, &bytes[..*end]).unwrap();
            let output = fixture.replay(false, &cut);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success()
                    && output.stdout.is_empty()
                    && stderr.contains("no trailer"),
                "cut at {end}: {output:?}"
            );
            doubted += usize::from(stderr.contains("in doubt: request"));
        }
        assert!(doubted > 0, "native={native}");
        // A torn last record is cut like a boundary; earlier damage is corruption.
        let cut = fixture.0.join("torn.journal");
        std::fs::write(&cut, &bytes[..bytes.len() - 1]).unwrap();
        assert!(String::from_utf8_lossy(&fixture.replay(true, &cut).stderr).contains("no trailer"));
        let mut damaged = bytes.clone();
        damaged[ends[1] + 20] ^= 1;
        std::fs::write(&cut, &damaged).unwrap();
        let output = fixture.replay(false, &cut);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("corrupt"),
            "{output:?}"
        );
    }
}

#[test]
fn the_recording_level_and_the_launch_destination_must_agree() {
    let refused = |output: &Output, text: &str| {
        assert!(
            !output.status.success()
                && output.stdout.is_empty()
                && String::from_utf8_lossy(&output.stderr).contains(text),
            "{output:?}"
        );
    };
    for native in [false, true] {
        let durable = Fixture::new(&format!("durable-{native}"), Some("durable"));
        refused(&durable.live(native, None), "needs a recording destination");
        let none = Fixture::new(&format!("none-{native}"), Some("none"));
        let tape = none.0.join("tape");
        refused(&none.live(native, Some(&tape)), "selects recording none");
        assert_ran(&none.live(native, None), "none");
        // Level record, and an absent level with a destination, write the ordinary tape.
        for level in [Some("record"), None] {
            let fixture = Fixture::new(&format!("tape-{native}-{level:?}"), level);
            let tape = fixture.0.join("tape");
            assert_ran(&fixture.live(native, Some(&tape)), "tape");
            assert!(
                std::fs::read(&tape)
                    .unwrap()
                    .starts_with(hirsel_run_schema::tape::MAGIC)
            );
            assert_ran(&fixture.replay(native, &tape), "tape replay");
        }
    }
}
