use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn header_diagnostics_point_to_the_offending_name_and_earlier_binding() {
    with_compiler_stack(|| {
        for (source, rule, primary, secondary) in [
            ("fn f(a:u8) uses {zz} {}", "LANG-4", "zz", None),
            ("fn f[T,T](a:T) {}", "GEN-1", "T]", None),
            ("fn f(x a:u8,x b:u8) {}", "LANG-12", "x b", None),
            ("type A = X() | X", "LANG-7", "X", None),
            ("fn f(a:u8,a:u8) {}", "LANG-4", "a:u8)", Some("a:u8,")),
            (
                "fn f(a:u8)->u8 { let a:u8=2u8; a }",
                "LANG-4",
                "a:u8=",
                Some("a:u8)"),
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("header.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(diagnostic.rule.as_str(), rule, "{source}");
            let start = source.rfind(primary).unwrap() as u32;
            let name = primary.split([':', ' ', ']']).next().unwrap();
            assert_eq!(
                {
                    let at = output.sources.resolve(diagnostic.primary);
                    (at.start, at.end)
                },
                (start, start + name.len() as u32),
                "{source}"
            );
            if let Some(previous) = secondary {
                assert_eq!(diagnostic.secondary.len(), 1, "{source}");
                let span = output.sources.resolve(diagnostic.secondary[0].span);
                assert_eq!(
                    (span.start, span.end),
                    (
                        source.find(previous).unwrap() as u32,
                        source.find(previous).unwrap() as u32 + 1
                    ),
                    "{source}"
                );
            }
        }
    });
}

// Corpus headers pin rule/site; this pins the diagnostic's actionable text.
#[test]
fn unknown_variant_lists_the_expected_types_variants() {
    with_compiler_stack(|| {
        for (source, expected) in [
            (
                "fn lookup() -> Result[u64, FileError] { Error(Fault()) }",
                "Fault is not a variant of FileError; variants: FileFault, Canceled, NotGranted, NotFound, PermissionDenied, AlreadyExists, IsDirectory, NotDirectory, DirectoryNotEmpty, CrossDevice, Truncated",
            ),
            (
                "fn lookup() { let x = Fault() }",
                "bare variant needs an unambiguous expected type",
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("variant.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(diagnostic.rule.as_str(), "LANG-2");
            assert_eq!(diagnostic.message, expected);
        }
    });
}

#[test]
fn near_miss_names_cover_scopes_case_underscores_and_renames() {
    with_compiler_stack(|| {
        for (source, before, after, help) in [
            (
                "fn f(length:u64)->u64 { lenght }",
                "lenght",
                "length",
                "help: did you mean `length`?",
            ),
            (
                "type Record { value:u64 }\nfn f(x:Recrod) {}",
                "Recrod",
                "Record",
                "help: did you mean `Record`?",
            ),
            (
                "fn f[Element](x:Elemnet) {}",
                "Elemnet",
                "Element",
                "help: did you mean `Element`?",
            ),
            (
                "fn f(is_empty:bool)->bool { isEmpty }",
                "isEmpty",
                "is_empty",
                "help: did you mean `is_empty`?",
            ),
            (
                "fn f()->u64 { bytes.count(b\"abc\") }",
                "count",
                "len",
                "help: did you mean `bytes.len`?",
            ),
            (
                "fn f()->u64 { byets.len(b\"abc\") }",
                "byets",
                "bytes",
                "help: did you mean `bytes`?",
            ),
            (
                "fn f()->u64 { core@1/bytes/lne(b\"abc\") }",
                "lne",
                "len",
                "help: did you mean `core@1/bytes/len`?",
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("near-miss.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostic = output.diagnostics.first().unwrap();
            assert_eq!(
                diagnostic.notes,
                [help],
                "{source}: {:?}",
                output.diagnostics
            );
            let fix = diagnostic.fix.as_ref().expect("unambiguous edit");
            assert_eq!(engine.sources.slice(fix.span), Some(before), "{source}");
            assert_eq!(fix.replacement, after);
            let mut repaired = source.to_owned();
            let at = engine.sources.resolve(fix.span);
            repaired.replace_range(at.start as usize..at.end as usize, &fix.replacement);
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("near-miss-fixed.hir", repaired).unwrap();
            let fixed = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                !fixed
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == hirsel_diag::Severity::Error),
                "{source}: {:?}",
                fixed.diagnostics
            );
        }
    });
}

#[test]
fn near_miss_help_refreshes_when_a_candidate_is_added_or_removed() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let main = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: default_context().package,
            modules: vec![],
            name: "example".into(),
        });
        for (declaration, expected) in [
            ("", None),
            (
                "fn length()->u64 { 1 }",
                Some("help: did you mean `length`?"),
            ),
            ("", None),
        ] {
            let source = format!("{declaration}\nfn example()->u64 {{ lenght() }}");
            let file = engine.sources.add("candidate.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let Err(QueryError::Diagnostics(diagnostics)) = engine.resolve_definition(&main) else {
                panic!("typo must fail");
            };
            let diagnostic = diagnostics.first().unwrap();
            assert_eq!(diagnostic.notes.first().map(String::as_str), expected);
        }
    });
}

fn sendability_check(source: &str) -> CheckOutput {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("task-result.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}

const TASK_RESULT: &str = include_str!("../../../../examples/kvd/repros/incident-3.hir");

#[test]
fn task_result_sendability_folds_both_requirements() {
    with_compiler_stack(|| {
        let output = sendability_check(TASK_RESULT);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-4");
        assert_eq!(d.severity, hirsel_diag::Severity::Error);
        assert!(d.message.starts_with("each actual generic occurrence"));
        let primary = output.sources.slice(d.primary).unwrap();
        assert!(primary.starts_with("task.async("), "{primary}");
        assert!(d.root_cause.is_some());
        assert!(
            d.secondary
                .iter()
                .any(|l| l.message == "generic parameter instantiation failed")
        );
        assert!(d.notes.iter().any(|n| {
            n.starts_with("this also fails: MODE-4: the callback result must be sendable")
        }));
        assert_eq!(d.notes.iter().filter(|n| n.starts_with("help:")).count(), 1);
        for rendered in [
            d.render_human(&output.sources),
            d.to_json(&output.sources).to_string(),
        ] {
            for text in [
                "each actual generic occurrence",
                "the callback result must be sendable",
                "Result.Error[0].E.Bad(String)",
                "declared callback result",
                "unique work()",
            ] {
                assert!(rendered.contains(text), "{rendered}");
            }
        }
        let json = d.to_json(&output.sources);
        let at = output.sources.resolve(d.primary);
        assert_eq!(json["primary"]["start"], at.start);
        assert_eq!(json["primary"]["end"], at.end);
        let fix = d.fix.as_ref().expect("existing unique-result repair");
        assert_eq!(fix.replacement, "unique ");
        let at = output.sources.resolve(fix.span);
        assert_eq!(json["fix"]["span"]["start"], at.start);
        assert_eq!(at.start, at.end);
        let mut repaired = TASK_RESULT.to_owned();
        repaired.insert_str(at.start as usize, &fix.replacement);
        assert!(repaired.contains("unique work()"));
        let fixed = sendability_check(&repaired);
        assert!(fixed.diagnostics.is_empty(), "{:?}", fixed.diagnostics);
    });
}

#[test]
fn task_result_sendability_folds_isolate_and_named_callbacks() {
    with_compiler_stack(|| {
        for callback in ["fn() -> Result[(), E] { work() }", "callback"] {
            let source = format!(
                "type E = Bad(String)\nfn work() -> Result[(), E] {{ Ok(()) }}\nfn callback() -> Result[(), E] {{ work() }}\nfn example(tasks: Tasks) {{ let _ = task.isolate(tasks, {callback}) }}"
            );
            let output = sendability_check(&source);
            let [d] = &output.diagnostics[..] else {
                panic!("{:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "MODE-4");
            assert!(d.message.starts_with("each actual generic occurrence"));
            assert!(d.root_cause.is_some());
            assert_eq!(
                d.notes
                    .iter()
                    .filter(|n| n.starts_with("this also fails:"))
                    .count(),
                1
            );
            assert!(
                output
                    .sources
                    .slice(d.primary)
                    .unwrap()
                    .starts_with("task.isolate(")
            );
            if callback == "callback" {
                assert!(d.fix.is_none(), "a named callback is not a local edit");
            } else {
                let fix = d.fix.as_ref().unwrap();
                let at = output.sources.resolve(fix.span);
                let mut repaired = source;
                repaired.insert_str(at.start as usize, &fix.replacement);
                let output = sendability_check(&repaired);
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            }
        }
    });
}

// Independent copies have equal contents but distinct ownership (Rust std
// alloctests/tests/vec.rs:257-292, f6c1107). Calls likewise keep distinct causes.
#[test]
fn task_result_sendability_keeps_two_callbacks_and_failed_fields_separate() {
    with_compiler_stack(|| {
        for different_fields in [false, true] {
            let mut source = TASK_RESULT.replace(
                "      let _ = task.await(s, take h)",
                "      let _ = task.await(s, take h)\n      let h2 = task.async(s, fn() -> Result[(), E2] { work2() })\n      let _ = task.await(s, take h2)",
            );
            if different_fields {
                source = source.replace(
                    "type E = Bad(String)",
                    "type Left { text: String }\ntype E = Bad(Left)",
                );
                source += "\ntype Right { detail: String }\ntype E2 = Bad(Right)\nfn work2() -> Result[(), E2] { Ok(()) }\n";
            } else {
                source = source.replace("E2", "E").replace("work2()", "work()");
            }
            let output = sendability_check(&source);
            assert_eq!(output.diagnostics.len(), 2, "{:?}", output.diagnostics);
            let a = &output.diagnostics[0];
            let b = &output.diagnostics[1];
            assert_ne!(a.root_cause, b.root_cause);
            for d in [a, b] {
                assert_eq!(d.rule.as_str(), "MODE-4");
                assert!(d.message.starts_with("each actual generic occurrence"));
                assert_eq!(
                    d.notes
                        .iter()
                        .filter(|n| n.starts_with("this also fails:"))
                        .count(),
                    1
                );
            }
            if different_fields {
                assert!(a.message.contains("Left.text"), "{a:?}");
                assert!(b.message.contains("Right.detail"), "{b:?}");
            }
        }
    });
}

#[test]
fn task_result_sendability_keeps_generic_only_and_earlier_errors() {
    with_compiler_stack(|| {
        let generic = "fn text() -> String { \"text\" }\nfn need[T: sendable](f: fn() -> T) {}\nfn example() { need(fn() -> String { text() }) }";
        let output = sendability_check(generic);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-4");
        assert!(d.message.starts_with("each actual generic occurrence"));
        assert!(d.root_cause.is_none());
        assert!(!d.notes.iter().any(|n| n.starts_with("this also fails:")));

        let source = TASK_RESULT.replace(
            "      let h = task.async(",
            "      let _ = need(fn() -> String { text() })\n      let h = task.async(",
        ) + "\nfn text() -> String { \"text\" }\nfn need[T: sendable](f: fn() -> T) {}\n";
        let output = sendability_check(&source);
        assert_eq!(output.diagnostics.len(), 2, "{:?}", output.diagnostics);
        let first = &output.diagnostics[0];
        assert_eq!(first.rule.as_str(), "MODE-4");
        assert!(
            output
                .sources
                .slice(first.primary)
                .unwrap()
                .starts_with("need(")
        );
        assert!(first.root_cause.is_none());
        assert!(output.diagnostics[1].root_cause.is_some());
    });
}

#[test]
fn task_result_sendability_preserves_specialized_owners_and_accepted_twins() {
    with_compiler_stack(|| {
        for (bad, good, rule) in [
            (
                include_str!("../../../../conformance/CONC/CONC-5-result-sendable-bad.hir"),
                include_str!("../../../../conformance/CONC/CONC-5-result-sendable-ok.hir"),
                "CONC-5",
            ),
            (
                include_str!("../../../../conformance/CONC/CONC-6-open-receiver-payload-bad.hir"),
                include_str!("../../../../conformance/CONC/CONC-6-open-receiver-payload-ok.hir"),
                "CONC-6",
            ),
        ] {
            let output = sendability_check(bad);
            assert!(!output.diagnostics.is_empty());
            assert!(
                output.diagnostics.iter().all(|d| d.rule.as_str() == rule),
                "{:?}",
                output.diagnostics
            );
            if rule == "CONC-5" {
                assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
                assert!(
                    output.diagnostics[0]
                        .notes
                        .iter()
                        .any(|n| n.starts_with("this also fails: CONC-5:"))
                );
            }
            let output = sendability_check(good);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        }
    });
}

#[test]
fn task_result_sendability_uses_definition_identity_across_source_unit_order() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        // Equal definition-relative call offsets and spellings in two source
        // units must not give the two definitions one root cause.
        let (_, body) = TASK_RESULT.split_once("pub fn main").unwrap();
        std::fs::write(dir.path().join("a.hir"), format!("fn first{body}")).unwrap();
        std::fs::write(dir.path().join("b.hir"), format!("fn other{body}")).unwrap();
        std::fs::write(
            dir.path().join("types.hir"),
            "type E = Bad(String)\nfn work() -> Result[(), E] { Ok(()) }\npub fn main() {}",
        )
        .unwrap();
        let mut before = None;
        for paths in [
            ["a.hir", "b.hir", "types.hir"],
            ["types.hir", "b.hir", "a.hir"],
        ] {
            let manifest = dir.path().join("hirsel.json");
            let sources: Vec<_> = paths
                .iter()
                .map(|p| serde_json::json!({"path": p, "module": []}))
                .collect();
            std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"fold","version":"0"},"sources": sources,"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
            let mut program = load_program(&manifest).unwrap();
            let output = program.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert_eq!(output.diagnostics.len(), 2, "{:?}", output.diagnostics);
            assert_ne!(
                output.diagnostics[0].root_cause,
                output.diagnostics[1].root_cause
            );
            let rendered: Vec<_> = output
                .diagnostics
                .iter()
                .map(|d| {
                    assert_eq!(d.rule.as_str(), "MODE-4");
                    assert_eq!(
                        d.notes
                            .iter()
                            .filter(|n| n.starts_with("this also fails:"))
                            .count(),
                        1
                    );
                    let mut json = d.to_json(&output.sources);
                    json["primary"].as_object_mut().unwrap().remove("file_id");
                    for label in json["secondary"].as_array_mut().unwrap() {
                        label["span"].as_object_mut().unwrap().remove("file_id");
                    }
                    json["fix"]["span"]
                        .as_object_mut()
                        .unwrap()
                        .remove("file_id");
                    (d.root_cause, d.render_human(&output.sources), json)
                })
                .collect();
            if let Some(before) = &before {
                assert_eq!(before, &rendered);
            }
            before = Some(rendered);
        }
    });
}
