//! The bounded pattern APIs must stay ordinary audited source, with no host hooks.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn glob_exports_have_source_owners_and_no_builtin_operations() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for name in ["SEM-3-glob-run", "SEM-3-glob-files-run"] {
            let path = root.join(format!("conformance/SEM/{name}.hir"));
            let mut loaded = load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let snapshot = loaded.engine.export_native_input().unwrap();
            let names: Vec<_> = snapshot
                .source_owners
                .keys()
                .filter(|p| p.0.package.name_is("std") && p.0.modules == ["glob"])
                .map(|p| p.0.name.as_str())
                .collect();
            for function in if name.contains("files") {
                vec!["compile", "is_match", "files"]
            } else {
                vec!["compile", "is_match"]
            } {
                assert!(
                    names.contains(&function),
                    "missing source owner for glob.{function}"
                );
                assert!(
                    registry::BuiltinRegistry::new()
                        .get(&hirsel_syntax::DefPath::core(&format!("glob.{function}")))
                        .is_none()
                );
            }
            let run = run_path(&path, &mut Vec::new()).unwrap();
            assert!(
                matches!(run, RunOutcome::Returned(hirsel_run_schema::Returned::Ok)),
                "{name}: {run:?}"
            );
        }
    });
}

#[test]
fn regex_exports_have_source_owners_and_no_builtin_operations() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = root.join("conformance/SEM/SEM-3-regex-edges-run.hir");
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let snapshot = loaded.engine.export_native_input().unwrap();
        let names: Vec<_> = snapshot
            .source_owners
            .keys()
            .filter(|p| p.0.package.name_is("std") && p.0.modules == ["regex"])
            .map(|p| p.0.name.as_str())
            .collect();
        for function in ["compile", "find", "is_match"] {
            assert!(
                names.contains(&function),
                "missing source owner for regex.{function}"
            );
            assert!(
                registry::BuiltinRegistry::new()
                    .get(&hirsel_syntax::DefPath::core(&format!("regex.{function}")))
                    .is_none()
            );
        }
    });
}

// Go syntax/parse_test.go:505-507 at ebe2feef. Each exact input independently
// exceeds the corpus's 1M interpreter steps, so keep it in a bounded driver run.
fn regex_large_invalid(line: u16) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/regex-go-invalid-{line}.hir"));
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(5_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "Go syntax/parse_test.go:{line}: {outcome:?}"
        );
    });
}

#[test]
fn regex_go_nested_nullable_stars_exceed_the_program_bound() {
    regex_large_invalid(505);
}

#[test]
fn regex_go_large_group_repeat_exceeds_the_program_bound() {
    regex_large_invalid(506);
}

#[test]
fn regex_go_long_sequence_exceeds_the_program_bound() {
    regex_large_invalid(507);
}
