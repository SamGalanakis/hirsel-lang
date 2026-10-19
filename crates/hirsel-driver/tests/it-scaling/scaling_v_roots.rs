use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn temporary_ordering_visits_current_live_temporaries() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let source = format!(
                "fn f(a:u8)->u8{{{}a}}",
                (0..count)
                    .map(|i| format!("let x{i}=fn()->u8{{a}}\n"))
                    .collect::<String>()
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("v-roots.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCleanupRootRows);
            let output = engine
                .lower_to_tir(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty());
            let rows =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCleanupRootRows)
                    - before;
            assert!(
                rows <= 64 * count + 256,
                "{count} closures inspected {rows} cleanup/temporary roots"
            );
        }
    });
}
