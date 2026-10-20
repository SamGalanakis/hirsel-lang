//! Published SigV4 vectors execute more than the corpus's 1M source steps.
use hirsel_driver::run_path_with_step_budget;
use hirsel_engine::{RunOutcome, SimulationConfig, Verify, with_compiler_stack};
use std::path::PathBuf;

#[test]
fn published_aws_sigv4_vectors_execute_package_source() {
    with_compiler_stack(|| {
        for index in 0..6 {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../packages/s3@1/tests/aws-{index}.hir.json"));
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = run_path_with_step_budget(
                &path,
                SimulationConfig::default(),
                Some(5_000_000),
                Verify::On,
                &mut stdout,
                &mut stderr,
            )
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "AWS vector {index}: {outcome:?}; stdout={}; stderr={}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
    });
}
