//! Full main, independent cost/path validation, public fixtures and immutable tapes.
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
        include_str!("../../../../examples/routeplan/main.hir"),
    ),
    (
        "network",
        include_str!("../../../../examples/routeplan/network.hir"),
    ),
    (
        "search",
        include_str!("../../../../examples/routeplan/search.hir"),
    ),
    (
        "query",
        include_str!("../../../../examples/routeplan/query.hir"),
    ),
    (
        "routes",
        include_str!("../../../../examples/routeplan/routes.hir"),
    ),
    (
        "report",
        include_str!("../../../../examples/routeplan/report.hir"),
    ),
];
fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/route_planner_33/cases.json")).unwrap()
}
fn known_steps(id: &str) -> bool {
    [
        "edges-cap",
        "edges-over",
        "closures-cap",
        "closures-over",
        "input-cap",
    ]
    .contains(&id)
}
fn selected(row: &Value) -> bool {
    std::env::var("ROUTE_PLANNER_33_CASE").map_or(true, |v| v.split(',').any(|s| row["id"] == s))
}
fn scratch(label: &str) -> PathBuf {
    // Host execution is explicit: this archive must survive Buck's scratch cleanup.
    let base = std::env::var_os("ROUTE_PLANNER_33_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("route-planner-33-evidence"));
    fs::create_dir_all(&base).unwrap();
    let p = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep();
    eprintln!("ROUTE_ARCHIVE {}", p.display());
    p
}
fn package(root: &Path) -> PathBuf {
    package_entry(root, SOURCES[0].1)
}
fn package_entry(root: &Path, entry: &str) -> PathBuf {
    for (name, src) in SOURCES {
        fs::write(root.join(format!("{name}.hir")), src).unwrap();
    }
    fs::write(root.join("main.hir"), entry).unwrap();
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/routeplan/hirsel.json")).unwrap();
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
        include_bytes!("../support/route_planner_33/run_capped.py"),
    )
    .unwrap();
    fs::write(
        root.join("cases.json"),
        include_bytes!("../support/route_planner_33/cases.json"),
    )
    .unwrap();
    fs::write(
        root.join("seal.py"),
        include_bytes!("../support/route_planner_33/seal.py"),
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
        "data".into(),
        "network.json".into(),
        "routes.json".into(),
        row["jobs"].as_u64().unwrap_or(jobs).to_string(),
    ];
    if row["pending"] == true {
        args.push("--interruptible".into());
    }
    Fixture {
        args,
        files: BTreeMap::from([
            (
                "data/network.json".into(),
                row["input"].as_str().unwrap().as_bytes().to_vec(),
            ),
            ("data/routes.json".into(), b"preserve".to_vec()),
        ]),
        mounts: BTreeMap::from([(
            "data".into(),
            if row["access"] == "read" {
                Access::Read
            } else {
                Access::ReadWrite
            },
        )]),
        file_reads: if row["pending"] == true {
            BTreeMap::from([("data/network.json".into(), FileReadScript::Pending)])
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
// Independent exhaustive SIMPLE paths. This has no frontier, distances or
// predecessor model in common with the implementation (graphs <=7 only).
fn exhaustive(input: &Value, request: &Value, excluded: Option<u64>) -> Option<u64> {
    #[allow(clippy::too_many_arguments)]
    fn visit(
        b: &Value,
        q: &Value,
        excluded: Option<u64>,
        v: u64,
        seen: u64,
        cost: u64,
        best: &mut Option<u64>,
    ) {
        if v == q["target"].as_u64().unwrap() {
            *best = Some(best.map_or(cost, |x| x.min(cost)));
            return;
        }
        for (i, e) in b["edges"].as_array().unwrap().iter().enumerate() {
            let target = e["target"].as_u64().unwrap();
            if e["source"] != v
                || seen & (1 << target) != 0
                || excluded == Some(i as u64)
                || q["closed"].as_array().unwrap().contains(&json!(i))
            {
                continue;
            }
            let Some(next) = cost.checked_add(e["weight"].as_u64().unwrap()) else {
                continue;
            };
            if q["max_cost"].as_u64().is_some_and(|cap| next > cap) {
                continue;
            }
            visit(b, q, excluded, target, seen | (1 << target), next, best);
        }
    }
    let source = request["source"].as_u64().unwrap();
    let mut result = None;
    visit(
        input,
        request,
        excluded,
        source,
        1 << source,
        0,
        &mut result,
    );
    result
}
fn validate_path(b: &Value, q: &Value, result: &Value, excluded: Option<u64>) {
    if result["status"] != "found" {
        assert_eq!(result["cost"], Value::Null);
        assert_eq!(result["edges"], json!([]));
        return;
    }
    let mut cursor = q["source"].as_u64().unwrap();
    let mut visited = BTreeSet::from([cursor]);
    let mut cost = 0u64;
    for ordinal in result["edges"].as_array().unwrap() {
        let id = ordinal.as_u64().unwrap();
        assert_ne!(excluded, Some(id));
        assert!(!q["closed"].as_array().unwrap().contains(ordinal));
        let edge = &b["edges"][id as usize];
        assert_eq!(edge["source"].as_u64(), Some(cursor));
        cursor = edge["target"].as_u64().unwrap();
        assert!(visited.insert(cursor), "path repeats a node");
        cost = cost.checked_add(edge["weight"].as_u64().unwrap()).unwrap();
    }
    assert_eq!(q["target"].as_u64(), Some(cursor));
    assert_eq!(result["cost"].as_u64(), Some(cost));
    assert!(q["max_cost"].as_u64().is_none_or(|cap| cost <= cap));
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
        assert_eq!(state.stderr, format!("routeplan: {error}\n").as_bytes());
        assert_eq!(files, &expected, "domain failure changed files");
        return b"preserve".to_vec();
    }
    assert_eq!(o.exit_code, Some(0));
    assert!(state.stderr.is_empty());
    let input: Value = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
    assert_eq!(
        o.output,
        format!(
            "routeplan: {} queries\n",
            input["queries"].as_array().unwrap().len()
        )
        .as_bytes()
    );
    let bytes = files["data/routes.json"].clone();
    expected.insert("data/routes.json".into(), bytes.clone());
    assert_eq!(files, &expected, "temporary leaked or input mutated");
    assert!(bytes.len() <= 65536 && bytes.ends_with(b"\n"));
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["revision"], input["revision"]);
    let results = doc["results"].as_array().unwrap();
    assert_eq!(results.len(), input["queries"].as_array().unwrap().len());
    for (i, result) in results.iter().enumerate() {
        let q = &input["queries"][i];
        let primary = &result["primary"];
        validate_path(&input, q, primary, None);
        if row["statuses"][i] == "overflow" {
            assert_eq!(primary["status"], "overflow");
        } else {
            assert_eq!(primary["cost"], row["costs"][i], "frozen independent cost");
            assert_eq!(
                primary["status"],
                if row["costs"][i].is_null() {
                    "unreachable"
                } else {
                    "found"
                }
            );
            if input["nodes"].as_u64().unwrap() <= 7 {
                assert_eq!(primary["cost"].as_u64(), exhaustive(&input, q, None));
            }
        }
        if let Some(excluded) = q["alternative"]
            .as_u64()
            .filter(|_| primary["status"] == "found")
        {
            let alternative = &result["alternative"];
            assert!(
                primary["edges"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(excluded))
            );
            validate_path(&input, q, alternative, Some(excluded));
            assert_eq!(
                alternative["cost"].as_u64(),
                exhaustive(&input, q, Some(excluded))
            );
        } else {
            assert!(result["alternative"].is_null());
        }
    }
    if row["id"] == "ties-parallel" {
        assert_eq!(results[0]["primary"]["edges"], json!([0, 2]));
        assert_eq!(results[0]["alternative"]["edges"], json!([1, 3]));
    }
    bytes
}
#[test]
fn route_planner_33_reference_checked_workflows() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("simulation-");
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
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
                let id = row["id"].as_str().unwrap();
                eprintln!("ROUTE_STARTED {id} {engine}");
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
                        if let Some(observed) = execution.observation {
                            verify(row, &observed, &world);
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
                            reference = Some(observed);
                            receipt["completed"] = json!(true);
                            receipt["passed"] = json!(true);
                        } else {
                            if known_steps(id) {
                                assert!(
                                    matches!(&execution.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == 1_000_000)
                                );
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
                eprintln!("ROUTE_RECEIPT {receipt}");
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

// This test also records every fixture and replays each immutable tape through
// both native backends and both interpreters, without files/readiness scripts.
#[test]
fn route_planner_33_native_four_engine_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("native-");
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
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
        let baseline: Option<Value> = std::env::var_os("ROUTE_PLANNER_33_BASELINE")
            .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap());
        if let Some(baseline) = &baseline {
            for (name, source) in SOURCES {
                assert_eq!(
                    baseline["sources"][name].as_str(),
                    Some(source),
                    "retained baseline source changed"
                );
            }
        }
        let mut receipts = vec![];
        let mut controls_failed = false;
        let mut missing_live_stderr = vec![];
        for row in cases().iter().filter(|r| selected(r)) {
            let id = row["id"].as_str().unwrap();
            let world = fixture(row, 4);
            let cap_oracles: Value = serde_json::from_str(include_str!(
                "../support/route_planner_33/native-cap-oracles.json"
            ))
            .unwrap();
            let oracle_row = if let Some(baseline) = &baseline {
                Some(&baseline["records"][id])
            } else if known_steps(id) {
                Some(&cap_oracles[id])
            } else {
                None
            };
            let (observation, expected_stderr, output) = if let Some(row) = oracle_row {
                assert!(!row.is_null(), "no retained reference for {id}");
                (
                    Observation {
                        exit_code: row["exit"].as_i64().map(|x| x as i32),
                        blocked: false,
                        budget: None,
                        signaled: None,
                        output: serde_json::from_value(row["stdout"].clone()).unwrap(),
                        failures: vec![],
                        roots: vec![],
                    },
                    serde_json::from_value::<Vec<u8>>(row["stderr"].clone()).unwrap(),
                    serde_json::from_value::<Vec<u8>>(row["file"].clone()).unwrap(),
                )
            } else {
                let reference = hirsel_engine::run_observed_snapshot_bounded(
                    &mut loaded.engine,
                    &run,
                    (&world).into(),
                    1_000_000,
                )
                .unwrap();
                let observable = reference.observation.expect("original over-budget reference: select retained baseline or keep this obligation incomplete");
                let output = verify(row, &observable, &world);
                (
                    Observation::of_reference(&observable),
                    observable.final_state.as_ref().unwrap().stderr.clone(),
                    output,
                )
            };
            let cell = root.join(id);
            fs::create_dir(&cell).unwrap();
            fs::write(cell.join("fixture.json"), serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"signals":world.signals})).unwrap()).unwrap();
            // Heap differential is an explicit additional execution, not a V check.
            if ["networkx-xg", "version", "denial", "pending-cancel"].contains(&id)
                && !std::env::var("ROUTE_PLANNER_33_COMPLETED_DIFFERENTIAL")
                    .unwrap_or_default()
                    .split(',')
                    .any(|s| s == id)
            {
                for (name, outcome) in differential_in(
                    &input,
                    &loaded.entry,
                    &observation,
                    &matrix,
                    &tools,
                    &cell,
                    &world,
                ) {
                    assert_eq!(outcome, Outcome::Agree, "{id}: {name}");
                }
            }
            let mut runnables = vec![];
            for (backend, emitted) in &objects {
                let dir = cell.join(format!("{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, emitted, &tools, &dir, &world)
                        .unwrap();
                runnables.push((runnable, dir));
            }
            for (runnable, dir) in &runnables {
                let retained = std::env::var_os("ROUTE_PLANNER_33_RECOVER_ROOT")
                    .map(|root| {
                        PathBuf::from(root)
                            .join(id)
                            .join(dir.file_name().unwrap())
                            .join("recording")
                    })
                    .filter(|p| p.is_file());
                let tape = retained.clone().unwrap_or_else(|| dir.join("recording"));
                let mut raw_stderr_matches = expected_stderr.is_empty();
                if retained.is_none() {
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
                    assert_eq!(observe(&live, &ready, &sites).unwrap(), observation);
                    raw_stderr_matches = live.stderr == expected_stderr;
                    if !raw_stderr_matches {
                        assert!(
                            live.stderr.is_empty() && !expected_stderr.is_empty(),
                            "unexpected stderr mismatch: {:?}",
                            live.stderr
                        );
                    }
                    fs::write(dir.join("live-streams.json"),serde_json::to_vec_pretty(&json!({"stdout":live.stdout,"stderr":live.stderr,"exit":live.exit,"stderr_matches":raw_stderr_matches})).unwrap()).unwrap();
                } else {
                    eprintln!("ROUTE_REUSED_LIVE {id} {}", tape.display());
                }
                if !raw_stderr_matches {
                    missing_live_stderr.push(format!(
                        "{id}:{}",
                        dir.file_name().unwrap().to_str().unwrap()
                    ));
                    eprintln!(
                        "ROUTE_INCOMPLETE {id} {} live raw stderr: public Fixture sink; exact tape stderr still required",
                        dir.display()
                    );
                }
                let frozen = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                let stderr: Vec<_> = record
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                assert_eq!(stderr, expected_stderr);
                assert_eq!(record.arguments, world.args);
                record.bound_run().unwrap();
                // File write payloads prove exact published bytes independently
                // of Observation, which intentionally omits snapshot final files.
                let writes: Vec<_> = record
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                if row["error"].is_null() {
                    assert_eq!(writes, output);
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
                let replay_world = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    signals: None,
                    ..world.clone()
                };
                let reused_replays = std::env::var("ROUTE_PLANNER_33_COMPLETED_REPLAYS")
                    .unwrap_or_default()
                    .split(',')
                    .any(|s| s == format!("{id}:{}", dir.file_name().unwrap().to_str().unwrap()));
                if !reused_replays {
                    for (player, replay_dir) in &runnables {
                        let replayed = launch_fixture(
                            player,
                            &ready.target,
                            &tools,
                            replay_dir,
                            true,
                            Tape::Replay(&tape),
                            &replay_world,
                        )
                        .unwrap();
                        assert_eq!(observe(&replayed, &ready, &sites).unwrap(), observation);
                        assert_eq!(replayed.stderr, expected_stderr);
                        assert!(
                            fs::read_dir(replay_dir.join("root"))
                                .unwrap()
                                .next()
                                .is_none(),
                            "offline fixture replay mutated host root"
                        );
                    }
                    for replay in [
                        hirsel_driver::replay_observations,
                        hirsel_driver::replay_observations_checked,
                    ] {
                        let replayed = replay(&path, &tape, Some(1_000_000));
                        if known_steps(id) {
                            let error = replayed.unwrap_err().to_string();
                            assert!(
                                error.contains("ToolingLimit")
                                    && error.contains("Steps")
                                    && error.contains("limit: 1000000"),
                                "{id}: {error}"
                            );
                            eprintln!("ROUTE_INCOMPLETE {id} interpreter replay Steps1M");
                        } else {
                            assert_eq!(Observation::of_reference(&replayed.unwrap()), observation);
                        }
                    }
                } else {
                    eprintln!(
                        "ROUTE_REUSED_REPLAYS {id} immutable retained native+interpreter completions"
                    );
                }
                fs::write(root.join(format!("{id}-{}-replays-completed.json",dir.file_name().unwrap().to_str().unwrap())), serde_json::to_vec_pretty(&json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"tape":tape,"native_replays":2,"interpreter_replays":if known_steps(id){0}else{2},"retained_reuse":reused_replays})).unwrap()).unwrap();
                if id == "networkx-xg" && dir.ends_with("Llvm") {
                    for kind in ["binding", "request", "missing", "extra"] {
                        if std::env::var("ROUTE_PLANNER_33_COMPLETED_CONTROLS")
                            .unwrap_or_default()
                            .split(',')
                            .any(|k| k == kind)
                        {
                            eprintln!("ROUTE_REUSED_CONTROL {kind}");
                            continue;
                        }
                        let control_result =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let mut changed = record.clone();
                                let expected = match kind {
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
                                    assert!(error.contains(expected), "{kind}: {error}");
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
                                        &replay_world,
                                    ) {
                                        Err(error) => {
                                            assert!(
                                                error.message.contains(expected),
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
                                                    &observation.output
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
                                                    .contains(expected)
                                                    || refused.record.contains(expected)
                                                    || fs::read_to_string(
                                                        replay_dir.join("refusal")
                                                    )
                                                    .unwrap_or_default()
                                                    .contains(expected),
                                                "{kind}: {} {:?}",
                                                refused.record,
                                                refused.stderr
                                            );
                                            // A late refusal may have printed only the exact
                                            // valid prefix, never arbitrary output.
                                            let prefix: &[u8] = if kind == "extra" {
                                                &observation.output
                                            } else {
                                                b""
                                            };
                                            assert_eq!(refused.stdout, prefix, "{kind}");
                                        }
                                    }
                                }
                            }));
                        if control_result.is_ok() {
                            eprintln!("ROUTE_REFUSAL_PASS {kind} four engines");
                        } else {
                            controls_failed = true;
                            eprintln!(
                                "ROUTE_REFUSAL_FAIL {kind}; retained; continuing independent workflows"
                            );
                        }
                    }
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                receipts.push(json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"selected":true,"started":true,"completed":raw_stderr_matches,"passed":raw_stderr_matches,"live_stdout_status_file_and_tape_stderr":true,"live_raw_stderr_matches":raw_stderr_matches,"raw_stderr_owner":if raw_stderr_matches {"none"} else {"native-file-faults-33 / host-replay-parity-31"},"tape":tape,"native_replays":2,"interpreter_replays_completed":if known_steps(id) {0} else {2},"expected_provenance":if known_steps(id) {"independent original cap oracle; never evaluator receipt"} else {"reference+checked retained or fresh reference"},"interpreter_replay_disposition":if known_steps(id) {"INCOMPLETE Steps1M"} else {"pass"}}));
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
                eprintln!(
                    "ROUTE_RECORDED {id} {} two native replays; live raw stderr and interpreter replay dispositions recorded separately",
                    dir.display()
                );
            }
        }
        assert!(!receipts.is_empty());
        assert!(
            !controls_failed,
            "a refusal control failed; independent cells retained"
        );
        assert!(
            missing_live_stderr.is_empty(),
            "INCOMPLETE native Fixture raw stderr; owner native-file-faults-33 / host-replay-parity-31; exact cells: {}",
            missing_live_stderr.join(",")
        );
    });
}

#[test]
fn route_planner_33_output_cap_and_predecessor_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("helper-controls-");
        let path = package_entry(
            &root,
            include_str!("../support/route_planner_33/helper.hir"),
        );
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for count in [65536, 65537] {
            let fixture = Fixture {
                files: BTreeMap::from([
                    ("data/probe".into(), vec![b'x'; count]),
                    ("data/output".into(), b"preserve".to_vec()),
                ]),
                mounts: BTreeMap::from([("data".into(), Access::ReadWrite)]),
                ..Default::default()
            };
            let mut previous = None;
            for checked in [false, true] {
                let execution = if checked {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        (&fixture).into(),
                        1_000_000,
                    )
                } else {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        (&fixture).into(),
                        1_000_000,
                    )
                }
                .unwrap();
                let observation = execution
                    .observation
                    .expect("helper must complete within original budget");
                assert_eq!(observation.exit_code, Some(0));
                assert!(observation.failures.is_empty());
                let state = observation.final_state.as_ref().unwrap();
                let mut expected = fixture.files.clone();
                if count == 65536 {
                    expected.insert("data/output".into(), vec![b'x'; count]);
                }
                assert_eq!(state.trees[&0].1, expected);
                assert!(observation.output.is_empty() && state.stderr.is_empty());
                if let Some(previous) = &previous {
                    assert_eq!(&observation, previous);
                }
                previous = Some(observation);
                eprintln!(
                    "ROUTE_HELPER_PASS bytes={count} checked={checked}; cap and explicit predecessor guard"
                );
            }
        }
    });
}
