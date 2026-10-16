use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn the_corrected_shared_list_specimen_finishes_with_its_exact_diagnostic() {
    with_compiler_stack(|| {
        for (source, rule, line) in [
            (
                include_str!("checker-delay-shared-corrected.hir"),
                Some("LANG-2"),
                36,
            ),
            (
                include_str!("../../../../conformance/LANG/LANG-2-recursive-shared-checker-ok.hir"),
                None,
                0,
            ),
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
            assert_eq!(
                output.diagnostics.first().map(|d| d.rule.as_str()),
                rule,
                "{:?}",
                output.diagnostics
            );
            if let Some(diagnostic) = output.diagnostics.first() {
                let start = diagnostic.primary.start as usize;
                assert_eq!(
                    source[..start].bytes().filter(|b| *b == b'\n').count() + 1,
                    line
                );
                assert_eq!(
                    &source[start..diagnostic.primary.end as usize],
                    "b\"---\\n\""
                );
            } else {
                assert_eq!(output.completion, CheckCompletion::Complete);
            }
        }
    });
}
