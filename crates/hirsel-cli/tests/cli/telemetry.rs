//! Telemetry capture end to end: every engine records spans, events, logs and an
//! abandonment, every engine replays every engine's recording with no host provider, a
//! changed payload refuses replay, and the null provider's exporter congestion changes
//! nothing a program observes.
use std::path::{Path, PathBuf};
use std::process::Command;

const PROGRAM: &str = r#"pub type E = Grant(GrantError) | Ctx(std@1/telemetry.ContextError) | Event(
  std@1/telemetry.EventError
)

fn spec(name: String, links: List[std@1/telemetry.TraceContext]) -> Result[
  std@1/telemetry.SpanSpec,
  E
] {
  Ok(std@1/telemetry.span_spec(name, Internal(), [], links, true) ? Event)
}

fn hex_id(ctx: std@1/telemetry.TraceContext) -> String {
  std@1/hex.encode(std@1/bytes.from_inline(ctx.trace_id))
}

fn worked(t: Telemetry, take parent: std@1/telemetry.TraceContext, n: u64) -> bool {
  match work(t, parent, n) {
    Ok(()) => true,
    Error(_) => false,
  }
}

fn work(t: Telemetry, parent: std@1/telemetry.TraceContext, n: u64) -> Result[(), E] {
  let span = std@1/telemetry.begin(t, Some(parent), spec("work", [parent])?) ? Ctx
  let field = std@1/telemetry.Attribute(name: "n", value: Unsigned(n))
  std@1/telemetry.span_event(t, span, std@1/telemetry.check_event("step", [field]) ? Event)
  if n == 2 {
    // Dropped unfinished: recorded as abandoned.
    return Ok(())
  }
  std@1/telemetry.finish(t, take span, Success()) ? Event
  Ok(())
}

pub fn main(rt: Io) -> Result[(), E] {
  let t = io.telemetry(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let out = io.stdout(rt) ? Grant
  let root = std@1/telemetry.begin(t, None(), spec("root", [])?) ? Ctx
  let ctx = std@1/telemetry.context(t, root)
  let results = task.scope(
    tasks,
    fn(s: Scope) -> u64 {
      let (c1, c2, c3) = (unique ctx, unique ctx, unique ctx)
      let a = task.async(s, fn() -> bool { worked(t, take c1, 1) })
      let b = task.async(s, fn() -> bool { worked(t, take c2, 2) })
      let c = task.async(s, fn() -> bool { worked(t, take c3, 3) })
      let mut ok: u64 = 0
      if task.await(s, take a) { ok += 1 }
      if task.await(s, take b) { ok += 1 }
      if task.await(s, take c) { ok += 1 }
      ok
    }
  )
  let entry = std@1/telemetry.LogRecord(
    level: Info(),
    name: "done",
    message: "three workers",
    metadata: std@1/telemetry.Metadata(trace: Some(unique ctx), fields: [])
  )
  std@1/telemetry.emit_log(t, std@1/telemetry.check_log(entry) ? Event)
  std@1/telemetry.finish(t, take root, Success()) ? Event
  let _ = out.print(out, f"{hex_id(ctx)} {results}\n")
  Ok(())
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hirsel-telemetry-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"traced","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn source(&self) -> PathBuf {
        self.0.join("main.hir")
    }
    /// A grants file with this Telemetry grant and an optional seeded scheduler.
    fn grants(&self, name: &str, telemetry: serde_json::Value, scheduler: bool) -> PathBuf {
        let mut record = serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[],"telemetry":telemetry}});
        if scheduler {
            record["inputs"] = serde_json::json!({"hash_seed":0,"random_seed":0,"test_seed":0,
                "scheduler":{"algorithm":"random","version":1,"seed":5}});
        }
        let path = self.0.join(name);
        std::fs::write(&path, record.to_string()).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const BACKENDS: [Option<&str>; 3] = [None, Some("llvm"), Some("cranelift")];

fn hirsel(backend: Option<&str>, source: &Path, args: &[&std::ffi::OsStr]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.arg("run").arg(source).args(args);
    if let Some(backend) = backend {
        command.args(["--native", "--backend", backend]);
    }
    command
}

fn record(fixture: &Fixture, backend: Option<&str>, grants: &Path, tape: &Path) -> Vec<u8> {
    let output = hirsel(
        backend,
        &fixture.source(),
        &[
            "--grants".as_ref(),
            grants.as_os_str(),
            "--record".as_ref(),
            tape.as_os_str(),
        ],
    )
    .output()
    .unwrap();
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{backend:?}: {output:?}"
    );
    output.stdout
}

/// The tape's lines after its `events` marker: what the run observed and decided.
fn events(tape: &Path) -> Vec<String> {
    hirsel_rt::launch::read_recording_text(tape)
        .unwrap()
        .lines()
        .skip_while(|line| *line != "events")
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_engine_replays_every_engines_capture_without_a_provider() {
    let fixture = Fixture::new("engines");
    let grants = fixture.grants(
        "host.json",
        serde_json::json!({"provider":"telemetry.capture@1"}),
        false,
    );
    for recorder in BACKENDS {
        let tape = fixture
            .0
            .join(format!("{}.tape", recorder.unwrap_or("interp")));
        let stdout = record(&fixture, recorder, &grants, &tape);
        assert!(stdout.ends_with(b" 3\n"), "{recorder:?}: {stdout:?}");
        let lines = events(&tape);
        for kind in [
            "telemetry_begin ",
            "telemetry_event ",
            "telemetry_finish ",
            "telemetry_log ",
        ] {
            assert!(
                lines.iter().any(|line| line.starts_with(kind)),
                "{recorder:?} records {kind}"
            );
        }
        // The worker that returned early dropped its span: one abandonment, no outcome.
        let begins = lines
            .iter()
            .filter(|l| l.starts_with("telemetry_begin "))
            .count();
        let finishes = lines
            .iter()
            .filter(|l| l.starts_with("telemetry_finish "))
            .count();
        assert_eq!((begins, finishes), (4, 3), "{recorder:?}");
        for player in BACKENDS {
            let output = hirsel(
                player,
                &fixture.source(),
                &["--replay".as_ref(), tape.as_os_str()],
            )
            .output()
            .unwrap();
            assert!(
                output.status.success() && output.stdout == stdout,
                "{recorder:?} replayed by {player:?}: {output:?}"
            );
        }
    }
}

#[test]
fn a_changed_observation_refuses_replay() {
    let fixture = Fixture::new("changed");
    let grants = fixture.grants(
        "null.json",
        serde_json::json!({"provider":"telemetry.null@1","seed":9}),
        false,
    );
    for backend in [None, Some("llvm")] {
        let tape = fixture.0.join("run.tape");
        record(&fixture, backend, &grants, &tape);
        let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
        // The event's attribute value is the payload's last field: change n from 1 to 4.
        let line = text
            .lines()
            .find(|line| line.starts_with("telemetry_event "))
            .unwrap();
        let (request, completion) = line.split_once(" -> ").unwrap();
        let at = request.rfind("0100000000000000").unwrap();
        let changed = format!(
            "{}0400000000000000{} -> {completion}",
            &request[..at],
            &request[at + 16..]
        );
        assert_ne!(changed, line, "the payload carries the attribute");
        std::fs::write(&tape, text.replacen(line, &changed, 1)).unwrap();
        for player in [None, Some("llvm")] {
            let output = hirsel(
                player,
                &fixture.source(),
                &["--replay".as_ref(), tape.as_os_str()],
            )
            .output()
            .unwrap();
            assert!(
                !output.status.success(),
                "{backend:?} replayed by {player:?}: {output:?}"
            );
        }
    }
}

#[test]
fn exporter_congestion_changes_no_observation_or_schedule() {
    let fixture = Fixture::new("congestion");
    for backend in BACKENDS {
        let mut seen: Option<(Vec<u8>, Vec<String>)> = None;
        for (name, grant) in [
            (
                "open",
                serde_json::json!({"provider":"telemetry.null@1","seed":3}),
            ),
            (
                "full",
                serde_json::json!({"provider":"telemetry.null@1","seed":3,"queue":0}),
            ),
            (
                "narrow",
                serde_json::json!({"provider":"telemetry.null@1","seed":3,"queue":2}),
            ),
        ] {
            let grants = fixture.grants(&format!("{name}.json"), grant, true);
            let tape = fixture.0.join(format!("{name}.tape"));
            let stdout = record(&fixture, backend, &grants, &tape);
            let lines = events(&tape);
            assert!(
                lines.iter().any(|line| line.starts_with("schedule ")),
                "{backend:?} {name}: the seeded scheduler records its choices"
            );
            match &seen {
                None => seen = Some((stdout, lines)),
                Some((first_stdout, first_lines)) => {
                    assert_eq!(&stdout, first_stdout, "{backend:?} {name}");
                    assert_eq!(&lines, first_lines, "{backend:?} {name}");
                }
            }
        }
    }
}

#[test]
fn diagnostic_timing_is_default_isolated_and_used_by_the_viewer() {
    let fixture = Fixture::new("timing");
    let grants = fixture.grants(
        "timed.json",
        serde_json::json!({"provider":"telemetry.null@1", "seed":3}),
        true,
    );
    for backend in BACKENDS {
        let mut seen = None;
        for enabled in [false, true] {
            let tape = fixture
                .0
                .join(format!("{}-{enabled}.tape", backend.unwrap_or("interp")));
            let mut command = hirsel(
                backend,
                &fixture.source(),
                &[
                    "--grants".as_ref(),
                    grants.as_os_str(),
                    "--record".as_ref(),
                    tape.as_os_str(),
                ],
            );
            command.env("HIRSEL_TIMING_CAPTURE_ID", "timing-test");
            if enabled {
                command.env_remove("HIRSEL_TIMING");
            } else {
                command.env("HIRSEL_TIMING", "off");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success() && output.stderr.is_empty(),
                "{backend:?}: {output:?}"
            );
            let observations = (output.stdout.clone(), events(&tape));
            if let Some(before) = &seen {
                assert_eq!(before, &observations);
            } else {
                seen = Some(observations);
            }
            let sidecar = PathBuf::from(format!("{}.timing.json", tape.display()));
            assert_eq!(sidecar.exists(), enabled, "{backend:?}: {tape:?}");
            let view = fixture.0.join("view.json");
            let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("view")
                .arg(&tape)
                .args(["--capture", "timing-test", "-o"])
                .arg(&view)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            let document: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&view).unwrap()).unwrap();
            let rows = document["events"].as_array().unwrap();
            let mut previous = 0;
            for row in rows {
                if enabled {
                    let ns = row["original_time_ns"].as_u64().unwrap();
                    assert!(ns >= previous);
                    previous = ns;
                } else {
                    assert!(row["original_time_ns"].is_null());
                }
            }
            let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("view")
                .arg(&tape)
                .args(["--capture", "timing-test", "--perfetto", "-o"])
                .arg(&view)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            let trace: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&view).unwrap()).unwrap();
            assert_eq!(
                trace["hirsel"]["time_basis"],
                if enabled {
                    "host-monotonic-ns"
                } else {
                    "logical-order"
                }
            );
            assert!(
                trace["traceEvents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|e| e.get("dur").is_none())
            );
            // Replaying never reads or rewrites original timing, even when enabled.
            let original = std::fs::read(&sidecar).ok();
            let replay = hirsel(
                backend,
                &fixture.source(),
                &["--replay".as_ref(), tape.as_os_str()],
            )
            .env_remove("HIRSEL_TIMING")
            .output()
            .unwrap();
            assert!(
                replay.status.success() && replay.stdout == output.stdout,
                "{replay:?}"
            );
            assert_eq!(std::fs::read(&sidecar).ok(), original);
        }
    }
}

#[test]
fn diagnostic_sidecar_cannot_alias_the_tape_or_a_program_mount() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("timing-overlap");
    let grants = fixture.grants(
        "timed.json",
        serde_json::json!({"provider":"telemetry.null@1", "seed":3}),
        false,
    );
    let program_directory = fixture.0.join("program");
    std::fs::create_dir(&program_directory).unwrap();
    let protected = program_directory.join("protected");
    std::fs::write(&protected, "keep me").unwrap();
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&grants).unwrap()).unwrap();
    config["io"]["dirs"] =
        serde_json::json!([{"path":"data", "host":program_directory, "access":"read_write"}]);
    std::fs::write(&grants, config.to_string()).unwrap();
    for alias in [false, true] {
        let tape = fixture.0.join(format!("{alias}.tape"));
        let sidecar = PathBuf::from(format!("{}.timing.json", tape.display()));
        std::fs::write(&tape, "old tape").unwrap();
        if alias {
            std::fs::hard_link(&tape, &sidecar).unwrap();
        } else {
            symlink(&protected, &sidecar).unwrap();
        }
        let output = hirsel(
            None,
            &fixture.source(),
            &[
                "--grants".as_ref(),
                grants.as_os_str(),
                "--record".as_ref(),
                tape.as_os_str(),
            ],
        )
        .env_remove("HIRSEL_TIMING")
        .output()
        .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(std::fs::read_to_string(&protected).unwrap(), "keep me");
        assert!(
            hirsel_rt::launch::read_recording_text(&tape)
                .unwrap()
                .contains("complete ")
        );
        if alias {
            assert_eq!(
                std::fs::read(&tape).unwrap(),
                std::fs::read(&sidecar).unwrap()
            );
        }
    }
}
