//! Measurement harness: copy temporarily into the conformance crate's bin directory.
use hirsel_conformance::native::{self, Case};
use hirsel_driver::native::{Engine, Outcome, Toolchain, full_matrix};

fn main() {
    let id = std::env::args().nth(1).expect("case path");
    let mode = std::env::args().nth(2).unwrap_or_default();
    let matrix: Vec<_> = full_matrix()
        .into_iter()
        .filter(|c| {
            mode == "matrix"
                || (matches!(c.engine, Engine::Heap { campaign: true })
                    && c.options.target.triple.starts_with("x86_64"))
        })
        .map(|mut c| {
            if mode != "matrix" {
                c.engine = Engine::Heap {
                    campaign: mode == "campaign",
                };
            }
            c
        })
        .collect();
    assert_eq!(matrix.len(), if mode == "matrix" { 44 } else { 1 });
    let mut targets = Vec::new();
    if mode == "matrix" {
        for c in &matrix {
            if !targets
                .iter()
                .any(|t: &&hirsel_driver::native::TargetSpec| t.triple == c.options.target.triple)
            {
                targets.push(&c.options.target);
            }
        }
    }
    let toolchain =
        Toolchain::in_tree(&native::scratch("holder-runtime").unwrap().join("cache"), &targets).unwrap();
    let case = Case {
        source: std::fs::read_to_string(&id).unwrap(),
        id,
    };
    let started = std::time::Instant::now();
    let results = native::run_cases(
        &[case],
        &matrix,
        &toolchain,
        &native::scratch("holder-measure").unwrap(),
        1,
    );
    print!("{}", native::render_matrix(&results));
    eprintln!(
        "holder-measure seconds={:.6}",
        started.elapsed().as_secs_f64()
    );
    for result in &results {
        assert!(result.snapshot.is_none(), "{:?}", result.snapshot);
        assert_eq!(result.outcomes.len(), matrix.len());
        assert!(
            result.outcomes.iter().all(|(_, _, o)| *o == Outcome::Agree),
            "{:?}",
            result.outcomes
        );
    }
}
