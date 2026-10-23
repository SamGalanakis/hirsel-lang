//! Complete application execution, with an independent input-order byte oracle.
use hirsel_engine::{RunConfiguration, SimulationConfig};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Command};

fn scratch() -> tempfile::TempDir {
    let base = std::env::var_os("SHARDSEARCH_27_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    tempfile::Builder::new()
        .prefix("shardsearch-27-")
        .tempdir_in(base)
        .unwrap()
}

fn package(root: &std::path::Path) -> PathBuf {
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/shardsearch/main.hir"),
    )
    .unwrap();
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/shardsearch/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let selection = root.join("hirsel.json");
    fs::write(&selection, manifest.to_string()).unwrap();
    selection
}

#[test]
fn shardsearch_27_reference_and_checked_application() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch();
        let selection = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../support/shardsearch_27/cases.json")).unwrap();
        let mut completed = 0;
        for case in cases {
            for jobs in [1, 2, 4] {
                let mut world = SimulationConfig {
                    args: vec![
                        "fixtures".into(),
                        case["query"].as_str().unwrap().into(),
                        jobs.to_string(),
                        case["limit"].to_string(),
                    ],
                    mounts: [("fixtures".into(), hirsel_run_schema::Access::Read)].into(),
                    ..Default::default()
                };
                world.args.extend(
                    case["paths"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_owned()),
                );
                for (name, data) in case["files"].as_object().unwrap() {
                    let bytes = if let Some(s) = data.as_str() {
                        s.as_bytes().to_vec()
                    } else {
                        data.as_array()
                            .unwrap()
                            .iter()
                            .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
                            .collect()
                    };
                    world.files.insert(format!("fixtures/{name}"), bytes);
                }
                let mut prior = None;
                for engine in ["reference", "checked-h"] {
                    eprintln!("SHARDSEARCH_27_STARTED {} {engine} jobs={jobs}", case["id"]);
                    let execution = if engine == "reference" {
                        hirsel_engine::run_observed_snapshot(
                            &mut loaded.engine,
                            &config,
                            world.clone(),
                        )
                    } else {
                        hirsel_engine::run_observed_checked(
                            &mut loaded.engine,
                            &config,
                            world.clone(),
                        )
                    }
                    .unwrap();
                    assert!(
                        matches!(
                            execution.outcome,
                            Ok(hirsel_engine::RunOutcome::Returned(_))
                        ),
                        "{} {engine}: {:?}",
                        case["id"],
                        execution.outcome
                    );
                    let observed = execution.observation.unwrap();
                    assert_eq!(
                        observed.exit_code,
                        Some(case["status"].as_i64().unwrap() as i32),
                        "{} {engine}",
                        case["id"]
                    );
                    assert_eq!(
                        observed.output,
                        case["stdout"].as_str().unwrap().as_bytes(),
                        "{} {engine}",
                        case["id"]
                    );
                    let state = observed.final_state.as_ref().unwrap();
                    assert_eq!(
                        state.stderr,
                        case["stderr"].as_str().unwrap().as_bytes(),
                        "{} {engine}",
                        case["id"]
                    );
                    assert_eq!(state.trees[&0].1, world.files);
                    assert!(
                        observed.failures.is_empty()
                            && observed.blocked.is_none()
                            && observed.signaled.is_none()
                    );
                    // Full semantic observations retain task IDs, payloads and order.
                    if let Some(prior) = &prior {
                        assert_eq!(&observed, prior, "{} jobs={jobs}", case["id"]);
                    }
                    eprintln!(
                        "SHARDSEARCH_27_PASS {}",
                        json!({"id":case["id"],"engine":engine,"jobs":jobs,"completed":1})
                    );
                    prior = Some(observed);
                    completed += 1;
                }
            }
        }
        assert_eq!(completed, 324);
    });
}

fn native(backend: &str) {
    native_cases(
        backend,
        include_str!("../support/shardsearch_27/cases.json"),
    );
}

fn native_cases(backend: &str, cases: &str) {
    let scratch = scratch();
    let root = scratch.path();
    package(root);
    fs::write(root.join("cases.json"), cases).unwrap();
    fs::write(
        root.join("verify.py"),
        include_str!("../support/shardsearch_27/verify.py"),
    )
    .unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let result = Command::new("python3")
        .arg(root.join("verify.py"))
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(root)
        .arg(backend)
        .output()
        .unwrap();
    eprintln!("{}", String::from_utf8_lossy(&result.stdout));
    if !result.status.success() {
        let evidence = scratch.keep();
        panic!(
            "{}: {}",
            evidence.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    // Typed public tape readback: every opened file is closed before result
    // output, and no file continuation can publish an effect after it.
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "tape") {
            continue;
        }
        let recording = hirsel_rt::host_launch::read_recording(&path).unwrap();
        let mut files = std::collections::BTreeSet::new();
        let mut printing = false;
        for event in recording.events {
            use hirsel_rt::io::{Completion, Request};
            match event.request {
                Request::Open { .. } => {
                    assert!(!printing, "late file open: {}", path.display());
                    if let Completion::Handle(Ok(id)) = event.completion {
                        files.insert(id);
                    }
                }
                Request::Close { handle } => {
                    files.remove(&handle);
                }
                Request::FileRead { .. } => {
                    assert!(!printing, "late file read: {}", path.display());
                }
                Request::Print { .. } => {
                    assert!(
                        files.is_empty(),
                        "unjoined file handles: {}",
                        path.display()
                    );
                    printing = true;
                }
                _ => (),
            }
        }
        assert!(files.is_empty(), "leaked file handles: {}", path.display());
    }
}

#[test]
fn shardsearch_27_native_bounds() {
    for backend in ["llvm", "cranelift"] {
        native_cases(backend, include_str!("../support/shardsearch_27/caps.json"));
    }
}

#[test]
fn shardsearch_27_joined_cutoff() {
    for backend in ["llvm", "cranelift"] {
        native_cases(
            backend,
            include_str!("../support/shardsearch_27/joined.json"),
        );
    }
}

#[test]
fn shardsearch_27_llvm_application() {
    native("llvm");
}

#[test]
fn shardsearch_27_cranelift_application() {
    native("cranelift");
}
