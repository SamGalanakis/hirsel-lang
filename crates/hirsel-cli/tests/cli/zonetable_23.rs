//! Explicit TZif scheduling with pinned Jiff answers and bounded effects.
use std::{fs, path::PathBuf, process::Command};

fn package(root: &std::path::Path) -> PathBuf {
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/zonetable/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/zonetable/model.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, source) in [
        (
            "success.json",
            include_str!("../../../../examples/zonetable/fixtures/success.json"),
        ),
        (
            "invalid.json",
            include_str!("../../../../examples/zonetable/fixtures/invalid.json"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), source).unwrap();
    }
    for (name, source) in [
        (
            "new-york.tzif",
            include_bytes!("../../../../examples/zonetable/fixtures/new-york.tzif").as_slice(),
        ),
        (
            "utc.tzif",
            include_bytes!("../../../../examples/zonetable/fixtures/utc.tzif").as_slice(),
        ),
        (
            "plus-five.tzif",
            include_bytes!("../../../../examples/zonetable/fixtures/plus-five.tzif").as_slice(),
        ),
        (
            "malformed.tzif",
            include_bytes!("../../../../examples/zonetable/fixtures/malformed.tzif").as_slice(),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), source).unwrap();
    }
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/zonetable/hirsel.json")).unwrap();
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
fn zonetable_23_upstream_vectors_and_application_three_engines() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let _ = package(root);
    let script = root.join("verify.py");
    fs::write(&script, include_str!("../support/zonetable_23/verify.py")).unwrap();
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
    eprintln!("zonetable matrix: {result}");
    assert!(result["cases"].as_u64().unwrap() >= 40);
    assert_eq!(
        result["observations"].as_u64().unwrap(),
        result["cases"].as_u64().unwrap() * 3
    );
    assert_eq!(result["vectors"], 12);
    assert_eq!(result["cleanup"], true);
    scratch.close().unwrap();
}

#[test]
fn zonetable_23_pure_model_checked_heap() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let manifest = package(scratch.path());
        let source = r#"
const TZIF: Bytes = b"TZ_BYTES"
pub fn main() -> Result[(), ()] uses {} {
  let Ok(rows) = model.parse(b"[{\"local\":\"2024-03-10T02:30:00\",\"source\":\"a\",\"destination\":\"b\",\"policy\":\"Earlier\"}]") else {
    failure.check(false, Arithmetic())
    return Error(())
  }
  let Ok(converted) = model.convert(rows[0], TZIF, TZIF, model.DEFAULT_FORMAT) else {
    failure.check(false, Arithmetic())
    return Error(())
  }
  failure.check(converted.instant == "2024-03-10T06:30:00Z", Arithmetic())
  failure.check(converted.source_time == "2024-03-10T01:30:00", Arithmetic())
  failure.check(converted.destination_time == "2024-03-10T01:30:00.0-0500", Arithmetic())
  failure.check(converted.resolution == "gap", Arithmetic())
  let rejected = model.Row(source_input: "2024-11-03T01:30:00", source: "a", destination: "b", policy: Reject())
  match model.convert(rejected, TZIF, TZIF, model.DEFAULT_FORMAT) {
    Error(Resolve(Fold())) => {},
    _ => failure.check(false, Arithmetic()),
  }
  match model.validate_format("abc%Q") {
    Error(OutputFormat(Invalid(3))) => {},
    _ => failure.check(false, Arithmetic()),
  }
  failure.check(!model.valid_path("../zone"), Arithmetic())
  Ok(())
}
"#;
        let bytes = include_bytes!("../../../../examples/zonetable/fixtures/new-york.tzif");
        let literal: String = bytes.iter().map(|byte| format!("\\x{byte:02x}")).collect();
        fs::write(
            scratch.path().join("main.hir"),
            source.replace("TZ_BYTES", &literal),
        )
        .unwrap();
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
