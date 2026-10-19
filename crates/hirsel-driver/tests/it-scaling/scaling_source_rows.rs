use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn bounded_source_shapes_use_indexed_rows() {
    with_compiler_stack(|| {
        for count in [100, 400, 1600] {
            let grouped = |kind| {
                (0..count)
                    .step_by(32)
                    .enumerate()
                    .map(|(i, start)| {
                        let depth = 32.min(count - start);
                        let value = match kind {
                            0 => format!("{}a{}", "{".repeat(depth), "}".repeat(depth)),
                            1 => format!("{}a{}", "id(".repeat(depth), ")".repeat(depth)),
                            _ => format!(
                                "{}{{a}}",
                                (start..start + depth)
                                    .map(|n| format!("if a == {n}u64 {{a}} else "))
                                    .collect::<String>()
                            ),
                        };
                        format!("let x{i}:u64 = {value}\n")
                    })
                    .collect::<String>()
            };
            for (shape, statements) in [
                ("nested", grouped(0)),
                ("calls", grouped(1)),
                ("else-if", grouped(2)),
                (
                    "closures",
                    (0..count)
                        .map(|i| format!("let x{i} = fn()->u64{{a}}\n"))
                        .collect(),
                ),
            ] {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine
                    .sources
                    .add(
                        "source-rows.hir",
                        format!("fn id(a:u64)->u64{{a}}\nfn f(a:u64)->u64{{{statements}a}}"),
                    )
                    .unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                let path = &parsed.value.items[1].item.definition;
                let h = hirsel_intern::work::read(hirsel_intern::work::Counter::BodySourceRows);
                let j =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::AuthoritySourceRows);
                let result = engine.mode_definition(path).unwrap();
                assert!(result.diagnostics.is_empty());
                let h = hirsel_intern::work::read(hirsel_intern::work::Counter::BodySourceRows) - h;
                let j =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::AuthoritySourceRows)
                        - j;
                eprintln!("{shape} {count}: H {h}, J {j} row visits");
                assert!(
                    h <= 512 * count as u64 + 8192,
                    "{shape} {count}: H scanned {h} source rows"
                );
                assert!(
                    j <= 512 * count as u64 + 8192,
                    "{shape} {count}: J scanned {j} source rows"
                );
            }
        }
    });
}
