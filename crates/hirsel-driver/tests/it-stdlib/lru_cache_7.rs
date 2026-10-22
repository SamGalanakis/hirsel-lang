//! LRU ownership, independent vectors and bounded reference-model traces.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str, steps: Option<u64>) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/lru-cache-7/{name}.hir"));
        if let Some(steps) = steps {
            let outcome = run_path_with_step_budget(
                &path,
                SimulationConfig::default(),
                Some(steps),
                Verify::On,
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{error:?}"),
                error => panic!("{error}"),
            });
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{outcome:?}"
            );
        }
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            // Standalone builds own unique scratch directories; run_path_native
            // shares one process-wide directory across concurrent tests.
            let directory = std::env::temp_dir().join(format!(
                "lru-cache-7-{}-{name}-{backend:?}",
                std::process::id()
            ));
            std::fs::create_dir_all(&directory).unwrap();
            let executable = directory.join("fixture");
            let grants = directory.join("grants.json");
            std::fs::write(&grants, "{}").unwrap();
            native::build_path_native_backend_with_validation(
                &path,
                &executable,
                native::TargetSpec::x86_64_unknown_linux_gnu(),
                native::BuildProfile::Default,
                backend,
                None,
                false,
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap_or_else(|error| panic!("{name} {backend:?}: {error}"))
            .unwrap_or_else(|error| panic!("{name} {backend:?}: {error:?}"));
            let output = std::process::Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .output()
                .unwrap();
            assert!(output.status.success(), "{name} {backend:?}: {output:?}");
            assert!(output.stdout.is_empty(), "{name} {backend:?}: {output:?}");
            assert!(output.stderr.is_empty(), "{name} {backend:?}: {output:?}");
            std::fs::remove_dir_all(&directory).unwrap();
        }
    });
}

#[test]
fn lru_cache_7_vectors() {
    fixture("vectors", Some(1_000_000));
}

#[test]
fn lru_cache_7_seeded_model() {
    fixture("model", Some(50_000_000));
}

#[test]
fn lru_cache_7_owned_payloads_outlive_cache() {
    fixture("owned", Some(1_000_000));
}

#[test]
fn lru_cache_7_peek_ended() {
    fixture("peek-ended", Some(1_000_000));
}

#[test]
fn lru_cache_7_compile_refusals() {
    with_compiler_stack(|| {
        for (name, rule) in [("peek-live-bad", "MODE-1"), ("noncopy-peek-bad", "GEN-1")] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/lru-cache-7/{name}.hir"));
            let mut loaded = load_program(&path).unwrap();
            let output = loaded.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            assert_eq!(errors[0].rule.as_str(), rule, "{name}: {errors:?}");
        }
    });
}
