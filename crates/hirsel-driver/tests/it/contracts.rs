use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn fact_predicates_are_boolean_even_without_a_constructor_use() {
    hirsel_engine::with_compiler_stack(
        fact_predicates_are_boolean_even_without_a_constructor_use_on_compiler_thread,
    );
}
fn fact_predicates_are_boolean_even_without_a_constructor_use_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("facts.hir", "distinct type Bad(v:u8) where v".into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.diagnostics[0].rule.as_str(), "FACT-2");
}
