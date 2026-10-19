//! The native gate's own rules: the set-wise ratchet, mismatch artifacts that
//! replay from another path and another arena numbering, and a reducer that
//! keeps the same discrepancy. The discrepancy is synthetic, so these hold
//! when no engine disagrees.
use hirsel_conformance::native::artifact::{self, Discrepancy, Probe};
use hirsel_conformance::native::{self, CaseResult, Class, Exclusion, Exclusions};
use hirsel_driver::native::{NativeError, Observation, Outcome, StageName};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An engine that reports every SEM-1 failure one column to the right, and
/// loses the output of a program that fails by FACT-1.
struct Skewed {
    warm: Option<&'static str>,
}

impl Probe for Skewed {
    fn observe(&self, source: &str) -> Option<(Observation, Observation)> {
        let expected = artifact::reference(source, self.warm)?;
        let mut actual = expected.clone();
        for failure in &mut actual.failures {
            if failure.rule == "SEM-1" {
                failure.column += 1;
            }
        }
        if actual.failures.iter().any(|f| f.rule == "FACT-1") {
            actual.output.clear();
        }
        Some((expected, actual))
    }
}

const FIXTURE: &str = "native-tests/replay/column-skew";
const WARM: &str = "type Warm { a: u64, b: Bytes }\nfn warm(x: u64) -> u64 { x + 1u64 }\n";

fn on_compiler<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    hirsel_engine::with_compiler_stack(f)
}

#[test]
fn a_stored_artifact_replays_from_another_path_and_arena_numbering() {
    on_compiler(|| {
        // The checked-in fixture: `tools/bless.sh replay` rewrites it from its source.
        if std::env::var_os("HIRSEL_BLESS").is_some() {
            let dir = root().join(FIXTURE);
            let source = std::fs::read_to_string(dir.join("source.hir")).unwrap();
            let (expected, actual) = Skewed { warm: None }.observe(&source).unwrap();
            artifact::write(
                &dir,
                FIXTURE,
                &source,
                "synthetic/column-skew",
                &expected,
                &actual,
            )
            .unwrap();
        }
        // Another path: the fixture is copied out of the repository.
        let moved = std::env::temp_dir().join(format!("hirsel-replay-{}", std::process::id()));
        std::fs::create_dir_all(&moved).unwrap();
        for name in ["source.hir", "case.txt"] {
            std::fs::copy(root().join(FIXTURE).join(name), moved.join(name)).unwrap();
        }
        let stored = artifact::read(&moved).unwrap();
        std::fs::remove_dir_all(&moved).unwrap();
        assert_ne!(stored.expected, stored.actual);
        // Another arena numbering: the engine has already interned another
        // file, types and a body before it sees the case.
        artifact::replay(&stored, &Skewed { warm: None }).unwrap();
        artifact::replay(&stored, &Skewed { warm: Some(WARM) }).unwrap();
        // An engine that no longer disagrees does not replay.
        struct Fixed;
        impl Probe for Fixed {
            fn observe(&self, source: &str) -> Option<(Observation, Observation)> {
                let expected = artifact::reference(source, None)?;
                Some((expected.clone(), expected))
            }
        }
        let error = artifact::replay(&stored, &Fixed).unwrap_err();
        assert!(error.contains("the actual observation changed"), "{error}");
    });
}

#[test]
fn an_artifact_of_another_schema_version_is_refused() {
    let dir = std::env::temp_dir().join(format!("hirsel-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let case = std::fs::read_to_string(root().join(FIXTURE).join("case.txt")).unwrap();
    std::fs::write(
        dir.join("case.txt"),
        case.replace(
            &format!(
                "observation-schema {}",
                hirsel_driver::native::OBSERVATION_SCHEMA
            ),
            "observation-schema 0",
        ),
    )
    .unwrap();
    std::fs::write(dir.join("source.hir"), "").unwrap();
    let Err(error) = artifact::read(&dir) else {
        panic!("another schema version was read")
    };
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(error.contains("observation-schema"), "{error}");
}

#[test]
fn the_reducer_keeps_the_same_discrepancy() {
    on_compiler(|| {
        let source = std::fs::read_to_string(root().join(FIXTURE).join("source.hir")).unwrap();
        let probe = Skewed { warm: None };
        let of = |text: &str| {
            let (expected, actual) = probe.observe(text)?;
            Discrepancy::between(&expected, &actual)
        };
        let wanted = of(&source).expect("the fixture disagrees");
        let reduced = artifact::reduce(&source, &probe).unwrap();
        assert_eq!(of(&reduced), Some(wanted.clone()));
        assert!(
            reduced.lines().count() < source.lines().count(),
            "{reduced}"
        );
        // The statement that fails stays; statements it does not need go.
        assert!(reduced.contains("big + "), "{reduced}");
        assert!(!reduced.contains("unrelated"), "{reduced}");
        // The earlier print is part of the discrepancy's expected output only
        // if both sides print it: the reducer may drop it, since output
        // agrees either way; but it never trades the SEM-1 column skew for
        // the FACT-1 output loss, which is another discrepancy.
        assert_eq!(wanted.expected_failures[0].0, "SEM-1");
        assert!(!wanted.output_differs);
        // A source with no discrepancy has nothing to preserve.
        let agreeing = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  Ok(())\n}\n";
        assert!(artifact::reduce(agreeing, &probe).is_err());
    });
}

fn unavailable(message: &str) -> Outcome {
    Outcome::Unavailable(NativeError {
        stage: StageName::Import,
        definition: None,
        validator: false,
        message: message.into(),
    })
}

fn result(id: &str, heap: Outcome, llvm: Outcome) -> CaseResult {
    CaseResult {
        id: id.into(),
        snapshot: None,
        outcomes: vec![
            ("h".into(), Class::Heap, heap),
            ("l".into(), Class::Llvm, llvm),
        ],
        reference: None,
        wall: Default::default(),
    }
}

fn row(reason: &str) -> Exclusion {
    Exclusion {
        class: "native-unavailable".into(),
        reason: reason.into(),
    }
}

fn wrong(output: &[u8]) -> Outcome {
    Outcome::Mismatch(Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        output: output.to_vec(),
        failures: Vec::new(),
    })
}

#[test]
fn an_engine_filtered_report_never_claims_an_unrun_engine() {
    let mut results = vec![
        result("a", Outcome::Agree, Outcome::Agree),
        result("b", unavailable("Unsupported(\"limit\")"), Outcome::Agree),
    ];
    for r in &mut results {
        r.outcomes.retain(|(_, class, _)| *class == Class::Heap);
    }
    let other_engine: Exclusions = [(("a".into(), "l".into()), row("not part of this run"))].into();
    let report = native::judge_classes(&results, &other_engine, 1, false, &[Class::Heap]);
    assert_eq!(report.pass[&Class::Heap], 1);
    assert!(!report.pass.contains_key(&Class::Llvm));
    assert_eq!(report.unlisted.len(), 1);
    assert!(report.stale.is_empty());
}

/// Every disagreeing case and configuration must be exactly its named row: an
/// unlisted failure, a changed reason and a row that now agrees each fail the
/// gate.
#[test]
fn the_set_rule_admits_only_named_exclusions() {
    let gap = || unavailable("Layout(MissingOwner(\"Map\"))");
    let results = vec![
        result("a", Outcome::Agree, Outcome::Agree),
        result("b", gap(), Outcome::Agree),
    ];
    let listed: Exclusions = [(
        ("b".to_string(), "h".to_string()),
        row("import: no native layout for Map"),
    )]
    .into();
    let report = native::judge(&results, &listed, 2, true);
    assert!(report.ok(), "{report:?}");
    assert_eq!(report.pass[&Class::Heap], 1);
    assert_eq!(report.pass[&Class::Llvm], 2);
    // Unlisted.
    let report = native::judge(&results, &Exclusions::new(), 2, true);
    assert_eq!(report.unlisted.len(), 1, "{report:?}");
    // Listed under another reason.
    let other: Exclusions = [(
        ("b".to_string(), "h".to_string()),
        row("import: something else"),
    )]
    .into();
    assert_eq!(native::judge(&results, &other, 2, true).unlisted.len(), 1);
    // Stale: the configuration agrees now, the case left the baseline, or
    // the matrix no longer has the configuration.
    let mut stale = listed.clone();
    stale.insert(
        ("a".to_string(), "l".to_string()),
        row("import: no native layout for Map"),
    );
    stale.insert(("gone".to_string(), "h".to_string()), row("x"));
    stale.insert(("b".to_string(), "renamed".to_string()), row("x"));
    let report = native::judge(&results, &stale, 2, true);
    assert_eq!(report.stale.len(), 3, "{report:?}");
    // A mismatch is never unavailable: its class is a known bug.
    let mismatch = result("c", wrong(b""), Outcome::Agree);
    assert_eq!(mismatch.exclusions(&[Class::Heap])[0].1.class, "known-bug");
}

/// A row excludes one case under one configuration: another configuration of
/// the same engine that starts disagreeing is a new, unlisted failure.
#[test]
fn a_row_for_one_configuration_never_hides_another_of_its_engine() {
    let mut case = result("a", Outcome::Agree, gap_at_o0());
    case.outcomes
        .push(("l-O3".into(), Class::Llvm, Outcome::Agree));
    let listed: Exclusions =
        native::parse_exclusions(&native::render_exclusions(&[case.clone()])).unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(native::judge(std::slice::from_ref(&case), &listed, 3, true).ok());
    // The same case now miscompiles at O3 too.
    case.outcomes[2].2 = wrong(b"miscompiled");
    let report = native::judge(&[case], &listed, 3, true);
    assert_eq!(report.unlisted.len(), 1, "{report:?}");
    assert!(
        report.unlisted[0].starts_with("a under l-O3: observed"),
        "{report:?}"
    );
}

fn gap_at_o0() -> Outcome {
    unavailable("Unsupported(\"limit\")")
}

/// Each mismatch's reason is its own: a listed wrong answer that changes is
/// unlisted again.
#[test]
fn a_listed_mismatch_whose_wrong_answer_changes_is_unlisted() {
    let before = result("a", Outcome::Agree, wrong(b"1\n"));
    let after = result("a", Outcome::Agree, wrong(b"2\n"));
    let listed = native::parse_exclusions(&native::render_exclusions(&[before.clone()])).unwrap();
    assert!(native::judge(&[before.clone()], &listed, 2, true).ok());
    let report = native::judge(&[after.clone()], &listed, 2, true);
    assert_eq!(report.unlisted.len(), 1, "{report:?}");
    let reason = |r: &CaseResult| r.exclusions(&[Class::Llvm])[0].1.reason.clone();
    assert_ne!(reason(&before), reason(&after));
    assert!(reason(&before).starts_with("observed exit 0 | output \"1\\n\""));
}

/// The checked-in exclusions parse, are sorted, and every row names an
/// engine, an exclusion class and a reason; the smoke and topology lists
/// name files that exist and have no exclusion.
#[test]
fn the_checked_in_lists_are_well_formed() {
    let root = root();
    let exclusions = native::read_exclusions(&root).unwrap();
    for ((id, _), exclusion) in &exclusions {
        assert!(root.join(id).is_file(), "{id}");
        assert!(
            [
                "m2-unavailable",
                "native-unavailable",
                "compiler-error",
                "known-bug"
            ]
            .contains(&exclusion.class.as_str()),
            "{id}: {}",
            exclusion.class
        );
        assert!(!exclusion.reason.is_empty(), "{id}");
    }
    for list in [
        "native-tests/regressions/smoke.txt",
        "native-tests/regressions/memcheck.txt",
        "native-tests/topology/cases.txt",
    ] {
        let text = std::fs::read_to_string(root.join(list)).unwrap();
        for id in text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            assert!(root.join(id).is_file(), "{list}: {id}");
            assert!(
                !exclusions.keys().any(|(case, _)| case == id),
                "{list}: {id} is excluded"
            );
        }
    }
}

/// Generated programs are deterministic, valid and run by the reference; a
/// directed failure fails by its rule after output.
#[test]
fn generated_programs_are_valid_and_directed_failures_fail_by_their_rule() {
    use hirsel_conformance::native::generate::{FAULTS, Fault, program};
    on_compiler(|| {
        for seed in 0..12 {
            let source = program(seed, None);
            assert_eq!(source, program(seed, None));
            let observed = artifact::reference(&source, None)
                .unwrap_or_else(|| panic!("seed {seed} does not run:\n{source}"));
            assert_eq!(observed.exit_code, Some(0), "{source}");
            let text = String::from_utf8_lossy(&observed.output).into_owned();
            assert!(text.contains("end") && text.contains("drop "), "{source}");
            let fault = FAULTS[(seed % 5) as usize];
            let source = program(seed, Some(fault));
            let observed = artifact::reference(&source, None)
                .unwrap_or_else(|| panic!("seed {seed} {fault:?} does not run:\n{source}"));
            let rule = match fault {
                Fault::Overflow | Fault::Underflow | Fault::DivideByZero => "SEM-1",
                Fault::Index => "MEM-14",
                Fault::Requires => "FACT-1",
            };
            assert_eq!(observed.failures[0].rule, rule, "{source}");
            assert!(!observed.output.ends_with(b"end"), "{source}");
            assert!(
                !String::from_utf8_lossy(&observed.output).contains("end"),
                "{source}"
            );
            // The live resource's finalizer ran during the failure.
            let text = String::from_utf8_lossy(&observed.output);
            assert!(text.contains("drop "), "{source}");
        }
    });
}

/// The larger generated window is reserved for the on-demand full gate.
#[test]
#[ignore = "full gate: generated program validity window"]
fn full_gate_generated_program_window_runs_by_the_reference() {
    on_compiler(|| {
        let seed = std::env::var("PROPTEST_RNG_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        for case in hirsel_conformance::native::generate::cases(seed, 64) {
            let observed = artifact::reference(&case.source, None)
                .unwrap_or_else(|| panic!("{} does not run:\n{}", case.id, case.source));
            assert_eq!(
                observed.exit_code,
                Some(if case.id.contains("success-") { 0 } else { 2 }),
                "{}",
                case.id
            );
        }
    });
}

#[test]
fn stored_configuration_retains_independent_seeds_and_baseline_names() {
    for config in hirsel_driver::native::full_matrix()
        .into_iter()
        .chain(hirsel_driver::native::topology_matrix(20))
    {
        let baseline = config.name();
        assert!(!baseline.contains("/hash-"));
        assert_eq!(artifact::configuration(&baseline).unwrap().name(), baseline);
        for launch in [
            hirsel_driver::native::LaunchInputs {
                hash_seed: u64::MAX,
                ..Default::default()
            },
            hirsel_driver::native::LaunchInputs {
                hash_seed: 2,
                random_seed: 3,
                test_seed: 5,
                scheduler: None,
                fault_seed: None,
            },
        ] {
            let mut seeded = config.clone();
            seeded.options.launch = launch.clone();
            let name = seeded.name();
            let restored = artifact::configuration(&name).unwrap();
            assert_eq!(restored.options.launch, launch);
            assert_eq!(restored.name(), name);
        }
        for suffix in [
            "/hash-0",
            "/hash-1/hash-1",
            "/hash--1",
            "/hash-18446744073709551616",
            "/test-1/hash-1",
            "/run-é",
            "/run-00",
        ] {
            assert!(artifact::configuration(&format!("{baseline}{suffix}")).is_none());
        }
    }
}

fn fresh_parent(name: &str) -> PathBuf {
    let parent = std::env::temp_dir().join(format!(
        "hirsel-native-scratch-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&parent);
    parent
}

/// Two runs in one checkout (the same executable) get disjoint scratch roots,
/// so one run's job directories never hold the other's observation files.
#[test]
fn concurrent_runs_get_disjoint_scratch_roots() {
    use hirsel_conformance::native::scratch::RunRoot;
    let parent = fresh_parent("disjoint");
    let first = RunRoot::create_in(&parent).unwrap();
    let second = RunRoot::create_in(&parent).unwrap();
    assert_ne!(first.dir(), second.dir());
    let (a, b) = (first.join("full").unwrap(), second.join("full").unwrap());
    assert!(!a.starts_with(second.dir()) && !b.starts_with(first.dir()));
    std::fs::write(a.join("observation"), "first").unwrap();
    std::fs::write(b.join("observation"), "second").unwrap();
    assert_eq!(
        std::fs::read_to_string(a.join("observation")).unwrap(),
        "first"
    );
    // A third run prunes neither live root.
    let third = RunRoot::create_in(&parent).unwrap();
    assert!(first.dir().is_dir() && second.dir().is_dir());
    // A finished run's root is removed by the next run, and a live one kept.
    let finished = first.dir().to_path_buf();
    drop(first);
    let fourth = RunRoot::create_in(&parent).unwrap();
    assert!(!finished.exists() && !finished.with_extension("lock").exists());
    assert!(second.dir().is_dir() && third.dir().is_dir() && fourth.dir().is_dir());
    drop((second, third, fourth));
    std::fs::remove_dir_all(&parent).unwrap();
}

#[test]
fn clock_fixture_recordings_replay_timer_and_script_waits() {
    // Corpus execution checks results; this control checks the launcher's fixture
    // forwarding and the completion tape's park/ready protocol independently.
    on_compiler(|| {
        let matrix = hirsel_driver::native::smoke_matrix();
        let toolchain = hirsel_driver::native::Toolchain::for_matrix(
            &native::runtime_cache().unwrap(),
            &matrix,
        )
        .unwrap();
        let scratch = native::scratch::RunRoot::create_in(
            &std::env::temp_dir().join("hirsel-clock-fixture-replay"),
        )
        .unwrap();
        for name in [
            "tied-timers",
            "deadline-net",
            "deadline-child",
            "process-races",
        ] {
            let id = format!("conformance/CONC/CONC-3-clock-{name}-run.hir");
            let path = root().join(&id);
            let source = std::fs::read_to_string(&path).unwrap();
            let dir = scratch.join(name).unwrap();
            let result = native::replay_source_with_config(
                &id,
                &source,
                Some(&path),
                &toolchain,
                &dir,
                Default::default(),
            );
            assert!(result.snapshot.is_none(), "{id}: {:?}", result.snapshot);
            assert_eq!(result.outcomes.len(), 1, "{id}");
            assert!(
                matches!(result.outcomes[0].2, Outcome::Agree),
                "{id}: {:?}",
                result.outcomes
            );
        }
    });
}

#[test]
fn cranelift_does_not_hide_a_failed_llvm_control() {
    for id in [
        "native-tests/library/file-walk.hir",
        "native-tests/regressions/compiler-error.hir",
        "conformance/CAP/control.hir",
    ] {
        let mut failed = result(id, Outcome::Agree, unavailable("LLVM compiler error"));
        failed
            .outcomes
            .push(("c".into(), Class::Cranelift, Outcome::Agree));
        assert!(
            !native::cranelift::judge(&[failed], &Default::default(), &Default::default()).ok(),
            "an unlisted LLVM failure must fail: {id}"
        );
    }
}

#[test]
fn cranelift_llvm_failures_require_exact_native_exclusion_rows() {
    let parity = Default::default();
    for failure in [
        unavailable("LLVM compiler error"),
        unavailable("native executable terminated by signal 11"),
        wrong(b"wrong answer"),
    ] {
        let failed = result("a", Outcome::Agree, failure);
        let listed =
            native::parse_exclusions(&native::render_exclusions(&[failed.clone()])).unwrap();
        assert!(native::cranelift::judge(&[failed.clone()], &parity, &listed).ok());
        let mut changed = listed.clone();
        changed
            .values_mut()
            .next()
            .unwrap()
            .reason
            .push_str(" changed");
        assert!(!native::cranelift::judge(&[failed.clone()], &parity, &changed).ok());
        let wrong_config = [(
            ("a".into(), "h".into()),
            listed.values().next().unwrap().clone(),
        )]
        .into();
        assert!(!native::cranelift::judge(&[failed], &parity, &wrong_config).ok());
        let passing = result("a", Outcome::Agree, Outcome::Agree);
        assert!(!native::cranelift::judge(&[passing], &parity, &listed).ok());
    }
    let mut snapshot = result("snapshot", Outcome::Agree, Outcome::Agree);
    snapshot.snapshot = Some("a native stage panicked".into());
    snapshot.outcomes.clear();
    assert!(!native::cranelift::judge(&[snapshot.clone()], &parity, &Exclusions::new()).ok());
    let listed = native::parse_exclusions(&native::render_exclusions(&[snapshot.clone()])).unwrap();
    assert!(native::cranelift::judge(&[snapshot], &parity, &listed).ok());
    // A missing control cannot authorize a parity result, even with no failure recorded.
    let mut missing = result("missing", Outcome::Agree, Outcome::Agree);
    missing
        .outcomes
        .retain(|(_, class, _)| *class != Class::Llvm);
    let report = native::cranelift::judge(&[missing], &parity, &Exclusions::new());
    assert!(!report.ok());
    assert_eq!(report.llvm.pass[&Class::Llvm], 0);
    assert!(report.llvm.unlisted[0].contains("no LLVM run"));
}

#[test]
fn cranelift_keeps_the_parity_ratchet_when_the_llvm_control_agrees() {
    let mut case = result("a", Outcome::Agree, Outcome::Agree);
    case.outcomes
        .push(("c".into(), Class::Cranelift, wrong(b"wrong answer")));
    let llvm_exclusions = Exclusions::new();
    assert!(!native::cranelift::judge(&[case.clone()], &Default::default(), &llvm_exclusions).ok());
    let parity = native::cranelift::parse(&native::cranelift::render(&[case.clone()])).unwrap();
    let report = native::cranelift::judge(&[case.clone()], &parity, &llvm_exclusions);
    assert!(report.ok(), "{report:?}");
    assert_eq!(report.gated, 1);
    assert_eq!(report.passing, 0);
    case.outcomes[2].2 = Outcome::Agree;
    assert!(!native::cranelift::judge(&[case.clone()], &parity, &llvm_exclusions).ok());
    let report = native::cranelift::judge(&[case], &Default::default(), &llvm_exclusions);
    assert!(report.ok(), "{report:?}");
    assert_eq!(report.passing, 1);
}

#[test]
fn compiled_only_library_oracles_skip_interpretation_on_both_backends() {
    on_compiler(|| {
        let id = "native-tests/library/oracle-expected-stdout.hir";
        let case = native::cranelift::cases(&root())
            .unwrap()
            .into_iter()
            .find(|case| case.id == id)
            .unwrap();
        let mut matrix = native::cranelift::matrix(
            hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu(),
        );
        matrix.push(hirsel_driver::native::smoke_matrix().remove(0));
        let toolchain = hirsel_driver::native::Toolchain::for_matrix(
            &native::runtime_cache().unwrap(),
            &matrix,
        )
        .unwrap();
        let scratch = native::scratch::RunRoot::create_in(&fresh_parent("stdout-oracle")).unwrap();
        let result = case.run(&matrix, &toolchain, &scratch.join("oracle").unwrap());
        assert!(result.snapshot.is_none(), "{:?}", result.snapshot);
        let expected = result.reference.as_ref().unwrap();
        assert_eq!(expected.exit_code, Some(0), "{expected:?}");
        assert_eq!(expected.output, b"499999500000\n");
        assert_eq!(result.outcomes.len(), 2);
        assert!(
            result
                .outcomes
                .iter()
                .all(|(_, class, _)| *class != Class::Heap)
        );
        assert!(
            result
                .outcomes
                .iter()
                .all(|(_, _, outcome)| matches!(outcome, Outcome::Agree)),
            "{:?}",
            result.outcomes
        );
    });
}

#[test]
fn stdout_oracles_require_library_paths_reasons_and_exact_sidecar_bytes() {
    let parent = fresh_parent("oracle-header");
    let id = "native-tests/library/control.hir";
    let path = parent.join(id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    std::fs::write(path.with_extension("stdout"), [0, 255, b'\n']).unwrap();
    let header = "// oracle: expected-stdout\n// oracle-reason: independent reference vectors\n";
    let case = native::Case {
        id: id.into(),
        source: header.into(),
        path: Some(path.clone()),
    };
    let expected = case.expected_stdout().unwrap().unwrap();
    assert_eq!(expected.output, [0, 255, b'\n']);
    assert_eq!(expected.exit_code, Some(0));
    assert!(!expected.blocked && expected.budget.is_none() && expected.failures.is_empty());
    let ordinary = native::Case {
        source: "// ordinary interpreted control\n".into(),
        ..case.clone()
    };
    assert!(ordinary.expected_stdout().unwrap().is_none());
    for source in [
        "// oracle: expected-stdout\n",
        "// oracle: expected-stdout\n// oracle-reason: \n",
        "// oracle-reason: no oracle\n",
        "// oracle: unknown\n// oracle-reason: reason\n",
        "// oracle expected-stdout\n",
        &format!("{header}// oracle: expected-stdout\n"),
        &format!("{header}// oracle-reason: duplicate\n"),
        &format!("pub fn main() {{}}\n{header}"),
    ] {
        let invalid = native::Case {
            source: source.into(),
            ..case.clone()
        };
        assert!(invalid.expected_stdout().is_err(), "{source}");
    }
    for id in [
        "conformance/CAP/control.hir",
        "native-tests/regressions/control.hir",
        "native-tests/library/../control.hir",
        "/native-tests/library/control.hir",
    ] {
        let outside = native::Case {
            id: id.into(),
            ..case.clone()
        };
        assert!(outside.expected_stdout().is_err(), "{id}");
    }
    let no_path = native::Case {
        path: None,
        ..case.clone()
    };
    assert!(no_path.expected_stdout().is_err());
    let outside_path = parent.join("outside.hir");
    std::fs::write(&outside_path, "").unwrap();
    let outside = native::Case {
        path: Some(outside_path.clone()),
        ..case.clone()
    };
    assert!(outside.expected_stdout().is_err());
    #[cfg(unix)]
    {
        let link = path.with_file_name("alias.hir");
        std::os::unix::fs::symlink(&outside_path, &link).unwrap();
        let alias = native::Case {
            id: "native-tests/library/alias.hir".into(),
            path: Some(link),
            ..case.clone()
        };
        assert!(alias.expected_stdout().is_err());
    }
    std::fs::write(path.with_extension("stdout"), []).unwrap();
    assert!(case.expected_stdout().unwrap().unwrap().output.is_empty());
    std::fs::remove_file(path.with_extension("stdout")).unwrap();
    assert!(case.expected_stdout().unwrap_err().contains(".stdout"));
    assert!(ordinary.expected_stdout().unwrap().is_none());
    std::fs::remove_dir_all(parent).unwrap();
}

#[test]
fn stdout_oracles_reject_wrong_native_answers_and_keep_unmarked_references() {
    on_compiler(|| {
        let id = "native-tests/library/oracle-expected-stdout.hir";
        let path = root().join(id);
        let mut case = native::Case {
            id: id.into(),
            source: std::fs::read_to_string(&path).unwrap(),
            path: Some(path),
        };
        let matrix = native::cranelift::matrix(
            hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu(),
        );
        let toolchain = hirsel_driver::native::Toolchain::for_matrix(
            &native::runtime_cache().unwrap(),
            &matrix,
        )
        .unwrap();
        let scratch =
            native::scratch::RunRoot::create_in(&fresh_parent("oracle-controls")).unwrap();
        let heap = [hirsel_driver::native::smoke_matrix().remove(0)];
        let unrun = case.run(&heap, &toolchain, &scratch.join("heap-only").unwrap());
        assert!(
            unrun
                .snapshot
                .unwrap()
                .contains("requires a compiled native configuration")
        );
        let suite = native::Case {
            source: format!("// expect: test\n{}", case.source),
            ..case.clone()
        };
        let refused = suite.run(&matrix, &toolchain, &scratch.join("suite").unwrap());
        assert!(refused.snapshot.unwrap().contains("main entry"));
        case.source = case.source.replace("1000000u64", "4u64");
        let wrong = case.run(&matrix, &toolchain, &scratch.join("wrong").unwrap());
        assert!(wrong.snapshot.is_none(), "{:?}", wrong.snapshot);
        assert_eq!(wrong.outcomes.len(), 2);
        assert!(
            wrong
                .outcomes
                .iter()
                .all(|(_, _, outcome)| matches!(outcome, Outcome::Mismatch(_))),
            "{:?}",
            wrong.outcomes
        );
        assert!(!native::cranelift::judge(&[wrong], &Default::default(), &Exclusions::new()).ok());
        case.source = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"ordinary\\n\")\n Ok(()) }".into();
        let ordinary = case.run(&matrix, &toolchain, &scratch.join("ordinary").unwrap());
        assert!(ordinary.snapshot.is_none(), "{:?}", ordinary.snapshot);
        assert_eq!(ordinary.reference.as_ref().unwrap().output, b"ordinary\n");
        assert!(
            ordinary
                .outcomes
                .iter()
                .all(|(_, _, outcome)| matches!(outcome, Outcome::Agree)),
            "{:?}",
            ordinary.outcomes
        );
    });
}
