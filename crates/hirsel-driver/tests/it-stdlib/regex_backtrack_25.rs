//! Compare every capture resolver with the unchanged ordered PikeVM.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

pub(super) fn engines_for(backend: Option<native::Backend>, entries: &[&str]) {
    engines_for_budget(backend, entries, 200_000_000);
}

fn engines_for_budget(backend: Option<native::Backend>, entries: &[&str], steps: u64) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = super::regex_capture_span_17::private_fixture(temp.path(), false);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "path": root.join("tests/fixtures/regex-backtrack-25.hir").canonicalize().unwrap(),
                "module": ["regex"]
            }));
        for &entry in entries {
            eprintln!("capture engines {entry} {backend:?}");
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
                let prepared = request_tir(&path, Verify::On, None).unwrap();
                let limits = hirsel_interp::Limits {
                    steps: Some(steps),
                    ..Default::default()
                };
                let config = RunConfiguration {
                    entry: prepared.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                };
                let plain = testing::observe_tir(
                    &prepared,
                    &config,
                    SimulationConfig::default(),
                    limits,
                    false,
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
                let checked = testing::observe_tir(
                    &prepared,
                    &config,
                    SimulationConfig::default(),
                    limits,
                    true,
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
                assert_eq!(plain.observation, checked.observation, "{entry}");
            }
        }
    });
}

fn engines(backend: Option<native::Backend>) {
    engines_for(
        backend,
        &[
            "backtrack25_edges",
            "backtrack25_ascii",
            "backtrack25_random",
            "backtrack25_limits",
        ],
    );
}
#[test]
fn interpreter_and_checked_h() {
    engines(None);
}
#[test]
fn llvm() {
    engines(Some(native::Backend::Llvm));
}
#[test]
fn cranelift() {
    engines(Some(native::Backend::Cranelift));
}

// The unchanged legacy entry set, each retaining its original 100M-step budget.
// Separate libtest cells bound the process and add checked-H observation parity.
macro_rules! legacy {
    ($name:ident) => {
        #[test]
        fn $name() {
            engines_for_budget(None, &[stringify!($name)], 100_000_000);
        }
    };
}
legacy!(capture_exhaustive_0);
legacy!(capture_exhaustive_1);
legacy!(capture_exhaustive_2);
legacy!(capture_exhaustive_3);
legacy!(capture_exhaustive_4);
legacy!(capture_exhaustive_5);
legacy!(capture_exhaustive_6);
legacy!(capture_exhaustive_7);
legacy!(capture_exhaustive_8);
legacy!(capture_exhaustive_9);
legacy!(capture_exhaustive_10);
legacy!(capture_exhaustive_11);
legacy!(capture_exhaustive_12);
legacy!(capture_exhaustive_13);
legacy!(capture_exhaustive_14);
legacy!(capture_exhaustive_15);
legacy!(capture_replacements_0_0);
legacy!(capture_replacements_0_1);
legacy!(capture_replacements_0_2);
legacy!(capture_replacements_1_0);
legacy!(capture_replacements_1_1);
legacy!(capture_replacements_1_2);
legacy!(capture_replacements_2_0);
legacy!(capture_replacements_2_1);
legacy!(capture_replacements_2_2);
legacy!(capture_replacements_3_0);
legacy!(capture_replacements_3_1);
legacy!(capture_replacements_3_2);
legacy!(capture_budgets);
legacy!(capture_goldens);
legacy!(capture_retention);
legacy!(capture_expand_partial);
legacy!(capture_forced_limits);
legacy!(capture_context_0);
legacy!(capture_context_1);
legacy!(capture_context_2);
legacy!(capture_context_3);
legacy!(capture_context_4);
legacy!(capture_context_5);
legacy!(capture_context_6);
legacy!(capture_context_7);
legacy!(capture_context_8);
