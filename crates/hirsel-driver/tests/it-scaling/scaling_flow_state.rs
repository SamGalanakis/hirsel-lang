use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn flow_snapshots_copy_only_changed_rows() {
    with_compiler_stack(|| {
        for count in [16, 32, 64, 128] {
            let source = format!(
                "fn f(a: u64) -> u64 {{ {} {{ a }} }}",
                (0..count)
                    .map(|i| format!("if a == {i}u64 {{ a }} else "))
                    .collect::<String>()
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("flow-state.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let path = &parsed.value.tree.items[0].definition;
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCopiedRows);
            let output = engine.lower_to_tir(path).unwrap();
            assert!(output.diagnostics.is_empty());
            let copied =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCopiedRows)
                    - before;
            assert!(
                copied <= 2048 * count + 8192,
                "{count} else-if levels copied {copied} flow rows"
            );
        }
    });
}
