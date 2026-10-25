//! The published page is an executable view of the ordinary conformance corpus.
use hirsel_conformance::{Compiler, Driver, Expect, Status, parse_header, spec_rules};
use std::{collections::BTreeSet, fs, path::Path};

const PAGE: &str = include_str!("../../../../docs/tour.md");
const START: &str = "<!-- tour: ";
const END: &str = "<!-- /tour -->";
// Only a program that calls C may be checked without running: the reference
// interpreter does not call C.
const ACCEPT: &str = "`hirsel check` accepts it; the reference interpreter does not call C, \
                      so run it with `hirsel run --native --grants '{\"io\":{\"stdout\":\"stdout.bytes@1\",\"dirs\":[]},\"unsafe\":true}'`.";

fn snippets() -> Vec<(&'static str, &'static str)> {
    PAGE.split(START)
        .skip(1)
        .map(|section| {
            let (path, rest) = section.split_once(" -->\n").unwrap();
            assert!(path.starts_with("conformance/") && path.ends_with(".hir"));
            let (rendered, _) = rest.split_once(END).unwrap();
            (path, rendered)
        })
        .collect()
}

#[test]
fn every_tour_program_runs_with_checked_execution() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rules = spec_rules(&root).unwrap();
    let mut failures = Vec::new();
    for (path, _) in snippets() {
        let full = root.join(path);
        let source = fs::read_to_string(&full).unwrap();
        let header = parse_header(Path::new(path), &source, &rules).unwrap();
        let (status, observation) = Compiler.evaluate(&full, &header.expect);
        if status != Status::Pass || observation.checked.is_some() {
            failures.push(format!("{path}: {status:?}; {observation:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn input_program_uses_supplied_snapshots_and_stream() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let outcome = hirsel_driver::run_path_with_simulation(
            &root.join("conformance/CAP/CAP-1-tour-10-inputs-run.hir"),
            hirsel_engine::SimulationConfig {
                args: vec!["agent".into()],
                env: [("TOUR_LABEL".into(), Some("hello".into()))].into(),
                stdin: b"one\ntwo".to_vec(),
                ..Default::default()
            },
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        assert_eq!(
            outcome,
            hirsel_engine::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        );
        assert_eq!(stdout, b"agent\nhello\none\ntwo");
        assert!(stderr.is_empty());
    });
}

// Full runs must retain every published native-capable example even if the
// general campaign's coverage minimizer finds its features in another program.
