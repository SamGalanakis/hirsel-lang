//! Full main, independent matching/certificate validation, public fixtures and immutable tapes.
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
        include_str!("../../../../examples/assignplan/main.hir"),
    ),
    (
        "model",
        include_str!("../../../../examples/assignplan/model.hir"),
    ),
    (
        "constraints",
        include_str!("../../../../examples/assignplan/constraints.hir"),
    ),
    (
        "matching",
        include_str!("../../../../examples/assignplan/matching.hir"),
    ),
    (
        "explain",
        include_str!("../../../../examples/assignplan/explain.hir"),
    ),
    (
        "report",
        include_str!("../../../../examples/assignplan/report.hir"),
    ),
];
fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/assignment_planner_34/cases.json")).unwrap()
}
fn known_steps(id: &str) -> bool {
    ["allowed-cap", "input-cap"].contains(&id)
}
fn selected(row: &Value) -> bool {
    std::env::var("ASSIGNMENT_PLANNER_34_CASE")
        .map_or(true, |v| v.split(',').any(|s| row["id"] == s))
}
fn scratch(label: &str) -> PathBuf {
    // Host execution is explicit: this archive must survive Buck's scratch cleanup.
    let base = std::env::var_os("ASSIGNMENT_PLANNER_34_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("assignment-planner-34-evidence"));
    fs::create_dir_all(&base).unwrap();
    let p = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep();
    eprintln!("ASSIGN_ARCHIVE {}", p.display());
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
        serde_json::from_str(include_str!("../../../../examples/assignplan/hirsel.json")).unwrap();
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
        include_bytes!("../support/assignment_planner_34/run_capped.py"),
    )
    .unwrap();
    fs::write(
        root.join("cases.json"),
        include_bytes!("../support/assignment_planner_34/cases.json"),
    )
    .unwrap();
    fs::write(
        root.join("seal.py"),
        include_bytes!("../support/assignment_planner_34/seal.py"),
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
        "batch.json".into(),
        "assignments.json".into(),
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
            ("data/assignments.json".into(), b"preserve".to_vec()),
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
            .map(|v| serde_json::from_value(v.clone()).unwrap())
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
// Exhaustive enumeration over jobs, without BFS, layers, matching mutation or
// alternating-path logic. This oracle is intentionally limited to <=7+7.
fn exhaustive(m: &Value) -> u64 {
    fn visit(m: &Value, job: usize, used: u64) -> u64 {
        if job == m["jobs"].as_array().unwrap().len() {
            return 0;
        }
        let mut best = visit(m, job + 1, used);
        for e in m["allowed"].as_array().unwrap() {
            let resource = e["resource"].as_u64().unwrap();
            if e["job"] == job
                && used & (1 << resource) == 0
                && !m["disabled"].as_array().unwrap().contains(&json!(resource))
            {
                best = best.max(1 + visit(m, job + 1, used | (1 << resource)));
            }
        }
        best
    }
    visit(m, 0, 0)
}
fn validate_certificate(m: &Value, c: &Value) {
    let jobs = m["jobs"].as_array().unwrap().len();
    let resources = m["resources"].as_array().unwrap().len();
    let left = c["job_to_resource"].as_array().unwrap();
    let right = c["resource_to_job"].as_array().unwrap();
    assert_eq!(left.len(), jobs);
    assert_eq!(right.len(), resources);
    let mut pairs = vec![];
    let mut used = BTreeSet::new();
    let mut unmatched = BTreeSet::new();
    for (j, r) in left.iter().enumerate() {
        if let Some(r) = r.as_u64() {
            assert!(used.insert(r));
            assert_eq!(right[r as usize], j);
            assert!(!m["disabled"].as_array().unwrap().contains(&json!(r)));
            let pair = json!({"job":j,"resource":r});
            assert!(m["allowed"].as_array().unwrap().contains(&pair));
            pairs.push(pair);
        } else {
            unmatched.insert(j as u64);
        }
    }
    for (r, j) in right.iter().enumerate() {
        if let Some(j) = j.as_u64() {
            assert_eq!(left[j as usize], r);
        }
    }
    assert_eq!(c["pairs"], json!(pairs));
    assert_eq!(c["cardinality"], pairs.len());
    let set = |key: &str, bound: usize| -> BTreeSet<u64> {
        let a = c[key].as_array().unwrap();
        let xs: Vec<_> = a.iter().map(|v| v.as_u64().unwrap()).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]));
        assert!(xs.iter().all(|x| (*x as usize) < bound));
        xs.into_iter().collect()
    };
    assert_eq!(set("unmatched", jobs), unmatched);
    // Independent fixed point, different from the producer's queue traversal.
    let mut reached_jobs = unmatched;
    let mut reached_resources = BTreeSet::new();
    loop {
        let previous = (reached_jobs.clone(), reached_resources.clone());
        for e in m["allowed"].as_array().unwrap() {
            let j = e["job"].as_u64().unwrap();
            let r = e["resource"].as_u64().unwrap();
            if reached_jobs.contains(&j)
                && left[j as usize] != r
                && !m["disabled"].as_array().unwrap().contains(&json!(r))
            {
                reached_resources.insert(r);
            }
        }
        for r in &reached_resources {
            if let Some(j) = right[*r as usize].as_u64() {
                reached_jobs.insert(j);
            }
        }
        if previous == (reached_jobs.clone(), reached_resources.clone()) {
            break;
        }
    }
    assert_eq!(set("reached_jobs", jobs), reached_jobs);
    assert_eq!(set("reached_resources", resources), reached_resources);
    let cover_jobs = set("cover_jobs", jobs);
    let cover_resources = set("cover_resources", resources);
    assert_eq!(
        cover_jobs,
        (0..jobs as u64)
            .filter(|j| !reached_jobs.contains(j))
            .collect()
    );
    assert_eq!(cover_resources, reached_resources);
    assert_eq!(cover_jobs.len() + cover_resources.len(), pairs.len());
    for e in m["allowed"].as_array().unwrap() {
        if !m["disabled"].as_array().unwrap().contains(&e["resource"]) {
            assert!(
                cover_jobs.contains(&e["job"].as_u64().unwrap())
                    || cover_resources.contains(&e["resource"].as_u64().unwrap())
            );
        }
    }
    if jobs <= 7 && resources <= 7 {
        assert_eq!(pairs.len() as u64, exhaustive(m));
    }
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
        assert_eq!(state.stderr, format!("assignplan: {error}\n").as_bytes());
        assert_eq!(
            files, &expected,
            "error changed files or leaked atomic temporary"
        );
        return b"preserve".to_vec();
    }
    assert_eq!(o.exit_code, Some(0));
    assert!(state.stderr.is_empty());
    let input: Value = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
    let manifests = input["manifests"].as_array().unwrap();
    assert_eq!(
        o.output,
        format!("assignplan: {} manifests\n", manifests.len()).as_bytes()
    );
    let bytes = files["data/assignments.json"].clone();
    expected.insert("data/assignments.json".into(), bytes.clone());
    assert_eq!(files, &expected, "input changed or temporary leaked");
    assert!(bytes.len() <= 16384 && bytes.ends_with(b"\n"));
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["revision"], input["revision"]);
    let results = doc["results"].as_array().unwrap();
    assert_eq!(results.len(), manifests.len());
    for (i, (m, c)) in manifests.iter().zip(results).enumerate() {
        validate_certificate(m, c);
        if let Some(optimum) = row["optima"].get(i) {
            assert_eq!(&c["cardinality"], optimum);
        }
        if let Some(optimum) = row["cardinality"].get(i) {
            assert_eq!(&c["cardinality"], optimum);
        }
        if let Some(golden) = row["golden"].get(i) {
            assert_eq!(&c["job_to_resource"], golden);
        }
    }
    bytes
}
#[test]
fn assignment_planner_34_reference_checked_workflows() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("simulation-");
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
                if std::env::var("ASSIGNMENT_PLANNER_34_ENGINE")
                    .is_ok_and(|v| !v.split(',').any(|e| e == engine))
                {
                    continue;
                }
                let id = row["id"].as_str().unwrap();
                eprintln!("ASSIGN_STARTED {id} {engine}");
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
                eprintln!("ASSIGN_RECEIPT {receipt}");
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
// Keep the complete fixture matrix in six disjoint test actions. Every
// partition has fresh tapes and native programs and shares only its checked
// source queries between its reference and checked replay consumers.
#[test]
fn assignment_planner_34_native_four_engine_replay() {
    native_four_engine_replay(0);
}
#[test]
fn assignment_planner_34_native_four_engine_replay_part_1() {
    native_four_engine_replay(1);
}
#[test]
fn assignment_planner_34_native_four_engine_replay_part_2() {
    native_four_engine_replay(2);
}
#[test]
fn assignment_planner_34_native_four_engine_replay_part_3() {
    native_four_engine_replay(3);
}
#[test]
fn assignment_planner_34_native_four_engine_replay_part_4() {
    native_four_engine_replay(4);
}
#[test]
fn assignment_planner_34_native_four_engine_replay_part_5() {
    native_four_engine_replay(5);
}

fn native_four_engine_replay(partition: usize) {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("native-");
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
        let baseline: Option<Value> = std::env::var_os("ASSIGNMENT_PLANNER_34_BASELINE")
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
        let rows = cases();
        let mut compared = 0;
        for row in rows
            .iter()
            .enumerate()
            .filter(|(index, row)| index % 6 == partition && selected(row))
            .map(|(_, row)| row)
        {
            compared += 1;
            let id = row["id"].as_str().unwrap();
            let world = fixture(row, 4);
            let cap_oracles: Value = serde_json::from_str(include_str!(
                "../support/assignment_planner_34/native-cap-oracles.json"
            ))
            .unwrap();
            let oracle_row = if let Some(b) = &baseline {
                Some(&b["records"][id])
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
            fs::write(cell.join("fixture.json"), serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"file_faults":world.file_faults,"signals":world.signals})).unwrap()).unwrap();
            // Heap differential is an explicit additional execution, not a V check.
            if ["networkx", "version", "denial", "pending-cancel"].contains(&id)
                && !std::env::var("ASSIGNMENT_PLANNER_34_COMPLETED_DIFFERENTIAL")
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
                let retained = std::env::var_os("ASSIGNMENT_PLANNER_34_RECOVER_ROOT")
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
                    eprintln!("ASSIGN_REUSED_LIVE {id} {}", tape.display());
                }
                if !raw_stderr_matches {
                    missing_live_stderr.push(format!(
                        "{id}:{}",
                        dir.file_name().unwrap().to_str().unwrap()
                    ));
                    eprintln!(
                        "ASSIGN_INCOMPLETE {id} {} live raw stderr: public Fixture sink; exact tape stderr still required",
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
                } else if matches!(
                    row["fault_operation"].as_str(),
                    Some("file.write_all" | "file.commit")
                ) {
                    assert_eq!(
                        writes,
                        serde_json::from_value::<Vec<u8>>(row["expected_write"].clone()).unwrap()
                    );
                    assert_eq!(
                        record
                            .events
                            .iter()
                            .filter(|e| matches!(e.request, hirsel_rt::io::Request::Commit { .. }))
                            .count(),
                        usize::from(row["fault_operation"] == "file.commit")
                    );
                    assert!(record.events.iter().any(|e| matches!(
                        &e.completion,
                        hirsel_rt::io::Completion::Unit(Err(hirsel_rt::io::IoError::Fault(77)))
                    )));
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
                    file_faults: Default::default(),
                    signals: None,
                    ..world.clone()
                };
                let reused_replays = std::env::var("ASSIGNMENT_PLANNER_34_COMPLETED_REPLAYS")
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
                        fs::write(root.join(format!("{id}-{}-{}-native-replay.json",dir.file_name().unwrap().to_str().unwrap(),replay_dir.file_name().unwrap().to_str().unwrap())), serde_json::to_vec_pretty(&json!({"id":id,"tape":tape,"completed":true,"passed":true,"player":replay_dir.file_name().unwrap().to_str()})).unwrap()).unwrap();
                    }
                    // Replay state is fresh for every tape and engine. Reuse
                    // the checked source graph already used for the reference.
                    for checked in [false, true] {
                        let replayed = hirsel_driver::replay_loaded_observations(
                            &mut loaded,
                            &tape,
                            Some(1_000_000),
                            checked,
                        );
                        if known_steps(id) {
                            let error = replayed.unwrap_err().to_string();
                            assert!(
                                error.contains("ToolingLimit")
                                    && error.contains("Steps")
                                    && error.contains("limit: 1000000"),
                                "{id}: {error}"
                            );
                            eprintln!("ASSIGN_INCOMPLETE {id} interpreter replay Steps1M");
                        } else {
                            assert_eq!(Observation::of_reference(&replayed.unwrap()), observation);
                        }
                    }
                } else {
                    eprintln!(
                        "ASSIGN_REUSED_REPLAYS {id} immutable retained native+interpreter completions"
                    );
                }
                fs::write(root.join(format!("{id}-{}-replays-completed.json",dir.file_name().unwrap().to_str().unwrap())), serde_json::to_vec_pretty(&json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"tape":tape,"native_replays":2,"interpreter_replays":if known_steps(id){0}else{2},"retained_reuse":reused_replays})).unwrap()).unwrap();
                if id == "networkx" && dir.ends_with("Llvm") {
                    for kind in ["binding", "request", "missing", "extra"] {
                        if std::env::var("ASSIGNMENT_PLANNER_34_COMPLETED_CONTROLS")
                            .unwrap_or_default()
                            .split(',')
                            .any(|k| k == kind)
                        {
                            eprintln!("ASSIGN_REUSED_CONTROL {kind}");
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
                                for checked in [false, true] {
                                    let error = hirsel_driver::replay_loaded_observations(
                                        &mut loaded,
                                        &bad,
                                        Some(1_000_000),
                                        checked,
                                    )
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
                            eprintln!("ASSIGN_REFUSAL_PASS {kind} four engines");
                        } else {
                            controls_failed = true;
                            eprintln!(
                                "ASSIGN_REFUSAL_FAIL {kind}; retained; continuing independent workflows"
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
                    "ASSIGN_RECORDED {id} {} two native replays; live raw stderr and interpreter replay dispositions recorded separately",
                    dir.display()
                );
            }
        }
        assert!(!receipts.is_empty());
        let selected_count = rows
            .iter()
            .enumerate()
            .filter(|(index, row)| index % 6 == partition && selected(row))
            .count();
        assert_eq!(compared, selected_count);
        eprintln!(
            "assignment partition={partition}/6 inventory={} compared={compared}",
            rows.len()
        );
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
fn assignment_planner_34_output_cap_and_certificate_controls() {
    hirsel_engine::with_compiler_stack(|| {
        // Missing-only recovery for an immutable original tape before a runtime
        // revision. Never treats a new recording as recovery of historical bytes.
        if let Some(tape) = std::env::var_os("ASSIGNMENT_PLANNER_34_INTERPRETER_TAPE") {
            let root = scratch("interpreter-recovery-");
            let path = package(&root);
            let tape = PathBuf::from(tape);
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
            assert_eq!(stderr, b"assignplan: endpoint\n");
            for (index, replay) in [
                hirsel_driver::replay_observations,
                hirsel_driver::replay_observations_checked,
            ]
            .into_iter()
            .enumerate()
            {
                if std::env::var("ASSIGNMENT_PLANNER_34_COMPLETED_ORIGINAL_INTERPRETER")
                    .unwrap_or_default()
                    .split(',')
                    .any(|x| x == index.to_string())
                {
                    continue;
                }
                let observed = replay(&path, &tape, Some(1_000_000)).unwrap();
                fs::write(
                    root.join(format!("endpoint-original-{index}-observation.txt")),
                    format!("{observed:#?}"),
                )
                .unwrap();
                assert_eq!(observed.exit_code, Some(2));
                assert!(
                    observed.output.is_empty()
                        && observed.failures.is_empty()
                        && observed.blocked.is_none()
                );
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                fs::write(
                    root.join(format!("endpoint-original-{index}.json")),
                    serde_json::to_vec_pretty(
                        &json!({"tape":tape,"completed":true,"passed":true,"engine":index}),
                    )
                    .unwrap(),
                )
                .unwrap();
                eprintln!("ASSIGN_ORIGINAL_ENDPOINT_REPLAY_PASS {index}");
            }
        }

        let root = scratch("helper-controls-");
        let path = package_entry(
            &root,
            include_str!("../support/assignment_planner_34/helper.hir"),
        );
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for count in [16384, 16385] {
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
                if count == 16384 {
                    expected.insert("data/output".into(), vec![b'x'; count]);
                }
                assert_eq!(state.trees[&0].1, expected);
                assert!(observation.output.is_empty() && state.stderr.is_empty());
                if let Some(previous) = &previous {
                    assert_eq!(&observation, previous);
                }
                previous = Some(observation);
                eprintln!(
                    "ASSIGN_HELPER_PASS bytes={count} checked={checked}; cap and explicit certificate guards"
                );
            }
        }
    });
}

// Deferred until the coordinator deploys supported exact CPU requests.
// Never invokes topology overrides, reservations or a substitute input.
#[test]
#[ignore = "requires coordinator exact-CPU deployment; all 18 original cells remain incomplete"]
fn assignment_planner_34_ordered_capacity() {
    let root = scratch("ordered-");
    let _evidence = super::width_capacity_36::Evidence(root.clone());
    let path = package(&root);
    fs::write(
        root.join("ordered.py"),
        include_str!("../support/assignment_planner_34/ordered.py"),
    )
    .unwrap();
    let selection = std::env::var("ASSIGNMENT_PLANNER_34_ORDERED").unwrap_or_default();
    for backend in ["llvm", "cranelift"] {
        let binary = root.join(format!("ordered-{backend}"));
        let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", backend])
            .arg(&path)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        fs::write(root.join(format!("{backend}-build.stdout")), &built.stdout).unwrap();
        fs::write(root.join(format!("{backend}-build.stderr")), &built.stderr).unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        for width in [1, 2, 4] {
            for kind in ["success", "domain", "cancel"] {
                let id = format!("{backend}:{width}:{kind}");
                if !selection.is_empty() && !selection.split(',').any(|x| x == id) {
                    continue;
                }
                let cell = root.join(format!("{backend}-{width}-{kind}"));
                let run = Command::new("python3")
                    .arg(root.join("ordered.py"))
                    .arg("--binary")
                    .arg(&binary)
                    .arg("--root")
                    .arg(&cell)
                    .arg("--fixture-root")
                    .arg(&root)
                    .arg("--width")
                    .arg(width.to_string())
                    .arg("--kind")
                    .arg(kind)
                    .output()
                    .unwrap();
                fs::write(root.join(format!("{id}-stdout")), &run.stdout).unwrap();
                fs::write(root.join(format!("{id}-stderr")), &run.stderr).unwrap();
                assert!(
                    run.status.success(),
                    "{id}: {}",
                    String::from_utf8_lossy(&run.stderr)
                );
            }
        }
    }
}

#[test]
fn input_cap_retains_two_million_steps_of_work() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("input-cap-work-");
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let row = cases()
            .into_iter()
            .find(|r| r["id"] == "input-cap")
            .unwrap();
        assert_eq!(row["input"].as_str().unwrap().len(), 32768);
        let world = fixture(&row, 4);
        for checked in [false, true] {
            let execute = if checked {
                hirsel_engine::run_observed_checked_bounded
            } else {
                hirsel_engine::run_observed_snapshot_bounded
            };
            let execution = execute(&mut loaded.engine, &run, (&world).into(), 2_000_000).unwrap();
            assert!(execution.observation.is_none());
            assert!(matches!(execution.outcome,
                Ok(hirsel_engine::RunOutcome::ToolingLimit(ref limit))
                if limit.kind == "Steps" && limit.limit == 2_000_000));
            assert_eq!(execution.step_usage.unwrap().consumed, 2_000_000);
            eprintln!("input-cap checked={checked} work exceeds 2000000 steps");
        }
    });
}
