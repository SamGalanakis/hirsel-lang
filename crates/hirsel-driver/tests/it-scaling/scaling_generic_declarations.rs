use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn closure_proofs_share_generic_declarations_in_smoke() {
    closure_proofs_at(&[16, 64]);
}

fn closure_proofs_at(counts: &'static [u64]) {
    with_compiler_stack(move || {
        for &count in counts {
            let statements = (0..count)
                .map(|i| format!("let x{i} = fn() -> u8 {{ a }}\n"))
                .collect::<String>();
            let source = format!("fn f(a:u8)->u8{{{statements}a}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("generic-declarations.hir", source)
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before = hirsel_intern::work::read(
                hirsel_intern::work::Counter::CopiedGenericDeclarationRows,
            );
            let result = engine
                .lower_to_tir(&parsed.value.tree.items[0].definition)
                .unwrap();
            assert!(result.diagnostics.is_empty());
            let rows = hirsel_intern::work::read(
                hirsel_intern::work::Counter::CopiedGenericDeclarationRows,
            ) - before;
            assert!(
                rows <= 64 * count + 256,
                "{count} closures copied {rows} generic declaration rows"
            );
        }
    });
}
