use hirsel_driver::*;
use hirsel_engine::*;

/// A constructor's failure edge joins the state after every operand prefix.
/// Each join visits only what its operand changed, so a long table literal
/// costs V linear join work (interp-perf: 4096 elements took 9 s).
#[test]
fn constructor_failure_joins_visit_only_changed_rows() {
    with_compiler_stack(|| {
        let mut visited = vec![];
        for count in [256, 1024] {
            let source = format!(
                "const TABLE: Inline[u64, {count}] = [{}]\n",
                (0..count)
                    .map(|i| format!("{}", i * 7 % 1000))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("validate-joins.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before = hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateJoinRows);
            let output = engine
                .lower_to_tir(&parsed.value.items[0].item.definition)
                .unwrap();
            assert!(output.diagnostics.is_empty());
            visited.push(
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateJoinRows) - before,
            );
        }
        assert!(visited[0] > 0, "the table's failure edge joins no rows");
        assert!(
            visited[1] <= 5 * visited[0],
            "join rows grew from {} to {} for 4x the elements",
            visited[0],
            visited[1]
        );
    });
}
