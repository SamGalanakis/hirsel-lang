//! Full main, independent solve/certificate validation, public fixtures and immutable tapes.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_run_schema::{Access, FileReadScript};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SOURCES: [(&str, &str); 6] = [
    (
        "main",
        include_str!("../../../../examples/flowaudit/main.hir"),
    ),
    (
        "model",
        include_str!("../../../../examples/flowaudit/model.hir"),
    ),
    (
        "residual",
        include_str!("../../../../examples/flowaudit/residual.hir"),
    ),
    (
        "solve",
        include_str!("../../../../examples/flowaudit/solve.hir"),
    ),
    (
        "certificate",
        include_str!("../../../../examples/flowaudit/certificate.hir"),
    ),
    (
        "report",
        include_str!("../../../../examples/flowaudit/report.hir"),
    ),
];
fn cases() -> Vec<Value> {
    {
        let mut rows: Vec<Value> =
            serde_json::from_str(include_str!("../support/flow_audit_36/cases.json")).unwrap();
        rows.extend(
            serde_json::from_str::<Vec<Value>>(include_str!(
                "../support/flow_audit_36/verify-cases.json"
            ))
            .unwrap(),
        );
        rows.extend(
            serde_json::from_str::<Vec<Value>>(include_str!(
                "../support/flow_audit_36/logical-jobs-cases.json"
            ))
            .unwrap(),
        );
        rows.extend(
            serde_json::from_str::<Vec<Value>>(include_str!(
                "../support/flow_audit_36/validation-cases.json"
            ))
            .unwrap(),
        );
        rows
    }
}
// Permanent deterministic cap expectations are independent of evaluator Stops.
fn cap_oracle(row: &Value) -> Option<Value> {
    let declarations: Value =
        serde_json::from_str(include_str!("../support/flow_audit_36/cap-oracles.json")).unwrap();
    let declaration = declarations.get(row["id"].as_str().unwrap())?;
    let file = if declaration["file"] == "@supplied-report" {
        row["report"].as_str().unwrap()
    } else {
        declaration["file"].as_str().unwrap()
    };
    Some(
        json!({"exit":declaration["exit"],"stdout":declaration["stdout"].as_str().unwrap().as_bytes(),"stderr":declaration["stderr"].as_str().unwrap().as_bytes(),"file":file.as_bytes()}),
    )
}
fn selected(row: &Value) -> bool {
    std::env::var("FLOW_AUDIT_36_CASE").map_or(true, |v| v.split(',').any(|s| row["id"] == s))
}
fn scratch(label: &str) -> PathBuf {
    // Host execution is explicit: this archive must survive Buck's scratch cleanup.
    let base = std::env::var_os("FLOW_AUDIT_36_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("flow-audit-36-cells"));
    fs::create_dir_all(&base).unwrap();
    let p = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep();
    eprintln!("FLOW_ARCHIVE {}", p.display());
    p
}
struct Evidence(PathBuf);
impl Drop for Evidence {
    fn drop(&mut self) {
        let output = Command::new("python3")
            .arg("-c")
            .arg(include_str!("../support/flow_audit_36/export.py"))
            .arg(&self.0)
            .output()
            .expect("archive transport");
        eprint!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "evidence archive failed");
    }
}
fn package(root: &Path) -> PathBuf {
    package_entry(root, SOURCES[0].1)
}
fn package_entry(root: &Path, entry: &str) -> PathBuf {
    for (name, src) in SOURCES {
        fs::write(root.join(format!("{name}.hir")), src).unwrap();
    }
    fs::write(root.join("main.hir"), entry).unwrap();
    if std::env::var_os("FLOW_AUDIT_36_ORIGINAL_NOALLOC").is_some() {
        fs::write(
            root.join("residual.hir"),
            include_str!("../support/flow_audit_36/residual-original.hir"),
        )
        .unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/flowaudit/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let p = root.join("hirsel.json");
    fs::write(&p, manifest.to_string()).unwrap();
    fs::write(
        root.join("capped"),
        include_bytes!("../../../../tools/capped"),
    )
    .unwrap();
    fs::write(
        root.join("run_capped.py"),
        include_bytes!("../support/flow_audit_36/run_capped.py"),
    )
    .unwrap();
    fs::write(
        root.join("cases.json"),
        include_bytes!("../support/flow_audit_36/cases.json"),
    )
    .unwrap();
    fs::write(
        root.join("seal.py"),
        include_bytes!("../support/flow_audit_36/seal.py"),
    )
    .unwrap();
    let sealed = Command::new("python3")
        .arg(root.join("seal.py"))
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .arg(root.join("closure"))
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(
        sealed.status.success(),
        "seal: {}",
        String::from_utf8_lossy(&sealed.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&sealed.stdout));
    p
}
fn fixture(row: &Value, jobs: u64) -> Fixture {
    let mut args = vec![
        row["command"].as_str().unwrap_or("audit").into(),
        "data".into(),
        "batch.json".into(),
        "report.json".into(),
        row["cli_jobs"].as_u64().unwrap_or(jobs).to_string(),
    ];
    if row["pending"] == true {
        args.push("--interruptible".into());
    }
    Fixture {
        args,
        files: BTreeMap::from([
            (
                "data/batch.json".into(),
                row["input"].as_str().unwrap().as_bytes().to_vec(),
            ),
            (
                "data/report.json".into(),
                row["report"]
                    .as_str()
                    .unwrap_or("preserve")
                    .as_bytes()
                    .to_vec(),
            ),
        ]),
        mounts: BTreeMap::from([(
            "data".into(),
            if row["access"] == "read" {
                Access::Read
            } else {
                Access::ReadWrite
            },
        )]),
        file_faults: row
            .get("file_faults")
            .map(|v| {
                let mut faults = v.clone();
                if let Some(steps) = faults["file.commit"].as_array_mut() {
                    for step in steps {
                        if step["error"] == "permission_denied" {
                            step["error"] = json!("PermissionDenied");
                        }
                    }
                }
                serde_json::from_value(faults).unwrap()
            })
            .unwrap_or_default(),
        file_reads: if row["pending"] == true {
            BTreeMap::from([("data/batch.json".into(), FileReadScript::Pending)])
        } else {
            BTreeMap::new()
        },
        signals: if row["pending"] == true {
            Some(serde_json::from_value(json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]})).unwrap())
        } else {
            None
        },
        ..Default::default()
    }
}
// Exhaustive cuts: no path search, no residual mutation, no NetworkX dependency.
fn exhaustive(m: &Value, capacities: &[u64]) -> u64 {
    let vertices = m["vertices"].as_array().unwrap();
    let index = |v: &Value| vertices.iter().position(|x| x == v).unwrap();
    let source = index(&m["source"]);
    let sink = index(&m["sink"]);
    (0..(1u64 << vertices.len()))
        .filter(|mask| mask & (1 << source) != 0 && mask & (1 << sink) == 0)
        .map(|mask| {
            m["arcs"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    mask & (1 << index(&e["source"])) != 0 && mask & (1 << index(&e["target"])) == 0
                })
                .map(|(i, _)| capacities[i])
                .sum()
        })
        .min()
        .unwrap()
}
fn validate_certificate(m: &Value, scenario: &Value, c: &Value) {
    let vertices = m["vertices"].as_array().unwrap();
    let arcs = m["arcs"].as_array().unwrap();
    let index = |v: &Value| vertices.iter().position(|x| x == v).unwrap();
    let mut capacities: Vec<u64> = arcs
        .iter()
        .map(|a| a["capacity"].as_u64().unwrap())
        .collect();
    for o in scenario["overrides"].as_array().unwrap() {
        let i = arcs.iter().position(|a| a["name"] == o["arc"]).unwrap();
        capacities[i] = o["capacity"].as_u64().unwrap();
    }
    assert_eq!(c["name"], scenario["name"]);
    assert_eq!(c["capacities"], json!(capacities));
    let flows: Vec<u64> = serde_json::from_value(c["flows"].clone()).unwrap();
    assert_eq!(flows.len(), arcs.len());
    let mut balance = vec![0i64; vertices.len()];
    for (i, e) in arcs.iter().enumerate() {
        assert!(flows[i] <= capacities[i]);
        balance[index(&e["source"])] -= flows[i] as i64;
        balance[index(&e["target"])] += flows[i] as i64;
    }
    let value = c["value"].as_u64().unwrap();
    let source = index(&m["source"]);
    let sink = index(&m["sink"]);
    for (u, b) in balance.iter().enumerate() {
        assert_eq!(
            *b,
            if u == source {
                -(value as i64)
            } else if u == sink {
                value as i64
            } else {
                0
            }
        );
    }
    let reached: Vec<bool> = serde_json::from_value(c["reachable"].clone()).unwrap();
    assert_eq!(reached.len(), vertices.len());
    assert!(reached[source] && !reached[sink]);
    let mut actual = vec![false; vertices.len()];
    actual[source] = true;
    loop {
        let previous = actual.clone();
        for (i, e) in arcs.iter().enumerate() {
            let u = index(&e["source"]);
            let v = index(&e["target"]);
            if actual[u] && flows[i] < capacities[i] {
                actual[v] = true;
            }
            if actual[v] && flows[i] > 0 {
                actual[u] = true;
            }
        }
        if previous == actual {
            break;
        }
    }
    assert_eq!(actual, reached);
    let cut: Vec<_> = arcs
        .iter()
        .enumerate()
        .filter(|(_, e)| reached[index(&e["source"])] && !reached[index(&e["target"])])
        .map(|(i, _)| i)
        .collect();
    assert_eq!(c["cut_arcs"], json!(cut));
    let capacity: u64 = cut.iter().map(|i| capacities[*i]).sum();
    assert_eq!(c["cut_capacity"], capacity);
    assert_eq!(capacity, value);
    assert_eq!(value, exhaustive(m, &capacities));
}
fn verify(row: &Value, o: &hirsel_interp::observation::Observable, world: &Fixture) -> Vec<u8> {
    assert!(o.failures.is_empty(), "{}: {:?}", row["id"], o.failures);
    assert!(o.blocked.is_none());
    let state = o.final_state.as_ref().unwrap();
    let files = &state.trees[&0].1;
    let mut expected = world.files.clone();
    if let Some(error) = row["error"].as_str() {
        assert_eq!(o.exit_code, Some(2));
        assert!(o.output.is_empty());
        assert_eq!(state.stderr, format!("flowaudit: {error}\n").as_bytes());
        assert_eq!(files, &expected, "error mutated files or leaked temporary");
        return files["data/report.json"].clone();
    }
    assert_eq!(o.exit_code, Some(0));
    assert!(state.stderr.is_empty());
    let m: Value = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
    let scenarios = m["scenarios"].as_array().unwrap();
    assert_eq!(
        o.output,
        format!("flowaudit: {} scenarios\n", scenarios.len()).as_bytes()
    );
    let bytes = files["data/report.json"].clone();
    expected.insert("data/report.json".into(), bytes.clone());
    assert_eq!(files, &expected);
    assert!(bytes.len() <= 32768);
    if row["command"] == "verify" {
        assert_eq!(bytes, world.files["data/report.json"]);
    }
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], 1);
    let results = doc["results"].as_array().unwrap();
    assert_eq!(results.len(), scenarios.len());
    for (i, (scenario, c)) in scenarios.iter().zip(results).enumerate() {
        validate_certificate(&m, scenario, c);
        assert_eq!(c["value"], row["optima"][i]);
    }
    bytes
}
#[test]
fn flow_audit_36_reference_checked_workflows() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("simulation-");
        let _evidence = Evidence(root.clone());
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut receipts = vec![];
        let mut failures = vec![];
        for row in cases().iter().filter(|r| selected(r)) {
            let world = fixture(row, 4);
            let mut reference = None;
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "reference" };
                if std::env::var("FLOW_AUDIT_36_ENGINE")
                    .is_ok_and(|v| !v.split(',').any(|e| e == engine))
                {
                    continue;
                }
                let id = row["id"].as_str().unwrap();
                eprintln!("FLOW_STARTED {id} {engine}");
                let outcome = if checked {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        SimulationConfig::from(&world),
                        1_000_000,
                    )
                } else {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        SimulationConfig::from(&world),
                        1_000_000,
                    )
                };
                let mut receipt = json!({"id":id,"engine":engine,"selected":true,"started":true,"completed":false,"passed":false});
                match outcome {
                    Ok(execution) => {
                        receipt["outcome"] = json!(format!("{:?}", execution.outcome));
                        receipt["consumed"] = json!(execution.step_usage.map(|u| u.consumed));
                        if let Some(observed) = execution.observation {
                            let validation =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    verify(row, &observed, &world)
                                }));
                            if validation.is_err() {
                                failures.push(format!("{id}/{engine}: oracle mismatch"));
                                receipt["error"] = json!("oracle mismatch");
                                fs::write(
                                    root.join(format!("{id}-{engine}-failed-observation.txt")),
                                    format!("{observed:#?}"),
                                )
                                .unwrap();
                                receipts.push(receipt);
                                fs::write(
                                    root.join("receipts.json"),
                                    serde_json::to_vec_pretty(&receipts).unwrap(),
                                )
                                .unwrap();
                                continue;
                            }
                            if row["pending"] == true {
                                assert!(observed.effects.iter().any(|e| matches!(&e.effect, hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation == "file.read.pending")));
                            }
                            if let Some(previous) = &reference {
                                assert_eq!(&observed, previous);
                            }
                            fs::write(
                                root.join(format!("{id}-{engine}-observation.txt")),
                                format!("{observed:#?}"),
                            )
                            .unwrap();
                            fs::write(root.join(format!("{id}-oracle.json")), serde_json::to_vec_pretty(&json!({"exit":observed.exit_code,"stdout":observed.output,"stderr":observed.final_state.as_ref().unwrap().stderr,"file":verify(row, &observed, &world)})).unwrap()).unwrap();
                            reference = Some(observed);
                            receipt["completed"] = json!(true);
                            receipt["passed"] = json!(true);
                        } else {
                            if matches!(
                                &execution.outcome,
                                Ok(hirsel_engine::RunOutcome::ToolingLimit(_))
                            ) {
                                assert!(
                                    matches!(&execution.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == 1_000_000)
                                );
                                failures.push(format!("{id}/{engine}: INCOMPLETE Steps1M"));
                                receipt["disposition"] = json!(
                                    "INCOMPLETE: original Steps1M; never a completed app cell"
                                );
                            } else {
                                failures.push(format!("{id}/{engine}: {:?}", execution.outcome));
                            }
                        }
                    }
                    Err(error) => {
                        receipt["error"] = json!(error.to_string());
                        failures.push(format!("{id}/{engine}: {error}"));
                    }
                }
                eprintln!("FLOW_RECEIPT {receipt}");
                receipts.push(receipt);
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        assert!(!receipts.is_empty());
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[test]
fn flow_audit_36_native_four_engine_replay() {
    hirsel_engine::with_compiler_stack(|| {
        // Reuse compilation for this immutable package; each tape gets a fresh runtime.
        hirsel_driver::session::enable_resident(1);
        let root = scratch("native-");
        let _evidence = Evidence(root.clone());
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![Configuration {
            options: options.clone(),
            engine: Engine::Heap { campaign: false },
        }];
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.wrapper = vec![
            "python3".into(),
            root.join("run_capped.py").to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
        ];
        tools.launch_timeout = std::time::Duration::from_secs(65);
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let mut objects = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
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
        let mut receipts = vec![];
        let mut failed = vec![];
        for row in cases().iter().filter(|r| selected(r)) {
            let id = row["id"].as_str().unwrap();
            let world = fixture(row, 4);
            let cell = root.join(id);
            fs::create_dir(&cell).unwrap();
            fs::write(cell.join("fixture.json"),serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"file_faults":world.file_faults,"signals":world.signals})).unwrap()).unwrap();
            // Reuse accepted semantic oracles explicitly; a baseline is expected
            // data, never a claim that a new engine or replay cell ran.
            let baseline: Option<Value> = std::env::var("FLOW_AUDIT_36_BASELINE_JSON")
                .ok()
                .map(|s| serde_json::from_str(&s).unwrap());
            let golden = baseline
                .as_ref()
                .and_then(|b| b.get(id))
                .cloned()
                .or_else(|| cap_oracle(row));
            let (expected, stderr, output) = if let Some(b) = golden.as_ref() {
                fs::write(
                    cell.join("baseline.json"),
                    serde_json::to_vec_pretty(b).unwrap(),
                )
                .unwrap();
                (
                    Observation {
                        exit_code: b["exit"].as_i64().map(|x| x as i32),
                        blocked: false,
                        budget: None,
                        signaled: None,
                        output: serde_json::from_value(b["stdout"].clone()).unwrap(),
                        failures: vec![],
                        roots: vec![],
                    },
                    serde_json::from_value::<Vec<u8>>(b["stderr"].clone()).unwrap(),
                    serde_json::from_value::<Vec<u8>>(b["file"].clone()).unwrap(),
                )
            } else {
                let reference = hirsel_engine::run_observed_snapshot_bounded(
                    &mut loaded.engine,
                    &run,
                    (&world).into(),
                    1_000_000,
                )
                .unwrap();
                let Some(full) = reference.observation else {
                    failed.push(format!("{id}: reference {:?}", reference.outcome));
                    receipts.push(json!({"id":id,"selected":true,"started":true,"completed":false,"outcome":format!("{:?}",reference.outcome)}));
                    continue;
                };
                let output = verify(row, &full, &world);
                (
                    Observation::of_reference(&full),
                    full.final_state.as_ref().unwrap().stderr.clone(),
                    output,
                )
            };
            let mut runnables = vec![];
            for (backend, emitted) in &objects {
                let dir = cell.join(format!("{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, emitted, &tools, &dir, &world)
                        .unwrap();
                runnables.push((runnable, dir));
            }
            if ["classic", "malformed", "denial", "pending"].contains(&id) {
                for (name, outcome) in differential_in(
                    &input,
                    &loaded.entry,
                    &expected,
                    &matrix,
                    &tools,
                    &cell,
                    &world,
                ) {
                    assert_eq!(outcome, Outcome::Agree, "{id}/{name}");
                }
            }
            for (runnable, dir) in &runnables {
                eprintln!("FLOW_STARTED {id} {}", dir.display());
                let tape = dir.join("recording");
                let live = launch_fixture(
                    runnable,
                    &ready.target,
                    &tools,
                    dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    &world,
                )
                .unwrap();
                fs::write(
                    dir.join("live.json"),
                    serde_json::to_vec_pretty(
                        &json!({"stdout":live.stdout,"stderr":live.stderr,"exit":live.exit}),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(observe(&live, &ready, &sites).unwrap(), expected);
                assert_eq!(live.stderr, stderr);
                assert_eq!(live.stdout, expected.output);
                let frozen = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                assert_eq!(record.arguments, world.args);
                record.bound_run().unwrap();
                let writes: Vec<_> = record
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                if row["command"] == "verify" {
                    assert!(writes.is_empty());
                } else if row["error"].is_null() || id == "commit-failure" {
                    if id != "commit-failure" {
                        assert_eq!(writes, output);
                    }
                    assert_eq!(
                        record
                            .events
                            .iter()
                            .filter(|e| matches!(e.request, hirsel_rt::io::Request::Commit { .. }))
                            .count(),
                        1
                    );
                } else {
                    assert!(writes.is_empty());
                }
                if row["pending"] == true {
                    assert!(record.events.iter().any(|e| matches!(
                        (&e.request, &e.completion),
                        (
                            hirsel_rt::io::Request::FileRead { .. },
                            hirsel_rt::io::Completion::Blocked { committed: false }
                        )
                    )));
                }
                let offline = Fixture {
                    files: Default::default(),
                    file_reads: Default::default(),
                    file_faults: Default::default(),
                    signals: None,
                    ..world.clone()
                };
                for (player, replay_dir) in &runnables {
                    let replay = launch_fixture(
                        player,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &offline,
                    )
                    .unwrap();
                    assert_eq!(observe(&replay, &ready, &sites).unwrap(), expected);
                    assert_eq!(replay.stderr, stderr);
                    assert!(
                        fs::read_dir(replay_dir.join("root"))
                            .unwrap()
                            .next()
                            .is_none()
                    );
                }
                let mut interpreter_replays = 0;
                for (index, replay) in [
                    hirsel_driver::replay_observations,
                    hirsel_driver::replay_observations_checked,
                ]
                .into_iter()
                .enumerate()
                {
                    let engine = if index == 0 { "reference" } else { "checked-h" };
                    match replay(&path, &tape, Some(1_000_000)) {
                        Ok(value) => {
                            assert_eq!(Observation::of_reference(&value), expected);
                            interpreter_replays += 1;
                        }
                        Err(error) => {
                            let message = error.to_string();
                            fs::write(dir.join(format!("{engine}-replay-error.txt")), &message)
                                .unwrap();
                            assert!(
                                [
                                    "vertices-arcs-cap",
                                    "overrides-cap",
                                    "overrides-over",
                                    "input-cap",
                                    "report-cap"
                                ]
                                .contains(&id)
                                    && message.contains("ToolingLimit")
                                    && message.contains("Steps")
                                    && message.contains("1000000"),
                                "{id}/{engine}: {message}"
                            );
                            failed.push(format!(
                                "{id}/{engine}/{}: INCOMPLETE Steps1M",
                                dir.file_name().unwrap().to_string_lossy()
                            ));
                        }
                    }
                }
                if id == "classic" && dir.ends_with("Llvm") {
                    let root = &cell;
                    let mut controls_failed = false;
                    for kind in ["binding", "request", "missing", "extra"] {
                        if std::env::var("FLOW_AUDIT_36_COMPLETED_CONTROLS")
                            .unwrap_or_default()
                            .split(',')
                            .any(|k| k == kind)
                        {
                            eprintln!("FLOW_REUSED_CONTROL {kind}");
                            continue;
                        }
                        let control_result =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let mut changed = record.clone();
                                let refusal_expected = match kind {
                                    "binding" => {
                                        changed.arguments.push("different".into());
                                        "header disagrees"
                                    }
                                    "request" => {
                                        let request = changed
                                            .events
                                            .iter_mut()
                                            .find_map(|e| match &mut e.request {
                                                hirsel_rt::io::Request::Directory {
                                                    path, ..
                                                } => Some(path),
                                                _ => None,
                                            })
                                            .unwrap();
                                        request.push_str("-different");
                                        "ReplayMismatch"
                                    }
                                    "missing" => {
                                        changed.events.pop().unwrap();
                                        "MissingInput"
                                    }
                                    _ => {
                                        changed.events.push(record.events[0].clone());
                                        "UnconsumedInput"
                                    }
                                };
                                let bad = cell.join(format!("refused-{kind}.tape"));
                                fs::write(
                                    &bad,
                                    hirsel_rt::launch::render_recording(&changed).unwrap(),
                                )
                                .unwrap();
                                for replay in [
                                    hirsel_driver::replay_observations,
                                    hirsel_driver::replay_observations_checked,
                                ] {
                                    let error = replay(&path, &bad, Some(1_000_000))
                                        .unwrap_err()
                                        .to_string();
                                    assert!(error.contains(refusal_expected), "{kind}: {error}");
                                }
                                for (player, replay_dir) in &runnables {
                                    let before: BTreeSet<_> = fs::read_dir(&root)
                                        .unwrap()
                                        .map(|e| e.unwrap().path())
                                        .filter(|p| {
                                            p.file_name()
                                                .unwrap()
                                                .to_string_lossy()
                                                .starts_with("launch-")
                                        })
                                        .collect();
                                    match launch_fixture(
                                        player,
                                        &ready.target,
                                        &tools,
                                        replay_dir,
                                        true,
                                        Tape::Replay(&bad),
                                        &offline,
                                    ) {
                                        Err(error) => {
                                            assert!(
                                                error.message.contains(refusal_expected),
                                                "{kind}: {error}"
                                            );
                                            let after: BTreeSet<_> = fs::read_dir(&root)
                                                .unwrap()
                                                .map(|e| e.unwrap().path())
                                                .filter(|p| {
                                                    p.file_name()
                                                        .unwrap()
                                                        .to_string_lossy()
                                                        .starts_with("launch-")
                                                })
                                                .collect();
                                            let created: Vec<_> =
                                                after.difference(&before).collect();
                                            if kind == "binding" {
                                                assert!(created.is_empty());
                                            } else {
                                                assert_eq!(
                                                    created.len(),
                                                    1,
                                                    "refusal must retain exact captured output"
                                                );
                                                let stdout =
                                                    fs::read(created[0].join("stdout")).unwrap();
                                                let prefix: &[u8] = if kind == "extra" {
                                                    &expected.output
                                                } else {
                                                    b""
                                                };
                                                assert_eq!(
                                                    stdout, prefix,
                                                    "{kind}: refused stdout prefix"
                                                );
                                            }
                                        }
                                        Ok(refused) => {
                                            assert_eq!(
                                                refused.exit,
                                                Some(3),
                                                "{kind}: {}",
                                                refused.record
                                            );
                                            assert!(
                                                String::from_utf8_lossy(&refused.stderr)
                                                    .contains(refusal_expected)
                                                    || refused.record.contains(refusal_expected)
                                                    || fs::read_to_string(
                                                        replay_dir.join("refusal")
                                                    )
                                                    .unwrap_or_default()
                                                    .contains(refusal_expected),
                                                "{kind}: {} {:?}",
                                                refused.record,
                                                refused.stderr
                                            );
                                            // A late refusal may have printed only the exact
                                            // valid prefix, never arbitrary output.
                                            let prefix: &[u8] = if kind == "extra" {
                                                &expected.output
                                            } else {
                                                b""
                                            };
                                            assert_eq!(refused.stdout, prefix, "{kind}");
                                        }
                                    }
                                }
                            }));
                        if control_result.is_ok() {
                            eprintln!("FLOW_REFUSAL_PASS {kind} four engines");
                        } else {
                            controls_failed = true;
                            eprintln!(
                                "FLOW_REFUSAL_FAIL {kind}; retained; continuing independent workflows"
                            );
                        }
                    }

                    assert!(!controls_failed);
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                receipts.push(json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"selected":true,"started":true,"completed":interpreter_replays==2,"passed":interpreter_replays==2,"live_passed":true,"native_replays":2,"interpreter_replays":interpreter_replays,"tape":tape,"expected_provenance":if ["vertices-arcs-cap","overrides-cap","overrides-over","input-cap","report-cap"].contains(&id){"independent external expected data; never interpreter completion"}else{"accepted semantic oracle"}}));
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        fs::write(
            root.join("receipts.json"),
            serde_json::to_vec_pretty(&receipts).unwrap(),
        )
        .unwrap();
        assert!(!receipts.is_empty());
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    });
}

#[test]
#[ignore = "explicit exact-CPU bounded matrix; FLOW_AUDIT_36_ORDERED=all or backend:width:kind"]
fn flow_audit_36_ordered_capacity() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("ordered-");
        let _evidence = Evidence(root.clone());
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        }];
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.execution = Execution::Executable;
        tools.wrapper = vec![
            "python3".into(),
            root.join("run_capped.py").to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
            "--inherit-exact-cpus".into(),
        ];
        tools.launch_timeout = std::time::Duration::from_secs(65);
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let mut objects = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
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
        let selection = std::env::var("FLOW_AUDIT_36_ORDERED").unwrap_or_else(|_| "all".into());
        let mut cells = vec![];
        let rows = cases();
        for (backend, emitted) in &objects {
            let name = if *backend == Backend::Llvm {
                "llvm"
            } else {
                "cranelift"
            };
            for width in [1u64, 2, 4] {
                for kind in ["success", "domain", "cancel"] {
                    let id = format!("{name}:{width}:{kind}");
                    let row = rows
                        .iter()
                        .find(|r| {
                            r["id"]
                                == if kind == "success" {
                                    "four-scenarios"
                                } else if kind == "domain" {
                                    "malformed"
                                } else {
                                    "pending"
                                }
                        })
                        .unwrap();
                    let world = fixture(row, 4);
                    let dir = root.join(format!("{name}-{width}-{kind}"));
                    fs::create_dir(&dir).unwrap();
                    let control_dir = tempfile::tempdir().unwrap();
                    let control = control_dir.path().join("ctl");
                    link_fixture_runnable(&ready, &loaded.entry, emitted, &tools, &dir, &world)
                        .unwrap();
                    let bound =
                        hirsel_run_config::RunConfig::read_path(&dir.join("run.json")).unwrap();
                    let mut record = bound.record().clone();
                    record.placement=Some(hirsel_run_config::Placement::from_json(&serde_json::to_vec(&json!({"cores":width,"commit":"shared","reorder":"off","control":control})).unwrap()).unwrap());
                    let requested = record.validated().unwrap();
                    let runnable = link_runnable(
                        &ready,
                        &loaded.entry,
                        emitted,
                        &tools,
                        &dir,
                        Some(&requested),
                    )
                    .unwrap();
                    fs::write(dir.join("fixture.json"),serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"signals":world.signals})).unwrap()).unwrap();
                    cells.push((
                        id,
                        width,
                        kind.to_owned(),
                        runnable,
                        dir,
                        world,
                        control_dir,
                    ));
                }
            }
        }
        assert!(!cells.is_empty());
        let mut receipts = vec![];
        let mut failures = vec![];
        for (id, width, kind, runnable, dir, world, control_dir) in &cells {
            if selection != "all" && !selection.split(',').any(|s| s == id) {
                continue;
            }
            eprintln!("FLOW_ORDERED_STARTED {id}");
            receipts.push(
                json!({"id":id,"selected":true,"started":true,"completed":false,"passed":false}),
            );
            fs::write(
                root.join("receipts.json"),
                serde_json::to_vec_pretty(&receipts).unwrap(),
            )
            .unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let tape = dir.join("recording");
                let control = control_dir.path().join("ctl");
                let mut replies = vec![];
                let launched = std::thread::scope(|scope| {
                    let child = scope.spawn(|| {
                        launch_fixture(
                            runnable,
                            &ready.target,
                            &tools,
                            dir,
                            true,
                            Tape::Live {
                                record: Some(&tape),
                                revocations: None,
                            },
                            world,
                        )
                    });
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !child.is_finished() && std::time::Instant::now() < deadline {
                        if let Ok(mut connection) = UnixStream::connect(&control) {
                            connection
                                .set_read_timeout(Some(std::time::Duration::from_millis(100)))
                                .unwrap();
                            if connection.write_all(b"{\"metrics\":true}\n").is_ok() {
                                let mut line = String::new();
                                if BufReader::new(connection).read_line(&mut line).is_ok()
                                    && let Ok(value) = serde_json::from_str::<Value>(&line)
                                {
                                    replies.push(value);
                                }
                            }
                        }
                        if replies.iter().any(|m| {
                            m["worker_phase"] == "running"
                                && m["width"]["requested"] == *width
                                && m["width"]["effective"] == *width
                                && m["width"]["pending"] == 0
                        }) {
                            break;
                        }
                        std::thread::yield_now();
                    }
                    child.join().unwrap()
                });
                let capacity = replies.iter().any(|m| {
                    m["worker_phase"] == "running"
                        && m["width"]["requested"] == *width
                        && m["width"]["effective"] == *width
                        && m["width"]["pending"] == 0
                });
                fs::write(
                    dir.join("capacity.json"),
                    serde_json::to_vec_pretty(
                        &json!({"id":id,"replies":replies,"capacity":capacity}),
                    )
                    .unwrap(),
                )
                .unwrap();
                let launched = launched.unwrap();
                fs::write(dir.join("live.json"),serde_json::to_vec_pretty(&json!({"stdout":launched.stdout,"stderr":launched.stderr,"exit":launched.exit})).unwrap()).unwrap();
                let observed = observe(&launched, &ready, &sites).unwrap();
                let frozen = fs::read(&tape).unwrap();
                let recording = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                assert_eq!(
                    observed.exit_code,
                    Some(if kind == "success" { 0 } else { 2 })
                );
                assert!(observed.failures.is_empty());
                assert_eq!(
                    launched.stdout,
                    if kind == "success" {
                        b"flowaudit: 4 scenarios\n".to_vec()
                    } else {
                        vec![]
                    }
                );
                assert_eq!(
                    launched.stderr,
                    if kind == "success" {
                        vec![]
                    } else if kind == "domain" {
                        b"flowaudit: json\n".to_vec()
                    } else {
                        b"flowaudit: canceled\n".to_vec()
                    }
                );
                assert_eq!(recording.arguments, world.args);
                recording.bound_run().unwrap();
                if kind == "success" {
                    let bytes: Vec<u8> = recording
                        .events
                        .iter()
                        .filter_map(|e| match &e.request {
                            hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes.clone()),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    let doc: Value = serde_json::from_slice(&bytes).unwrap();
                    let m: Value = serde_json::from_slice(&world.files["data/batch.json"]).unwrap();
                    assert_eq!(doc["results"].as_array().unwrap().len(), 4);
                    for (s, w) in m["scenarios"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .zip(doc["results"].as_array().unwrap())
                    {
                        validate_certificate(&m, s, w);
                    }
                }
                if kind == "cancel" {
                    assert!(recording.events.iter().any(|e| matches!(
                        (&e.request, &e.completion),
                        (
                            hirsel_rt::io::Request::FileRead { .. },
                            hirsel_rt::io::Completion::Blocked { committed: false }
                        )
                    )));
                }
                let mut semantic = recording.clone();
                let binding = semantic.binding.as_mut().unwrap();
                binding.admitted_at = 0;
                binding.placement = None;
                fs::write(
                    dir.join("semantic.tape"),
                    hirsel_rt::launch::render_recording(&semantic).unwrap(),
                )
                .unwrap();
                let offline = Fixture {
                    files: Default::default(),
                    file_reads: Default::default(),
                    file_faults: Default::default(),
                    signals: None,
                    ..world.clone()
                };
                let mut replays = 0;
                for (_, player_width, player_kind, player, player_dir, _, _) in &cells {
                    if player_width != width || player_kind != kind {
                        continue;
                    }
                    let replay = launch_fixture(
                        player,
                        &ready.target,
                        &tools,
                        player_dir,
                        true,
                        Tape::Replay(&tape),
                        &offline,
                    )
                    .unwrap();
                    assert_eq!(observe(&replay, &ready, &sites).unwrap(), observed);
                    assert_eq!(replay.stderr, launched.stderr);
                    assert!(
                        fs::read_dir(player_dir.join("root"))
                            .unwrap()
                            .next()
                            .is_none()
                    );
                    replays += 1;
                }
                assert!(replays >= 1);
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                assert!(!control.exists(), "control socket leaked");
                assert!(
                    capacity,
                    "INCOMPLETE: exact running/effective=requested/pending0 reply missing for {id}"
                );
                json!({"id":id,"selected":true,"started":true,"completed":true,"passed":true,"capacity":true,"native_replays":replays,"tape":tape})
            }));
            let receipt = match result {
                Ok(v) => v,
                Err(_) => {
                    failures.push(id.clone());
                    json!({"id":id,"selected":true,"started":true,"completed":false,"passed":false,"disposition":"INCOMPLETE/failed; retained raw cell"})
                }
            };
            *receipts.last_mut().unwrap() = receipt;
            fs::write(
                root.join("receipts.json"),
                serde_json::to_vec_pretty(&receipts).unwrap(),
            )
            .unwrap();
        }
        // Scripted fixtures have identical inputs. Compare every semantic byte;
        // only documented binding admission/placement metadata was removed.
        for kind in ["success", "domain", "cancel"] {
            let mut expected = None;
            for (_, _, k, _, dir, _, _) in &cells {
                if k != kind {
                    continue;
                }
                let path = dir.join("semantic.tape");
                if !path.exists() {
                    continue;
                }
                let bytes = fs::read(path).unwrap();
                if let Some(prior) = &expected {
                    assert_eq!(
                        &bytes, prior,
                        "{kind}: semantic tape differed across widths/backends"
                    );
                } else {
                    expected = Some(bytes);
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join(","));
    });
}

#[test]
fn flow_audit_36_unique_projection_small_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("projection-");
        let _evidence = Evidence(root.clone());
        let mut failed = vec![];
        for (name, source) in [
            (
                "original",
                include_str!("../support/flow_audit_36/projection-original.hir"),
            ),
            (
                "adapted",
                include_str!("../support/flow_audit_36/projection-adapted.hir"),
            ),
        ] {
            let path = root.join(format!("{name}.hir"));
            fs::write(&path, source).unwrap();
            let mut loaded = hirsel_driver::load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let run = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let execution = hirsel_engine::run_observed_checked_bounded(
                &mut loaded.engine,
                &run,
                SimulationConfig::default(),
                1_000_000,
            )
            .unwrap();
            fs::write(
                root.join(format!("{name}-outcome.txt")),
                format!("{execution:#?}"),
            )
            .unwrap();
            // Both legal producers must run. Preserve an original disagreement as red.
            if !matches!(
                execution.outcome,
                Ok(hirsel_engine::RunOutcome::Returned(_))
            ) {
                failed.push(format!("{name}: {:?}", execution.outcome));
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    });
}
