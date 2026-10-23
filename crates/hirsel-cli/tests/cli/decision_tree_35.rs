//! Full entry fixtures; each bounded cell runs in its own capped child process.
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

fn cases() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("../support/decision_tree_35/cases.json"))
        .unwrap()["cases"]
        .as_array()
        .unwrap()
        .clone()
}

fn scratch(label: &str) -> PathBuf {
    let base = std::env::var_os("DECISION_TREE_35_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if std::env::var_os("HIRSEL_TEST_POOL").is_some() {
                std::env::temp_dir()
            } else {
                PathBuf::from("/workspace/code/hirsel-briefs/decision-tree-35")
            }
        });
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(&format!("decision-tree-35-{label}-"))
        .tempdir_in(base)
        .unwrap()
        .keep()
}

fn package(root: &Path) -> PathBuf {
    for (name, data) in [
        (
            "main.hir",
            include_str!("../../../../examples/treelearn/main.hir"),
        ),
        (
            "dataset.hir",
            include_str!("../../../../examples/treelearn/dataset.hir"),
        ),
        (
            "split.hir",
            include_str!("../../../../examples/treelearn/split.hir"),
        ),
        (
            "train.hir",
            include_str!("../../../../examples/treelearn/train.hir"),
        ),
        (
            "predict.hir",
            include_str!("../../../../examples/treelearn/predict.hir"),
        ),
        (
            "store.hir",
            include_str!("../../../../examples/treelearn/store.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "archive.py",
            include_str!("../support/decision_tree_35/archive.py"),
        ),
        (
            "oracle.py",
            include_str!("../support/decision_tree_35/oracle.py"),
        ),
        (
            "cases.json",
            include_str!("../support/decision_tree_35/cases.json"),
        ),
        (
            "seal_inputs.py",
            include_str!("../support/decision_tree_35/seal_inputs.py"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/treelearn/hirsel.json")).unwrap();
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

fn fixture(row: &Value, jobs: u64) -> Fixture {
    let mut recipe = Fixture {
        args: vec![
            row["command"].as_str().unwrap().into(),
            "input".into(),
            "workflow.json".into(),
            "model.json".into(),
            jobs.to_string(),
        ],
        files: BTreeMap::from([
            (
                "input/workflow.json".into(),
                row["input"].as_str().unwrap().as_bytes().to_vec(),
            ),
            ("input/model.json".into(), b"old\n".to_vec()),
        ]),
        mounts: BTreeMap::from([("input".into(), Access::ReadWrite)]),
        ..Default::default()
    };
    if row["commit_denied"] == true {
        recipe.file_faults.insert(
            FileOperation::Commit,
            std::collections::VecDeque::from([FileFault {
                requested: None,
                error: Some(FileFaultError::PermissionDenied),
                partial: 0,
            }]),
        );
    }
    if row["denial"] == true {
        recipe.mounts.clear();
    }
    if row["pending"] == true {
        recipe.args.push("--interruptible".into());
        recipe
            .file_reads
            .insert("input/workflow.json".into(), FileReadScript::Pending);
        recipe.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
            )
            .unwrap(),
        );
    }
    recipe
}

fn simulation(
    loaded: &mut hirsel_engine::LoadedProgram,
    row: &Value,
    recipe: &Fixture,
) -> Option<Observation> {
    let run = RunConfiguration {
        entry: loaded.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut previous = None;
    for checked in [false, true] {
        let engine = if checked { "checked-h" } else { "reference" };
        if std::env::var("DECISION_TREE_35_ENGINE").is_ok_and(|v| v != engine) {
            continue;
        }
        receipt(
            &std::env::var_os("DECISION_TREE_35_ROOT")
                .map(PathBuf::from)
                .unwrap(),
            engine,
            "started",
        );
        let world = SimulationConfig::from(recipe);
        let execution = if checked {
            hirsel_engine::run_observed_checked_bounded(&mut loaded.engine, &run, world, 1_000_000)
        } else {
            hirsel_engine::run_observed_snapshot_bounded(&mut loaded.engine, &run, world, 1_000_000)
        }
        .unwrap();
        let Some(observed) = execution.observation else {
            assert!(
                matches!(&execution.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == 1_000_000),
                "{:?}",
                execution.outcome
            );
            fs::write(std::env::var_os("DECISION_TREE_35_ROOT").map(PathBuf::from).unwrap().join("incomplete.json"),
                json!({"id":row["id"],"engine":engine,"disposition":format!("{:?}",execution.outcome),"completed":false,"passed":false}).to_string()).unwrap();
            return None;
        };
        assert_eq!(
            observed.exit_code,
            row["exit"].as_i64().map(|n| n as i32),
            "{observed:?}"
        );
        assert_eq!(observed.output, row["stdout"].as_str().unwrap().as_bytes());
        assert!(observed.failures.is_empty(), "{:?}", observed.failures);
        assert!(observed.blocked.is_none());
        let state = observed.final_state.as_ref().unwrap();
        assert_eq!(state.stderr, row["stderr"].as_str().unwrap().as_bytes());
        let mut files = recipe.files.clone();
        files.insert(
            "input/model.json".into(),
            row["output"].as_str().unwrap().as_bytes().to_vec(),
        );
        let actual: BTreeMap<String, Vec<u8>> = state
            .trees
            .values()
            .flat_map(|(_, files)| files.clone())
            .collect();
        // Denied mounts expose no snapshot tree to the application.
        if row["denial"] != true {
            assert_eq!(actual, files);
        }
        if row["pending"] == true {
            assert!(observed.effects.iter().any(|e| matches!(&e.effect,
                hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation == "file.read.pending")), "Pending never delivered");
        }
        if let Some(previous) = &previous {
            assert_eq!(&observed, previous);
        }
        previous = Some(observed);
        receipt(
            &std::env::var_os("DECISION_TREE_35_ROOT")
                .map(PathBuf::from)
                .unwrap(),
            engine,
            "passed",
        );
    }
    Some(Observation::of_reference(&previous.unwrap()))
}

fn receipt(root: &Path, engine: &str, phase: &str) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("engines.jsonl"))
        .unwrap();
    writeln!(file, "{}", json!({"engine":engine,"phase":phase})).unwrap();
}

fn refusals(
    manifest: &Path,
    dir: &Path,
    recording: &hirsel_rt::launch::Recording,
    runnable: &Runnable,
    ready: &ReadyProgram,
    tools: &Toolchain,
    recipe: &Fixture,
    expected: &Observation,
) {
    let mut changes = Vec::new();
    let mut changed = recording.clone();
    changed.hash_seed ^= 1;
    changes.push(("binding", changed, "header disagrees"));
    let mut changed = recording.clone();
    let path = changed
        .events
        .iter_mut()
        .find_map(|e| match &mut e.request {
            hirsel_rt::io::Request::Directory { path, .. } => Some(path),
            _ => None,
        })
        .unwrap();
    path.push_str("-changed");
    changes.push(("request", changed, "ReplayMismatch"));
    let mut changed = recording.clone();
    changed.events.pop().unwrap();
    changes.push(("missing", changed, "MissingInput"));
    let mut changed = recording.clone();
    changed.events.push(recording.events[0].clone());
    changes.push(("extra", changed, "UnconsumedInput"));
    for (name, changed, code) in changes {
        let tape = dir.join(format!("refuse-{name}.tape"));
        fs::write(
            &tape,
            hirsel_rt::launch::render_recording(&changed).unwrap(),
        )
        .unwrap();
        for checked in [false, true] {
            let error = if checked {
                hirsel_driver::replay_observations_checked(manifest, &tape, Some(1_000_000))
            } else {
                hirsel_driver::replay_observations(manifest, &tape, Some(1_000_000))
            }
            .unwrap_err();
            assert!(error.to_string().contains(code), "{name}: {error}");
        }
        let replay = launch_fixture(
            runnable,
            &ready.target,
            tools,
            dir,
            true,
            Tape::Replay(&tape),
            recipe,
        );
        let detail = match replay {
            Err(e) => e.to_string(),
            Ok(refused) => {
                assert_eq!(refused.exit, Some(3), "{name}: {}", refused.record);
                assert!(
                    refused.stdout.is_empty() || refused.stdout == expected.output,
                    "{name}: invalid recorded stdout prefix"
                );
                fs::write(dir.join(format!("refuse-{name}.stdout")), &refused.stdout).unwrap();
                // Late refusal writes its own retained diagnostic. Existing
                // rc/destructure_failure_order_17 uses the same artifact.
                format!(
                    "{} {} {}",
                    String::from_utf8_lossy(&refused.stderr),
                    refused.record,
                    fs::read_to_string(dir.join("refusal")).unwrap_or_default()
                )
            }
        };
        fs::write(dir.join(format!("refuse-{name}.diagnostic")), &detail).unwrap();
        assert!(detail.contains(code), "{name}: {detail}");
    }
}

fn semantic(recording: &hirsel_rt::launch::Recording) -> hirsel_rt::launch::Recording {
    let mut result = recording.clone();
    if let Some(binding) = &mut result.binding {
        binding.admitted_at = 0;
        binding.placement = None;
    }
    result
}

fn accepted(root: &Path, id: &str) -> bool {
    fs::read(root.join("resume.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .is_some_and(|v| v[id] == true)
}
fn accept(root: &Path, id: &str) {
    let mut v: Value = fs::read(root.join("resume.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    v[id] = json!(true);
    fs::write(
        root.join("resume.json"),
        serde_json::to_vec_pretty(&v).unwrap(),
    )
    .unwrap();
    receipt(root, id, "passed");
}

fn native(
    loaded: &mut hirsel_engine::LoadedProgram,
    root: &Path,
    row: &Value,
    recipe: &Fixture,
    expected: &Observation,
) {
    let input = loaded.engine.export_native_input().unwrap();
    let input = loaded.engine.import_native_input(input).unwrap();
    let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    let matrix = vec![
        Configuration {
            options: options.clone(),
            engine: Engine::Heap { campaign: false },
        },
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options: options.clone(),
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ];
    let tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
    let scripted_width = std::env::var("DECISION_TREE_35_SCRIPTED_WIDTH").ok();
    let prepare_only = std::env::var_os("DECISION_TREE_35_PREPARE_ONLY").is_some();
    if !prepare_only && scripted_width.is_none() && !accepted(root, "h-differential") {
        receipt(root, "h-differential", "started");
        for (name, result) in differential_in(
            &input,
            &loaded.entry,
            expected,
            &matrix[..1],
            &tools,
            root,
            recipe,
        ) {
            match result {
                Outcome::Agree => accept(root, "h-differential"),
                Outcome::Unavailable(error) => {
                    let message = error.to_string();
                    assert!(
                        message.contains("Steps")
                            || message.contains("step limit")
                            || (!recipe.file_faults.is_empty()
                                && message
                                    == "native heap: file faults require reference/checked simulation or native snapshot providers"),
                        "{name}: {message}"
                    );
                    receipt(root, "h-differential", &format!("INCOMPLETE {message}"));
                    fs::write(
                        root.join("incomplete.json"),
                        json!({"engine":"h-differential","detail":message}).to_string(),
                    )
                    .unwrap();
                }
                mismatch => panic!("{name}: {mismatch:?}"),
            }
        }
    }
    let ready = build(&input, &options).unwrap();
    let sites = Sites::new(input.snapshot());
    let mut runnables = Vec::new();
    for backend in [Backend::Llvm, Backend::Cranelift] {
        if scripted_width.is_some() {
            let selected = std::env::var("DECISION_TREE_35_BACKEND").unwrap();
            if selected
                != if backend == Backend::Llvm {
                    "llvm"
                } else {
                    "cranelift"
                }
            {
                continue;
            }
        }
        let dir = root.join(format!("{backend:?}"));
        fs::create_dir_all(&dir).unwrap();
        let objects = emit_backend_objects(
            &ready,
            backend,
            OptLevel::None,
            Partition::Whole,
            false,
            None,
        )
        .unwrap();
        let runnable =
            link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, recipe).unwrap();
        if let Some(raw) = &scripted_width {
            let width: u64 = raw.parse().unwrap();
            assert!([1, 2, 4].contains(&width));
            let run_path = dir.join("run.json");
            let mut run: Value = serde_json::from_slice(&fs::read(&run_path).unwrap()).unwrap();
            run["placement"] = json!({"cores":width,"commit":"shared","reorder":"off"});
            fs::write(&run_path, serde_json::to_vec(&run).unwrap()).unwrap();
            hirsel_run_config::RunConfig::read_path(&run_path).unwrap();
        }
        runnables.push((runnable, dir));
    }
    assert_eq!(
        runnables.len(),
        if scripted_width.is_some() { 1 } else { 2 }
    );
    if prepare_only {
        fs::write(
            root.join("artifact-setup.json"),
            json!({"prepared":true,"executed":false}).to_string(),
        )
        .unwrap();
        return;
    }
    let mut reference = None;
    for (runnable, dir) in &runnables {
        let tape = dir.join("recording.tape");
        let recorder_name = format!(
            "{:?}",
            if dir.ends_with("Llvm") {
                Backend::Llvm
            } else {
                Backend::Cranelift
            }
        );
        let live_id = format!("{recorder_name}-live");
        if !accepted(root, &live_id) {
            receipt(root, &live_id, "started");
            let recorded = launch_fixture(
                runnable,
                &ready.target,
                &tools,
                dir,
                true,
                Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
                recipe,
            )
            .unwrap();
            assert_eq!(observe(&recorded, &ready, &sites).unwrap(), *expected);
            assert_eq!(recorded.stderr, row["stderr"].as_str().unwrap().as_bytes());
            accept(root, &live_id);
        }
        let bytes = fs::read(&tape).unwrap();
        let recording = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
        let published: Vec<u8> = recording
            .events
            .iter()
            .filter_map(|e| match &e.request {
                hirsel_rt::io::Request::WriteAll { bytes, .. } => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        if row["exit"] == 0 {
            assert_eq!(published, row["output"].as_str().unwrap().as_bytes());
        }
        let commits: Vec<_> = recording
            .events
            .iter()
            .filter(|e| matches!(e.request, hirsel_rt::io::Request::Commit { .. }))
            .collect();
        let atomic: Vec<_> = recording
            .events
            .iter()
            .filter(|e| matches!(e.request, hirsel_rt::io::Request::CreateAtomic { .. }))
            .collect();
        if row["exit"] == 0 {
            assert_eq!(atomic.len(), 1);
            assert!(
                matches!(&atomic[0].request, hirsel_rt::io::Request::CreateAtomic { path, .. } if path == "model.json")
            );
            assert_eq!(commits.len(), 1);
            assert!(matches!(
                commits[0].completion,
                hirsel_rt::io::Completion::Unit(Ok(()))
            ));
        } else if row["commit_denied"] == true {
            assert_eq!(atomic.len(), 1);
            assert_eq!(commits.len(), 1);
            assert!(matches!(
                commits[0].completion,
                hirsel_rt::io::Completion::Unit(Err(hirsel_rt::io::IoError::PermissionDenied))
            ));
        } else {
            assert!(atomic.is_empty() && commits.is_empty() && published.is_empty());
        }
        assert_eq!(recording.arguments, recipe.args);
        assert_eq!(recording.tree.files, recipe.files);
        recording.bound_run().unwrap();
        if let Some(width) = &scripted_width {
            let placement = recording
                .binding
                .as_ref()
                .unwrap()
                .placement
                .as_ref()
                .unwrap();
            let placement: Value = serde_json::from_slice(&placement.to_json()).unwrap();
            assert_eq!(placement["cores"], width.parse::<u64>().unwrap());
            assert_eq!(placement["reorder"], "off");
        }
        let stderr: Vec<u8> = recording
            .events
            .iter()
            .filter_map(|e| match &e.request {
                hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        assert_eq!(stderr, row["stderr"].as_str().unwrap().as_bytes());
        if row["pending"] == true {
            assert!(
                recording.events.iter().any(|e| matches!(
                    (&e.request, &e.completion),
                    (
                        hirsel_rt::io::Request::FileRead { .. },
                        hirsel_rt::io::Completion::Blocked { committed: false }
                    )
                )),
                "native read never parked"
            );
        }
        if let Some(reference) = &reference {
            assert_eq!(&semantic(&recording), reference);
        }
        reference = Some(semantic(&recording));
        let replay_recipe = Fixture {
            files: BTreeMap::new(),
            file_reads: BTreeMap::new(),
            file_faults: Default::default(),
            signals: None,
            ..recipe.clone()
        };
        for (target, target_dir) in &runnables {
            let replay_id = format!(
                "{recorder_name}-replay-{}",
                if target_dir.ends_with("Llvm") {
                    "Llvm"
                } else {
                    "Cranelift"
                }
            );
            if accepted(root, &replay_id) {
                continue;
            }
            receipt(root, &replay_id, "started");
            let replayed = launch_fixture(
                target,
                &ready.target,
                &tools,
                target_dir,
                true,
                Tape::Replay(&tape),
                &replay_recipe,
            )
            .unwrap();
            assert_eq!(observe(&replayed, &ready, &sites).unwrap(), *expected);
            assert_eq!(replayed.stderr, row["stderr"].as_str().unwrap().as_bytes());
            accept(root, &replay_id);
        }
        if scripted_width.is_some() {
            assert_eq!(fs::read(&tape).unwrap(), bytes);
            continue;
        }
        for checked in [false, true] {
            let replay_id = format!(
                "{recorder_name}-replay-{}",
                if checked { "checked" } else { "reference" }
            );
            if accepted(root, &replay_id) {
                continue;
            }
            receipt(root, &replay_id, "started");
            let replayed = if checked {
                hirsel_driver::replay_observations_checked(
                    &root.join("hirsel.json"),
                    &tape,
                    Some(1_000_000),
                )
            } else {
                hirsel_driver::replay_observations(
                    &root.join("hirsel.json"),
                    &tape,
                    Some(1_000_000),
                )
            };
            match replayed {
                Ok(replayed) => {
                    assert_eq!(Observation::of_reference(&replayed), *expected);
                    // Replay enrichment intentionally omits final_state. Exact
                    // write/commit events are checked below against the oracle;
                    // native players consume the whole immutable event stream.
                    accept(root, &replay_id);
                }
                Err(error) => {
                    let message = error.to_string();
                    assert!(message.contains("Steps"), "{replay_id}: {message}");
                    receipt(root, &replay_id, &format!("INCOMPLETE {message}"));
                    fs::write(
                        root.join("incomplete.json"),
                        json!({"engine":replay_id,"detail":message}).to_string(),
                    )
                    .unwrap();
                }
            }
        }
        if row["id"] == "toy" {
            refusals(
                &root.join("hirsel.json"),
                dir,
                &recording,
                runnable,
                &ready,
                &tools,
                recipe,
                expected,
            );
        }
        assert_eq!(fs::read(&tape).unwrap(), bytes);
    }
}

// Selected original vectors remain individually named in the durable receipts.
fn campaign(phase: &str) {
    campaign_selected(phase, None);
}

fn campaign_selected(phase: &str, only: Option<&str>) {
    let root = scratch(phase);
    let manifest = package(&root);
    // Setup may rebuild a stale std image; it owns this copy and runs before
    // the immutable qualification closure and per-process execution caps.
    let image = root.join("qualified.image");
    fs::copy(std::env::var_os("HIRSEL_IMAGE").unwrap(), &image).unwrap();
    hirsel_engine::with_compiler_stack(|| {
        hirsel_driver::load_program_source_with_images(
            &manifest,
            None,
            &hirsel_engine::StdImages::at(&image),
        )
        .unwrap_or_else(|e| panic!("image setup: {e}"));
    });
    let closure = Command::new("python3")
        .arg(root.join("seal_inputs.py"))
        .arg(&root)
        .arg(env!("CARGO_MANIFEST_DIR"))
        .arg(std::env::current_exe().unwrap())
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .output()
        .unwrap();
    fs::write(root.join("seal-stdout"), &closure.stdout).unwrap();
    fs::write(root.join("seal-stderr"), &closure.stderr).unwrap();
    assert!(closure.status.success(), "input closure: {closure:?}");
    let mut receipts = Vec::new();
    for row in cases() {
        let id = row["id"].as_str().unwrap();
        if only.is_some_and(|selected| selected != id) {
            continue;
        }
        if std::env::var("DECISION_TREE_35_CASE").is_ok_and(|s| !s.split(',').any(|v| v == id)) {
            continue;
        }
        let cell = std::env::var_os("DECISION_TREE_35_RESUME_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(id));
        fs::create_dir_all(&cell).unwrap();
        fs::write(cell.join("row.json"), row.to_string()).unwrap();
        let output = Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "120", "prlimit", "--cpu=60:61"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "decision_tree_35::decision_tree_35_cell",
                "--ignored",
                "--nocapture",
            ])
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_IMAGE", &image)
            .env("DECISION_TREE_35_ROW_FILE", cell.join("row.json"))
            .env("DECISION_TREE_35_ROOT", &cell)
            .env("DECISION_TREE_35_PHASE", phase)
            .output()
            .unwrap();
        fs::write(cell.join("stdout"), &output.stdout).unwrap();
        fs::write(cell.join("stderr"), &output.stderr).unwrap();
        let incomplete = cell.join("incomplete.json").exists();
        let receipt = json!({"id":id,"phase":phase,"selected":true,"started":true,"completed":output.status.success() && !incomplete,"passed":output.status.success() && !incomplete,"status":output.status.code(),"incomplete":incomplete,"root":cell});
        eprintln!("DECISION_TREE_35 {receipt}");
        receipts.push(receipt);
        fs::write(
            root.join("receipts.json"),
            serde_json::to_vec_pretty(&receipts).unwrap(),
        )
        .unwrap();
    }
    if phase == "scripted" || std::env::var_os("DECISION_TREE_35_TRANSPORT").is_some() {
        let archive = Command::new("python3")
            .arg(root.join("archive.py"))
            .arg(&root)
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&archive.stdout));
        assert!(
            archive.status.success(),
            "{}",
            String::from_utf8_lossy(&archive.stderr)
        );
    }
    assert!(!receipts.is_empty(), "zero selected vectors");
    assert!(
        receipts.iter().all(|r| r["passed"] == true),
        "failed/incomplete cells: {receipts:?}"
    );
}

#[test]
fn decision_tree_35_frozen_oracle() {
    let root = scratch("oracle");
    package(&root);
    let output = Command::new("python3")
        .arg(root.join("oracle.py"))
        .arg("--check")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!cases().is_empty());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("{} frozen independent vectors", cases().len()))
    );
}

#[test]
#[ignore = "full bounded acceptance inventory; original cap Stops remain INCOMPLETE"]
fn decision_tree_35_simulation() {
    campaign("simulation");
}

#[test]
#[ignore = "full native/replay acceptance inventory with durable evidence"]
fn decision_tree_35_native_record_replay() {
    campaign("native");
}

#[test]
#[ignore = "bounded per-vector worker, launched by the campaign"]
fn decision_tree_35_cell() {
    hirsel_engine::with_compiler_stack(|| {
        let root = PathBuf::from(std::env::var_os("DECISION_TREE_35_ROOT").unwrap());
        let row: Value = serde_json::from_slice(
            &fs::read(std::env::var_os("DECISION_TREE_35_ROW_FILE").unwrap()).unwrap(),
        )
        .unwrap();
        let manifest = package(&root);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let recipe = fixture(&row, row["jobs"].as_u64().unwrap_or(4));
        if std::env::var("DECISION_TREE_35_PHASE").unwrap() != "simulation" {
            let expected = Observation {
                exit_code: row["exit"].as_i64().map(|n| n as i32),
                blocked: false,
                budget: None,
                signaled: None,
                output: row["stdout"].as_str().unwrap().as_bytes().to_vec(),
                failures: vec![],
                roots: vec![],
            };
            native(&mut loaded, &root, &row, &recipe, &expected);
        } else {
            if simulation(&mut loaded, &row, &recipe).is_none() {
                return;
            }
        }
        if std::env::var_os("DECISION_TREE_35_PREPARE_ONLY").is_some() {
            return;
        }
        if root.join("incomplete.json").exists() {
            return;
        }
        fs::write(
            root.join("passed.json"),
            json!({"id":row["id"],"jobs":4,"passed":true}).to_string(),
        )
        .unwrap();
    });
}

#[test]
#[ignore = "requires landed exact-CPU client and durable per-cell transport"]
fn decision_tree_35_ordered_capacity() {
    let root = scratch("ordered");
    let manifest = package(&root);
    for (name, data) in [
        (
            "ordered.py",
            include_str!("../support/decision_tree_35/ordered.py"),
        ),
        (
            "archive.py",
            include_str!("../support/decision_tree_35/archive.py"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    let backend = std::env::var("DECISION_TREE_35_BACKEND").expect("select llvm/cranelift");
    let width = std::env::var("DECISION_TREE_35_WIDTH").expect("select 1/2/4");
    let kind = std::env::var("DECISION_TREE_35_KIND").expect("select success/domain/cancel");
    let binary = root.join(format!("ordered-{backend}"));
    let build = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["build", "--backend", &backend])
        .arg(&manifest)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    fs::write(root.join("build-stdout"), &build.stdout).unwrap();
    fs::write(root.join("build-stderr"), &build.stderr).unwrap();
    // Actual source, executable, std image and effective tool inputs are bound
    // before the measured program reader starts. External immutable tool bytes
    // are matched to the host-side retained closure after transport extraction.
    let input_seal = Command::new("python3").arg("-c").arg(r#"
import hashlib,json,os,sys
from pathlib import Path
r=Path(sys.argv[1]);names=['HIRSEL_IMAGE','HIRSEL_RUNTIME_ARCHIVES','LLVM_SYS_231_PREFIX','HIRSEL_LINK_KIT','HIRSEL_GLIBC_FLOOR']
roots={k:Path(os.environ[k]) for k in names if k in os.environ};roots['cli']=Path(sys.argv[2])
rows=[]
for key,p in roots.items():
 for f in ([p] if p.is_file() else sorted(x for x in p.rglob('*') if x.is_file())):
  h=hashlib.sha256(f.read_bytes()).hexdigest();rows.append(dict(name=key,path=str(f),sha256=h,bytes=f.stat().st_size))
for f in sorted(r.rglob('*')):
 if f.is_file():rows.append(dict(name='cell',path=str(f.relative_to(r)),sha256=hashlib.sha256(f.read_bytes()).hexdigest(),bytes=f.stat().st_size))
(r/'consumer-inputs.json').write_text(json.dumps(rows,indent=2)+'\n')
"#).arg(&root).arg(env!("CARGO_BIN_EXE_hirsel")).output().unwrap();
    assert!(input_seal.status.success(), "{input_seal:?}");
    let output = if build.status.success() {
        Some(
            Command::new("python3")
                .arg(root.join("capped"))
                .args(["--timeout", "60", "prlimit", "--cpu=60:61", "python3"])
                .arg(root.join("ordered.py"))
                .arg("--hirsel")
                .arg(env!("CARGO_BIN_EXE_hirsel"))
                .arg("--manifest")
                .arg(&manifest)
                .arg("--binary")
                .arg(&binary)
                .arg("--backend")
                .arg(&backend)
                .arg("--root")
                .arg(root.join("cell"))
                .arg("--fixture-root")
                .arg(&root)
                .arg("--width")
                .arg(&width)
                .arg("--kind")
                .arg(&kind)
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .output()
                .unwrap(),
        )
    } else {
        None
    };
    if let Some(output) = &output {
        fs::write(root.join("qualification-stdout"), &output.stdout).unwrap();
        fs::write(root.join("qualification-stderr"), &output.stderr).unwrap();
    }
    // Export partial evidence even when the body failed. A failed child never
    // earns credit just because transport succeeded.
    let archive = Command::new("python3")
        .arg(root.join("archive.py"))
        .arg(&root)
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&archive.stdout));
    assert!(
        archive.status.success(),
        "{}",
        String::from_utf8_lossy(&archive.stderr)
    );
    assert!(build.status.success(), "{build:?}");
    let output = output.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn decision_tree_35_minimum_bounds_full_main() {
    let root = scratch("minimum");
    package(&root);
    let row = cases()
        .into_iter()
        .find(|r| r["id"] == "minimum-row")
        .unwrap();
    // The pool's prepared image is already an immutable input. Bind the exact
    // external bytes instead of copying and recompressing the full SDK.
    let seal = Command::new("python3").arg("-c").arg(r#"
import hashlib,json,os,sys
from pathlib import Path
r=Path(sys.argv[1]);roots={'harness':Path(sys.argv[2]),'cli':Path(sys.argv[3]),'stdlib':Path(sys.argv[4])/'../../stdlib/std@1'}
for k in ['HIRSEL_IMAGE','HIRSEL_RUNTIME_ARCHIVES','LLVM_SYS_231_PREFIX','HIRSEL_LINK_KIT','HIRSEL_GLIBC_FLOOR']:
 if k in os.environ:roots[k]=Path(os.environ[k])
rows=[]
for key,p in roots.items():
 p=p.resolve(strict=True)
 for f in ([p] if p.is_file() else sorted(x for x in p.rglob('*') if x.is_file())):
  h=hashlib.sha256()
  with f.open('rb') as inp:
   for b in iter(lambda:inp.read(1024*1024),b''):h.update(b)
  rows.append(dict(name=key,path=str(f),sha256=h.hexdigest(),bytes=f.stat().st_size))
(r/'consumer-inputs.json').write_text(json.dumps(rows,indent=2)+'\n')
(r/'effective-env.json').write_text(json.dumps({k:v for k,v in os.environ.items() if k.startswith(('HIRSEL_','LLVM_','DECISION_TREE_35_'))},indent=2)+'\n')
"#).arg(&root).arg(std::env::current_exe().unwrap()).arg(env!("CARGO_BIN_EXE_hirsel")).arg(env!("CARGO_MANIFEST_DIR")).output().unwrap();
    fs::write(root.join("seal-stderr"), &seal.stderr).unwrap();
    assert!(seal.status.success(), "{seal:?}");
    let mut receipts = Vec::new();
    for engine in ["reference", "checked-h"] {
        if std::env::var("DECISION_TREE_35_ENGINE").is_ok_and(|selected| selected != engine) {
            continue;
        }
        let cell = root.join(engine);
        fs::create_dir_all(&cell).unwrap();
        fs::write(cell.join("row.json"), row.to_string()).unwrap();
        eprintln!("DT35_MINIMUM_STARTED {engine}");
        let output = Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "120", "prlimit", "--cpu=60:61"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "decision_tree_35::decision_tree_35_cell",
                "--ignored",
                "--nocapture",
            ])
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("DECISION_TREE_35_ROOT", &cell)
            .env("DECISION_TREE_35_ROW_FILE", cell.join("row.json"))
            .env("DECISION_TREE_35_PHASE", "simulation")
            .env("DECISION_TREE_35_ENGINE", engine)
            .output()
            .unwrap();
        fs::write(cell.join("stdout"), &output.stdout).unwrap();
        fs::write(cell.join("stderr"), &output.stderr).unwrap();
        let passed = output.status.success() && !cell.join("incomplete.json").exists();
        receipts.push(json!({"engine":engine,"id":"minimum-row","selected":true,"started":true,"completed":passed,"passed":passed,"status":output.status.code()}));
        fs::write(
            root.join("receipts.json"),
            serde_json::to_vec_pretty(&receipts).unwrap(),
        )
        .unwrap();
    }
    if std::env::var_os("DECISION_TREE_35_TRANSPORT").is_some() {
        let archive = Command::new("python3")
            .arg(root.join("archive.py"))
            .arg(&root)
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&archive.stdout));
        assert!(
            archive.status.success(),
            "{}",
            String::from_utf8_lossy(&archive.stderr)
        );
    }
    assert!(!receipts.is_empty());
    assert!(receipts.iter().all(|r| r["passed"] == true), "{receipts:?}");
}

#[test]
#[ignore = "explicit scripted Ordered width qualification inventory"]
fn decision_tree_35_scripted_ordered() {
    let case = std::env::var("DECISION_TREE_35_CASE").unwrap();
    assert!(["batch-four", "features-5", "pending-cancel"].contains(&case.as_str()));
    let backend = std::env::var("DECISION_TREE_35_BACKEND").unwrap();
    assert!(["llvm", "cranelift"].contains(&backend.as_str()));
    assert!(
        std::env::var("DECISION_TREE_35_SCRIPTED_WIDTH").is_ok(),
        "select width1/2/4"
    );
    campaign("scripted");
}

#[test]
#[ignore = "compare retained full scripted tapes after transport verification"]
fn decision_tree_35_scripted_tape_comparison() {
    let root = std::env::var_os("DECISION_TREE_35_TAPES_DIR").map(PathBuf::from);
    let packed: Option<BTreeMap<String, Vec<u8>>> = if std::env::var_os(
        "DECISION_TREE_35_TAPES_B64",
    )
    .is_some()
    {
        let decoded = Command::new("python3").args(["-c", "import base64,os,sys,zlib;sys.stdout.buffer.write(zlib.decompress(base64.b64decode(os.environ['DECISION_TREE_35_TAPES_B64'],validate=True)))"]).output().unwrap();
        assert!(decoded.status.success(), "{decoded:?}");
        Some(serde_json::from_slice(&decoded.stdout).unwrap())
    } else {
        None
    };
    assert!(root.is_some() || packed.is_some());
    for case in ["batch-four", "features-5", "pending-cancel"] {
        let mut expected = None;
        for (backend, directory) in [("llvm", "Llvm"), ("cranelift", "Cranelift")] {
            for width in [1, 2, 4] {
                let key = format!("{backend}-w{width}-{case}");
                let tape = root.as_ref().map(|r| {
                    r.join(&key)
                        .join(case)
                        .join(directory)
                        .join("recording.tape")
                });
                let raw = if let Some(packed) = &packed {
                    packed.get(&key).unwrap().clone()
                } else {
                    fs::read(tape.as_ref().unwrap()).unwrap()
                };
                let observed = hirsel_rt::launch::parse_recording_bytes(&raw).unwrap();
                observed.bound_run().unwrap();
                if let Some(expected) = &expected {
                    assert_eq!(&semantic(&observed), expected, "{backend} w{width} {case}");
                } else {
                    expected = Some(semantic(&observed));
                }
                if let Some(tape) = tape {
                    assert_eq!(fs::read(tape).unwrap(), raw);
                }
            }
        }
    }
}
