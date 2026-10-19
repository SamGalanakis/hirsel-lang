use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn closure_inputs_inspect_only_their_bindings_and_captures() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let statements = (0..count)
                .map(|i| format!("let x{i} = fn() -> u8 {{ a }}\n"))
                .collect::<String>();
            let source = format!("fn f(a:u8)->u8{{{statements}a}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("ownership-initial.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::OwnershipInitializedRows);
            let result = engine
                .lower_to_tir(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(result.diagnostics.is_empty());
            let rows =
                hirsel_intern::work::read(hirsel_intern::work::Counter::OwnershipInitializedRows)
                    - before;
            assert!(
                rows <= 16 * count + 128,
                "{count} closures initialized {rows} binding rows"
            );
        }
    });
}
