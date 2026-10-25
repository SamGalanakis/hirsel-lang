//! Compare every capture resolver with the unchanged ordered PikeVM.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixed_for(backend: Option<native::Backend>, entries: &[&str]) {
    fixed_for_mode(backend, entries, None);
}

fn fixed_for_mode(backend: Option<native::Backend>, entries: &[&str], checked_only: Option<bool>) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = super::regex_capture_span_17::private_fixture(temp.path(), false);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["sources"].as_array_mut().unwrap().push(serde_json::json!({
            "path": root.join("tests/fixtures/capture-fixed-tags-37.hir").canonicalize().unwrap(),
            "module": ["regex"]
        }));
        for &entry in entries {
            eprintln!("fixed tags {entry} {backend:?}");
            manifest["entry"]["name"] = entry.into();
            std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            if let Some(backend) = backend {
                let outcome = native::run_path_native_backend(
                    &path,
                    HostLaunch::empty(),
                    None,
                    backend,
                    native::Output::Streams {
                        stdout: &mut Vec::new(),
                        stderr: &mut Vec::new(),
                    },
                    Verify::On,
                )
                .unwrap()
                .unwrap();
                assert!(
                    matches!(
                        outcome,
                        RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                    ),
                    "{entry} {backend:?}: {outcome:?}"
                );
            } else {
                let mut loaded = load_program(&path).unwrap();
                loaded.check_for_execution().unwrap();
                loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                    limits: hirsel_interp::Limits {
                        steps: Some(200_000_000),
                        ..Default::default()
                    },
                    ..Default::default()
                }));
                let config = RunConfiguration {
                    entry: loaded.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                };
                let mut plain_observation = None;
                if checked_only != Some(true) {
                    let plain = run_observed_snapshot(
                        &mut loaded.engine,
                        &config,
                        SimulationConfig::default(),
                    )
                    .unwrap();
                    assert!(
                        matches!(
                            plain.outcome,
                            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                        ),
                        "{entry}: {:?}",
                        plain.outcome
                    );
                    plain_observation = Some(plain.observation);
                }
                if checked_only != Some(false) {
                    let checked = run_observed_checked(
                        &mut loaded.engine,
                        &config,
                        SimulationConfig::default(),
                    )
                    .unwrap();
                    assert!(
                        matches!(
                            checked.outcome,
                            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                        ),
                        "{entry}: {:?}",
                        checked.outcome
                    );
                    if let Some(plain) = plain_observation {
                        assert_eq!(plain, checked.observation, "{entry}");
                    }
                }
            }
        }
    });
}

macro_rules! fixed_test {
    ($name:ident, $backend:expr, $entry:literal) => {
        #[test]
        fn $name() {
            fixed_for($backend, &[$entry]);
        }
    };
}
fixed_test!(vectors_reference_checked, None, "fixed37_vectors");
fixed_test!(vectors_llvm, Some(native::Backend::Llvm), "fixed37_vectors");
fixed_test!(
    vectors_cranelift,
    Some(native::Backend::Cranelift),
    "fixed37_vectors"
);
fixed_test!(generated_reference_checked, None, "fixed37_generated");
fixed_test!(
    generated_llvm,
    Some(native::Backend::Llvm),
    "fixed37_generated"
);
fixed_test!(
    generated_cranelift,
    Some(native::Backend::Cranelift),
    "fixed37_generated"
);
fixed_test!(limits_reference_checked, None, "fixed37_limits");
fixed_test!(limits_llvm, Some(native::Backend::Llvm), "fixed37_limits");
fixed_test!(
    limits_cranelift,
    Some(native::Backend::Cranelift),
    "fixed37_limits"
);
// The combined work cell exceeded RE600. Each engine retains the complete
// original entry, vectors, assertions and independent 200M-step limit.
#[test]
fn work_reference() {
    fixed_for_mode(None, &["fixed37_work"], Some(false));
}
// The complete checked-H work entry also exceeded RE600. These disjoint
// partitions preserve every original helper sweep and public-budget assertion.
#[test]
fn work_checked_output_zero() {
    fixed_for_mode(None, &["fixed37_work_output_zero"], Some(true));
}
#[test]
fn work_checked_output_two() {
    fixed_for_mode(None, &["fixed37_work_output_two"], Some(true));
}
#[test]
fn work_checked_output_large() {
    fixed_for_mode(None, &["fixed37_work_output_large"], Some(true));
}
#[test]
fn work_checked_search() {
    fixed_for_mode(None, &["fixed37_work_search"], Some(true));
}
#[test]
fn work_checked_public() {
    fixed_for_mode(None, &["fixed37_work_public"], Some(true));
}

fixed_test!(work_llvm, Some(native::Backend::Llvm), "fixed37_work");
fixed_test!(
    work_cranelift,
    Some(native::Backend::Cranelift),
    "fixed37_work"
);

// Exact original backtrack-25 native aggregate entries, bounded separately.
// Keep the existing aggregate and all original fixtures/assertions unchanged.
macro_rules! backtrack_control {
    ($name:ident, $backend:ident, $entry:literal) => {
        #[test]
        #[ignore = "bounded recovery selector; original aggregates remain registered"]
        fn $name() {
            super::regex_backtrack_25::engines_for(Some(native::Backend::$backend), &[$entry]);
        }
    };
}

// Exercise public package import, nonzero String views and caught callback
// cleanup on the remaining engines. Corpus already supplies the reference run.
fn public_case(file: &str, backend: Option<native::Backend>) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("conformance")
            .join(file);
        let outcome = if let Some(backend) = backend {
            // Cleanup receives Io solely to project its Tasks capability.
            // An explicit live Io provider grants that entry root; no file,
            // network, clock or output capability is needed by the fixture.
            let launch = if file.starts_with("FAIL/") {
                HostLaunch::Live {
                    config: LaunchConfig::Grants {
                        input: std::ffi::OsStr::new(
                            r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#,
                        ),
                        arguments: None,
                    },
                    record: None,
                    revocations: None,
                }
            } else {
                HostLaunch::empty()
            };
            native::run_path_native_backend(
                &path,
                launch,
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut Vec::new(),
                    stderr: &mut Vec::new(),
                },
                Verify::On,
            )
            .unwrap()
            .unwrap()
        } else {
            let mut loaded = load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(1_000_000),
                    ..Default::default()
                },
                ..Default::default()
            }));
            let config = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default())
                .unwrap()
                .outcome
                .unwrap()
        };
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{file} {backend:?}: {outcome:?}"
        );
    });
}
macro_rules! public_test {
    ($name:ident, $file:literal, $backend:expr) => {
        #[test]
        fn $name() {
            public_case($file, $backend);
        }
    };
}
