use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn recursive_shared_list_specimens_finish_complete_checking() {
    with_compiler_stack(|| {
        for source in [
            include_str!("checker-delay-shared-corrected.hir"),
            include_str!("../../../../conformance/LANG/LANG-2-recursive-shared-checker-ok.hir"),
        ] {
            let output = if source.contains("std@1/") {
                load_program_source(
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../hirsel-driver/tests/fixtures/std_numeric.hir"),
                    Some(source),
                )
                .unwrap()
                .check()
                .unwrap()
            } else {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine
                    .sources
                    .add("shared-list.hir", source.into())
                    .unwrap();
                check_progress(&mut engine, file, &default_context()).unwrap()
            };
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            assert_eq!(output.completion, CheckCompletion::Complete);
        }
    });
}
