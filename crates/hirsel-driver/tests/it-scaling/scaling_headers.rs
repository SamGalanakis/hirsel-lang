use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn headers_compose_module_declarations_once() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let source = (0..count)
                .map(|i| format!("fn f{i}(a: u8) -> u8 {{ a }}\n"))
                .collect::<String>();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ModuleDeclarationRows);
            let file = engine.sources.add("headers.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            assert!(parsed.diagnostics.is_empty());
            assert_eq!(parsed.value.items.len(), count as usize);
            for id in &parsed.value.items {
                let path = &id.item.definition;
                let resolved = engine.resolve_definition(path).unwrap();
                assert!(resolved.diagnostics.is_empty());
                let signature = engine.item_signature(path).unwrap();
                assert!(signature.diagnostics.is_empty());
            }
            let inspected =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ModuleDeclarationRows)
                    - before;
            assert!(
                inspected <= 8 * count,
                "{count} headers inspected {inspected} module declaration rows"
            );
        }
    });
}
