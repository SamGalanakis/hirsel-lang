//! Full-main event time, restart, strict checkpoint admission and publication.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, with_compiler_stack};
use hirsel_run_schema::{Access, FileReadScript};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Evidence(PathBuf);
impl Drop for Evidence {
    fn drop(&mut self) {
        let paired = if self.0.join("actual-inputs.json").exists() {
            tool_seal(&self.0, "after")
        } else {
            false
        };
        let status = Command::new("python3")
            .arg(self.0.join("archive.py"))
            .arg(&self.0)
            .status();
        let ok = paired && status.is_ok_and(|s| s.success());
        if std::thread::panicking() {
            if !ok {
                eprintln!("EVENT_WINDOW_39_ARCHIVE_FAILURE {}", self.0.display());
            }
        } else {
            assert!(ok, "evidence archive failed");
        }
    }
}

fn root(label: &str) -> PathBuf {
    let base = std::env::var_os("EVENT_WINDOW_39_EVIDENCE")
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
        include_str!("../support/event_window_39/archive.py"),
    )
    .unwrap();
    fs::write(
        root.join("seal.py"),
        include_str!("../support/event_window_39/seal.py"),
    )
    .unwrap();
    root
}

fn package(root: &Path) -> PathBuf {
    for (name, bytes) in [
        (
            "main.hir",
            include_str!("../../../../examples/eventwindow/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/eventwindow/model.hir"),
        ),
        (
            "input.hir",
            include_str!("../../../../examples/eventwindow/input.hir"),
        ),
        (
            "window.hir",
            include_str!("../../../../examples/eventwindow/window.hir"),
        ),
        (
            "checkpoint.hir",
            include_str!("../../../../examples/eventwindow/checkpoint.hir"),
        ),
        (
            "report.hir",
            include_str!("../../../../examples/eventwindow/report.hir"),
        ),
        (
            "oracle.py",
            include_str!("../support/event_window_39/oracle.py"),
        ),
        (
            "contracts.hir",
            include_str!("../support/event_window_39/contracts.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), bytes).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/eventwindow/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    seal(root);
    path
}

// The helper captures actual tool trees and source/fixture inputs independently
// of compiler metadata, follows symlink directories with cycle protection, and
// compares the last before-program capture with the after-consumer inventory.
fn tool_seal(root: &Path, phase: &str) -> bool {
    Command::new("python3")
        .arg(root.join("seal.py"))
        .arg(phase)
        .arg(root)
        .arg(std::env::current_exe().unwrap())
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .status()
        .is_ok_and(|s| s.success())
}
fn seal(root: &Path) {
    assert!(tool_seal(root, "before"), "actual input capture failed");
}

fn cases(root: &Path, partition: &str) -> Vec<Value> {
    let output = Command::new("python3")
        .arg(root.join("oracle.py"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(root.join("oracle-cases.json"), &output.stdout).unwrap();
    let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let selected: Vec<_> = cases
        .into_iter()
        .filter(|r| r["partition"] == partition)
        .filter(|r| {
            std::env::var("EVENT_WINDOW_39_CASE")
                .map_or(true, |v| v.split(',').any(|s| r["id"] == s))
        })
        .collect();
    assert!(!selected.is_empty(), "zero selected bodies");
    fs::write(
        root.join("selected.json"),
        serde_json::to_vec_pretty(&selected).unwrap(),
    )
    .unwrap();
    seal(root);
    selected
}

fn fixture(row: &Value) -> Fixture {
    let mut f = Fixture {
        args: serde_json::from_value(row["args"].clone()).unwrap(),
        mounts: [
            ("input".into(), Access::Read),
            ("output".into(), Access::ReadWrite),
        ]
        .into(),
        files: [
            (
                "input/state.json".into(),
                row["checkpoint"].as_str().unwrap().as_bytes().to_vec(),
            ),
            (
                "input/events.ndjson".into(),
                row["input"].as_str().unwrap().as_bytes().to_vec(),
            ),
            ("output/result.json".into(), b"preserve\n".to_vec()),
        ]
        .into(),
        steps: row["steps"].as_u64(),
        ..Default::default()
    };
    for (member, path) in [
        ("input_bytes", "input/events.ndjson"),
        ("checkpoint_bytes", "input/state.json"),
    ] {
        if let Some(value) = row.get(member) {
            f.files
                .insert(path.into(), serde_json::from_value(value.clone()).unwrap());
        }
    }
    if row["deny_output"] == true {
        f.mounts.remove("output");
    }
    if row["pending"] == true {
        f.file_reads
            .insert("input/state.json".into(), FileReadScript::Pending);
        f.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
            )
            .unwrap(),
        );
    }
    if !row["faults"].is_null() {
        f.file_faults = serde_json::from_value(row["faults"].clone()).unwrap();
    }
    f
}

fn expected_files(row: &Value, f: &Fixture) -> BTreeMap<String, Vec<u8>> {
    let mut files = f.files.clone();
    if row["exit"] == 0 {
        files.insert(
            "output/result.json".into(),
            row["expected"].as_str().unwrap().as_bytes().to_vec(),
        );
    }
    files
}
fn expected(row: &Value, observed: &Observation) {
    assert_eq!(
        observed.exit_code,
        Some(row["exit"].as_i64().unwrap() as i32),
        "{row}: {observed:?}"
    );
    assert!(
        !observed.blocked
            && observed.budget.is_none()
            && observed.signaled.is_none()
            && observed.failures.is_empty(),
        "{observed:?}"
    );
    assert!(observed.output.is_empty(), "{observed:?}");
}
fn receipt(root: &Path, id: &str, stage: &str, result: &str) {
    fs::write(root.join(format!("{id}-{stage}.receipt")), result).unwrap();
    eprintln!("EVENT_WINDOW_39 {result} {id}/{stage}");
}

fn reference(partition: &str) {
    with_compiler_stack(|| {
        let root = root(&format!("event-window-{partition}-reference-"));
        let _evidence = Evidence(root.clone());
        let manifest = package(&root);
        let cases = cases(&root, partition);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut failed = vec![];
        for row in cases {
            let id = row["id"].as_str().unwrap();
            let f = fixture(&row);
            let mut prior = None;
            for engine in ["reference", "checked-h"] {
                receipt(&root, id, engine, "STARTED");
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let result = if engine == "reference" {
                        hirsel_engine::run_observed_snapshot_bounded(
                            &mut loaded.engine,
                            &run,
                            (&f).into(),
                            f.steps.unwrap(),
                        )
                    } else {
                        hirsel_engine::run_observed_checked_bounded(
                            &mut loaded.engine,
                            &run,
                            (&f).into(),
                            f.steps.unwrap(),
                        )
                    }
                    .unwrap();
                    fs::write(
                        root.join(format!("{id}-{engine}.observation")),
                        format!("{result:#?}"),
                    )
                    .unwrap();
                    let observed = result
                        .observation
                        .as_ref()
                        .unwrap_or_else(|| panic!("INCOMPLETE {id}/{engine}: {result:?}"));
                    expected(&row, &Observation::of_reference(observed));
                    let state = observed.final_state.as_ref().unwrap();
                    assert_eq!(state.stderr, row["stderr"].as_str().unwrap().as_bytes());
                    assert_eq!(
                        state.trees[&0].1,
                        expected_files(&row, &f),
                        "{id}: snapshot/input unchanged and temporary cleanup"
                    );
                    if row["pending"] == true {
                        assert!(observed.effects.iter().any(|e| matches!(&e.effect, hirsel_interp::observation::Effect::CapabilityConsumed(c) if c.operation == "file.read.pending")), "read never parked");
                    }
                    if let Some(p) = &prior {
                        assert_eq!(observed, p);
                    }
                    prior = Some(observed.clone());
                }));
                receipt(
                    &root,
                    id,
                    engine,
                    if result.is_ok() {
                        "PASS"
                    } else {
                        "FAIL_OR_INCOMPLETE"
                    },
                );
                if result.is_err() {
                    failed.push(format!("{id}/{engine}"));
                }
            }
        }
        assert!(failed.is_empty(), "{}: {failed:?}", root.display());
    });
}

// Account publication independently from the app and compare complete bytes.
fn tape_files(record: &hirsel_rt::launch::Recording, row: &Value, f: &Fixture) {
    use hirsel_rt::io::{Completion as C, Request as R};
    assert_eq!(record.tree.files, f.files);
    record.bound_run().unwrap();
    let mut files = f.files.clone();
    let mut dirs = BTreeMap::new();
    let mut atomic = BTreeMap::<u64, (String, Vec<u8>)>::new();
    let mut opened = std::collections::BTreeSet::new();
    let mut stderr = vec![];
    let mut commits = 0;
    for e in &record.events {
        match (&e.request, &e.completion) {
            (R::Directory { path, .. }, C::Handle(Ok(id))) => {
                dirs.insert(*id, path.clone());
            }
            (R::Open { .. }, C::Handle(Ok(id))) => {
                assert!(opened.insert(*id));
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
                    commits += 1;
                }
            }
            (R::Close { handle }, _) => {
                atomic.remove(handle);
                opened.remove(handle);
            }
            (R::PrintStderr { bytes, .. }, C::Unit(Ok(()))) => stderr.extend_from_slice(bytes),
            (
                R::Create { .. }
                | R::FileWrite { .. }
                | R::Remove { .. }
                | R::Rename { .. }
                | R::OpenAppend { .. },
                _,
            ) => panic!("unexpected mutation"),
            _ => (),
        }
    }
    assert!(
        atomic.is_empty() && opened.is_empty(),
        "unclosed file/atomic handle"
    );
    assert_eq!(commits, if row["exit"] == 0 { 1 } else { 0 });
    assert_eq!(files, expected_files(row, f));
    assert_eq!(stderr, row["stderr"].as_str().unwrap().as_bytes());
}

type Replay = fn(
    &Path,
    &Path,
    Option<u64>,
) -> Result<hirsel_interp::observation::Observable, hirsel_engine::ToolError>;

fn native(partition: &str) {
    with_compiler_stack(|| {
        let root = root(&format!("event-window-{partition}-native-"));
        let _evidence = Evidence(root.clone());
        let manifest = package(&root);
        let cases = cases(&root, partition);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
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
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.execution = Execution::Executable;
        tools.wrapper = vec![
            "env".into(),
            "HIRSEL_MEMORY_LIMIT=2G".into(),
            "HIRSEL_JOBS=1".into(),
            "python3".into(),
            root.join("capped").to_string_lossy().into_owned(),
            "--timeout".into(),
            "60".into(),
            "prlimit".into(),
            "--cpu=60:61".into(),
        ];
        tools.launch_timeout = std::time::Duration::from_secs(75);
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let mut objects = vec![];
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
            let object_dir = root.join("object-inputs");
            fs::create_dir_all(&object_dir).unwrap();
            let mut inputs = vec![];
            for (index, object) in emitted.iter().enumerate() {
                let digest = format!("{:x}", Sha256::digest(&object.bytes));
                fs::write(
                    object_dir.join(format!("{backend:?}-{index}-{digest}.bin")),
                    &object.bytes,
                )
                .unwrap();
                inputs.push(json!({"index":index,"sha256":digest,"bytes":object.bytes.len()}));
            }
            fs::write(
                root.join(format!("{backend:?}-object-inputs.json")),
                serde_json::to_vec_pretty(&inputs).unwrap(),
            )
            .unwrap();
            objects.push((backend, emitted));
        }
        let mut failures = vec![];
        for row in cases {
            let id = row["id"].as_str().unwrap();
            let f = fixture(&row);
            let mut linked = vec![];
            for (backend, objects) in &objects {
                let dir = root.join(format!("{id}-{backend:?}"));
                fs::create_dir(&dir).unwrap();
                linked.push((
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &f)
                        .unwrap(),
                    dir,
                ));
            }
            for (recorder, dir) in &linked {
                let stage = dir.file_name().unwrap().to_str().unwrap();
                receipt(&root, id, stage, "STARTED");
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let tape = dir.join("recording");
                    receipt(&root, id, &format!("{stage}-record"), "STARTED");
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
                    expected(&row, &observation);
                    receipt(&root, id, &format!("{stage}-record"), "PASS");
                    let bytes = fs::read(&tape).unwrap();
                    let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                    assert_eq!(record.arguments, f.args);
                    tape_files(&record, &row, &f);
                    let absent = Fixture {
                        files: BTreeMap::new(),
                        file_reads: BTreeMap::new(),
                        file_faults: BTreeMap::new(),
                        ..f.clone()
                    };
                    for (runnable, replay_dir) in &linked {
                        let replay_stage = format!(
                            "{stage}-replay-{}",
                            replay_dir.file_name().unwrap().to_str().unwrap()
                        );
                        receipt(&root, id, &replay_stage, "STARTED");
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
                        receipt(&root, id, &replay_stage, "PASS");
                    }
                    for (name, replay) in [
                        ("reference", hirsel_driver::replay_observations as Replay),
                        (
                            "checked-h",
                            hirsel_driver::replay_observations_checked as Replay,
                        ),
                    ] {
                        let replay_stage = format!("{stage}-replay-{name}");
                        receipt(&root, id, &replay_stage, "STARTED");
                        assert_eq!(
                            Observation::of_reference(&replay(&manifest, &tape, f.steps).unwrap()),
                            observation
                        );
                        receipt(&root, id, &replay_stage, "PASS");
                    }
                    if id == "out-of-order" || id == "duplicate" {
                        for control in ["binding", "request", "missing", "extra"] {
                            let mut changed = record.clone();
                            let needle = match control {
                                "binding" => "header disagrees",
                                "request" => "ReplayMismatch",
                                "missing" => "MissingInput",
                                _ => "UnconsumedInput",
                            };
                            match control {
                                "binding" => changed.arguments.push("tampered".into()),
                                "request" => {
                                    let e = changed
                                        .events
                                        .iter_mut()
                                        .find(|e| {
                                            matches!(
                                                e.request,
                                                hirsel_rt::io::Request::FileRead { .. }
                                            )
                                        })
                                        .unwrap();
                                    if let hirsel_rt::io::Request::FileRead { max, .. } =
                                        &mut e.request
                                    {
                                        *max += 1;
                                    }
                                }
                                "missing" => {
                                    changed.events.pop();
                                }
                                _ => changed.events.push(changed.events.last().unwrap().clone()),
                            }
                            let bad = dir.join(control);
                            fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap())
                                .unwrap();
                            for replay in [
                                hirsel_driver::replay_observations,
                                hirsel_driver::replay_observations_checked,
                            ] {
                                let error =
                                    replay(&manifest, &bad, f.steps).unwrap_err().to_string();
                                assert!(error.contains(needle), "{control}: {error}");
                            }
                            for (runnable, replay_dir) in &linked {
                                let refused = launch_fixture(
                                    runnable,
                                    &ready.target,
                                    &tools,
                                    replay_dir,
                                    true,
                                    Tape::Replay(&bad),
                                    &absent,
                                );
                                match refused {
                                    Err(error) => assert!(
                                        error.message.contains(needle),
                                        "{control}: {error}"
                                    ),
                                    Ok(refused) => {
                                        assert_eq!(refused.exit, Some(3));
                                        assert!(
                                            format!("{:?}", refused.record).contains(needle),
                                            "{control}: {:?}",
                                            refused.record
                                        );
                                    }
                                }
                            }
                        }
                    }
                    assert_eq!(fs::read(&tape).unwrap(), bytes);
                }));
                receipt(
                    &root,
                    id,
                    stage,
                    if result.is_ok() {
                        "PASS_REPLAY_4"
                    } else {
                        "FAIL_OR_INCOMPLETE"
                    },
                );
                if result.is_err() {
                    failures.push(format!("{id}/{stage}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}: {failures:?}", root.display());
    });
}

#[test]
fn event_window_39_semantics_reference_checked() {
    reference("semantics")
}
#[test]
fn event_window_39_invalid_reference_checked() {
    reference("invalid")
}
#[test]
fn event_window_39_caps_reference_checked() {
    reference("caps")
}
#[test]
fn event_window_39_faults_reference_checked() {
    reference("faults")
}
#[test]
fn event_window_39_semantics_native_replay() {
    native("semantics")
}
#[test]
fn event_window_39_invalid_native_replay() {
    native("invalid")
}
#[test]
fn event_window_39_caps_native_replay() {
    native("caps")
}
#[test]
fn event_window_39_faults_native_replay() {
    native("faults")
}

#[test]
fn event_window_39_arithmetic_and_output_boundary() {
    with_compiler_stack(|| {
        let root = root("event-window-contracts-");
        let _evidence = Evidence(root.clone());
        let path = package(&root);
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path":"contracts.hir","module":["contracts"]}));
        manifest["entry"]["module"] = json!(["contracts"]);
        fs::write(&path, manifest.to_string()).unwrap();
        fs::write(
            root.join("limits.json"),
            r#"{"steps":5000000,"cpu_seconds":60,"memory_bytes":2147483648,"action_seconds":600}"#,
        )
        .unwrap();
        seal(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let result = hirsel_engine::run_observed_snapshot_bounded(
            &mut loaded.engine,
            &run,
            Default::default(),
            5_000_000,
        )
        .unwrap();
        let observation = result
            .observation
            .unwrap_or_else(|| panic!("INCOMPLETE {:?}", result.outcome));
        assert_eq!(observation.exit_code, Some(0));
        assert!(observation.failures.is_empty());
        let checked = hirsel_engine::run_observed_checked_bounded(
            &mut loaded.engine,
            &run,
            Default::default(),
            5_000_000,
        )
        .unwrap();
        assert_eq!(checked.observation.as_ref(), Some(&observation));
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
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.execution = Execution::Executable;
        tools.wrapper = vec![
            "env".into(),
            "HIRSEL_MEMORY_LIMIT=2G".into(),
            "HIRSEL_JOBS=1".into(),
            "python3".into(),
            root.join("capped").to_string_lossy().into_owned(),
            "--timeout".into(),
            "60".into(),
            "prlimit".into(),
            "--cpu=60:61".into(),
        ];
        for (engine, outcome) in differential_in(
            &input,
            &loaded.entry,
            &Observation::of_reference(&observation),
            &matrix,
            &tools,
            &root,
            &Fixture {
                steps: Some(5_000_000),
                ..Default::default()
            },
        ) {
            assert_eq!(outcome, Outcome::Agree, "{engine}");
        }
    });
}

#[test]
fn event_window_39_registry_reference_checked() {
    reference("registry")
}
#[test]
fn event_window_39_registry_native_replay() {
    native("registry")
}

#[test]
fn event_window_39_beam_bundles_reference_checked() {
    reference("beam")
}
#[test]
fn event_window_39_beam_bundles_native_replay() {
    native("beam")
}

#[test]
fn event_window_39_scanner_reference_checked() {
    reference("scanner")
}
#[test]
fn event_window_39_scanner_native_replay() {
    native("scanner")
}
