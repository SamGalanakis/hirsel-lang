//! The O1–O4 harnesses on today's compiler: every check either passes or names
//! a switch the compiler lacks, and each comparison fails on a planted difference.
use hirsel_conformance::oracle::{
    self, Artifacts, Report, SCENARIOS, Verdict, edits::Scenario, o2, o3, o4, switches::Switch,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn work(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("hirsel-oracle-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// No check failed, and each `Todo` names a switch this compiler lacks.
fn holds(report: &Report) {
    assert!(report.failures().is_empty(), "{}", report.text());
    for check in &report.checks {
        if let Verdict::Todo(switch) = check.verdict {
            assert!(!switch.available(), "{}: {switch} is available", check.name);
        }
    }
}

fn scenarios(oracle: &str) -> Vec<Scenario> {
    let dir = root().join(SCENARIOS).join(oracle);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| Scenario::read(p, &root()).unwrap())
        .collect()
}

#[test]
fn o1_cells_agree_with_the_baseline_including_native_artifacts() {
    let work = work("o1");
    let output = Command::new(env!("CARGO_BIN_EXE_oracle"))
        .args(["o1", "--native", "--json", "--allow-todo", "--work"])
        .arg(&work)
        .args(["--root"])
        .arg(root())
        .args(["--path", "conformance/CT/CT-2-dependent-constant.hir"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["fail"], 0, "{stderr}");
    assert!(report["pass"].as_u64().unwrap() >= 2, "{stderr}");
    let baseline = work.join("0/baseline");
    for artifact in [
        "diagnostics",
        "tir",
        "receipts",
        "objects/llvm/0000.o",
        "objects/cranelift/0000.o",
    ] {
        assert!(
            baseline.join(artifact).is_file(),
            "{artifact} missing: {stderr}"
        );
    }
    // Without --allow-todo, a missing switch fails the oracle.
    if !Switch::IdSalt.available() {
        let status = Command::new(env!("CARGO_BIN_EXE_oracle"))
            .args(["o1", "--salts", "1", "--jobs", "1", "--root"])
            .arg(root())
            .args(["--work"])
            .arg(&work)
            .args(["--path", "conformance/CT/CT-2-dependent-constant.hir"])
            .output()
            .unwrap();
        assert!(!status.status.success());
        assert!(String::from_utf8_lossy(&status.stderr).contains("TODO(ids): IdSalt"));
    }
    let _ = std::fs::remove_dir_all(&work);
}

/// Every scenario except those that copy std (which check std from source:
/// about 270 s and 2.5 GB today; run them with `oracle o2 --scenario`).
#[test]
fn o2_warm_runs_equal_cold_after_every_scripted_edit() {
    let work = work("o2");
    let all = scenarios("o2");
    let selected: Vec<_> = all.iter().filter(|s| !s.with_std).cloned().collect();
    assert!(selected.len() > 20, "{} scenarios", selected.len());
    let report = o2::run(
        Path::new(env!("CARGO_BIN_EXE_oracle")),
        &root(),
        &selected,
        &work,
    );
    holds(&report);
    assert_eq!(report.checks.len(), 3 * selected.len(), "{}", report.text());
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn o3_recompute_claims_hold_and_a_false_claim_fails() {
    let report = o3::run(&scenarios("o3"));
    holds(&report);
    assert!(
        report.count(|v| *v == Verdict::Pass) >= 4,
        "{}",
        report.text()
    );
    // The control: the caller of an edited signature recomputes, so claiming it
    // unchanged fails.
    let false_claim = "// o3: control\n// unchanged: caller\n=== main.hir\n\
        fn callee(x: u64) -> u64 {\n  x\n}\n\nfn caller() -> u64 {\n  callee(1)\n}\n\
        ~~~ main.hir\n- fn callee(x: u64) -> u64 {\n+ fn callee(x: u32) -> u64 {\n";
    let scenario = Scenario::parse("control", false_claim, &root()).unwrap();
    let report = o3::run(&[scenario]);
    let failures = report.failures();
    assert_eq!(failures.len(), 1, "{}", report.text());
    assert!(
        failures[0].name.contains("caller unchanged"),
        "{}",
        report.text()
    );
}

#[test]
fn o4_store_hits_equal_cold_runs() {
    let work = work("o4");
    let programs = oracle::corpus(
        &root(),
        &[
            "conformance/CT/CT-5-cross-module-spelling-run.hir".into(),
            "conformance/CT/CT-5-generator-error-bad.hir".into(),
        ],
        None,
        None,
    )
    .unwrap();
    let report = o4::run(
        Path::new(env!("CARGO_BIN_EXE_oracle")),
        &root(),
        &programs,
        &work,
    );
    holds(&report);
    assert_eq!(
        report.count(|v| *v == Verdict::Pass),
        2,
        "{}",
        report.text()
    );
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn artifact_differences_name_the_first_differing_line_or_byte() {
    let text = |s: &str| s.as_bytes().to_vec();
    let a: Artifacts = [
        ("tir".to_string(), text("one\ntwo\n")),
        ("o".into(), vec![0, 2, 3]),
    ]
    .into();
    assert_eq!(oracle::difference(&a, &a), None);
    let mut b = a.clone();
    b.insert("tir".into(), text("one\nTWO\n"));
    assert_eq!(
        oracle::difference(&a, &b).unwrap(),
        "tir: line 2: expected \"two\", got \"TWO\""
    );
    let mut b = a.clone();
    b.insert("o".into(), vec![0, 9, 3]);
    assert!(
        oracle::difference(&a, &b)
            .unwrap()
            .starts_with("o: bytes differ at offset 1")
    );
    let mut b = a.clone();
    b.remove("o");
    assert_eq!(oracle::difference(&a, &b).unwrap(), "o: missing");
    let mut c = a.clone();
    c.insert("extra".into(), vec![]);
    assert_eq!(oracle::difference(&a, &c).unwrap(), "extra: unexpected");
}

#[test]
fn scenarios_refuse_ambiguous_or_empty_edits() {
    let parse = |text: &str| Scenario::parse("s", text, &root());
    let base = "// o2: s\n=== main.hir\nfn a() {}\nfn a() {}\n";
    for (edit, error) in [
        (
            "~~~ main.hir\n- fn a() {}\n+ fn b() {}\n",
            "does not occur exactly once",
        ),
        (
            "~~~ other.hir\n- a\n+ b\n",
            "patch of a file the tree lacks",
        ),
        ("--- other.hir\n", "a deletion names a file of the tree"),
        (
            "+++ main.hir\nfn a() {}\nfn a() {}\n",
            "the edit changes nothing",
        ),
    ] {
        let error_text = parse(&format!("{base}{edit}")).unwrap_err();
        assert!(error_text.contains(error), "{edit}: {error_text}");
    }
    assert!(parse("=== main.hir\n").unwrap_err().contains("first line"));
    let scenario = parse(&format!("{base}--- main.hir\n+++ lib.hir\nfn c() {{}}\n")).unwrap();
    assert_eq!(scenario.after.keys().collect::<Vec<_>>(), ["lib.hir"]);
    assert_eq!(scenario.before["main.hir"], "fn a() {}\nfn a() {}\n");
}
