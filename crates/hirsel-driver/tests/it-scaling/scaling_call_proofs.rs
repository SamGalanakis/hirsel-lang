use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn calls_query_only_required_binding_origins() {
    with_compiler_stack(|| {
        for count in [16, 64, 256, 400, 1600] {
            let statements = (0..count)
                .step_by(32)
                .enumerate()
                .map(|(i, start)| {
                    let depth = 32.min(count - start);
                    format!(
                        "let x{i}:u8 = {}a{}\n",
                        "id(".repeat(depth),
                        ")".repeat(depth)
                    )
                })
                .collect::<String>();
            let source = format!("fn id(a:u8)->u8{{a}}\nfn f(a:u8)->u8{{{statements}a}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("call-proofs.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let path = &parsed.value.items[1].item.definition;
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCallProofRows);
            let result = engine.lower_to_tir(path).unwrap();
            assert!(result.diagnostics.is_empty());
            let rows =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCallProofRows)
                    - before;
            eprintln!("{count} calls: {rows} binding-origin queries");
            assert!(
                rows <= (32 * count + 256) as u64,
                "{count} calls examined {rows} binding origins"
            );
        }
    });
}
