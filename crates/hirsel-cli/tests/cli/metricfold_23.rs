//! Independent integer histogram oracle, real CLI engines and checked-H model.
use std::{fs, path::Path, path::PathBuf, process::Command};

fn package(root: &Path) -> PathBuf {
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/metricfold/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("model.hir"),
        include_str!("../../../../examples/metricfold/model.hir"),
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/metricfold/hirsel.json")).unwrap();
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
fn metricfold_23_integer_oracle_cli_engines_and_effects() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    package(root);
    fs::write(
        root.join("grants.json"),
        include_str!("../../../../examples/metricfold/grants.json"),
    )
    .unwrap();
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, data) in [
        (
            "jobs.ndjson",
            include_str!("../../../../examples/metricfold/fixtures/jobs.ndjson"),
        ),
        (
            "invalid.ndjson",
            include_str!("../../../../examples/metricfold/fixtures/invalid.ndjson"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), data).unwrap();
    }
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let script = root.join("verify.py");
    fs::write(&script, include_str!("../support/metricfold_23/verify.py")).unwrap();
    let output = Command::new("python3")
        .arg(script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
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
    assert_eq!(result["cases"], 44);
    assert_eq!(result["executions"], 132);
    assert_eq!(result["cleanup"], true);
    let owned = scratch.path().to_path_buf();
    scratch.close().unwrap();
    assert!(!owned.exists());
}

#[test]
fn metricfold_23_pure_model_interpreter_and_checked_h() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        let path = package(root);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        fs::write(
            root.join("control.hir"),
            include_str!("../support/metricfold_23/model.hir"),
        )
        .unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "path": "control.hir", "module": []
            }));
        manifest["entry"]["name"] = "model_contract".into();
        fs::write(&path, manifest.to_string()).unwrap();
        let mut loaded =
            hirsel_driver::load_program(&path).unwrap_or_else(|error| panic!("{error}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let plain = hirsel_engine::run_observed_snapshot(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        let checked = hirsel_engine::run_observed_checked(
            &mut loaded.engine,
            &config,
            hirsel_engine::SimulationConfig::default(),
        )
        .unwrap();
        for run in [&plain, &checked] {
            assert!(
                matches!(
                    run.outcome,
                    Ok(hirsel_engine::RunOutcome::Returned(
                        hirsel_run_schema::Returned::Ok
                    ))
                ),
                "{:?}",
                run.outcome
            );
        }
        assert_eq!(plain.observation, checked.observation);
    });
}
