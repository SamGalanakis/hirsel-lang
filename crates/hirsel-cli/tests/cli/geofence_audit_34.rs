//! Full main, independent exact rational geometry validation, public fixtures and immutable tapes.
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
        include_str!("../../../../examples/geoaudit/main.hir"),
    ),
    (
        "model",
        include_str!("../../../../examples/geoaudit/model.hir"),
    ),
    (
        "validate",
        include_str!("../../../../examples/geoaudit/validate.hir"),
    ),
    (
        "geometry",
        include_str!("../../../../examples/geoaudit/geometry.hir"),
    ),
    (
        "audit",
        include_str!("../../../../examples/geoaudit/audit.hir"),
    ),
    (
        "report",
        include_str!("../../../../examples/geoaudit/report.hir"),
    ),
];
fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/geofence_audit_34/cases.json")).unwrap()
}
fn selected(row: &Value) -> bool {
    std::env::var("GEOFENCE_AUDIT_34_CASE").map_or(true, |v| v.split(',').any(|s| row["id"] == s))
}
fn scratch(label: &str) -> PathBuf {
    // Host execution is explicit: this archive must survive Buck's scratch cleanup.
    let base = std::env::var_os("GEOFENCE_AUDIT_34_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("geofence-audit-34-evidence"));
    fs::create_dir_all(&base).unwrap();
    let p = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep();
    eprintln!("GEO_ARCHIVE {}", p.display());
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
        serde_json::from_str(include_str!("../../../../examples/geoaudit/hirsel.json")).unwrap();
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
        include_bytes!("../support/geofence_audit_34/run_capped.py"),
    )
    .unwrap();
    fs::write(
        root.join("cases.json"),
        include_bytes!("../support/geofence_audit_34/cases.json"),
    )
    .unwrap();
    fs::write(
        root.join("seal.py"),
        include_bytes!("../support/geofence_audit_34/seal.py"),
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
        "tracks.json".into(),
        "audit.json".into(),
        row["jobs"].as_u64().unwrap_or(jobs).to_string(),
    ];
    if row["pending"] == true {
        args.push("--interruptible".into());
    }
    Fixture {
        args,
        files: BTreeMap::from([
            (
                "data/tracks.json".into(),
                row["input"].as_str().unwrap().as_bytes().to_vec(),
            ),
            ("data/audit.json".into(), b"preserve".to_vec()),
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
            .get("faults")
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .unwrap_or_default(),
        file_reads: if row["pending"] == true {
            BTreeMap::from([("data/tracks.json".into(), FileReadScript::Pending)])
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
fn verify(row: &Value, o: &hirsel_interp::observation::Observable, world: &Fixture) -> Vec<u8> {
    assert!(o.failures.is_empty(), "{}: {:?}", row["id"], o.failures);
    assert!(o.blocked.is_none());
    let state = o.final_state.as_ref().unwrap();
    let files = &state.trees[&0].1;
    let mut expected = world.files.clone();
    if let Some(error) = row["error"].as_str() {
        assert_eq!(o.exit_code, Some(2));
        assert!(o.output.is_empty());
        assert_eq!(state.stderr, format!("geoaudit: {error}\n").as_bytes());
        assert_eq!(files, &expected, "domain failure changed files");
        return b"preserve".to_vec();
    }
    assert_eq!(o.exit_code, Some(0));
    assert!(state.stderr.is_empty());
    let input: Value = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
    assert_eq!(
        o.output,
        format!(
            "geoaudit: {} tracks\n",
            input["tracks"].as_array().unwrap().len()
        )
        .as_bytes()
    );
    let bytes = files["data/audit.json"].clone();
    expected.insert("data/audit.json".into(), bytes.clone());
    assert_eq!(files, &expected, "temporary leaked or input mutated");
    assert!(bytes.len() <= 16384 && bytes.ends_with(b"\n"));
    assert_eq!(
        bytes,
        row["expected"].as_str().unwrap().as_bytes(),
        "{}: frozen rational parity oracle",
        row["id"]
    );
    bytes
}
#[test]
fn geofence_audit_34_reference_checked_workflows() {
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
                if std::env::var("GEOFENCE_AUDIT_34_ENGINE")
                    .is_ok_and(|v| !v.split(',').any(|e| e == engine))
                {
                    continue;
                }
                let id = row["id"].as_str().unwrap();
                eprintln!("GEO_STARTED {id} {engine}");
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
                            if matches!(
                                &execution.outcome,
                                Ok(hirsel_engine::RunOutcome::ToolingLimit(_))
                            ) {
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
                eprintln!("GEO_RECEIPT {receipt}");
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
fn geofence_audit_34_native_four_engine_replay() {
    native_workflows(0);
}
#[test]
fn geofence_audit_34_native_part_1() {
    native_workflows(1);
}
#[test]
fn geofence_audit_34_native_part_2() {
    native_workflows(2);
}
#[test]
fn geofence_audit_34_native_part_3() {
    native_workflows(3);
}

fn native_workflows(part: usize) {
    // Explicit recovery menus use the original test identity. Ordinary suite
    // runs distribute the complete matrix across four executor-sized tests.
    if part != 0
        && (std::env::var_os("GEOFENCE_AUDIT_34_RECOVERY").is_some()
            || std::env::var_os("GEOFENCE_AUDIT_34_CASE").is_some())
    {
        return;
    }
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("native-");
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
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
        let recovery: Option<Value> = std::env::var_os("GEOFENCE_AUDIT_34_RECOVERY")
            .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap());
        let mut receipts = vec![];
        let mut controls_failed = false;
        let mut missing_live_stderr = vec![];
        for (ordinal, row) in cases().iter().enumerate().filter(|(_, r)| selected(r)) {
            if recovery.is_none()
                && std::env::var_os("GEOFENCE_AUDIT_34_CASE").is_none()
                && ordinal % 4 != part
            {
                continue;
            }
            let id = row["id"].as_str().unwrap();
            if recovery.as_ref().is_some_and(|r| r[id].is_null()) {
                continue;
            }
            let world = fixture(row, 4);
            let error = row["error"].as_str();
            let expected_stderr = error
                .map(|e| format!("geoaudit: {e}\n").into_bytes())
                .unwrap_or_default();
            let output = error
                .map(|_| b"preserve".to_vec())
                .unwrap_or_else(|| row["expected"].as_str().unwrap().as_bytes().to_vec());
            let stdout = if error.is_some() {
                vec![]
            } else {
                let b: Value = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
                format!(
                    "geoaudit: {} tracks\n",
                    b["tracks"].as_array().unwrap().len()
                )
                .into_bytes()
            };
            let observation = Observation {
                exit_code: Some(if error.is_some() { 2 } else { 0 }),
                blocked: false,
                budget: None,
                signaled: None,
                output: stdout,
                failures: vec![],
                roots: vec![],
            };
            let cell = root.join(id);
            fs::create_dir(&cell).unwrap();
            fs::write(cell.join("fixture.json"), serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"file_faults":world.file_faults,"signals":world.signals})).unwrap()).unwrap();
            // Heap differential is an explicit additional execution, not a V check.
            if ["geo-square", "version", "denial", "pending-cancel"].contains(&id)
                && !std::env::var("GEOFENCE_AUDIT_34_COMPLETED_DIFFERENTIAL")
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
                let backend_name = dir.file_name().unwrap().to_str().unwrap();
                let selection = recovery.as_ref().map(|r| &r[id][backend_name]);
                if selection.is_some_and(Value::is_null) {
                    continue;
                }
                let live_requested = selection.map_or(true, |r| r["live"].as_bool().unwrap());

                let retained = if let Some(r) = selection {
                    if live_requested {
                        None
                    } else {
                        Some(PathBuf::from(
                            r["tape"]
                                .as_str()
                                .expect("replay-only needs original immutable tape"),
                        ))
                    }
                } else {
                    std::env::var_os("GEOFENCE_AUDIT_34_RECOVER_ROOT")
                        .map(|root| {
                            PathBuf::from(root)
                                .join(id)
                                .join(backend_name)
                                .join("recording")
                        })
                        .filter(|p| p.is_file())
                };
                let tape = retained.clone().unwrap_or_else(|| dir.join("recording"));
                let mut raw_stderr_matches = expected_stderr.is_empty();
                if retained.is_none() {
                    fs::write(dir.join("live-receipt.json"), serde_json::to_vec_pretty(&json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"selected":true,"started":true,"completed":false,"passed":false})).unwrap()).unwrap();
                    eprintln!("GEO_NATIVE_STARTED {id} {}", dir.display());
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
                    fs::write(dir.join("live-receipt.json"), serde_json::to_vec_pretty(&json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"selected":true,"started":true,"completed":raw_stderr_matches,"passed":raw_stderr_matches,"disposition":if raw_stderr_matches{"pass"}else{"INCOMPLETE: native fixture stderr owner"}})).unwrap()).unwrap();
                } else {
                    eprintln!("GEO_REUSED_LIVE {id} {}", tape.display());
                }
                if live_requested && !raw_stderr_matches {
                    missing_live_stderr.push(format!(
                        "{id}:{}",
                        dir.file_name().unwrap().to_str().unwrap()
                    ));
                    eprintln!(
                        "GEO_INCOMPLETE {id} {} live raw stderr: public Fixture sink; exact tape stderr still required",
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
                } else if let Some(attempt) = row["write_attempt"].as_str() {
                    assert_eq!(writes, attempt.as_bytes());
                    let commits: Vec<_> = record
                        .events
                        .iter()
                        .filter(|e| matches!(e.request, hirsel_rt::io::Request::Commit { .. }))
                        .collect();
                    if row["faults"]["file.commit"].is_array() {
                        assert_eq!(commits.len(), 1);
                        assert_eq!(
                            commits[0].completion,
                            hirsel_rt::io::Completion::Unit(Err(hirsel_rt::io::IoError::Fault(77)))
                        );
                    } else {
                        assert!(commits.is_empty());
                        let fault = record
                            .events
                            .iter()
                            .find(|e| matches!(e.request, hirsel_rt::io::Request::WriteAll { .. }))
                            .unwrap()
                            .fault
                            .as_ref()
                            .unwrap();
                        assert_eq!(fault.partial, 5);
                        assert_eq!(fault.error, hirsel_rt::io::IoError::Fault(77));
                    }
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
                    file_faults: BTreeMap::new(),
                    signals: None,
                    ..world.clone()
                };
                let replay_tape = selection
                    .and_then(|r| r["replay_tape"].as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| tape.clone());
                let interpreter_replays_expected =
                    selection.map_or(2, |r| r["interpreters"].as_array().unwrap().len());
                let mut interpreter_replays_completed = 0u64;
                let mut native_replays_completed = 0u64;
                let reused_replays = std::env::var("GEOFENCE_AUDIT_34_COMPLETED_REPLAYS")
                    .unwrap_or_default()
                    .split(',')
                    .any(|s| s == format!("{id}:{}", dir.file_name().unwrap().to_str().unwrap()));
                if !reused_replays {
                    for (player, replay_dir) in &runnables {
                        if selection.is_some_and(|r| {
                            !r["native"].as_array().unwrap().iter().any(|p| {
                                p.as_str()
                                    == Some(replay_dir.file_name().unwrap().to_str().unwrap())
                            })
                        }) {
                            continue;
                        }

                        let replay_receipt = dir.join(format!(
                            "native-{}-replay.json",
                            replay_dir.file_name().unwrap().to_string_lossy()
                        ));
                        fs::write(&replay_receipt, serde_json::to_vec_pretty(&json!({"id":id,"source_backend":dir.file_name().unwrap().to_str(),"player":replay_dir.file_name().unwrap().to_str(),"selected":true,"started":true,"completed":false,"passed":false})).unwrap()).unwrap();
                        let replayed = launch_fixture(
                            player,
                            &ready.target,
                            &tools,
                            replay_dir,
                            true,
                            Tape::Replay(&replay_tape),
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
                        fs::write(&replay_receipt, serde_json::to_vec_pretty(&json!({"id":id,"source_backend":dir.file_name().unwrap().to_str(),"player":replay_dir.file_name().unwrap().to_str(),"selected":true,"started":true,"completed":true,"passed":true})).unwrap()).unwrap();
                        native_replays_completed += 1;
                    }
                    for (index, replay) in [
                        hirsel_driver::replay_observations,
                        hirsel_driver::replay_observations_checked,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let engine = ["reference", "checked-h"][index];
                        if selection.is_some_and(|r| {
                            !r["interpreters"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|p| p.as_str() == Some(engine))
                        }) {
                            continue;
                        }
                        let replayed = replay(&path, &replay_tape, Some(1_000_000));
                        let mut receipt = json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"engine":engine,"selected":true,"started":true,"completed":false,"passed":false});
                        match replayed {
                            Ok(value) => {
                                assert_eq!(Observation::of_reference(&value), observation);
                                receipt["completed"] = json!(true);
                                receipt["passed"] = json!(true);
                                interpreter_replays_completed += 1;
                            }
                            Err(error) => {
                                let message = error.to_string();
                                assert!(
                                    (message.contains("ToolingLimit") && message.contains("Steps"))
                                        || message.contains("header disagrees")
                                        || message.contains("revision"),
                                    "{id}/{engine}: {message}"
                                );
                                receipt["disposition"] = json!(format!("INCOMPLETE {message}"));
                            }
                        }
                        fs::write(
                            dir.join(format!("{engine}-replay-receipt.json")),
                            serde_json::to_vec_pretty(&receipt).unwrap(),
                        )
                        .unwrap();
                        eprintln!("GEO_REPLAY_RECEIPT {receipt}");
                    }
                } else {
                    eprintln!(
                        "GEO_REUSED_REPLAYS {id} immutable retained native+interpreter completions"
                    );
                }
                fs::write(root.join(format!("{id}-{}-replays-completed.json",dir.file_name().unwrap().to_str().unwrap())), serde_json::to_vec_pretty(&json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"tape":tape,"native_replays":native_replays_completed,"interpreter_replays":interpreter_replays_completed,"retained_reuse":reused_replays})).unwrap()).unwrap();
                if id == "geo-square" && dir.ends_with("Llvm") {
                    for kind in ["binding", "request", "missing", "extra"] {
                        if std::env::var("GEOFENCE_AUDIT_34_COMPLETED_CONTROLS")
                            .unwrap_or_default()
                            .split(',')
                            .any(|k| k == kind)
                        {
                            eprintln!("GEO_REUSED_CONTROL {kind}");
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
                            eprintln!("GEO_REFUSAL_PASS {kind} four engines");
                        } else {
                            controls_failed = true;
                            eprintln!(
                                "GEO_REFUSAL_FAIL {kind}; retained; continuing independent workflows"
                            );
                        }
                    }
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                receipts.push(json!({"id":id,"backend":dir.file_name().unwrap().to_str(),"live_selected":live_requested,"selected":true,"started":true,"completed":raw_stderr_matches,"passed":raw_stderr_matches,"live_stdout_status_file_and_tape_stderr":true,"live_raw_stderr_matches":raw_stderr_matches,"raw_stderr_owner":if !live_requested {"not selected"} else if raw_stderr_matches {"none"} else {"native-file-faults-33 / host-replay-parity-31"},"tape":tape,"native_replays":native_replays_completed,"interpreter_replays_completed":interpreter_replays_completed,"expected_provenance":"frozen rational parity + exact error oracle","interpreter_replay_disposition":if reused_replays || interpreter_replays_expected == 0 {"not selected; prior receipts retained"} else if interpreter_replays_completed < interpreter_replays_expected as u64 {"INCOMPLETE; see per-engine receipts"} else {"pass"}}));
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
                eprintln!(
                    "GEO_RECORDED {id} {} {native_replays_completed} native replays; live raw stderr and interpreter replay dispositions recorded separately",
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
fn geofence_audit_34_geometry_and_output_cap_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("helper-controls-");
        let path = package_entry(
            &root,
            include_str!("../support/geofence_audit_34/helper.hir"),
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
        for count in [16384, 16385] {
            let world = Fixture {
                files: BTreeMap::from([
                    ("data/probe".into(), vec![b'x'; count]),
                    ("data/output".into(), b"preserve".to_vec()),
                ]),
                mounts: BTreeMap::from([("data".into(), Access::ReadWrite)]),
                ..Default::default()
            };
            for checked in [false, true] {
                let e = if checked {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        (&world).into(),
                        1_000_000,
                    )
                } else {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        (&world).into(),
                        1_000_000,
                    )
                }
                .unwrap();
                let o = e.observation.expect("helper must complete");
                assert!(o.failures.is_empty(), "{:?}", o.failures);
                assert_eq!(o.exit_code, Some(0));
                let files = &o.final_state.as_ref().unwrap().trees[&0].1;
                assert_eq!(
                    files["data/output"],
                    if count == 16384 {
                        vec![b'x'; count]
                    } else {
                        b"preserve".to_vec()
                    }
                );
                assert_eq!(files.len(), 2);
                eprintln!("GEO_HELPER_PASS {count} checked={checked}");
            }
        }
    });
}

#[test]
#[ignore = "requires deployed exact-CPU admission; coordinator owns reruns"]
fn geofence_audit_34_ordered_capacity() {
    let root = scratch("ordered-");
    let _evidence = super::width_capacity_36::Evidence(root.clone());
    let manifest = package(&root);
    fs::write(
        root.join("ordered.py"),
        include_bytes!("../support/geofence_audit_34/ordered.py"),
    )
    .unwrap();
    let backend = std::env::var("GEOFENCE_AUDIT_34_BACKEND").expect("select llvm or cranelift");
    let width = std::env::var("GEOFENCE_AUDIT_34_WIDTH").expect("select width 1/2/4");
    let kind = std::env::var("GEOFENCE_AUDIT_34_KIND").expect("select success/domain/cancel");
    let output = Command::new("python3")
        .arg(root.join("capped"))
        .args(["--timeout", "60", "python3"])
        .arg(root.join("ordered.py"))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--manifest")
        .arg(manifest)
        .arg("--backend")
        .arg(backend)
        .arg("--root")
        .arg(root.join("cell"))
        .arg("--fixture-root")
        .arg(&root)
        .arg("--width")
        .arg(&width)
        .arg("--kind")
        .arg(kind)
        .output()
        .unwrap();
    fs::write(root.join("qualification-stdout"), &output.stdout).unwrap();
    fs::write(root.join("qualification-stderr"), &output.stderr).unwrap();
    assert!(output.status.success(), "{output:?}");
    let recording = hirsel_rt::host_launch::read_recording(&root.join("cell/recording")).unwrap();
    recording.bound_run().unwrap();
    let placement = recording
        .binding
        .as_ref()
        .unwrap()
        .placement
        .as_ref()
        .unwrap();
    let capacity = placement.proves(width.parse().unwrap());
    let path = root.join("cell/receipt.json");
    let mut receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    receipt["placement"] = serde_json::to_value(placement).unwrap();
    receipt["capacity"] = json!(capacity);
    fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    assert!(
        capacity,
        "INCOMPLETE: exact admission capacity missing: {placement:?}"
    );
}
