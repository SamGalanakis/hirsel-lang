use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn nonfailing_assignments_prepare_no_unused_failure_states() {
    with_compiler_stack(|| {
        for count in [16, 64, 256] {
            let source = format!(
                "fn f(a:u8)->u8{{ {} a }}",
                (0..count)
                    .map(|i| format!("let x{i}:u8=a\n"))
                    .collect::<String>()
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("failure-states.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFailureStateRows);
            let output = engine
                .lower_to_tir(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty());
            assert_eq!(
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFailureStateRows),
                before,
                "{count} nonfailing copies prepared unused failure snapshots"
            );
        }
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("failure-states.hir", "fn f(a:u8)->u8{ a+1u8 }".into())
            .unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap();
        let before =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFailureStateRows);
        assert!(
            engine
                .lower_to_tir(&parsed.value.items[0].item.definition)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert!(
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFailureStateRows)
                > before,
            "potentially failing arithmetic must retain its extraction-state union"
        );
    });
}
