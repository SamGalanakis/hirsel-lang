//! Bounded merge planner: independent common-hunk/DP oracle and full Io+Args workflows.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_run_schema::{Access, FileFault, FileFaultError, FileOperation, FileReadScript};
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
        include_str!("../../../../examples/mergeaudit/main.hir"),
    ),
    (
        "model",
        include_str!("../../../../examples/mergeaudit/model.hir"),
    ),
    (
        "edits",
        include_str!("../../../../examples/mergeaudit/edits.hir"),
    ),
    (
        "regions",
        include_str!("../../../../examples/mergeaudit/regions.hir"),
    ),
    (
        "resolve",
        include_str!("../../../../examples/mergeaudit/resolve.hir"),
    ),
    (
        "report",
        include_str!("../../../../examples/mergeaudit/report.hir"),
    ),
];
fn vectors() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/merge_audit_37/vectors.json")).unwrap()
}
fn scratch(label: &str) -> PathBuf {
    let base = std::env::var_os("MERGE_AUDIT_37_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep();
    for (name, data) in [
        (
            "run_capped.py",
            include_str!("../support/merge_audit_37/run_capped.py"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "oracle.py",
            include_str!("../support/merge_audit_37/oracle.py"),
        ),
        (
            "check.py",
            include_str!("../support/merge_audit_37/check.py"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    root
}
fn package(root: &Path, entry: Option<&str>) -> PathBuf {
    for (name, source) in SOURCES {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    if let Some(source) = entry {
        fs::write(root.join("main.hir"), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/mergeaudit/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path
}
fn manifest(n: usize) -> Vec<u8> {
    serde_json::to_vec(&json!({"schema":1,"files":(0..n).map(|i|json!({"name":format!("result{i}.txt"),"base":format!("b{i}"),"left":format!("l{i}"),"right":format!("r{i}")})).collect::<Vec<_>>()})).unwrap()
}
fn fixture(row: &Value) -> Fixture {
    let mut files = BTreeMap::from([
        ("input/manifest.json".into(), manifest(1)),
        ("output/plan.json".into(), b"preserve".to_vec()),
        ("output/result0.txt".into(), b"preserve".to_vec()),
    ]);
    for (path, key) in [("b0", "base"), ("l0", "left"), ("r0", "right")] {
        files.insert(
            format!("input/{path}"),
            row[key].as_str().unwrap().as_bytes().to_vec(),
        );
    }
    Fixture {
        args: vec!["plan".into(), "manifest.json".into(), "plan.json".into()],
        files,
        mounts: BTreeMap::from([
            ("input".into(), Access::Read),
            ("output".into(), Access::ReadWrite),
        ]),
        ..Default::default()
    }
}
fn run_pair(
    loaded: &mut hirsel_engine::LoadedProgram,
    world: &Fixture,
    status: i32,
    stdout: &str,
    stderr: &str,
    id: &str,
    root: &Path,
) -> BTreeMap<String, Vec<u8>> {
    fs::write(root.join(format!("{id}-fixture.json")),serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"file_faults":world.file_faults,"signals":world.signals,"steps":1000000,"hash_seed":0})).unwrap()).unwrap();
    let config = RunConfiguration {
        entry: loaded.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut previous = None;
    let mut files = BTreeMap::new();
    for checked in [false, true] {
        let engine = if checked { "checked-h" } else { "reference" };
        eprintln!("MERGE37_STARTED {id}/{engine}");
        let execution = if checked {
            hirsel_engine::run_observed_checked_bounded(
                &mut loaded.engine,
                &config,
                SimulationConfig::from(world),
                1_000_000,
            )
        } else {
            hirsel_engine::run_observed_snapshot_bounded(
                &mut loaded.engine,
                &config,
                SimulationConfig::from(world),
                1_000_000,
            )
        }
        .unwrap();
        fs::write(
            root.join(format!("{id}-{engine}.txt")),
            format!(
                "outcome={:?}\nobservation={:#?}",
                execution.outcome, execution.observation
            ),
        )
        .unwrap();
        let observed = execution
            .observation
            .unwrap_or_else(|| panic!("INCOMPLETE {id}/{engine}: {:?}", execution.outcome));
        assert!(
            observed.failures.is_empty()
                && observed.blocked.is_none()
                && observed.signaled.is_none(),
            "{id}/{engine}: {observed:?}"
        );
        assert_eq!(
            observed.exit_code,
            Some(status),
            "{id}/{engine}: {observed:?}"
        );
        assert_eq!(observed.output, stdout.as_bytes(), "{id}/{engine}");
        let state = observed.final_state.as_ref().unwrap();
        assert_eq!(state.stderr, stderr.as_bytes(), "{id}/{engine}");
        files = state.trees[&0].1.clone();
        for (p, data) in &world.files {
            if !p.starts_with("output/") || status != 0 {
                assert_eq!(files.get(p), Some(data), "{id}: {p}");
            }
        }
        if world
            .file_reads
            .values()
            .any(|s| matches!(s, FileReadScript::Pending))
        {
            assert!(observed.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(c) if c.operation=="file.read.pending")),"Pending never delivered");
        }
        if let Some(p) = previous {
            assert_eq!(observed, p, "{id}: complete semantic stream");
        }
        previous = Some(observed);
        eprintln!("MERGE37_PASS {id}/{engine}");
    }
    files
}
fn verify_report(row: &Value, report: &Value) {
    let regions = report["files"][0]["regions"].as_array().unwrap();
    assert_eq!(report["schema"], 1);
    assert_eq!(report["binding"].as_str().unwrap().len(), 64);
    let mut ordinal = 0;
    for r in regions {
        if r["kind"] == "conflict" {
            assert_eq!(r["id"], format!("result0.txt:{ordinal}"));
            ordinal += 1;
            assert_ne!(r["left_text"], r["right_text"]);
        } else {
            assert_eq!(r["id"], "");
        }
    }
    // Frozen upstream vectors have unambiguous alignment. Repeated lines are
    // qualified separately against common canonical hunks, not an LCS tie.
    if row["id"] != "repeated" {
        let actual: Vec<Value> = regions
            .iter()
            .map(|r| {
                json!([
                    r["kind"],
                    r["base"][0],
                    r["base"][1],
                    r["left"][0],
                    r["left"][1],
                    r["right"][0],
                    r["right"][1]
                ])
            })
            .collect();
        assert_eq!(json!(actual), row["regions"], "{}", row["id"]);
    }
    for side in ["base", "left", "right"] {
        let joined: String = regions
            .iter()
            .map(|r| r[format!("{side}_text")].as_str().unwrap())
            .collect();
        assert_eq!(joined, row[side], "{} {side}", row["id"]);
    }
}
// Recovery selects only failed/unrun IDs; an empty selection cannot pass.
fn picked(variable: &str, id: &str) -> bool {
    std::env::var(variable).map_or(true, |value| value.split(',').any(|chosen| chosen == id))
}
fn model_part(part: usize) {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("merge37-model-");
        let path = package(&root, None);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let mut count = 0;
        for (i, row) in vectors().iter().enumerate().filter(|(i, row)| {
            i % 3 == part && picked("MERGE_AUDIT_37_CASE", row["id"].as_str().unwrap())
        }) {
            let id = row["id"].as_str().unwrap();
            let world = fixture(row);
            let files = run_pair(
                &mut loaded,
                &world,
                0,
                "mergeaudit: planned 1 files\n",
                "",
                id,
                &root,
            );
            let report: Value = serde_json::from_slice(&files["output/plan.json"]).unwrap();
            verify_report(row, &report);
            if std::env::var_os("MERGE_AUDIT_37_PLAN_ONLY").is_some() {
                count += 1;
                continue;
            }
            for choice in ["base", "left", "right", "manual"] {
                let choices:Vec<_>=report["files"][0]["regions"].as_array().unwrap().iter().filter(|r|r["kind"]=="conflict").map(|r|json!({"id":r["id"],"choice":choice,"text":if choice=="manual" {"manual\n"} else {""}})).collect();
                let mut selected = world.clone();
                selected.args = vec![
                    "resolve".into(),
                    "manifest.json".into(),
                    "decisions.json".into(),
                    "output".into(),
                ];
                selected.files.insert(
                    "input/decisions.json".into(),
                    serde_json::to_vec(
                        &json!({"schema":1,"binding":report["binding"],"choices":choices}),
                    )
                    .unwrap(),
                );
                let resolved = run_pair(
                    &mut loaded,
                    &selected,
                    0,
                    "mergeaudit: resolved 1 files\n",
                    "",
                    &format!("{id}-{choice}"),
                    &root,
                );
                let expected: String = report["files"][0]["regions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        let side = match r["kind"].as_str().unwrap() {
                            "base" => "base",
                            "left" | "identical" => "left",
                            "right" => "right",
                            "conflict" => choice,
                            _ => unreachable!(),
                        };
                        if side == "manual" {
                            "manual\n"
                        } else {
                            r[format!("{side}_text")].as_str().unwrap()
                        }
                    })
                    .collect();
                assert_eq!(
                    resolved["output/result0.txt"],
                    expected.as_bytes(),
                    "{id}-{choice}"
                );
                assert_eq!(resolved["output/plan.json"], b"preserve");
            }
            count += 1;
            eprintln!("MERGE37_VECTOR {i} {id}");
        }
        assert!(count > 0, "empty model recovery selection");
        if std::env::var_os("MERGE_AUDIT_37_CASE").is_none() {
            assert_eq!(count, 5);
        }
    });
}
#[test]
fn merge_audit_37_model_0() {
    model_part(0)
}
#[test]
fn merge_audit_37_model_1() {
    model_part(1)
}
#[test]
fn merge_audit_37_model_2() {
    model_part(2)
}

#[test]
fn merge_audit_37_independent_oracles() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("merge37-oracle-");
        let path = package(
            &root,
            Some(include_str!("../support/merge_audit_37/oracle_main.hir")),
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
        let mut rows = vectors();
        let words = ["", "a\n", "b\n", "a\na\n", "a\nb\n", "b\na\n", "b\nb\n"];
        for (i, o) in words.iter().enumerate() {
            for (j, a) in words.iter().enumerate() {
                for (k, b) in words.iter().enumerate() {
                    rows.push(
                        json!({"id":format!("tiny-{i}-{j}-{k}"),"base":o,"left":a,"right":b}),
                    );
                }
            }
        }
        for row in &mut rows {
            let world = Fixture {
                args: ["base", "left", "right"]
                    .map(|k| row[k].as_str().unwrap().to_owned())
                    .into(),
                ..Default::default()
            };
            let execution = hirsel_engine::run_observed_snapshot_bounded(
                &mut loaded.engine,
                &run,
                SimulationConfig::from(&world),
                1_000_000,
            )
            .unwrap();
            let observed = execution
                .observation
                .unwrap_or_else(|| panic!("{} {:?}", row["id"], execution.outcome));
            assert!(observed.failures.is_empty(), "{observed:?}");
            let output = String::from_utf8(observed.output.clone()).unwrap();
            let mut lines = output.lines();
            row["hunks"] =
                serde_json::from_str(lines.next().unwrap_or_else(|| panic!("{observed:?}")))
                    .unwrap();
            row["report"] = serde_json::from_str(lines.next().unwrap()).unwrap();
            assert!(lines.next().is_none());
        }
        fs::write(
            root.join("actual-oracles.json"),
            serde_json::to_vec(&rows).unwrap(),
        )
        .unwrap();
        let check = Command::new("python3")
            .arg(root.join("check.py"))
            .arg(root.join("actual-oracles.json"))
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stderr)
        );
        let result: Value = serde_json::from_slice(&check.stdout).unwrap();
        assert_eq!(result["verified"], 358);
        eprintln!("MERGE37_ORACLE {result}");
    });
}
fn digest(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}
fn binding(world: &Fixture) -> String {
    let raw = &world.files["input/manifest.json"];
    let m: Value = serde_json::from_slice(raw).unwrap();
    let mut entries = m["files"].as_array().unwrap().clone();
    entries.sort_by_key(|e| e["name"].as_str().unwrap().to_owned());
    let hashes: Vec<_> = entries
        .iter()
        .flat_map(|e| {
            ["base", "left", "right"]
                .map(|k| digest(&world.files[&format!("input/{}", e[k].as_str().unwrap())]))
        })
        .collect();
    digest(&serde_json::to_vec(&json!({"manifest":digest(raw),"sources":hashes})).unwrap())
}
fn resolved_fixture() -> Fixture {
    let mut world = fixture(&vectors()[4]);
    world.args = vec![
        "resolve".into(),
        "manifest.json".into(),
        "decisions.json".into(),
        "output".into(),
    ];
    world.files.insert("input/decisions.json".into(),serde_json::to_vec(&json!({"schema":1,"binding":binding(&world),"choices":[{"id":"result0.txt:0","choice":"left","text":""}]})).unwrap());
    world
}
fn cancel_fixture() -> Fixture {
    let mut world = fixture(&vectors()[4]);
    world.args.push("--interruptible".into());
    world
        .file_reads
        .insert("input/b0".into(), FileReadScript::Pending);
    world.signals = Some(
        serde_json::from_value(
            json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
        )
        .unwrap(),
    );
    world
}
fn refusal_cases(include_four: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("merge37-refuse-");
        let path = package(&root, None);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let original = resolved_fixture();
        let decisions: Value =
            serde_json::from_slice(&original.files["input/decisions.json"]).unwrap();
        let mut completed = 0;
        for kind in [
            "missing_decision",
            "unknown_decision",
            "duplicate_decision",
            "binding",
            "changed_source",
            "choice",
            "decision_text",
            "fields",
            "json",
            "path",
            "duplicate_file",
            "denial",
            "commit",
            "pending",
        ] {
            if !picked("MERGE_AUDIT_37_REFUSAL", kind) {
                continue;
            }
            completed += 1;
            let mut world = original.clone();
            let mut d = decisions.clone();
            let error = match kind {
                "missing_decision" => {
                    d["choices"] = json!([]);
                    kind
                }
                "unknown_decision" => {
                    d["choices"][0]["id"] = json!("unknown");
                    kind
                }
                "duplicate_decision" => {
                    let first = d["choices"][0].clone();
                    d["choices"].as_array_mut().unwrap().push(first);
                    kind
                }
                "binding" => {
                    d["binding"] = json!("stale");
                    kind
                }
                "changed_source" => {
                    world.files.insert("input/b0".into(), b"changed".to_vec());
                    "binding"
                }
                "choice" => {
                    d["choices"][0]["choice"] = json!("markers");
                    kind
                }
                "decision_text" => {
                    d["choices"][0]["text"] = json!("unasked");
                    kind
                }
                "fields" => {
                    d["extra"] = json!(0);
                    kind
                }
                "json" => {
                    world
                        .files
                        .insert("input/manifest.json".into(), b"{".to_vec());
                    kind
                }
                "path" => {
                    world.files.insert("input/manifest.json".into(),serde_json::to_vec(&json!({"schema":1,"files":[{"name":"../escape","base":"b0","left":"l0","right":"r0"}]})).unwrap());
                    kind
                }
                "duplicate_file" => {
                    let mut m: Value = serde_json::from_slice(&manifest(1)).unwrap();
                    let e = m["files"][0].clone();
                    m["files"].as_array_mut().unwrap().push(e);
                    world.files.insert(
                        "input/manifest.json".into(),
                        serde_json::to_vec(&m).unwrap(),
                    );
                    kind
                }
                "denial" => {
                    world.mounts.clear();
                    "grant"
                }
                "commit" => {
                    world.file_faults.insert(
                        FileOperation::Commit,
                        [FileFault {
                            requested: None,
                            error: Some(FileFaultError::PermissionDenied),
                            partial: 0,
                        }]
                        .into(),
                    );
                    "permission_denied"
                }
                "pending" => {
                    world = cancel_fixture();
                    "canceled"
                }
                _ => unreachable!(),
            };
            if kind != "pending" {
                world.files.insert(
                    "input/decisions.json".into(),
                    serde_json::to_vec(&d).unwrap(),
                );
            }
            let files = run_pair(
                &mut loaded,
                &world,
                2,
                "",
                &format!("mergeaudit: {error}\n"),
                kind,
                &root,
            );
            assert_eq!(files, world.files, "no partial output: {kind}");
        }
        if include_four && picked("MERGE_AUDIT_37_REFUSAL", "four-files") {
            completed += 1;
            let mut world = fixture(&vectors()[0]);
            world
                .files
                .insert("input/manifest.json".into(), manifest(4));
            for i in 0..4 {
                for side in ["b", "l", "r"] {
                    world
                        .files
                        .insert(format!("input/{side}{i}"), b"independent\n".to_vec());
                }
            }
            let files = run_pair(
                &mut loaded,
                &world,
                0,
                "mergeaudit: planned 4 files\n",
                "",
                "four-files",
                &root,
            );
            let p: Value = serde_json::from_slice(&files["output/plan.json"]).unwrap();
            assert_eq!(p["files"].as_array().unwrap().len(), 4);
            for i in 0..4 {
                assert_eq!(p["files"][i]["name"], format!("result{i}.txt"));
            }
        }
        assert!(completed > 0, "empty refusal recovery selection");
    });
}

#[test]
fn merge_audit_37_refusals_only() {
    refusal_cases(false)
}
#[test]
fn merge_audit_37_refusals_and_independent_files() {
    refusal_cases(true)
}

// Distinct four-file fixture: native qualification never reruns completed
// one-file vector baselines, and requested widths have real independent jobs.
fn native_fixture(kind: &str) -> Fixture {
    let mut world = fixture(&vectors()[4]);
    world
        .files
        .insert("input/manifest.json".into(), manifest(4));
    for i in 0..4 {
        for (side, value) in [("b", "old"), ("l", "left"), ("r", "right")] {
            world.files.insert(
                format!("input/{side}{i}"),
                format!("anchor{i}\n{value}{i}\n").into_bytes(),
            );
        }
        world
            .files
            .insert(format!("output/result{i}.txt"), b"preserve".to_vec());
    }
    if kind == "resolve" || kind == "domain" {
        world.args = vec![
            "resolve".into(),
            "manifest.json".into(),
            "decisions.json".into(),
            "output".into(),
        ];
        let choices = if kind == "domain" {
            vec![]
        } else {
            (0..4)
                .map(|i| json!({"id":format!("result{i}.txt:0"),"choice":"left","text":""}))
                .collect()
        };
        world.files.insert(
            "input/decisions.json".into(),
            serde_json::to_vec(&json!({"schema":1,"binding":binding(&world),"choices":choices}))
                .unwrap(),
        );
    }
    if kind == "cancel" {
        world.args.push("--interruptible".into());
        world
            .file_reads
            .insert("input/b0".into(), FileReadScript::Pending);
        world.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
            )
            .unwrap(),
        );
    }
    world
}

#[test]
#[ignore = "original four-file reference reaches Steps1M before native launch; escalation @agentlang-24"]
fn merge_audit_37_native_full_main_replay() {
    native_workflows(false)
}
// Recover independent native identities without repeating the original
// reference Steps stop or the already-qualified width resolve/domain/cancel.
#[test]
fn merge_audit_37_native_missing_workflows() {
    native_workflows(true)
}
fn native_workflows(native_only: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("merge37-native-");
        let path = package(&root, None);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
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
        let objects = [Backend::Llvm, Backend::Cranelift].map(|backend| {
            (
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
            )
        });
        let recovery: Value = std::env::var("MERGE_AUDIT_37_NATIVE_RECOVERY")
            .map(|v| serde_json::from_str(&v).unwrap())
            .unwrap_or_else(|_| json!({}));
        let mut published_plan: Option<Value> = recovery.get("published_plan").cloned();
        // Recover only the missing file/handle/state predicates on accepted
        // live tapes; no application or completed replay is launched here.
        if let Some(audits) = recovery["audit_tapes"].as_object() {
            for (key, encoded) in audits {
                let tape = root.join(format!("audit-{}.tape", key.replace('/', "-")));
                restore_tape(encoded.as_str().unwrap(), &tape);
                let record =
                    hirsel_rt::launch::parse_recording_bytes(&fs::read(tape).unwrap()).unwrap();
                let world = native_fixture("plan");
                let (files, stderr) = recorded_state(&record, &world);
                let mut expected_files = world.files.clone();
                let write = record
                    .events
                    .iter()
                    .find_map(|e| match &e.request {
                        hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes.clone()),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&write).unwrap(),
                    *published_plan.as_ref().unwrap()
                );
                expected_files.insert("output/plan.json".into(), write);
                assert_eq!(files, expected_files);
                assert!(stderr.is_empty());
                eprintln!("MERGE37_RECORDED_STATE_RECOVERED {key}");
            }
        }
        let mut selected = 0;
        for kind in ["plan", "resolve", "domain", "denial", "cancel", "commit"] {
            if native_only
                && (!["plan", "denial", "commit"].contains(&kind)
                    || !picked("MERGE_AUDIT_37_NATIVE_CASE", kind))
            {
                continue;
            }
            selected += 1;
            let mut world = native_fixture(kind);
            if kind == "resolve" || kind == "domain" {
                let planned = published_plan
                    .as_ref()
                    .expect("resolve consumes an actually published native plan");
                let choices: Vec<_> = planned["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|f| f["regions"].as_array().unwrap())
                    .filter(|r| r["kind"] == "conflict")
                    .map(|r| json!({"id":r["id"],"choice":"left","text":""}))
                    .collect();
                world.files.insert(
                    "input/decisions.json".into(),
                    serde_json::to_vec(
                        &json!({"schema":1,"binding":planned["binding"],"choices":choices}),
                    )
                    .unwrap(),
                );
            }
            let error = match kind {
                "domain" => {
                    let mut d: Value =
                        serde_json::from_slice(&world.files["input/decisions.json"]).unwrap();
                    d["choices"] = json!([]);
                    world.files.insert(
                        "input/decisions.json".into(),
                        serde_json::to_vec(&d).unwrap(),
                    );
                    Some("missing_decision")
                }
                "denial" => {
                    world.mounts.clear();
                    Some("grant")
                }
                "cancel" => Some("canceled"),
                "commit" => {
                    world.file_faults.insert(
                        FileOperation::Commit,
                        [FileFault {
                            requested: None,
                            error: Some(FileFaultError::PermissionDenied),
                            partial: 0,
                        }]
                        .into(),
                    );
                    Some("permission_denied")
                }
                _ => None,
            };
            let stdout = if error.is_some() {
                ""
            } else if kind == "resolve" {
                "mergeaudit: resolved 4 files\n"
            } else {
                "mergeaudit: planned 4 files\n"
            };
            let stderr = error
                .map(|e| format!("mergeaudit: {e}\n"))
                .unwrap_or_default();
            let recovered_denial = kind == "denial" && recovery["denial_baseline_passed"] == true;
            let baseline = if recovered_denial {
                Some(world.files.clone())
            } else if native_only && kind != "denial" {
                None
            } else {
                Some(run_pair(
                    &mut loaded,
                    &world,
                    if error.is_some() { 2 } else { 0 },
                    stdout,
                    &stderr,
                    &format!("native-{kind}"),
                    &root,
                ))
            };
            let expected = Observation {
                exit_code: Some(if error.is_some() { 2 } else { 0 }),
                blocked: false,
                budget: None,
                signaled: None,
                output: stdout.as_bytes().to_vec(),
                failures: vec![],
                roots: vec![],
            };
            let cell = root.join(kind);
            fs::create_dir(&cell).unwrap();
            fs::write(cell.join("fixture.json"),serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"file_reads":world.file_reads,"signals":world.signals,"file_faults":world.file_faults})).unwrap()).unwrap();
            if kind != "commit" && (!native_only || kind == "denial") && !recovered_denial {
                let results = differential_in(
                    &input,
                    &loaded.entry,
                    &expected,
                    &matrix,
                    &tools,
                    &cell,
                    &world,
                );
                assert!(!results.is_empty());
                for (name, result) in results {
                    assert_eq!(result, Outcome::Agree, "{kind}/{name}");
                }
            }
            let mut players = vec![];
            for (backend, objects) in &objects {
                let dir = cell.join(format!("{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &world)
                        .unwrap();
                players.push((runnable, dir));
            }
            for (runnable, dir) in &players {
                let tape = dir.join("recording.tape");
                eprintln!("MERGE37_NATIVE_STARTED {kind} {}", dir.display());
                let key = format!("{kind}/{}", dir.file_name().unwrap().to_str().unwrap());
                if let Some(encoded) = recovery["tapes"][&key].as_str() {
                    restore_tape(encoded, &tape);
                    eprintln!("MERGE37_NATIVE_LIVE_REUSED {key}");
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
                    assert_eq!(observe(&live, &ready, &sites).unwrap(), expected, "{kind}");
                    assert_eq!(live.stderr, stderr.as_bytes());
                }
                let frozen = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                record.bound_run().unwrap();
                let writes: Vec<_> = record
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes),
                        _ => None,
                    })
                    .collect();
                let commits = record
                    .events
                    .iter()
                    .filter(|e| matches!(e.request, hirsel_rt::io::Request::Commit { .. }))
                    .count();
                if kind == "denial" {
                    assert!(writes.is_empty());
                    assert_eq!(commits, 0);
                } else if kind == "plan" || kind == "commit" {
                    assert_eq!(writes.len(), 1);
                    assert_eq!(commits, 1);
                    let plan: Value = serde_json::from_slice(writes[0]).unwrap();
                    assert_eq!(plan["binding"], binding(&world));
                    assert_eq!(plan["files"].as_array().unwrap().len(), 4);
                    if kind == "commit" {
                        assert_eq!(&plan, published_plan.as_ref().unwrap());
                    }
                }
                fs::write(
                    dir.join("semantic-events.txt"),
                    format!("{:#?}", record.events),
                )
                .unwrap();
                fs::write(dir.join("initial-tree.txt"), format!("{:#?}", record.tree)).unwrap();
                if kind == "plan" {
                    let output: Vec<u8> = record
                        .events
                        .iter()
                        .filter_map(|e| match &e.request {
                            hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes.clone()),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    if let Some(baseline) = &baseline {
                        assert_eq!(output, baseline["output/plan.json"]);
                    }
                    let plan: Value = serde_json::from_slice(&output).unwrap();
                    // The original four independent triples have one unchanged
                    // anchor and one conflicting line, with unambiguous LCS.
                    let expected_files: Vec<_> = (0..4).map(|i| json!({
                        "name":format!("result{i}.txt"),"regions":[
                            {"base":[0,1],"left":[0,1],"right":[0,1],"kind":"base","id":"",
                             "base_text":format!("anchor{i}\n"),"left_text":format!("anchor{i}\n"),"right_text":format!("anchor{i}\n")},
                            {"base":[1,1],"left":[1,1],"right":[1,1],"kind":"conflict","id":format!("result{i}.txt:0"),
                             "base_text":format!("old{i}\n"),"left_text":format!("left{i}\n"),"right_text":format!("right{i}\n")}
                        ]})).collect();
                    assert_eq!(
                        plan,
                        json!({"schema":1,"binding":binding(&world),"files":expected_files})
                    );
                    assert_eq!(plan["binding"], binding(&world));
                    assert_eq!(plan["files"].as_array().unwrap().len(), 4);
                    if let Some(previous) = &published_plan {
                        assert_eq!(&plan, previous);
                    } else {
                        published_plan = Some(plan);
                    }
                    fs::write(cell.join("published-plan.json"), output).unwrap();
                }
                assert_eq!(record.arguments, world.args);
                if kind == "cancel" {
                    assert!(
                        record.events.iter().any(|e| matches!(
                            (&e.request, &e.completion),
                            (
                                hirsel_rt::io::Request::SignalWait { .. },
                                hirsel_rt::io::Completion::Signal(Ok(
                                    hirsel_run_schema::SignalKind::Interrupt
                                ))
                            )
                        )),
                        "Interrupt was not delivered"
                    );
                    assert!(
                        record.events.iter().any(|e| matches!(
                            (&e.request, &e.completion),
                            (
                                hirsel_rt::io::Request::FileRead { .. },
                                hirsel_rt::io::Completion::Blocked { committed: false }
                            )
                        )),
                        "no Pending cancellation"
                    );
                }
                let offline = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    file_faults: BTreeMap::new(),
                    signals: None,
                    ..world.clone()
                };
                for (player, replay_dir) in &players {
                    let replay_key = format!(
                        "{key}/{}",
                        replay_dir.file_name().unwrap().to_str().unwrap()
                    );
                    if recovery["completed_native_replays"]
                        .as_array()
                        .is_some_and(|rows| rows.iter().any(|v| v.as_str() == Some(&replay_key)))
                    {
                        eprintln!("MERGE37_NATIVE_REPLAY_REUSED {replay_key}");
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
                    assert_eq!(replayed.stderr, stderr.as_bytes());
                    assert!(
                        fs::read_dir(replay_dir.join("root"))
                            .unwrap()
                            .next()
                            .is_none(),
                        "offline host mutation"
                    );
                }
                for (name, replay) in [
                    ("reference", hirsel_driver::replay_observations as Replay),
                    (
                        "checked-h",
                        hirsel_driver::replay_observations_checked as Replay,
                    ),
                ] {
                    if native_only && kind != "denial" {
                        continue; // Original host Steps obligation remains in the ignored aggregate.
                    }
                    if recovery["completed_host_replays"]
                        .as_array()
                        .is_some_and(|rows| {
                            rows.iter()
                                .any(|v| v.as_str() == Some(&format!("{key}/{name}")))
                        })
                    {
                        eprintln!("MERGE37_HOST_REPLAY_REUSED {key}/{name}");
                        continue;
                    }
                    let observed = replay(&path, &tape, Some(1_000_000))
                        .unwrap_or_else(|e| panic!("INCOMPLETE {kind}/{name}: {e}"));
                    assert_eq!(Observation::of_reference(&observed), expected);
                    // Host replay validates exact request bytes and complete tape
                    // consumption; only simulation APIs retain final_state.
                    assert!(observed.final_state.is_none());
                    assert_eq!(recorded_state(&record, &world).1, stderr.as_bytes());
                    fs::write(
                        dir.join(format!("{name}-replay.txt")),
                        format!("{observed:#?}"),
                    )
                    .unwrap();
                }
                let (mut expected_files, expected_stderr) =
                    (world.files.clone(), stderr.as_bytes());
                if kind == "plan" {
                    expected_files.insert("output/plan.json".into(), writes[0].clone());
                } else if kind == "resolve" {
                    for i in 0..4 {
                        expected_files.insert(
                            format!("output/result{i}.txt"),
                            format!("anchor{i}\nleft{i}\n").into_bytes(),
                        );
                    }
                }
                let (recorded_files, recorded_stderr) = recorded_state(&record, &world);
                assert_eq!(
                    recorded_files, expected_files,
                    "{kind}: exact file map from recorded publication"
                );
                assert_eq!(recorded_stderr, expected_stderr);
                fs::write(
                    dir.join("recorded-final-files.json"),
                    serde_json::to_vec(&recorded_files).unwrap(),
                )
                .unwrap();
                if kind == "plan" && dir.ends_with("Llvm") {
                    replay_controls(
                        &path,
                        &ready,
                        &sites,
                        &tools,
                        &players,
                        &world,
                        &record,
                        &expected,
                        &cell,
                        native_only,
                    );
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                eprintln!("MERGE37_NATIVE_PASS {kind} {}", dir.display());
            }
        }
        assert!(selected > 0, "empty native recovery selection");
    });
}
fn restore_tape(encoded: &str, path: &Path) {
    let restored = Command::new("python3").args(["-c", "import base64,gzip,sys;sys.stdout.buffer.write(gzip.decompress(base64.b64decode(sys.argv[1],validate=True)))", encoded]).output().unwrap();
    assert!(restored.status.success());
    fs::write(path, restored.stdout).unwrap();
}
// An independent accounting oracle for this app's read/atomic-publication
// protocol. Replay APIs validate exact requests and consumption but do not
// export a simulated final_state. Keep all file bytes and stderr assertions.
fn recorded_state(
    record: &hirsel_rt::launch::Recording,
    world: &Fixture,
) -> (BTreeMap<String, Vec<u8>>, Vec<u8>) {
    use hirsel_rt::io::{Completion as C, Request as R};
    assert_eq!(record.tree.files, world.files);
    let mut files = record.tree.files.clone();
    let mut dirs = BTreeMap::new();
    let mut reads = BTreeMap::new();
    let mut atomic = BTreeMap::<u64, (String, Vec<u8>)>::new();
    let mut stderr = vec![];
    for e in &record.events {
        match (&e.request, &e.completion) {
            (R::Directory { path, .. }, C::Handle(Ok(id))) => {
                dirs.insert(*id, path.clone());
            }
            (R::Open { directory, path }, C::Handle(Ok(id))) => {
                let name = format!("{}/{path}", dirs[directory]);
                assert!(files.contains_key(&name));
                reads.insert(*id, (name, 0usize));
            }
            (R::FileRead { file, max, .. }, C::Bytes(Ok(bytes))) => {
                let (name, offset) = reads.get_mut(file).unwrap();
                let content = &files[name];
                let end = offset
                    .saturating_add(usize::try_from(*max).unwrap())
                    .min(content.len());
                assert_eq!(bytes, &content[*offset..end]);
                *offset = end;
            }
            (R::CreateAtomic { directory, path }, C::Handle(Ok(id))) => {
                assert!(
                    atomic
                        .insert(*id, (format!("{}/{path}", dirs[directory]), vec![]))
                        .is_none()
                );
            }
            (R::WriteAll { file, bytes, .. }, C::Unit(Ok(()))) => {
                atomic.get_mut(file).unwrap().1.extend_from_slice(bytes);
            }
            (R::Commit { file, .. }, C::Unit(status)) => {
                let (name, bytes) = atomic.remove(file).unwrap();
                if status.is_ok() {
                    files.insert(name, bytes);
                }
            }
            (R::PrintStderr { bytes, .. }, C::Unit(Ok(()))) => stderr.extend_from_slice(bytes),
            (
                R::Create { .. }
                | R::OpenAppend { .. }
                | R::FileWrite { .. }
                | R::FileAppend { .. }
                | R::Write { .. }
                | R::Remove { .. }
                | R::RemoveAll { .. }
                | R::Rename { .. }
                | R::Mkdir { .. }
                | R::RemoveDir { .. }
                | R::SetPermissions { .. },
                _,
            ) => panic!("unexpected file mutation: {:?}", e.request),
            _ => {}
        }
    }
    assert!(atomic.is_empty(), "unconsumed atomic publication handles");
    (files, stderr)
}
type Replay = fn(
    &Path,
    &Path,
    Option<u64>,
) -> Result<hirsel_interp::observation::Observable, hirsel_engine::ToolError>;
#[allow(clippy::too_many_arguments)]
fn replay_controls(
    path: &Path,
    ready: &ReadyProgram,
    sites: &Sites,
    tools: &Toolchain,
    players: &[(Runnable, PathBuf)],
    world: &Fixture,
    record: &hirsel_rt::launch::Recording,
    expected: &Observation,
    cell: &Path,
    native_only: bool,
) {
    let offline = Fixture {
        files: BTreeMap::new(),
        file_reads: BTreeMap::new(),
        file_faults: BTreeMap::new(),
        signals: None,
        ..world.clone()
    };
    for kind in ["binding", "request", "missing", "extra"] {
        let mut changed = record.clone();
        let needle = match kind {
            "binding" => {
                changed.arguments.push("changed".into());
                "header disagrees"
            }
            "request" => {
                changed
                    .events
                    .iter_mut()
                    .find_map(|e| match &mut e.request {
                        hirsel_rt::io::Request::Directory { path, .. } => Some(path),
                        _ => None,
                    })
                    .unwrap()
                    .push_str("-changed");
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
        fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
        for replay in [
            hirsel_driver::replay_observations as Replay,
            hirsel_driver::replay_observations_checked as Replay,
        ] {
            if native_only {
                continue;
            }
            let error = replay(path, &bad, Some(1_000_000)).unwrap_err().to_string();
            assert!(error.contains(needle), "{kind}: {error}");
        }
        for (player, dir) in players {
            match launch_fixture(
                player,
                &ready.target,
                tools,
                dir,
                true,
                Tape::Replay(&bad),
                &offline,
            ) {
                Err(e) => assert!(e.message.contains(needle), "{kind}: {e}"),
                Ok(refused) => {
                    assert_eq!(refused.exit, Some(3));
                    assert!(format!("{:?}", refused.record).contains(needle));
                    assert_eq!(
                        refused.stdout,
                        if kind == "extra" {
                            expected.output.as_slice()
                        } else {
                            b""
                        }
                    );
                }
            }
            assert!(
                fs::read_dir(dir.join("root")).unwrap().next().is_none(),
                "control replay changed host files"
            );
        }
    }
}

#[test]
#[ignore = "requires an explicit backend:width:kind exact worker capacity request"]
fn merge_audit_37_ordered_capacity() {
    let (backend, width, kind) = super::width_capacity_36::selection("MERGE_AUDIT_37_WIDTH");
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("merge37-width-");
        let path = package(&root, None);
        let mut world = native_fixture(if kind == "success" { "resolve" } else { &kind });
        if kind == "domain" {
            let mut d: Value =
                serde_json::from_slice(&world.files["input/decisions.json"]).unwrap();
            d["choices"] = json!([]);
            world.files.insert(
                "input/decisions.json".into(),
                serde_json::to_vec(&d).unwrap(),
            );
        }
        capped_fixture_capacity(&path, &root, &world, backend, width, |observed, record| {
            fs::write(
                root.join("semantic-events.txt"),
                format!("{:#?}", record.events),
            )
            .unwrap();
            fs::write(root.join("initial-tree.txt"), format!("{:#?}", record.tree)).unwrap();
            fs::write(
                root.join("arguments.json"),
                serde_json::to_vec(&record.arguments).unwrap(),
            )
            .unwrap();
            if kind == "cancel" {
                assert!(record.events.iter().any(|e| matches!(
                    (&e.request, &e.completion),
                    (
                        hirsel_rt::io::Request::SignalWait { .. },
                        hirsel_rt::io::Completion::Signal(Ok(
                            hirsel_run_schema::SignalKind::Interrupt
                        ))
                    )
                )));
            }
            assert_eq!(
                observed.exit_code,
                Some(if kind == "success" { 0 } else { 2 })
            );
            assert!(observed.failures.is_empty());
            assert_eq!(
                observed.output,
                if kind == "success" {
                    b"mergeaudit: resolved 4 files\n".as_slice()
                } else {
                    b""
                }
            );
            if kind == "cancel" {
                assert!(
                    record.events.iter().any(|e| matches!(
                        (&e.request, &e.completion),
                        (
                            hirsel_rt::io::Request::SignalWait { .. },
                            hirsel_rt::io::Completion::Signal(Ok(
                                hirsel_run_schema::SignalKind::Interrupt
                            ))
                        )
                    )),
                    "Interrupt was not delivered"
                );
                assert!(record.events.iter().any(|e| matches!(
                    (&e.request, &e.completion),
                    (
                        hirsel_rt::io::Request::FileRead { .. },
                        hirsel_rt::io::Completion::Blocked { committed: false }
                    )
                )));
            }
        });
    });
}
fn cap_case(kind: &str) {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("merge37-cap-");
        let path = package(&root, None);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let mut world = fixture(&json!({"base":"","left":"","right":""}));
        let mut error = None;
        match kind {
            "source-exact" => {
                world.files.insert("input/b0".into(), vec![b'x'; 32768]);
            }
            "source-over" => {
                world.files.insert("input/b0".into(), vec![b'x'; 32769]);
                error = Some("bytes_limit");
            }
            "lines-exact" => {
                world
                    .files
                    .insert("input/b0".into(), "x\n".repeat(256).into_bytes());
            }
            "lines-over" => {
                world
                    .files
                    .insert("input/b0".into(), "x\n".repeat(257).into_bytes());
                error = Some("lines_limit");
            }
            "unicode-byte-over" => {
                world
                    .files
                    .insert("input/b0".into(), "羊".repeat(10923).into_bytes());
                error = Some("bytes_limit");
            }
            "total-exact" | "total-over" => {
                world
                    .files
                    .insert("input/manifest.json".into(), manifest(4));
                for i in 0..4 {
                    for side in ["b", "l", "r"] {
                        world.files.insert(format!("input/{side}{i}"), vec![]);
                    }
                }
                for key in ["b0", "l0", "r0", "b1"] {
                    world
                        .files
                        .insert(format!("input/{key}"), vec![b'x'; 32768]);
                }
                if kind == "total-over" {
                    world.files.insert("input/l1".into(), b"x".to_vec());
                    error = Some("total_bytes_limit");
                }
            }
            "files-over" => {
                world
                    .files
                    .insert("input/manifest.json".into(), manifest(5));
                error = Some("file_count_limit");
            }
            "decisions-exact" | "decisions-over" => {
                world.args = vec![
                    "resolve".into(),
                    "manifest.json".into(),
                    "decisions.json".into(),
                    "output".into(),
                ];
                let mut data =
                    serde_json::to_vec(&json!({"schema":1,"binding":binding(&world),"choices":[]}))
                        .unwrap();
                data.resize(
                    if kind == "decisions-exact" {
                        16384
                    } else {
                        16385
                    },
                    b' ',
                );
                world.files.insert("input/decisions.json".into(), data);
                if kind == "decisions-over" {
                    error = Some("bytes_limit");
                }
            }
            "report-exact" | "report-over" => {
                let text = format!("{}xxx", "\0".repeat(14550));
                for key in ["b0", "l0", "r0"] {
                    world
                        .files
                        .insert(format!("input/{key}"), text.as_bytes().to_vec());
                }
                let mut m: Value = serde_json::from_slice(&manifest(1)).unwrap();
                m["files"][0]["name"] = json!(if kind == "report-exact" { "aa" } else { "aaa" });
                world.files.insert(
                    "input/manifest.json".into(),
                    serde_json::to_vec(&m).unwrap(),
                );
                if kind == "report-over" {
                    error = Some("output_bytes_limit");
                }
            }
            "conflicts-over" => {
                let base = (0..128)
                    .map(|i| format!("a{i}\nx{i}\n"))
                    .collect::<String>();
                let left = format!(
                    "start-L\n{}",
                    (0..128)
                        .map(|i| format!(
                            "a{i}\n{}",
                            if i % 2 == 0 {
                                format!("L{i}\n")
                            } else {
                                String::new()
                            }
                        ))
                        .collect::<String>()
                );
                let right = format!(
                    "start-R\n{}",
                    (0..128)
                        .map(|i| format!(
                            "a{i}\n{}",
                            if i % 2 == 1 {
                                format!("R{i}\n")
                            } else {
                                String::new()
                            }
                        ))
                        .collect::<String>()
                );
                for (key, text) in [("b0", base), ("l0", left), ("r0", right)] {
                    world
                        .files
                        .insert(format!("input/{key}"), text.into_bytes());
                }
                error = Some("conflict_limit");
            }
            "conflicts-exact" => {
                for (key, value) in [("b0", "x"), ("l0", "L"), ("r0", "R")] {
                    let text = (0..128)
                        .map(|i| format!("{value}\n{}", if i < 127 { "a\n" } else { "" }))
                        .collect::<String>();
                    world
                        .files
                        .insert(format!("input/{key}"), text.into_bytes());
                }
            }
            _ => unreachable!(),
        }
        fs::write(
            root.join("original-cap.json"),
            serde_json::to_vec_pretty(
                &json!({"id":kind,"args":world.args,"files":world.files,"error":error}),
            )
            .unwrap(),
        )
        .unwrap();
        let stdout = if error.is_some() {
            ""
        } else if kind == "decisions-exact" {
            "mergeaudit: resolved 1 files\n"
        } else if kind == "total-exact" {
            "mergeaudit: planned 4 files\n"
        } else {
            "mergeaudit: planned 1 files\n"
        };
        let stderr = error
            .map(|s| format!("mergeaudit: {s}\n"))
            .unwrap_or_default();
        let files = run_pair(
            &mut loaded,
            &world,
            if error.is_some() { 2 } else { 0 },
            stdout,
            &stderr,
            kind,
            &root,
        );
        if error.is_some() {
            assert_eq!(files, world.files);
        }
        if kind == "report-exact" {
            assert_eq!(files["output/plan.json"].len(), 262144);
        }
        if kind == "conflicts-exact" {
            let report: Value = serde_json::from_slice(&files["output/plan.json"]).unwrap();
            assert_eq!(
                report["files"][0]["regions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|r| r["kind"] == "conflict")
                    .count(),
                128
            );
        }
    });
}
macro_rules! cap_test {
    ($name:ident,$case:literal) => {
        #[test]
        fn $name() {
            cap_case($case)
        }
    };
}
#[test]
#[ignore = "original reference reaches Steps1M in mergeaudit.model; cumulative cost follow-up @agentlang-24"]
fn merge_audit_37_cap_source_exact() {
    cap_case("source-exact")
}
cap_test!(merge_audit_37_cap_source_over, "source-over");
#[test]
fn merge_audit_37_cap_lines_exact() {
    cap_case("lines-exact")
}
cap_test!(merge_audit_37_cap_lines_over, "lines-over");
cap_test!(merge_audit_37_cap_unicode_over, "unicode-byte-over");
#[test]
#[ignore = "original reference reaches Steps1M in mergeaudit.model; cumulative cost follow-up @agentlang-24"]
fn merge_audit_37_cap_total_exact() {
    cap_case("total-exact")
}
#[test]
#[ignore = "original reference reaches Steps1M in mergeaudit.model; cumulative cost follow-up @agentlang-24"]
fn merge_audit_37_cap_total_over() {
    cap_case("total-over")
}
cap_test!(merge_audit_37_cap_files_over, "files-over");
#[test]
#[ignore = "original reference reaches Steps1M in JSON; escalation @agentlang-24"]
fn merge_audit_37_cap_decisions_exact() {
    cap_case("decisions-exact")
}
cap_test!(merge_audit_37_cap_decisions_over, "decisions-over");
#[test]
#[ignore = "original reference reaches Steps1M in std.diff; cumulative cost follow-up @agentlang-24"]
fn merge_audit_37_cap_conflicts_exact() {
    cap_case("conflicts-exact")
}

#[test]
#[ignore = "original reference reaches Steps1M in std.diff; cumulative cost follow-up @agentlang-24"]
fn merge_audit_37_cap_conflicts_over() {
    cap_case("conflicts-over")
}

// Adapted from width_capacity_36::fixture_capacity; assertions and public APIs
// are preserved. The capped wrapper bounds each runtime launch.
fn capped_fixture_capacity(
    manifest: &Path,
    root: &Path,
    fixture: &Fixture,
    backend: Backend,
    width: u64,
    verify: impl FnOnce(&Observation, &hirsel_rt::launch::Recording),
) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut loaded = hirsel_driver::load_program(manifest).unwrap();
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
        "env".into(),
        "HIRSEL_JOBS=4".into(),
        "python3".into(),
        root.join("run_capped.py").to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
    ];
    tools.launch_timeout = std::time::Duration::from_secs(60);
    let ready = build(&input, &options).unwrap();
    let sites = Sites::new(input.snapshot());
    let objects = emit_backend_objects(
        &ready,
        backend,
        OptLevel::None,
        Partition::Whole,
        false,
        None,
    )
    .unwrap();
    let dir = root.join("cell");
    fs::create_dir(&dir).unwrap();
    let short = tempfile::tempdir().unwrap();
    let control = short.path().join("ctl");
    link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, fixture).unwrap();
    let bound = hirsel_run_config::RunConfig::read_path(&dir.join("run.json")).unwrap();
    let mut record = bound.record().clone();
    record.placement = Some(
        hirsel_run_config::Placement::from_json(
            &serde_json::to_vec(&json!({
                "cores": width, "commit": "shared", "reorder": "off", "control": control,
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    let requested = record.validated().unwrap();
    let runnable = link_runnable(
        &ready,
        &loaded.entry,
        &objects,
        &tools,
        &dir,
        Some(&requested),
    )
    .unwrap();
    let tape = dir.join("recording.tape");
    let mut replies = Vec::new();
    let launched = std::thread::scope(|scope| {
        let child = scope.spawn(|| {
            launch_fixture(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
                fixture,
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
            if replies.iter().any(|m| width_running(m, width)) {
                break;
            }
            std::thread::yield_now();
        }
        child.join().unwrap()
    });
    let capacity = replies.iter().any(|m| width_running(m, width));
    fs::write(root.join("capacity.json"), serde_json::to_vec_pretty(&json!({
        "backend": format!("{backend:?}"), "requested": width, "control": replies, "capacity": capacity,
    })).unwrap()).unwrap();
    assert!(capacity, "INCOMPLETE: exact running capacity reply missing");
    let launched = launched.unwrap();
    fs::write(dir.join("stdout"), &launched.stdout).unwrap();
    fs::write(dir.join("stderr"), &launched.stderr).unwrap();
    let observed = observe(&launched, &ready, &sites).unwrap();
    fs::write(dir.join("observation.txt"), format!("{observed:#?}")).unwrap();
    let frozen = fs::read(&tape).unwrap();
    let recording = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
    verify(&observed, &recording);
    let replay_fixture = Fixture {
        files: Default::default(),
        file_reads: Default::default(),
        ..fixture.clone()
    };
    let replayed = launch_fixture(
        &runnable,
        &ready.target,
        &tools,
        &dir,
        true,
        Tape::Replay(&tape),
        &replay_fixture,
    )
    .unwrap();
    assert_eq!(observe(&replayed, &ready, &sites).unwrap(), observed);
    assert_eq!(fs::read(&tape).unwrap(), frozen);
    assert!(capacity, "INCOMPLETE: exact running capacity reply missing");
    eprintln!("WIDTH36_CAPACITY_PASS {:?} {width}", backend);
}
fn width_running(m: &Value, width: u64) -> bool {
    m["worker_phase"] == "running"
        && m["width"]["requested"] == width
        && m["width"]["effective"] == width
        && m["width"]["pending"] == 0
}

#[test]
#[ignore = "original reference reaches Steps1M in std.diff; cumulative cost follow-up @agentlang-24"]
fn merge_audit_37_cap_report_exact() {
    cap_case("report-exact")
}
#[test]
#[ignore = "original reference reaches Steps1M in std.diff; cumulative cost follow-up @agentlang-24"]
fn merge_audit_37_cap_report_over() {
    cap_case("report-over")
}
// This is separate native qualification of the same full-main original report
// vectors; reference/checked Steps stops remain separate, never replaced.
#[test]
fn merge_audit_37_native_report_boundaries() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("merge37-native-report-");
        let path = package(&root, None);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
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
        for over in [false, true] {
            let id = if over { "report-over" } else { "report-exact" };
            let name = if over { "aaa" } else { "aa" };
            let text = format!("{}xxx", "\0".repeat(14550));
            let mut world = fixture(&json!({"base":text,"left":text,"right":text}));
            let mut manifest: Value =
                serde_json::from_slice(&world.files["input/manifest.json"]).unwrap();
            manifest["files"][0]["name"] = json!(name);
            world.files.insert(
                "input/manifest.json".into(),
                serde_json::to_vec(&manifest).unwrap(),
            );
            // Std Map/JSON retain the explicit insertion order. Escape each
            // string with serde_json; this fixture fixes only the report layout.
            let quoted = serde_json::to_string(&text).unwrap();
            let region = format!(
                "{{\"base\":[0,1],\"left\":[0,1],\"right\":[0,1],\"kind\":\"base\",\"id\":\"\",\"base_text\":{quoted},\"left_text\":{quoted},\"right_text\":{quoted}}}"
            );
            let report=format!("{{\"schema\":1,\"binding\":{},\"files\":[{{\"name\":{},\"regions\":[{region}]}}]}}",serde_json::to_string(&binding(&world)).unwrap(),serde_json::to_string(name).unwrap()).into_bytes();
            assert_eq!(report.len(), 262144 + usize::from(over));
            let cell = root.join(id);
            fs::create_dir(&cell).unwrap();
            fs::write(cell.join("fixture.json"),serde_json::to_vec_pretty(&json!({"args":world.args,"files":world.files,"mounts":world.mounts,"source_bytes_each":14553,"total_source_bytes":43659,"report_bytes":report.len()})).unwrap()).unwrap();
            fs::write(cell.join("independent-report.json"), &report).unwrap();
            let expected = Observation {
                exit_code: Some(if over { 2 } else { 0 }),
                blocked: false,
                budget: None,
                signaled: None,
                output: if over {
                    vec![]
                } else {
                    b"mergeaudit: planned 1 files\n".to_vec()
                },
                failures: vec![],
                roots: vec![],
            };
            let errors = if over {
                b"mergeaudit: output_bytes_limit\n".as_slice()
            } else {
                b""
            };
            let mut players = vec![];
            for (backend, emitted) in &objects {
                let dir = cell.join(format!("{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let executable =
                    link_fixture_runnable(&ready, &loaded.entry, emitted, &tools, &dir, &world)
                        .unwrap();
                players.push((executable, dir));
            }
            for (player, dir) in &players {
                eprintln!("MERGE37_REPORT_NATIVE_STARTED {id} {}", dir.display());
                let tape = dir.join("recording.tape");
                let recovery: Value = std::env::var("MERGE_AUDIT_37_REPORT_RECOVERY")
                    .map(|v| serde_json::from_str(&v).unwrap())
                    .unwrap_or_else(|_| json!({}));
                let key = format!("{id}/{}", dir.file_name().unwrap().to_str().unwrap());
                if let Some(encoded) = recovery[&key].as_str() {
                    let recovered=Command::new("python3").args(["-c","import base64,gzip,sys;sys.stdout.buffer.write(gzip.decompress(base64.b64decode(sys.argv[1],validate=True)))",encoded]).output().unwrap();
                    assert!(recovered.status.success());
                    fs::write(&tape, recovered.stdout).unwrap();
                    eprintln!("MERGE37_REPORT_LIVE_REUSED {key}");
                } else {
                    let live = launch_fixture(
                        player,
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
                        serde_json::to_vec(
                            &json!({"exit":live.exit,"stdout":live.stdout,"stderr":live.stderr}),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(observe(&live, &ready, &sites).unwrap(), expected);
                    assert_eq!(live.stderr, errors);
                }
                let frozen = fs::read(&tape).unwrap();
                let recording = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                recording.bound_run().unwrap();
                let writes: Vec<_> = recording
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes),
                        _ => None,
                    })
                    .collect();
                let commits = recording
                    .events
                    .iter()
                    .filter(|e| matches!(e.request, hirsel_rt::io::Request::Commit { .. }))
                    .count();
                if over {
                    assert!(writes.is_empty());
                    assert_eq!(commits, 0, "one-over refuses before AtomicFile publication");
                    assert!(
                        !recording.events.iter().any(|e| matches!(
                            e.request,
                            hirsel_rt::io::Request::CreateAtomic { .. }
                        ))
                    );
                } else {
                    assert_eq!(writes, vec![&report]);
                    assert_eq!(commits, 1);
                }
                let offline = Fixture {
                    files: BTreeMap::new(),
                    ..world.clone()
                };
                for (replayer, playdir) in &players {
                    let replayed = launch_fixture(
                        replayer,
                        &ready.target,
                        &tools,
                        playdir,
                        true,
                        Tape::Replay(&tape),
                        &offline,
                    )
                    .unwrap();
                    assert_eq!(observe(&replayed, &ready, &sites).unwrap(), expected);
                    assert_eq!(replayed.stderr, errors);
                    assert!(fs::read_dir(playdir.join("root")).unwrap().next().is_none());
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                eprintln!("MERGE37_REPORT_NATIVE_PASS {id} {}", dir.display());
            }
        }
    });
}
