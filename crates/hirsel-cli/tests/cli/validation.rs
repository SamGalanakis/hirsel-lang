//! The production default and the explicit harness mode lower identical native
//! code, with four fewer LIR validations per unit in the production mode.
use super::file;
use std::process::Command;

#[test]
fn validate_stages_preserves_native_bytes_and_restores_four_checks_per_unit() {
    // task.async names task_main only through ClosureSite.code: the worker's
    // dependency graph must keep that template and bind it into the object key.
    let source = file(
        r#"
        pub fn main(rt: Io) -> Result[(), GrantError] {
          let tasks = io.tasks(rt)?
          task.scope(tasks, fn(s: Scope) {
            let child = task.async(s, fn() -> u64 { 1u64 })
            let answer = task.await(s, take child)
          })
          Ok(())
        }
    "#,
    );
    let output = source.with_extension("ready.exe");
    let mut results = Vec::new();
    for stages in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        cmd.env_remove("HIRSEL_VALIDATE_STAGES")
            .env_remove("HIRSEL_OBJECTS")
            .env("HIRSEL_JOBS", "1")
            .args([
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
        assert!(
            report["counts"]["object misses"].as_u64().unwrap() > 0,
            "worker path not selected: {report}"
        );
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
