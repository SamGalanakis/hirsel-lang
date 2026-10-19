use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn repeated_parameter_reads_use_exact_scoped_storage_lookup() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let statements = (0..count)
                .map(|i| format!("let x{i}:u8=a\n"))
                .collect::<String>();
            let source = format!("fn f(a:u8)->u8{{{statements}a}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("lower-bindings.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::LoweringBindingRows);
            let output = engine
                .lower_to_tir(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty());
            let rows = hirsel_intern::work::read(hirsel_intern::work::Counter::LoweringBindingRows)
                - before;
            assert!(
                rows <= 16 * count + 128,
                "{count} parameter reads inspected {rows} scoped bindings"
            );
        }
    });
}
