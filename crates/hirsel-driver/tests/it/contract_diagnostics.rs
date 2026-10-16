use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn contract_pseudo_values_are_rejected_before_lowering_outside_ensures() {
    with_compiler_stack(|| {
        for (source, operation) in [
            ("fn f()->u8 { result }", "result"),
            ("fn f(a:u8)->u8 requires result > 0u8 { a }", "result"),
            ("fn f(a:u8)->u8 { before(a) }", "before(a)"),
            ("fn f(a:u8)->u8 requires before(a) > 0u8 { a }", "before(a)"),
            (
                "fn f(a:u8)->u8 ensures (fn()->bool { result > 0u8 })() { a }",
                "result",
            ),
            (
                "fn f(a:u8)->u8 ensures { fn g()->u8 { before(a) } g() > 0u8 } { a }",
                "before(a)",
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("contracts.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete, "{source}");
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(diagnostic.rule.as_str(), "FACT-1", "{source}");
            assert_eq!(
                output.sources.slice(diagnostic.primary),
                Some(operation),
                "{source}"
            );
        }
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "valid.hir",
                "fn f(a:u8)->u8 ensures result == before(a) { a }".into(),
            )
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    });
}
