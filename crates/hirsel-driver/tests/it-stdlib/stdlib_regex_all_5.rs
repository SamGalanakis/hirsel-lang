//! Enumeration stays ordinary source, with the existing Pike VM and no host hook.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

#[test]
fn regex_all_5_logtail_importer_on_interpreter_llvm_and_cranelift() {
    with_compiler_stack(|| {
        let directory_scratch = tempfile::Builder::new()
            .prefix("regex-all-5-logtail-")
            .tempdir()
            .unwrap();
        let directory = directory_scratch.path().to_path_buf();
        let source = directory.join("main.hir");
        std::fs::write(
            &source,
            include_str!("../../../../examples/logtail/main.hir"),
        )
        .unwrap();
        let std_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = serde_json::json!({
            "schema": 1, "package": {"name": "regex_all_5_logtail", "version": "1"},
            "entry": {"module": [], "name": "main"},
            "sources": [{"path": "main.hir", "module": []}],
            "dependencies": [{"manifest": std_manifest, "package": {"name": "std", "version": "1"}, "expose": []}]
        });
        let path = directory.join("hirsel.json");
        std::fs::write(&path, manifest.to_string()).unwrap();
        let results = testing::run_path(
            &path,
            &[],
            Some(1),
            false,
            Some(1_000_000),
            &testing::Schedules::Default,
            Verify::On,
        )
        .unwrap();
        assert_eq!(results.len(), 3);
        assert!(
            results.iter().all(|result| result.failure.is_none()),
            "{results:?}"
        );
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let results = native::test_path(
                &path,
                &[],
                Some(1),
                backend,
                false,
                &testing::Schedules::Default,
                Verify::On,
            )
            .unwrap();
            assert_eq!(results.len(), 3);
            assert!(
                results.iter().all(|result| result.failure.is_none()),
                "{backend:?}: {results:?}"
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    });
}
