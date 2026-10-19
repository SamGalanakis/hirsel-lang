use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn temporary_identities_query_their_actual_surviving_holders() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let statements = (0..count)
                .map(|i| format!("let x{i}=fn()->u8{{a}}\n"))
                .collect::<String>();
            let source = format!("fn f(a:u8)->u8{{{statements}a}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("authority-holders.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::AuthorityHolderRows);
            let output = engine
                .mode_definition(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty());
            let rows = hirsel_intern::work::read(hirsel_intern::work::Counter::AuthorityHolderRows)
                - before;
            assert!(
                rows <= 32 * count + 128,
                "{count} closures inspected {rows} identity holders"
            );
        }
    });
}
