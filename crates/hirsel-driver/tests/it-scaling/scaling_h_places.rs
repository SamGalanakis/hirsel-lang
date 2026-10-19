use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn body_place_publication_uses_expression_identity() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let source = format!(
                "fn f(a:u8)->u8{{{}a}}",
                (0..count)
                    .map(|i| format!("let x{i}=fn()->u8{{a}}\n"))
                    .collect::<String>()
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("h-places.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::BodySourcePlaceRows);
            let output = engine
                .check_definition(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty());
            let rows = hirsel_intern::work::read(hirsel_intern::work::Counter::BodySourcePlaceRows)
                - before;
            assert!(
                rows <= 64 * count + 256,
                "{count} closures inspected {rows} source place rows"
            );
        }
    });
}
