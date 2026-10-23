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

#[test]
fn plain_contract_22_rejects_effectful_helper_predicates() {
    with_compiler_stack(|| {
        // Even a helper returning a constant boolean retains its declared
        // authority. Optimizing its result cannot make a contract pure.
        for clause in ["requires", "ensures"] {
            let source = format!(
                "fn helper(d:Dir)->bool uses {{d}} {{true}}\nfn probe(d:Dir) {clause} helper(d) {{}}\npub fn main() {{}}\n"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("impure.hir", source).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let errors: Vec<_> = output
                .diagnostics
                .iter()
                .filter(|d| d.severity == hirsel_diag::Severity::Error)
                .collect();
            assert_eq!(errors.len(), 1, "{clause}: {errors:?}");
            assert_eq!(errors[0].rule.as_str(), "FACT-1");
            assert_eq!(output.sources.slice(errors[0].primary), Some("helper(d)"));
        }
    });
}
