use hirsel_cli_args::Shard;
use hirsel_conformance::{
    Actual, Compiler, Driver, End, Expect, Observation, Status, TestResult, extract_rules,
    parse_header, run_suite, run_suite_shard,
};
use hirsel_diag::RuleId;
use std::{
    fs,
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

#[test]
fn corpus_cases_cannot_select_a_compiled_only_native_oracle() {
    let ordinary = "// rule: OWN-6\n// expect: run\npub fn main() {}\n";
    header(ordinary).unwrap();
    for directive in [
        "// oracle: expected-stdout\n",
        "// oracle-reason: too large\n",
    ] {
        let source = format!("{directive}{ordinary}");
        assert!(
            header(&source)
                .unwrap_err()
                .contains("native-tests/library")
        );
    }
}
/// Every file passes except those whose path ends with a listed name.
struct Statuses(Vec<(&'static str, Status)>);
impl Driver for Statuses {
    fn evaluate(&self, path: &Path, _: &Expect) -> (Status, Observation) {
        let status = self
            .0
            .iter()
            .find(|(name, _)| path.ends_with(name))
            .map_or(Status::Pass, |(_, status)| status.clone());
        (status, Observation::default())
    }
}
fn passing() -> Statuses {
    Statuses(vec![])
}
fn failing(name: &'static str) -> Statuses {
    Statuses(vec![(name, Status::Fail("fixture".into()))])
}
fn rule(id: &str) -> RuleId {
    RuleId::new(id).unwrap()
}
fn ran(end: End, stdout: &str) -> Actual {
    Actual::Ran {
        end,
        stdout: stdout.into(),
    }
}
fn failure(id: &str, line: usize) -> End {
    End::Failure {
        rule: rule(id),
        line,
    }
}

#[test]
fn reject_verdict_needs_the_first_errors_rule_and_exact_line() {
    let expect = header(include_str!("../fixtures/conformance/OWN/OWN-6-reject.hir"))
        .unwrap()
        .expect;
    let at = |id: &str, line| Actual::Rejected {
        rule: rule(id),
        line,
    };
    assert!(expect.matches(&at("OWN-6", 4)));
    for actual in [
        at("OWN-6", 3),
        at("OWN-7", 4),
        Actual::Accepted,
        ran(End::Exit(0), ""),
    ] {
        assert!(!expect.matches(&actual), "{actual:?}");
    }
    let accept = header(include_str!("../fixtures/conformance/OWN/OWN-6-accept.hir"))
        .unwrap()
        .expect;
    assert!(accept.matches(&Actual::Accepted));
    assert!(!accept.matches(&at("OWN-6", 3)));
    assert!(!Expect::Test.matches(&Actual::Accepted));
}
#[test]
fn run_verdict_needs_literal_stdout_and_exit() {
    let expect = header(include_str!(
        "../fixtures/conformance/OWN/nested/OWN-6-run.hir"
    ))
    .unwrap()
    .expect;
    assert_eq!(
        expect,
        Expect::Run {
            stdout: "hello\n space stays \n\n".into(),
            exit: 7
        }
    );
    assert!(expect.matches(&ran(End::Exit(7), "hello\n space stays \n\n")));
    for actual in [
        ran(End::Exit(0), "hello\n space stays \n\n"),
        ran(End::Exit(7), "hello"),
        ran(failure("OWN-6", 3), "hello\n space stays \n\n"),
        ran(End::Blocked, "hello\n space stays \n\n"),
        ran(End::Other, "hello\n space stays \n\n"),
        Actual::Accepted,
    ] {
        assert!(!expect.matches(&actual), "{actual:?}");
    }
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
fn fail_verdict_matches_rule_line_and_stdout_only_when_present() {
    let expect = header(include_str!("../fixtures/conformance/OWN/OWN-6-fail.hir"))
        .unwrap()
        .expect;
    assert!(expect.matches(&ran(failure("OWN-7", 3), "unchecked output")));
    for actual in [
        ran(failure("OWN-6", 3), ""),
        ran(failure("OWN-7", 2), ""),
        ran(End::Exit(0), ""),
        Actual::Rejected {
            rule: rule("OWN-7"),
            line: 3,
        },
    ] {
        assert!(!expect.matches(&actual), "{actual:?}");
    }
    let source = "// rule: OWN-6\n// expect: fail OWN-6 at line 6\n// stdout: before\n// stdout: dropped\nfn main() {\n  fail()\n}\n";
    let expect = header(source).unwrap().expect;
    assert!(expect.matches(&ran(failure("OWN-6", 6), "before\ndropped\n")));
    for output in ["before\n", "dropped\nbefore\n", "before\ndropped\nextra\n"] {
        assert!(
            !expect.matches(&ran(failure("OWN-6", 6), output)),
            "{output}"
        );
    }
    let unchecked = header("// rule: OWN-6\n// expect: fail OWN-6 at line 3\nfail()\n")
        .unwrap()
        .expect;
    assert!(matches!(&unchecked, Expect::Fail { stdout: None, .. }));
    assert!(
        header("// rule: OWN-6\n// expect: fail OWN-6 at line 4\n// exit: 1\nfail()\n").is_err()
    );
}

#[test]
fn runtime_failure_header_can_name_a_dependency_line_beyond_the_fixture() {
    let body = "fn f() {}\n";
    let runtime = format!("// rule: OWN-6\n// expect: fail OWN-6 at line 99\n{body}");
    let expect = header(&runtime).unwrap().expect;
    assert!(expect.matches(&ran(failure("OWN-6", 99), "")));
    assert!(!expect.matches(&ran(failure("OWN-6", 98), "")));
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
    assert!(expect.matches(&ran(End::Blocked, "waiting\n")));
    for actual in [
        ran(End::Blocked, ""),
        ran(End::Blocked, "waiting\ndestroyed\n"),
        ran(End::Exit(0), "waiting\n"),
    ] {
        assert!(!expect.matches(&actual), "{actual:?}");
    }
    assert!(header("// rule: OWN-6\n// expect: blocked\n// exit: 0\nfn main() {}\n").is_err());
}

/// One real program, judged by the production runner against each header.
fn production(source: &str, cases: &[(Expect, bool)]) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "hirsel-conformance-production-{}-{}.hir",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, source).unwrap();
    for (expect, passes) in cases {
        let (status, observation) = Compiler.evaluate(&path, expect);
        assert_eq!(
            status == Status::Pass,
            *passes,
            "{expect:?}: {status:?}, observed {}",
            observation.actual
        );
        assert!(*passes || matches!(status, Status::Fail(_)), "{status:?}");
    }
    fs::remove_file(path).unwrap();
}
fn reject(id: &str, line: usize) -> Expect {
    Expect::Reject {
        rule: rule(id),
        line,
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
        (
            "// rule: OWN-6\n// expect: reject OWN-6 at line 3\n// twin: OWN/../OWN-6-ok.hir\n",
            "twin must name",
        ),
        (
            "// rule: OWN-6\n// expect: reject OWN-6 at line 3\n// twin: OWN/a-ok.hir\n// twin: OWN/b-ok.hir\n",
            "duplicate twin",
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
fn pending(root: &Fixture, text: &str) {
    fs::write(root.0.join("conformance/pending.txt"), text).unwrap();
}
fn errors_with(report: &hirsel_conformance::Report, text: &str) -> Vec<String> {
    report
        .errors
        .iter()
        .filter(|e| e.contains(text))
        .cloned()
        .collect()
}
#[test]
fn an_unlisted_failing_file_fails_its_shard() {
    let root = Fixture::new();
    let path = "conformance/OWN/OWN-6-reject.hir";
    for status in [Status::NotImplemented, Status::Fail("new".into())] {
        let driver = Statuses(vec![("OWN-6-reject.hir", status.clone())]);
        let owners: Vec<_> = (1..=3)
            .map(|index| {
                run_suite_shard(&root.0, &driver, Some(Shard { index, count: 3 })).unwrap()
            })
            .filter(|report| !report.success())
            .collect();
        assert_eq!(owners.len(), 1);
        assert_eq!(
            owners[0].errors,
            [format!("{path}: an in-gate file must pass, got {status:?}")]
        );
    }
}
#[test]
fn a_pending_failing_file_is_reported_with_its_reason_and_does_not_fail() {
    let root = Fixture::new();
    let path = "conformance/OWN/OWN-6-reject.hir";
    pending(&root, &format!("{path} # needs the M2 runtime\n"));
    let report = run_suite(&root.0, &failing("OWN-6-reject.hir")).unwrap();
    assert!(report.success(), "{:?}", report.errors);
    assert_eq!(report.counts.failing, 1);
    assert_eq!(report.in_gate.failing, 0);
    assert_eq!(report.exclusions[path], ["pending: needs the M2 runtime"]);
    let json = report.json();
    let case = json["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["path"] == path)
        .unwrap();
    assert_eq!(case["in_gate"], false);
    assert_eq!(case["exclusions"][0], "pending: needs the M2 runtime");
}
#[test]
fn a_pending_passing_file_fails() {
    let root = Fixture::new();
    let path = "conformance/OWN/OWN-6-reject.hir";
    pending(&root, &format!("{path} # needs the M2 runtime\n"));
    let report = run_suite(&root.0, &passing()).unwrap();
    assert_eq!(
        report.errors,
        [format!(
            "{path}: passes; remove it from conformance/pending.txt"
        )]
    );
}
#[test]
fn a_pending_missing_path_fails_the_shard_that_owns_it() {
    let root = Fixture::new();
    let path = "conformance/OWN/absent.hir";
    pending(&root, &format!("{path} # needs the M2 runtime\n"));
    let owners: Vec<_> = (1..=3)
        .filter(|&index| {
            let report =
                run_suite_shard(&root.0, &passing(), Some(Shard { index, count: 3 })).unwrap();
            !errors_with(&report, "pending path names no corpus file").is_empty()
        })
        .collect();
    assert_eq!(owners.len(), 1);
    assert!(
        Shard {
            index: owners[0],
            count: 3
        }
        .holds(path)
    );
}
#[test]
fn every_pending_line_is_a_sorted_unique_path_with_a_reason() {
    let root = Fixture::new();
    let a = "conformance/OWN/OWN-6-fail.hir";
    let b = "conformance/OWN/OWN-6-reject.hir";
    for malformed in [
        format!("{a}\n"),
        format!("{a} # \n"),
        format!("{a} #reason\n"),
        format!("{b} # x\n{a} # y\n"),
        format!("{a} # x\n{a} # y\n"),
        "conformance/../escape.hir # x\n".into(),
        "OWN/OWN-6-fail.hir # x\n".into(),
        "\n".into(),
    ] {
        pending(&root, &malformed);
        assert!(
            run_suite(&root.0, &failing("OWN-6-fail.hir"))
                .unwrap_err()
                .starts_with("conformance/pending.txt: "),
            "{malformed:?}"
        );
    }
    pending(&root, &format!("{a} # x\n{b} # y\n"));
    let driver = Statuses(vec![
        ("OWN-6-fail.hir", Status::Fail("x".into())),
        ("OWN-6-reject.hir", Status::NotImplemented),
    ]);
    assert!(run_suite(&root.0, &driver).unwrap().success());
    fs::remove_file(root.0.join("conformance/pending.txt")).unwrap();
    assert!(run_suite(&root.0, &passing()).is_err());
}

#[test]
fn shards_partition_the_corpus() {
    assert_eq!(Shard::parse("2/3"), Some(Shard { index: 2, count: 3 }));
    for invalid in ["0/3", "4/3", "1/0", "3", "a/b"] {
        assert_eq!(Shard::parse(invalid), None, "{invalid}");
    }
    let root = Fixture::new();
    let whole = run_suite(&root.0, &passing()).unwrap();
    for count in [1, 2, 3, 5] {
        let shards: Vec<_> = (1..=count)
            .map(|index| {
                run_suite_shard(&root.0, &passing(), Some(Shard { index, count })).unwrap()
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
            "conformance/OWN/OWN-6-accept.hir",
            "conformance/OWN/OWN-6-reject.hir",
            "conformance/OWN/OWN-6-fail.hir",
            "conformance/OWN/nested/OWN-6-run.hir",
        ] {
            // A fresh file: the tests edit the copies, whatever the fixtures' modes.
            fs::write(root.join(file), fs::read(fixtures.join(file)).unwrap()).unwrap();
        }
        fs::write(root.join("conformance/pending.txt"), "").unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn suite_discovers_nested_files_and_fails_every_unpassed_in_gate_file() {
    let root = Fixture::new();
    let report = run_suite(&root.0, &Compiler).unwrap();
    assert_eq!(report.counts.validated, 4);
    assert_eq!(report.counts.not_implemented, 1);
    assert_eq!(report.counts.passing, 1);
    assert_eq!(report.counts.failing, 2);
    // Unlisted files gate too: each in-gate file that did not pass is an error.
    let mut unpassed: Vec<_> = report
        .errors
        .iter()
        .filter(|e| e.contains("an in-gate file must pass"))
        .map(|e| e.split(':').next().unwrap())
        .collect();
    unpassed.sort();
    assert_eq!(
        unpassed,
        [
            "conformance/OWN/OWN-6-fail.hir",
            "conformance/OWN/OWN-6-reject.hir",
            "conformance/OWN/nested/OWN-6-run.hir"
        ],
        "{:?}",
        report.errors
    );
    assert!(run_suite(&root.0, &passing()).unwrap().success());
    fs::write(
        root.0.join("conformance/OWN/invalid.hir"),
        "// rule: OWN-99\n// expect: accept\n",
    )
    .unwrap();
    assert!(!run_suite(&root.0, &passing()).unwrap().success());
}
#[test]
fn names_rules_and_twins_must_agree_with_their_headers() {
    // The fixture's rejected case names its accepted twin.
    assert!(run_suite(&Fixture::new().0, &passing()).unwrap().success());
    let accept = "// rule: OWN-6\n// expect: accept\nfn f() {}\n";
    let reject = |twin: &str| {
        format!("// rule: OWN-6\n// expect: reject OWN-6 at line 4\n// twin: {twin}\nfn f() {{}}\n")
    };
    for (file, source, message) in [
        (
            "OWN-6-accepted-bad.hir",
            accept.to_owned(),
            "needs expect: reject or fail",
        ),
        (
            "OWN-6-rejected-ok.hir",
            reject("OWN/OWN-6-accept.hir"),
            "cannot expect reject",
        ),
        (
            "OWN-7-misnamed-bad.hir",
            reject("OWN/OWN-6-accept.hir"),
            "differs from the name's rule prefix",
        ),
        (
            "OWN-6-untwinned-bad.hir",
            "// rule: OWN-6\n// expect: reject OWN-6 at line 3\nfn f() {}\n".to_owned(),
            "needs a twin: header",
        ),
        (
            "OWN-6-twin-rejected-bad.hir",
            reject("OWN/OWN-6-reject.hir"),
            "is not an accepted, run or test case",
        ),
        (
            "OWN-6-twin-missing-bad.hir",
            reject("OWN/OWN-6-absent-ok.hir"),
            "names no corpus case",
        ),
        (
            "OWN-6-twinned-ok.hir",
            "// rule: OWN-6\n// expect: accept\n// twin: OWN/OWN-6-accept.hir\nfn f() {}\n"
                .to_owned(),
            "twin is only valid for expect: reject",
        ),
    ] {
        let root = Fixture::new();
        let path = format!("conformance/OWN/{file}");
        fs::write(root.0.join(&path), source).unwrap();
        let report = run_suite(&root.0, &passing()).unwrap();
        assert!(
            report.errors.len() == 1
                && report.errors[0].starts_with(&format!("{path}: "))
                && report.errors[0].contains(message),
            "{file}: {:?}",
            report.errors
        );
    }
}
#[test]
fn binary_json_and_exit_status_follow_validation_and_the_gate() {
    let root = Fixture::new();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_hirsel-conformance"))
            .args(["--json", "--root"])
            .arg(&root.0)
            .output()
            .unwrap()
    };
    // The fixtures the real compiler does not pass leave the gate.
    let pending = root.0.join("conformance/pending.txt");
    fs::write(
        &pending,
        "conformance/OWN/OWN-6-fail.hir # M2\nconformance/OWN/OWN-6-reject.hir # M2\nconformance/OWN/nested/OWN-6-run.hir # M2\n",
    )
    .unwrap();
    let output = run();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["summary"]["validated"], 4);
    assert_eq!(json["summary"]["not_implemented"], 1);
    assert_eq!(json["in_gate"]["validated"], 1);
    assert_eq!(json["unchecked"], 0);
    assert!(
        json["tests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["unchecked"] == 0)
    );
    // Back in the gate, an unlisted file that does not pass fails the run.
    fs::write(
        &pending,
        "conformance/OWN/OWN-6-fail.hir # M2\nconformance/OWN/nested/OWN-6-run.hir # M2\n",
    )
    .unwrap();
    let output = run();
    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["errors"].as_array().unwrap().len(), 1, "{json}");
    assert!(
        json["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("conformance/OWN/OWN-6-reject.hir: an in-gate file must pass, got Fail(")
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

#[test]
fn gate_exclusions_are_an_offline_union_of_pending_and_open_spec_issues() {
    let root = Fixture::new();
    fs::write(
        root.0.join("conformance/pending.txt"),
        "conformance/OWN/OWN-6-accept.hir # M2\n",
    )
    .unwrap();
    let accept = root.0.join("conformance/OWN/OWN-6-accept.hir");
    let source = fs::read_to_string(&accept).unwrap();
    // Open issues overlap the pending line; the closed #5 marker must add no exclusion.
    fs::write(
        &accept,
        format!("// spec: #4\n// spec: #7\n// spec: #5\n{source}"),
    )
    .unwrap();
    // Out of the gate, a file may fail.
    let report = run_suite(&root.0, &failing("OWN-6-accept.hir")).unwrap();
    assert!(report.success(), "{:?}", report.errors);
    assert_eq!(report.counts.failing, 1);
    assert_eq!(report.counts.validated, 4);
    assert_eq!(report.in_gate.validated, 3);
    assert_eq!(
        report.exclusions["conformance/OWN/OWN-6-accept.hir"].len(),
        3
    );
    assert!(
        !report.exclusions["conformance/OWN/OWN-6-accept.hir"]
            .iter()
            .any(|reason| reason == "open spec issue #5")
    );
    let json = report.json();
    let case = json["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["path"] == "conformance/OWN/OWN-6-accept.hir")
        .unwrap();
    assert_eq!(json["unchecked"], 0);
    assert_eq!(case["unchecked"], 0);
    assert_eq!(case["in_gate"], false);
    assert_eq!(case["exclusions"].as_array().unwrap().len(), 3);
    // Only a pending line leaves the gate; open spec issues alone keep it out too.
    fs::write(root.0.join("conformance/pending.txt"), "").unwrap();
    let report = run_suite(&root.0, &failing("OWN-6-accept.hir")).unwrap();
    assert!(report.success(), "{:?}", report.errors);
    assert_eq!(
        report.exclusions["conformance/OWN/OWN-6-accept.hir"].len(),
        2
    );
}

#[test]
fn path_selection_runs_only_matches_and_gates_them() {
    use hirsel_conformance::{Selection, run_suite_selected};
    let root = Fixture::new();
    let paths = ["conformance/OWN/OWN-6-accept.hir".to_owned()];
    let selection = Selection {
        shard: None,
        paths: &paths,
    };
    let report = run_suite_selected(&root.0, &passing(), selection).unwrap();
    assert!(report.success(), "{:?}", report.errors);
    assert_eq!(report.counts.validated, 1);
    assert_eq!(report.tests[0].path, paths[0]);
    let empty = ["conformance/absent".to_owned()];
    assert!(
        run_suite_selected(
            &root.0,
            &passing(),
            Selection {
                shard: None,
                paths: &empty
            }
        )
        .unwrap_err()
        .contains("selected no corpus files")
    );
    let report = run_suite_selected(&root.0, &failing("OWN-6-accept.hir"), selection).unwrap();
    assert_eq!(report.counts.failing, 1);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("an in-gate file must pass"))
    );
    // Formatting shares the filter: an unrelated malformed file cannot enter it.
    let mut report = hirsel_conformance::Report::default();
    hirsel_conformance::corpus_format::check_canonical_selected(&root.0, &mut report, selection)
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    fs::write(
        root.0.join("conformance/OWN/OWN-6-reject.hir"),
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
fn a_run_past_the_memory_limit_ends_the_runner_with_the_guard_message() {
    // Checking and running even an empty main needs more than 64M (the
    // compiler's stack alone is 128M), so the runner must stop at the limit.
    let root = Fixture::new();
    fs::write(
        root.0.join("conformance/OWN/empty-run.hir"),
        "// rule: OWN-6\n// expect: run\npub fn main() -> Result[(), ()] {\n  Ok(())\n}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel-conformance"))
        .args([
            "--shard",
            "1/1",
            "--path",
            "conformance/OWN/empty-run.hir",
            "--root",
        ])
        .arg(&root.0)
        .env("HIRSEL_MEMORY_LIMIT", "64M")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(hirsel_guard::EXIT_STATUS),
        "{stderr}"
    );
    assert!(
        stderr.starts_with("hirsel: memory limit 64M reached (HIRSEL_MEMORY_LIMIT); allocating "),
        "{stderr}"
    );
}

#[test]
fn path_selection_requires_expected_outcomes() {
    use hirsel_conformance::{Selection, run_suite_selected};
    let root = Fixture::new();
    let paths = ["conformance/OWN/OWN-6-accept.hir".to_owned()];
    let selection = Selection {
        shard: None,
        paths: &paths,
    };
    let report = run_suite_selected(&root.0, &failing("OWN-6-accept.hir"), selection).unwrap();
    assert_eq!(report.counts.failing, 1);
    assert!(
        !report.success(),
        "unlisted selected failures must fail the run"
    );
    let passed = run_suite_selected(&root.0, &passing(), selection).unwrap();
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
        hirsel_conformance::fixture("// grant: no-output\npub fn main() {}\n")
            .unwrap()
            .deny_output
    );
    assert!(
        hirsel_conformance::fixture("// grant: no-output\n// grant: no-output\npub fn main() {}\n")
            .is_err()
    );
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

#[test]
fn a_steps_header_raises_the_budget_and_names_its_reason() {
    let run = |lines: &str| {
        header(&format!(
            "// rule: OWN-6\n// expect: run\n{lines}pub fn main() {{}}\n"
        ))
    };
    for lines in [
        "// steps: 20000000\n// a 33-limb Karatsuba product takes about 150k steps\n",
        "// steps: 1000001\n// reason: prose may hold a colon\n",
    ] {
        assert!(run(lines).is_ok(), "{lines}");
    }
    let accepted = "// steps: 20000000\n// why\n";
    assert_eq!(
        hirsel_conformance::fixture(accepted).unwrap().steps,
        Some(20_000_000)
    );
    assert_eq!(hirsel_conformance::fixture("").unwrap().steps, None);
    for (lines, error) in [
        ("// steps: 20000001\n// why\n", "at most 20000000"),
        ("// steps: 1000000\n// why\n", "exceed the default 1000000"),
        ("// steps: +2000000\n// why\n", "whole number"),
        ("// steps: 2e6\n// why\n", "whole number"),
        ("// steps: 2000000\n", "reason comment"),
        ("// steps: 2000000\n//\n", "reason comment"),
        ("// steps: 2000000\n// exit: 0\n", "reason comment"),
        (
            "// steps: 2000000\n// one\n// steps: 3000000\n// two\n",
            "duplicate steps",
        ),
    ] {
        let message = run(lines).unwrap_err();
        assert!(message.contains(error), "{lines}: {message}");
    }
    // Only an executed program has a budget to raise.
    let message =
        header("// rule: OWN-6\n// expect: accept\n// steps: 2000000\n// why\nfn f() {}\n")
            .unwrap_err();
    assert!(message.contains("steps requires"), "{message}");
}

#[test]
fn without_its_steps_header_a_long_case_stops_at_the_default_budget() {
    // The corpus case shows every engine honouring its raised budget; the same
    // program without the header must exhaust the default one.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source =
        fs::read_to_string(root.join("conformance/PROG/PROG-3-steps-header-run.hir")).unwrap();
    let fixture = hirsel_conformance::fixture(&source).unwrap();
    assert_eq!(fixture.steps, Some(3_000_000));
    let dir = std::env::temp_dir().join(format!("hirsel-steps-header-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loop.hir");
    let body: String = source
        .lines()
        .filter(|line| !line.starts_with("//"))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&path, body).unwrap();
    let expect = Expect::Run {
        stdout: String::new(),
        exit: 0,
    };
    let (status, observation) = Compiler.evaluate(&path, &expect);
    assert!(
        matches!(status, Status::Fail(_)) && observation.actual.contains("Steps"),
        "{status:?}: {}",
        observation.actual
    );
    fs::remove_dir_all(dir).unwrap();
}
