//! Task D's `rc-trace v1` protocol vectors, run on the heap executor's own
//! ledger: each must print exactly its checked `.expected` output, the output
//! `lake exe rc-trace` produces from the proved Lean machine.
use std::path::PathBuf;

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../native-tests/model")
}

#[test]
fn every_model_vector_prints_its_checked_expected_output() {
    let mut names: Vec<PathBuf> = std::fs::read_dir(vectors())
        .expect("native-tests/model exists")
        .map(|e| e.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "trace"))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no vectors in native-tests/model");
    let mut failures = Vec::new();
    for trace in &names {
        let text = std::fs::read_to_string(trace).expect("vector reads");
        let expected =
            std::fs::read_to_string(trace.with_extension("expected")).expect("expected output");
        let reference = hirsel_heapcheck::rc_trace_with_config(
            &text,
            &hirsel_heapcheck::Config {
                full_recount_every_step: true,
                ..hirsel_heapcheck::Config::default()
            },
        );
        let incremental = hirsel_heapcheck::rc_trace_with_config(
            &text,
            &hirsel_heapcheck::Config {
                full_recount_every_step: false,
                ..hirsel_heapcheck::Config::default()
            },
        );
        assert_eq!(incremental, reference, "{}: audit modes", trace.display());
        match incremental {
            Ok(actual) if actual == expected => {}
            Ok(actual) => failures.push(format!(
                "{}:\n--- expected\n{expected}--- heapcheck\n{actual}",
                trace.display()
            )),
            Err(e) => failures.push(format!("{}: {e}", trace.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} vectors disagree:\n{}",
        failures.len(),
        names.len(),
        failures.join("\n")
    );
}

#[test]
fn an_unknown_version_is_rejected() {
    let e = hirsel_heapcheck::rc_trace("rc-trace v2\nmax 4\nalloc r1 0\n").unwrap_err();
    assert!(e.contains("rc-trace v1"), "{e}");
}
