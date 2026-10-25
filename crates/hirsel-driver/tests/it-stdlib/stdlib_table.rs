//! Typed-table end-to-end scenarios exceed the corpus's 1M-step bound with
//! the real durable store/checksum implementation. Keep them bounded here.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn execute(name: &str) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/typed_tables")
            .join(format!("{name}.hir"));
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(16_000_000),
                call_depth: 256,
                source: None,
            },
            ..Default::default()
        }));
        let run = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry,
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{name}: {:?}",
            run.outcome
        );
    });
}

#[test]
fn typed_rows_reopen_and_ordered_ranges() {
    execute("STORE-5-typed-rows-run");
}
#[test]
fn refusals_leave_the_transaction_unchanged() {
    execute("STORE-5-typed-limits-run");
}
#[test]
fn version_reorder_rename_and_type_change_history() {
    execute("STORE-7-schema-history-run");
}
#[test]
fn reordered_record_keys_keep_logical_range_order() {
    execute("STORE-7-key-reorder-run");
}
#[test]
fn renaming_one_owner_splits_shared_record_shapes() {
    execute("STORE-7-shared-record-rename-run");
}
