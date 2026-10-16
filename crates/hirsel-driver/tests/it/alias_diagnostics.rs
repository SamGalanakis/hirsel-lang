use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn mutual_alias_cycles_are_language_failures_in_either_declaration_order() {
    with_compiler_stack(|| {
        for source in [
            "type A = B\ntype B = A\nfn f(a:A) {}",
            "type B = A\ntype A = B\nfn f(a:A) {}",
            "type A = B\ntype B = C\ntype C = A",
            "type A = A",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("aliases.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete, "{source}");
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(
                diagnostic.rule.as_str(),
                if source == "type A = A" {
                    "MEM-7"
                } else {
                    "LANG-7"
                },
                "{source}"
            );
            assert_eq!(
                diagnostic.primary.end - diagnostic.primary.start,
                if source == "type A = A" {
                    source.len() as u32
                } else {
                    1
                },
                "{source}"
            );
        }
    });
}
