//! MODE-2: copying a unique local must preserve the saved value across mutation.
use hirsel_engine::{DefinitionQueries, QueryEngine, default_context, default_environment};

#[test]
fn copied_unique_local_survives_mutable_handback() {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let context = default_context();
    let file = engine
        .sources
        .add(
            "unique-copy.hir",
            "fn keep(mut x: List[u64]) -> () {}\nfn main() -> u64 { let mut xs: unique List[u64] = [10u64]; let saved = copy xs; keep(mut xs); xs[0u64] = 11u64; xs[0u64] + saved[0u64] }".into(),
        )
        .unwrap();
    let diagnostics = hirsel_engine::check_snapshot(&mut engine, file, &context).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let entry = hirsel_engine::entry_path(&context);
    engine.lower_to_tir(&entry).unwrap();
    let runtime = engine.checked_runtime_types().unwrap();
    let sources = engine.sources.clone();
    let execution = hirsel_interp::run_with_runtime(
        &mut engine,
        &sources,
        &hirsel_tir::RunConfiguration {
            entry,
            hash_seed: 0,
            inputs: vec![],
        },
        vec![],
        hirsel_interp::Limits::default(),
        &mut hirsel_interp::NoOperations,
        &runtime,
    )
    .unwrap();
    assert!(
        matches!(execution.outcome, hirsel_interp::Outcome::Returned { value: hirsel_interp::Value::Integer(n), .. } if n.bits() == 21),
        "{execution:?}"
    );
}
