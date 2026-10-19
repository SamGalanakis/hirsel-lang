use hirsel_driver::*;
use hirsel_engine::*;
#[test]
fn expiration_visits_only_dependencies_of_the_ended_owner() {
    with_compiler_stack(|| {
        for closures in [false, true] {
            for count in [16, 64, 256] {
                let statements = (0..count)
                    .map(|i| {
                        if closures {
                            format!("let x{i}=fn()->u8{{let _=p;a}}\n")
                        } else {
                            format!("let x{i}:List[u8] of a=p\n")
                        }
                    })
                    .collect::<String>();
                let source = format!("fn f(a:u8,p:List[u8] of a)->u8{{{statements}a}}");
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine
                    .sources
                    .add("origin-dependencies.hir", source)
                    .unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                let before = hirsel_intern::work::read(
                    hirsel_intern::work::Counter::ValidateOriginDependencyRows,
                );
                let output = engine
                    .lower_to_tir(&parsed.value.items[0].item.definition)
                    .unwrap();
                assert!(output.diagnostics.is_empty());
                let rows = hirsel_intern::work::read(
                    hirsel_intern::work::Counter::ValidateOriginDependencyRows,
                ) - before;
                assert!(
                    rows <= 128 * count + 512,
                    "{count} closures={closures} inspected {rows} expiry dependencies"
                );
            }
        }
    });
}
