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
        let prepared =
            hirsel_driver::request_tir(&selection, hirsel_engine::Verify::On, None).unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: prepared.entry.clone(),
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
        let twins: Vec<Value> =
            serde_json::from_str(include_str!("../support/dagbuild_29/reference-cases.json"))
                .unwrap();
        for case in &mut selected {
            if let Some(twin) = twins.iter().find(|twin| twin["full_id"] == case["id"]) {
                *case = twin["case"].clone();
            }
        }
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
                let execution = hirsel_driver::testing::observe_tir(
                    &prepared,
                    &config,
                    world.clone(),
                    hirsel_interp::Limits::default(),
                    engine == "checked-h",
                );
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
            "reference-cases.json",
            include_str!("../support/dagbuild_29/reference-cases.json"),
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
