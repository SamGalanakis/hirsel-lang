//! Offline Cargo requirement auditing with pinned upstream answers.
use std::{fs, path::PathBuf, process::Command};

fn package(root: &std::path::Path) -> PathBuf {
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/versionaudit/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/versionaudit/model.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, source) in [
        (
            "success.json",
            include_str!("../../../../examples/versionaudit/fixtures/success.json"),
        ),
        (
            "invalid.json",
            include_str!("../../../../examples/versionaudit/fixtures/invalid.json"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), source).unwrap();
    }
    let mut manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../examples/versionaudit/hirsel.json"
    ))
    .unwrap();
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
fn versionaudit_23_upstream_vectors_and_application_three_engines() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let _ = package(root);
    let script = root.join("verify.py");
    fs::write(
        &script,
        include_str!("../support/versionaudit_23/verify.py"),
    )
    .unwrap();
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
    eprintln!("versionaudit matrix: {result}");
    assert_eq!(result["cases"], 35);
    assert_eq!(result["observations"], 105);
    assert_eq!(result["vectors"], 44);
    assert_eq!(result["cleanup"], true);
    scratch.close().unwrap();
}

#[test]
fn versionaudit_23_pure_model_checked_heap() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let manifest = package(scratch.path());
        fs::write(scratch.path().join("main.hir"), r#"
pub fn main() -> Result[(), ()] uses {} {
  let Ok(report) = model.audit(b"[{\"name\":\"dep\",\"requirement\":\"^0.2.3\",\"version\":\"0.3.0\"}]") else {
    failure.check(false, Arithmetic())
    return Error(())
  }
  failure.check(report.checked == 1 && list.len(report.mismatches) == 1, Arithmetic())
  failure.check(model.encode(report) == b"{\"mismatches\":[{\"name\":\"dep\",\"requirement\":\"^0.2.3\",\"version\":\"0.3.0\"}],\"summary\":{\"class\":\"mismatch\",\"checked\":1,\"mismatched\":1}}\n", Arithmetic())
  let Error(error) = model.audit(b"[{\"name\":\"dep\",\"requirement\":\"||\",\"version\":\"1.0.0\"}]") else {
    failure.check(false, Arithmetic())
    return Error(())
  }
  failure.check(model.encode_error(error) == b"{\"class\":\"invalid_input\",\"error\":\"requirement\",\"row\":1,\"detail\":\"Syntax\",\"offset\":0}\n", Arithmetic())
  Ok(())
}
"#).unwrap();
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
