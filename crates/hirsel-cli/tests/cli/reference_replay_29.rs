//! Native tapes cross the host adapter unchanged; evaluator cuts are not divergences.
use hirsel_rt::io::Request;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    root: tempfile::TempDir,
    manifest: PathBuf,
    grants: PathBuf,
}
impl Fixture {
    fn new(source: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("main.hir"), source).unwrap();
        fs::create_dir(root.path().join("data")).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = root.path().join("hirsel.json");
        fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"reference_replay","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let grants = root.path().join("grants.json");
        Self {
            root,
            manifest,
            grants,
        }
    }
    fn configure(&self, args: &[String], payload: &[u8]) {
        fs::write(self.root.path().join("data/payload"), payload).unwrap();
        let children: Vec<_> = (0..3).map(|_| serde_json::json!({"program":"/fixture/child","args":args,"steps":[{"exit":{"exited":0}}]})).collect();
        fs::write(&self.grants, serde_json::json!({"args":args,"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"data","host":self.root.path().join("data"),"access":"read"}],"process":{"provider":"process.script@1","rules":[{"program":"/fixture/child","env":["KEY"]}],"runs":[],"children":children}},"placement":{"cores":1,"commit":"shared","reorder":"auto"}}).to_string()).unwrap();
    }
    fn build(&self) {
        for backend in ["llvm", "cranelift"] {
            success(
                Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .args(["build", "--backend", backend])
                    .arg(&self.manifest)
                    .arg("-o")
                    .arg(self.root.path().join(backend)),
                b"",
            );
        }
    }
    fn tape(&self, name: &str) -> PathBuf {
        self.root.path().join(format!("{name}.tape"))
    }
    fn record(&self, backend: &str, tape: &Path, expected: &[u8]) {
        let grants: serde_json::Value =
            serde_json::from_slice(&fs::read(&self.grants).unwrap()).unwrap();
        let args: Vec<_> = grants["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        success(
            Command::new(self.root.path().join(backend))
                .arg("--hirsel-record")
                .arg(tape)
                .arg("--hirsel-grants")
                .arg(&self.grants)
                .args(args),
            expected,
        );
    }
    fn replay(&self, tape: &Path, expected: &[u8]) {
        let ordinary =
            hirsel_driver::replay_observations(&self.manifest, tape, Some(1_000_000)).unwrap();
        let checked =
            hirsel_driver::replay_observations_checked(&self.manifest, tape, Some(1_000_000))
                .unwrap();
        assert!(ordinary.equivalent(&checked));
        assert_eq!(ordinary.output, expected);
        assert_eq!(ordinary.exit_code, Some(0));
        for backend in ["llvm", "cranelift"] {
            success(
                Command::new(self.root.path().join(backend))
                    .arg("--hirsel-replay")
                    .arg(tape),
                expected,
            );
        }
    }
    fn refused(&self, tape: &Path, kind: &str) {
        assert!(
            hirsel_driver::replay_observations(&self.manifest, tape, Some(1_000_000))
                .unwrap_err()
                .to_string()
                .contains(kind)
        );
        assert!(
            hirsel_driver::replay_observations_checked(&self.manifest, tape, Some(1_000_000))
                .unwrap_err()
                .to_string()
                .contains(kind)
        );
        for backend in ["llvm", "cranelift"] {
            let out = Command::new(self.root.path().join(backend))
                .arg("--hirsel-replay")
                .arg(tape)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(3), "{backend}: {out:?}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(kind),
                "{backend}: {out:?}"
            );
        }
    }
}
fn success(command: &mut Command, expected: &[u8]) -> Output {
    let out = command.env("HIRSEL_JOBS", "1").output().unwrap();
    assert!(out.status.success(), "{command:?}: {out:?}");
    assert_eq!(out.stdout, expected, "{command:?}");
    assert!(out.stderr.is_empty(), "{command:?}: {out:?}");
    out
}

#[test]
fn native_process_argv_and_archive_payloads_replay_exactly_in_four_engines() {
    hirsel_engine::with_compiler_stack(|| {
        let fixture = Fixture::new(include_str!("../support/reference_replay_29/main.hir"));
        fixture.build();
        // Include NUL/invalid UTF-8 in file bytes, and multibyte/empty argv entries.
        for (name, args, size) in [
            ("argv4096", vec!["x".repeat(4096)], 8192),
            ("argv4097", vec!["x".repeat(4097)], 16384),
            (
                "argv64",
                (0..64)
                    .map(|i| {
                        if i == 0 {
                            String::new()
                        } else {
                            format!("λ{i}")
                        }
                    })
                    .collect(),
                262144,
            ),
        ] {
            let payload: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            fixture.configure(&args, &payload);
            for backend in ["llvm", "cranelift"] {
                let tape = fixture.tape(&format!("{name}-{backend}"));
                fixture.record(backend, &tape, &payload);
                let original = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
                let commands: Vec<_> = record
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        Request::ProcessSpawn { command, .. } => Some(command),
                        _ => None,
                    })
                    .collect();
                assert_eq!(commands.len(), 3);
                assert!(commands.iter().all(|c| c.args == args
                    && c.directory.is_some()
                    && c.env == Some(vec![("KEY".into(), "value".into())])));
                fixture.replay(&tape, &payload);
                assert_eq!(fs::read(&tape).unwrap(), original);
                let mut tampered = record.clone();
                let command = tampered
                    .events
                    .iter_mut()
                    .find_map(|e| match &mut e.request {
                        Request::ProcessSpawn { command, .. } => Some(command),
                        _ => None,
                    })
                    .unwrap();
                command.args.push("tampered".into());
                let bad = fixture.tape("tampered");
                fs::write(
                    &bad,
                    hirsel_rt::launch::render_recording(&tampered).unwrap(),
                )
                .unwrap();
                fixture.refused(&bad, "ReplayMismatch");
                let mut extra = record.clone();
                extra.events.push(record.events[0].clone());
                fs::write(&bad, hirsel_rt::launch::render_recording(&extra).unwrap()).unwrap();
                fixture.refused(&bad, "UnconsumedInput");
            }
        }
    });
}

#[test]
fn native_tape_step_cuts_report_steps_before_finish_or_cleanup() {
    hirsel_engine::with_compiler_stack(|| {
        let fixture = Fixture::new(include_str!("../support/reference_replay_29/stopped.hir"));
        fixture.configure(&[], b"payload");
        fixture.build();
        for backend in ["llvm", "cranelift"] {
            let tape = fixture.tape(backend);
            fixture.record(backend, &tape, b"");
            let original = fs::read(&tape).unwrap();
            fixture.replay(&tape, b"");
            // 1 stops before any request; 100 stops with an acquired File whose
            // cleanup Close would otherwise mismatch the next recorded FileRead.
            for budget in [1, 100] {
                let out = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .arg("run")
                    .arg(&fixture.manifest)
                    .arg("--replay")
                    .arg(&tape)
                    .args(["--max-steps", &budget.to_string()])
                    .output()
                    .unwrap();
                assert_eq!(out.status.code(), Some(3), "{out:?}");
                assert_eq!(out.stderr, b"evaluator tooling limit: Steps\n", "{out:?}");
                assert!(out.stdout.is_empty());
                assert!(
                    hirsel_driver::replay_observations_checked(
                        &fixture.manifest,
                        &tape,
                        Some(budget)
                    )
                    .is_err()
                );
            }
            // A mismatch before a tooling guard is still a mismatch.
            let mut changed = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
            let path = changed
                .events
                .iter_mut()
                .find_map(|e| match &mut e.request {
                    Request::Directory { path, .. } => Some(path),
                    _ => None,
                })
                .unwrap();
            path.push_str("-changed");
            let bad = fixture.tape("changed-prefix");
            fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
            let out = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&fixture.manifest)
                .arg("--replay")
                .arg(&bad)
                .args(["--max-steps", "100"])
                .output()
                .unwrap();
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("ReplayMismatch"),
                "{out:?}"
            );
            assert_eq!(fs::read(&tape).unwrap(), original);
        }
    });
}

#[test]
fn language_failure_still_replays_its_resource_cleanup() {
    hirsel_engine::with_compiler_stack(|| {
        let source = include_str!("../support/reference_replay_29/stopped.hir").replace(
            "let mut count = 0u64",
            "failure.check(false, Arithmetic())\n  let mut count = 0u64",
        );
        let fixture = Fixture::new(&source);
        fixture.configure(&[], b"payload");
        fixture.build();
        for backend in ["llvm", "cranelift"] {
            let tape = fixture.tape(backend);
            let recorded = Command::new(fixture.root.path().join(backend))
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(&fixture.grants)
                .output()
                .unwrap();
            assert_eq!(recorded.status.code(), Some(2), "{recorded:?}");
            let original = fs::read(&tape).unwrap();
            let record = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
            assert!(
                record
                    .events
                    .iter()
                    .any(|e| matches!(e.request, Request::Close { .. }))
            );
            let ordinary =
                hirsel_driver::replay_observations(&fixture.manifest, &tape, Some(1_000_000))
                    .unwrap();
            let checked = hirsel_driver::replay_observations_checked(
                &fixture.manifest,
                &tape,
                Some(1_000_000),
            )
            .unwrap();
            assert!(ordinary.equivalent(&checked));
            assert!(!ordinary.failures.is_empty());
            for target in ["llvm", "cranelift"] {
                let out = Command::new(fixture.root.path().join(target))
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert_eq!(
                    out.status.code(),
                    recorded.status.code(),
                    "{target}: {out:?}"
                );
                assert_eq!(out.stdout, recorded.stdout);
                assert_eq!(out.stderr, recorded.stderr);
            }
            assert_eq!(fs::read(&tape).unwrap(), original);
        }
    });
}
