//! cespare/xxhash TestAll and stripe transitions exceed the corpus's step budget.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::Path;

fn execute(source: &str) {
    with_compiler_stack(|| {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
        let prepared = crate::source_std::tir(&fixture, Some(source), None).unwrap();
        let run = testing::observe_tir(
            &prepared,
            &RunConfiguration {
                entry: prepared.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
            hirsel_interp::Limits {
                steps: Some(100_000_000),
                call_depth: 256,
                source: None,
            },
            false,
        )
        .unwrap();
        assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{:?}",
            run.outcome
        );
    });
}

#[test]
fn source_xxhash64_testall_every_chunk_size() {
    execute(include_str!(
        "../../../../native-tests/library/xxhash64-testall.hir"
    ));
}

#[test]
fn source_xxhash64_binary_stripes_and_mixed_updates() {
    execute(include_str!(
        "../../../../native-tests/library/xxhash64-stripes.hir"
    ));
}
