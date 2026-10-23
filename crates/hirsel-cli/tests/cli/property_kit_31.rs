//! Full property-kit entry: independent fixtures, four engines and immutable replay.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, with_compiler_stack};
use hirsel_run_schema::{Access, FileReadScript};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/propkit/main.hir"),
        ),
        (
            "strategy",
            include_str!("../../../../examples/propkit/strategy.hir"),
        ),
        (
            "shrink",
            include_str!("../../../../examples/propkit/shrink.hir"),
        ),
        (
            "runner",
            include_str!("../../../../examples/propkit/runner.hir"),
        ),
        (
            "replay",
            include_str!("../../../../examples/propkit/replay.hir"),
        ),
        (
            "properties",
            include_str!("../../../../examples/propkit/properties.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/propkit/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path
}
fn scratch(label: &str) -> tempfile::TempDir {
    let base = std::env::var_os("PROPERTY_KIT_31_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
}
fn cases() -> Vec<Value> {
    let mut cases: Vec<Value> =
        serde_json::from_str(include_str!("../support/property_kit_31/cases.json")).unwrap();
    if let Ok(ids) = std::env::var("PROPERTY_KIT_31_CASE") {
        cases.retain(|c| ids.split(',').any(|id| c["id"] == id));
    }
    assert!(!cases.is_empty(), "zero-test selection");
    cases
}
fn fixture(case: &Value) -> Fixture {
    let mut f = Fixture {
        args: serde_json::from_value(case["args"].clone()).unwrap(),
        mounts: BTreeMap::new(),
        steps: case["steps"].as_u64(),
        ..Default::default()
    };
    if case["denied"] != true {
        f.mounts.insert("records".into(), Access::Read);
    }
    f.mounts.insert("output".into(), Access::ReadWrite);
    if !case["record"].is_null() {
        let mut raw = case["record"].to_string().into_bytes();
        if let Some(size) = case["size"].as_u64() {
            raw.resize(size as usize, b' ');
        }
        f.files.insert("records/failure.json".into(), raw);
    }
    if case["pending"] == true {
        f.file_reads
            .insert("records/failure.json".into(), FileReadScript::Pending);
    }
    if case["release"] == true {
        f.file_reads.insert(
            "records/failure.json".into(),
            FileReadScript::AfterWrite {
                path: "output/release".into(),
            },
        );
    }
    f
}
fn expected(case: &Value, observed: &Observation) {
    assert_eq!(
        observed.exit_code,
        Some(case["exit"].as_i64().unwrap() as i32),
        "{case}: {observed:?}"
    );
    assert!(
        !observed.blocked
            && observed.failures.is_empty()
            && observed.budget.is_none()
            && observed.signaled.is_none(),
        "{observed:?}"
    );
    if case["json_output"] == true {
        assert_eq!(
            serde_json::from_slice::<Value>(&observed.output).unwrap(),
            serde_json::from_str::<Value>(case["output"].as_str().unwrap()).unwrap()
        );
    } else {
        assert_eq!(
            observed.output,
            case["output"].as_str().unwrap().as_bytes(),
            "{}",
            case["id"]
        );
    }
}

#[test]
fn property_kit_31_reference_checked_full_entry() {
    with_compiler_stack(|| {
        let temp = scratch("propkit-reference-");
        let selection = package(temp.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(problem);
        loaded.check_for_execution().unwrap_or_else(problem);
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut completed = 0;
        for case in cases() {
            let id = case["id"].as_str().unwrap();
            let f = fixture(&case);
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                eprintln!("PROPERTY_KIT_31_STARTED {id}/{engine}");
                let world = (&f).into();
                let result = if engine == "reference" {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &config,
                        world,
                        f.steps.unwrap_or(1_000_000),
                    )
                } else {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &config,
                        world,
                        f.steps.unwrap_or(1_000_000),
                    )
                }
                .unwrap();
                eprintln!(
                    "PROPERTY_KIT_31_STEPS {id}/{engine} {:?}",
                    result.step_usage
                );
                let observed = result
                    .observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("INCOMPLETE {id}/{engine}: {result:?}"));
                expected(&case, &Observation::of_reference(observed));
                if case["pending"] == true || case["release"] == true {
                    assert!(observed.effects.iter().any(|e| matches!(&e.effect, hirsel_interp::observation::Effect::CapabilityConsumed(event) if event.operation == "file.read.pending")), "read never parked");
                }
                if let Some(previous) = &previous {
                    assert_eq!(observed, previous);
                }
                previous = Some(observed.clone());
                completed += 1;
                eprintln!("PROPERTY_KIT_31_PASS {id}/{engine}");
            }
        }
        assert!(completed > 0);
        eprintln!("PROPERTY_KIT_31_COMPLETED {completed}");
        if std::env::var_os("PROPERTY_KIT_31_EVIDENCE").is_some() {
            let _ = temp.keep();
        }
    });
}

#[test]
fn property_kit_31_native_full_entry_and_four_engine_replay() {
    with_compiler_stack(|| {
        let temp = scratch("propkit-native-");
        let selection = package(temp.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(problem);
        loaded.check_for_execution().unwrap_or_else(problem);
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = [
            Engine::Native {
                opt: OptLevel::None,
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
        let mut tools = Toolchain::for_matrix(&temp.path().join("runtime"), &matrix).unwrap();
        tools.launch_timeout = std::time::Duration::from_secs(60);
        capped_tools(&mut tools, temp.path());
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let mut objects = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            eprintln!("PROPERTY_KIT_31_BUILD {backend:?}");
            objects.push((
                backend,
                emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap(),
            ));
        }
        let mut completed = 0;
        for case in cases() {
            let id = case["id"].as_str().unwrap();
            let f = fixture(&case);
            let mut linked = vec![];
            for (backend, objects) in &objects {
                let dir = temp.path().join(format!("{id}-{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &f)
                        .unwrap();
                linked.push((runnable, dir));
            }
            for (recorder, dir) in &linked {
                eprintln!("PROPERTY_KIT_31_STARTED {id}/{}", dir.display());
                let tape = dir.join("recording");
                let recorded = launch_fixture(
                    recorder,
                    &ready.target,
                    &tools,
                    dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    &f,
                )
                .unwrap();
                let observation = observe(&recorded, &ready, &sites).unwrap();
                expected(&case, &observation);
                let bytes = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                assert_eq!(record.arguments, f.args);
                if case["pending"] == true || case["release"] == true {
                    assert!(
                        record.events.iter().any(|e| matches!(
                            (&e.request, &e.completion),
                            (
                                hirsel_rt::io::Request::FileRead { .. },
                                hirsel_rt::io::Completion::Blocked { committed: false }
                            )
                        )),
                        "read never parked"
                    );
                    assert!(
                        record
                            .events
                            .iter()
                            .any(|e| matches!(e.request, hirsel_rt::io::Request::Close { .. })),
                        "no cleanup"
                    );
                }
                let absent = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    ..f.clone()
                };
                for (runnable, replay_dir) in &linked {
                    let replayed = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &absent,
                    )
                    .unwrap();
                    assert_eq!(observe(&replayed, &ready, &sites).unwrap(), observation);
                }
                for replay in [
                    hirsel_driver::replay_observations,
                    hirsel_driver::replay_observations_checked,
                ] {
                    let observed =
                        replay(&selection, &tape, Some(f.steps.unwrap_or(1_000_000))).unwrap();
                    assert_eq!(Observation::of_reference(&observed), observation);
                }
                let bad = dir.join("invalid-header");
                let mut changed = record.clone();
                changed.arguments.push("tampered".into());
                fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
                for replay in [
                    hirsel_driver::replay_observations,
                    hirsel_driver::replay_observations_checked,
                ] {
                    assert!(replay(&selection, &bad, Some(1_000_000)).is_err());
                }
                assert_eq!(bytes, fs::read(&tape).unwrap());
                completed += 1;
                eprintln!("PROPERTY_KIT_31_PASS {id}/{} replay=4", dir.display());
            }
        }
        assert!(completed > 0);
        eprintln!("PROPERTY_KIT_31_COMPLETED_NATIVE {completed}");
        if std::env::var_os("PROPERTY_KIT_31_EVIDENCE").is_some() {
            let _ = temp.keep();
        }
    });
}

#[test]
fn property_kit_31_independent_generic_contract_vectors() {
    with_compiler_stack(|| {
        let temp = scratch("propkit-contracts-");
        let selection = package(temp.path());
        fs::write(
            temp.path().join("contracts.hir"),
            include_str!("../support/property_kit_31/contracts.hir"),
        )
        .unwrap();
        let mut manifest: Value = serde_json::from_slice(&fs::read(&selection).unwrap()).unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"path":"contracts.hir","module":["contracts"]}));
        manifest["entry"]["module"] = serde_json::json!(["contracts"]);
        fs::write(&selection, manifest.to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(problem);
        loaded.check_for_execution().unwrap_or_else(problem);
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        const STEPS: u64 = 10_000_000;
        let reference = hirsel_engine::run_observed_snapshot_bounded(
            &mut loaded.engine,
            &config,
            Default::default(),
            STEPS,
        )
        .unwrap();
        eprintln!(
            "PROPERTY_KIT_31_STEPS contracts/reference {:?}",
            reference.step_usage
        );
        let observed = reference
            .observation
            .as_ref()
            .unwrap_or_else(|| panic!("INCOMPLETE {reference:?}"));
        let expected = Observation::of_reference(observed);
        assert_eq!(expected.exit_code, Some(0));
        assert!(expected.failures.is_empty() && !expected.blocked);
        let checked = hirsel_engine::run_observed_checked_bounded(
            &mut loaded.engine,
            &config,
            Default::default(),
            STEPS,
        )
        .unwrap();
        assert_eq!(reference.observation, checked.observation);
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = [
            Engine::Native {
                opt: OptLevel::None,
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
        let mut tools = Toolchain::for_matrix(&temp.path().join("runtime"), &matrix).unwrap();
        capped_tools(&mut tools, temp.path());
        let fixture = Fixture {
            steps: Some(STEPS),
            ..Default::default()
        };
        for (engine, outcome) in differential_in(
            &input,
            &loaded.entry,
            &expected,
            &matrix,
            &tools,
            temp.path(),
            &fixture,
        ) {
            assert_eq!(outcome, hirsel_driver::native::Outcome::Agree, "{engine}");
            eprintln!("PROPERTY_KIT_31_PASS contracts/{engine}");
        }
    });
}

fn problem<T>(error: hirsel_engine::ToolError) -> T {
    match error {
        hirsel_engine::ToolError::Query { error, .. } => panic!("{error:?}"),
        other => panic!("{other}"),
    }
}

#[test]
#[ignore = "production Ordered qualification requires a local four CPU allowance"]
fn property_kit_31_production_ordered_widths_and_replay() {
    let temp = scratch("p-");
    let retained = std::env::var_os("PROPERTY_KIT_31_PRODUCTION_REPLAY_DIR").map(PathBuf::from);
    let root = retained.as_deref().unwrap_or_else(|| temp.path());
    let report: Value = if retained.is_some() {
        serde_json::from_slice(&fs::read(root.join("run.stdout")).unwrap()).unwrap()
    } else {
        package(root);
        fs::write(
            root.join("production.py"),
            include_str!("../support/property_kit_31/production.py"),
        )
        .unwrap();
        fs::write(
            root.join("failure.json"),
            include_bytes!("../../../../examples/propkit/fixtures/failure.json"),
        )
        .unwrap();
        let capped = root.join("capped");
        fs::write(&capped, include_str!("../../../../tools/capped")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&capped, fs::Permissions::from_mode(0o755)).unwrap();
        let executed = std::process::Command::new("python3")
            .arg(root.join("production.py"))
            .arg("--hirsel")
            .arg(env!("CARGO_BIN_EXE_hirsel"))
            .arg("--root")
            .arg(root)
            .arg("--capped")
            .arg(capped)
            .output()
            .unwrap();
        fs::write(root.join("production.stdout"), &executed.stdout).unwrap();
        fs::write(root.join("production.stderr"), &executed.stderr).unwrap();
        if !executed.status.success() {
            let retained = temp.keep();
            panic!(
                "production INCOMPLETE at {}: {}",
                retained.display(),
                String::from_utf8_lossy(&executed.stderr)
            );
        }
        serde_json::from_slice(&executed.stdout).unwrap()
    };
    assert!(report["passed"].as_u64().unwrap() > 0);
    assert_eq!(report["passed"], report["selected"]);
    let mut tapes = BTreeMap::new();
    if std::env::var("PROPERTY_KIT_31_PRODUCTION_REPLAY_PHASE").as_deref() != Ok("controls") {
        with_compiler_stack(|| {
            for receipt in report["production"].as_array().unwrap() {
                let cell = receipt["cell"].as_str().unwrap();
                let directory = root.join(cell);
                let tape = directory.join("recording.tape");
                let bytes = fs::read(&tape).unwrap();
                let expected_output = fs::read(directory.join("stdout")).unwrap();
                let code: i32 = fs::read_to_string(directory.join("exit"))
                    .unwrap()
                    .parse()
                    .unwrap();
                let mut record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                let binding = record.binding.as_mut().unwrap();
                binding.admitted_at = 0;
                binding.placement = None;
                let parts: Vec<_> = cell.split('-').collect();
                let key = format!("{}-{}", parts[0], parts[2]);
                if let Some(previous) = tapes.insert(key, record.clone()) {
                    assert_eq!(previous, record, "semantic tape {cell}");
                }
                if cell.ends_with("cancel") {
                    assert!(
                        record.events.iter().any(|event| matches!(
                            (&event.request, &event.completion),
                            (
                                hirsel_rt::io::Request::ReadStdin { .. },
                                hirsel_rt::io::Completion::Blocked { committed: false }
                            )
                        )),
                        "pending stdin proof {cell}"
                    );
                }
                for replay in [
                    hirsel_driver::replay_observations,
                    hirsel_driver::replay_observations_checked,
                ] {
                    let observed =
                        replay(&root.join("hirsel.json"), &tape, Some(1_000_000)).unwrap();
                    assert_eq!(observed.output, expected_output, "{cell}");
                    assert_eq!(observed.exit_code, Some(code), "{cell}");
                    assert!(observed.failures.is_empty() && observed.blocked.is_none());
                }
                assert_eq!(fs::read(&tape).unwrap(), bytes);
                assert!(!root.join("records/failure.json").exists());
                assert_eq!(fs::read_dir(root.join("output")).unwrap().count(), 0);
                eprintln!(
                    "PROPERTY_KIT_31_PASS production/{cell} public_replay=2 native_replay=previous"
                );
            }
        });
    }
    with_compiler_stack(|| {
        let cell = report["production"][0]["cell"].as_str().unwrap();
        let tape = root.join(cell).join("recording.tape");
        let original = fs::read(&tape).unwrap();
        let record = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
        let mut inputs = record.clone();
        inputs.arguments.push("changed-input".into());
        let mut config = record.clone();
        let mut run: Value =
            serde_json::from_slice(&config.binding.as_ref().unwrap().canonical).unwrap();
        run["inputs"]["hash_seed"] = 1u64.into();
        config.binding.as_mut().unwrap().canonical = serde_json::to_vec(&run).unwrap();
        let mut suffix = record.clone();
        suffix.events.push(record.events[0].clone());
        for (kind, changed) in [("input", inputs), ("config", config), ("suffix", suffix)] {
            if let Ok(selected) = std::env::var("PROPERTY_KIT_31_CONTROL_KINDS") {
                if !selected.split(',').any(|item| item == kind) {
                    continue;
                }
            }
            let bad = temp.path().join(format!("refuse-{kind}.tape"));
            fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
            if std::env::var_os("PROPERTY_KIT_31_NATIVE_CONTROLS_ONLY").is_none() {
                for replay in [
                    hirsel_driver::replay_observations,
                    hirsel_driver::replay_observations_checked,
                ] {
                    assert!(
                        replay(&root.join("hirsel.json"), &bad, Some(1_000_000)).is_err(),
                        "{kind}"
                    );
                }
            }
            for backend in ["llvm", "cranelift"] {
                let refused = std::process::Command::new("python3")
                    .arg(root.join("capped"))
                    .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                    .arg(root.join(backend))
                    .arg("--hirsel-replay")
                    .arg(&bad)
                    .env("HIRSEL_MEMORY_LIMIT", "2G")
                    .env("HIRSEL_JOBS", "4")
                    .output()
                    .unwrap();
                assert_eq!(
                    refused.status.code(),
                    Some(3),
                    "{kind}/{backend}: {refused:?}"
                );
                if kind == "suffix" {
                    assert_eq!(
                        refused.stdout,
                        fs::read(root.join(cell).join("stdout")).unwrap(),
                        "late refusal prefix"
                    );
                } else {
                    assert!(
                        refused.stdout.is_empty(),
                        "pre-entry refusal performed output"
                    );
                }
            }
            let adapters = if std::env::var_os("PROPERTY_KIT_31_NATIVE_CONTROLS_ONLY").is_some() {
                2
            } else {
                4
            };
            eprintln!("PROPERTY_KIT_31_PASS refusal/{kind} adapters_this_invocation={adapters}");
        }
        for replay in [
            hirsel_driver::replay_observations,
            hirsel_driver::replay_observations_checked,
        ] {
            let error = replay(&root.join("hirsel.json"), &tape, Some(1))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("Steps") && error.contains("limit: 1"),
                "{error}"
            );
        }
        assert_eq!(fs::read(&tape).unwrap(), original);
    });
    eprintln!("PROPERTY_KIT_31_PRODUCTION {report}");
    if std::env::var_os("PROPERTY_KIT_31_EVIDENCE").is_some() {
        let _ = temp.keep();
    }
}

fn capped_tools(tools: &mut Toolchain, root: &Path) {
    let capped = root.join("capped");
    fs::write(&capped, include_str!("../../../../tools/capped")).unwrap();
    tools.execution = Execution::Executable;
    tools.wrapper = vec![
        "env".into(),
        "HIRSEL_MEMORY_LIMIT=2G".into(),
        "HIRSEL_JOBS=4".into(),
        "python3".into(),
        capped.to_string_lossy().into_owned(),
        "--timeout".into(),
        "60".into(),
        "prlimit".into(),
        "--cpu=60:61".into(),
    ];
}
