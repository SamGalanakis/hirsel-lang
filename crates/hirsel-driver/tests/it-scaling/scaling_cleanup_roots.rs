use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn cleanup_compilation_inspects_only_changed_live_roots() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            for closure in [false, true] {
                let statements = (0..count)
                    .map(|i| {
                        if closure {
                            format!("let x{i} = fn()->u8{{a}}\n")
                        } else {
                            format!("let x{i}:u8 = a\n")
                        }
                    })
                    .collect::<String>();
                let source = format!("fn f(a:u8)->u8{{{statements}a}}");
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("cleanup-roots.hir", source).unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                let before = hirsel_intern::work::read(
                    hirsel_intern::work::Counter::LoweringCleanupRootRows,
                );
                let result = engine
                    .lower_to_tir(&parsed.value.items[0].item.definition)
                    .unwrap();
                assert!(result.diagnostics.is_empty());
                let rows = hirsel_intern::work::read(
                    hirsel_intern::work::Counter::LoweringCleanupRootRows,
                ) - before;
                assert!(
                    rows <= 128 * count + 512,
                    "{count} statements (closure {closure}) compiled {rows} live root rows"
                );
            }
        }
    });
}
