//! Bounded recovery-neighbor vectors (rust-analyzer parser tests.rs:61-130,
//! 3131751e), with deletion/scalar shrinking (proptest shrinking-basics.md:1-25,
//! 70cf37d). Inputs stay in Hirsel's semantic header grammar; no syntax fuzzing.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::SignatureKind;

struct Case {
    name: &'static str,
    owner: &'static str,
    rule: &'static str,
    source: &'static str,
    before: &'static str,
    after: &'static str,
}
const CASES: &[Case] = &[
    Case {
        name: "size_as_type",
        owner: "broken",
        rule: "GEN-1",
        source: "fn broken[n:size](value: (u64, n)) {}",
        before: "(u64, n)",
        after: "(u64, u64)",
    },
    Case {
        name: "kind",
        owner: "broken",
        rule: "GEN-1",
        source: "fn broken(value: Inline[u64, u64]) {}",
        before: "Inline[u64, u64]",
        after: "Inline[u64, 2]",
    },
    Case {
        name: "arity",
        owner: "broken",
        rule: "GEN-1",
        source: "fn broken(value: List[u64, u64]) {}",
        before: "List[u64, u64]",
        after: "List[u64]",
    },
    Case {
        name: "labels",
        owner: "broken",
        rule: "LANG-12",
        source: "fn broken(label first:u64, label second:u64) {}",
        before: "label second",
        after: "other second",
    },
    Case {
        name: "unknown_public",
        owner: "broken",
        rule: "LANG-4",
        source: "pub fn broken(value: Missing, marker:u64) {}",
        before: "Missing",
        after: "u64",
    },
    Case {
        name: "opaque_bound",
        owner: "Broken",
        rule: "GEN-1",
        source: "pub opaque type Broken: plain uses {} { value:String, marker:u64 }",
        before: ": plain",
        after: ": ()",
    },
    Case {
        name: "loop_association",
        owner: "View",
        rule: "SEM-4",
        source: "@loop_source(open, next)\ntype View { value:String, marker:u64 }\ntype Cursor { value:String }\nfn open(take v:View) -> Cursor in v { Cursor(value: copy v.value) }\nfn next(mut c:Cursor) -> Option[u64] { None }",
        before: "open(take v:View)",
        after: "open(v:View)",
    },
    Case {
        name: "format_association",
        owner: "View",
        rule: "SEM-3",
        source: "@format(render)\ntype View { value:u64 }\nfn render(take v:View, mut out:List[u8]) {}",
        before: "render(take v:View",
        after: "render(v:View",
    },
];
const DOWNSTREAM: &str = "\npub fn downstream() -> u8 uses {} { 7u8 }\n";

fn path(name: &str) -> DefPath {
    DefPath(hirsel_syntax::ItemPath {
        name: name.into(),
        ..entry_path(&default_context()).0
    })
}
fn parse(source: &str) -> (QueryEngine, hirsel_span::FileId) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("recovery.hir", source.into()).unwrap();
    let parsed = engine
        .parse_file(file, &default_context())
        .expect("parser-valid seed");
    assert!(
        parsed.diagnostics.is_empty(),
        "{source}: {:?}",
        parsed.diagnostics
    );
    (engine, file)
}
fn diagnostic(engine: &mut QueryEngine, owner: &str, rule: &str) -> (String, String) {
    match engine.item_signature(&path(owner)) {
        Err(QueryError::Diagnostics(errors)) => {
            assert_eq!(errors.len(), 1, "{errors:?}");
            let d = &errors[0];
            assert_eq!(d.rule.as_str(), rule, "{d:?}");
            assert_eq!(d.severity, hirsel_diag::Severity::Error);
            (
                d.message.clone(),
                engine.sources.slice(d.primary).unwrap().to_owned(),
            )
        }
        Err(error) => panic!("tooling/query failure is not a source diagnostic: {error:?}"),
        Ok(signature) => panic!(
            "invalid header published a signature: {:?}",
            signature.value
        ),
    }
}
fn downstream(engine: &mut QueryEngine, validate: bool) {
    let sig = engine.item_signature(&path("downstream")).unwrap();
    assert!(sig.diagnostics.is_empty());
    assert!(matches!(&sig.value.kind, SignatureKind::Function(f) if f.parameters.is_empty()));
    if validate {
        // Direct lowering validates the retained neighbor in this signature universe.
        engine.lower_to_tir(&path("downstream")).unwrap();
    }
}
fn source(case: &Case, stage: usize, repaired: bool) -> String {
    let mut text = if repaired {
        case.source.replace(case.before, case.after)
    } else {
        case.source.to_owned()
    };
    if stage == 0 {
        text = format!("type Padding {{ value:u64 }}\n{text}");
    }
    if stage == 2 {
        text = text.replace("u64", "u8");
    }
    text + DOWNSTREAM
}

#[test]
fn signature_recovery_22_headers_and_repaired_neighbors() {
    with_compiler_stack(|| {
        let mut count = 0;
        for case in &CASES[..6] {
            let mut original = None;
            for stage in 0..3 {
                for repaired in [false, true] {
                    count += 1;
                    let text = source(case, stage, repaired);
                    let (mut engine, file) = parse(&text);
                    if repaired {
                        let output = check_progress(&mut engine, file, &default_context()).unwrap();
                        assert!(
                            output.diagnostics.is_empty(),
                            "{} stage {stage}: {:?}",
                            case.name,
                            output.diagnostics
                        );
                        engine.item_signature(&path(case.owner)).unwrap();
                    } else {
                        let cause = diagnostic(&mut engine, case.owner, case.rule);
                        if stage == 0 {
                            original = Some(cause.0.clone());
                        }
                        assert_eq!(
                            Some(&cause.0),
                            original.as_ref(),
                            "{} stage {stage}",
                            case.name
                        );
                    }
                    downstream(&mut engine, true);
                }
            }
        }
        assert_eq!(count, 36);
    });
}

#[test]
fn signature_recovery_22_protocol_preflight_rejects_unused_bad_associations() {
    with_compiler_stack(|| {
        let mut count = 0;
        for case in &CASES[6..] {
            for stage in 0..3 {
                for repaired in [false, true] {
                    count += 1;
                    let text = source(case, stage, repaired);
                    let (mut engine, file) = parse(&text);
                    // Association signatures are intentionally checked in stage two.
                    engine.item_signature(&path(case.owner)).unwrap();
                    let output = check_progress(&mut engine, file, &default_context()).unwrap();
                    assert_eq!(output.completion, CheckCompletion::Complete);
                    let rules: Vec<_> =
                        output.diagnostics.iter().map(|d| d.rule.as_str()).collect();
                    assert_eq!(
                        rules,
                        if repaired { vec![] } else { vec![case.rule] },
                        "{} stage {stage}: {:?}",
                        case.name,
                        output.diagnostics
                    );
                    if !repaired {
                        assert!(
                            engine
                                .sources
                                .slice(output.diagnostics[0].primary)
                                .unwrap()
                                .starts_with('@')
                        );
                    }
                    // A malformed association invalidates the V universe. Its
                    // neighbor still has a signature; only the repair can lower.
                    downstream(&mut engine, repaired);
                }
            }
        }
        assert_eq!(count, 12);
    });
}

#[test]
fn signature_recovery_22_independent_errors_keep_their_primary_owner() {
    with_compiler_stack(|| {
        let a = "fn bad_kind(value:Inline[u8,u8]) {}";
        let b = "fn bad_label(label first:u8,label second:u8) {}";
        let mut baseline = None;
        for (first, second) in [(a, b), (b, a)] {
            let text = format!("{first}\n{second}{DOWNSTREAM}");
            let (mut engine, _) = parse(&text);
            let causes = [
                diagnostic(&mut engine, "bad_kind", "GEN-1"),
                diagnostic(&mut engine, "bad_label", "LANG-12"),
            ];
            assert_eq!(causes[1].1, "label");
            if let Some(old) = &baseline {
                assert_eq!(&causes, old);
            }
            baseline = Some(causes);
            downstream(&mut engine, true);
        }
    });
}
