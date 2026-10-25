//! The bounded pattern APIs must stay ordinary audited source, with no host hooks.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

// Go syntax/parse_test.go:505-507 at ebe2feef. Each exact input independently
// exceeds the corpus's 1M interpreter steps, so keep it in a bounded driver run.
fn regex_large_invalid(line: u16) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/regex-go-invalid-{line}.hir"));
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(5_000_000),
            hirsel_engine::Verify::On,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ),
            "Go syntax/parse_test.go:{line}: {outcome:?}"
        );
    });
}

#[test]
fn regex_go_nested_nullable_stars_exceed_the_program_bound() {
    regex_large_invalid(505);
}

#[test]
fn regex_go_large_group_repeat_exceeds_the_program_bound() {
    regex_large_invalid(506);
}

#[test]
fn regex_go_long_sequence_exceeds_the_program_bound() {
    regex_large_invalid(507);
}

#[path = "stdlib_regex_all_5.rs"]
mod regex_all_5;
