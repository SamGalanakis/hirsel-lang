use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn ownership_snapshots_copy_only_changed_binding_rows() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            for closures in [false, true] {
                let statements = (0..count)
                    .map(|i| {
                        if closures {
                            format!("let x{i} = fn() -> u8 {{ a }}\n")
                        } else {
                            format!("let x{i}:u8 = a\n")
                        }
                    })
                    .collect::<String>();
                let source = format!("fn f(a:u8)->u8{{{statements}a}}");
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("ownership-slots.hir", source).unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                let before =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedSlotRows);
                let result = engine
                    .lower_to_tir(&parsed.value.tree.items[0].definition)
                    .unwrap();
                assert!(result.diagnostics.is_empty());
                let rows = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedSlotRows)
                    - before;
                assert!(
                    rows <= 128 * count + 512,
                    "{count} {} statements copied {rows} ownership slot rows",
                    if closures { "closure" } else { "let" }
                );
            }
        }
    });
}
