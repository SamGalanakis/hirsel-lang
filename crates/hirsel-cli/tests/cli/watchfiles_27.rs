//! Complete application loop with deterministic read providers and real directory runs.
use std::{fs, path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn prepare(scripted: bool) -> tempfile::TempDir {
    let scratch = tempfile::tempdir().unwrap();
    for (name, text) in [
        (
            "main.hir",
            if scripted {
                include_str!("../support/watchfiles_27/scripted.hir")
            } else {
                include_str!("../../../../examples/watchfiles/main.hir")
            },
        ),
        (
            "watch.hir",
            include_str!("../../../../examples/watchfiles/watch.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/watchfiles/model.hir"),
        ),
    ] {
        fs::write(scratch.path().join(name), text).unwrap();
    }
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/watchfiles/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = root()
        .join("stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fs::write(scratch.path().join("hirsel.json"), manifest.to_string()).unwrap();
    scratch
}

#[test]
fn watchfiles_27_scripted_interpreter_and_checked_h() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = prepare(true);
        let script = scratch.path().join("oracle.py");
        fs::write(&script, include_str!("../support/watchfiles_27/oracle.py")).unwrap();
        let oracle = Command::new("python3").arg(script).output().unwrap();
        assert!(oracle.status.success(), "{oracle:?}");
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&oracle.stdout).unwrap();
        assert_eq!(cases.len(), 12);
        let mut loaded = hirsel_driver::load_program(&scratch.path().join("hirsel.json")).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for case in cases {
            let mut world = hirsel_engine::SimulationConfig::default();
            world.grants.insert("Test".into());
            world.test_seed = case["id"].as_u64().unwrap();
            let plain =
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone())
                    .unwrap();
            let checked =
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world).unwrap();
            for (engine, execution) in [("H", &plain), ("checked-H", &checked)] {
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(hirsel_engine::RunOutcome::Returned(
                            hirsel_run_schema::Returned::Ok
                        ))
                    ),
                    "{engine} {}: {:?}",
                    case["name"],
                    execution.outcome
                );
                let observed = execution.observation.as_ref().unwrap();
                assert_eq!(
                    observed.output,
                    case["stdout"].as_str().unwrap().as_bytes(),
                    "{engine} {}",
                    case["name"]
                );
                eprintln!("watchfiles_27 PASS {engine} {}", case["name"]);
            }
            assert_eq!(plain.observation, checked.observation, "{}", case["name"]);
        }
    });
}

fn native_workflows(real: bool) {
    let scratch = prepare(true);
    for (name, text) in [
        (
            "verify.py",
            include_str!("../support/watchfiles_27/verify.py"),
        ),
        (
            "oracle.py",
            include_str!("../support/watchfiles_27/oracle.py"),
        ),
        (
            "app-main.hir",
            include_str!("../../../../examples/watchfiles/main.hir"),
        ),
        (
            "handshake.hir",
            include_str!("../support/watchfiles_27/handshake.hir"),
        ),
    ] {
        fs::write(scratch.path().join(name), text).unwrap();
    }
    let output = Command::new("python3")
        .arg(scratch.path().join("verify.py"))
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(scratch.path())
        .arg(if real {
            "--real-only"
        } else {
            "--recordings-only"
        })
        .output()
        .unwrap();
    if !output.status.success() {
        let retained = scratch.keep();
        panic!(
            "evidence {} stdout={} stderr={}",
            retained.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["scripted"], if real { 0 } else { 72 });
    assert_eq!(receipt["real"], if real { 6 } else { 0 });
    assert_eq!(receipt["handshake"], if real { 3 } else { 0 });
    assert_eq!(receipt["cleanup"], true);
    if !real {
        let mut reference = std::collections::BTreeMap::new();
        for cell in receipt["recordings"].as_array().unwrap() {
            let mut recording = hirsel_rt::host_launch::read_recording(std::path::Path::new(
                cell["tape"].as_str().unwrap(),
            ))
            .unwrap();
            let binding = recording.binding.as_mut().unwrap();
            binding.admitted_at = 0;
            binding.placement = None;
            let which = cell["case"].as_u64().unwrap();
            if let Some(expected) = reference.get(&which) {
                assert_eq!(&recording, expected, "full semantic tape mismatch: {cell}");
            } else {
                reference.insert(which, recording);
            }
        }
        assert_eq!(reference.len(), 12);
    }
    eprintln!("watchfiles_27 executed {receipt}");
    let removed = scratch.path().to_owned();
    scratch.close().unwrap();
    assert!(!removed.exists());
}

#[test]
fn watchfiles_27_native_scripted() {
    native_workflows(false);
}

#[test]
fn watchfiles_27_real_files() {
    native_workflows(true);
}

#[test]
fn watchfiles_27_argument_and_snapshot_boundaries() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = prepare(false);
        let mut loaded = hirsel_driver::load_program(&scratch.path().join("hirsel.json")).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let invalid: Vec<Vec<String>> = vec![
            vec![],
            vec!["0", "1", "1000", "a"],
            vec!["1", "0", "1000", "a"],
            vec!["1", "17", "1000", "a"],
            vec!["x", "1", "1000", "a"],
            vec!["1", "1", "1000", "/a"],
            vec!["1", "1", "1000", "../a"],
            vec!["1", "1", "1000", "a//b"],
            vec!["1", "1", "1000", "./a"],
            vec!["1", "1", "1000", "a\\b"],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_owned).collect())
        .collect();
        for args in invalid.into_iter().chain([
            [
                vec!["1".into(), "1".into(), "1000".into()],
                vec!["a".into(); 17],
            ]
            .concat(),
            vec!["1".into(), "1".into(), "1000".into(), "a".repeat(1025)],
        ]) {
            let world = hirsel_engine::SimulationConfig {
                args: args.clone(),
                ..Default::default()
            };
            let plain =
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone())
                    .unwrap();
            let checked =
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world).unwrap();
            for execution in [&plain, &checked] {
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(hirsel_engine::RunOutcome::Returned(
                            hirsel_run_schema::Returned::Error
                        ))
                    ),
                    "{args:?}: {:?}",
                    execution.outcome
                );
                assert!(execution.observation.as_ref().unwrap().output.is_empty());
            }
            assert_eq!(plain.observation, checked.observation);
        }
        // The same complete CLI path reads all 16 files at the exact byte cap.
        let mut world = hirsel_engine::SimulationConfig::default();
        world.grants.insert("MonotonicClock".into());
        world.projections.insert("monotonic_clock".into());
        world
            .mounts
            .insert("data".into(), hirsel_run_schema::Access::Read);
        world.directories.insert("data".into());
        world.args = vec!["1".into(), "1".into(), "1000".into()];
        for index in 0..16 {
            let name = format!("f{index}");
            world.args.push(name.clone());
            world.files.insert(
                format!("data/{name}"),
                if index == 0 {
                    vec![b'x'; 65536]
                } else {
                    vec![]
                },
            );
        }
        let plain =
            hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone()).unwrap();
        let checked = hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world).unwrap();
        let expected = (0..16)
            .map(|index| {
                format!(
                    "0\t{index}\tinitial\t{}\n",
                    if index == 0 { 65536 } else { 0 }
                )
            })
            .collect::<String>()
            + "complete\n";
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
            assert_eq!(
                execution.observation.as_ref().unwrap().output,
                expected.as_bytes()
            );
        }
        assert_eq!(plain.observation, checked.observation);
    });
}

#[test]
fn watchfiles_27_clock_errors_and_overflow() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = prepare(false);
        let mut loaded = hirsel_driver::load_program(&scratch.path().join("hirsel.json")).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for (id, now, interval, timeout, fault, output, report) in [
            (
                "whole-deadline-overflow",
                u64::MAX,
                "1",
                "1",
                None,
                "",
                "error: Watch(Overflow)",
            ),
            (
                "interval-overflow",
                10,
                "18446744073709551615",
                "1000",
                None,
                "0\t0\tinitial\t1\n",
                "error: Watch(Overflow)",
            ),
            (
                "clock-unavailable",
                0,
                "1",
                "1000",
                Some(0),
                "",
                "error: Watch(Clock(Unavailable))",
            ),
            (
                "clock-out-of-range",
                0,
                "1",
                "1000",
                Some(1),
                "",
                "error: Watch(Clock(OutOfRange))",
            ),
        ] {
            let mut world = hirsel_engine::SimulationConfig::default();
            world.grants.insert("MonotonicClock".into());
            world.projections.insert("monotonic_clock".into());
            world
                .mounts
                .insert("data".into(), hirsel_run_schema::Access::Read);
            world.directories.insert("data".into());
            world.files.insert("data/a".into(), b"x".to_vec());
            world.monotonic = now;
            world.args = vec![interval.into(), "2".into(), timeout.into(), "a".into()];
            if let Some(code) = fault {
                world.faults.insert(
                    "clock.monotonic".into(),
                    [hirsel_interp::library::simulation::Fault::Fault { code, partial: 0 }].into(),
                );
            }
            let plain =
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone())
                    .unwrap();
            let checked =
                hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world).unwrap();
            for execution in [&plain, &checked] {
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(hirsel_engine::RunOutcome::Returned(
                            hirsel_run_schema::Returned::Error
                        ))
                    ),
                    "{id}: {:?}",
                    execution.outcome
                );
                assert_eq!(
                    execution.observation.as_ref().unwrap().output,
                    output.as_bytes(),
                    "{id}"
                );
                assert_eq!(
                    execution.exit_report.as_ref().unwrap().text.trim(),
                    report,
                    "{id}"
                );
            }
            assert_eq!(plain.observation, checked.observation, "{id}");
        }
    });
}
