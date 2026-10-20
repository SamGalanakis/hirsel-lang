//! Published Markdown vectors above the corpus interpreter's 1M-step budget.
//! Inputs and expected HTML remain unchanged; each case has a finite 10M-step
//! budget and the Buck2 test runner enforces its wall-clock timeout.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn run_vector(name: &str) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/markdown")
            .join(name);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            Some(10_000_000),
            hirsel_engine::Verify::On,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_config::Returned::Ok)
            ),
            "{name}: {outcome:?}\n{}\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        );
    });
}

macro_rules! vector {
    ($test:ident, $fixture:literal) => {
        #[test]
        fn $test() {
            run_vector($fixture);
        }
    };
}

vector!(
    cmark_pathological_1,
    "SEM-3-markdown-cmark-pathological-1-run.hir"
);
vector!(
    cmark_pathological_14,
    "SEM-3-markdown-cmark-pathological-14-run.hir"
);
vector!(
    cmark_pathological_16,
    "SEM-3-markdown-cmark-pathological-16-run.hir"
);
vector!(
    cmark_pathological_17,
    "SEM-3-markdown-cmark-pathological-17-run.hir"
);
vector!(
    cmark_pathological_18,
    "SEM-3-markdown-cmark-pathological-18-run.hir"
);
vector!(
    cmark_pathological_19,
    "SEM-3-markdown-cmark-pathological-19-run.hir"
);
vector!(
    cmark_pathological_20,
    "SEM-3-markdown-cmark-pathological-20-run.hir"
);
vector!(
    cmark_pathological_21,
    "SEM-3-markdown-cmark-pathological-21-run.hir"
);
vector!(
    cmark_pathological_6,
    "SEM-3-markdown-cmark-pathological-6-run.hir"
);
vector!(
    cmark_pathological_9,
    "SEM-3-markdown-cmark-pathological-9-run.hir"
);
vector!(
    cmark_regression_22,
    "SEM-3-markdown-cmark-regression-22-run.hir"
);
vector!(
    cmark_regression_8,
    "SEM-3-markdown-cmark-regression-8-run.hir"
);
vector!(
    js_pathological_12,
    "SEM-3-markdown-js-pathological-12-run.hir"
);
vector!(
    js_pathological_3,
    "SEM-3-markdown-js-pathological-3-run.hir"
);
vector!(
    js_pathological_4,
    "SEM-3-markdown-js-pathological-4-run.hir"
);
vector!(js_regression_25, "SEM-3-markdown-js-regression-25-run.hir");
