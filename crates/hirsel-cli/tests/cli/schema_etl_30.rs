//! Actual six-module application entry, deterministic providers, full bytes and cleanup.
use hirsel_engine::*;
use hirsel_interp::library::simulation::{Fault, FileError, ObservedGrant};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn package(directory: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/schemaetl/main.hir"),
        ),
        (
            "plan",
            include_str!("../../../../examples/schemaetl/plan.hir"),
        ),
        (
            "decode",
            include_str!("../../../../examples/schemaetl/decode.hir"),
        ),
        (
            "validate",
            include_str!("../../../../examples/schemaetl/validate.hir"),
        ),
        (
            "stages",
            include_str!("../../../../examples/schemaetl/stages.hir"),
        ),
        (
            "sink",
            include_str!("../../../../examples/schemaetl/sink.hir"),
        ),
    ] {
        fs::write(directory.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/schemaetl/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = root()
        .join("stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let selection = directory.join("hirsel.json");
    fs::write(&selection, manifest.to_string()).unwrap();
    selection
}
fn cases() -> Vec<Value> {
    let scratch = tempfile::tempdir().unwrap();
    fs::create_dir(scratch.path().join("fixtures")).unwrap();
    for (name, source) in [
        (
            "pipeline.json",
            include_str!("../../../../examples/schemaetl/fixtures/pipeline.json"),
        ),
        (
            "a.csv",
            include_str!("../../../../examples/schemaetl/fixtures/a.csv"),
        ),
        (
            "b.jsonl",
            include_str!("../../../../examples/schemaetl/fixtures/b.jsonl"),
        ),
    ] {
        fs::write(scratch.path().join("fixtures").join(name), source).unwrap();
    }
    let oracle = scratch.path().join("oracle.py");
    fs::write(&oracle, include_str!("../support/schema_etl_30/oracle.py")).unwrap();
    let output = Command::new("python3").arg(oracle).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    if let Ok(filter) = std::env::var("SCHEMA_ETL_30_CASE") {
        cases.retain(|c| filter.split(',').any(|id| c["id"] == id));
    }
    assert!(!cases.is_empty(), "zero selected identities");
    cases
}
fn bytes(value: &Value) -> Vec<u8> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap() as u8)
        .collect()
}
fn world(case: &Value) -> SimulationConfig {
    let mut world = SimulationConfig {
        args: vec![
            case["command"].as_str().unwrap().into(),
            "manifest.json".into(),
            "pipeline.json".into(),
        ],
        mounts: [
            ("input".into(), hirsel_run_schema::Access::Read),
            ("output".into(), hirsel_run_schema::Access::ReadWrite),
        ]
        .into(),
        directories: ["input".into(), "output".into()].into(),
        ..Default::default()
    };
    world.files.insert(
        "input/manifest.json".into(),
        case["manifest"].to_string().into_bytes(),
    );
    world.files.insert(
        "input/pipeline.json".into(),
        case["pipeline"].to_string().into_bytes(),
    );
    for (name, data) in case["files"].as_object().unwrap() {
        world.files.insert(format!("input/{name}"), bytes(data));
    }
    if !case["pending"].is_null() {
        world.file_reads.insert(
            "input/f0.jsonl".into(),
            serde_json::from_value(case["pending"].clone()).unwrap(),
        );
    }
    if case["fault"] == true {
        world.faults.insert(
            "file.write_all".into(),
            [Fault::File {
                error: FileError::Fault(7),
                partial: 0,
            }]
            .into(),
        );
    }
    world
}
fn assert_app(case: &Value, observed: &hirsel_interp::observation::Observable) {
    assert_eq!(
        observed.exit_code,
        Some(case["exit"].as_i64().unwrap() as i32),
        "{observed:?}"
    );
    assert!(
        observed.failures.is_empty() && observed.blocked.is_none(),
        "{observed:?}"
    );
    assert_eq!(observed.output, case["stdout"].as_str().unwrap().as_bytes());
    let final_state = observed.final_state.as_ref().unwrap();
    assert!(
        !final_state.handles.values().any(|h| matches!(
            h,
            ObservedGrant::File { .. } | ObservedGrant::AtomicFile { .. }
        )),
        "open file after entry"
    );
    let files = final_state
        .trees
        .values()
        .flat_map(|(_, files)| files.iter())
        .collect::<std::collections::BTreeMap<_, _>>();
    if case["exit"] == 0 && case["command"] == "run" {
        for (path, key) in [
            ("output/data.jsonl", "data"),
            ("output/summary.json", "summary"),
        ] {
            assert_eq!(
                files.get(&path.to_string()).unwrap().as_slice(),
                case[key].as_str().unwrap().as_bytes(),
                "{path}"
            );
        }
        let count = case["data"].as_str().unwrap().len();
        assert_eq!(files.get(&"output/completion.json".to_string()).unwrap().as_slice(), format!("{{\"schema\":1,\"data\":\"data.jsonl\",\"summary\":\"summary.json\",\"bytes\":{count}}}\n").as_bytes());
    } else {
        assert!(!files.contains_key(&"output/completion.json".to_string()));
    }
    if !case["pending"].is_null() {
        assert!(
            format!("{:?}", observed.effects).contains("file.read.pending"),
            "pending read never started"
        );
    }
}
#[test]
fn schema_etl_30_scripted_reference_checked() {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let selection = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut count = 0;
        for case in cases() {
            let id = case["id"].as_str().unwrap();
            let steps = case["steps"].as_u64().unwrap();
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                if std::env::var("SCHEMA_ETL_30_ENGINE").is_ok_and(|selected| selected != engine) {
                    continue;
                }
                eprintln!("SCHEMA_ETL_30_STARTED {id} {engine} steps={steps}");
                let result = if engine == "reference" {
                    run_observed_snapshot_bounded(&mut loaded.engine, &config, world(&case), steps)
                } else {
                    run_observed_checked_bounded(&mut loaded.engine, &config, world(&case), steps)
                }
                .unwrap();
                eprintln!("SCHEMA_ETL_30_USAGE {id} {engine} {:?}", result.step_usage);
                let Some(observed) = result.observation.as_ref() else {
                    eprintln!("SCHEMA_ETL_30_INCOMPLETE {id} {engine}: {result:?}");
                    assert!(
                        ["bytes-65536", "rows-1024", "output-262144", "output-262145"]
                            .contains(&id),
                        "unexpected tooling stop"
                    );
                    assert!(matches!(result.outcome, Ok(RunOutcome::ToolingLimit(_))));
                    count += 1;
                    continue;
                };
                assert_app(&case, observed);
                if let Some(previous) = previous {
                    assert_eq!(*observed, previous, "{id}: semantic order");
                }
                previous = Some(observed.clone());
                eprintln!("SCHEMA_ETL_30_PASS {id} {engine}");
                count += 1;
            }
        }
        assert!(count > 0);
    });
}

#[test]
fn schema_etl_30_native_host_and_four_engine_replay() {
    if let Some(evidence) = std::env::var_os("SCHEMA_ETL_30_EXISTING") {
        with_compiler_stack(|| existing_replay(Path::new(&evidence)));
        return;
    }
    with_compiler_stack(|| {
        let evidence = std::env::var_os("SCHEMA_ETL_30_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&evidence).unwrap();
        let scratch = tempfile::Builder::new()
            .prefix("schema-etl-30-native-")
            .tempdir_in(&evidence)
            .unwrap();
        let directory = scratch.path();
        let manifest = package(directory);
        let capped = directory.join("capped");
        fs::write(&capped, include_str!("../../../../tools/capped")).unwrap();
        let selected: Vec<_> = cases()
            .into_iter()
            .filter(|case| {
                !["bytes-65536", "rows-1024", "output-262144", "output-262145"]
                    .contains(&case["id"].as_str().unwrap())
            })
            .collect();
        let mut completed = 0;
        for backend in ["llvm", "cranelift"] {
            let executable = directory.join(backend);
            let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap();
            assert!(built.status.success(), "{built:?}");
        }
        for backend in ["llvm", "cranelift"] {
            let executable = directory.join(backend);
            for case in &selected {
                // Native host files supplement scripted pending/fault fixtures.
                if !case["pending"].is_null() || case["fault"] == true {
                    continue;
                }
                let id = case["id"].as_str().unwrap();
                let work = directory.join(format!("{backend}-{id}"));
                fs::create_dir_all(work.join("input")).unwrap();
                fs::create_dir(work.join("output")).unwrap();
                fs::write(
                    work.join("input/manifest.json"),
                    case["manifest"].to_string(),
                )
                .unwrap();
                fs::write(
                    work.join("input/pipeline.json"),
                    case["pipeline"].to_string(),
                )
                .unwrap();
                for (name, data) in case["files"].as_object().unwrap() {
                    fs::write(work.join("input").join(name), bytes(data)).unwrap();
                }
                let grants = serde_json::json!({"args":[],"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"input","host":work.join("input"),"access":"read"},{"path":"output","host":work.join("output"),"access":"read_write"}]},"placement":{"cores":1,"commit":"shared","reorder":"auto"}});
                let grants_path = work.join("grants.json");
                fs::write(&grants_path, grants.to_string()).unwrap();
                let tape = work.join("record.tape");
                eprintln!("SCHEMA_ETL_30_STARTED {id} {backend} host-files");
                let output = Command::new("python3")
                    .arg(&capped)
                    .args(["--timeout", "60"])
                    .arg("prlimit")
                    .args(["--cpu=60:61", "--as=2147483648"])
                    .arg(&executable)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-grants")
                    .arg(&grants_path)
                    .args([
                        case["command"].as_str().unwrap(),
                        "manifest.json",
                        "pipeline.json",
                    ])
                    .env("HIRSEL_JOBS", "4")
                    .output()
                    .unwrap();
                fs::write(work.join("stdout"), &output.stdout).unwrap();
                fs::write(work.join("stderr"), &output.stderr).unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(case["exit"].as_i64().unwrap() as i32),
                    "{id}/{backend}: {output:?}"
                );
                assert_eq!(output.stdout, case["stdout"].as_str().unwrap().as_bytes());
                if case["exit"] == 0 && case["command"] == "run" {
                    assert_eq!(
                        fs::read(work.join("output/data.jsonl")).unwrap(),
                        case["data"].as_str().unwrap().as_bytes()
                    );
                    assert_eq!(
                        fs::read(work.join("output/summary.json")).unwrap(),
                        case["summary"].as_str().unwrap().as_bytes()
                    );
                    assert!(work.join("output/completion.json").exists());
                } else {
                    assert!(!work.join("output/completion.json").exists());
                }
                fs::rename(work.join("input"), work.join("unavailable-input")).unwrap();
                fs::rename(work.join("output"), work.join("unavailable-output")).unwrap();
                let original = fs::read(&tape).unwrap();
                let ordinary = hirsel_driver::replay_observations(
                    &manifest,
                    &tape,
                    Some(case["steps"].as_u64().unwrap()),
                )
                .unwrap();
                let checked = hirsel_driver::replay_observations_checked(
                    &manifest,
                    &tape,
                    Some(case["steps"].as_u64().unwrap()),
                )
                .unwrap();
                assert!(ordinary.equivalent(&checked));
                assert_eq!(ordinary.output, output.stdout);
                assert_eq!(ordinary.exit_code, output.status.code());
                for replay_backend in ["llvm", "cranelift"] {
                    let replay = Command::new(directory.join(replay_backend))
                        .arg("--hirsel-replay")
                        .arg(&tape)
                        .output()
                        .unwrap();
                    assert_eq!(replay.status.code(), output.status.code());
                    assert_eq!(replay.stdout, output.stdout);
                    assert!(replay.stderr.starts_with(if case["exit"] == 0 {
                        b""
                    } else {
                        b"error:"
                    }));
                }
                assert_eq!(original, fs::read(&tape).unwrap());
                assert!(!work.join("input").exists() && !work.join("output").exists());
                eprintln!("SCHEMA_ETL_30_PASS {id} {backend} host-files replay-reference-checked");
                completed += 1;
            }
        }
        assert!(completed > 0);
        if std::env::var_os("SCHEMA_ETL_30_EVIDENCE").is_some() {
            let _ = scratch.keep();
        }
    });
}

fn existing_replay(evidence: &Path) {
    use hirsel_rt::io::{Completion, Request};
    let scratch = tempfile::tempdir().unwrap();
    let manifest = package(scratch.path());
    let mut passed = 0;
    let mut incomplete = 0;
    for case in cases() {
        if !case["pending"].is_null() || case["fault"] == true {
            continue;
        }
        let id = case["id"].as_str().unwrap();
        for recorder in ["llvm", "cranelift"] {
            let work = evidence.join(format!("native-{recorder}-{id}"));
            let tape = work.join("record.tape");
            let original = fs::read(&tape).unwrap();
            for name in ["input", "output"] {
                if work.join(name).exists() {
                    fs::rename(work.join(name), work.join(format!("unavailable-{name}"))).unwrap();
                }
            }
            let mut observations = Vec::new();
            for engine in ["reference", "checked-h"] {
                eprintln!("SCHEMA_ETL_30_REPLAY_STARTED {id} {recorder} {engine}");
                let result = if engine == "reference" {
                    hirsel_driver::replay_observations(
                        &manifest,
                        &tape,
                        Some(case["steps"].as_u64().unwrap()),
                    )
                } else {
                    hirsel_driver::replay_observations_checked(
                        &manifest,
                        &tape,
                        Some(case["steps"].as_u64().unwrap()),
                    )
                };
                match result {
                    Ok(observed) => {
                        assert_eq!(observed.output, case["stdout"].as_str().unwrap().as_bytes());
                        assert_eq!(
                            observed.exit_code,
                            Some(case["exit"].as_i64().unwrap() as i32)
                        );
                        assert!(observed.failures.is_empty());
                        observations.push(observed);
                        eprintln!("SCHEMA_ETL_30_REPLAY_PASS {id} {recorder} {engine}");
                        passed += 1;
                    }
                    Err(error) => {
                        eprintln!(
                            "SCHEMA_ETL_30_REPLAY_INCOMPLETE {id} {recorder} {engine}: {error:?}"
                        );
                        assert!(
                            ["bytes-65536", "rows-1024", "output-262144", "output-262145"]
                                .contains(&id),
                            "unexpected replay error: {error}"
                        );
                        assert!(error.to_string().contains("no semantic observation"));
                        incomplete += 1;
                    }
                }
            }
            if observations.len() == 2 {
                assert!(observations[0].equivalent(&observations[1]));
            }
            for engine in ["llvm", "cranelift"] {
                let executable = evidence.join(format!("final-{engine}"));
                let output = Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(case["exit"].as_i64().unwrap() as i32),
                    "{id}/{recorder}/{engine}: {output:?}"
                );
                assert_eq!(output.stdout, case["stdout"].as_str().unwrap().as_bytes());
                eprintln!("SCHEMA_ETL_30_REPLAY_PASS {id} {recorder} {engine}");
                passed += 1;
            }
            let recording = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
            let mut handles = std::collections::BTreeSet::new();
            for event in &recording.events {
                match (&event.request, &event.completion) {
                    (
                        Request::Open { .. } | Request::CreateAtomic { .. },
                        Completion::Handle(Ok(handle)),
                    ) => {
                        handles.insert(*handle);
                    }
                    (Request::Close { handle } | Request::Commit { file: handle, .. }, _) => {
                        handles.remove(handle);
                    }
                    _ => (),
                }
            }
            assert!(handles.is_empty(), "unclosed native resources: {handles:?}");
            if id == "mixed-success" {
                let mut input = recording.clone();
                let event = input
                    .events
                    .iter_mut()
                    .find(|e| matches!(e.request, Request::FileRead { .. }))
                    .unwrap();
                event.completion = Completion::Bytes(Ok(b"{}".to_vec()));
                let mut config = recording.clone();
                config.run_identity[0] ^= 1;
                let mut extra = recording.clone();
                extra.events.push(recording.events[0].clone());
                for (kind, changed) in [("input", input), ("config", config), ("tape", extra)] {
                    let bad = work.join(format!("changed-{kind}.tape"));
                    fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap())
                        .unwrap();
                    assert!(
                        hirsel_driver::replay_observations(&manifest, &bad, Some(8_000_000))
                            .is_err()
                    );
                    assert!(
                        hirsel_driver::replay_observations_checked(
                            &manifest,
                            &bad,
                            Some(8_000_000)
                        )
                        .is_err()
                    );
                    for engine in ["llvm", "cranelift"] {
                        let refused = Command::new(evidence.join(format!("final-{engine}")))
                            .arg("--hirsel-replay")
                            .arg(&bad)
                            .output()
                            .unwrap();
                        assert_eq!(
                            refused.status.code(),
                            Some(3),
                            "{kind}/{engine}: {refused:?}"
                        );
                    }
                    eprintln!("SCHEMA_ETL_30_REFUSAL_PASS {recorder} {kind} four-engines");
                }
            }
            assert_eq!(original, fs::read(&tape).unwrap());
            assert!(!work.join("input").exists() && !work.join("output").exists());
        }
    }
    assert!(passed > 0);
    eprintln!("SCHEMA_ETL_30_REPLAY_COUNTS pass={passed} incomplete={incomplete}");
}

// Exact current limitation witness, not a semantic pass for native cancellation.
// Fixture has scripted file gates but no Args/files/mount snapshot members.
#[test]
fn schema_etl_30_native_scripted_entry_boundary_witness() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let selection = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = [
            Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .map(|engine| Configuration {
            options: options.clone(),
            engine,
        });
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: b"complete\n".to_vec(),
            failures: vec![],
            roots: vec![],
        };
        let mut tools = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        tools.launch_timeout = std::time::Duration::from_secs(60);
        let fixture = Fixture {
            file_reads: [(
                "input/f0.jsonl".into(),
                hirsel_run_schema::FileReadScript::Pending,
            )]
            .into(),
            ..Default::default()
        };
        for (engine, outcome) in differential_in(
            &input,
            &loaded.entry,
            &expected,
            &matrix,
            &tools,
            scratch.path(),
            &fixture,
        ) {
            let Outcome::Mismatch(actual) = outcome else {
                panic!("witness changed: {outcome:?}")
            };
            assert_eq!(actual.exit_code, Some(1));
            assert!(actual.failures.is_empty() && actual.output.is_empty());
            eprintln!(
                "SCHEMA_ETL_30_ADAPTER_INCOMPLETE {engine}: actual entry received empty fixture Args, returned usage before input projection; no Pending cancellation observation"
            );
        }
    });
}
