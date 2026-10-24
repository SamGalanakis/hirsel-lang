//! JSON transport of the MIT run-schema vectors and public typed fixture recipes.
//! Fresh adapter identities; historical app campaigns are not reclassified here.
#[path = "../../src/simulation_world.rs"]
mod world;
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_interp::{library::simulation::ObservedGrant, observation::Effect};
use hirsel_run_schema::{
    Access, FileFault, FileFaultError, FileOperation, FileReadScript, SignalKind, SignalsGrant,
};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

// Seal the actual compiler, image, runtime and link kit before any test reader.
// The recursive digest includes each relative path and byte hash, so it seals
// both content and directory membership without emitting private configuration.
fn seal_tools() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        fn visit(root: &Path, path: &Path, tree: &mut Sha256, count: &mut u64) {
            if path.is_dir() {
                let mut paths: Vec<_> = fs::read_dir(path)
                    .unwrap()
                    .map(|p| p.unwrap().path())
                    .collect();
                paths.sort();
                for path in paths {
                    visit(root, &path, tree, count);
                }
            } else if path.is_file() {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes();
                tree.update((name.len() as u64).to_le_bytes());
                tree.update(name);
                let mut hash = Sha256::new();
                let mut f = fs::File::open(path).unwrap();
                let mut bytes = [0; 65536];
                loop {
                    let n = f.read(&mut bytes).unwrap();
                    if n == 0 {
                        break;
                    }
                    hash.update(&bytes[..n]);
                }
                tree.update(hash.finalize());
                *count += 1;
            }
        }
        let mut roots = vec![
            ("harness".to_owned(), std::env::current_exe().unwrap()),
            ("cli".into(), env!("CARGO_BIN_EXE_hirsel").into()),
        ];
        for name in [
            "HIRSEL_IMAGE",
            "HIRSEL_RUNTIME_ARCHIVES",
            "LLVM_SYS_231_PREFIX",
            "HIRSEL_LINK_KIT",
            "HIRSEL_GLIBC_FLOOR",
        ] {
            if let Some(path) = std::env::var_os(name) {
                roots.push((name.into(), path.into()));
            }
        }
        for (name, root) in roots {
            let mut hash = Sha256::new();
            let mut count = 0;
            visit(&root, &root, &mut hash, &mut count);
            assert!(
                count > 0,
                "missing actual tool input {name}: {}",
                root.display()
            );
            eprintln!(
                "SIMULATION_WORLD_CLI_39_SEAL {name} files={count} sha256={:x} path={}",
                hash.finalize(),
                root.display()
            );
        }
        for name in [
            "HIRSEL_JOBS",
            "HIRSEL_MEMORY_LIMIT",
            "HIRSEL_DAEMON",
            "HIRSEL_RESULTS",
            "HIRSEL_TEST_CPUS",
        ] {
            eprintln!(
                "SIMULATION_WORLD_CLI_39_CONFIG {name}={:?}",
                std::env::var(name).ok()
            );
        }
    });
}
fn fixture_text(id: &str) -> &'static str {
    match id {
        "pending-interrupt" => {
            include_str!("../support/simulation_world_cli_39/pending-interrupt.json")
        }
        "no-signal" => include_str!("../support/simulation_world_cli_39/no-signal.json"),
        "after-write" => include_str!("../support/simulation_world_cli_39/after-write.json"),
        "failed-write" => include_str!("../support/simulation_world_cli_39/failed-write.json"),
        "put-new" => include_str!("../support/simulation_world_cli_39/put-new.json"),
        "commit-denied" => include_str!("../support/simulation_world_cli_39/commit-denied.json"),
        "binary-readonly" => {
            include_str!("../support/simulation_world_cli_39/binary-readonly.json")
        }
        _ => panic!("unknown fixture {id}"),
    }
}
fn source_text(name: &str) -> &'static str {
    match name {
        "cancel" => include_str!("../support/simulation_world_cli_39/cancel.hir"),
        "read" => include_str!("../support/simulation_world_cli_39/read.hir"),
        "commit" => include_str!("../support/simulation_world_cli_39/commit.hir"),
        "binary" => include_str!("../support/simulation_world_cli_39/binary.hir"),
        _ => panic!("unknown source {name}"),
    }
}
fn fixture(id: &str) -> Value {
    serde_json::from_str(fixture_text(id)).unwrap()
}
fn typed(id: &str) -> SimulationConfig {
    let mut config = SimulationConfig {
        projections: ["stdout", "stderr", "tasks", "dir", "signals", "telemetry"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        mounts: [
            ("input".into(), Access::Read),
            ("output".into(), Access::ReadWrite),
        ]
        .into(),
        files: [("input/data".into(), vec![0, 255])].into(),
        directories: Default::default(),
        ..Default::default()
    };
    if matches!(id, "pending-interrupt" | "no-signal") {
        config
            .file_reads
            .insert("input/data".into(), FileReadScript::Pending);
        config.signals = Some(SignalsGrant {
            provider: "signals.script@1".into(),
            kinds: vec![SignalKind::Interrupt],
            script: Some(if id == "no-signal" {
                vec![]
            } else {
                vec![SignalKind::Interrupt]
            }),
        });
    }
    if matches!(id, "after-write" | "failed-write" | "put-new") {
        config.file_reads.insert(
            "input/data".into(),
            FileReadScript::AfterWrite {
                path: "output/release".into(),
            },
        );
    }
    if id == "failed-write" {
        config.args = vec!["failed".into()];
    }
    if id == "put-new" {
        config.args = vec!["put_new".into()];
    }
    if matches!(id, "failed-write" | "binary-readonly" | "commit-denied") {
        config.file_faults.insert(
            if id == "commit-denied" {
                FileOperation::Commit
            } else {
                FileOperation::WriteBytes
            },
            [FileFault {
                requested: (id != "commit-denied").then_some(1),
                error: Some(FileFaultError::PermissionDenied),
                partial: 0,
            }]
            .into(),
        );
    }
    if id == "commit-denied" {
        config.files = [("output/old".into(), b"old".to_vec())].into();
    }
    config
}

#[test]
fn simulation_world_cli_39_worlds_match_typed_and_real_cli() {
    seal_tools();
    hirsel_engine::with_compiler_stack(|| {
        let mut selected = 0;
        for (id, source, stdout, stderr, blocked) in [
            (
                "pending-interrupt",
                "cancel",
                "canceled; joined\n",
                "pending adapter\n",
                false,
            ),
            ("no-signal", "cancel", "", "", true),
            ("after-write", "read", "read checked\n", "", false),
            ("failed-write", "read", "read checked\n", "", false),
            ("put-new", "read", "", "", true),
            (
                "commit-denied",
                "commit",
                "commit denied; old preserved\n",
                "",
                false,
            ),
            (
                "binary-readonly",
                "binary",
                "binary; read only\n",
                "",
                false,
            ),
        ] {
            if std::env::var("SIMULATION_WORLD_CLI_39_CASES")
                .is_ok_and(|selection| !selection.split(',').any(|selected| selected == id))
            {
                continue;
            }
            selected += 1;
            let json = fixture(id);
            let decoded = world::decode(json).unwrap();
            let control = typed(id);
            assert_eq!(
                decoded, control,
                "{id}: entire typed world including defaults"
            );
            let scratch = tempfile::tempdir().unwrap();
            let path = scratch.path().join(format!("{source}.hir"));
            fs::write(&path, source_text(source)).unwrap();
            let world_path = scratch.path().join(format!("{id}.json"));
            fs::write(&world_path, fixture_text(id)).unwrap();
            let mut loaded =
                hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{id}: {e}"));
            loaded
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{id}: {e}"));
            let run = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let mut previous = None;
            for (adapter, config) in [("json", decoded), ("typed", control)] {
                for checked in [false, true] {
                    let engine = if checked { "checked-h" } else { "reference" };
                    eprintln!("SIMULATION_WORLD_CLI_39_STARTED {id}/{adapter}/{engine}");
                    let execution = if checked {
                        hirsel_engine::run_observed_checked(
                            &mut loaded.engine,
                            &run,
                            config.clone(),
                        )
                    } else {
                        hirsel_engine::run_observed_snapshot(
                            &mut loaded.engine,
                            &run,
                            config.clone(),
                        )
                    }
                    .unwrap();
                    let observed = execution
                        .observation
                        .unwrap_or_else(|| panic!("{id}: {:?}", execution.outcome));
                    assert_eq!(observed.output, stdout.as_bytes(), "{id}");
                    assert_eq!(
                        observed.blocked.is_some(),
                        blocked,
                        "{id}: {:?}",
                        execution.outcome
                    );
                    assert!(
                        observed.failures.is_empty(),
                        "{id}: {:?}",
                        observed.failures
                    );
                    let state = observed.final_state.as_ref().unwrap();
                    assert_eq!(state.stderr, stderr.as_bytes(), "{id}");
                    let mut files = config.files.clone();
                    match id {
                        "pending-interrupt" => {
                            files.insert("output/joined".into(), b"yes".to_vec());
                        }
                        "after-write" | "put-new" => {
                            files.insert("output/release".into(), b"x".to_vec());
                        }
                        "commit-denied" => {
                            files.insert("output/published".into(), b"new".to_vec());
                        }
                        "binary-readonly" => {
                            files.insert("output/written".into(), b"x".to_vec());
                        }
                        _ => (),
                    }
                    assert_eq!(
                        state.trees[&0].1, files,
                        "{id}: exact bytes, no leaked temporary"
                    );
                    if !blocked {
                        assert_eq!(observed.exit_code, Some(0), "{id}");
                        assert!(
                            !state.handles.values().any(|h| matches!(
                                h,
                                ObservedGrant::File { .. } | ObservedGrant::AtomicFile { .. }
                            )),
                            "{id}: file handle cleanup"
                        );
                    }
                    if matches!(
                        id,
                        "pending-interrupt"
                            | "no-signal"
                            | "after-write"
                            | "failed-write"
                            | "put-new"
                    ) {
                        assert!(observed.effects.iter().any(|e| matches!(&e.effect, Effect::CapabilityConsumed(e) if e.operation == "file.read.pending")), "{id}: must actually park");
                    }
                    if let Some(previous) = &previous {
                        assert_eq!(
                            &observed, previous,
                            "{id}/{adapter}/{engine}: complete observation"
                        );
                    }
                    previous = Some(observed);
                    eprintln!("SIMULATION_WORLD_CLI_39_PASS {id}/{adapter}/{engine}");
                }
            }
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["run", "--simulation"])
                .arg(&world_path)
                .arg(&path)
                .output()
                .unwrap();
            assert_eq!(output.stdout, stdout.as_bytes(), "{id}: {output:?}");
            if blocked {
                assert!(!output.status.success());
                assert!(
                    String::from_utf8_lossy(&output.stderr).starts_with("Blocked:"),
                    "{id}: {output:?}"
                );
            } else {
                assert_eq!(output.status.code(), Some(0), "{id}: {output:?}");
                assert_eq!(output.stderr, stderr.as_bytes(), "{id}");
            }
            eprintln!("SIMULATION_WORLD_CLI_39_PASS {id}/cli");
        }
        assert!(selected > 0, "zero-world selection");
    });
}

// Port the run-schema negative vectors, retaining exact tags, counts and nulls.
#[test]
fn simulation_world_cli_39_malformed_worlds_refuse_before_entry() {
    seal_tools();
    let scratch = tempfile::tempdir().unwrap();
    let program = scratch.path().join("main.hir");
    fs::write(
        &program,
        "pub fn main() -> Result[(), ()] { failure.check(false, Arithmetic()); Ok(()) }",
    )
    .unwrap();
    let fault = |op: &str, requested: Value, error: Value, partial: Value| json!({"file_faults": {op: [{"requested":requested,"error":error,"partial":partial}]}});
    let mut bad = vec![
        json!(null),
        json!([]),
        json!({"unknown":0}),
        json!({"random_seed":42}),
        json!({"signals":null}),
        json!({"signals":{}}),
        json!({"signals":{"provider":"signals.script@1","kinds":["interrupt"]}}),
        json!({"signals":{"provider":"signals.script@1","kinds":["interrupt"],"script":null}}),
        json!({"signals":{"provider":"signals.script@1","kinds":[],"script":[]}}),
        json!({"signals":{"provider":"signals.script@1","kinds":["interrupt","interrupt"],"script":[]}}),
        json!({"signals":{"provider":"signals.script@1","kinds":["Interrupt"],"script":[]}}),
        json!({"signals":{"provider":"signals.script@1","kinds":["interrupt"],"script":["kill"]}}),
        json!({"signals":{"provider":"signals.other@1","kinds":["interrupt"]}}),
        json!({"signals":{"provider":"signals.host@1","kinds":["interrupt"],"script":[]}}),
        json!({"signals":{"provider":"signals.host@1","kinds":["interrupt"],"script":null}}),
        json!({"signals":{"provider":"signals.script@1","kinds":["interrupt"],"script":[],"extra":true}}),
        json!({"mounts":null}),
        json!({"mounts":{"input":"Read"}}),
        json!({"mounts":{"input":null}}),
        json!({"file_reads":null}),
        json!({"file_reads":{"input":null}}),
        json!({"file_reads":{"input":"pending"}}),
        json!({"file_reads":{"input":{"AfterWrite":{"path":"release","extra":true}}}}),
        json!({"file_reads":{"input":{"AfterWrite":{"path":null}}}}),
        json!({"file_reads":{"input":{"AfterWrite":{}}}}),
        json!({"file_faults":null}),
        json!({"file_faults":{"file.read":[]}}),
        json!({"file_faults":{"read":[]}}),
        json!({"file_faults":{"file.commit":[{"requested":null,"error":"PermissionDenied","partial":0,"extra":1}]}}),
        fault("file.read", json!(null), json!(null), json!(0)),
        fault("file.commit", json!(1), json!(null), json!(0)),
        fault(
            "file.write_bytes",
            json!(1),
            json!({"Canceled":4}),
            json!(0),
        ),
        fault(
            "file.write_bytes",
            json!(1),
            json!({"Canceled":0}),
            json!(1),
        ),
        fault("file.read", json!(1), json!("PermissionDenied"), json!(1)),
        fault("file.write_bytes", json!(1), json!(null), json!(1)),
        fault(
            "file.write_bytes",
            json!(1),
            json!("PermissionDenied"),
            json!(2),
        ),
        fault("file.write_bytes", json!(-1), json!(null), json!(0)),
        fault("file.write_bytes", json!(1.5), json!(null), json!(0)),
        fault(
            "file.write_bytes",
            json!(1),
            json!({"Fault":4294967296u64}),
            json!(0),
        ),
        fault(
            "file.write_bytes",
            json!(1),
            json!("permission_denied"),
            json!(0),
        ),
        json!({"files":{"input":null}}),
        json!({"files":{"input":[0,256]}}),
        json!({"files":{"input":[-1]}}),
        json!({"files":{"input":[1.5]}}),
        json!({"files":{"input":[true]}}),
        json!({"files":[]}),
        json!({"stdin":[256]}),
        json!({"stdin":null}),
        json!({"wall":9223372036854775808u64}),
        json!({"monotonic":-1}),
        json!({"projections":["unknown"]}),
    ];
    for name in ["monotonic_clock", "wall_clock", "clock"] {
        let value = json!({"projections":[name]});
        let error = world::decode(value.clone()).unwrap_err();
        assert!(
            error.contains("MonotonicClock") && error.contains("monotonic/wall"),
            "{error}"
        );
        bad.push(value);
    }
    for (i, value) in bad.iter().enumerate() {
        let error = world::decode(value.clone()).unwrap_err();
        let path = scratch.path().join(format!("bad-{i}.json"));
        fs::write(&path, value.to_string()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--simulation"])
            .arg(&path)
            .arg(&program)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(3), "{i}: {value}: {output:?}");
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(&error),
            "{i}: {output:?}; {error}"
        );
        eprintln!("SIMULATION_WORLD_CLI_39_PASS negative-{i}/cli");
    }
    // JSON syntax and a number outside even u64 must fail at the real read boundary.
    for (i, raw) in ["{", r#"{"file_faults":{"file.read":[{"requested":18446744073709551616,"error":null,"partial":0}]}}"#].iter().enumerate() {
        let path = scratch.path().join(format!("syntax-{i}.json")); fs::write(&path, raw).unwrap();
        assert!(world::read(&path).is_err());
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel")).args(["run", "--simulation"]).arg(path).arg(&program).output().unwrap();
        assert_eq!(output.status.code(), Some(3)); assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("hirsel: simulation:"));
        eprintln!("SIMULATION_WORLD_CLI_39_PASS syntax-{i}/cli");
    }
}

#[test]
fn simulation_world_cli_39_defaults_inventory_and_authority() {
    seal_tools();
    assert_eq!(
        world::decode(json!({})).unwrap(),
        SimulationConfig::default()
    );
    let empty =
        world::decode(json!({"mounts":{},"file_reads":{},"file_faults":{},"projections":[]}))
            .unwrap();
    assert!(empty.mounts.is_empty() && empty.projections.is_empty());
    assert!(empty.signals.is_none());
    let mut supported = SimulationConfig::default().projections;
    supported.insert("signals".into());
    assert_eq!(
        supported,
        world::PROJECTIONS.iter().map(|p| p.to_string()).collect()
    );
    let provider = concat!(
        include_str!("../../../hirsel-interp/src/library/dispatch.rs"),
        include_str!("../../../hirsel-interp/src/library/telemetry.rs"),
        include_str!("../../../hirsel-interp/src/library/process.rs")
    );
    for projection in world::PROJECTIONS {
        assert!(
            provider.contains(&format!("\"io.{projection}\"")),
            "provider inventory: {projection}"
        );
        let decoded =
            world::decode(json!({"projections":[projection],"mounts":{},"grants":[]})).unwrap();
        assert!(
            decoded.grants.is_empty() && decoded.mounts.is_empty() && decoded.signals.is_none()
        );
    }
    let decoded = world::decode(
        json!({"files":{"binary":[0,255],"text":"羊"},"stdin":[0,255],"env":{"unset":null}}),
    )
    .unwrap();
    assert_eq!(decoded.files["binary"], [0, 255]);
    assert_eq!(decoded.stdin, [0, 255]);
    assert_eq!(decoded.files["text"], "羊".as_bytes());
    assert_eq!(decoded.env["unset"], None);
    // Match the existing standalone signal validator, including canonical kind sorting
    // and a script kind outside the observed set (which can take default action).
    let decoded = world::decode(json!({"signals":{"provider":"signals.script@1","kinds":["terminate","interrupt"],"script":["hangup"]}})).unwrap();
    assert_eq!(
        decoded.signals.unwrap().kinds,
        [SignalKind::Interrupt, SignalKind::Terminate]
    );
    let decoded =
        world::decode(json!({"signals":{"provider":"signals.host@1","kinds":["interrupt"]}}))
            .unwrap();
    assert_eq!(decoded.signals.unwrap().script, None);
    let bytes = world::decode(json!({"file_faults":{"file.write_bytes":[{"requested":2,"error":{"Fault":77},"partial":1}]}})).unwrap();
    assert_eq!(bytes.file_faults[&FileOperation::WriteBytes][0].partial, 1);
}

#[test]
fn simulation_world_cli_39_native_importer_build_and_incompatibility() {
    seal_tools();
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("commit.hir");
    fs::write(&source, source_text("commit")).unwrap();
    let world_path = scratch.path().join("commit-denied.json");
    fs::write(&world_path, fixture_text("commit-denied")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["build", "--debug", "--backend", "cranelift"])
        .arg(&source)
        .arg("-o")
        .arg(scratch.path().join("app"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "actual CLI/importer native build: {output:?}"
    );
    assert!(scratch.path().join("app").is_file());
    eprintln!("SIMULATION_WORLD_CLI_39_PASS native-importer-build/cranelift");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native", "--simulation"])
        .arg(world_path)
        .arg(source)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--simulation"),
        "{output:?}"
    );
    eprintln!("SIMULATION_WORLD_CLI_39_PASS native-simulation-incompatible/cli");
}

#[test]
fn simulation_world_cli_39_launch_admission_clocks_and_request_counts() {
    seal_tools();
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("main.hir");
    fs::write(&source, r#"pub fn main(rt: Io) -> Result[(), GrantError] {
      failure.check(match io.signals(rt) { Error(NotGranted()) => true, _ => false }, Arithmetic())
      failure.check(match io.dir(rt, path.of("input")) { Error(NotGranted()) => true, _ => false }, Arithmetic())
      failure.check(match io.monotonic_clock(rt) { Error(NotGranted()) => true, _ => false }, Arithmetic())
      failure.check(match io.wall_clock(rt) { Error(NotGranted()) => true, _ => false }, Arithmetic())
      let _ = io.telemetry(rt)?
      let _ = out.print(io.stdout(rt)?, "authority checked\n")
      Ok(())
    }"#).unwrap();
    let path = scratch.path().join("world.json");
    fs::write(&path, json!({"projections":["signals","telemetry","dir","stdout"],"mounts":{},"files":{"input/data":[0,255]},"monotonic":7,"wall":-9}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&path)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"authority checked\n");
    assert!(output.stderr.is_empty());
    eprintln!("SIMULATION_WORLD_CLI_39_PASS projection-no-authority/cli");
    fs::write(
        &source,
        r#"pub fn main(rt: Io) -> Result[(), GrantError] {
      let _ = io.monotonic_clock(rt)?
      let _ = io.wall_clock(rt)?
      let _ = out.print(io.stdout(rt)?, "clock grants checked\n")
      Ok(())
    }"#,
    )
    .unwrap();
    fs::write(&path, json!({"projections":["stdout"],"grants":["Io","MonotonicClock","WallClock"],"monotonic":7,"wall":-9}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&path)
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"clock grants checked\n");
    assert!(output.stderr.is_empty());
    eprintln!("SIMULATION_WORLD_CLI_39_PASS explicit-clock-grants/cli");
    fs::write(
        &path,
        json!({"grants":[],"projections":["stdout","signals","telemetry"]}).to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--simulation"])
        .arg(&path)
        .arg(&source)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    eprintln!("SIMULATION_WORLD_CLI_39_PASS projection-no-root/cli");
    // These shapes decode, then the existing launch/provider admission refuses them.
    for (id, program, config, diagnostic) in [
        (
            "escape",
            "pub fn main() -> Result[(), ()] { failure.check(false, Arithmetic()); Ok(()) }",
            json!({"files":{"../outside":"x"}}),
            "simulation file escapes root",
        ),
        (
            "count",
            "pub fn main(rt: Io) -> Result[(), FileError] { let d = io.dir(rt, path.of(\"\"))?; let _ = file.write_bytes(d, path.of(\"a\"), b\"x\")?; Ok(()) }",
            json!({"file_faults":{"file.write_bytes":[{"requested":2,"error":"PermissionDenied","partial":0}]}}),
            "file fault request count mismatch",
        ),
    ] {
        fs::write(&source, program).unwrap();
        fs::write(&path, config.to_string()).unwrap();
        world::decode(config).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--simulation"])
            .arg(&path)
            .arg(&source)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(diagnostic),
            "{id}: {output:?}"
        );
        eprintln!("SIMULATION_WORLD_CLI_39_PASS downstream-{id}/cli");
    }
}
