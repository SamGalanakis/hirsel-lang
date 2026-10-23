//! Bounded framing vectors, checked H, and independent Unix peers on three engines.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn package(root: &Path) -> PathBuf {
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/localrpc/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("model.hir"),
        include_str!("../../../../examples/localrpc/model.hir"),
    )
    .unwrap();
    fs::write(
        root.join("normalize.json"),
        include_str!("../../../../examples/localrpc/fixtures/normalize.json"),
    )
    .unwrap();
    fs::write(
        root.join("invalid.json"),
        include_str!("../../../../examples/localrpc/fixtures/invalid.json"),
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/localrpc/hirsel.json")).unwrap();
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

#[test]
fn localrpc_23_checked_model_vectors() {
    let scratch = tempfile::tempdir().unwrap();
    let manifest = package(scratch.path());
    fs::write(
        scratch.path().join("main.hir"),
        include_str!("../../../../examples/localrpc/fixtures/vectors.hir"),
    )
    .unwrap();
    hirsel_engine::with_compiler_stack(|| {
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution = hirsel_engine::run_observed_checked(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        let observed = hirsel_driver::native::Observation::of_reference(
            execution.observation.as_ref().unwrap(),
        );
        assert_eq!(observed.exit_code, Some(0), "{observed:?}");
        assert!(
            observed.failures.is_empty() && observed.output.is_empty(),
            "{observed:?}"
        );
    });
}

#[test]
fn localrpc_23_unix_peers_all_engines() {
    let scratch = tempfile::Builder::new().prefix("lr23-").tempdir().unwrap();
    package(scratch.path());
    fs::write(
        scratch.path().join("vectors.hir"),
        include_str!("../../../../examples/localrpc/fixtures/vectors.hir"),
    )
    .unwrap();
    let script = scratch.path().join("verify.py");
    fs::write(&script, include_str!("../support/localrpc_23/verify.py")).unwrap();
    let output = Command::new("python3")
        .arg(script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(scratch.path())
        .arg("--output")
        .arg(scratch.path().join("evidence"))
        .output()
        .unwrap();
    if !output.status.success() {
        let retained = scratch.keep();
        panic!(
            "evidence: {}\nstdout={}\nstderr={}",
            retained.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["engines"],
        serde_json::json!(["h", "llvm", "cranelift"])
    );
    assert!(report["executions"].as_u64().unwrap() >= 150, "{report}");
    eprintln!("localrpc_23 execution report: {report}");
}
