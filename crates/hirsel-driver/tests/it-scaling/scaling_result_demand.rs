use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn result_permissions_follow_backward_cfg_dependencies() {
    with_compiler_stack(|| {
        for count in [8, 16, 32, 64] {
            let source = format!(
                "fn f(a:u64)->u64{{ {} {{a}} }}",
                (0..count)
                    .map(|i| format!("if a=={i}u64{{a}} else "))
                    .collect::<String>()
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("result-demand.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateResultDemandSteps);
            assert!(
                engine
                    .lower_to_tir(&parsed.value.tree.items[0].definition)
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
            let steps =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateResultDemandSteps)
                    - before;
            assert!(
                steps <= 32 * count + 128,
                "{count} else-if levels required {steps} result-demand transfers"
            );
        }
    });
}
