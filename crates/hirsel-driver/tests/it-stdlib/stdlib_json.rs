//! Source JSON conformance plus an independent Python json/number-lexeme oracle.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{path::PathBuf, process::Command};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn fixture(name: &str) -> PathBuf {
    root().join(format!("conformance/SEM/SEM-3-json-{name}-run.hir"))
}
fn execute_source(source: &str) {
    let mut loaded = load_program_source(&fixture("corpus"), Some(source)).unwrap();
    loaded.check_for_execution().unwrap();
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
        matches!(run.outcome, Ok(RunOutcome::Exit(0))),
        "{:?}",
        run.outcome
    );
}
fn python_oracle(fuzz: bool) -> String {
    let mut command = Command::new("python3");
    command.arg(root().join("crates/hirsel-driver/tests/fixtures/stdlib_json/oracle.py"));
    if fuzz {
        command.arg("--fuzz");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn python_json_oracle_preserves_every_valid_corpus_number_lexeme() {
    with_compiler_stack(|| execute_source(&python_oracle(false)));
}
#[test]
fn seeded_bounded_json_fuzz_matches_python_policy_and_exact_numbers() {
    // One 32-tree + 32-mutation campaign in the ordinary per-push driver tests.
    with_compiler_stack(|| execute_source(&python_oracle(true)));
}
