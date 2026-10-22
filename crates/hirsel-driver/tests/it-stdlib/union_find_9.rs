//! rustc union-find vectors and an independent adjacency reachability oracle.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str, backend: Option<native::Backend>) {
    with_compiler_stack(|| {
        let relative = format!("tests/fixtures/union-find-9/{name}.hir");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
        if let Some(backend) = backend {
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
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("{error}"));
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
        } else {
            let outcome = run_path_with_step_budget(
                &path,
                SimulationConfig::default(),
                Some(20_000_000),
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
    });
}

macro_rules! cases {
    ($h:ident, $llvm:ident, $cl:ident, $fixture:literal) => {
        #[test]
        fn $h() {
            fixture($fixture, None);
        }
        #[test]
        fn $llvm() {
            fixture($fixture, Some(native::Backend::Llvm));
        }
        #[test]
        fn $cl() {
            fixture($fixture, Some(native::Backend::Cranelift));
        }
    };
}
cases!(
    reference_h,
    reference_llvm,
    reference_cranelift,
    "reference"
);
cases!(
    contracts_h,
    contracts_llvm,
    contracts_cranelift,
    "contracts"
);
cases!(model_h, model_llvm, model_cranelift, "model");
cases!(
    structure_h,
    structure_llvm,
    structure_cranelift,
    "structure"
);
