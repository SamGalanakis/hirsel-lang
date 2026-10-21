//! Lane-private bounded streaming vectors; exhaustive partitions exceed the corpus budget.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str, steps: Option<u64>) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/base64-stream-5/{name}.hir"));
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
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = native::run_path_native_backend_with_validation(
                &path,
                HostLaunch::Live {
                    config: LaunchConfig::Grants {
                        input: std::ffi::OsStr::new("{}"),
                        arguments: None,
                    },
                    record: None,
                    revocations: None,
                },
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{backend:?}: {outcome:?}"
            );
            assert!(stdout.is_empty(), "{backend:?}: {stdout:?}");
            assert!(
                stderr.is_empty(),
                "{backend:?}: {}",
                String::from_utf8_lossy(&stderr)
            );
        }
    });
}

#[test]
fn base64_stream_5_every_partition() {
    fixture("partitions", Some(100_000_000));
}

#[test]
fn base64_stream_5_many_small_chunks() {
    fixture("bounded", Some(50_000_000));
}

#[test]
fn base64_stream_5_long_native() {
    // Preserve the original 24KiB control after its 50M-step interpreter red.
    fixture("bounded-long", None);
}
