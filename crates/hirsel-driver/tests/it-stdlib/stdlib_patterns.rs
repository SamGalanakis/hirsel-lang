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
                vec!["compile", "matches", "files"]
            } else {
                vec!["compile", "matches"]
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
            assert!(matches!(run, RunOutcome::Exit(0)), "{name}: {run:?}");
        }
    });
}
