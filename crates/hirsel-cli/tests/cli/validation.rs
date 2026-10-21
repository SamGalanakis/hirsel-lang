//! The production default and the explicit harness mode lower identical native
//! code, with four fewer LIR validations per unit in the production mode.
use super::file;
use std::process::Command;

#[test]
fn validate_stages_preserves_native_bytes_and_restores_four_checks_per_unit() {
    let source = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let output = source.with_extension("ready.exe");
    let mut results = Vec::new();
    for stages in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        cmd.env_remove("HIRSEL_VALIDATE_STAGES").args([
            "--timings=json",
            "build",
            "--debug",
            "--backend",
            "cranelift",
        ]);
        if stages {
            cmd.arg("--validate-stages");
        }
        let built = cmd.arg(&source).arg("-o").arg(&output).output().unwrap();
        assert!(built.status.success(), "{stages}: {built:?}");
        let report: serde_json::Value = serde_json::from_str(
            std::str::from_utf8(&built.stderr)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        let calls = report["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["phase"] == "lir validate")
            .unwrap()["calls"]
            .as_u64()
            .unwrap();
        results.push((
            std::fs::read(&output).unwrap(),
            calls,
            report["counts"]["units"].as_u64().unwrap(),
        ));
    }
    assert_eq!(
        results[0].0, results[1].0,
        "validation policy changed the native executable"
    );
    assert!(results[0].2 > 0);
    assert_eq!(results[0].2, results[1].2);
    assert_eq!(results[1].1 - results[0].1, 4 * results[0].2);
    let invalid = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--validate-stages"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("--validate-stages requires --native")
    );
}
