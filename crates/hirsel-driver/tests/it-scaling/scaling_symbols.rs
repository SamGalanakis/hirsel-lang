use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn header_queries_share_spelling_snapshots() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let source = (0..count)
                .map(|i| format!("fn f{i}(a: u8) -> u8 {{ a }}\n"))
                .collect::<String>();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("symbols.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedSymbols);
            for id in &parsed.value.roots {
                let path = &parsed.value.tree.items[id.0 as usize].definition;
                assert!(
                    engine
                        .resolve_definition(path)
                        .unwrap()
                        .diagnostics
                        .is_empty()
                );
                assert!(engine.item_signature(path).unwrap().diagnostics.is_empty());
            }
            let copied =
                hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedSymbols) - before;
            assert!(
                copied <= 8 * count,
                "{count} headers copied {copied} spelling rows"
            );
        }
    });
}
