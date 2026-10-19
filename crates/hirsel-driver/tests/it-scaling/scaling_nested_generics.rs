use hirsel_driver::*;
use hirsel_engine::*;
/// Mode checking `fn f(x: Wrap[...Leaf]) -> Wrap[Wrap[...Leaf]]` once compared
/// every nested instantiation a property proof entered with every earlier
/// one, walking both complete graphs each time, and recomputed heights per
/// growth check: equality work grew as depth^4. Structural classes shape each
/// row once per proof; what remains is one proof per body row, so doubling
/// the depth may at most quadruple the work.
#[test]
fn nested_generic_instantiations_compare_structurally_once() {
    with_compiler_stack(|| {
        let work = |depth: usize| {
            let wrap = |k: usize| "Wrap[".repeat(k) + "Leaf" + &"]".repeat(k);
            let source = format!(
                "type Leaf {{ v: u64 }}\ntype Wrap[T] {{ inner: T }}\nfn f(x: {}) -> {} {{ Wrap(inner: x) }}\n",
                wrap(depth),
                wrap(depth + 1)
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("nested-generics.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before = hirsel_intern::work::read(hirsel_intern::work::Counter::EqualityRows);
            let output = engine
                .mode_definition(&parsed.value.items[2].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            hirsel_intern::work::read(hirsel_intern::work::Counter::EqualityRows) - before
        };
        let (half, full) = (work(32), work(64));
        assert!(
            full <= 5 * half,
            "{half} equality rows at depth 32, {full} at depth 64"
        );
    });
}
