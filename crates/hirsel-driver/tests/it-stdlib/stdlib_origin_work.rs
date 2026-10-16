//! Deterministic work bound for selecting the ordinary numeric std package.
use hirsel_driver::*;
use hirsel_engine::*;

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir")
}

#[test]
fn selecting_numeric_std_checks_origins_with_bounded_visits() {
    with_compiler_stack(|| {
        use hirsel_intern::work::{self, Counter};
        let mut loaded = load_program_source(&fixture(), Some("pub fn main() -> () {}")).unwrap();
        let before = work::read(Counter::ValidateOriginDependencyRows);
        loaded.check_for_execution().unwrap();
        let visits = work::read(Counter::ValidateOriginDependencyRows) - before;
        eprintln!("empty main with std@1: {visits} origin visits");
        assert!(visits <= 100_000, "{visits} origin visits exceed 100000");
    });
}
