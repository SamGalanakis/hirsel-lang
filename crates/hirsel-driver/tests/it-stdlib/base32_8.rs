//! Bounded Go/RFC4648 vectors, run on H, LLVM and Cranelift with stage validation.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/base32-8/{name}.hir"));
        {
            let steps = 20_000_000;
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
fn goldens() {
    fixture("goldens");
}

#[test]
fn corruption() {
    fixture("corruption");
}

#[test]
fn newlines_standard() {
    fixture("newlines-standard");
}

#[test]
fn newlines_hex() {
    fixture("newlines-hex");
}

#[test]
fn unused_bits() {
    fixture("unused-bits");
}

#[test]
fn sizes() {
    fixture("sizes");
}
