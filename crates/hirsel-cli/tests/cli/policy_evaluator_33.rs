//! Full entry fixtures; each bounded cell runs in its own capped child process.
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

fn cases() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("../support/policy_evaluator_33/cases.json"))
        .unwrap()["cases"]
        .as_array()
        .unwrap()
        .clone()
}

fn scratch(label: &str) -> PathBuf {
    let base = std::env::var_os("POLICY_EVALUATOR_33_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(&format!("policy-evaluator-33-{label}-"))
        .tempdir_in(base)
        .unwrap()
        .keep()
}

fn package(root: &Path) -> PathBuf {
    for (name, data) in [
        (
            "main.hir",
            include_str!("../../../../examples/policycheck/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/policycheck/model.hir"),
        ),
        (
            "roles.hir",
            include_str!("../../../../examples/policycheck/roles.hir"),
        ),
        (
            "matchers.hir",
            include_str!("../../../../examples/policycheck/matchers.hir"),
        ),
        (
            "evaluate.hir",
            include_str!("../../../../examples/policycheck/evaluate.hir"),
        ),
        (
            "store.hir",
            include_str!("../../../../examples/policycheck/store.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "oracle.py",
            include_str!("../support/policy_evaluator_33/oracle.py"),
        ),
        (
            "cases.json",
            include_str!("../support/policy_evaluator_33/cases.json"),
        ),
        (
            "seal_inputs.py",
            include_str!("../support/policy_evaluator_33/seal_inputs.py"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/policycheck/hirsel.json")).unwrap();
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
        args: vec!["input".into(), "workflow.json".into(), jobs.to_string()],
        files: BTreeMap::from([(
            "input/workflow.json".into(),
            row["input"].as_str().unwrap().as_bytes().to_vec(),
        )]),
        mounts: BTreeMap::from([("input".into(), Access::Read)]),
        ..Default::default()
    };
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
    prepared: &hirsel_driver::requests::TirExecution,
    row: &Value,
    recipe: &Fixture,
) -> Option<Observation> {
    let run = RunConfiguration {
        entry: prepared.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut previous = None;
    for checked in [false, true] {
        let engine = if checked { "checked-h" } else { "reference" };
        if std::env::var("POLICY_EVALUATOR_33_ENGINE").is_ok_and(|v| v != engine) {
            continue;
        }
        receipt(
            &std::env::var_os("POLICY_EVALUATOR_33_ROOT")
                .map(PathBuf::from)
                .unwrap(),
            engine,
            "started",
        );
        let world = SimulationConfig::from(recipe);
        let execution = hirsel_driver::testing::observe_tir(
            prepared,
            &run,
            world,
            hirsel_interp::Limits {
                steps: Some(1_000_000),
                ..Default::default()
            },
            checked,
        )
        .unwrap();
        let Some(observed) = execution.observation else {
            assert!(
                matches!(&execution.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == 1_000_000),
                "{:?}",
                execution.outcome
            );
            fs::write(std::env::var_os("POLICY_EVALUATOR_33_ROOT").map(PathBuf::from).unwrap().join("incomplete.json"),
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
        assert_eq!(state.trees[&0].1, recipe.files);
        if row["pending"] == true {
            assert!(observed.effects.iter().any(|e| matches!(&e.effect,
                hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation == "file.read.pending")), "Pending never delivered");
        }
        if let Some(previous) = &previous {
            assert_eq!(&observed, previous);
        }
        previous = Some(observed);
        receipt(
            &std::env::var_os("POLICY_EVALUATOR_33_ROOT")
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
    writeln!(file, "{}", json!({"engine":engine,"phase":phase,"generation":recovery_plan().map(|p| p["generation"].clone())})).unwrap();
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

fn recovery_plan() -> Option<Value> {
    std::env::var_os("POLICY_EVALUATOR_33_RECOVERY_PLAN")
        .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap())
}

fn validate_recovery_plan(plan: &Value, selected: &str) -> Result<(), String> {
    if plan["case"].as_str() != Some(selected) || selected.contains(',') {
        return Err("recovery requires exactly the plan's single case".into());
    }
    let row = cases().into_iter().find(|r| r["id"] == selected);
    if row.as_ref() != Some(&plan["row"]) {
        return Err("recovery row differs from the frozen vector".into());
    }
    let phases = plan["phases"].as_array().ok_or("missing recovery phases")?;
    if phases.is_empty() {
        return Err("zero selected recovery phases".into());
    }
    let mut allowed = vec!["h-differential".to_string()];
    for recorder in ["Llvm", "Cranelift"] {
        allowed.push(format!("{recorder}-live"));
        for player in ["Llvm", "Cranelift", "reference", "checked"] {
            allowed.push(format!("{recorder}-replay-{player}"));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for phase in phases {
        let name = phase.as_str().ok_or("non-string recovery phase")?;
        if !allowed.iter().any(|v| v == name) || !seen.insert(name) {
            return Err(format!("invalid or duplicate recovery phase: {name}"));
        }
        if let Some((recorder, _)) = name.split_once("-replay-") {
            if !phases.iter().any(|v| v == &format!("{recorder}-live")) {
                return Err("fresh replay requires its new recording phase".into());
            }
        }
    }
    Ok(())
}

fn accepted(root: &Path, id: &str) -> bool {
    if let Some(plan) = recovery_plan() {
        // Historical accepted identities remain external; their markers and
        // tapes never enter this compiler generation. Only listed phases run.
        return !plan["phases"].as_array().unwrap().iter().any(|v| v == id);
    }
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
    if !accepted(root, "h-differential") {
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
                        message.contains("Steps") || message.contains("step limit"),
                        "{name}: {message}"
                    );
                    receipt(root, "h-differential", &format!("INCOMPLETE {message}"));
                }
                mismatch => panic!("{name}: {mismatch:?}"),
            }
        }
    }
    let ready = build(&input, &options).unwrap();
    let sites = Sites::new(input.snapshot());
    let mut runnables = Vec::new();
    for backend in [Backend::Llvm, Backend::Cranelift] {
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
        runnables.push((runnable, dir));
    }
    let recovery = recovery_plan().is_some();
    let mut recovery_events = None;
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
        if recovery && accepted(root, &live_id) {
            continue;
        }
        let newly_recorded = !accepted(root, &live_id);
        if newly_recorded {
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
            fs::write(dir.join("live-streams.json"), serde_json::to_vec_pretty(&json!({"stdout":recorded.stdout,"stderr":recorded.stderr,"exit":recorded.exit})).unwrap()).unwrap();
            assert_eq!(observe(&recorded, &ready, &sites).unwrap(), *expected);
            assert_eq!(recorded.stderr, row["stderr"].as_str().unwrap().as_bytes());
        }
        let bytes = fs::read(&tape).unwrap();
        let recording = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
        assert_eq!(recording.arguments, recipe.args);
        assert_eq!(recording.tree.files, recipe.files);
        recording.bound_run().unwrap();
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
            if recovery {
                let file = recording
                    .events
                    .iter()
                    .find_map(|e| match (&e.request, &e.completion) {
                        (
                            hirsel_rt::io::Request::FileRead { file, .. },
                            hirsel_rt::io::Completion::Blocked { committed: false },
                        ) => Some(*file),
                        _ => None,
                    })
                    .unwrap();
                assert!(recording.events.iter().any(|e| matches!(e.request, hirsel_rt::io::Request::Close { handle } if handle == file)), "parked file leaked");
                assert!(
                    recording.events.iter().any(|e| matches!(
                        &e.completion,
                        hirsel_rt::io::Completion::Signal(Ok(
                            hirsel_run_schema::SignalKind::Interrupt
                        ))
                    )),
                    "scripted Interrupt never delivered"
                );
            }
        }
        if newly_recorded {
            accept(root, &live_id);
        }
        if recovery {
            if let Some(events) = &recovery_events {
                assert_eq!(&recording.events, events);
            }
            recovery_events = Some(recording.events.clone());
        }
        if !recovery {
            if let Some(reference) = &reference {
                assert_eq!(&semantic(&recording), reference);
            }
            reference = Some(semantic(&recording));
        }
        let replay_recipe = Fixture {
            files: BTreeMap::new(),
            file_reads: BTreeMap::new(),
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
                    accept(root, &replay_id);
                }
                Err(error) => {
                    let message = error.to_string();
                    assert!(message.contains("Steps"), "{replay_id}: {message}");
                    receipt(root, &replay_id, &format!("INCOMPLETE {message}"));
                }
            }
        }
        if row["id"] == "inherited-allow" {
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

fn prepare_product_inputs(root: &Path, manifest: &Path) -> PathBuf {
    // Ordinary products are immutable action inputs. Package loading and the
    // qualification closure use the same tree before bounded cells start.
    let products = PathBuf::from(std::env::var_os("HIRSEL_PRODUCTS").unwrap());
    hirsel_engine::with_compiler_stack(|| {
        hirsel_driver::load_program_source(manifest, None)
            .unwrap_or_else(|e| panic!("ordinary package setup: {e}"));
    });
    if recovery_plan().is_some() {
        return products;
    }
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
    products
}

// Selected original vectors remain individually named in the durable receipts.
fn campaign(phase: &str) {
    let plan = recovery_plan();
    if let Some(plan) = &plan {
        assert_eq!(phase, "native");
        validate_recovery_plan(plan, &std::env::var("POLICY_EVALUATOR_33_CASE").unwrap()).unwrap();
        let cell = PathBuf::from(std::env::var_os("POLICY_EVALUATOR_33_RESUME_ROOT").unwrap());
        assert!(
            !cell.exists(),
            "recovery requires a unique new single-cell root"
        );
        assert!(!plan["phases"].as_array().unwrap().is_empty());
    }
    let root = scratch(phase);
    let manifest = package(&root);
    let products = prepare_product_inputs(&root, &manifest);
    let mut receipts = Vec::new();
    for row in cases() {
        let id = row["id"].as_str().unwrap();
        if std::env::var("POLICY_EVALUATOR_33_CASE").is_ok_and(|s| !s.split(',').any(|v| v == id)) {
            continue;
        }
        let cell = std::env::var_os("POLICY_EVALUATOR_33_RESUME_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(id));
        fs::create_dir_all(&cell).unwrap();
        fs::write(cell.join("row.json"), row.to_string()).unwrap();
        let output = Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "120", "prlimit", "--cpu=60:60"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "policy_evaluator_33::policy_evaluator_33_cell",
                "--ignored",
                "--nocapture",
            ])
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_PRODUCTS", &products)
            .env("POLICY_EVALUATOR_33_ROW_FILE", cell.join("row.json"))
            .env("POLICY_EVALUATOR_33_ROOT", &cell)
            .env("POLICY_EVALUATOR_33_PHASE", phase)
            .output()
            .unwrap();
        fs::write(cell.join("stdout"), &output.stdout).unwrap();
        fs::write(cell.join("stderr"), &output.stderr).unwrap();
        let incomplete = cell.join("incomplete.json").exists();
        let receipt = json!({"id":id,"phase":phase,"selected":true,"started":true,"completed":output.status.success() && !incomplete,"passed":output.status.success() && !incomplete,"status":output.status.code(),"incomplete":incomplete,"root":cell});
        eprintln!("POLICY_EVALUATOR_33 {receipt}");
        receipts.push(receipt);
        fs::write(
            root.join("receipts.json"),
            serde_json::to_vec_pretty(&receipts).unwrap(),
        )
        .unwrap();
    }
    assert!(!receipts.is_empty(), "zero selected vectors");
    assert!(
        receipts.iter().all(|r| r["passed"] == true),
        "failed/incomplete cells: {receipts:?}"
    );
}

#[test]
fn policy_evaluator_33_simulation() {
    campaign("simulation");
}

#[test]
fn policy_evaluator_33_native_record_replay() {
    campaign("native");
}

#[test]
#[ignore = "bounded per-vector worker, launched by the campaign"]
fn policy_evaluator_33_cell() {
    hirsel_engine::with_compiler_stack(|| {
        let root = PathBuf::from(std::env::var_os("POLICY_EVALUATOR_33_ROOT").unwrap());
        let row: Value = serde_json::from_slice(
            &fs::read(std::env::var_os("POLICY_EVALUATOR_33_ROW_FILE").unwrap()).unwrap(),
        )
        .unwrap();
        let manifest = package(&root);
        let recipe = fixture(&row, row["jobs"].as_u64().unwrap_or(4));
        if std::env::var("POLICY_EVALUATOR_33_PHASE").unwrap() == "native" {
            let mut loaded =
                hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
            loaded
                .check_for_execution()
                .unwrap_or_else(|e| panic!("{e}"));
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
            let prepared = hirsel_driver::request_tir(&manifest, hirsel_engine::Verify::On, None)
                .unwrap_or_else(|e| panic!("{e}"));
            if simulation(&prepared, &row, &recipe).is_none() {
                return;
            }
        }
        fs::write(
            root.join("passed.json"),
            json!({"id":row["id"],"jobs":4,"passed":true}).to_string(),
        )
        .unwrap();
    });
}

#[test]
#[ignore = "selected exact-capacity pool cell"]
fn policy_evaluator_33_ordered_capacity() {
    hirsel_engine::with_compiler_stack(|| {
        let (backend, width, kind) =
            super::width_capacity_36::selection("POLICY_EVALUATOR_33_ORDERED");
        let root = scratch("ordered");
        let _evidence = super::width_capacity_36::Evidence(root.clone());
        let manifest = package(&root);
        let id = match kind.as_str() {
            "success" => "inherited-allow",
            "domain" => "domain-json",
            "cancel" => "pending-cancellation",
            _ => unreachable!(),
        };
        let row = cases().into_iter().find(|r| r["id"] == id).unwrap();
        let recipe = fixture(&row, 4);
        super::width_capacity_36::fixture_capacity(
            &manifest,
            &root,
            &recipe,
            backend,
            width,
            |observed, recording| {
                assert_eq!(observed.exit_code, row["exit"].as_i64().map(|n| n as i32));
                assert_eq!(observed.output, row["stdout"].as_str().unwrap().as_bytes());
                assert!(observed.failures.is_empty() && !observed.blocked);
                assert_eq!(recording.arguments, recipe.args);
                assert_eq!(recording.tree.files, recipe.files);
                let stderr: Vec<_> = recording
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
                    assert!(recording.events.iter().any(|e| matches!(
                        (&e.request, &e.completion),
                        (
                            hirsel_rt::io::Request::FileRead { .. },
                            hirsel_rt::io::Completion::Blocked { committed: false }
                        )
                    )));
                }
            },
        );
    });
}
