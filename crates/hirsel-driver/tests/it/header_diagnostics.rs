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

#[test]
fn callback_mode_mismatch_labels_contracts_and_checks_the_adapter() {
    with_compiler_stack(|| {
        let source = "fn invoke(cb:fn(mut xs:List[u8])->u64)->u64 {let mut xs:List[u8]=[];cb(mut xs)}\nfn borrow(xs:List[u8])->u64 {list.len(xs)}\nfn caller()->u64{invoke(borrow)}";
        let output = sendability_check(source);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-7");
        assert!(
            d.message.contains("expected `fn(mut List[u8]) -> u64`"),
            "{d:?}"
        );
        assert!(d.message.contains("found `fn(List[u8]) -> u64`"), "{d:?}");
        assert!(
            d.notes.iter().any(|n| n
                .contains("parameter 1 (`xs`) requires `mut`, but the callback declares `borrow`")),
            "{d:?}"
        );
        assert_eq!(d.secondary.len(), 2, "{d:?}");
        assert_eq!(output.sources.slice(d.secondary[0].span), Some("borrow"));
        assert_eq!(output.sources.slice(d.secondary[1].span), Some("invoke"));
        for rendered in [
            d.render_human(&output.sources),
            d.to_json(&output.sources).to_string(),
        ] {
            for text in [
                "callback declared here",
                "receiving callable requirement",
                "callback_adapter",
                "borrow(xs)",
            ] {
                assert!(rendered.contains(text), "{rendered}");
            }
        }
        let recipe = d
            .notes
            .iter()
            .find_map(|n| {
                n.strip_prefix(
                    "help: Define a named adapter and pass `callback_adapter` instead:\n",
                )
            })
            .expect("complete recipe");
        let repaired = source.replace("invoke(borrow)", "invoke(callback_adapter)") + "\n" + recipe;
        let output = sendability_check(&repaired);
        assert!(
            output.diagnostics.is_empty(),
            "{repaired}: {:?}",
            output.diagnostics
        );
    });
}

#[test]
fn callback_mode_mismatch_covers_indirect_generic_resource_and_unsafe_repairs() {
    with_compiler_stack(|| {
        for (source, mode, adapter) in [
            (
                "fn invoke(cb:fn(mut xs:u64)->u64) {}\nfn read(xs:u64)->u64 {xs}\nfn caller(){let cb=read;invoke(cb)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(mut xs:u64)->u64) {}\nfn caller(cb:fn(xs:u64)->u64){invoke(cb)}",
                true,
                false,
            ),
            (
                "fn caller(receive:fn(cb:fn(mut xs:u64)->u64), cb:fn(xs:u64)->u64){receive(cb)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(mut xs:u64)->u64) {}\nfn read[T:plain](xs:T)->u64 {0}\nfn caller(){invoke(read)}",
                true,
                true,
            ),
            (
                "fn invoke[T](x:T,cb:fn(mut xs:T)->u64) {}\nfn read[T](xs:T)->u64 {0}\nfn caller[T](x:T){invoke(x,read)}",
                true,
                false,
            ),
            (
                "resource type R { n:u64 } drop(x) {}\nfn invoke(cb:fn(mut xs:R)->u64) {}\nfn read(xs:R)->u64 {xs.n}\nfn caller(){invoke(read)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(mut xs:once fn())->u64) {}\nfn read(xs:once fn())->u64 {0}\nfn caller(){invoke(read)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(mut xs:List[u8])->u64) {}\nfn consume(take xs:List[u8])->u64 {list.len(xs)}\nfn caller(){invoke(consume)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(xs:u64)->u64) {}\nfn change(mut xs:u64)->u64 {xs}\nfn caller(){invoke(change)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(mut xs:u64)->u64) {}\nfn read(xs:u64)->bool {true}\nfn caller(){invoke(read)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:@noalloc fn(mut xs:u64)->u64) {}\nfn read(xs:u64)->u64 {xs}\nfn caller(){invoke(read)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(mut xs:Stdin)->u64 uses {}) {}\nfn read(xs:Stdin)->u64 {0}\nfn caller(){invoke(read)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:once fn(mut xs:u64)->u64) {}\nfn caller(take cb:once fn(xs:u64)->u64){invoke(take cb)}",
                true,
                false,
            ),
            (
                "fn invoke(cb:fn(xs:u64)->bool) {}\nfn read(xs:u64)->u64 {xs}\nfn caller(){invoke(read)}",
                false,
                false,
            ),
        ] {
            let output = sendability_check(source);
            let [d] = &output.diagnostics[..] else {
                panic!("{source}: {:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "LANG-7", "{source}: {d:?}");
            assert_eq!(
                d.notes.iter().any(|n| n.starts_with("callback parameter")),
                mode,
                "{source}: {d:?}"
            );
            let recipe = d
                .notes
                .iter()
                .find_map(|n| n.split_once("instead:\n").map(|(_, code)| code));
            assert_eq!(recipe.is_some(), adapter, "{source}: {d:?}");
            if let Some(recipe) = recipe {
                let fixed =
                    source.replace("invoke(read)", "invoke(callback_adapter)") + "\n" + recipe;
                let output = sendability_check(&fixed);
                assert!(
                    output.diagnostics.is_empty(),
                    "{fixed}: {:?}",
                    output.diagnostics
                );
            }
            if source.contains("invoke(consume)") {
                assert!(
                    d.notes
                        .iter()
                        .any(|n| n.contains("must hand the caller's value back"))
                );
            }
        }
        for source in [
            "resource type R { n:u64 } drop(x) {}\nfn invoke(cb:fn(mut xs:R)->u64) {}\nfn read(xs:R)->u64 {xs.n}\nfn adapter(mut xs:R)->u64 {read(xs)}\nfn caller(){invoke(adapter)}",
            "fn invoke(cb:fn(mut xs:u64)->u64) {}\nfn read(xs:u64)->u64 {xs}\nfn adapter(mut xs:u64)->u64 {read(xs)}\nfn caller(){invoke(adapter)}",
            "fn invoke(cb:fn(mut a:u8,mut xs:u64)->u64) {}\nfn read(mut a:u8,xs:u64)->u64 {xs}\nfn adapter(mut a:u8,mut xs:u64)->u64 {read(mut a,xs)}\nfn caller(){invoke(adapter)}",
        ] {
            let output = sendability_check(source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
        }
        let source = "fn invoke(cb:fn(mut a:u8,mut xs:u64)->u64) {}\nfn read(mut a:u8,xs:u64)->u64 {xs}\nfn caller(){invoke(read)}";
        let output = sendability_check(source);
        assert!(
            output.diagnostics[0]
                .notes
                .iter()
                .any(|n| n.starts_with("callback parameter 2 (`xs`)")),
            "{:?}",
            output.diagnostics
        );
        let output = sendability_check(
            "fn consume(take xs:List[u8])->u64 {list.len(xs)}\nfn adapter(mut xs:List[u8])->u64 {consume(take xs)}",
        );
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str().starts_with("OWN-")),
            "{:?}",
            output.diagnostics
        );
    });
}

#[test]
fn callback_mode_mismatch_cites_imported_contracts_and_checks_qualified_adapter() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("callbacks.hir"),
            "pub fn invoke(cb:fn(mut xs:u64)->u64) {}\npub fn read(xs:u64)->u64 {xs}",
        )
        .unwrap();
        let main = dir.path().join("main.hir");
        let source = "pub fn main(){callbacks.invoke(callbacks.read)}";
        std::fs::write(&main, source).unwrap();
        let manifest = dir.path().join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"callbacks_test","version":"0"},"sources":[{"path":"main.hir","module":[]},{"path":"callbacks.hir","module":["callbacks"]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let mut program = load_program(&manifest).unwrap();
        let output = program.check(LoweringPolicy::Comptime).unwrap();
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-7");
        assert!(
            d.notes.iter().any(|n| n.contains("callback declared here")),
            "{d:?}"
        );
        assert!(
            d.notes
                .iter()
                .any(|n| n.contains("receiving callable requirement")),
            "{d:?}"
        );
        let recipe = d
            .notes
            .iter()
            .find_map(|n| n.split_once("instead:\n").map(|(_, code)| code))
            .unwrap();
        assert!(recipe.contains("callbacks.read(xs)"), "{recipe}");
        std::fs::write(
            &main,
            source.replace(
                "callbacks.invoke(callbacks.read)",
                "callbacks.invoke(callback_adapter)",
            ) + "\n"
                + recipe,
        )
        .unwrap();
        let mut program = load_program(&manifest).unwrap();
        let output = program.check(LoweringPolicy::Comptime).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    });
}

#[test]
fn callback_mode_mismatch_scalar_adapter_runs_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.hir"), "fn invoke(cb:fn(mut xs:u64)->u64)->u64 {let mut xs=42u64;let answer=cb(mut xs);xs+answer}\nfn read(xs:u64)->u64 {xs}\nfn callback_adapter(mut xs:u64)->u64 {read(xs)}\npub fn main()->Result[(),()]{if invoke(callback_adapter)==84 {Ok(())} else {Error(())}}").unwrap();
        let manifest = dir.path().join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"callback_runtime","version":"0"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let mut program = load_program(&manifest).unwrap();
        program.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        let reference = native::Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty());
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let scratch =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("callback-mode-repairs-20");
        std::fs::create_dir_all(&scratch).unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let outcomes = native::differential(
            &validated,
            &program.entry,
            &reference,
            &configurations,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

#[test]
fn callback_mode_mismatch_stdin_recipe_and_existing_api_alternative_are_checked() {
    with_compiler_stack(|| {
        for (declarations, adapter) in [
            ("", "stdin_adapter"),
            (
                "fn input() {}\nfn buffer() {}\nfn requested() {}\nfn stdin_adapter() {}\n",
                "stdin_adapter_2",
            ),
        ] {
            let source = format!(
                "{declarations}fn invoke(refill:fn(mut Stdin,mut List[u8],u64)->Result[u64,InputError]) {{}}\nfn caller(){{invoke(stdin.read)}}"
            );
            let output = sendability_check(&source);
            let [d] = &output.diagnostics[..] else {
                panic!("{:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "LANG-7");
            assert!(
                d.notes.iter().any(|n| n.contains("bufio.stdin_read")),
                "{d:?}"
            );
            let prefix =
                format!("help: For stdin, define this named adapter and pass `{adapter}`:\n");
            let recipe = d
                .notes
                .iter()
                .find_map(|n| n.strip_prefix(&prefix))
                .unwrap();
            if !declarations.is_empty() {
                for parameter in ["input_2", "buffer_2", "requested_2"] {
                    assert!(recipe.contains(parameter), "{recipe}");
                }
            }
            let repaired =
                source.replace("invoke(stdin.read)", &format!("invoke({adapter})")) + "\n" + recipe;
            let output = sendability_check(&repaired);
            assert!(
                output.diagnostics.is_empty(),
                "{repaired}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn callback_mode_mismatch_does_not_offer_adapters_for_effects_or_other_modes() {
    with_compiler_stack(|| {
        for source in [
            "fn invoke(cb:@noalloc fn(xs:u64)->u64) {}\nfn read(xs:u64)->u64 {xs}\nfn caller(){invoke(read)}",
            "fn invoke(cb:fn(xs:Stdin)->u64 uses {}) {}\nfn read(xs:Stdin)->u64 {0}\nfn caller(){invoke(read)}",
            "fn invoke(cb:fn()->unique String) {}\nfn read()->String {\"x\"}\nfn caller(){invoke(read)}",
            "fn invoke(cb:fn()) {}\nfn caller(take cb:once fn()){invoke(take cb)}",
        ] {
            let output = sendability_check(source);
            assert!(!output.diagnostics.is_empty(), "{source}");
            for d in &output.diagnostics {
                assert!(
                    !d.notes.iter().any(|n| n.contains("named adapter")),
                    "{source}: {d:?}"
                );
                assert!(
                    !d.notes.iter().any(|n| n.starts_with("callback parameter")),
                    "{source}: {d:?}"
                );
                assert_ne!(d.rule.as_str(), "LANG-4", "{source}: {d:?}");
            }
        }
    });
}

#[test]
fn callback_mode_mismatch_recipe_avoids_shadowing_callback_and_adapter_names() {
    with_compiler_stack(|| {
        let source = "fn invoke(cb:fn(mut state:u64)->u64) {}\nfn xs(item:u64)->u64 {item}\nfn callback_adapter() {}\nfn caller(){invoke(xs)}";
        let output = sendability_check(source);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        let recipe = d
            .notes
            .iter()
            .find_map(|n| n.split_once("instead:\n").map(|(_, code)| code))
            .unwrap();
        assert!(
            recipe.contains("fn callback_adapter_2(mut xs_2: u64)"),
            "{recipe}"
        );
        let fixed = source.replace("invoke(xs)", "invoke(callback_adapter_2)") + "\n" + recipe;
        let output = sendability_check(&fixed);
        assert!(
            output.diagnostics.is_empty(),
            "{fixed}: {:?}",
            output.diagnostics
        );
    });
}

#[test]
fn callback_mode_mismatch_bufio_stdin_witness_has_no_name_setup_errors() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = "fn read(mut r:bufio.Reader,mut input:Stdin,mut output:List[u8])->Result[u64,bufio.ReadError[InputError]]{bufio.read(mut r,mut input,stdin.read,mut output,4096)}\npub fn main() {}";
        let main = dir.path().join("main.hir");
        std::fs::write(&main, source).unwrap();
        let manifest = dir.path().join("hirsel.json");
        let std_manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(&manifest,serde_json::json!({"schema":1,"package":{"name":"callback_stdin","version":"0"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":std_manifest,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let mut program = load_program(&manifest).unwrap();
        let output = program.check(LoweringPolicy::Comptime).unwrap();
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "LANG-7");
        let recipe = d
            .notes
            .iter()
            .find_map(|n| {
                n.strip_prefix(
                    "help: For stdin, define this named adapter and pass `stdin_adapter`:\n",
                )
            })
            .unwrap();
        let repaired = source.replace(",stdin.read,", ",stdin_adapter,") + "\n" + recipe;
        let alternative = source.replace(
            "bufio.read(mut r,mut input,stdin.read,mut output,4096)",
            "bufio.stdin_read(input,mut r,mut output,4096)",
        );
        for accepted in [repaired, alternative] {
            std::fs::write(&main, &accepted).unwrap();
            let mut program = load_program(&manifest).unwrap();
            let output = program.check(LoweringPolicy::Comptime).unwrap();
            assert!(
                output.diagnostics.is_empty(),
                "{accepted}: {:?}",
                output.diagnostics
            );
        }
    });
}
