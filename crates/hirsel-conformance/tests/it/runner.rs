use hirsel_cli_args::Shard;
use hirsel_conformance::{
    Compiler, Driver, Expect, Status, TestResult, classify, extract_rules, parse_header, ratchet,
    run_suite, run_suite_shard,
};
use hirsel_diag::{Diagnostic, RuleId, Severity};
use hirsel_engine::{CheckOutput, NotImplemented, RunOutcome};
use hirsel_span::{FileId, SourceMap, Span};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

fn rules() -> hirsel_conformance::Rules {
    extract_rules(include_str!("../fixtures/docs/spec/ownership.md")).unwrap()
}
fn header(source: &str) -> Result<hirsel_conformance::Header, String> {
    parse_header(Path::new("conformance/OWN/test.hir"), source, &rules())
}
fn diagnostic(rule: Option<&str>, line: usize, severity: Severity) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        RuleId::new(rule.unwrap_or("LANG-1")).unwrap(),
        Span {
            file: FileId(0),
            start: (line - 1) as u32,
            end: (line - 1) as u32,
        },
        "fixture",
    );
    diagnostic.severity = severity;
    diagnostic
}
struct Fake {
    diagnostics: Vec<Diagnostic>,
    outcome: Result<RunOutcome, NotImplemented>,
    stdout: &'static str,
}
impl Driver for Fake {
    fn check(&self, _: &Path) -> Result<CheckOutput, NotImplemented> {
        let mut sources = SourceMap::default();
        sources.add("fixture.hir", "\n".repeat(100)).unwrap();
        Ok(CheckOutput {
            completion: Default::default(),

            sources,
            diagnostics: self.diagnostics.clone(),
            unsafe_ffi: vec![],
        })
    }
    fn run(&self, _: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented> {
        stdout.write_all(self.stdout.as_bytes()).unwrap();
        self.outcome.clone()
    }
}
fn fake() -> Fake {
    Fake {
        diagnostics: vec![],
        outcome: Ok(RunOutcome::Exit(0)),
        stdout: "",
    }
}

#[test]
fn only_known_comment_names_are_header_directives() {
    assert!(header("// note: authoring example\n// rule: OWN-6\n// expect: accept\n// explanation: ordinary prose\nfn f() {}\n// note: trailing prose\n").is_ok());
    for source in [
        "// rule OWN-6\n// expect: accept\n",
        "// rule: OWN-6\n// expect: nope\n",
        "// rule: OWN-6\n// expect: accept\nfn f() {}\n// entry: f\n",
    ] {
        assert!(header(source).is_err(), "{source}");
    }
}

#[test]
fn accepts_warnings_but_not_errors() {
    let expect = header(include_str!("../fixtures/conformance/OWN/accept.hir"))
        .unwrap()
        .expect;
    let mut driver = fake();
    driver
        .diagnostics
        .push(diagnostic(Some("OWN-6"), 3, Severity::Warning));
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    driver
        .diagnostics
        .push(diagnostic(None, 3, Severity::Error));
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
}
#[test]
fn reject_matches_first_diagnostic_with_a_rule_and_exact_line() {
    let expect = header(include_str!("../fixtures/conformance/OWN/reject.hir"))
        .unwrap()
        .expect;
    let mut driver = fake();
    driver.diagnostics = vec![diagnostic(Some("OWN-6"), 3, Severity::Warning)];
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    driver.diagnostics = vec![diagnostic(Some("OWN-6"), 3, Severity::Error)];
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    driver
        .diagnostics
        .insert(0, diagnostic(Some("OWN-7"), 3, Severity::Warning));
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    driver
        .diagnostics
        .insert(0, diagnostic(Some("OWN-7"), 3, Severity::Error));
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    driver.diagnostics = vec![diagnostic(Some("OWN-6"), 2, Severity::Error)];
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
}
#[test]
fn run_matches_literal_stdout_and_exit() {
    let expect = header(include_str!("../fixtures/conformance/OWN/nested/run.hir"))
        .unwrap()
        .expect;
    assert_eq!(
        expect,
        Expect::Run {
            stdout: "hello\n space stays \n\n".into(),
            exit: 7
        }
    );
    let mut driver = fake();
    driver.stdout = "hello\n space stays \n\n";
    driver.outcome = Ok(RunOutcome::Exit(7));
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    driver.outcome = Ok(RunOutcome::Exit(0));
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    driver.outcome = Ok(RunOutcome::Exit(7));
    driver.stdout = "hello";
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    assert_eq!(
        header("// rule: OWN-6\n// expect: run\nfn main() {}\n")
            .unwrap()
            .expect,
        Expect::Run {
            stdout: "".into(),
            exit: 0
        }
    );
}
#[test]
fn fail_checks_before_running_and_matches_rule_and_line() {
    let expect = header(include_str!("../fixtures/conformance/OWN/fail.hir"))
        .unwrap()
        .expect;
    let mut driver = fake();
    driver.outcome = Ok(RunOutcome::Failure {
        operation: None,
        rule: const { RuleId::of("OWN-7") },
        site: hirsel_engine::FailureSite {
            line: 3,
            ..Default::default()
        },
    });
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    driver.outcome = Ok(RunOutcome::Failure {
        operation: None,
        rule: const { RuleId::of("OWN-6") },
        site: hirsel_engine::FailureSite {
            line: 3,
            ..Default::default()
        },
    });
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    driver.outcome = Ok(RunOutcome::Failure {
        operation: None,
        rule: const { RuleId::of("OWN-7") },
        site: hirsel_engine::FailureSite {
            line: 2,
            ..Default::default()
        },
    });
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    driver
        .diagnostics
        .push(diagnostic(Some("OWN-6"), 3, Severity::Error));
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
}
#[test]
fn unimplemented_check_or_run_is_classified() {
    for text in [
        include_str!("../fixtures/conformance/OWN/accept.hir"),
        include_str!("../fixtures/conformance/OWN/reject.hir"),
        include_str!("../fixtures/conformance/OWN/nested/run.hir"),
        include_str!("../fixtures/conformance/OWN/fail.hir"),
    ] {
        assert_eq!(
            classify(&Compiler, Path::new("test"), &header(text).unwrap().expect),
            Status::NotImplemented
        );
    }
    let mut driver = fake();
    driver.outcome = Err(NotImplemented);
    assert_eq!(
        classify(
            &driver,
            Path::new("test"),
            &Expect::Run {
                stdout: "".into(),
                exit: 0
            }
        ),
        Status::NotImplemented
    );
}
#[test]
fn malformed_headers_name_the_file_and_problem() {
    for (source, message) in [
        ("// rule: OWN-6\n", "missing expect"),
        ("// expect: accept\n", "missing rule"),
        (
            "// rule: OWN-6\n// expect: accept\n// expect: run\n",
            "duplicate expect",
        ),
        (
            "// rule: OWN-6\n// rule: OWN-7\n// expect: accept\n",
            "duplicate rule",
        ),
        (
            "// rule: OWN-6\n// expect reject OWN-6 at line 3\nfn f() {}\n",
            "missing ':'",
        ),
        (
            "// rule: OWN-6\n// expect: rejected OWN-6 at line 3\nfn f() {}\n",
            "malformed expect",
        ),
        (
            "// rule: OWN-6\n// expect: reject OWN-6 at line 0\n",
            "outside file",
        ),
        (
            "// rule: OWN-6\n// expect: reject OWN-6 at line 99\n",
            "outside file",
        ),
        (
            "// rule: OWN-6\n// expect: accept\n// stdout: nope\n",
            "must follow expect: run",
        ),
        ("// rule: OWN-6\n// expect: run\n// exit: x\n", "integer"),
        (
            "// rule: OWN-6\n// expect: run\n// exit: 0\n// exit: 0\n",
            "duplicate exit",
        ),
        ("// rules: OWN-6\n// expect: accept\n", "missing rule"),
        (
            "// rule: OWN-6\nfn f() {}\n// expect: accept\n",
            "after source",
        ),
    ] {
        let error = header(source).unwrap_err();
        assert!(
            error.contains("conformance/OWN/test.hir") && error.contains(message),
            "{error}"
        );
    }
}
#[test]
fn unknown_ids_are_checked_in_both_directives_and_directory_is_checked() {
    for source in [
        "// rule: OWN-99\n// expect: accept\n",
        "// rule: OWN-6\n// expect: reject OWN-99 at line 2\n",
        "// rule: OWN-6\n// expect: fail OWN-99 at line 2\n",
    ] {
        assert!(
            header(source)
                .unwrap_err()
                .contains("unknown spec rule ID OWN-99")
        );
    }
    assert!(
        parse_header(
            Path::new("conformance/MEM/test.hir"),
            "// rule: OWN-6\n// expect: accept\n",
            &rules()
        )
        .unwrap_err()
        .contains("conformance/OWN/")
    );
    assert!(header("// rule: OWN-6\n// expect: accept\n// spec: #12\nfn f() {}\n").is_ok());
    assert!(extract_rules("no rule IDs").is_err());
    assert!(extract_rules("### bad: Title\n").is_err());
    assert!(extract_rules("### OWN-6: Copying\n### OWN-6: Again\n").is_err());
    assert!(extract_rules("```text\n### OWN-6: Copying\n```\n").is_err());
}
fn result(path: &str, status: Status) -> TestResult {
    TestResult {
        path: path.into(),
        prefix: "OWN".into(),
        status,
    }
}
#[test]
fn ratchet_adds_passes_and_never_drops_regressions_or_missing_paths() {
    let a = "conformance/OWN/a.hir";
    let b = "conformance/OWN/b.hir";
    let tests = [
        result(b, Status::Pass),
        result(a, Status::Pass),
        result("conformance/OWN/c.hir", Status::NotImplemented),
    ];
    assert_eq!(ratchet("", &tests, true).unwrap(), format!("{a}\n{b}\n"));
    assert_eq!(
        ratchet(&format!("{a}\n"), &tests, false).unwrap(),
        format!("{a}\n")
    );
    for status in [Status::NotImplemented, Status::Fail("regression".into())] {
        assert!(
            ratchet(&format!("{a}\n"), &[result(a, status)], true).unwrap_err()[0]
                .contains("ratchet requires pass")
        );
    }
    assert!(ratchet(&format!("{a}\n"), &[], true).unwrap_err()[0].contains("missing"));
    for malformed in [
        format!("{b}\n{a}\n"),
        format!("{a}\n{a}\n"),
        "../escape.hir\n".into(),
    ] {
        assert!(ratchet(&malformed, &tests, true).is_err());
    }
}

#[test]
fn shards_partition_the_corpus_and_ratchet_only_their_own_paths() {
    assert_eq!(Shard::parse("2/3"), Some(Shard { index: 2, count: 3 }));
    for invalid in ["0/3", "4/3", "1/0", "3", "a/b"] {
        assert_eq!(Shard::parse(invalid), None, "{invalid}");
    }
    let root = Fixture::new();
    let whole = run_suite(&root.0, &fake(), false).unwrap();
    for count in [1, 2, 3, 5] {
        let shards: Vec<_> = (1..=count)
            .map(|index| {
                run_suite_shard(&root.0, &fake(), false, Some(Shard { index, count })).unwrap()
            })
            .collect();
        let mut paths: Vec<_> = shards
            .iter()
            .flat_map(|r| r.tests.iter().map(|t| t.path.clone()))
            .collect();
        paths.sort();
        let mut expected: Vec<_> = whole.tests.iter().map(|t| t.path.clone()).collect();
        expected.sort();
        assert_eq!(paths, expected, "{count} shards");
        for field in [
            |c: &hirsel_conformance::Counts| c.validated,
            |c: &hirsel_conformance::Counts| c.passing,
            |c: &hirsel_conformance::Counts| c.failing,
            |c: &hirsel_conformance::Counts| c.not_implemented,
        ] {
            assert_eq!(
                shards.iter().map(|r| field(&r.counts)).sum::<usize>(),
                field(&whole.counts)
            );
            assert_eq!(
                shards.iter().map(|r| field(&r.in_gate)).sum::<usize>(),
                field(&whole.in_gate)
            );
        }
    }
    // A listed path that vanished fails exactly the shard that owns it.
    fs::write(
        root.0.join("conformance/passing.txt"),
        "conformance/OWN/accept.hir\n",
    )
    .unwrap();
    fs::remove_file(root.0.join("conformance/OWN/accept.hir")).unwrap();
    let missing = |r: &hirsel_conformance::Report| {
        r.errors.iter().any(|e| e.contains("ratchet path missing"))
    };
    let owners: Vec<_> = (1..=3)
        .filter(|&index| {
            missing(
                &run_suite_shard(&root.0, &fake(), false, Some(Shard { index, count: 3 })).unwrap(),
            )
        })
        .collect();
    assert_eq!(owners.len(), 1);
    assert!(
        Shard {
            index: owners[0],
            count: 3
        }
        .holds("conformance/OWN/accept.hir")
    );
    assert!(run_suite_shard(&root.0, &fake(), true, Some(Shard { index: 1, count: 3 })).is_err());
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hirsel-conformance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("docs/spec")).unwrap();
        fs::create_dir_all(root.join("conformance/OWN/nested")).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for file in [
            "docs/spec/ownership.md",
            "conformance/passing.txt",
            "conformance/OWN/accept.hir",
            "conformance/OWN/reject.hir",
            "conformance/OWN/fail.hir",
            "conformance/OWN/nested/run.hir",
        ] {
            // A fresh file: the tests edit the copies, whatever the fixtures' modes.
            fs::write(root.join(file), fs::read(fixtures.join(file)).unwrap()).unwrap();
        }
        fs::write(root.join("conformance/m2-runtime.txt"), "").unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn suite_discovers_nested_files_and_preserves_ratchet_on_validation_errors() {
    let root = Fixture::new();
    let report = run_suite(&root.0, &Compiler, false).unwrap();
    assert!(report.success());
    assert_eq!(report.counts.validated, 4);
    assert_eq!(report.counts.not_implemented, 1);
    assert_eq!(report.counts.passing, 1);
    assert_eq!(report.counts.failing, 2);
    let report = run_suite(&root.0, &fake(), true).unwrap();
    assert!(report.success());
    assert_eq!(report.counts.passing, 1);
    let passing = root.0.join("conformance/passing.txt");
    assert_eq!(
        fs::read_to_string(&passing).unwrap(),
        "conformance/OWN/accept.hir\n"
    );
    fs::write(
        root.0.join("conformance/OWN/invalid.hir"),
        "// rule: OWN-99\n// expect: accept\n",
    )
    .unwrap();
    assert!(!run_suite(&root.0, &fake(), true).unwrap().success());
    assert_eq!(
        fs::read_to_string(&passing).unwrap(),
        "conformance/OWN/accept.hir\n"
    );
    fs::remove_file(root.0.join("conformance/OWN/accept.hir")).unwrap();
    assert!(
        run_suite(&root.0, &Compiler, false)
            .unwrap()
            .errors
            .iter()
            .any(|e| e.contains("ratchet path missing"))
    );
}
#[test]
fn binary_json_and_exit_status_follow_validation_and_ratchet() {
    let root = Fixture::new();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_hirsel-conformance"))
            .args(["--json", "--root"])
            .arg(&root.0)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["validated"], 4);
    assert_eq!(json["summary"]["not_implemented"], 1);
    assert_eq!(json["in_gate"]["validated"], 4);
    assert_eq!(json["unchecked"], 0);
    assert!(
        json["tests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["unchecked"] == 0)
    );
    fs::write(
        root.0.join("conformance/passing.txt"),
        "conformance/OWN/reject.hir\n",
    )
    .unwrap();
    let output = run();
    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["success"], false);
    assert!(
        json["errors"][0]
            .as_str()
            .unwrap()
            .contains("ratchet requires pass")
    );
    fs::write(
        root.0.join("conformance/OWN/invalid.hir"),
        "// rule: OWN-99\n// expect: accept\n",
    )
    .unwrap();
    let output = run();
    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["errors"].as_array().unwrap().iter().any(|e| {
        e.as_str()
            .unwrap()
            .contains("invalid.hir: unknown spec rule")
    }));
}

struct Incomplete(Fake);
impl Driver for Incomplete {
    fn complete(&self) -> bool {
        false
    }
    fn check(&self, path: &Path) -> Result<CheckOutput, NotImplemented> {
        self.0.check(path)
    }
    fn run(&self, path: &Path, stdout: &mut dyn Write) -> Result<RunOutcome, NotImplemented> {
        self.0.run(path, stdout)
    }
}
#[test]
fn incomplete_pipeline_keeps_accept_run_fail_unavailable_and_checks_reject_exactly() {
    let mut driver = Incomplete(fake());
    driver
        .0
        .diagnostics
        .push(diagnostic(Some("LANG-1"), 3, Severity::Error));
    for expect in [
        Expect::Accept,
        Expect::Run {
            stdout: "".into(),
            exit: 0,
        },
        Expect::Fail {
            rule: const { RuleId::of("FAIL-1") },
            line: 3,
            stdout: None,
        },
    ] {
        assert_eq!(
            classify(&driver, Path::new("test"), &expect),
            Status::NotImplemented
        );
    }
    let matching = Expect::Reject {
        rule: const { RuleId::of("LANG-1") },
        line: 3,
    };
    assert_eq!(
        classify(&driver, Path::new("test"), &matching),
        Status::Pass
    );
    let wrong = Expect::Reject {
        rule: const { RuleId::of("LANG-7") },
        line: 3,
    };
    assert!(matches!(
        classify(&driver, Path::new("test"), &wrong),
        Status::Fail(_)
    ));
}

#[test]
fn gate_exclusions_are_offline_union_and_never_bypass_the_ratchet() {
    let root = Fixture::new();
    fs::write(
        root.0.join("conformance/m2-runtime.txt"),
        "conformance/OWN/accept.hir\tM2\n",
    )
    .unwrap();
    fs::write(
        root.0.join("conformance/M2-RUNTIME.md"),
        "`conformance/OWN/reject.hir`",
    )
    .unwrap();
    let accept = root.0.join("conformance/OWN/accept.hir");
    let source = fs::read_to_string(&accept).unwrap();
    // Open issues overlap M2; the closed #5 marker must add no exclusion.
    fs::write(
        &accept,
        format!("// spec: #4\n// spec: #7\n// spec: #5\n{source}"),
    )
    .unwrap();
    let report = run_suite(&root.0, &fake(), true).unwrap();
    assert_eq!(report.counts.validated, 4);
    assert_eq!(report.in_gate.validated, 3);
    assert_eq!(report.exclusions["conformance/OWN/accept.hir"].len(), 3);
    assert!(
        !report.exclusions["conformance/OWN/accept.hir"]
            .iter()
            .any(|reason| reason == "open spec issue #5")
    );
    let json = report.json();
    let case = json["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["path"] == "conformance/OWN/accept.hir")
        .unwrap();
    assert_eq!(json["unchecked"], 0);
    assert_eq!(case["unchecked"], 0);
    assert_eq!(case["in_gate"], false);
    assert_eq!(case["exclusions"].as_array().unwrap().len(), 3);
    let mut broken = fake();
    broken
        .diagnostics
        .push(diagnostic(Some("OWN-6"), 3, Severity::Error));
    assert!(
        run_suite(&root.0, &broken, true)
            .unwrap()
            .errors
            .iter()
            .any(|e| e.contains("ratchet requires pass"))
    );
}

#[test]
fn fail_stdout_is_optional_and_exact_when_present() {
    let source = "// rule: OWN-6\n// expect: fail OWN-6 at line 6\n// stdout: before\n// stdout: dropped\nfn main() {\n  fail()\n}\n";
    let expect = header(source).unwrap().expect;
    let mut driver = fake();
    driver.outcome = Ok(RunOutcome::Failure {
        operation: None,
        rule: const { RuleId::of("OWN-6") },
        site: hirsel_engine::FailureSite {
            line: 6,
            ..Default::default()
        },
    });
    driver.stdout = "before\ndropped\n";
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    for output in ["before\n", "dropped\nbefore\n", "before\ndropped\nextra\n"] {
        driver.stdout = output;
        assert!(matches!(
            classify(&driver, Path::new("test"), &expect),
            Status::Fail(_)
        ));
    }
    let unchecked = header("// rule: OWN-6\n// expect: fail OWN-6 at line 3\nfail()\n")
        .unwrap()
        .expect;
    assert!(matches!(&unchecked, Expect::Fail { stdout: None, .. }));
    driver.outcome = Ok(RunOutcome::Failure {
        operation: None,
        rule: const { RuleId::of("OWN-6") },
        site: hirsel_engine::FailureSite {
            line: 3,
            ..Default::default()
        },
    });
    assert_eq!(
        classify(&driver, Path::new("test"), &unchecked),
        Status::Pass
    );
    assert!(
        header("// rule: OWN-6\n// expect: fail OWN-6 at line 4\n// exit: 1\nfail()\n").is_err()
    );
}

#[test]
fn runtime_failure_header_can_name_a_dependency_line_beyond_the_fixture() {
    let body = "fn f() {}\n";
    let runtime = format!("// rule: OWN-6\n// expect: fail OWN-6 at line 99\n{body}");
    let expect = header(&runtime).unwrap().expect;
    let mut driver = fake();
    driver.outcome = Ok(RunOutcome::Failure {
        operation: None,
        rule: const { RuleId::of("OWN-6") },
        site: hirsel_engine::FailureSite {
            line: 99,
            ..Default::default()
        },
    });
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    driver.outcome = Ok(RunOutcome::Failure {
        operation: None,
        rule: const { RuleId::of("OWN-6") },
        site: hirsel_engine::FailureSite {
            line: 98,
            ..Default::default()
        },
    });
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    let static_twin = runtime.replace("expect: fail", "expect: reject");
    assert!(header(&static_twin).unwrap_err().contains("outside file"));
    let zero = runtime.replace("line 99", "line 0");
    assert!(header(&zero).unwrap_err().contains("outside file"));
}

#[test]
fn blocked_header_requires_noncompletion_and_the_exact_effect_prefix() {
    let expect = header("// rule: OWN-6\n// expect: blocked\n// stdout: waiting\nfn main() {}\n")
        .unwrap()
        .expect;
    assert_eq!(
        expect,
        Expect::Blocked {
            stdout: "waiting\n".into()
        }
    );
    let mut driver = fake();
    driver.stdout = "waiting\n";
    driver.outcome = Ok(RunOutcome::Blocked(vec![]));
    assert_eq!(classify(&driver, Path::new("test"), &expect), Status::Pass);
    for output in ["", "waiting\ndestroyed\n"] {
        driver.stdout = output;
        assert!(matches!(
            classify(&driver, Path::new("test"), &expect),
            Status::Fail(_)
        ));
    }
    driver.stdout = "waiting\n";
    driver.outcome = Ok(RunOutcome::Exit(0));
    assert!(matches!(
        classify(&driver, Path::new("test"), &expect),
        Status::Fail(_)
    ));
    assert!(header("// rule: OWN-6\n// expect: blocked\n// exit: 0\nfn main() {}\n").is_err());
}

#[test]
fn path_selection_runs_only_matches_and_keeps_the_selected_ratchet() {
    use hirsel_conformance::{Selection, run_suite_selected};
    let root = Fixture::new();
    let paths = ["conformance/OWN/accept.hir".to_owned()];
    let selection = Selection {
        shard: None,
        paths: &paths,
    };
    fs::write(
        root.0.join("conformance/passing.txt"),
        "conformance/OWN/accept.hir\nconformance/OWN/reject.hir\n",
    )
    .unwrap();
    let report = run_suite_selected(&root.0, &fake(), false, selection).unwrap();
    assert!(report.success(), "{:?}", report.errors);
    assert_eq!(report.counts.validated, 1);
    assert_eq!(report.tests[0].path, paths[0]);
    assert!(run_suite_selected(&root.0, &fake(), true, selection).is_err());
    let empty = ["conformance/absent".to_owned()];
    assert!(
        run_suite_selected(
            &root.0,
            &fake(),
            false,
            Selection {
                shard: None,
                paths: &empty
            }
        )
        .unwrap_err()
        .contains("selected no corpus files")
    );
    let mut failing = fake();
    failing
        .diagnostics
        .push(diagnostic(Some("OWN-6"), 3, Severity::Error));
    let report = run_suite_selected(&root.0, &failing, false, selection).unwrap();
    assert_eq!(report.counts.failing, 1);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("ratchet requires pass"))
    );
    // Formatting shares the filter: an unrelated malformed file cannot enter it.
    let mut report = hirsel_conformance::Report::default();
    hirsel_conformance::corpus_format::check_canonical_selected(&root.0, &mut report, selection)
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    fs::write(
        root.0.join("conformance/OWN/reject.hir"),
        "malformed unrelated source",
    )
    .unwrap();
    fs::write(
        root.0.join(&paths[0]),
        "// rule: OWN-6\n// expect: accept\nfn accepted()   {}\n",
    )
    .unwrap();
    let mut report = hirsel_conformance::Report::default();
    hirsel_conformance::corpus_format::check_canonical_selected(&root.0, &mut report, selection)
        .unwrap();
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(report.errors[0].contains("accept.hir: corpus source is not formatter-canonical"));
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel-conformance"))
        .args([
            "--root",
            root.0.to_str().unwrap(),
            "--path",
            &paths[0],
            "--check-format",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 validated / 1 passing"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not formatter-canonical"));
}

#[test]
fn path_selection_requires_unratcheted_expected_outcomes() {
    use hirsel_conformance::{Selection, run_suite_selected};
    let root = Fixture::new();
    fs::write(root.0.join("conformance/passing.txt"), "").unwrap();
    let paths = ["conformance/OWN/accept.hir".to_owned()];
    let selection = Selection {
        shard: None,
        paths: &paths,
    };
    let mut failing = fake();
    failing
        .diagnostics
        .push(diagnostic(Some("OWN-6"), 3, Severity::Error));
    let report = run_suite_selected(&root.0, &failing, false, selection).unwrap();
    assert_eq!(report.counts.failing, 1);
    assert!(
        !report.success(),
        "unlisted selected failures must fail the run"
    );
    let passed = run_suite_selected(&root.0, &fake(), false, selection).unwrap();
    assert!(passed.success());
    for (source, passes) in [
        (
            "// rule: OWN-6\n// expect: accept\nfn selected() { let x: bool = 1u64 }\n",
            false,
        ),
        (
            "// rule: OWN-6\n// expect: accept\nfn selected() { let x: bool = true }\n",
            true,
        ),
    ] {
        fs::write(root.0.join(&paths[0]), source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel-conformance"))
            .args(["--root", root.0.to_str().unwrap(), "--path", &paths[0]])
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            passes,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 validated"));
    }
}

#[test]
fn fixture_headers_are_explicit_validated_and_header_only() {
    let accepted = "// rule: OWN-6\n// expect: run\n// grant: clock\n// net: {\"provider\":\"net.script@2\",\"rules\":[],\"peers\":[]}\n// process: {\"provider\":\"process.script@1\",\"rules\":[],\"runs\":[]}\npub fn main() {}\n";
    assert!(header(accepted).is_ok());
    let fixture = hirsel_conformance::fixture(accepted).unwrap();
    assert_eq!(fixture.monotonic, Some(0));
    assert!(fixture.net.is_some());
    assert!(fixture.process.is_some());
    assert!(
        hirsel_conformance::fixture("// ordinary comment\npub fn main() {}\n")
            .unwrap()
            .monotonic
            .is_none()
    );
    for source in [
        accepted.replace("expect: run", "expect: test"),
        accepted.replace("grant: clock", "grant: host"),
        accepted.replace("grant: clock", "grant clock"),
        accepted.replace("// grant: clock", "// grant: clock\n// grant: clock"),
        accepted.replace("pub fn main() {}", "pub fn main() {}\n// grant: clock"),
        accepted.replace("net.script@2", "net.host@1"),
        accepted.replace("process.script@1", "process.spawn@1"),
        accepted.replace("\"rules\":[],\"peers\":[]", "\"rules\":[]"),
        accepted.replace("\"runs\":[]", "\"runs\":[],\"unknown\":0"),
        accepted.replace("// process:", "// process: {}\n// process:"),
        accepted.replace("// net:", "// net: {}\n// net:"),
    ] {
        assert!(header(&source).is_err(), "{source}");
    }
}
