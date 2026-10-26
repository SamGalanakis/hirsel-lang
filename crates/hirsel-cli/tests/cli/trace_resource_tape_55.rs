//! Recorded resource footprints: a traced native run whose stdio or a file shares
//! one host resource replays the same canonical events from its ordinary or
//! durable tape, byte for byte, and the reference engine replays the same tape.
use super::*;

const STDIO: &str = r#"pub type AppError = Grant(GrantError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let error = io.stderr(rt) ? Grant
  task.scope(tasks, fn(s: Scope) { () })
  out.print(output, "out\n") ? Output
  err.print(error, "err\n") ? Output
  Ok(())
}"#;

const SHARED: &str = r#"pub type AppError = Grant(GrantError) | Files(FileError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  task.scope(tasks, fn(s: Scope) { () })
  out.print(output, "shared\n") ? Output
  let mut f = file.open(d, path.of("out")) ? Files
  let mut bytes: List[u8] = []
  let read = file.read(d, mut f, mut bytes, 64u64) ? Files
  failure.check(read == 7u64 && bytes.from_list(take bytes) == b"shared\n", Arithmetic())
  Ok(())
}"#;

/// Where the live run's stdout and stderr go: both into one file, or stdout
/// into the granted file the program then opens.
#[derive(Clone, Copy)]
enum Streams {
    OneFile,
    GrantedFile,
}

#[test]
fn stdio_aliases_replay_their_recorded_resource_graph() {
    cells("stdio", STDIO, Streams::OneFile);
}

#[test]
fn a_shared_file_sink_replays_its_recorded_resource_graph() {
    cells("shared", SHARED, Streams::GrantedFile);
}

fn cells(name: &str, program: &str, streams: Streams) {
    let source = std_file(program);
    for backend in ["llvm", "cranelift"] {
        let executable = build_executable(&source, &["--backend", backend]);
        for (width, recording) in [(1, "record"), (2, "record"), (1, "durable")] {
            let cell = format!("{name} {backend} w{width} {recording}");
            let dir = tempfile::tempdir().unwrap();
            let data = dir.path().join("data");
            std::fs::create_dir(&data).unwrap();
            let sink = match streams {
                Streams::OneFile => dir.path().join("stdio"),
                Streams::GrantedFile => data.join("out"),
            };
            let (trace, tape) = (dir.path().join("events.jsonl"), dir.path().join("run.tape"));
            let grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"data","host":data,"access":"read_write"}]},"placement":{"cores":width,"commit":"shared","reorder":"auto"},"recording":recording}).to_string();
            let out = std::fs::File::create(&sink).unwrap();
            let mut live = clean(&executable);
            live.arg("--hirsel-trace-events")
                .arg(&trace)
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(&grants)
                .stdout(out.try_clone().unwrap());
            match streams {
                Streams::OneFile => live.stderr(out),
                Streams::GrantedFile => live.stderr(std::process::Stdio::piped()),
            };
            let output = live.output().unwrap();
            assert_eq!(output.status.code(), Some(0), "{cell}: {output:?}");
            let expected: &[u8] = match streams {
                Streams::OneFile => b"out\nerr\n",
                Streams::GrantedFile => b"shared\n",
            };
            assert_eq!(std::fs::read(&sink).unwrap(), expected, "{cell}");
            let evidence = std::fs::read(&trace).unwrap();
            let text = std::str::from_utf8(&evidence).unwrap();
            assert!(text.contains(r#""kind":"root_end""#), "{cell}: {text}");
            // Live keyed evidence: the declared graph, never the coupled fallback.
            assert!(
                !text.contains(r#""key":{"kind":"coupled"}"#),
                "{cell}: {text}"
            );
            // Both stdio sinks, or the sink and the opened file, name one resource.
            let created: Vec<_> = text
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .filter(|row| row["outcome"]["kind"] == "host_created")
                .map(|row| row["outcome"]["creation"].clone())
                .collect();
            let of = |kind: &str| -> Vec<_> {
                created
                    .iter()
                    .filter(|creation| creation["resource_kind"]["kind"] == kind)
                    .map(|creation| creation["resource"].clone())
                    .collect()
            };
            let (sinks, files) = (of("sink"), of("file"));
            match streams {
                Streams::OneFile => {
                    assert_eq!(sinks.len(), 2, "{cell}: {text}");
                    assert_eq!(sinks[0], sinks[1], "{cell}: {text}");
                }
                Streams::GrantedFile => {
                    // The directory grant, then the opened file.
                    assert_eq!((sinks.len(), files.len()), (1, 2), "{cell}: {text}");
                    assert_eq!(files[1], sinks[0], "{cell}: {text}");
                    assert_ne!(files[0], sinks[0], "{cell}: {text}");
                }
            }
            let original = std::fs::read(&tape).unwrap();
            let mut replay = clean(&executable);
            replay.env("HIRSEL_RT_WORKERS", width.to_string());
            let output = replay
                .arg("--hirsel-trace-events")
                .arg(&trace)
                .args([
                    "--hirsel-replay-policy",
                    r#"{"commit":"shared","reorder":"auto"}"#,
                ])
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "{cell} replay: {output:?}");
            assert_eq!(std::fs::read(&tape).unwrap(), original, "{cell}");
            assert_eq!(std::fs::read(&trace).unwrap(), evidence, "{cell}");
            let reference = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&source)
                .arg("--replay")
                .arg(&tape)
                .output()
                .unwrap();
            assert_eq!(
                reference.status.code(),
                Some(0),
                "{cell} reference: {reference:?}"
            );
        }
        std::fs::remove_file(executable).unwrap();
    }
    std::fs::remove_file(source.with_extension("hir.json")).unwrap();
    std::fs::remove_file(source).unwrap();
}

fn clean(executable: &std::path::Path) -> Command {
    let mut command = Command::new(executable);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("HIRSEL_RT_") {
            command.env_remove(key);
        }
    }
    command.env("HIRSEL_JOBS", "4");
    command
}
