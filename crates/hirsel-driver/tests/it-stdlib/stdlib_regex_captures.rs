//! Captures, flags and substitution stay ordinary bounded source code.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn regex_captures_exports_and_ownership_are_source_only() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = root.join("conformance/SEM/SEM-3-regex-captures-api-run.hir");
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let snapshot = loaded.engine.export_native_input().unwrap();
        // Source owners describe reachable code. The API fixture exercises
        // expanded/callback replacement; the Go replacement fixture also
        // reaches the literal replacement entry point.
        let literal_path = root.join("conformance/SEM/SEM-3-regex-captures-replacement-1-run.hir");
        let mut literal = load_program(&literal_path).unwrap();
        literal.check_for_execution().unwrap();
        let literal_snapshot = literal.engine.export_native_input().unwrap();
        for function in [
            "compile_flags",
            "find_captures",
            "find_all_captures",
            "expand",
            "replace_all",
            "replace_all_literal",
            "replace_all_func",
            "num_subexp",
            "subexp_names",
        ] {
            assert!(
                snapshot
                    .source_owners
                    .keys()
                    .chain(literal_snapshot.source_owners.keys())
                    .any(|p| {
                        p.0.package.name_is("std")
                            && p.0.modules == ["regex"]
                            && p.0.name == function
                    }),
                "missing regex.{function} source owner"
            );
            assert!(
                registry::BuiltinRegistry::new()
                    .get(&hirsel_syntax::DefPath::core(&format!("regex.{function}")))
                    .is_none()
            );
        }
        let run = run_path(&path, &mut Vec::new()).unwrap();
        assert!(
            matches!(run, RunOutcome::Returned(hirsel_run_schema::Returned::Ok)),
            "{run:?}"
        );
    });
}

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
