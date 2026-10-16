use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn unchanged_branch_bindings_do_not_multiply_type_rows() {
    with_compiler_stack(|| {
        for count in [100, 400, 1600] {
            let statements = (0..count)
                .step_by(32)
                .enumerate()
                .map(|(i, start)| {
                    format!(
                        "let x{i}:u64 = {}{{a}}\n",
                        (start..(start + 32).min(count))
                            .map(|j| format!("if a == {j}u64 {{a}} else "))
                            .collect::<String>()
                    )
                })
                .collect::<String>();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add(
                    "branch-types.hir",
                    format!("fn f(a:u64)->u64{{{statements}a}}"),
                )
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let path = &parsed.value.tree.items[0].definition;
            let output = engine.mode_definition(path).unwrap();
            assert!(output.diagnostics.is_empty());
            let rows = output.value.typed.checked.types.len();
            eprintln!("{count} bounded else-if nodes: {rows} type rows");
            assert!(
                rows <= 24 * count + 128,
                "{count} else-if nodes allocated {rows} type rows"
            );
        }
    });
}
