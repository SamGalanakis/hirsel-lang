//! Complete pnmtiles entry/config/module workflows, with external integer vectors.
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_interp::observation::Effect;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn cases() -> Value {
    serde_json::from_str(include_str!("../support/pnm_tiles_30/cases.json")).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    if value.is_array() {
        return serde_json::from_value(value.clone()).unwrap();
    }
    let mut out: Vec<u8> = serde_json::from_value(value["prefix"].clone()).unwrap();
    out.extend(std::iter::repeat_n(
        value["value"].as_u64().unwrap() as u8,
        value["repeat"].as_u64().unwrap() as usize,
    ));
    out
}
fn scratch(label: &str) -> PathBuf {
    let base = std::env::var_os("PNM_TILES_30_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(&format!("pnm-tiles-30-{label}-"))
        .tempdir_in(base)
        .unwrap()
        .keep()
}
fn package(root: &Path) -> PathBuf {
    for (name, data) in [
        (
            "main.hir",
            include_str!("../../../../examples/pnmtiles/main.hir"),
        ),
        (
            "pnm.hir",
            include_str!("../../../../examples/pnmtiles/pnm.hir"),
        ),
        (
            "image.hir",
            include_str!("../../../../examples/pnmtiles/image.hir"),
        ),
        (
            "filter.hir",
            include_str!("../../../../examples/pnmtiles/filter.hir"),
        ),
        (
            "tiles.hir",
            include_str!("../../../../examples/pnmtiles/tiles.hir"),
        ),
        (
            "report.hir",
            include_str!("../../../../examples/pnmtiles/report.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "verify.py",
            include_str!("../support/pnm_tiles_30/verify.py"),
        ),
        (
            "cases.json",
            include_str!("../support/pnm_tiles_30/cases.json"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/pnmtiles/hirsel.json")).unwrap();
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
fn selected(row: &Value) -> bool {
    std::env::var("PNM_TILES_30_CASE").map_or(true, |v| v.split(',').any(|id| row["id"] == id))
}
fn simulated(large: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("simulation");
        let selection = package(&root);
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let data = cases();
        let mut receipts = Vec::new();
        let mut failures = Vec::new();
        for row in data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| selected(r) && (r["id"].as_str().unwrap().starts_with("cap-") == large))
        {
            let id = row["id"].as_str().unwrap();
            let mut world = SimulationConfig {
                args: serde_json::from_value(row["args"].clone()).unwrap(),
                projections: ["stdout", "stderr", "dir", "tasks"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                mounts: [(
                    "data".into(),
                    if row["access"] == "read" {
                        hirsel_run_schema::Access::Read
                    } else {
                        hirsel_run_schema::Access::ReadWrite
                    },
                )]
                .into(),
                files: [
                    (format!("data/{id}"), bytes(&data["files"][id])),
                    ("data/output".into(), b"preserve".to_vec()),
                ]
                .into(),
                ..Default::default()
            };
            world.directories.insert("data".into());
            if row["pending"] == true {
                world.projections.insert("signals".into());
                world.signals = Some(serde_json::from_value(json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]})).unwrap());
                world.file_reads.insert(
                    format!("data/{id}"),
                    hirsel_run_schema::FileReadScript::Pending,
                );
            }
            if let Some(operation) = row["fault"].as_str() {
                world.faults.insert(
                    operation.into(),
                    [hirsel_interp::library::simulation::Fault::File {
                        error: hirsel_interp::library::simulation::FileError::Fault(77),
                        partial: row["partial"].as_u64().unwrap() as usize,
                    }]
                    .into(),
                );
            }
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                if std::env::var("PNM_TILES_30_ENGINE")
                    .is_ok_and(|v| !v.split(',').any(|e| e == engine))
                {
                    continue;
                }
                eprintln!("PNM_TILES_30_STARTED {id} {engine}");
                let bound = row["bound"].as_u64().unwrap();
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
                let mut receipt = json!({"id":id,"engine":engine,"started":true,"completed":false,"passed":false,"bound":bound});
                let checked = execution.map_err(|e| e.to_string()).and_then(|execution| {
                    receipt["consumed"] = json!(execution.step_usage.map(|s| s.consumed));
                    if row["reference_disposition"] == "Steps" {
                        assert!(execution.observation.is_none());
                        assert!(matches!(&execution.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == bound));
                        assert_eq!(execution.step_usage.unwrap().consumed, bound);
                        receipt["disposition"] = json!("INCOMPLETE: Steps");
                        return Ok(());
                    }
                    let observed = execution.observation.ok_or_else(|| format!("tooling/no observation: {:?}", execution.outcome))?;
                    receipt["completed"] = json!(true);
                    if observed.exit_code != row["exit"].as_i64().map(|n| n as i32) || observed.output != bytes(&row["stdout"]) || observed.final_state.as_ref().unwrap().stderr != bytes(&row["stderr"]) || !observed.failures.is_empty() || observed.blocked.is_some() {
                        return Err(format!("outcome {:?}, stdout {:?}, stderr {:?}, failures {:?}", execution.outcome, observed.output, observed.final_state.as_ref().unwrap().stderr, observed.failures));
                    }
                    let state = observed.final_state.as_ref().unwrap();
                    let mut expected = world.files.clone();
                    if !row["expected"].is_null() { expected.insert("data/output".into(), bytes(&row["expected"])); }
                    if state.trees[&0].1 != expected { return Err("tree mismatch/temporary leaked".into()); }
                    if row["pending"] == true && !observed.effects.iter().any(|e| matches!(&e.effect, Effect::CapabilityConsumed(e) if e.operation == "file.read.pending")) { return Err("read did not pend".into()); }
                    if let Some(previous) = &previous { assert_eq!(&observed, previous, "{id}: complete reference/checked observation"); }
                    previous = Some(observed);
                    Ok(())
                });
                match checked {
                    Ok(()) => {
                        receipt["passed"] = json!(receipt["completed"] == true);
                        eprintln!(
                            "PNM_TILES_30_{} {id} {engine} {}",
                            if receipt["completed"] == true {
                                "PASS"
                            } else {
                                "INCOMPLETE"
                            },
                            receipt
                        );
                    }
                    Err(error) => {
                        receipt["error"] = json!(error);
                        failures.push(format!("{id}/{engine}: {error}"));
                        eprintln!("PNM_TILES_30_FAIL {id} {engine}: {error}");
                    }
                }
                receipts.push(receipt);
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        assert!(!receipts.is_empty(), "zero-test selection");
        assert!(
            failures.is_empty(),
            "evidence {}: {}",
            root.display(),
            failures.join("\n")
        );
    });
}
#[test]
fn pnm_tiles_30_reference_checked_workflows() {
    simulated(false);
}
#[test]
fn pnm_tiles_30_reference_checked_caps() {
    simulated(true);
}

#[test]
fn pnm_tiles_30_native_record_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let root = if let Some(root) = std::env::var_os("PNM_TILES_30_REPLAY_ROOT") {
            PathBuf::from(root)
        } else {
            let root = scratch("native");
            package(&root);
            let output = Command::new("python3")
                .arg(root.join("verify.py"))
                .arg("--root")
                .arg(&root)
                .arg("--hirsel")
                .arg(env!("CARGO_BIN_EXE_hirsel"))
                .output()
                .unwrap();
            fs::write(root.join("stdout"), &output.stdout).unwrap();
            fs::write(root.join("stderr"), &output.stderr).unwrap();
            assert!(
                output.status.success(),
                "{}: {}\n{}",
                root.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            root
        };
        let manifest = root.join("hirsel.json");
        let compiler = hirsel_driver::COMPILER
            .clone()
            .with_config(hirsel_engine::CompileConfig {
                store: hirsel_engine::store::store_by_name("dir@1", Some(root.join("products")))
                    .unwrap(),
                ..hirsel_driver::COMPILER.configuration()
            });
        let record_path = std::env::var_os("PNM_TILES_30_RECORDS")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("tapes.json"));
        let records: Vec<Value> = serde_json::from_slice(&fs::read(record_path).unwrap()).unwrap();
        let mut receipts = Vec::new();
        assert!(!records.is_empty());
        // Each replay opens fresh tape/provider state. Only the immutable source
        // query graph is shared across tapes and reference/checked consumers.
        let config = compiler.configuration();
        let mut requests = hirsel_driver::requests::CompilerRequests::for_host(compiler, config);
        let output = requests
            .execute(
                &manifest,
                hirsel_driver::requests::CompileOperation::Tir { definition: None },
            )
            .unwrap();
        let hirsel_driver::requests::CompileResult::Tir(prepared) = &output.result else {
            panic!("expected prepared TIR")
        };
        for row in records {
            if !selected(&row) {
                continue;
            }
            let tape = root.join(row["tape"].as_str().unwrap());
            let frozen = fs::read(&tape).unwrap();
            let mut previous = None;
            for checked in [false, true] {
                let engine = if checked {
                    "checked-h-replay"
                } else {
                    "reference-replay"
                };
                let bound = row["bound"].as_u64().unwrap();
                eprintln!("PNM_TILES_30_STARTED {} {engine}", row["id"]);
                if row["reference_disposition"] == "Steps" {
                    // The observation helper currently discards the tooling outcome.
                    // The public ordinary host launcher retains its exact kind/bound;
                    // checked host classification stays explicitly incomplete.
                    if checked {
                        eprintln!(
                            "PNM_TILES_30_INCOMPLETE {} {engine}: unrun; public helper loses tooling kind/bound",
                            row["id"]
                        );
                    } else {
                        let outcome = hirsel_driver::run_path_on_host(
                            &manifest,
                            hirsel_driver::HostLaunch::Replay(&tape),
                            Some(bound),
                            hirsel_engine::Verify::On,
                        )
                        .unwrap();
                        assert!(
                            matches!(&outcome, hirsel_engine::RunOutcome::ToolingLimit(limit) if limit.kind == "Steps" && limit.limit == bound),
                            "{outcome:?}"
                        );
                        eprintln!(
                            "PNM_TILES_30_INCOMPLETE {} {engine}: {outcome:?}",
                            row["id"]
                        );
                    }
                    receipts.push(json!({"id":row["id"],"engine":engine,"completed":false,"passed":false,"budget":bound,"disposition": if checked { "INCOMPLETE unrun: public helper loses tooling outcome" } else { "INCOMPLETE exact Steps" }}));
                    let receipt_path = std::env::var_os("PNM_TILES_30_RECEIPTS")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| root.join("host-replay-receipts.json"));
                    fs::write(receipt_path, serde_json::to_vec_pretty(&receipts).unwrap()).unwrap();
                    continue;
                }
                let replayed = hirsel_driver::replay_prepared_observations(
                    prepared,
                    &tape,
                    Some(bound),
                    checked,
                );
                let observed = replayed.unwrap();
                assert_eq!(observed.exit_code, row["exit"].as_i64().map(|n| n as i32));
                assert_eq!(observed.output, bytes(&row["stdout"]));
                assert!(observed.failures.is_empty());
                assert!(
                    observed.final_state.is_none(),
                    "host replay does not provide a simulated tree"
                );
                let recording = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                let stderr: Vec<u8> = recording
                    .events
                    .iter()
                    .filter_map(|event| match &event.request {
                        hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes.as_slice()),
                        _ => None,
                    })
                    .flatten()
                    .copied()
                    .collect();
                assert_eq!(stderr, bytes(&row["stderr"]));
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                if let Some(previous) = &previous {
                    assert!(
                        observed.equivalent(previous),
                        "full checked host replay agreement"
                    );
                }
                previous = Some(observed);
                receipts.push(json!({"id":row["id"],"engine":engine,"completed":true,"passed":true,"budget":bound}));
                let receipt_path = std::env::var_os("PNM_TILES_30_RECEIPTS")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root.join("host-replay-receipts.json"));
                fs::write(receipt_path, serde_json::to_vec_pretty(&receipts).unwrap()).unwrap();
                eprintln!("PNM_TILES_30_PASS {} {engine}", row["id"]);
            }
        }
    });
}
// These controls reuse the first successful tape; no live provider is needed.
fn refused(manifest: &Path, root: &Path, tape: &Path, kind: &str) {
    for checked in [false, true] {
        let error = if checked {
            hirsel_driver::replay_observations_checked(manifest, tape, Some(1_000_000))
        } else {
            hirsel_driver::replay_observations(manifest, tape, Some(1_000_000))
        }
        .unwrap_err();
        assert!(error.to_string().contains(kind), "{kind}: {error}");
    }
    for backend in ["llvm", "cranelift"] {
        let output = Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(root.join("native").join(backend))
            .arg("--hirsel-replay")
            .arg(tape)
            .env("HIRSEL_JOBS", "4")
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(kind),
            "{output:?}"
        );
    }
}

#[test]
fn pnm_tiles_30_replay_refusals() {
    hirsel_engine::with_compiler_stack(|| {
        let root = if let Some(path) = std::env::var_os("PNM_TILES_30_REPLAY_ROOT") {
            PathBuf::from(path)
        } else {
            let root = scratch("refusals");
            package(&root);
            let output = Command::new("python3")
                .arg(root.join("verify.py"))
                .args(["--root"])
                .arg(&root)
                .arg("--hirsel")
                .arg(env!("CARGO_BIN_EXE_hirsel"))
                .env("PNM_TILES_30_CASE", "1x1-c1-info")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            root
        };
        let manifest = root.join("hirsel.json");
        let records: Vec<Value> =
            serde_json::from_slice(&fs::read(root.join("tapes.json")).unwrap()).unwrap();
        let original = root.join(records[0]["tape"].as_str().unwrap());
        let bytes = fs::read(&original).unwrap();
        let recording = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
        let bad = root.join("changed-input.tape");
        let mut changed = recording.clone();
        let request = changed
            .events
            .iter_mut()
            .find_map(|e| match &mut e.request {
                hirsel_rt::io::Request::Directory { path, .. } => Some(path),
                _ => None,
            })
            .unwrap();
        request.push_str("-changed");
        fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
        refused(&manifest, &root, &bad, "ReplayMismatch");
        let bad = root.join("extra-event.tape");
        let mut changed = recording.clone();
        changed.events.push(recording.events[0].clone());
        fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
        refused(&manifest, &root, &bad, "UnconsumedInput");
        let bad = root.join("changed-config.tape");
        let mut changed = recording.clone();
        changed.hash_seed ^= 1;
        fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
        refused(&manifest, &root, &bad, "header disagrees");
        assert_eq!(fs::read(original).unwrap(), bytes);
        eprintln!("PNM_TILES_30_PASS refusal controls: 3 identities x 4 replay engines");
    });
}
