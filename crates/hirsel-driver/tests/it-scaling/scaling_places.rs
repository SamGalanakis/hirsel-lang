use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn ownership_accesses_inspect_only_their_storage_root() {
    with_compiler_stack(|| {
        for closures in [false, true] {
            for count in [16, 32, 64, 128, 256] {
                let source = format!(
                    "fn f(a: u8) -> u8 {{ {} a }}",
                    (0..count)
                        .map(|i| if closures {
                            format!("let x{i} = fn()->u8{{a}}\n")
                        } else {
                            format!("let x{i}: u8 = a\n")
                        })
                        .collect::<String>()
                );
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("places.hir", source).unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                let path = &parsed.value.tree.items[0].definition;
                let before =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::ValidatePlaceRows);
                let output = engine.lower_to_tir(path).unwrap();
                assert!(output.diagnostics.is_empty());
                let inspected =
                    hirsel_intern::work::read(hirsel_intern::work::Counter::ValidatePlaceRows)
                        - before;
                assert!(
                    inspected
                        <= if closures {
                            256 * count + 512
                        } else {
                            128 * count + 256
                        },
                    "{count} closures={closures} inspected {inspected} candidate places"
                );
            }
        }
    });
}
