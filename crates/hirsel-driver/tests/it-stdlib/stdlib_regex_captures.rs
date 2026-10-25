//! Captures, flags and substitution stay ordinary bounded source code.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn regex_captures_deep_groups_and_exhausted_work_are_bounded() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = root.join("crates/hirsel-driver/tests/fixtures/regex-captures-deep.hir");
        let result = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(100_000_000),
            Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                result,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "{result:?}"
        );
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
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
                "{backend:?}: {outcome:?}"
            );
        }
    });
}
