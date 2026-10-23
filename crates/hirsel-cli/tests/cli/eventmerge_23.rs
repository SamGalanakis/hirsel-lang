//! Canonical UUID vectors, checked heap model, and three-engine event exports.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/eventmerge/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/eventmerge/model.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, source) in [
        (
            "job-a.ndjson",
            include_str!("../../../../examples/eventmerge/fixtures/job-a.ndjson"),
        ),
        (
            "job-b.ndjson",
            include_str!("../../../../examples/eventmerge/fixtures/job-b.ndjson"),
        ),
        (
            "conflict.ndjson",
            include_str!("../../../../examples/eventmerge/fixtures/conflict.ndjson"),
        ),
        (
            "uuid-vectors.json",
            include_str!("../../../../examples/eventmerge/fixtures/uuid-vectors.json"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), source).unwrap();
    }
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/eventmerge/hirsel.json")).unwrap();
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

#[test]
fn eventmerge_23_independent_vectors_and_three_engines() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let _ = package(root);
    let script = root.join("verify.py");
    fs::write(&script, include_str!("../support/eventmerge_23/verify.py")).unwrap();
    let output = Command::new("python3")
        .arg(script)
        .args(["--hirsel", env!("CARGO_BIN_EXE_hirsel"), "--root"])
        .arg(root)
        .arg("--output")
        .arg(root.join("evidence"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    eprintln!("eventmerge matrix: {result}");
    assert_eq!(result["cases"], 43);
    assert_eq!(
        result["observations"].as_u64().unwrap(),
        result["cases"].as_u64().unwrap() * 3
    );
    assert_eq!(result["vectors"], 31);
    assert_eq!(result["cleanup"], true);
    scratch.close().unwrap();
}

#[test]
fn eventmerge_23_pure_model_and_uuid_bytes_checked_heap() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let manifest = package(scratch.path());
        let mut source = String::from(
            r#"
pub fn main() -> Result[(), ()] uses {} {
  let Ok(report) = model.merge(b"{\"id\":\"f47ac10b-58cc-4372-8567-0e02b2c3d479\",\"payload\":{\"a\":1,\"b\":null}}\n", "a", b"{\"id\":\"F47AC10B-58CC-4372-8567-0E02B2C3D479\",\"payload\":{\"b\":null,\"a\":1}}\n", "b") else {
    failure.check(false, Arithmetic())
    return Error(())
  }
  failure.check(report.records == 2 && report.duplicates == 1 && list.len(report.events) == 1, Arithmetic())
  let Error(error) = model.merge(b"{\"id\":\"00000000-0000-0000-0000-000000000001\",\"payload\":1}\n", "left", b"{\"id\":\"00000000-0000-0000-0000-000000000001\",\"payload\":1.0}\n", "right") else {
    failure.check(false, Arithmetic())
    return Error(())
  }
  failure.check(error.first.source == "left" && error.first.line == 1 && error.location.source == "right" && error.location.line == 1, Arithmetic())
"#,
        );
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../examples/eventmerge/fixtures/uuid-vectors.json"
        ))
        .unwrap();
        for (i, vector) in vectors.as_array().unwrap().iter().enumerate() {
            let spelling = serde_json::to_string(vector["input"].as_str().unwrap()).unwrap();
            let canonical = serde_json::to_string(vector["canonical"].as_str().unwrap()).unwrap();
            let bytes = vector["bytes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|byte| format!("\\x{:02x}", byte.as_u64().unwrap()))
                .collect::<String>();
            source.push_str(&format!(r#"
  let Ok(parsed_{i}) = uuid.parse({spelling}) else {{ failure.check(false, Arithmetic()); return Error(()) }}
  failure.check(uuid.to_string(parsed_{i}) == {canonical}, Arithmetic())
  failure.check(uuid.to_bytes(parsed_{i}) == b"{bytes}", Arithmetic())
  failure.check(uuid.from_bytes(b"{bytes}") == Some(parsed_{i}), Arithmetic())
"#));
        }
        source.push_str("  Ok(())\n}\n");
        fs::write(scratch.path().join("main.hir"), source).unwrap();
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let checked = hirsel_engine::run_observed_checked(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                checked.outcome,
                Ok(hirsel_engine::RunOutcome::Returned(
                    hirsel_run_schema::Returned::Ok
                ))
            ),
            "{:?}",
            checked.outcome
        );
        scratch.close().unwrap();
    });
}
