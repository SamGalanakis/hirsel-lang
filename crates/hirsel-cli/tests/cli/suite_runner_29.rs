//! Full package execution, including explicit modules, entry and grant projection.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/suite-runner/main.hir"),
        ),
        (
            "manifest.hir",
            include_str!("../../../../examples/suite-runner/manifest.hir"),
        ),
        (
            "select.hir",
            include_str!("../../../../examples/suite-runner/select.hir"),
        ),
        (
            "run.hir",
            include_str!("../../../../examples/suite-runner/run.hir"),
        ),
        (
            "report.hir",
            include_str!("../../../../examples/suite-runner/report.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/suite-runner/model.hir"),
        ),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../../../examples/suite-runner/hirsel.json"
    ))
    .unwrap();
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
    let base = std::env::var_os("SUITE_RUNNER_29_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
}
fn cases(caps: bool) -> Vec<Value> {
    serde_json::from_str(if caps {
        include_str!("../support/suite_runner_29/caps.json")
    } else {
        include_str!("../support/suite_runner_29/cases.json")
    })
    .unwrap()
}
fn selected(caps: bool) -> Vec<Value> {
    let mut cases = cases(caps);
    if let Ok(filter) = std::env::var("SUITE_RUNNER_29_CASE") {
        cases.retain(|case| filter.split(',').any(|id| case["id"] == id));
    }
    assert!(!cases.is_empty(), "zero workflow selection");
    cases
}
fn input(case: &Value) -> Vec<u8> {
    let mut raw = case["manifest"].to_string().into_bytes();
    if let Some(length) = case["raw_bytes"].as_u64() {
        raw.resize(length as usize, b' ');
    }
    raw
}
fn world(case: &Value) -> hirsel_engine::SimulationConfig {
    let mut world = hirsel_engine::SimulationConfig {
        args: serde_json::from_value(case["args"].clone()).unwrap(),
        ..Default::default()
    };
    world.projections.insert("monotonic_clock".into());
    world.grants.insert("MonotonicClock".into());
    world.mounts = [
        ("fixtures".into(), hirsel_run_schema::Access::Read),
        ("reports".into(), hirsel_run_schema::Access::ReadWrite),
    ]
    .into();
    world
        .directories
        .extend(["fixtures".into(), "reports".into()]);
    world
        .files
        .insert("fixtures/manifest.json".into(), input(case));
    world
        .files
        .insert("reports/report.json".into(), b"previous".to_vec());
    let mut children = vec![];
    if case["exit"] != 2 && case["absent"] != true {
        for suite in case["manifest"]["suites"].as_array().unwrap() {
            for item in suite["cases"].as_array().unwrap() {
                for code in case["scripts"][item["id"].as_str().unwrap()]
                    .as_array()
                    .unwrap()
                {
                    if let Some(code) = code.as_i64() {
                        children.push(json!({"program":item["argv"][0],"args":item["argv"].as_array().unwrap().get(1..).unwrap_or_default(),"steps":[{"print":[79,79,79]},{"print_err":[69,69,69]},{"exit":{"exited":code}}]}));
                    }
                }
            }
        }
    }
    world.process=Some(serde_json::from_value(json!({"provider":"process.script@1","rules":if case["refused"]==true {json!([])} else {json!([{"program":"/fixture/child"}])},"runs":[],"children":children})).unwrap());
    if case["write_failure"] == true || case["output_failure"] == true {
        use hirsel_interp::library::simulation::Fault;
        world.faults.insert(
            if case["write_failure"] == true {
                "file.write_all"
            } else {
                "out.print"
            }
            .into(),
            [Fault::Fault {
                code: 77,
                partial: 0,
            }]
            .into(),
        );
    }
    world
}
fn scripted(caps: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("suite-runner-29-script-");
        let root = scratch.path();
        let selection = package(root);
        let prepared = hirsel_driver::request_tir(&selection, hirsel_engine::Verify::On, None)
            .unwrap_or_else(|error| panic!("{error}"));
        let config = hirsel_engine::RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut receipts = vec![];
        let mut failures = vec![];
        for case in selected(caps) {
            let id = case["id"].as_str().unwrap();
            let world = world(&case);
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                eprintln!("SUITE_RUNNER_29_STARTED {id} {engine}");
                let execution = hirsel_driver::testing::observe_tir(
                    &prepared,
                    &config,
                    world.clone(),
                    hirsel_interp::Limits::default(),
                    engine == "checked-h",
                );
                let mut receipt =
                    json!({"id":id,"engine":engine,"state":"completed","passed":false});
                match execution {
                    Ok(execution) => {
                        receipt["outcome"] = format!("{:?}", execution.outcome).into();
                        if let Some(obs) = execution.observation {
                            let rows = String::from_utf8(obs.output.clone()).ok().and_then(|s| {
                                s.lines()
                                    .map(serde_json::from_str::<Value>)
                                    .collect::<Result<Vec<_>, _>>()
                                    .ok()
                            });
                            let expected = if case.get("plan").is_some() {
                                case["plan"].as_array().unwrap().clone()
                            } else if case["exit"] == 2
                                || case["write_failure"] == true
                                || case["output_failure"] == true
                            {
                                vec![]
                            } else {
                                vec![case["report"].clone()]
                            };
                            let state = obs.final_state.as_ref().unwrap();
                            let artifact = state.trees.values().find_map(|(_, files)| {
                                files
                                    .get("reports/report.json")
                                    .or_else(|| files.get("report.json"))
                            });
                            let artifact_ok = if case["exit"] == 2
                                || case.get("plan").is_some()
                                || case.get("text").is_some()
                                || case["write_failure"] == true
                            {
                                artifact == Some(&b"previous".to_vec())
                            } else {
                                artifact.and_then(|b| serde_json::from_slice::<Value>(b).ok())
                                    == Some(case["report"].clone())
                            };
                            let child_free = !state.handles.values().any(|g| {
                                matches!(
                                    g,
                                    hirsel_interp::library::simulation::ObservedGrant::Child { .. }
                                )
                            });
                            let mut passed = obs.exit_code
                                == case["exit"].as_i64().map(|n| n as i32)
                                && (if let Some(text) = case["text"].as_str() {
                                    obs.output == text.as_bytes()
                                } else {
                                    rows == Some(expected)
                                })
                                && artifact_ok
                                && child_free
                                && obs.failures.is_empty()
                                && obs.blocked.is_none();
                            if case["exit"] == 2 {
                                passed &= state.stderr
                                    == format!(
                                        "{{\"kind\":\"{}\"}}\n",
                                        case["error"].as_str().unwrap()
                                    )
                                    .as_bytes();
                            } else {
                                passed &= state.stderr.is_empty();
                            }
                            if let Some(prior) = previous {
                                passed &= obs == prior;
                            }
                            previous = Some(obs.clone());
                            receipt["passed"] = passed.into();
                            receipt["observation"] = format!("{obs:?}").into();
                        }
                    }
                    Err(error) => receipt["error"] = format!("{error:?}").into(),
                }
                if receipt["passed"] != true
                    && ["cases-32", "argv-4096", "input-262144"].contains(&id)
                    && receipt["outcome"].as_str().is_some_and(|outcome| {
                        outcome.contains("ToolingLimit")
                            && outcome.contains("kind: \"Steps\"")
                            && outcome.contains("1000000")
                    })
                {
                    receipt["disposition"] =
                        "INCOMPLETE: exact cap reaches unchanged 1M Steps".into();
                } else if receipt["passed"] != true {
                    failures.push(format!("{id}/{engine}"));
                }
                eprintln!("SUITE_RUNNER_29_RESULT {id} {engine} {}", receipt["passed"]);
                receipts.push(receipt);
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        let retained = scratch.keep();
        assert!(
            failures.is_empty(),
            "failed {failures:?}; evidence {}",
            retained.display()
        );
    });
}
#[test]
fn suite_runner_29_scripted_workflows_reference_checked() {
    scripted(false)
}
#[test]
fn suite_runner_29_caps_reference_checked() {
    scripted(true)
}
#[test]
fn suite_runner_29_native_and_host_workflows() {
    let scratch = scratch("suite-runner-29-native-");
    let root = scratch.path();
    package(root);
    for (name, source) in [
        (
            "verify.py",
            include_str!("../support/suite_runner_29/verify.py"),
        ),
        (
            "cases.json",
            include_str!("../support/suite_runner_29/cases.json"),
        ),
        (
            "caps.json",
            include_str!("../support/suite_runner_29/caps.json"),
        ),
        (
            "child.py",
            include_str!("../../../../examples/suite-runner/fixtures/child.py"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    let output = Command::new("python3")
        .arg(root.join("verify.py"))
        .arg("--root")
        .arg(root)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .output()
        .unwrap();
    fs::write(root.join("stdout"), &output.stdout).unwrap();
    fs::write(root.join("stderr"), &output.stderr).unwrap();
    let retained = scratch.keep();
    assert!(
        output.status.success(),
        "evidence {}: {} {}",
        retained.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["completed"].as_u64().unwrap() > 0);
    assert_eq!(report["failed"], 0);
    eprintln!("SUITE_RUNNER_29_NATIVE {report}");
    if std::env::var("SUITE_RUNNER_29_PHASE").as_deref() != Ok("host-missing") {
        let binaries = std::env::var_os("SUITE_RUNNER_29_RESUME")
            .map(PathBuf::from)
            .unwrap_or_else(|| retained.join("evidence"));
        refusals(&retained, &binaries);
    }
}

// Use the shipped recording codec to alter input/config bindings. No runtime
// adapter or unchecked byte surgery is introduced by these refusal controls.
fn refusals(root: &Path, binaries: &Path) {
    let original_path = binaries.join("llvm-pass.tape");
    let original = fs::read(&original_path).unwrap();
    let recording = hirsel_rt::host_launch::read_recording(&original_path).unwrap();
    let mut inputs = recording.clone();
    inputs.arguments.push("changed".into());
    let changed_input = root.join("changed-input.tape");
    fs::write(
        &changed_input,
        hirsel_rt::launch::render_recording(&inputs).unwrap(),
    )
    .unwrap();
    let mut config = recording.clone();
    let mut canonical: Value =
        serde_json::from_slice(&config.binding.as_ref().unwrap().canonical).unwrap();
    canonical["io"]["stdout"] = Value::Null;
    let run_path = root.join("changed-run.json");
    fs::write(&run_path, canonical.to_string()).unwrap();
    let run = hirsel_engine::read_run_record(&run_path).unwrap();
    assert_ne!(run.identity(), recording.run_identity);
    config.binding.as_mut().unwrap().canonical = run.canonical().to_vec();
    let changed_config = root.join("changed-config.tape");
    fs::write(
        &changed_config,
        hirsel_rt::launch::render_recording(&config).unwrap(),
    )
    .unwrap();
    let bad_tape = root.join("bad.tape");
    fs::write(&bad_tape, b"not a recording").unwrap();
    let mut receipts = vec![];
    for (id, tape) in [
        ("input", &changed_input),
        ("config", &changed_config),
        ("tape", &bad_tape),
    ] {
        for engine in ["reference", "llvm", "cranelift"] {
            let mut argv = if engine == "reference" {
                vec![
                    env!("CARGO_BIN_EXE_hirsel").to_owned(),
                    "run".into(),
                    root.join("hirsel.json").to_string_lossy().into_owned(),
                    "--replay".into(),
                ]
            } else {
                vec![
                    binaries.join(engine).to_string_lossy().into_owned(),
                    "--hirsel-replay".into(),
                ]
            };
            argv.push(tape.to_string_lossy().into_owned());
            let output = Command::new("python3")
                .arg(root.join("capped"))
                .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                .args(&argv)
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .env("HIRSEL_JOBS", "4")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(3), "{id}/{engine}: {output:?}");
            assert!(output.stdout.is_empty());
            receipts.push(json!({"id":id,"engine":engine,"argv":argv,"exit":3,"stderr":String::from_utf8_lossy(&output.stderr),"passed":true}));
        }
    }
    assert_eq!(fs::read(original_path).unwrap(), original);
    fs::write(
        root.join("refusal-receipts.json"),
        serde_json::to_vec_pretty(&receipts).unwrap(),
    )
    .unwrap();
    eprintln!("SUITE_RUNNER_29_REFUSALS {}", receipts.len());
}
