//! Complete firstreply orchestration through public engine/launcher adapters.
use std::{fs, path::PathBuf, process::Command};

fn package(root: &std::path::Path) -> PathBuf {
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let rpc = root.join("localrpc");
    fs::create_dir_all(&rpc).unwrap();
    fs::create_dir_all(root.join("fixtures")).unwrap();
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/firstreply/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("fixtures/workflows.json"),
        include_str!("../../../../examples/firstreply/fixtures/workflows.json"),
    )
    .unwrap();
    fs::write(
        root.join("fixtures/controls.json"),
        include_str!("../../../../examples/firstreply/fixtures/controls.json"),
    )
    .unwrap();
    fs::write(
        rpc.join("main.hir"),
        include_str!("../../../../examples/localrpc/main.hir"),
    )
    .unwrap();
    fs::write(
        rpc.join("model.hir"),
        include_str!("../../../../examples/localrpc/model.hir"),
    )
    .unwrap();
    let mut dependency: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/localrpc/hirsel.json")).unwrap();
    dependency["dependencies"][0]["manifest"] = std.to_string_lossy().as_ref().into();
    fs::write(rpc.join("hirsel.json"), dependency.to_string()).unwrap();
    let mut app: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/firstreply/hirsel.json")).unwrap();
    app["dependencies"][0]["manifest"] = std.to_string_lossy().as_ref().into();
    app["dependencies"][1]["manifest"] = rpc.join("hirsel.json").to_string_lossy().as_ref().into();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, app.to_string()).unwrap();
    manifest
}

#[test]
fn firstreply_27_scripted_reference_checked_h() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let rows: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../examples/firstreply/fixtures/workflows.json"
        ))
        .unwrap();
        let mut program = hirsel_driver::load_program(&package(scratch.path())).unwrap();
        program.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut errors = Vec::new();
        for row in rows.as_array().unwrap() {
            let id = row["id"].as_str().unwrap();
            let grants = &row["grants"];
            let mut world = hirsel_engine::SimulationConfig {
                args: serde_json::from_value(grants["args"].clone()).unwrap(),
                net: Some(serde_json::from_value(grants["io"]["net"].clone()).unwrap()),
                ..Default::default()
            };
            world.projections.insert("monotonic_clock".into());
            world.grants.insert("MonotonicClock".into());
            let mut reference = None;
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "interpreter" };
                eprintln!("FIRSTREPLY_27 STARTED {id} {engine}");
                let result = if checked {
                    hirsel_engine::run_observed_checked(&mut program.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_snapshot(
                        &mut program.engine,
                        &config,
                        world.clone(),
                    )
                };
                let execution = match result {
                    Ok(value) => value,
                    Err(error) => {
                        errors.push(format!("{id} {engine}: {error}"));
                        continue;
                    }
                };
                let Some(observed) = execution.observation else {
                    errors.push(format!("{id} {engine}: {:?}", execution.outcome));
                    continue;
                };
                let good = observed.exit_code
                    == row["expected"]["status"].as_i64().map(|v| v as i32)
                    && observed.output == row["expected"]["stdout"].as_str().unwrap().as_bytes()
                    && observed.final_state.as_ref().unwrap().stderr.is_empty()
                    && observed.failures.is_empty()
                    && observed.blocked.is_none()
                    && observed.signaled.is_none();
                if !good {
                    errors.push(format!(
                        "{id} {engine}: exit={:?} stdout={:?} failures={:?} blocked={:?}",
                        observed.exit_code,
                        String::from_utf8_lossy(&observed.output),
                        observed.failures,
                        observed.blocked
                    ));
                }
                // Preserve complete effect order, including cleanup. Checked H
                // must agree with the reference, not merely render the same row.
                if let Some(expected) = &reference {
                    if expected != &observed {
                        errors.push(format!("{id}: checked H semantic observations differ"));
                    }
                } else {
                    reference = Some(observed);
                }
                eprintln!("FIRSTREPLY_27 COMPLETED {id} {engine} pass={good}");
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    });
}

#[test]
fn firstreply_27_native_scripted_and_real_peers() {
    let scratch = tempfile::Builder::new().prefix("fr27-").tempdir().unwrap();
    let manifest = package(scratch.path());
    let script = scratch.path().join("verify.py");
    fs::write(&script, include_str!("../support/firstreply_27/verify.py")).unwrap();
    let output = Command::new("python3")
        .arg(script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--manifest")
        .arg(manifest)
        .arg("--output")
        .arg(scratch.path().join("evidence"))
        .output()
        .unwrap();
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    if !output.status.success() {
        panic!(
            "evidence: {}\n{}",
            scratch.keep().display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scripted_completed"], 20);
    assert_eq!(report["real_completed"], 4);
    assert_eq!(report["failures"], serde_json::json!([]));
}

#[test]
fn firstreply_27_caps_reference_checked_and_native() {
    let scratch = tempfile::Builder::new()
        .prefix("fr27-cap-")
        .tempdir()
        .unwrap();
    let manifest = package(scratch.path());
    hirsel_engine::with_compiler_stack(|| {
        let rows: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../examples/firstreply/fixtures/controls.json"
        ))
        .unwrap();
        let mut program = hirsel_driver::load_program(&manifest).unwrap();
        program.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for row in rows.as_array().unwrap() {
            let mut world = hirsel_engine::SimulationConfig {
                args: serde_json::from_value(row["grants"]["args"].clone()).unwrap(),
                net: Some(serde_json::from_value(row["grants"]["io"]["net"].clone()).unwrap()),
                ..Default::default()
            };
            world.grants.insert("MonotonicClock".into());
            world.projections.insert("monotonic_clock".into());
            for checked in [false, true] {
                let execution = if checked {
                    hirsel_engine::run_observed_checked(&mut program.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_snapshot(
                        &mut program.engine,
                        &config,
                        world.clone(),
                    )
                }
                .unwrap();
                let observed = execution.observation.unwrap();
                assert_eq!(
                    observed.exit_code,
                    row["expected"]["status"].as_i64().map(|v| v as i32),
                    "{} checked={checked}",
                    row["id"]
                );
                assert_eq!(
                    observed.output,
                    row["expected"]["stdout"].as_str().unwrap().as_bytes(),
                    "{} checked={checked}",
                    row["id"]
                );
                assert!(observed.failures.is_empty() && observed.blocked.is_none());
                eprintln!(
                    "FIRSTREPLY_27 CAP_COMPLETED {} checked={checked}",
                    row["id"]
                );
            }
        }
    });
    let script = scratch.path().join("verify.py");
    fs::write(&script, include_str!("../support/firstreply_27/verify.py")).unwrap();
    let output = Command::new("python3")
        .arg(script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--manifest")
        .arg(manifest)
        .arg("--output")
        .arg(scratch.path().join("evidence"))
        .arg("--controls-only")
        .output()
        .unwrap();
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scripted_completed"], 16);
    assert_eq!(report["failures"], serde_json::json!([]));
}

#[test]
fn firstreply_27_cleanup_finishes_before_output() {
    use hirsel_interp::{library::simulation::ObservedGrant, observation::Effect};
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let selection = package(scratch.path());
        let rows: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../examples/firstreply/fixtures/workflows.json"
        ))
        .unwrap();
        let row = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "loser-blocked-in-read")
            .unwrap();
        let mut program = hirsel_driver::load_program(&selection).unwrap();
        program.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut world = hirsel_engine::SimulationConfig {
            args: serde_json::from_value(row["grants"]["args"].clone()).unwrap(),
            net: Some(serde_json::from_value(row["grants"]["io"]["net"].clone()).unwrap()),
            ..Default::default()
        };
        world.grants.insert("MonotonicClock".into());
        world.projections.insert("monotonic_clock".into());
        let execution =
            hirsel_engine::run_observed_checked(&mut program.engine, &config, world).unwrap();
        let observed = execution.observation.unwrap();
        assert_eq!(
            observed.output,
            row["expected"]["stdout"].as_str().unwrap().as_bytes()
        );
        assert_eq!(observed.exit_code, Some(0));
        let output = observed
            .effects
            .iter()
            .position(|effect| {
                matches!(&effect.effect,
            Effect::CapabilityConsumed(event) if event.operation == "out.write")
            })
            .unwrap();
        let mut closed = 0;
        let mut joins = 0;
        for (index, effect) in observed.effects.iter().enumerate() {
            match &effect.effect {
                Effect::HandleDropped { kind, .. } if kind == "Stream" => {
                    assert!(index < output, "stream cleanup after result output");
                    closed += 1;
                }
                Effect::CapabilityConsumed(event) if event.operation == "task.await.complete" => {
                    assert!(index < output);
                    joins += 1;
                }
                Effect::CapabilityConsumed(event) if event.operation.starts_with("net.") => {
                    assert!(index < output, "client continuation after return");
                }
                _ => {}
            }
        }
        assert_eq!(closed, 2, "both streams close, including the blocked loser");
        assert_eq!(
            joins, 1,
            "select awaits the winner; cancel_all drains the loser"
        );
        assert!(
            !observed
                .final_state
                .unwrap()
                .handles
                .values()
                .any(|grant| matches!(grant, ObservedGrant::Stream { .. }))
        );
    });
}
