use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn acyclic_dataflow_visits_follow_cfg_dependencies() {
    with_compiler_stack(|| {
        for count in [8, 16, 32, 64] {
            for calls in [false, true] {
                let source = if calls {
                    format!(
                        "fn id(a:u8)->u8{{a}}\nfn f(a:u8)->u8{{{}a{}}}",
                        "id(".repeat(count),
                        ")".repeat(count)
                    )
                } else {
                    format!(
                        "fn f(a:u64)->u64{{{}{{a}}}}",
                        (0..count)
                            .map(|i| format!("if a=={i}u64{{a}}else "))
                            .collect::<String>()
                    )
                };
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("scheduling.hir", source).unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                assert!(parsed.diagnostics.is_empty());
                let before = (
                    hirsel_intern::work::read(hirsel_intern::work::Counter::OwnershipLivenessSteps),
                    hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFlowSteps),
                );
                for id in &parsed.value.items {
                    let path = &id.item.definition;
                    let output = engine.lower_to_tir(path).unwrap();
                    assert!(output.diagnostics.is_empty());
                }
                let steps = (
                    hirsel_intern::work::read(hirsel_intern::work::Counter::OwnershipLivenessSteps)
                        - before.0,
                    hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFlowSteps)
                        - before.1,
                );
                assert!(
                    steps.0 <= 32 * count as u64 + 128 && steps.1 <= 32 * count as u64 + 128,
                    "{count} {} levels: J demand {}, V flow {}",
                    if calls { "call" } else { "else-if" },
                    steps.0,
                    steps.1
                );
            }
        }
    });
}
