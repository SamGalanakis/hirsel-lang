//! The O1–O4 harnesses on today's compiler: every check either passes or names
//! a switch the compiler lacks, and each comparison fails on a planted difference.
use hirsel_conformance::oracle::{
    self, Artifacts, Report, SCENARIOS, Verdict, edits::Scenario, o2, o3, o5, switches::Switch,
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
        .args(["o1", "--native", "--json", "--work"])
        .arg(&work)
        .args(["--root"])
        .arg(root())
        .args([
            "--path",
            "conformance/CT/CT-2-dependent-constant.hir",
            "--path",
            "conformance/CT/CT-5-cross-module-spelling-run.hir",
            "--path",
            "conformance/CT/CT-5-generator-error-bad.hir",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["fail"], 0, "{stderr}");
    assert_eq!(report["todo"], 0, "{stderr}");
    assert_eq!(report["pass"], 33, "{stderr}");
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
    let mut selected: Vec<_> = all.iter().filter(|s| !s.with_std).cloned().collect();
    for scenario in &mut selected {
        if scenario.entry.ends_with(".hir") {
            // These scenarios exercise core-only programs. An explicit fixture
            // keeps every cold and warm cell from checking unrelated std bodies.
            let manifest = serde_json::json!({
                "schema": 1,
                "package": {"name": "standalone", "version": "0"},
                "sources": [{"path": scenario.entry, "module": []}],
                "entry": {"module": [], "name": "main"},
                "dependencies": []
            })
            .to_string();
            let sidecar = format!("{}.json", scenario.entry);
            assert!(!scenario.before.contains_key(&sidecar));
            assert!(!scenario.after.contains_key(&sidecar));
            scenario.before.insert(sidecar.clone(), manifest.clone());
            scenario.after.insert(sidecar, manifest);
        }
    }
    assert!(selected.len() > 20, "{} scenarios", selected.len());
    let report = o2::run(
        Path::new(env!("CARGO_BIN_EXE_oracle")),
        &root(),
        &selected,
        &work,
    );
    holds(&report);
    assert!(report.passed(false), "{}", report.text());
    assert_eq!(report.checks.len(), 3 * selected.len(), "{}", report.text());
    let _ = std::fs::remove_dir_all(&work);
}

fn require_accepted_export(artifacts: &Path) {
    let diagnostics = std::fs::read_to_string(artifacts.join("diagnostics")).unwrap();
    let tir = std::fs::read_to_string(artifacts.join("tir"))
        .unwrap_or_else(|e| panic!("{e}: {diagnostics}"));
    assert!(!tir.starts_with("export:"), "{tir}");
    let header = hirsel_proof_export::read_header(&tir).unwrap();
    assert_eq!(header.format, hirsel_proof_export::FORMAT);
    for line in diagnostics.lines() {
        let diagnostic: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_ne!(diagnostic["severity"], "error", "{line}");
    }
}

fn show_tir_difference(base: &Path) {
    let cold = std::fs::read_to_string(base.join("cold/tir")).unwrap();
    let expected: Vec<_> = cold.lines().collect();
    for mode in ["resident", "disk-after"] {
        let warm = std::fs::read_to_string(base.join(mode).join("tir")).unwrap();
        let actual: Vec<_> = warm.lines().collect();
        if let Some(first) = expected.iter().zip(&actual).position(|(a, b)| a != b) {
            let start = first.saturating_sub(8);
            eprintln!(
                "{mode}: TIR lines {} versus {}; first difference at {}\ncold: {}\nwarm: {}",
                expected.len(),
                actual.len(),
                first + 1,
                expected[start..(first + 16).min(expected.len())].join("\n"),
                actual[start..(first + 16).min(actual.len())].join("\n")
            );
        }
    }
}

#[test]
fn o2_std_option_body_edit_matches_cold_in_minimal_manifest() {
    let work = work("o2-std-minimal");
    let mut scenario = Scenario::read(
        &root().join(SCENARIOS).join("o2/std-touching.edit"),
        &root(),
    )
    .unwrap();
    // Keep the actual std option source and edit, narrowing only its manifest
    // to the module reached by this program.
    let manifest = r#"{"schema":1,"package":{"name":"std","version":"1"},"sources":[{"path":"option.hir","module":["option"]}],"dependencies":[]}"#;
    scenario
        .before
        .insert("std@1/hirsel.json".into(), manifest.into());
    scenario
        .after
        .insert("std@1/hirsel.json".into(), manifest.into());
    let report = o2::run(
        Path::new(env!("CARGO_BIN_EXE_oracle")),
        &root(),
        &[scenario],
        &work,
    );
    require_accepted_export(&work.join("std-touching/cold"));
    if !report.failures().is_empty() {
        show_tir_difference(&work.join("std-touching"));
    }
    assert!(report.passed(false), "{}", report.text());
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
fn o3_ten_thousand_line_body_edit_touches_only_affected_memos() {
    let mut scenario = scenarios("o3")
        .into_iter()
        .find(|s| s.name == "callee-body")
        .unwrap();
    let mut padding = String::new();
    for i in 0..2500 {
        padding.push_str(&format!("\nfn spare_{i}() -> u64 {{\n    {i}u64\n}}\n"));
    }
    for tree in [&mut scenario.before, &mut scenario.after] {
        let source = tree.get_mut(&scenario.entry).unwrap();
        source.push_str(&padding);
        assert!(source.lines().count() >= 10_000);
    }
    scenario
        .unchanged
        .extend(["spare_1".into(), "spare_1200".into()]);
    let report = o3::run(&[scenario]);
    assert!(report.passed(false), "{}", report.text());
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
    let output = Command::new(env!("CARGO_BIN_EXE_oracle"))
        .env("HIRSEL_ORACLE_CLI", env!("CARGO_BIN_EXE_hirsel"))
        .args(["o4", "--json", "--root"])
        .arg(root())
        .arg("--work")
        .arg(&work)
        .args(programs.iter().flat_map(|path| {
            [
                "--path".to_owned(),
                path.strip_prefix(root())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ]
        }))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["pass"], 6);
    assert_eq!(report["todo"], 0);
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

/// Programs where an annotation once leaked into meaning: a diagnostic over a
/// whole item, a FACT-2 predicate's revision, a recursive type group's key,
/// generated code and instances (CT-5), an export its import refuses, and a
/// program that already holds annotations.
const O5_PINNED: [&str; 7] = [
    "conformance/CAP/CAP-2-erased-carrier-inert-bound-bad.hir",
    "conformance/CAP/CAP-2-erased-carrier-inert-bound-ok.hir",
    "conformance/CT/CT-5-generic-declaration-bad.hir",
    "conformance/CT/CT-5-instance-cap-ok.hir",
    "conformance/FACT/FACT-2-percent-boundary.hir",
    "conformance/GEN/GEN-1-noalloc-drop-mutual-group-ok.hir",
    "conformance/PS/PS-17-analysis-annotations-run.hir",
];

/// About one corpus program in sixty-four, chosen by a seeded hash of its path, so
/// adding a program never reshuffles the others, and every pinned program.
fn o5_sampled(path: &Path) -> bool {
    let relative = path.strip_prefix(root()).unwrap_or(path);
    if O5_PINNED.iter().any(|p| relative == Path::new(p)) {
        return true;
    }
    let hash = hirsel_syntax::ContentHash::of_parts(
        "hirsel.test.sample",
        [
            b"hirsel.o5.sample.v1".as_slice(),
            relative.to_string_lossy().as_bytes(),
        ],
    );
    hash.0[0].is_multiple_of(64)
}

/// PS-19 per push: a probe at every annotation site of each sampled program
/// changes no diagnostic, hash, export section but its own, interpreted run
/// or, for two running programs, native run. `oracle o5` runs the whole corpus.
#[test]
fn o5_annotations_change_nothing_over_a_corpus_sample() {
    let work = work("o5");
    let rules = hirsel_conformance::spec_rules(&root()).unwrap();
    let programs: Vec<_> = oracle::corpus(&root(), &["conformance".into()], None, None)
        .unwrap()
        .into_iter()
        .filter(|p| o5_sampled(p))
        .collect();
    let report = o5::run(&root(), &rules, &programs, 2, &work);
    holds(&report);
    let passed = report.count(|v| *v == Verdict::Pass);
    assert!(passed >= 45, "{passed} programs\n{}", report.text());
    let native = report
        .checks
        .iter()
        .filter(|c| c.name.contains("native"))
        .count();
    assert_eq!(native, 2, "{}", report.text());
    let _ = std::fs::remove_dir_all(&work);
}

const SITES: &str = "// rule: PS-17\n// expect: run\n\
    fn f[T](x: u64, mut ys: List[T],) -> u64 { x }\n\
    type R { a: u64, b: fn(u64) -> u64 }\n\
    type V = A(u64, (u8, u8)) | B | C(x: u64)\n\
    distinct type D(v: u64)\n\
    pub fn main() -> Result[(), ()] {\n  Ok(())\n}\n";

#[test]
fn o5_probes_every_site_kind() {
    let probed = o5::probe(SITES).unwrap();
    let p = |n: usize| format!("@analysis(o5.probe: {n} [site]) ");
    let expected = format!(
        "// rule: PS-17\n// expect: run\n\
         {}fn f[T]({}x: u64, {}mut ys: List[T],) -> u64 {{ x }}\n\
         {}type R {{ {}a: u64, {}b: fn(u64) -> u64 }}\n\
         {}type V = A({}u64, {}(u8, u8)) | B | C({}x: u64)\n\
         {}distinct type D({}v: u64)\n\
         {}pub fn main() -> Result[(), ()] {{\n  Ok(())\n}}\n",
        p(0),
        p(1),
        p(2),
        p(3),
        p(4),
        p(5),
        p(6),
        p(7),
        p(8),
        p(9),
        p(10),
        p(11),
        p(12)
    );
    assert_eq!(probed.annotated, expected);
    assert_eq!(probed.sites, 13);
    assert_eq!(probed.blanked.len(), probed.annotated.len());
    let squeezed = |s: &str| s.split_whitespace().collect::<String>();
    assert_eq!(squeezed(&probed.blanked), squeezed(SITES));
}

/// The comparison is not vacuous: a twin that differs in meaning fails.
#[test]
fn o5_fails_when_the_twin_differs_in_meaning() {
    let work = work("o5-planted");
    let source = "// rule: PS-17\n// expect: run\n\
        fn twice(x: u64) -> u64 { x * 2u64 }\n\
        pub fn main() -> Result[(), ()] {\n  let n = twice(3u64)\n  Ok(())\n}\n";
    let probed = o5::probe(source).unwrap();
    let expect = hirsel_conformance::Expect::Run {
        stdout: String::new(),
        exit: 0,
    };
    let path = work.join("planted.hir");
    let same = o5::compare(
        &path,
        &expect,
        &probed.annotated,
        &probed.blanked,
        probed.sites,
        false,
    );
    assert_eq!(same, Verdict::Pass);
    let planted = probed.blanked.replace("2u64", "3u64");
    match o5::compare(
        &path,
        &expect,
        &probed.annotated,
        &planted,
        probed.sites,
        false,
    ) {
        Verdict::Fail(why) => assert!(why.starts_with("annotations changed"), "{why}"),
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn candidate_transactions_match_independent_cold_trees() {
    use hirsel_conformance::oracle::candidate;
    // Fixture parsing/materialization is independent of the frozen adapter.
    let root = root();
    let work = work("candidate-txn");
    let names = [
        "body-edit",
        "signature-private",
        "signature-public",
        "definition-add",
        "definition-remove",
        "generator-helper-body",
        "reflected-helper-lookup",
        "secondary-diagnostic-site",
        "duplicate-insert-file",
        "duplicate-remove-file",
        "edit-above-item",
        "opaque-representation",
        "candidate/recursive-type",
        "candidate/mutual-type",
        "candidate/missing-recursive-type",
    ];
    let mut completed = 0;
    for name in names {
        let relative = if name.contains('/') {
            name.to_owned()
        } else {
            format!("o2/{name}")
        };
        let scenario = Scenario::read(
            &root.join(SCENARIOS).join(format!("{relative}.edit")),
            &root,
        )
        .unwrap();
        let before = work.join(name).join("base");
        let after = work.join(name).join("cold");
        scenario.write(&before, &root, false).unwrap();
        scenario.write(&after, &root, true).unwrap();
        candidate::bounded_package(&scenario.entry(&before)).unwrap();
        candidate::bounded_package(&scenario.entry(&after)).unwrap();
        let config = oracle::Config::default();
        let workspace = candidate::workspace(&config).unwrap();
        let base = workspace.capture(&scenario.entry(&before)).unwrap();
        let lease = candidate::submit(&workspace, &base, 1, &scenario.after).unwrap();
        let actual = candidate::result(&workspace, &lease, &scenario.entry(&before)).unwrap();
        let expected = candidate::cold(&scenario.entry(&after), &config).unwrap();
        assert_eq!(
            oracle::difference(&expected, &actual),
            None,
            "{name}: {:?}",
            oracle::difference(&expected, &actual)
        );
        completed += 1;
    }
    assert_eq!(completed, names.len());
    std::fs::remove_dir_all(work).unwrap();
}

#[test]
fn candidate_transactions_jobs_salts_in_separate_processes() {
    let work = work("candidate-matrix");
    let mut cells = 0;
    for jobs in [1, 2, 8] {
        for salt in [0, 1, 7] {
            let cell = work.join(format!("jobs{jobs}-salt{salt}"));
            let output = Command::new(env!("CARGO_BIN_EXE_oracle"))
                .args([
                    "candidate-cell",
                    "--jobs",
                    &jobs.to_string(),
                    "--salt",
                    &salt.to_string(),
                    "--out",
                ])
                .arg(&cell)
                .arg(root())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "jobs{jobs}/salt{salt}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let counts = std::fs::read_to_string(cell.join("counts.txt")).unwrap();
            assert!(counts.starts_with("completed=22 "), "{counts}");
            cells += 1;
        }
    }
    assert_eq!(cells, 9);
    std::fs::remove_dir_all(work).unwrap();
}
/// Opposing ownership/capability verdicts and moved source maps are compared
/// through real independent disk checks. Swap each sibling artifact that
/// differs and require the ordinary comparison to reject it.
#[test]
fn candidate_opposing_siblings_and_swapped_artifacts_are_detected() {
    use hirsel_conformance::oracle::candidate;
    use std::collections::BTreeMap;
    let work = work("candidate-sibling-faults");
    let pairs = [
        (
            "ownership",
            "fn identity[T](take x:T)->T{x}\n",
            "fn identity[T](x:T)->T{x}\n",
        ),
        (
            "capability",
            "@noalloc\nfn inert_only[T:inert](x:T)->(){()}\nfn check(value:u32)->(){inert_only(value)}\n",
            "@noalloc\nfn inert_only[T:inert](x:T)->(){()}\nfn check(value:Dir)->(){inert_only(value)}\n",
        ),
    ];
    for (name, good, bad) in pairs {
        let dir = work.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("main.hir");
        std::fs::write(&entry, good).unwrap();
        candidate::bounded_package(&entry).unwrap();
        let config = oracle::Config::default();
        let w = candidate::workspace(&config).unwrap();
        let base = w.capture(&entry).unwrap();
        let a = candidate::submit(
            &w,
            &base,
            1,
            &BTreeMap::from([("main.hir".into(), good.into())]),
        )
        .unwrap();
        let b = candidate::submit(
            &w,
            &base,
            2,
            &BTreeMap::from([("main.hir".into(), bad.into())]),
        )
        .unwrap();
        let actual_a = candidate::result(&w, &a, &entry).unwrap();
        let actual_b = candidate::result(&w, &b, &entry).unwrap();
        let mut expected = vec![];
        for (i, text) in [good, bad].iter().enumerate() {
            let tree = dir.join(format!("cold-{i}"));
            std::fs::create_dir(&tree).unwrap();
            let path = tree.join("main.hir");
            std::fs::write(&path, text).unwrap();
            candidate::bounded_package(&path).unwrap();
            expected.push(candidate::cold(&path, &config).unwrap());
        }
        assert_eq!(expected[0]["state"], b"pass");
        assert_eq!(expected[1]["state"], b"reject");
        assert_eq!(oracle::difference(&expected[0], &actual_a), None);
        assert_eq!(oracle::difference(&expected[1], &actual_b), None);
        let mut swaps = 0;
        for (key, value) in &actual_b {
            if actual_a.get(key) == Some(value) {
                continue;
            }
            let mut swapped = actual_a.clone();
            swapped.insert(key.clone(), value.clone());
            assert!(
                oracle::difference(&expected[0], &swapped).is_some(),
                "{name}/{key}"
            );
            swaps += 1;
        }
        assert!(
            swaps >= 3,
            "opposing verdict/diagnostics/products swaps not exercised"
        );
        // Keep the old diagnostics, move text in a third isolated candidate.
        let shifted = format!("// moved anchor\n\n{bad}");
        let c = candidate::submit(
            &w,
            &base,
            3,
            &BTreeMap::from([("main.hir".into(), shifted.clone())]),
        )
        .unwrap();
        let actual_c = candidate::result(&w, &c, &entry).unwrap();
        assert_ne!(actual_b["diagnostics/json"], actual_c["diagnostics/json"]);
        let mut swapped = actual_c.clone();
        swapped.insert(
            "diagnostics/json".into(),
            actual_b["diagnostics/json"].clone(),
        );
        assert!(oracle::difference(&actual_c, &swapped).is_some());
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), good);
    }
    std::fs::remove_dir_all(work).unwrap();
}
