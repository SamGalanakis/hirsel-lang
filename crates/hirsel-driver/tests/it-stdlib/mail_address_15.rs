//! Go net/mail and mime vectors plus bounded hostile inputs.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/mail-address-15/{name}.hir"))
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
                Some(50_000_000),
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
            #[test]
            fn encoding() {
                run("encoding", $backend);
            }
        }
    };
}

cases!(interpreter, None);
cases!(llvm, Some(native::Backend::Llvm));
cases!(cranelift, Some(native::Backend::Cranelift));

mod h {
    use super::*;

    fn audit(name: &str) {
        with_compiler_stack(|| {
            let mut loaded = load_program(&fixture(name)).unwrap_or_else(|error| match error {
                ToolError::Query { error, .. } => panic!("{error:?}"),
                error => panic!("{error}"),
            });
            loaded
                .check_for_execution()
                .unwrap_or_else(|error| match error {
                    ToolError::Query { error, .. } => panic!("{error:?}"),
                    error => panic!("{error}"),
                });
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input.clone()).unwrap();
            let ready = native::build(
                &validated,
                &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            let run = native::run_heap_in(
                &ready,
                &loaded.entry,
                &native::Sites::new(&input),
                &native::HeapWorld {
                    steps: Some(50_000_000),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(run.observation.exit_code, Some(0), "{name}");
        });
    }

    #[test]
    fn vectors() {
        audit("vectors");
    }
    #[test]
    fn invalid() {
        audit("invalid");
    }
    #[test]
    fn encoding() {
        audit("encoding");
    }
}
