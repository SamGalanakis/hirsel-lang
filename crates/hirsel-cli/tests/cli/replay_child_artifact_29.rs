//! Confirmed child artifacts replay without host files or child execution.
use hirsel_rt::io::{Completion, FileInput, Request};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
#[allow(dead_code)]
#[path = "../../../hirsel-rt/tests/support/tape.rs"]
mod tape;

fn write_recording(path: &Path, record: &hirsel_rt::launch::Recording) {
    let header = hirsel_rt::launch::Recording {
        events: vec![],
        ..record.clone()
    };
    let mut writer =
        hirsel_rt::launch::RecordingWriter::new(fs::File::create(path).unwrap(), &header).unwrap();
    for event in &record.events {
        writer.append(event).unwrap();
    }
    writer.finish().unwrap();
}

const SOURCE: &str = r#"pub type AppError = Grant(GrantError) | File(FileError) | Proc(ProcessError) | Output(OutputError) | Arg(ArgError)
pub fn main(rt: Io, args: Args) -> Result[(), AppError] {
  let output = io.stdout(rt)?Grant
  let d = io.dir(rt, path.of("data"))?File
  let p = io.process(rt)?Grant
  let source = arg.text(args, 0u64)?Arg
  let middle = arg.text(args, 1u64)?Arg
  let target = arg.text(args, 2u64)?Arg
  let command = Command(program: path.of("/bin/cp"), args: [copy source, copy middle], dir: None(), env: None(), own_group: false)
  let mut child = proc.spawn(p, take command)?Proc
  child.close_stdin(p, mut child)
  let status = child.wait(p, take child, None())?Proc
  failure.check(status == Exited(0), Arithmetic())
  let size = file.size(d, path.of("mid"))?File
  let raw = file.read_bytes(d, path.of("mid"))?File
  out.print(output, std@1/u64.to_string(size))?Output
  out.print(output, "\n")?Output
  out.print(output, std@1/sha256.digest_hex(raw))?Output
  out.print(output, "\n")?Output
  let command2 = Command(program: path.of("/bin/cp"), args: [copy middle, copy target], dir: None(), env: None(), own_group: false)
  let mut child2 = proc.spawn(p, take command2)?Proc
  child.close_stdin(p, mut child2)
  let status2 = child.wait(p, take child2, None())?Proc
  failure.check(status2 == Exited(0), Arithmetic())
  Ok(())
}
"#;

fn success(command: &mut Command) -> Vec<u8> {
    let out = command.env("HIRSEL_JOBS", "1").output().unwrap();
    assert!(out.status.success(), "{command:?}: {out:?}");
    assert!(out.stderr.is_empty(), "{command:?}: {out:?}");
    out.stdout
}
struct Fixture {
    root: tempfile::TempDir,
    manifest: PathBuf,
    grants: PathBuf,
    steps: u64,
}
impl Fixture {
    fn new(name: &str, sources: &[(&str, &str)], mut manifest: Value, steps: u64) -> Self {
        let mut builder = tempfile::Builder::new();
        builder.prefix(name);
        let root = if let Some(evidence) = std::env::var_os("CHILD_ARTIFACT_29_EVIDENCE") {
            fs::create_dir_all(&evidence).unwrap();
            builder.tempdir_in(evidence).unwrap()
        } else {
            builder.tempdir().unwrap()
        };
        for (name, source) in sources {
            fs::write(root.path().join(name), source).unwrap();
        }
        manifest["dependencies"][0]["manifest"] = json!(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../stdlib/std@1/hirsel.json")
                .canonicalize()
                .unwrap()
        );
        let selection = root.path().join("hirsel.json");
        fs::write(&selection, manifest.to_string()).unwrap();
        Self {
            grants: root.path().join("grants.json"),
            root,
            manifest: selection,
            steps,
        }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    fn build(&self) {
        for backend in ["llvm", "cranelift"] {
            success(
                Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .args(["build", "--backend", backend])
                    .arg(&self.manifest)
                    .arg("-o")
                    .arg(self.path(backend)),
            );
        }
    }
    fn record(&self, name: &str, backend: &str) -> (PathBuf, Vec<u8>) {
        let config: Value = serde_json::from_slice(&fs::read(&self.grants).unwrap()).unwrap();
        let args: Vec<_> = config["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let tape = self.path(&format!("{backend}-{name}.tape"));
        let output = success(
            Command::new(self.path(backend))
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(&self.grants)
                .args(args),
        );
        fs::write(tape.with_extension("stdout"), &output).unwrap();
        eprintln!("CHILD_ARTIFACT recorded {backend} {name}");
        (tape, output)
    }
    fn replay(&self, tape: &Path, expected: &[u8]) {
        let original = fs::read(tape).unwrap();
        for backend in ["llvm", "cranelift"] {
            assert_eq!(
                success(
                    Command::new(self.path(backend))
                        .arg("--hirsel-replay")
                        .arg(tape)
                ),
                expected
            );
            eprintln!("CHILD_ARTIFACT replay {backend} {}", tape.display());
        }
        let plain =
            hirsel_driver::replay_observations(&self.manifest, tape, Some(self.steps)).unwrap();
        let checked =
            hirsel_driver::replay_observations_checked(&self.manifest, tape, Some(self.steps))
                .unwrap();
        assert!(plain.equivalent(&checked));
        assert_eq!(plain.output, expected);
        assert_eq!(plain.exit_code, Some(0));
        eprintln!("CHILD_ARTIFACT replay reference+checked {}", tape.display());
        assert_eq!(fs::read(tape).unwrap(), original);
    }
    fn refused(&self, tape: &Path, kind: &str) {
        for backend in ["llvm", "cranelift"] {
            let output = Command::new(self.path(backend))
                .arg("--hirsel-replay")
                .arg(tape)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(3), "{backend}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(kind),
                "{backend}: {output:?}"
            );
        }
        for checked in [false, true] {
            let result = if checked {
                hirsel_driver::replay_observations_checked(&self.manifest, tape, Some(self.steps))
            } else {
                hirsel_driver::replay_observations(&self.manifest, tape, Some(self.steps))
            };
            assert!(result.unwrap_err().to_string().contains(kind));
        }
    }
    fn retain(self) {
        if std::env::var_os("CHILD_ARTIFACT_29_EVIDENCE").is_some() {
            let _ = self.root.keep();
        }
    }
}

#[test]
fn child_artifact_native_tapes_replay_in_all_four_engines() {
    hirsel_engine::with_compiler_stack(|| {
        let fixture = Fixture::new(
            "minimal-",
            &[("main.hir", SOURCE)],
            json!({"schema":1,"package":{"name":"child_artifact","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":"","package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}),
            1_000_000,
        );
        let data = fixture.path("data");
        fs::create_dir(&data).unwrap();
        let args: Vec<PathBuf> = ["in", "mid", "out"].map(|name| data.join(name)).into();
        fs::write(&fixture.grants, json!({"args":args,"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"data","host":data,"access":"read_write"}],"process":{"provider":"process.spawn@1","rules":[{"program":"/bin/cp"}]}},"placement":{"cores":1,"commit":"shared","reorder":"auto"}}).to_string()).unwrap();
        fixture.build();
        for backend in ["llvm", "cranelift"] {
            let _ = fs::remove_file(data.join("mid"));
            let _ = fs::remove_file(data.join("out"));
            for (name, payload) in [
                ("first", b"alpha".as_slice()),
                ("input", b"beta".as_slice()),
            ] {
                fs::write(data.join("in"), payload).unwrap();
                let (tape, output) = fixture.record(name, backend);
                assert_eq!(
                    output,
                    format!("{}\n{:x}\n", payload.len(), Sha256::digest(payload)).as_bytes()
                );
                assert_eq!(fs::read(data.join("mid")).unwrap(), payload);
                assert_eq!(fs::read(data.join("out")).unwrap(), payload);
                let record =
                    hirsel_rt::launch::parse_recording_bytes(&fs::read(&tape).unwrap()).unwrap();
                assert_eq!(
                    record
                        .events
                        .iter()
                        .filter(|e| matches!(e.request, Request::ProcessSpawn { .. }))
                        .count(),
                    2
                );
                assert!(
                    record
                        .events
                        .iter()
                        .any(|e| matches!(e.completion, Completion::FileInput(FileInput::Size(_))))
                );
                // Inspection preserves the additive tag, including an empty read at EOF.
                fs::write(tape.with_extension("events.txt"), format!("{record:#?}")).unwrap();
                assert_eq!(tape::recording(&record), record);
                fs::rename(&data, fixture.path("unavailable")).unwrap();
                fixture.replay(&tape, &output);
                let mut tampered = record.clone();
                let bytes = tampered
                    .events
                    .iter_mut()
                    .find_map(|e| match &mut e.completion {
                        Completion::FileInput(FileInput::Bytes(bytes)) if !bytes.is_empty() => {
                            Some(bytes)
                        }
                        _ => None,
                    })
                    .unwrap();
                bytes[0] ^= 1;
                let bad = fixture.path("tampered.tape");
                write_recording(&bad, &tampered);
                fixture.refused(&bad, "ReplayMismatch");
                // An effect cannot be relabelled as a host file input.
                let mut forged = record.clone();
                let event = forged
                    .events
                    .iter_mut()
                    .find(|e| matches!(e.request, Request::Print { .. }))
                    .unwrap();
                event.completion = Completion::FileInput(FileInput::Size(0));
                write_recording(&bad, &forged);
                fixture.refused(&bad, "InvalidCompletion");
                assert!(!data.exists());
                fs::rename(fixture.path("unavailable"), &data).unwrap();
            }
        }
        fixture.retain();
    });
}

#[test]
fn dagbuild_child_artifact_first_input_and_twins_replay_in_all_four_engines() {
    hirsel_engine::with_compiler_stack(|| {
        let sources = [
            (
                "main.hir",
                include_str!("../../../../examples/dagbuild/main.hir"),
            ),
            (
                "manifest.hir",
                include_str!("../../../../examples/dagbuild/manifest.hir"),
            ),
            (
                "graph.hir",
                include_str!("../../../../examples/dagbuild/graph.hir"),
            ),
            (
                "fingerprint.hir",
                include_str!("../../../../examples/dagbuild/fingerprint.hir"),
            ),
            (
                "execute.hir",
                include_str!("../../../../examples/dagbuild/execute.hir"),
            ),
            (
                "report.hir",
                include_str!("../../../../examples/dagbuild/report.hir"),
            ),
        ];
        // Full-app hashing/parsing exceeds the corpus's 1M reference policy.
        // This is a bounded Rust application witness, not a default-policy change.
        let fixture = Fixture::new(
            "dagbuild-",
            &sources,
            serde_json::from_str(include_str!("../../../../examples/dagbuild/hirsel.json"))
                .unwrap(),
            16_000_000,
        );
        fs::write(
            fixture.path("oracle.py"),
            include_str!("../support/dagbuild_29/oracle.py"),
        )
        .unwrap();
        fixture.build();
        for backend in ["llvm", "cranelift"] {
            let project = fixture.path("project");
            let state = fixture.path("state");
            fs::create_dir(&project).unwrap();
            fs::create_dir(&state).unwrap();
            fs::write(project.join("in"), "alpha").unwrap();
            let task = |name, source, target, dependencies: Vec<&str>| json!({"name":name,"argv":["/bin/cp",project.join(source),project.join(target)],"dependencies":dependencies,"inputs":[source],"outputs":[target]});
            let mut tasks = vec![
                task("a", "in", "mid", vec![]),
                task("b", "mid", "out", vec!["a"]),
            ];
            fs::write(&fixture.grants, json!({"args":["build","manifest.json"],"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[{"path":"project","host":project,"access":"read_write"},{"path":"state","host":state,"access":"read_write"}],"process":{"provider":"process.spawn@1","rules":[{"program":"/bin/cp"}]}},"placement":{"cores":1,"commit":"shared","reorder":"auto"}}).to_string()).unwrap();
            for (name, status, reason) in [
                ("first", "built", "no_record"),
                ("unchanged", "clean", "unchanged"),
                ("input", "built", "fingerprint_changed"),
                ("command", "built", "fingerprint_changed"),
            ] {
                if name == "input" {
                    fs::write(project.join("in"), "beta").unwrap();
                }
                if name == "command" {
                    tasks[0]["argv"]
                        .as_array_mut()
                        .unwrap()
                        .insert(1, json!("-f"));
                }
                fs::write(
                    project.join("manifest.json"),
                    json!({"tasks":tasks,"jobs":2,"fail_fast":false}).to_string(),
                )
                .unwrap();
                let (tape, output) = fixture.record(name, backend);
                let rows: Vec<Value> = String::from_utf8(output.clone())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(rows.len(), 3);
                for row in &rows[..2] {
                    assert_eq!(row["status"], status);
                    assert_eq!(row["reason"], reason);
                }
                let payload = fs::read(project.join("in")).unwrap();
                for file in ["mid", "out"] {
                    assert_eq!(fs::read(project.join(file)).unwrap(), payload);
                }
                fs::write(fixture.path("oracle-input.json"), json!({"tasks":tasks,"files":{"in":String::from_utf8(payload.clone()).unwrap(),"mid":String::from_utf8(payload.clone()).unwrap(),"out":String::from_utf8(payload).unwrap()}}).to_string()).unwrap();
                let oracle = success(Command::new("python3").current_dir(fixture.root.path()).args(["-c", "import json,oracle; c=json.load(open('oracle-input.json')); print(json.dumps(oracle.record(c['tasks'],c['files'])))"]));
                let expected: Value = serde_json::from_slice(&oracle).unwrap();
                let actual: Value =
                    serde_json::from_slice(&fs::read(state.join("state.json")).unwrap()).unwrap();
                assert_eq!(actual, expected);
                fs::write(tape.with_extension("state.json"), actual.to_string()).unwrap();
                fs::rename(&project, fixture.path("project-unavailable")).unwrap();
                fs::rename(&state, fixture.path("state-unavailable")).unwrap();
                fixture.replay(&tape, &output);
                assert!(!project.exists() && !state.exists());
                fs::rename(fixture.path("project-unavailable"), &project).unwrap();
                fs::rename(fixture.path("state-unavailable"), &state).unwrap();
            }
            fs::remove_dir_all(project).unwrap();
            fs::remove_dir_all(state).unwrap();
        }
        fixture.retain();
    });
}
