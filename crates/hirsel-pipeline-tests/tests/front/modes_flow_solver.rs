use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn recursive_shared_list_specimens_finish_complete_checking() {
    with_compiler_stack(|| {
        for source in [
            include_str!("checker-delay-shared-corrected.hir"),
            include_str!("../../../../conformance/LANG/LANG-2-recursive-shared-checker-ok.hir"),
        ] {
            let output = load_program_source(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../hirsel-driver/tests/fixtures/std_numeric.hir"),
                Some(source),
            )
            .unwrap()
            .check()
            .unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            assert_eq!(output.completion, CheckCompletion::Complete);
        }
    });
}
