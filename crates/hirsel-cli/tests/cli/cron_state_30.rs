//! The complete scheduler entry, with independently fixed application vectors.
use hirsel_interp::observation::Effect;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn package(root: &Path) -> PathBuf {
    for (name, body) in [
        (
            "main",
            include_str!("../../../../examples/cronstate/main.hir"),
        ),
        (
            "spec",
            include_str!("../../../../examples/cronstate/spec.hir"),
        ),
        (
            "plan",
            include_str!("../../../../examples/cronstate/plan.hir"),
        ),
        (
            "state",
            include_str!("../../../../examples/cronstate/state.hir"),
        ),
        (
            "run",
            include_str!("../../../../examples/cronstate/run.hir"),
        ),
        (
            "report",
            include_str!("../../../../examples/cronstate/report.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), body).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/cronstate/hirsel.json")).unwrap();
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
fn scratch() -> tempfile::TempDir {
    let base = std::env::var_os("CRON_STATE_30_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("cron-state-")
        .tempdir_in(base)
        .unwrap()
}
fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/cron_state_30/cases.json")).unwrap()
}
#[test]
fn cron_state_30_reference_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let temp = scratch();
        let selection = package(temp.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut selected = cases();
        if let Ok(filter) = std::env::var("CRON_STATE_30_CASE") {
            selected.retain(|c| filter.split(',').any(|id| c["id"] == id));
        }
        assert!(!selected.is_empty());
        let mut receipts = vec![];
        let mut problems = vec![];
        for case in selected {
            let id = case["id"].as_str().unwrap();
            let mut world = hirsel_engine::SimulationConfig {
                args: serde_json::from_value(case["args"].clone()).unwrap(),
                wall: case["wall"].as_i64().unwrap_or(0),
                ..Default::default()
            };
            world
                .projections
                .extend(["wall_clock".into(), "monotonic_clock".into()]);
            world
                .grants
                .extend(["WallClock".into(), "MonotonicClock".into()]);
            world.mounts = [
                ("config".into(), hirsel_run_schema::Access::Read),
                (
                    "state".into(),
                    if case["deny"] == "state" {
                        hirsel_run_schema::Access::Read
                    } else {
                        hirsel_run_schema::Access::ReadWrite
                    },
                ),
            ]
            .into();
            world.directories = ["config".into(), "state".into()].into();
            if case["pending"] == true && case["already_expired"] != true {
                world.file_reads.insert(
                    "config/schedule.json".into(),
                    hirsel_run_schema::FileReadScript::Pending,
                );
            }
            if case["lock"] == true {
                world.directories.insert("state/owner.lock".into());
            }
            let mut raw = case["config"].to_string().into_bytes();
            if let Some(size) = case["raw_size"].as_u64() {
                raw.resize(size as usize, b' ');
            }
            world.files.insert("config/schedule.json".into(), raw);
            if !case["prior"].is_null() {
                let mut raw = case["prior"]
                    .as_str()
                    .map_or_else(|| case["prior"].to_string(), str::to_owned)
                    .into_bytes();
                if let Some(size) = case["state_size"].as_u64() {
                    raw.resize(size as usize, b' ');
                }
                world.files.insert("state/state.json".into(), raw);
            }
            if let Some(op) = case["fault"].as_str() {
                world.faults.insert(
                    op.into(),
                    [hirsel_interp::library::simulation::Fault::File {
                        error: hirsel_interp::library::simulation::FileError::PermissionDenied,
                        partial: 0,
                    }]
                    .into(),
                );
            }
            let children:Vec<Value>=case["config"]["jobs"].as_array().unwrap().iter().map(|j|json!({"program":j["program"],"args":j["argv"],"steps":[{"print":[79,79]},{"print_err":[69,69]},{"exit":{"exited":if j["argv"][0]=="fail" {7}else{0}}}]})).collect();
            world.process=Some(serde_json::from_value(json!({"provider":"process.script@1","rules":if case["deny"]=="spawn"{json!([])}else{json!([{"program":"/fixture/child"}])},"runs":[],"children":if case["missing_child"]==true{vec![]}else{children}})).unwrap());
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                eprintln!("CRON_STATE_30_STARTED {id}/{engine}");
                let bound = case["steps"].as_u64().unwrap();
                let execution = if engine == "reference" {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        bound,
                    )
                } else {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        bound,
                    )
                };
                let mut receipt =
                    json!({"id":id,"engine":engine,"requested_steps":bound,"state":"completed"});
                match execution {
                    Ok(execution) => {
                        receipt["step_usage"] = json!(format!("{:?}", execution.step_usage));
                        receipt["outcome"] = json!(format!("{:?}", execution.outcome));
                        if let Some(obs) = execution.observation {
                            let rows: Result<Vec<Value>, _> = String::from_utf8_lossy(&obs.output)
                                .lines()
                                .map(serde_json::from_str)
                                .collect();
                            let stderr = obs.final_state.as_ref().unwrap().stderr.clone();
                            let expected_stderr =
                                case["error"].as_str().map_or_else(Vec::new, |e| {
                                    format!("{}\n", json!({"error":e})).into_bytes()
                                });
                            let passed = obs.exit_code == case["exit"].as_i64().map(|n| n as i32)
                                && rows.as_ref().ok() == case["rows"].as_array()
                                && stderr == expected_stderr
                                && obs.failures.is_empty()
                                && obs.blocked.is_none();
                            receipt["passed"] = json!(passed);
                            receipt["actual"] = json!({"exit":obs.exit_code,"rows":rows.ok(),"stderr":String::from_utf8_lossy(&stderr)});
                            if !passed {
                                problems.push(format!("{id}/{engine}: {receipt}"));
                            }
                            let events: Vec<_> = obs
                                .effects
                                .iter()
                                .filter_map(|e| {
                                    if let Effect::CapabilityConsumed(e) = &e.effect {
                                        Some(e)
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            let witness=events.iter().map(|e|json!({"operation":e.operation,"input":e.input,"output":e.output})).collect::<Vec<_>>();
                            fs::write(
                                temp.path().join(format!("{id}-{engine}-events.json")),
                                serde_json::to_vec_pretty(&witness).unwrap(),
                            )
                            .unwrap();
                            fs::write(
                                temp.path().join(format!("{id}-{engine}-state.txt")),
                                format!("{:?}", obs.final_state),
                            )
                            .unwrap();
                            if case["pending"] == true && case["already_expired"] != true {
                                assert!(
                                    events.iter().any(|e| e.operation == "file.read.pending"),
                                    "read must actually park"
                                );
                            }
                            if let Some(final_state) = &obs.final_state {
                                for (_, (dirs, files)) in &final_state.trees {
                                    if case["lock"] != true {
                                        assert!(
                                            !dirs.contains("state/owner.lock"),
                                            "lease released after joining"
                                        );
                                    }
                                    if let Some(bytes) = files.get("state/state.json") {
                                        let book: Value = serde_json::from_slice(bytes).unwrap();
                                        if id == "gap-over" || id == "refusal-retained" {
                                            assert_eq!(book["refused"], true);
                                        }
                                        if id == "uncertain-future" {
                                            assert_eq!(book["rows"][0]["at"], 1);
                                            assert_eq!(book["rows"][0]["status"], "success");
                                        }
                                        if id == "backward-clock" {
                                            assert_eq!(book["rows"][0]["at"], 3);
                                            assert_eq!(book["seen_ns"], 180_000_000_000i64);
                                        }
                                    }
                                }
                            }
                            let spawns = events
                                .iter()
                                .filter(|e| e.operation == "proc.spawn")
                                .count();
                            receipt["spawns"] = json!(spawns);
                            if case["exit"] == 2 || case["exit"] == 3 {
                                assert_eq!(spawns, 0, "{id}/{engine}");
                            }
                            if let Some(previous) = &previous {
                                assert_eq!(&obs, previous, "{id}: complete observation");
                            }
                            previous = Some(obs);
                        } else {
                            receipt["passed"] = json!(false);
                            let expected_cut = ((id == "capacity-16" && bound == 10_000_000)
                                || (id == "backward-clock" && bound == 1_000_000))
                                && matches!(&execution.outcome,Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind=="Steps" && limit.limit==bound)
                                && execution
                                    .step_usage
                                    .is_some_and(|u| u.limit == Some(bound) && u.consumed == bound);
                            receipt["disposition"] = json!(if expected_cut {
                                "INCOMPLETE: original Steps at declared bound"
                            } else {
                                "unexpected noncompletion"
                            });
                            if !expected_cut {
                                problems.push(format!("{id}/{engine}: no completed observation"));
                            }
                        }
                    }
                    Err(error) => {
                        receipt["error"] = json!(error.to_string());
                        problems.push(format!("{id}/{engine}: {error}"));
                    }
                }
                eprintln!("CRON_STATE_30_RECEIPT {receipt}");
                receipts.push(receipt);
                fs::write(
                    temp.path().join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        let retained = temp.keep();
        assert!(
            problems.is_empty(),
            "evidence {}: {}",
            retained.display(),
            problems.join("\n")
        );
    });
}
fn native(backend: &str) {
    let temp = scratch();
    let manifest = package(temp.path());
    let script = temp.path().join("native.py");
    fs::write(&script, include_str!("../support/cron_state_30/native.py")).unwrap();
    let capped = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/capped")
        .canonicalize()
        .unwrap();
    let output = std::process::Command::new("python3")
        .arg(script)
        .arg("--root")
        .arg(temp.path().join("native"))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--manifest")
        .arg(&manifest)
        .arg("--capped")
        .arg(capped)
        .arg("--backend")
        .arg(backend)
        .output()
        .unwrap();
    fs::write(temp.path().join("stdout"), &output.stdout).unwrap();
    fs::write(temp.path().join("stderr"), &output.stderr).unwrap();
    let retained = temp.keep();
    assert!(
        output.status.success(),
        "{}: {}\n{}",
        retained.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Replays use the checked host adapter, with no live grant admission.
    hirsel_engine::with_compiler_stack(|| {
        for mode in ["success", "error", "cancel"] {
            for width in [1, 2, 4] {
                let d = retained.join(format!("native/{backend}-{mode}-{width}"));
                let ordinary = hirsel_driver::replay_observations(
                    &manifest,
                    &d.join("record.tape"),
                    Some(10_000_000),
                )
                .unwrap();
                let checked = hirsel_driver::replay_observations_checked(
                    &manifest,
                    &d.join("record.tape"),
                    Some(10_000_000),
                )
                .unwrap();
                assert!(ordinary.equivalent(&checked));
                assert_eq!(ordinary.output, fs::read(d.join("stdout")).unwrap());
            }
        }
    });
}
#[test]
fn cron_state_30_ordered_llvm() {
    native("llvm")
}
#[test]
fn cron_state_30_ordered_cranelift() {
    native("cranelift")
}

#[test]
#[ignore = "replays immutable external campaign tapes; requires CRON_STATE_30_REPLAY_INDEX"]
fn cron_state_30_replay_retained() {
    use sha2::{Digest, Sha256};
    hirsel_engine::with_compiler_stack(|| {
        let index = PathBuf::from(
            std::env::var_os("CRON_STATE_30_REPLAY_INDEX")
                .expect("explicit immutable replay inventory"),
        );
        let cells: Vec<Value> = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        assert!(!cells.is_empty());
        let mut receipts = vec![];
        let mut errors = vec![];
        for cell in cells {
            let tape = PathBuf::from(cell["tape"].as_str().unwrap());
            let selection = PathBuf::from(cell["manifest"].as_str().unwrap());
            let original = fs::read(&tape).unwrap();
            let mut previous = None;
            let bound = cell["steps"].as_u64().unwrap_or(20_000_000);
            for engine in ["reference", "checked-h"] {
                // Externally retained complete greens are not executed again.
                if cell["skip"]
                    .as_array()
                    .is_some_and(|xs| xs.contains(&json!(engine)))
                {
                    continue;
                }
                let execution = if engine == "reference" {
                    hirsel_driver::replay_observations(&selection, &tape, Some(bound))
                } else {
                    hirsel_driver::replay_observations_checked(&selection, &tape, Some(bound))
                };
                let mut row = json!({"id":cell["id"],"engine":engine,"tape":tape,"requested_steps":bound,"sha256":format!("{:x}",Sha256::digest(&original))});
                match execution {
                    Ok(observed) => {
                        let expected = fs::read(cell["stdout"].as_str().unwrap()).unwrap();
                        let good = observed.output == expected
                            && observed.exit_code == cell["exit"].as_i64().map(|n| n as i32)
                            && observed.failures.is_empty()
                            && observed.blocked.is_none();
                        if let Some(prior) = &previous {
                            assert!(observed.equivalent(prior));
                        }
                        previous = Some(observed);
                        row["passed"] = json!(good);
                        if !good {
                            errors.push(row.clone());
                        }
                    }
                    Err(e) => {
                        row["error"] = json!(e.to_string());
                        row["passed"] = json!(false);
                        errors.push(row.clone());
                    }
                }
                receipts.push(row);
                fs::write(
                    index.with_extension("replay-receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
            assert_eq!(fs::read(&tape).unwrap(), original);
        }
        assert!(errors.is_empty(), "{errors:?}");
    });
}
#[test]
#[ignore = "external native workflow campaign; requires CRON_STATE_30_BINARY and BACKEND"]
fn cron_state_30_native_workflows() {
    let temp = scratch();
    let script = temp.path().join("workflows.py");
    fs::write(
        &script,
        include_str!("../support/cron_state_30/workflows.py"),
    )
    .unwrap();
    let case_path = temp.path().join("cases.json");
    fs::write(
        &case_path,
        include_str!("../support/cron_state_30/cases.json"),
    )
    .unwrap();
    let root = temp.path().join("workflows");
    let capped = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/capped")
        .canonicalize()
        .unwrap();
    let output = std::process::Command::new("python3")
        .arg(script)
        .arg("--root")
        .arg(&root)
        .arg("--binary")
        .arg(std::env::var_os("CRON_STATE_30_BINARY").unwrap())
        .arg("--backend")
        .arg(std::env::var_os("CRON_STATE_30_BACKEND").unwrap())
        .arg("--capped")
        .arg(capped)
        .arg("--cases")
        .arg(case_path)
        .output()
        .unwrap();
    fs::write(temp.path().join("stdout"), &output.stdout).unwrap();
    fs::write(temp.path().join("stderr"), &output.stderr).unwrap();
    let retained = temp.keep();
    eprintln!("CRON_STATE_30_NATIVE_WORKFLOWS {}", retained.display());
    assert!(
        output.status.success(),
        "{}: {}\n{}",
        retained.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "audits immutable external tapes; requires CRON_STATE_30_REPLAY_INDEX"]
fn cron_state_30_persistence_tape_audit() {
    use hirsel_rt::io::{Completion, Request};
    let index = PathBuf::from(
        std::env::var_os("CRON_STATE_30_REPLAY_INDEX")
            .expect("explicit immutable replay inventory"),
    );
    let cells: Vec<Value> = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
    assert!(!cells.is_empty());
    let mut receipts = vec![];
    for cell in cells {
        let path = PathBuf::from(cell["tape"].as_str().unwrap());
        let tape = hirsel_rt::host_launch::read_recording(&path).unwrap();
        let mut files = std::collections::BTreeMap::new();
        let mut staged = None;
        let mut durable: Option<Value> = None;
        let mut renamed = false;
        let mut stage_synced = false;
        let mut unused = 0;
        let mut spawns = 0;
        let mut waits = 0;
        for event in &tape.events {
            match (&event.request, &event.completion) {
                (
                    Request::Create { path, .. } | Request::Open { path, .. },
                    Completion::Handle(Ok(handle)),
                ) => {
                    files.insert(*handle, path.clone());
                }
                (Request::FileAppend { file, bytes, .. }, Completion::Unit(Ok(()))) => {
                    if files.get(file).is_some_and(|p| p == "pending.json") {
                        staged = Some(serde_json::from_slice::<Value>(bytes).unwrap());
                        renamed = false;
                        stage_synced = false;
                    }
                }
                (Request::Sync { file, .. }, Completion::Unit(Ok(()))) => {
                    if files.get(file).is_some_and(|p| p == "pending.json") {
                        stage_synced = true;
                    }
                    if files.get(file).is_some_and(|p| p == "state.json") && renamed {
                        assert!(
                            stage_synced,
                            "{}: staged bytes/listing must be durable before publication",
                            path.display()
                        );
                        let next = staged.as_ref().unwrap();
                        let additions = next["rows"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|r| {
                                r["status"] == "admitted"
                                    && durable.as_ref().is_none_or(|old: &Value| {
                                        !old["rows"].as_array().unwrap().iter().any(|before| {
                                            before["id"] == r["id"]
                                                && before["at"] == r["at"]
                                                && before["status"] == "admitted"
                                        })
                                    })
                            })
                            .count();
                        unused += additions;
                        durable = Some(next.clone());
                        renamed = false;
                    }
                }
                (Request::Rename { from, to, .. }, Completion::Unit(Ok(())))
                    if from == "pending.json" && to == "state.json" =>
                {
                    renamed = true;
                }
                (Request::ProcessSpawn { .. }, _) => {
                    assert!(
                        unused > 0,
                        "{}: process starts only after a new durable admission",
                        path.display()
                    );
                    unused -= 1;
                    spawns += 1;
                }
                (Request::ChildWait { .. }, Completion::ChildExit(_)) => {
                    waits += 1;
                }
                _ => {}
            }
        }
        receipts.push(json!({"id":cell["id"],"spawns":spawns,"waits":waits,"passed":true}));
    }
    fs::write(
        index.with_extension("audit.json"),
        serde_json::to_vec_pretty(&receipts).unwrap(),
    )
    .unwrap();
}
#[test]
#[ignore = "immutable tape controls; requires CRON_STATE_30_REPLAY_INDEX and BINARIES"]
fn cron_state_30_tape_refusals() {
    hirsel_engine::with_compiler_stack(|| {
        let index = PathBuf::from(std::env::var_os("CRON_STATE_30_REPLAY_INDEX").unwrap());
        let cells: Vec<Value> = serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        let cell = &cells[0];
        let tape = PathBuf::from(cell["tape"].as_str().unwrap());
        let selection = PathBuf::from(cell["manifest"].as_str().unwrap());
        let original = fs::read(&tape).unwrap();
        let recording = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
        let mut controls = vec![];
        let mut changed = recording.clone();
        if let hirsel_rt::io::Request::Directory { path, .. } = &mut changed
            .events
            .iter_mut()
            .find(|e| matches!(e.request, hirsel_rt::io::Request::Directory { .. }))
            .unwrap()
            .request
        {
            path.push_str("-changed");
        }
        controls.push(("changed-input", changed));
        let mut extra = recording.clone();
        extra.events.push(recording.events[0].clone());
        controls.push(("extra-event", extra));
        let mut missing = recording.clone();
        missing.events.remove(0);
        controls.push(("missing-event", missing));
        let mut config = recording.clone();
        config.binding.as_mut().unwrap().canonical.push(b' ');
        controls.push(("noncanonical-config", config));
        let mut args = recording.clone();
        args.arguments.push("changed".into());
        controls.push(("changed-args", args));
        let binaries: Vec<PathBuf> =
            std::env::split_paths(&std::env::var_os("CRON_STATE_30_BINARIES").unwrap()).collect();
        assert_eq!(binaries.len(), 2);
        let mut receipts = vec![];
        let skipped = std::env::var("CRON_STATE_30_CONTROL_SKIP").unwrap_or_default();
        for (name, record) in controls {
            let path = index.with_extension(format!("{name}.tape"));
            fs::write(&path, hirsel_rt::launch::render_recording(&record).unwrap()).unwrap();
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "reference" };
                if skipped
                    .split(',')
                    .any(|id| id == format!("{name}/{engine}"))
                {
                    continue;
                }
                let result = if checked {
                    hirsel_driver::replay_observations_checked(&selection, &path, Some(20_000_000))
                } else {
                    hirsel_driver::replay_observations(&selection, &path, Some(20_000_000))
                };
                assert!(result.is_err(), "{name}: altered recording accepted");
                receipts.push(json!({"control":name,"engine":if checked{"checked-h"}else{"reference"},"refusal":result.unwrap_err().to_string()}));
            }
            for binary in &binaries {
                let engine = binary.file_name().unwrap().to_string_lossy();
                if skipped
                    .split(',')
                    .any(|id| id == format!("{name}/{engine}"))
                {
                    continue;
                }
                let out = std::process::Command::new(binary)
                    .arg("--hirsel-replay")
                    .arg(&path)
                    .env("HIRSEL_JOBS", "4")
                    .output()
                    .unwrap();
                fs::write(
                    index.with_extension(format!("{name}-{engine}.stdout")),
                    &out.stdout,
                )
                .unwrap();
                fs::write(
                    index.with_extension(format!("{name}-{engine}.stderr")),
                    &out.stderr,
                )
                .unwrap();
                assert_eq!(out.status.code(), Some(3), "{name}: {out:?}");
                if name == "extra-event" {
                    assert_eq!(
                        out.stdout,
                        fs::read(cell["stdout"].as_str().unwrap()).unwrap()
                    );
                } else {
                    assert!(out.stdout.is_empty(), "{name}/{engine}: {out:?}");
                }
                receipts.push(json!({"control":name,"engine":binary.file_name().unwrap().to_string_lossy(),"exit":3,"stderr":String::from_utf8_lossy(&out.stderr)}));
            }
            fs::write(
                index.with_extension("controls.json"),
                serde_json::to_vec_pretty(&receipts).unwrap(),
            )
            .unwrap();
        }
        assert_eq!(fs::read(tape).unwrap(), original);
        fs::write(
            index.with_extension("controls.json"),
            serde_json::to_vec_pretty(&receipts).unwrap(),
        )
        .unwrap();
    });
}
#[test]
#[ignore = "exact current native Fixture projection witness; not a semantic pass"]
fn cron_state_30_public_fixture_witness() {
    use hirsel_driver::native::*;
    hirsel_engine::with_compiler_stack(|| {
        let temp = scratch();
        let selection = package(temp.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let exported = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(exported).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
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
        let tools = Toolchain::for_matrix(&temp.path().join("runtime"), &configurations).unwrap();
        // This is the independently fixed pending-config observation, already
        // completed on R/H. No helper program replaces the actual main entry.
        let expected = Observation {
            exit_code: Some(1),
            blocked: false,
            budget: None,
            signaled: None,
            output: vec![],
            failures: vec![],
            roots: vec![],
        };
        let fixture = Fixture {
            file_reads: [(
                "config/schedule.json".into(),
                hirsel_run_schema::FileReadScript::Pending,
            )]
            .into(),
            monotonic: Some(0),
            steps: Some(1_000_000),
            ..Default::default()
        };
        let results = differential_in(
            &validated,
            &loaded.entry,
            &expected,
            &configurations,
            &tools,
            temp.path(),
            &fixture,
        );
        let mut receipts = vec![];
        for (engine, outcome) in results {
            receipts.push(json!({"engine":engine,"outcome":format!("{outcome:?}"),"disposition":"INCOMPLETE"}));
            // The public Fixture has no Args or mounts/initial-file input.
            // Its launcher supplies empty Args: actual main returns usage2.
            assert!(
                matches!(&outcome,Outcome::Mismatch(actual) if actual.exit_code==Some(2)),
                "current adapter behavior changed: {outcome:?}"
            );
        }
        fs::write(
            temp.path().join("projection-receipts.json"),
            serde_json::to_vec_pretty(&receipts).unwrap(),
        )
        .unwrap();
        let retained = temp.keep();
        eprintln!("CRON_STATE_30_PROJECTION_WITNESS {}", retained.display());
    });
}
#[test]
fn cron_state_30_crash_boundaries() {
    use hirsel_driver::native::*;
    hirsel_engine::with_compiler_stack(|| {
        let temp = scratch();
        let selection = package(temp.path());
        fs::write(
            temp.path().join("crash.hir"),
            include_str!("../support/cron_state_30/crash.hir"),
        )
        .unwrap();
        let mut manifest: Value = serde_json::from_slice(&fs::read(&selection).unwrap()).unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path":"crash.hir","module":[]}));
        manifest["entry"]["name"] = json!("crash_trial");
        fs::write(&selection, manifest.to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut config = hirsel_engine::SimulationConfig::default();
        config.grants.insert("Test".into());
        // Supplemental crash fixture has its own finite bound, frozen before execution.
        const STEPS: u64 = 5_000_000;
        let reference = hirsel_engine::run_observed_snapshot_bounded(
            &mut loaded.engine,
            &run,
            config.clone(),
            STEPS,
        )
        .unwrap();
        let checked =
            hirsel_engine::run_observed_checked_bounded(&mut loaded.engine, &run, config, STEPS)
                .unwrap();
        eprintln!(
            "CRON_STATE_30_CRASH_STEPS reference={:?} checked={:?}",
            reference.step_usage, checked.step_usage
        );
        let observation = reference
            .observation
            .as_ref()
            .expect("complete crash observation");
        assert_eq!(observation.exit_code, Some(0));
        assert!(observation.failures.is_empty());
        assert_eq!(reference.observation, checked.observation);
        let exported = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(exported).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
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
        let tools = Toolchain::for_matrix(&temp.path().join("runtime"), &matrix).unwrap();
        let fixture = Fixture {
            steps: Some(STEPS),
            ..Default::default()
        };
        for (engine, outcome) in differential_in(
            &validated,
            &loaded.entry,
            &Observation::of_reference(observation),
            &matrix,
            &tools,
            temp.path(),
            &fixture,
        ) {
            eprintln!("CRON_STATE_30_CRASH {engine}: {outcome:?}");
            assert_eq!(outcome, Outcome::Agree);
        }
        let retained = temp.keep();
        eprintln!("CRON_STATE_30_CRASH_EVIDENCE {}", retained.display());
    });
}
fn capped_native(command: &std::process::Command) -> std::process::Output {
    let cap = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/capped")
        .canonicalize()
        .unwrap();
    let mut output = std::process::Command::new(cap)
        .args([
            "--timeout",
            "15",
            "prlimit",
            "--cpu=60:61",
            "--as=2147483648",
        ])
        .arg(command.get_program())
        .args(command.get_args())
        .env("HIRSEL_JOBS", "4")
        .env("HIRSEL_MEMORY_LIMIT", "2G")
        .output()
        .unwrap();
    output.stderr = output
        .stderr
        .split_inclusive(|b| *b == b'\n')
        .filter(|line| !line.starts_with(b"capped:"))
        .flatten()
        .copied()
        .collect();
    output
}
#[test]
fn cron_state_30_numeric_step_native_and_tapes() {
    use sha2::{Digest, Sha256};
    let root = scratch().keep();
    let selection = package(root.as_path());
    let config = root.as_path().join("config");
    fs::create_dir(&config).unwrap();
    let case = cases()
        .into_iter()
        .find(|c| c["id"] == "large-step")
        .unwrap();
    fs::write(config.join("schedule.json"), case["config"].to_string()).unwrap();
    let expected = b"{\"id\":\"a\",\"next\":\"2012-07-09T16:00:00Z\"}\n";
    let grant = root.as_path().join("grants.json");
    let mut receipts = vec![];
    let mut reference = None;
    let binaries = [
        root.as_path().join("llvm"),
        root.as_path().join("cranelift"),
    ];
    for (backend, binary) in ["llvm", "cranelift"].into_iter().zip(&binaries) {
        let build = std::process::Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", backend])
            .arg(&selection)
            .arg("-o")
            .arg(binary)
            .env("HIRSEL_JOBS", "1")
            .output()
            .unwrap();
        fs::write(
            root.as_path().join(format!("{backend}-build.stdout")),
            &build.stdout,
        )
        .unwrap();
        fs::write(
            root.as_path().join(format!("{backend}-build.stderr")),
            &build.stderr,
        )
        .unwrap();
        assert!(build.status.success(), "{backend}: {build:?}");
        for width in [1, 2, 4] {
            let tape = root.as_path().join(format!("{backend}-{width}.tape"));
            let grants = json!({"args":case["args"],"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"config","host":config,"access":"read"}]},"placement":{"cores":width,"commit":"shared","reorder":"auto"}});
            fs::write(&grant, grants.to_string()).unwrap();
            let mut command = std::process::Command::new(binary);
            command
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(&grant)
                .args(
                    case["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap()),
                );
            let run = capped_native(&command);
            assert_eq!(run.status.code(), Some(0), "{backend}/{width}: {run:?}");
            assert_eq!(run.stdout, expected);
            assert!(run.stderr.is_empty());
            let mut semantic = hirsel_rt::host_launch::read_recording(&tape).unwrap();
            let binding = semantic.binding.as_mut().unwrap();
            binding.admitted_at = 0;
            binding.placement = None;
            if let Some(reference) = &reference {
                assert_eq!(
                    &semantic, reference,
                    "complete ordered events/payloads/IDs/errors; admission and placement only normalized"
                );
            } else {
                reference = Some(semantic);
            }
            receipts.push(json!({"id":format!("{backend}-{width}"),"tape":tape,"sha256":format!("{:x}",Sha256::digest(fs::read(&tape).unwrap())),"exit":0,"passed":true}));
        }
    }
    fs::rename(&config, root.as_path().join("config.unavailable")).unwrap();
    fs::rename(&grant, root.as_path().join("grants.unavailable")).unwrap();
    hirsel_engine::with_compiler_stack(|| {
        for receipt in &receipts {
            let tape = Path::new(receipt["tape"].as_str().unwrap());
            let original = fs::read(tape).unwrap();
            let ordinary =
                hirsel_driver::replay_observations(&selection, tape, Some(1_000_000)).unwrap();
            let checked =
                hirsel_driver::replay_observations_checked(&selection, tape, Some(1_000_000))
                    .unwrap();
            assert!(ordinary.equivalent(&checked));
            assert_eq!(ordinary.output, expected);
            assert_eq!(ordinary.exit_code, Some(0));
            for binary in &binaries {
                let mut command = std::process::Command::new(binary);
                command.arg("--hirsel-replay").arg(tape);
                let replay = capped_native(&command);
                assert_eq!(replay.status.code(), Some(0));
                assert_eq!(replay.stdout, expected);
                assert!(replay.stderr.is_empty());
            }
            assert_eq!(fs::read(tape).unwrap(), original);
        }
    });
    fs::write(
        root.as_path().join("receipts.json"),
        serde_json::to_vec_pretty(&receipts).unwrap(),
    )
    .unwrap();
    let retained = root;
    eprintln!("CRON_STATE_30_FINAL_NATIVE {}", retained.display());
}
#[test]
fn cron_state_30_past_deadline_native_replay() {
    let root = scratch().keep();
    let selection = package(&root);
    let data = root.join("config");
    fs::create_dir(&data).unwrap();
    let state = root.join("state");
    fs::create_dir(&state).unwrap();
    let case = cases()
        .into_iter()
        .find(|c| c["id"] == "past-until")
        .unwrap();
    fs::write(data.join("schedule.json"), case["config"].to_string()).unwrap();
    let grant = root.join("grants.json");
    let grants = json!({"args":case["args"],"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","tasks":"tasks.cooperative@1","wall_clock":"clock.wall.unix_ns@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[{"path":"config","host":data,"access":"read"},{"path":"state","host":state,"access":"read_write"}]},"placement":{"cores":1,"commit":"shared","reorder":"auto"}});
    fs::write(&grant, grants.to_string()).unwrap();
    let mut receipts = vec![];
    let binaries = [root.join("llvm"), root.join("cranelift")];
    for (backend, binary) in ["llvm", "cranelift"].into_iter().zip(&binaries) {
        let build = std::process::Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", backend])
            .arg(&selection)
            .arg("-o")
            .arg(binary)
            .env("HIRSEL_JOBS", "1")
            .output()
            .unwrap();
        fs::write(root.join(format!("{backend}-build.stderr")), &build.stderr).unwrap();
        assert!(build.status.success(), "{build:?}");
        let tape = root.join(format!("{backend}.tape"));
        let mut command = std::process::Command::new(binary);
        command
            .arg("--hirsel-record")
            .arg(&tape)
            .arg("--hirsel-grants")
            .arg(&grant)
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            );
        let out = capped_native(&command);
        fs::write(root.join(format!("{backend}.stdout")), &out.stdout).unwrap();
        fs::write(root.join(format!("{backend}.stderr")), &out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(out.stdout.is_empty());
        assert_eq!(out.stderr, b"{\"error\":\"run_until\"}\n");
        assert!(fs::read_dir(&state).unwrap().next().is_none());
        let recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
        assert!(
            !recording
                .events
                .iter()
                .any(|e| matches!(e.request, hirsel_rt::io::Request::ProcessSpawn { .. }))
        );
        receipts.push(json!({"backend":backend,"tape":tape,"exit":1,"passed":true}));
    }
    fs::rename(data, root.join("config.unavailable")).unwrap();
    fs::rename(state, root.join("state.unavailable")).unwrap();
    fs::rename(grant, root.join("grants.unavailable")).unwrap();
    hirsel_engine::with_compiler_stack(|| {
        for r in &receipts {
            let tape = Path::new(r["tape"].as_str().unwrap());
            let original = fs::read(tape).unwrap();
            let ordinary =
                hirsel_driver::replay_observations(&selection, tape, Some(1_000_000)).unwrap();
            let checked =
                hirsel_driver::replay_observations_checked(&selection, tape, Some(1_000_000))
                    .unwrap();
            assert!(ordinary.equivalent(&checked));
            assert_eq!(ordinary.exit_code, Some(1));
            assert!(ordinary.output.is_empty());
            assert!(ordinary.failures.is_empty());
            for binary in &binaries {
                let mut command = std::process::Command::new(binary);
                command.arg("--hirsel-replay").arg(tape);
                let out = capped_native(&command);
                assert_eq!(out.status.code(), Some(1));
                assert!(out.stdout.is_empty());
                assert_eq!(out.stderr, b"{\"error\":\"run_until\"}\n");
            }
            assert_eq!(fs::read(tape).unwrap(), original);
        }
    });
    fs::write(
        root.join("receipts.json"),
        serde_json::to_vec_pretty(&receipts).unwrap(),
    )
    .unwrap();
    eprintln!("CRON_STATE_30_PAST_NATIVE {}", root.display());
}
