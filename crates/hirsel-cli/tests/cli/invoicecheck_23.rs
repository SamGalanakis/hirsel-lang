//! Exact decimal invoices: independent oracle, actual CLI engines and checked H.
use std::{fs, path::PathBuf, process::Command};

const MAIN: &str = include_str!("../../../../examples/invoicecheck/main.hir");
const MODEL: &str = include_str!("../../../../examples/invoicecheck/model.hir");
const MANIFEST: &str = include_str!("../../../../examples/invoicecheck/hirsel.json");
const SUCCESS: &str = include_str!("../../../../examples/invoicecheck/fixtures/success.csv");
const INVALID: &str = include_str!("../../../../examples/invoicecheck/fixtures/invalid.csv");

fn package(root: &std::path::Path, main: &str) -> PathBuf {
    fs::write(root.join("main.hir"), main).unwrap();
    fs::write(root.join("model.hir"), MODEL).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
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
fn invoicecheck_23_oracle_cli_engines_and_effects() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    package(root, MAIN);
    fs::create_dir(root.join("fixtures")).unwrap();
    fs::write(root.join("fixtures/success.csv"), SUCCESS).unwrap();
    fs::write(root.join("fixtures/invalid.csv"), INVALID).unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let script = root.join("verify.py");
    fs::write(
        &script,
        include_str!("../support/invoicecheck_23/verify.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(&script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(root)
        .arg("--output")
        .arg(root.join("evidence"))
        .output()
        .unwrap();
    if !output.status.success() {
        let retained = scratch.keep();
        panic!(
            "evidence={}\nstdout={}\nstderr={}",
            retained.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    eprintln!("invoicecheck_23 acceptance: {result}");
    let cases = result["cases"].as_u64().unwrap();
    assert!(cases >= 35, "{result}");
    assert_eq!(result["observations"], cases * 3 + 3, "{result}");
    assert!(!root.join("fixtures").exists(), "input cleanup");
    let path = root.to_path_buf();
    scratch.close().unwrap();
    assert!(!path.exists(), "private process/build/log cleanup");
}

#[test]
fn invoicecheck_23_pure_model_checked_h() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let source = r#"
pub fn main() -> Result[(), scalar.ParseError] {
  let vectors: List[(String, i32, String)] = [("6.5", 0, "6"), ("7.5", 0, "8"), ("1.2250", 2, "1.22")]
  for (input, places, expected) in vectors {
    failure.check(decimal.to_string(decimal.rescale(decimal.parse(input)?, places, HalfEven())) == expected, Arithmetic())
  }
  failure.check(decimal.compare(decimal.add(decimal.parse("0.1")?, decimal.parse("0.2")?), decimal.parse("0.3")?) == 0, Arithmetic())
  failure.check(decimal.compare(decimal.parse("1.0")?, decimal.parse("1.00")?) == 0, Arithmetic())
  let Ok(report) = model.reconcile("invoice_id,quantity,unit_price,expected_total\na,1,1.2250,1.22\na,1,-0.12,-0.1200\na,0,99.99,0\na,1,1.2250,1.22\n") else { failure.check(false, Arithmetic()); return Ok(()) }
  failure.check(report.records == 4 && report.invoices[0].total == "2.32" && report.invoices[0].matches && list.len(report.mismatches) == 0, Arithmetic())
  let Error(error) = model.reconcile("invoice_id,quantity,unit_price,expected_total\na,1,1.0000000,1\n") else { failure.check(false, Arithmetic()); return Ok(()) }
  failure.check(error.record == 1 && model.reason(error.reason) == "decimal_scale", Arithmetic())
  Ok(())
}
"#;
        let selection = package(scratch.path(), source);
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
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
        let observation =
            hirsel_driver::native::Observation::of_reference(checked.observation.as_ref().unwrap());
        assert_eq!(observation.exit_code, Some(0), "{observation:?}");
        assert!(observation.failures.is_empty(), "{observation:?}");
        assert!(observation.output.is_empty(), "{observation:?}");
        let path = scratch.path().to_path_buf();
        scratch.close().unwrap();
        assert!(!path.exists());
    });
}
