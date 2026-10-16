use hirsel_driver::*;
use hirsel_engine::*;
#[test]
#[ignore = "full gate"]
fn full_gate_deepest_supported_sources_lower_and_validate_successfully() {
    with_compiler_stack(|| {
        let count = hirsel_parser::MAX_PARSE_DEPTH - 2;
        let sources = [
            format!(
                "fn f(a:u8)->u8{{ {}a{} }}",
                "{".repeat(count),
                "}".repeat(count)
            ),
            format!(
                "fn id(a:u8)->u8{{a}}\nfn f(a:u8)->u8{{ {}a{} }}",
                "id(".repeat(count),
                ")".repeat(count)
            ),
            format!(
                "fn f(a:bool)->bool{{ {}{{a}} }}",
                "if a { a } else ".repeat(count - 1)
            ),
            format!(
                "fn f(a:u8)->u8{{ {} }}",
                std::iter::repeat_n("a", hirsel_parser::MAX_PARSE_DEPTH - 1)
                    .collect::<Vec<_>>()
                    .join("+")
            ),
        ];
        for (shape, source) in ["nested", "calls", "else-if", "binary"]
            .into_iter()
            .zip(sources)
        {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add(format!("successful-depth-{shape}.hir"), source)
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            assert!(
                parsed.diagnostics.is_empty(),
                "{shape}: {:?}",
                parsed.diagnostics
            );
            assert!(!parsed.value.roots.is_empty(), "{shape}");
            for root in &parsed.value.roots {
                let path = &parsed.value.tree.items[root.0 as usize].definition;
                let output = engine.lower_to_tir(path).unwrap();
                assert!(
                    output.diagnostics.is_empty(),
                    "{shape}: {:?}",
                    output.diagnostics
                );
                assert!(!output.value.bodies.is_empty(), "{shape}");
            }
        }
    });
}
