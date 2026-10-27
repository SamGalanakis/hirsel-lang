//! Actual six-module Io+Args main, independent frozen Decimal oracle and immutable replay tapes.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_run_schema::{Access, FileReadScript};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SOURCES: [(&str, &str); 6] = [
    (
        "main",
        include_str!("../../../../examples/spectralaudit/main.hir"),
    ),
    (
        "samples",
        include_str!("../../../../examples/spectralaudit/samples.hir"),
    ),
    (
        "transform",
        include_str!("../../../../examples/spectralaudit/transform.hir"),
    ),
    (
        "analysis",
        include_str!("../../../../examples/spectralaudit/analysis.hir"),
    ),
    (
        "verify",
        include_str!("../../../../examples/spectralaudit/verify.hir"),
    ),
    (
        "report",
        include_str!("../../../../examples/spectralaudit/report.hir"),
    ),
];
fn cases() -> Vec<Value> {
    let mut rows: Vec<Value> =
        serde_json::from_str(include_str!("../support/spectral_audit_35/cases.json")).unwrap();
    rows.extend(
        serde_json::from_str::<Vec<Value>>(include_str!(
            "../support/spectral_audit_35/additional-cases.json"
        ))
        .unwrap(),
    );
    rows
}
type Replay = fn(
    &Path,
    &Path,
    Option<u64>,
) -> Result<hirsel_interp::observation::Observable, hirsel_engine::ToolError>;
fn recovery() -> Value {
    if let Ok(encoded) = std::env::var("SPECTRAL_AUDIT_35_RECOVERY_GZIP") {
        let output=Command::new("python3").args(["-c", "import base64,gzip,sys;sys.stdout.buffer.write(gzip.decompress(base64.b64decode(sys.argv[1],validate=True)))", &encoded]).output().unwrap();
        assert!(
            output.status.success(),
            "recovery decode: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    } else {
        std::env::var("SPECTRAL_AUDIT_35_RECOVERY")
            .map(|s| serde_json::from_str(&s).unwrap())
            .unwrap_or_else(|_| json!({}))
    }
}
fn listed(value: &Value, item: &str) -> bool {
    value
        .as_array()
        .is_some_and(|values| values.iter().any(|v| v == item))
}
fn selected(row: &Value, ordinal: usize, part: usize) -> bool {
    if let Ok(selection) = std::env::var("SPECTRAL_AUDIT_35_CASE") {
        return part == 0 && selection.split(',').any(|id| row["id"] == id);
    }
    let cap = ["length-32", "input-cap"].contains(&row["id"].as_str().unwrap());
    if part == 6 {
        cap
    } else {
        !cap && ordinal % 6 == part
    }
}
fn scratch(label: &str) -> PathBuf {
    // Runtime files live on the worker's private tmpfs. Archive's Drop returns
    // them through the authorized bounded log transport even on test failure.
    let base = std::env::var_os("SPECTRAL_AUDIT_35_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep();
    fs::write(
        root.join("archive.py"),
        include_bytes!("../support/spectral_audit_35/archive.py"),
    )
    .unwrap();
    eprintln!("SPECTRAL_ARCHIVE {}", root.display());
    root
}
struct Archive(PathBuf);
impl Drop for Archive {
    fn drop(&mut self) {
        let status = Command::new("python3")
            .arg(self.0.join("archive.py"))
            .arg(&self.0)
            .status();
        if !status.is_ok_and(|s| s.success()) {
            eprintln!("SPECTRAL_ARCHIVE_FAILURE {}", self.0.display());
            if !std::thread::panicking() {
                panic!("evidence archive failed")
            }
        }
    }
}
fn package(root: &Path) -> PathBuf {
    package_entry(root, SOURCES[0].1)
}
fn package_entry(root: &Path, entry: &str) -> PathBuf {
    for (name, data) in SOURCES {
        fs::write(root.join(format!("{name}.hir")), data).unwrap();
    }
    fs::write(root.join("main.hir"), entry).unwrap();
    if std::env::var_os("SPECTRAL_AUDIT_35_RECOVERY_GZIP").is_some()
        || std::env::var_os("SPECTRAL_AUDIT_35_RECOVERY").is_some()
    {
        fs::write(
            root.join("recovery-input.json"),
            serde_json::to_vec_pretty(&recovery()).unwrap(),
        )
        .unwrap();
    }
    for (name, data) in [
        ("capped", include_str!("../../../../tools/capped")),
        (
            "run_capped.py",
            include_str!("../support/spectral_audit_35/run_capped.py"),
        ),
        (
            "seal.py",
            include_str!("../support/spectral_audit_35/seal.py"),
        ),
        (
            "cases.json",
            include_str!("../support/spectral_audit_35/cases.json"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../../../examples/spectralaudit/hirsel.json"
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
    let seal = Command::new("python3")
        .arg(root.join("seal.py"))
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .arg(root.join("closure"))
        .env("SPECTRAL_AUDIT_35_TRANSPORT", "stdout")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(
        seal.status.success(),
        "{}",
        String::from_utf8_lossy(&seal.stderr)
    );
    path
}
fn fixture(row: &Value) -> Fixture {
    let mut args = vec![
        "data".into(),
        "samples.json".into(),
        "audit.json".into(),
        row["jobs"].as_u64().unwrap_or(4).to_string(),
    ];
    if row["pending"] == true {
        args.push("--interruptible".into());
    }
    Fixture {
        args,
        files: BTreeMap::from([
            (
                "data/samples.json".into(),
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
            BTreeMap::from([("data/samples.json".into(), FileReadScript::Pending)])
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
fn stdout(row: &Value) -> Vec<u8> {
    if row["error"].is_string() {
        vec![]
    } else {
        let input: Value = serde_json::from_str(row["input"].as_str().unwrap()).unwrap();
        format!(
            "spectral: {} channels\n",
            input["channels"].as_array().unwrap().len()
        )
        .into_bytes()
    }
}
fn stderr(row: &Value) -> Vec<u8> {
    row["error"]
        .as_str()
        .map(|e| format!("spectral: {e}\n").into_bytes())
        .unwrap_or_default()
}
fn verify_report(row: &Value, report: &[u8]) {
    assert!(report.len() <= 16384 && report.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(report).contains(":-0.000000"));
    let actual: Value = serde_json::from_slice(report).unwrap();
    let channels = actual["channels"].as_array().unwrap();
    let expected = row["oracle"]
        .as_array()
        .expect("every successful fixture has a frozen independent oracle");
    assert_eq!(channels.len(), expected.len());
    for (ordinal, (ch, oracle)) in channels.iter().zip(expected).enumerate() {
        assert_eq!(ch["ordinal"], ordinal);
        let bins = ch["bins"].as_array().unwrap();
        assert_eq!(bins.len(), oracle["bins"].as_array().unwrap().len());
        for (bin, intervals) in bins.iter().zip(oracle["bins"].as_array().unwrap()) {
            for (field, interval) in ["real", "imaginary"]
                .into_iter()
                .zip(intervals.as_array().unwrap())
            {
                let value = bin[field].as_f64().unwrap();
                let lo: f64 = interval[0].as_str().unwrap().parse().unwrap();
                let hi: f64 = interval[1].as_str().unwrap().parse().unwrap();
                // Rendering loses at most half a decimal micro-unit. The
                // unrounded algorithm intervals are checked separately below.
                assert!(
                    value >= lo - 0.0000005 && value <= hi + 0.0000005,
                    "{} {field}: {value} outside {lo}..{hi}",
                    row["id"]
                );
            }
        }
        let lo: f64 = oracle["energy"][0].as_str().unwrap().parse().unwrap();
        let hi: f64 = oracle["energy"][1].as_str().unwrap().parse().unwrap();
        for field in ["time_energy", "frequency_energy"] {
            let value = ch[field].as_f64().unwrap();
            assert!(
                value >= lo - 0.0000005 && value <= hi + 0.0000005,
                "{} {field}: {value}",
                row["id"]
            );
        }
        assert_eq!(ch["reconstruction_error"], 0.0);
        assert_eq!(ch["energy_error"], 0.0);
        if ["impulse", "zero", "mean"].contains(&row["id"].as_str().unwrap()) {
            assert_eq!(ch["peaks"], json!((0..bins.len()).collect::<Vec<_>>()));
        }
    }
}
fn reference_checked_baselines(
    root: &Path,
    path: &Path,
    part: usize,
) -> (BTreeMap<String, Vec<u8>>, Vec<String>) {
    let mut loaded = hirsel_driver::load_program(path).unwrap();
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{e}"));
    let run = RunConfiguration {
        entry: loaded.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut receipts = vec![];
    let mut incomplete = vec![];
    let recovery = recovery();
    let mut reused = 0;
    let mut reports: BTreeMap<String, Vec<u8>> = recovery
        .get("reports")
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .unwrap_or_default();
    for (_, row) in cases()
        .iter()
        .enumerate()
        .filter(|(index, r)| selected(r, *index, part))
    {
        let world = fixture(row);
        let mut previous = None;
        for checked in [false, true] {
            let engine = if checked { "checked-h" } else { "reference" };
            let id = row["id"].as_str().unwrap();
            if listed(&recovery["simulation_stops"], &format!("{id}/{engine}")) {
                incomplete.push(format!("{id}/{engine}: retained original Steps1M"));
                reused += 1;
                eprintln!("SPECTRAL_RETAINED_STOP {id}/{engine} Steps1M");
                continue;
            }
            if listed(&recovery["simulations"], &format!("{id}/{engine}")) {
                eprintln!("SPECTRAL_REUSED {id}/{engine}");
                reused += 1;
                continue;
            }
            eprintln!("SPECTRAL_STARTED {id} {engine}");
            let execution = if checked {
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
            }
            .unwrap();
            let mut receipt = json!({"id":id,"engine":engine,"selected":true,"started":true,"completed":false,"passed":false});
            receipt["outcome"] = json!(format!("{:?}", execution.outcome));
            if let Some(observed) = execution.observation {
                assert!(
                    observed.failures.is_empty(),
                    "{id}: {:?}",
                    observed.failures
                );
                assert!(observed.blocked.is_none());
                assert_eq!(
                    observed.exit_code,
                    Some(if row["error"].is_string() { 2 } else { 0 }),
                    "{id}"
                );
                assert_eq!(observed.output, stdout(row), "{id}");
                let state = observed.final_state.as_ref().unwrap();
                assert_eq!(state.stderr, stderr(row), "{id}");
                let mut expected = world.files.clone();
                if !row["error"].is_string() {
                    let output = &state.trees[&0].1["data/audit.json"];
                    verify_report(row, output);
                    reports.insert(id.to_owned(), output.clone());
                    expected.insert("data/audit.json".into(), output.clone());
                }
                assert_eq!(
                    state.trees[&0].1, expected,
                    "temporary leaked or file changed: {id}"
                );
                if row["pending"] == true {
                    assert!(observed.effects.iter().any(|e| matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation=="file.read.pending")));
                }
                if let Some(prior) = previous {
                    assert_eq!(observed, prior, "{id}: complete observation");
                }
                fs::write(
                    root.join(format!("{id}-{engine}-observation.txt")),
                    format!("{observed:#?}"),
                )
                .unwrap();
                previous = Some(observed);
                receipt["completed"] = json!(true);
                receipt["passed"] = json!(true);
            } else {
                incomplete.push(format!("{id}/{engine}: {:?}", execution.outcome));
            }
            eprintln!("SPECTRAL_RECEIPT {receipt}");
            receipts.push(receipt);
            fs::write(
                root.join("receipts.json"),
                serde_json::to_vec_pretty(&receipts).unwrap(),
            )
            .unwrap();
        }
    }
    assert!(!receipts.is_empty() || reused > 0);
    (reports, incomplete)
}

#[test]
fn spectral_audit_35_native_four_engine_replay() {
    native_workflows(0)
}
#[test]
fn spectral_audit_35_native_part_1() {
    native_workflows(1)
}
#[test]
fn spectral_audit_35_native_part_2() {
    native_workflows(2)
}
#[test]
fn spectral_audit_35_native_part_3() {
    native_workflows(3)
}
#[test]
fn spectral_audit_35_native_part_4() {
    native_workflows(4)
}
#[test]
fn spectral_audit_35_native_part_5() {
    native_workflows(5)
}
#[test]
#[ignore = "original length-32 and input-cap Steps1M obligations remain INCOMPLETE; never raise their caps"]
fn spectral_audit_35_original_caps() {
    native_workflows(6)
}

fn native_workflows(part: usize) {
    if part != 0 && std::env::var_os("SPECTRAL_AUDIT_35_CASE").is_some() {
        return;
    }
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("native-");
        let _archive = Archive(root.clone());
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
        let objects = [Backend::Llvm, Backend::Cranelift].map(|b| {
            (
                b,
                emit_backend_objects(&ready, b, OptLevel::None, Partition::Whole, false, None)
                    .unwrap(),
            )
        });
        let (baselines, mut incomplete) = reference_checked_baselines(&root, &path, part);
        let recovery = recovery();
        let mut count = 0;
        for (_, row) in cases()
            .iter()
            .enumerate()
            .filter(|(index, r)| selected(r, *index, part))
        {
            let id = row["id"].as_str().unwrap();
            let world = fixture(row);
            let cell = root.join(id);
            fs::create_dir(&cell).unwrap();
            let expected = Observation {
                exit_code: Some(if row["error"].is_string() { 2 } else { 0 }),
                blocked: false,
                budget: None,
                signaled: None,
                output: stdout(row),
                failures: vec![],
                roots: vec![],
            };
            fs::write(cell.join("fixture.json"),serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"file_faults":world.file_faults,"signals":world.signals})).unwrap()).unwrap();
            if ["rustfft-2", "component-over", "denial", "pending-cancel"].contains(&id)
                && !listed(&recovery["differential"], id)
            {
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
            let mut runnables = vec![];
            for (backend, emitted) in &objects {
                let dir = cell.join(format!("{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, emitted, &tools, &dir, &world)
                        .unwrap();
                runnables.push((runnable, dir));
            }
            let mut baseline = None;
            for (runnable, dir) in &runnables {
                let backend_name = dir.file_name().unwrap().to_str().unwrap();
                if listed(&recovery["completed_backends"][id], backend_name) {
                    if let Some(bytes) = recovery["recordings"][id][backend_name].get("tape") {
                        let bytes: Vec<u8> = serde_json::from_value(bytes.clone()).unwrap();
                        fs::write(dir.join("historical-incompatible.tape"), &bytes).unwrap();
                        let old = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                        baseline = Some(
                            old.events
                                .iter()
                                .filter_map(|e| match &e.request {
                                    hirsel_rt::io::Request::WriteAll { bytes, .. } => {
                                        Some(bytes.clone())
                                    }
                                    _ => None,
                                })
                                .flatten()
                                .collect(),
                        );
                    }
                    eprintln!("SPECTRAL_RETAINED_HISTORICAL_NATIVE {id} {backend_name}");
                    continue;
                }
                let tape = dir.join("recording");
                fs::write(
                    dir.join("receipt.json"),
                    b"{\"selected\":true,\"started\":true,\"completed\":false,\"passed\":false}",
                )
                .unwrap();
                eprintln!("SPECTRAL_NATIVE_STARTED {id} {}", dir.display());
                let retained =
                    &recovery["recordings"][id][dir.file_name().unwrap().to_str().unwrap()];
                if let Some(bytes) = retained.get("tape") {
                    let bytes: Vec<u8> = serde_json::from_value(bytes.clone()).unwrap();
                    fs::write(&tape, bytes).unwrap();
                    eprintln!("SPECTRAL_REUSED_LIVE {id} {}", dir.display());
                } else {
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
                        dir.join("streams.json"),
                        serde_json::to_vec_pretty(
                            &json!({"exit":live.exit,"stdout":live.stdout,"stderr":live.stderr}),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(observe(&live, &ready, &sites).unwrap(), expected, "{id}");
                    assert_eq!(live.stderr, stderr(row), "{id} raw stderr");
                }
                let frozen = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                assert_eq!(record.arguments, world.args);
                record.bound_run().unwrap();
                let output: Vec<u8> = record
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                if !row["error"].is_string() {
                    verify_report(row, &output);
                    if let Some(reference) = baselines.get(id) {
                        assert_eq!(&output, reference, "{id}: four-engine byte parity");
                    }
                    assert_eq!(
                        record
                            .events
                            .iter()
                            .filter(|e| matches!(e.request, hirsel_rt::io::Request::Commit { .. }))
                            .count(),
                        1
                    );
                } else if row.get("faults").is_none() {
                    assert!(output.is_empty());
                }
                if let Some(attempt) = row["write_attempt"].as_str() {
                    assert_eq!(
                        output,
                        attempt.as_bytes(),
                        "{id}: exact fault request count/payload"
                    );
                }
                if let Some(prior) = &baseline {
                    assert_eq!(&output, prior, "{id}: byte-identical backend reports");
                }
                baseline = Some(output);
                if row["pending"] == true {
                    assert!(record.events.iter().any(|e| matches!(
                        (&e.request, &e.completion),
                        (
                            hirsel_rt::io::Request::FileRead { .. },
                            hirsel_rt::io::Completion::Blocked { committed: false }
                        )
                    )));
                }
                if id == "rustfft-2" && dir.ends_with("Llvm") && retained["controls"] != true {
                    for kind in ["binding", "request", "missing", "extra"] {
                        let mut changed = record.clone();
                        let expected_error = match kind {
                            "binding" => {
                                changed.arguments.push("different".into());
                                "header disagrees"
                            }
                            "request" => {
                                let path = changed
                                    .events
                                    .iter_mut()
                                    .find_map(|e| match &mut e.request {
                                        hirsel_rt::io::Request::Directory { path, .. } => {
                                            Some(path)
                                        }
                                        _ => None,
                                    })
                                    .unwrap();
                                path.push_str("-different");
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
                        let bad = dir.join(format!("refused-{kind}.tape"));
                        fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap())
                            .unwrap();
                        for replay in [
                            hirsel_driver::replay_observations,
                            hirsel_driver::replay_observations_checked,
                        ] {
                            let error = replay(&path, &bad, Some(1_000_000))
                                .unwrap_err()
                                .to_string();
                            assert!(error.contains(expected_error), "{kind}: {error}");
                        }
                        for (player, replay_dir) in &runnables {
                            match launch_fixture(
                                player,
                                &ready.target,
                                &tools,
                                replay_dir,
                                true,
                                Tape::Replay(&bad),
                                &world,
                            ) {
                                Err(error) => assert!(
                                    error.message.contains(expected_error),
                                    "{kind}: {error}"
                                ),
                                Ok(refused) => {
                                    assert_eq!(refused.exit, Some(3));
                                    assert!(
                                        format!("{:?}", refused.record).contains(expected_error),
                                        "{kind}: {:?}",
                                        refused.record
                                    );
                                    let prefix = if kind == "extra" {
                                        expected.output.as_slice()
                                    } else {
                                        b""
                                    };
                                    assert_eq!(
                                        refused.stdout, prefix,
                                        "{kind}: exact valid stdout prefix"
                                    );
                                }
                            }
                        }
                    }
                }
                let offline = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    file_faults: BTreeMap::new(),
                    signals: None,
                    ..world.clone()
                };
                for (player, replay_dir) in &runnables {
                    let player_name = replay_dir.file_name().unwrap().to_str().unwrap();
                    if listed(&retained["native_replays"], player_name) {
                        eprintln!("SPECTRAL_REUSED_NATIVE_REPLAY {id} {player_name}");
                        continue;
                    }
                    let replayed = launch_fixture(
                        player,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &offline,
                    )
                    .unwrap();
                    assert_eq!(observe(&replayed, &ready, &sites).unwrap(), expected);
                    assert_eq!(replayed.stderr, stderr(row));
                    assert!(
                        fs::read_dir(replay_dir.join("root"))
                            .unwrap()
                            .next()
                            .is_none(),
                        "offline host mutation"
                    );
                }
                let mut interpreter_completed = 0;
                for (engine, replay) in [
                    ("reference", hirsel_driver::replay_observations as Replay),
                    (
                        "checked-h",
                        hirsel_driver::replay_observations_checked as Replay,
                    ),
                ] {
                    let receipt = dir.join(format!("{engine}-replay.json"));
                    if listed(&retained["interpreter_replays"], engine) {
                        interpreter_completed += 1;
                        eprintln!("SPECTRAL_REUSED_INTERPRETER_REPLAY {id} {engine}");
                        continue;
                    }
                    if listed(&retained["interpreter_stops"], engine) {
                        incomplete.push(format!("{id}/{engine} replay: retained original Steps1M"));
                        eprintln!("SPECTRAL_RETAINED_REPLAY_STOP {id} {engine}");
                        continue;
                    }
                    fs::write(&receipt,serde_json::to_vec_pretty(&json!({"id":id,"engine":engine,"selected":true,"started":true,"completed":false,"passed":false,"bound":1_000_000})).unwrap()).unwrap();
                    match replay(&path, &tape, Some(1_000_000)) {
                        Ok(observed) => {
                            assert_eq!(Observation::of_reference(&observed), expected);
                            let recorded_stderr: Vec<u8> = record
                                .events
                                .iter()
                                .filter_map(|e| match &e.request {
                                    hirsel_rt::io::Request::PrintStderr { bytes, .. } => {
                                        Some(bytes.clone())
                                    }
                                    _ => None,
                                })
                                .flatten()
                                .collect();
                            assert_eq!(recorded_stderr, stderr(row));
                            interpreter_completed += 1;
                            fs::write(&receipt,serde_json::to_vec_pretty(&json!({"id":id,"engine":engine,"selected":true,"started":true,"completed":true,"passed":true,"bound":1_000_000})).unwrap()).unwrap();
                        }
                        Err(error) => {
                            let message = error.to_string();
                            assert!(
                                message.contains("ToolingLimit")
                                    && message.contains("kind: \"Steps\", limit: 1000000"),
                                "{id}/{engine}: {message}"
                            );
                            fs::write(&receipt,serde_json::to_vec_pretty(&json!({"id":id,"engine":engine,"selected":true,"started":true,"completed":false,"passed":false,"bound":1_000_000,"disposition":"INCOMPLETE: Steps","error":message})).unwrap()).unwrap();
                            incomplete.push(format!("{id}/{engine}: {message}"));
                            eprintln!("SPECTRAL_REPLAY_INCOMPLETE {id} {engine}: {message}");
                        }
                    }
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                fs::write(dir.join("receipt.json"),serde_json::to_vec_pretty(&json!({"selected":true,"started":true,"completed":true,"passed":true,"native_replays":2,"interpreter_replays":interpreter_completed,"replay_complete":interpreter_completed==2})).unwrap()).unwrap();
                count += 1;
                eprintln!("SPECTRAL_NATIVE_PASS {id} {}", dir.display());
            }
        }
        assert!(count > 0);
        assert!(
            incomplete.is_empty(),
            "INCOMPLETE: {}",
            incomplete.join("\n")
        );
    });
}

#[test]
#[ignore = "original aggregate oracle reference Steps1M; explicit missing-engine selection preserves the stop"]
fn spectral_audit_35_unrounded_oracle() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("kernel-");
        let _archive = Archive(root.clone());
        let path = package_entry(
            &root,
            include_str!("../support/spectral_audit_35/kernel.hir"),
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
        let engines = std::env::var("SPECTRAL_AUDIT_35_ORACLE_ENGINE")
            .unwrap_or_else(|_| "reference,checked-h,llvm,cranelift".into());
        let mut incomplete = vec![];
        for checked in [false, true] {
            let engine = if checked { "checked-h" } else { "reference" };
            if !engines.split(',').any(|e| e == engine) {
                continue;
            }
            let execution = if checked {
                hirsel_engine::run_observed_checked_bounded(
                    &mut loaded.engine,
                    &run,
                    SimulationConfig::default(),
                    1_000_000,
                )
            } else {
                hirsel_engine::run_observed_snapshot_bounded(
                    &mut loaded.engine,
                    &run,
                    SimulationConfig::default(),
                    1_000_000,
                )
            }
            .unwrap();
            let observed = match execution.observation {
                Some(value) => value,
                None => {
                    assert!(
                        matches!(&execution.outcome,Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind=="Steps" && limit.limit==1_000_000)
                    );
                    let message = format!("INCOMPLETE {engine}: {:?}", execution.outcome);
                    fs::write(root.join(format!("{engine}-oracle-stop.txt")), &message).unwrap();
                    eprintln!("SPECTRAL_ORACLE_STOP {message}");
                    incomplete.push(message);
                    continue;
                }
            };
            assert!(observed.failures.is_empty(), "{:?}", observed.failures);
            assert_eq!(observed.output, b"kernel oracle ok\n");
            eprintln!("SPECTRAL_UNROUNDED_PASS checked={checked}");
        }
        let exported = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(exported).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let mut matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        matrix.retain(|c| match c.engine {
            Engine::Native { .. } => engines.split(',').any(|e| e == "llvm"),
            Engine::Cranelift { .. } => engines.split(',').any(|e| e == "cranelift"),
            _ => false,
        });
        assert!(
            !matrix.is_empty(),
            "select at least one missing native oracle engine"
        );
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.wrapper = vec![
            "python3".into(),
            root.join("run_capped.py").to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
        ];
        tools.launch_timeout = std::time::Duration::from_secs(65);
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: b"kernel oracle ok\n".to_vec(),
            failures: vec![],
            roots: vec![],
        };
        for (name, outcome) in differential_in(
            &input,
            &loaded.entry,
            &expected,
            &matrix,
            &tools,
            &root,
            &Fixture::default(),
        ) {
            assert_eq!(outcome, Outcome::Agree, "unrounded oracle: {name}");
            eprintln!("SPECTRAL_UNROUNDED_NATIVE_PASS {name}");
        }
        assert!(incomplete.is_empty(), "{}", incomplete.join("\n"));
    });
}

#[test]
#[ignore = "explicit exact4 pool capacity cells; select backend/width/kind"]
fn spectral_audit_35_ordered_capacity() {
    let root = scratch("ordered-");
    let _archive = Archive(root.clone());
    let manifest = package(&root);
    fs::write(
        root.join("ordered.py"),
        include_bytes!("../support/spectral_audit_35/ordered.py"),
    )
    .unwrap();
    let backend = std::env::var("SPECTRAL_AUDIT_35_BACKEND").expect("llvm/cranelift");
    let width = std::env::var("SPECTRAL_AUDIT_35_WIDTH").expect("1/2/4");
    let kind = std::env::var("SPECTRAL_AUDIT_35_KIND").expect("success/domain/cancel");
    let o = Command::new("python3")
        .arg(root.join("capped"))
        .args([
            "--timeout",
            "60",
            "prlimit",
            "--cpu=60:61",
            "--data=2147483648:2147483648",
            "python3",
        ])
        .env("HIRSEL_MEMORY_LIMIT", "2G")
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
        .arg(width)
        .arg("--kind")
        .arg(kind)
        .output()
        .unwrap();
    fs::write(root.join("qualification-stdout"), &o.stdout).unwrap();
    fs::write(root.join("qualification-stderr"), &o.stderr).unwrap();
    assert!(o.status.success(), "{o:?}");
}

#[test]
fn spectral_audit_35_output_cap_helpers() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("output-bounds-");
        let _archive = Archive(root.clone());
        let path = package_entry(
            &root,
            include_str!("../support/spectral_audit_35/output_bounds.hir"),
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
                args: vec![count.to_string()],
                files: BTreeMap::from([("data/audit.json".into(), b"preserve".to_vec())]),
                mounts: BTreeMap::from([("data".into(), Access::ReadWrite)]),
                ..Default::default()
            };
            for checked in [false, true] {
                let execution = if checked {
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
                let observed = execution.observation.unwrap_or_else(|| {
                    panic!("INCOMPLETE output helper {count}: {:?}", execution.outcome)
                });
                assert!(observed.failures.is_empty(), "{:?}", observed.failures);
                let files = &observed.final_state.as_ref().unwrap().trees[&0].1;
                assert_eq!(files.len(), 1);
                assert_eq!(
                    files["data/audit.json"],
                    if count == 16384 {
                        vec![b'x'; 16384]
                    } else {
                        b"preserve".to_vec()
                    }
                );
                eprintln!("SPECTRAL_OUTPUT_HELPER_PASS {count} checked={checked}");
            }
        }
    });
}

// ordered_qualification.rs's documented semantic projection: admission time
// and placement provenance alone may differ; keep every event and payload.
fn semantic_record(mut record: hirsel_rt::launch::Recording) -> hirsel_rt::launch::Recording {
    let binding = record.binding.as_mut().unwrap();
    binding.admitted_at = 0;
    binding.placement = None;
    record
}
#[test]
fn spectral_audit_35_scripted_ordered_streams() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("scripted-ordered-");
        let _archive = Archive(root.clone());
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let exported = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(exported).unwrap();
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
        let recovery = recovery();
        let mut baseline: BTreeMap<String, hirsel_rt::launch::Recording> = BTreeMap::new();
        if let Some(values) = recovery["scripted_baselines"].as_object() {
            for (id, bytes) in values {
                let bytes: Vec<u8> = serde_json::from_value(bytes.clone()).unwrap();
                baseline.insert(
                    id.clone(),
                    semantic_record(hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap()),
                );
            }
        }
        let mut count = 0;
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let emitted = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            for width in [1, 2, 4] {
                for id in ["channels-4", "component-over", "pending-cancel"] {
                    let identity = format!("{backend:?}:{width}:{id}");
                    if listed(&recovery["scripted_completed"], &identity) {
                        continue;
                    }
                    let row = cases().into_iter().find(|r| r["id"] == id).unwrap();
                    let world = fixture(&row);
                    let dir = root.join(&identity);
                    fs::create_dir(&dir).unwrap();
                    let runnable = link_fixture_runnable(
                        &ready,
                        &loaded.entry,
                        &emitted,
                        &tools,
                        &dir,
                        &world,
                    )
                    .unwrap();
                    // run.json is the public validated run format read by launch_fixture.
                    // Placement is excluded from the program's semantic identity.
                    let run = dir.join("run.json");
                    let mut value: Value =
                        serde_json::from_slice(&fs::read(&run).unwrap()).unwrap();
                    value["placement"] = json!({"cores":width,"commit":"shared","reorder":"off"});
                    fs::write(&run, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
                    hirsel_run_config::RunConfig::read_path(&run).unwrap();
                    fs::write(dir.join("receipt.json"),b"{\"selected\":true,\"started\":true,\"completed\":false,\"passed\":false}").unwrap();
                    let tape = dir.join("recording");
                    let live = launch_fixture(
                        &runnable,
                        &ready.target,
                        &tools,
                        &dir,
                        true,
                        Tape::Live {
                            record: Some(&tape),
                            revocations: None,
                        },
                        &world,
                    )
                    .unwrap();
                    assert_eq!(live.stdout, stdout(&row));
                    assert_eq!(live.stderr, stderr(&row));
                    let expected = Observation {
                        exit_code: Some(if row["error"].is_string() { 2 } else { 0 }),
                        blocked: false,
                        budget: None,
                        signaled: None,
                        output: stdout(&row),
                        failures: vec![],
                        roots: vec![],
                    };
                    assert_eq!(observe(&live, &ready, &sites).unwrap(), expected);
                    let frozen = fs::read(&tape).unwrap();
                    let record = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                    let placement = record
                        .binding
                        .as_ref()
                        .unwrap()
                        .placement
                        .as_ref()
                        .expect("record must retain requested placement");
                    assert_eq!(
                        placement.request.cores,
                        hirsel_run_config::Cores::Fixed(width)
                    );
                    assert_eq!(
                        placement.request.reorder,
                        Some(hirsel_run_config::Reorder::Off)
                    );
                    if id == "pending-cancel" {
                        assert!(record.events.iter().any(|e| matches!(
                            (&e.request, &e.completion),
                            (
                                hirsel_rt::io::Request::FileRead { .. },
                                hirsel_rt::io::Completion::Blocked { committed: false }
                            )
                        )));
                    }
                    if id == "channels-4" {
                        let output: Vec<u8> = record
                            .events
                            .iter()
                            .filter_map(|e| match &e.request {
                                hirsel_rt::io::Request::WriteAll { bytes, .. } => {
                                    Some(bytes.clone())
                                }
                                _ => None,
                            })
                            .flatten()
                            .collect();
                        verify_report(&row, &output);
                    }
                    let semantic = semantic_record(record);
                    if let Some(previous) = baseline.get(id) {
                        assert_eq!(
                            &semantic, previous,
                            "{identity}: full scripted event stream"
                        );
                    } else {
                        baseline.insert(id.into(), semantic);
                    }
                    let offline = Fixture {
                        files: BTreeMap::new(),
                        file_reads: BTreeMap::new(),
                        signals: None,
                        ..world.clone()
                    };
                    let replayed = launch_fixture(
                        &runnable,
                        &ready.target,
                        &tools,
                        &dir,
                        true,
                        Tape::Replay(&tape),
                        &offline,
                    )
                    .unwrap();
                    assert_eq!(observe(&replayed, &ready, &sites).unwrap(), expected);
                    assert_eq!(replayed.stderr, stderr(&row));
                    assert!(fs::read_dir(dir.join("root")).unwrap().next().is_none());
                    assert_eq!(fs::read(&tape).unwrap(), frozen);
                    fs::write(dir.join("receipt.json"),serde_json::to_vec_pretty(&json!({"identity":identity,"selected":true,"started":true,"completed":true,"passed":true,"semantic_events_equal":true,"replayed":true})).unwrap()).unwrap();
                    eprintln!("SPECTRAL_SCRIPTED_ORDERED_PASS {identity}");
                    count += 1;
                }
            }
        }
        assert!(count > 0);
    });
}

// Additional controls over the exact frozen original vectors. These do not
// qualify or rerun the original aggregate Steps1M obligation.
#[test]
fn spectral_audit_35_unrounded_vector_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("oracle-vectors-");
        let _archive = Archive(root.clone());
        let path = package_entry(
            &root,
            include_str!("../support/spectral_audit_35/kernel_cases.hir"),
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
        let mut count = 0;
        let mut incomplete = vec![];
        for row in cases().into_iter().filter(|r| {
            r["oracle"].is_array()
                && r["error"].is_null()
                && r["input"].as_str().unwrap().len() <= 4096
                && !r["id"].as_str().unwrap().starts_with("round-half")
        }) {
            let id = row["id"].as_str().unwrap();
            let world = Fixture {
                args: vec![id.into()],
                ..Default::default()
            };
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "reference" };
                let execution = if checked {
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
                let receipt = match execution.observation {
                    Some(observed) => {
                        assert!(
                            observed.failures.is_empty(),
                            "{id}/{engine}: {:?}",
                            observed.failures
                        );
                        assert!(observed.blocked.is_none());
                        assert_eq!(observed.exit_code, Some(0));
                        assert_eq!(observed.output, b"kernel oracle ok\n");
                        eprintln!("SPECTRAL_VECTOR_CONTROL_PASS {id} {engine}");
                        json!({"id":id,"engine":engine,"additional_control":true,"completed":true,"passed":true,"bound":1_000_000})
                    }
                    None => {
                        let message = format!("{:?}", execution.outcome);
                        incomplete.push(format!("{id}/{engine}: {message}"));
                        json!({"id":id,"engine":engine,"additional_control":true,"completed":false,"passed":false,"bound":1_000_000,"outcome":message})
                    }
                };
                fs::write(
                    root.join(format!("{id}-{engine}.json")),
                    serde_json::to_vec_pretty(&receipt).unwrap(),
                )
                .unwrap();
                count += 1;
            }
        }
        assert!(count > 0);
        assert!(
            incomplete.is_empty(),
            "INCOMPLETE controls: {}",
            incomplete.join("\n")
        );
    });
}

#[test]
fn evidence_archive_preserves_spectral_payloads_within_the_transport_limit() {
    let scratch = tempfile::tempdir().unwrap();
    let scripts = [(
        "spectral.py",
        include_str!("../support/spectral_audit_35/archive.py"),
    )];
    let mut command = Command::new("python3");
    command
        .arg("-c")
        .arg(include_str!("../support/archive_transport.py"));
    for (name, source) in scripts {
        let path = scratch.path().join(name);
        fs::write(&path, source).unwrap();
        command.arg(path);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
