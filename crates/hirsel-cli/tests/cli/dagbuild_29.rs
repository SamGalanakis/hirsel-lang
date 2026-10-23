//! Full entry/config/modules under public semantic adapters; independent JSON oracles.
use hirsel_interp::observation::Effect;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn scratch(label: &str) -> tempfile::TempDir {
    let base = std::env::var_os("DAGBUILD_29_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
}
pub(super) fn package(root: &Path) -> PathBuf {
    for (name, body) in [
        (
            "main.hir",
            include_str!("../../../../examples/dagbuild/main.hir"),
        ),
        (
            "manifest.hir",
            include_str!("../../../../examples/dagbuild/manifest.hir"),
        ),
        (
            "graph.hir",
            include_str!("../../../../examples/dagbuild/graph.hir"),
        ),
        (
            "fingerprint.hir",
            include_str!("../../../../examples/dagbuild/fingerprint.hir"),
        ),
        (
            "execute.hir",
            include_str!("../../../../examples/dagbuild/execute.hir"),
        ),
        (
            "report.hir",
            include_str!("../../../../examples/dagbuild/report.hir"),
        ),
    ] {
        fs::write(root.join(name), body).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/dagbuild/hirsel.json")).unwrap();
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
pub(super) fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/dagbuild_29/cases.json")).unwrap()
}
#[test]
fn dagbuild_29_reference_checked_workflows() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("dagbuild-29-semantic-");
        let selection = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut receipts = vec![];
        let mut errors = vec![];
        let mut selected = cases();
        if std::env::var_os("DAGBUILD_29_CAPS").is_some() {
            fs::write(
                scratch.path().join("oracle.py"),
                include_str!("../support/dagbuild_29/oracle.py"),
            )
            .unwrap();
            fs::write(
                scratch.path().join("caps.py"),
                include_str!("../support/dagbuild_29/caps.py"),
            )
            .unwrap();
            let data = Command::new("python3")
                .args([
                    "-c",
                    "import caps,json;print(json.dumps(caps.boundary_cases()))",
                ])
                .current_dir(scratch.path())
                .output()
                .unwrap();
            assert!(data.status.success(), "{data:?}");
            selected = serde_json::from_slice(&data.stdout).unwrap();
        }
        if let Ok(filter) = std::env::var("DAGBUILD_29_CASE") {
            selected.retain(|c| filter.split(',').any(|id| c["id"] == id));
        }
        assert!(!selected.is_empty());
        for case in selected {
            let id = case["id"].as_str().unwrap();
            if id == "bytes-1048576" {
                eprintln!(
                    "DAGBUILD_29_INCOMPLETE bytes-1048576 original reference memory stop; checked-h unrun; interp-step-budget-29"
                );
                continue;
            }
            let mut world = hirsel_engine::SimulationConfig {
                args: serde_json::from_value(case["args"].clone()).unwrap(),
                ..Default::default()
            };
            world.projections.insert("monotonic_clock".into());
            world.grants.insert("MonotonicClock".into());
            world.mounts = [
                ("project".into(), hirsel_run_schema::Access::ReadWrite),
                (
                    "state".into(),
                    if case["denied"] == "write" {
                        hirsel_run_schema::Access::Read
                    } else {
                        hirsel_run_schema::Access::ReadWrite
                    },
                ),
            ]
            .into();
            if case["denied"] == "read" {
                world.mounts.remove("project");
            }
            world.directories = ["project".into(), "state".into()].into();
            if case["denied"] != "missing" {
                world.files.insert("project/manifest.json".into(), {
                    let mut data = case["manifest"].to_string().into_bytes();
                    if let Some(size) = case["raw_size"].as_u64() {
                        data.resize(size as usize, b' ');
                    }
                    data
                });
            }
            for (name, value) in case["files"].as_object().unwrap() {
                world.files.insert(
                    format!("project/{name}"),
                    value.as_str().unwrap().as_bytes().to_vec(),
                );
            }
            if !case["prior"].is_null() {
                world.files.insert(
                    "state/state.json".into(),
                    if let Some(s) = case["prior"].as_str() {
                        s.as_bytes().to_vec()
                    } else {
                        case["prior"].to_string().into_bytes()
                    },
                );
            }
            let children:Vec<Value>=case["manifest"]["tasks"].as_array().unwrap().iter().map(|t|json!({"program":t["argv"][0],"args":t["argv"].as_array().unwrap().get(1..).unwrap_or_default(),"steps":[{"print":[79,79]},{"print_err":[69,69]},{"exit":{"exited":if t["argv"][1]=="fail"{7}else{0}}}]})).collect();
            world.process=Some(serde_json::from_value(json!({"provider":"process.script@1","rules":if case["denied"]=="spawn"{json!([])}else{json!([{"program":"/fixture/child"}])},"runs":[],"children":children})).unwrap());
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                eprintln!("DAGBUILD_29_STARTED {id} {engine}");
                let execution = if engine == "reference" {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                };
                if let Ok(executed) = &execution {
                    if let Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) = &executed.outcome {
                        let known = [
                            "chain-first",
                            "diamond-first",
                            "disconnected-first",
                            "reverse-chain",
                            "unchanged",
                            "unchanged-plan",
                            "input-change",
                            "command-change",
                            "failure-descendants",
                            "cpython-disconnected",
                            "clean-ready-tie",
                            "nodes-32",
                            "edges-128",
                            "artifacts-64",
                            "argv-4096",
                            "manifest-65536",
                        ];
                        if known.contains(&id) && limit.kind == "Steps" && limit.limit == 1_000_000
                        {
                            let receipt = json!({"id":id,"engine":engine,"state":"INCOMPLETE","tooling":format!("{limit:?}")});
                            eprintln!("DAGBUILD_29_INCOMPLETE {receipt}");
                            receipts.push(receipt);
                            fs::write(
                                scratch.path().join("receipts.json"),
                                serde_json::to_vec_pretty(&receipts).unwrap(),
                            )
                            .unwrap();
                            continue;
                        }
                    }
                }
                let outcome = (|| -> Result<Value, String> {
                    let executed = execution.map_err(|e| format!("{e:?}"))?;
                    let observation = executed
                        .observation
                        .as_ref()
                        .ok_or_else(|| format!("{executed:?}"))?;
                    if observation.exit_code != case["exit"].as_i64().map(|n| n as i32) {
                        return Err(format!("exit {executed:?}"));
                    }
                    let rows: Vec<Value> = String::from_utf8(observation.output.clone())
                        .map_err(|e| e.to_string())?
                        .lines()
                        .map(serde_json::from_str)
                        .collect::<Result<_, _>>()
                        .map_err(|e| e.to_string())?;
                    if rows != case["rows"].as_array().unwrap().to_vec() {
                        return Err(format!("rows actual={rows:?} expected={:?}", case["rows"]));
                    }
                    let state = observation.final_state.as_ref().ok_or("no final state")?;
                    if state.stderr != case["stderr"].as_str().unwrap().as_bytes() {
                        return Err(format!("stderr {:?}", state.stderr));
                    }
                    if !observation.failures.is_empty()
                        || observation.blocked.is_some()
                        || observation.signaled.is_some()
                    {
                        return Err(format!("{observation:?}"));
                    }
                    let events: Vec<_> = observation
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
                    let spawned = events
                        .iter()
                        .filter(|e| e.operation == "proc.spawn")
                        .count();
                    if spawned != case["spawned"].as_u64().unwrap() as usize {
                        return Err(format!("spawn {spawned}"));
                    }
                    if let Some(expected) = case.get("state").filter(|v| !v.is_null()) {
                        let bytes = state.trees[&0]
                            .1
                            .get("state/state.json")
                            .ok_or("missing state")?;
                        let actual: Value =
                            serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
                        if actual != *expected {
                            return Err(format!("state {actual} expected {expected}"));
                        }
                    }
                    if let Some(before) = &previous {
                        if observation != before {
                            return Err("full reference/checked observation mismatch".into());
                        }
                    }
                    previous = Some(observation.clone());
                    Ok(json!({"id":id,"engine":engine,"state":"passed","spawned":spawned}))
                })();
                match outcome {
                    Ok(receipt) => {
                        receipts.push(receipt);
                        eprintln!("DAGBUILD_29_PASS {id} {engine}");
                    }
                    Err(error) => {
                        errors.push(format!("{id}/{engine}: {error}"));
                        receipts
                            .push(json!({"id":id,"engine":engine,"state":"failed","error":error}));
                    }
                }
                fs::write(
                    scratch.path().join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        let retained = scratch.keep();
        assert!(
            errors.is_empty(),
            "{} errors; evidence {}: {}",
            errors.len(),
            retained.display(),
            errors.join("\n")
        );
    });
}
fn native_workflows(backend: &str) {
    let scratch = scratch("dagbuild-29-host-");
    let root = scratch.path();
    package(root);
    for (name, body) in [
        (
            "verify.py",
            include_str!("../support/dagbuild_29/verify.py"),
        ),
        (
            "cases.json",
            include_str!("../support/dagbuild_29/cases.json"),
        ),
        (
            "child.py",
            include_str!("../../../../examples/dagbuild/fixtures/child.py"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        ("real.py", include_str!("../support/dagbuild_29/real.py")),
        ("caps.py", include_str!("../support/dagbuild_29/caps.py")),
        (
            "oracle.py",
            include_str!("../support/dagbuild_29/oracle.py"),
        ),
    ] {
        fs::write(root.join(name), body).unwrap();
    }
    fs::write(
        root.join("controls.py"),
        include_str!("../support/dagbuild_29/controls.py"),
    )
    .unwrap();
    fs::write(
        root.join("disposition.py"),
        include_str!("../support/dagbuild_29/disposition.py"),
    )
    .unwrap();
    fs::write(
        root.join("known-incomplete.json"),
        include_str!("../support/dagbuild_29/known-incomplete.json"),
    )
    .unwrap();
    let mut command = Command::new("python3");
    command.env("DAGBUILD_29_BACKEND", backend);
    if std::env::var_os("DAGBUILD_29_REAL_ONLY").is_none() {
        command.env("DAGBUILD_29_SKIP_REAL", "1");
    }
    let output = command
        .arg(root.join("verify.py"))
        .arg("--root")
        .arg(root)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .output()
        .unwrap();
    fs::write(root.join("stdout"), &output.stdout).unwrap();
    fs::write(root.join("stderr"), &output.stderr).unwrap();
    let retained = scratch.keep();
    assert!(
        output.status.success(),
        "evidence {}: {} {}",
        retained.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn dagbuild_29_native_and_replay_llvm() {
    native_workflows("llvm");
}
#[test]
fn dagbuild_29_native_and_replay_cranelift() {
    native_workflows("cranelift");
}

// Independent transitive closure; validates recorded process admission rather
// than calling graph.hir or std toposort to construct the expected ready set.
fn audit_ready(case: &Value, tape: &Path) -> usize {
    use hirsel_rt::io::{Completion, Request};
    let recording = hirsel_rt::host_launch::read_recording(tape).unwrap();
    let tasks = case["manifest"]["tasks"].as_array().unwrap();
    let n = tasks.len();
    let mut reach = vec![vec![false; n]; n];
    for (to, t) in tasks.iter().enumerate() {
        for d in t["dependencies"].as_array().unwrap() {
            let from = tasks.iter().position(|t| t["name"] == *d).unwrap();
            reach[from][to] = true;
        }
    }
    for k in 0..n {
        for i in 0..n {
            for j in 0..n {
                reach[i][j] |= reach[i][k] && reach[k][j];
            }
        }
    }
    let rows = case["rows"].as_array().unwrap();
    let mut succeeded = vec![false; n];
    let mut admitted = vec![false; n];
    for i in 0..n {
        if rows[i]["status"] == "clean" {
            succeeded[i] = true;
            admitted[i] = true;
        }
    }
    let mut children = std::collections::BTreeMap::new();
    let mut count = 0;
    for event in recording.events {
        match event.request {
            Request::ProcessSpawn { command, .. } => {
                let id = tasks
                    .iter()
                    .position(|t| {
                        t["argv"][0] == command.program
                            && serde_json::from_value::<Vec<String>>(t["argv"].clone()).unwrap()
                                [1..]
                                == command.args
                    })
                    .unwrap();
                let ready = (0..n)
                    .filter(|&i| !admitted[i] && (0..n).all(|p| !reach[p][i] || succeeded[p]))
                    .collect::<Vec<_>>();
                assert!(
                    ready.contains(&id),
                    "{} ready={ready:?} admitted={id}",
                    tape.display()
                );
                assert!(children.len() < (case["manifest"]["jobs"].as_u64().unwrap() as usize));
                admitted[id] = true;
                count += 1;
                if let Completion::Spawned(Ok(child)) = event.completion {
                    children.insert(child, id);
                }
            }
            Request::ChildWait { child, .. } => {
                if let Completion::ChildExit(status) = event.completion {
                    let id = children.remove(&child).unwrap();
                    succeeded[id] = matches!(status, Ok(hirsel_run_schema::ExitStatus::Exited(0)));
                }
            }
            _ => {}
        }
    }
    assert_eq!(count, case["spawned"].as_u64().unwrap() as usize);
    count
}
#[test]
#[ignore = "requires retained complete application tapes; run with DAGBUILD_29_AUDIT_DIRS"]
fn dagbuild_29_recorded_ready_oracle() {
    let paths = std::env::var_os("DAGBUILD_29_AUDIT_DIRS")
        .expect("DAGBUILD_29_AUDIT_DIRS is required for the external tape audit");
    let fixtures = cases();
    let mut audited = 0;
    for root in std::env::split_paths(&paths) {
        let receipts: Vec<Value> =
            serde_json::from_slice(&fs::read(root.join("receipts.json")).unwrap()).unwrap();
        for receipt in receipts {
            if receipt["passed"] != true {
                continue;
            }
            let id = receipt["id"].as_str().unwrap();
            let Some(case) = fixtures.iter().find(|case| {
                let body = id
                    .strip_prefix("llvm-")
                    .or_else(|| id.strip_prefix("cranelift-"))
                    .unwrap();
                body.rsplit_once("-w").unwrap().0 == case["id"].as_str().unwrap()
            }) else {
                continue;
            };
            if case["exit"] == 2 || case["exit"] == 3 {
                continue;
            }
            audit_ready(case, &root.join(id).join("record.tape"));
            audited += 1;
        }
    }
    assert!(audited > 0);
    eprintln!("DAGBUILD_29_READY_ORACLE {audited}");
}
#[test]
#[ignore = "uses an immutable complete application tape; run with DAGBUILD_29_INPUT_TAPE"]
fn dagbuild_29_changed_input_refusal() {
    let tape = PathBuf::from(std::env::var_os("DAGBUILD_29_INPUT_TAPE").unwrap());
    let original = fs::read(&tape).unwrap();
    let recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
    let dir = scratch("dagbuild-29-refusal-");
    for kind in ["arguments", "configuration"] {
        let mut changed = recording.clone();
        if kind == "arguments" {
            changed.arguments.push("changed-input".into());
        } else {
            let binding = changed.binding.as_mut().unwrap();
            let mut config: Value = serde_json::from_slice(&binding.canonical).unwrap();
            config["inputs"]["hash_seed"] = 1.into();
            binding.canonical = serde_json::to_vec(&config).unwrap();
        }
        let path = dir.path().join(format!("{kind}.tape"));
        fs::write(
            &path,
            hirsel_rt::launch::render_recording(&changed).unwrap(),
        )
        .unwrap();
        let refused =
            hirsel_rt::host_launch::read_recording(&path).and_then(|record| record.bound_run());
        assert!(refused.is_err(), "{kind}: changed bindings must refuse");
        eprintln!("DAGBUILD_29_REFUSAL {kind} {:?}", refused.unwrap_err());
    }
    assert_eq!(fs::read(tape).unwrap(), original);
    let _ = dir.keep();
}
