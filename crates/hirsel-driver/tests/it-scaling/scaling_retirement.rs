use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn statement_retirement_visits_new_slots_and_ending_scopes() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            for nested in [false, true] {
                let statements = (0..count)
                    .map(|i| {
                        if nested {
                            format!("let x{i}:u8={{a}}\n")
                        } else {
                            format!("let x{i}:u8=a\n")
                        }
                    })
                    .collect::<String>();
                let source = format!("fn f(a:u8)->u8{{{statements}a}}");
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("retirement.hir", source).unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                let before =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::LoweringRetirementRows);
                let output = engine
                    .lower_to_tir(&parsed.value.tree.items[0].definition)
                    .unwrap();
                assert!(output.diagnostics.is_empty());
                let rows =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::LoweringRetirementRows)
                        - before;
                assert!(
                    rows <= 64 * count + 256,
                    "{count} statements (nested {nested}) inspected {rows} retirement rows"
                );
            }
        }
    });
}
