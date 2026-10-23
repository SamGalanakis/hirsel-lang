//! Bounded retry application, real children on three engines, and virtual-clock policy vectors.
use std::{fs, path::PathBuf, process::Command};

fn manifest() -> serde_json::Value {
    let mut package: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/retryrun/hirsel.json")).unwrap();
    package["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    package
}

fn workflows(shipped: bool) {
    let evidence = std::env::var_os("RETRYRUN_23_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let scratch = tempfile::Builder::new()
        .prefix("retryrun-23-")
        .tempdir_in(evidence)
        .unwrap();
    let root = scratch.path();
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/retryrun/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/retryrun/model.hir"),
        ),
        (
            "fixtures/child.py",
            include_str!("../../../../examples/retryrun/fixtures/child.py"),
        ),
        (
            "fixtures/success.json",
            include_str!("../../../../examples/retryrun/fixtures/success.json"),
        ),
        (
            "fixtures/invalid.json",
            include_str!("../../../../examples/retryrun/fixtures/invalid.json"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "grants.json",
            include_str!("../../../../examples/retryrun/grants.json"),
        ),
        (
            "verify.py",
            include_str!("../support/retryrun_23/verify.py"),
        ),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    fs::write(root.join("hirsel.json"), manifest().to_string()).unwrap();
    let mut command = Command::new("python3");
    if shipped {
        command.arg(root.join("verify.py")).arg("--shipped-only");
    } else {
        command.arg(root.join("verify.py"));
    }
    let output = command
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
            "evidence {}: stdout={}\nstderr={}",
            retained.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        if shipped {
            result["cases"] == 2
        } else {
            result["cases"].as_u64().unwrap() >= 50
        },
        "{result}"
    );
    assert_eq!(
        result["observations"],
        result["cases"].as_u64().unwrap() * 3
    );
    assert_eq!(result["cleanup"], true);
    eprintln!("retryrun_23 executed: {result}");
    let removed = root.to_owned();
    scratch.close().unwrap();
    assert!(!removed.exists());
}

#[test]
fn retryrun_23_oracle_engines_grants_and_child_cleanup() {
    workflows(false)
}

#[test]
fn retryrun_23_shipped_workflows_and_template() {
    workflows(true)
}

#[test]
fn retryrun_23_virtual_clock_vectors_checked_h() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        fs::write(
            root.join("main.hir"),
            include_str!("../../../../examples/retryrun/fixtures/policy.hir"),
        )
        .unwrap();
        fs::write(
            root.join("model.hir"),
            include_str!("../../../../examples/retryrun/model.hir"),
        )
        .unwrap();
        let selection = root.join("hirsel.json");
        fs::write(&selection, manifest().to_string()).unwrap();
        let mut loaded =
            hirsel_driver::load_program(&selection).unwrap_or_else(|error| panic!("{error}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut world = hirsel_engine::SimulationConfig::default();
        world.grants.insert("MonotonicClock".into());
        world.projections.insert("monotonic_clock".into());
        world.projections.insert("random".into());
        let plain =
            hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone()).unwrap();
        let checked = hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world).unwrap();
        for execution in [&plain, &checked] {
            assert!(
                matches!(
                    execution.outcome,
                    Ok(hirsel_engine::RunOutcome::Returned(
                        hirsel_run_schema::Returned::Ok
                    ))
                ),
                "{:?}",
                execution.outcome
            );
        }
        assert_eq!(plain.observation, checked.observation);
        eprintln!(
            "retryrun_23: 24 upstream delays, attempt/equality/operation-time controls, pure model admission and policy on H/checked-H"
        );
    });
}
