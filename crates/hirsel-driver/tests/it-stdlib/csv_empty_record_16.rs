//! Exact CSV bytes, opt-in reparsing, defaults and exhaustive delimiter errors.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/csv-empty-record-16/{name}.hir"))
}

fn run(name: &str, backend: Option<native::Backend>) {
    with_compiler_stack(|| {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = if let Some(backend) = backend {
            native::run_path_native_backend_with_validation(
                &fixture(name),
                HostLaunch::empty(),
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{error:?}"),
                error => panic!("{error}"),
            })
            .unwrap()
        } else {
            run_path_with_step_budget(
                &fixture(name),
                SimulationConfig::default(),
                Some(10_000_000),
                Verify::On,
                &mut stdout,
                &mut stderr,
            )
            .unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{error:?}"),
                error => panic!("{error}"),
            })
        };
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{name}: {outcome:?}; {}",
            String::from_utf8_lossy(&stderr)
        );
        assert!(stdout.is_empty());
    });
}

macro_rules! cases {
    ($name:ident, $backend:expr) => {
        mod $name {
            use super::*;
            #[test]
            fn vectors() {
                run("vectors", $backend);
            }
            #[test]
            fn invalid() {
                run("invalid", $backend);
            }
        }
    };
}

cases!(interpreter, None);
cases!(llvm, Some(native::Backend::Llvm));
cases!(cranelift, Some(native::Backend::Cranelift));
