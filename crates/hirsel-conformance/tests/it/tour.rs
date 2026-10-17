//! The published page is an executable view of the ordinary conformance corpus.
use hirsel_conformance::{Compiler, Driver, Expect, Status, parse_header, spec_rules};
use std::{collections::BTreeSet, fs, path::Path};

const PAGE: &str = include_str!("../../../../docs/tour.md");
const START: &str = "<!-- tour: ";
const END: &str = "<!-- /tour -->";

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
fn published_programs_and_outputs_match_the_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for template in [
        include_bytes!("../../../../docs/tour/stdout.json").as_slice(),
        include_bytes!("../../../../docs/tour/inputs.json").as_slice(),
    ] {
        hirsel_run_config::RunConfig::bind_grants(
            template,
            [1; 32],
            "tour@1/main".into(),
            hirsel_run_config::LaunchKind::Main,
        )
        .unwrap();
    }
    let rules = spec_rules(&root).unwrap();
    let mut paths = BTreeSet::new();
    let snippets = snippets();
    assert!(!snippets.is_empty());
    assert_eq!(PAGE.matches("```hirsel\n").count(), snippets.len());
    for (path, rendered) in snippets {
        assert!(paths.insert(path), "duplicate tour program: {path}");
        let source = fs::read_to_string(root.join(path)).unwrap();
        let header = parse_header(Path::new(path), &source, &rules).unwrap();
        let body = source
            .lines()
            .position(|line| !line.starts_with("//"))
            .map(|i| source.split_inclusive('\n').skip(i).collect::<String>())
            .unwrap();
        let Expect::Run { stdout, exit } = header.expect else {
            panic!("tour program must have a checked run expectation: {path}")
        };
        let expected = if stdout.is_empty() && exit == 0 {
            "No stdout; exit 0 (all checks pass).".into()
        } else if stdout.is_empty() {
            format!("No stdout; exit {exit}.")
        } else {
            format!("```text\n{stdout}```")
        };
        assert_eq!(
            rendered,
            format!(
                "[Runnable source](../{path})\n\n```hirsel\n{}```\n\nExpected:\n\n{expected}\n",
                body.trim_start_matches('\n')
            ),
            "{path}: run python3 docs/tour/render.py"
        );
    }
    // An orphaned tour fixture must not silently lose its place on the page.
    let corpus = root.join("conformance");
    for dir in fs::read_dir(&corpus).unwrap() {
        let dir = dir.unwrap();
        if !dir.file_type().unwrap().is_dir() {
            continue;
        }
        for file in fs::read_dir(dir.path()).unwrap() {
            let file = file.unwrap().path();
            if file.extension().is_some_and(|ext| ext == "hir")
                && file
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains("-tour-")
            {
                let path = file.strip_prefix(&root).unwrap().to_str().unwrap();
                assert!(paths.contains(path), "unpublished tour program: {path}");
            }
        }
    }
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
        assert_eq!(outcome, hirsel_engine::RunOutcome::Exit(0));
        assert_eq!(stdout, b"agent\nhello\none\ntwo");
        assert!(stderr.is_empty());
    });
}

// Full runs must retain every published native-capable example even if the
// general campaign's coverage minimizer finds its features in another program.
#[test]
#[ignore = "full gate"]
fn full_gate_tour_programs_agree_natively() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_conformance::native::{Case, Class};
        use hirsel_driver::native::{Toolchain, smoke_matrix};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tour-native");
        fs::create_dir_all(&dir).unwrap();
        let matrix = smoke_matrix();
        let tools = Toolchain::for_matrix(&dir.join("runtime"), &matrix).unwrap();
        for (path, _) in snippets() {
            // Native structured tasks are not implemented. This is stated in
            // the page; the ordinary and checked interpreter test still runs it.
            if path == "conformance/CONC/CONC-2-tour-11-concurrency-run.hir" {
                continue;
            }
            let full = root.join(path);
            let case = Case {
                id: path.into(),
                source: fs::read_to_string(&full).unwrap(),
                path: Some(full),
            };
            let case_dir = dir.join(Path::new(path).file_stem().unwrap());
            fs::create_dir_all(&case_dir).unwrap();
            let result = case.run(&matrix, &tools, &case_dir);
            assert_eq!(result.outcomes.len(), matrix.len(), "{result:?}");
            assert!(result.passes(Class::Heap), "{result:?}");
            assert!(result.passes(Class::Llvm), "{result:?}");
        }
    });
}
