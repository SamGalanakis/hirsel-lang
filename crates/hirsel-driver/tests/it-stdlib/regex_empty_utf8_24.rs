//! Compare every capture resolver with the unchanged ordered PikeVM.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn engines_for(backend: Option<native::Backend>, entries: &[&str]) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = super::regex_capture_span_17::private_fixture(temp.path(), false);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // Keep the production regex sources and replace the old test vectors.
        let sources = manifest["sources"].as_array_mut().unwrap();
        sources.retain(|source| {
            !source["path"]
                .as_str()
                .unwrap()
                .ends_with("regex-capture-span-17.hir")
        });
        sources.push(serde_json::json!({
            "path": PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/regex-empty-utf8-24.hir").canonicalize().unwrap(),
            "module": ["regex"]
        }));
        for &entry in entries {
            eprintln!("empty UTF-8 {entry} {backend:?}");
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
                        steps: Some(100_000_000),
                        ..Default::default()
                    },
                    ..Default::default()
                }));
                let config = RunConfiguration {
                    entry: loaded.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                };
                let plain =
                    run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
                        .unwrap();
                assert!(
                    matches!(
                        plain.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{entry}: {:?}",
                    plain.outcome
                );
                let checked =
                    run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default())
                        .unwrap();
                assert!(
                    matches!(
                        checked.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{entry}: {:?}",
                    checked.outcome
                );
                assert_eq!(plain.observation, checked.observation, "{entry}");
            }
        }
    });
}

#[test]
fn empty_snowman_all_four() {
    for backend in [
        None,
        Some(native::Backend::Llvm),
        Some(native::Backend::Cranelift),
    ] {
        engines_for(backend, &["empty_snowman"]);
    }
}
#[test]
fn nonword_delta_all_four() {
    for backend in [
        None,
        Some(native::Backend::Llvm),
        Some(native::Backend::Cranelift),
    ] {
        engines_for(backend, &["nonword_delta"]);
    }
}

#[test]
fn differential_interpreter_and_checked_h() {
    engines_for(
        None,
        &[
            "empty_utf8_0",
            "empty_utf8_1",
            "empty_utf8_2",
            "empty_utf8_3",
        ],
    );
}
#[test]
fn differential_llvm() {
    engines_for(Some(native::Backend::Llvm), &["empty_utf8_all"]);
}
#[test]
fn differential_cranelift() {
    engines_for(Some(native::Backend::Cranelift), &["empty_utf8_all"]);
}
