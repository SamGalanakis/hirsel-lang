use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn expiry_queries_only_brand_rows_of_its_owner() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let statements = (0..count)
                .map(|i| format!("let x{i}=fn()->u8{{a}}\n"))
                .collect::<String>();
            let source = format!("fn f(a:u8)->u8{{{statements}a}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("authority-invalidation.hir", source)
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::AuthorityInvalidationRows);
            let output = engine
                .mode_definition(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty());
            let rows =
                hirsel_intern::work::read(hirsel_intern::work::Counter::AuthorityInvalidationRows)
                    - before;
            assert!(
                rows <= 128 * count + 512,
                "{count} closures inspected {rows} brand candidates"
            );
        }
    });
}
