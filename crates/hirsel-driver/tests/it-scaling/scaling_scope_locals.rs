use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn lexical_scope_exits_visit_only_descendant_locals() {
    with_compiler_stack(|| {
        for count in [100, 400, 1600] {
            let statements = (0..count)
                .step_by(32)
                .enumerate()
                .map(|(i, start)| {
                    let depth = 32.min(count - start);
                    format!("let x{i}:u8={}a{}\n", "{".repeat(depth), "}".repeat(depth))
                })
                .collect::<String>();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add(
                    "scope-locals.hir",
                    format!("fn f(a:u8)->u8{{{statements}a}}"),
                )
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let path = &parsed.value.tree.items[0].definition;
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateScopeLocalRows);
            let result = engine.lower_to_tir(path).unwrap();
            assert!(result.diagnostics.is_empty());
            let rows =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateScopeLocalRows)
                    - before;
            eprintln!("{count} nested blocks: {rows} scope/local candidates");
            assert!(
                rows <= 512 * count as u64 + 8192,
                "{count} blocks scanned {rows} scope/local rows"
            );
        }
    });
}
