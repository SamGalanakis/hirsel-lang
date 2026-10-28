//! Private body summaries are evidence of direct calls, never callable types.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::{SignatureKind, UniqueResultRow};

fn check(source: &str) -> (QueryEngine, CheckOutput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    engine.verify = Verify::On;
    let file = engine
        .sources
        .add("private-helper.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    (engine, output)
}

#[test]
fn private_helper_uniqueness_59_mut_handback_and_fresh_result() {
    with_compiler_stack(|| {
        let source = "fn add(mut xs: List[u64]) { list.push(mut xs, 1) }\nfn code(n: u64) -> String { \"x\" }\nfn need(take xs: unique List[u64]) {}\nfn need_s(take s: unique String) {}\npub fn main() { let mut xs: unique List[u64] = []; add(mut xs); need(take xs); need_s(code(1)) }";
        let (mut engine, output) = check(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let code = input
            .signatures
            .values()
            .find(|s| s.definition.path().0.name == "code")
            .unwrap();
        assert!(
            code.metadata
                .inferred_unique
                .results
                .contains(&UniqueResultRow::Root)
        );
        let SignatureKind::Function(f) = &code.kind else {
            panic!()
        };
        assert_eq!(
            engine.types().row(f.result).modes.uniqueness,
            hirsel_types::Uniqueness::Aliased
        );
        engine.import_native_input(input).unwrap();
    });
}

#[test]
fn private_helper_uniqueness_59_negative_directness_and_constant_controls() {
    with_compiler_stack(|| {
        for source in [
            "fn add(mut xs: List[List[u64]], k: List[u64]) { list.push(mut xs, copy k) }\nfn need(take xs: unique List[List[u64]]) {}\npub fn main() { let mut xs: unique List[List[u64]] = []; add(mut xs, [1]); need(take xs) }",
            "pub fn add(mut xs: List[u64]) { list.push(mut xs, 1) }\nfn need(take xs: unique List[u64]) {}\npub fn main() { let mut xs: unique List[u64] = []; add(mut xs); need(take xs) }",
            "fn add(mut xs: List[u64]) { list.push(mut xs, 1) }\nfn need(take xs: unique List[u64]) {}\npub fn main() { let mut xs: unique List[u64] = []; let f: fn(mut List[u64]) -> () = add; f(mut xs); need(take xs) }",
            "const NAME: String = \"x\"\nfn code() -> String { NAME }\nfn need(take s: unique String) {}\npub fn main() { need(code()) }",
        ] {
            let (_, output) = check(source);
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "MODE-2"),
                "{source}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn private_helper_uniqueness_59_recursive_fixed_point_and_dropout() {
    with_compiler_stack(|| {
        let good = "fn a(mut xs: List[u64], n: u64) { if n == 0 { list.push(mut xs, 1) } else { b(mut xs, n - 1) } }\nfn b(mut xs: List[u64], n: u64) { a(mut xs, n) }\nfn need(take xs: unique List[u64]) {}\npub fn main() { let mut xs: unique List[u64] = []; a(mut xs, 2); need(take xs) }";
        let (_, output) = check(good);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let bad = format!(
            "const EMPTY: List[u64] = []\n{}",
            good.replace("list.push(mut xs, 1)", "xs = EMPTY")
        );
        let (_, output) = check(&bad);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MODE-2"),
            "{:?}",
            output.diagnostics
        );
    });
}

#[test]
fn private_helper_uniqueness_59_payload_rows_are_independent() {
    with_compiler_stack(|| {
        let source = "const NAME: String = \"shared\"\nfn make(flag: bool) -> Result[List[u64], String] { if flag { Ok([1]) } else { Error(NAME) } }\nfn need(take xs: unique List[u64]) {}\npub fn main() { match make(true) { Ok(xs) => need(take xs), Error(_) => () } }";
        let (mut engine, output) = check(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let header = input
            .signatures
            .values()
            .find(|s| s.definition.path().0.name == "make")
            .unwrap();
        assert!(
            !header
                .metadata
                .inferred_unique
                .results
                .contains(&UniqueResultRow::Root)
        );
        assert!(
            header
                .metadata
                .inferred_unique
                .results
                .contains(&UniqueResultRow::Payload {
                    variant: 0,
                    field: 0
                })
        );
        engine.import_native_input(input).unwrap();
    });
}

#[test]
fn private_helper_uniqueness_59_failure_before_repair_loses_summary() {
    with_compiler_stack(|| {
        let source = "const EMPTY: List[u64] = []\nfn reset(mut out: List[u64], ok: bool) { out = EMPTY; failure.check(ok, Bounds()); out = unique out }\nfn need(take xs: unique List[u64]) {}\npub fn main() { let mut xs: unique List[u64] = []; reset(mut xs, true); need(take xs) }";
        let (_, output) = check(source);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MODE-2"),
            "{:?}",
            output.diagnostics
        );
    });
}

#[test]
fn private_helper_uniqueness_59_propagated_errors_are_returns() {
    with_compiler_stack(|| {
        let source = "const NAME: String = \"x\"\nfn forward(take input: Result[(), String]) -> Result[(), String] { input?; Ok(()) }\npub fn main() { let _ = forward(Error(NAME)) }";
        let (mut engine, output) = check(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let header = input
            .signatures
            .values()
            .find(|s| s.definition.path().0.name == "forward")
            .unwrap();
        assert!(
            !header
                .metadata
                .inferred_unique
                .results
                .contains(&UniqueResultRow::Root)
        );
        assert!(
            !header
                .metadata
                .inferred_unique
                .results
                .contains(&UniqueResultRow::Payload {
                    variant: 1,
                    field: 0
                })
        );
        engine.import_native_input(input).unwrap();
    });
}

#[test]
fn private_helper_uniqueness_59_lost_summary_names_body_site() {
    with_compiler_stack(|| {
        for (source, helper, broken) in [
            (
                "const EMPTY: List[u64] = []\nfn reset(mut xs: List[u64]) { xs = EMPTY }\nfn need(take xs: unique List[u64]) {}\npub fn main() { let mut xs: unique List[u64] = []; reset(mut xs); need(take xs) }",
                "reset",
                "xs = EMPTY",
            ),
            (
                "const NAME: String = \"x\"\nfn code() -> String { NAME }\nfn need(take s: unique String) {}\npub fn main() { need(code()) }",
                "code",
                "NAME",
            ),
        ] {
            let (_, output) = check(source);
            let d = output
                .diagnostics
                .iter()
                .find(|d| d.rule.as_str() == "MODE-2")
                .expect("rejected");
            assert!(
                d.secondary.iter().any(|l| l.message.contains(helper)
                    && output
                        .sources
                        .slice(l.span)
                        .is_some_and(|text| text.contains(broken))),
                "{d:?}"
            );
            assert!(
                d.notes
                    .iter()
                    .all(|n| !n.contains("declare `@preserves_unique(")),
                "{d:?}"
            );
        }
    });
}
#[test]
fn private_helper_uniqueness_59_forged_headers_are_independently_reproved() {
    with_compiler_stack(|| {
        use hirsel_types::{ParamId, UniqueSummary};
        let preservation = UniqueSummary {
            nonretaining: Vec::new(),
            preserves_unique: vec![ParamId(0)],
            results: vec![],
        };
        let root = UniqueSummary {
            nonretaining: Vec::new(),
            preserves_unique: vec![],
            results: vec![UniqueResultRow::Root],
        };
        for (source, claimed) in [
            (
                "fn add(mut xs: List[List[u64]], take k: List[u64]) { list.push(mut xs, copy k) }\npub fn main() {}",
                vec![("add", preservation.clone())],
            ),
            (
                "pub fn add(mut xs: List[u64]) { list.push(mut xs, 1) }\npub fn main() {}",
                vec![("add", preservation.clone())],
            ),
            (
                "fn add(mut xs: List[u64]) { list.push(mut xs, 1) }\nfn via(mut xs: List[u64]) { let f: fn(mut List[u64]) -> () = add; f(mut xs) }\npub fn main() {}",
                vec![("via", preservation.clone())],
            ),
            (
                "const EMPTY: List[u64] = []\nfn reset(mut xs: List[u64], ok: bool) { xs = EMPTY; failure.check(ok, Bounds()); xs = unique xs }\npub fn main() {}",
                vec![("reset", preservation.clone())],
            ),
            (
                "const NAME: String = \"x\"\nfn code() -> String { NAME }\npub fn main() {}",
                vec![("code", root)],
            ),
            (
                "const EMPTY: List[u64] = []\nfn a(mut xs: List[u64], n: u64) { if n == 0 { xs = EMPTY } else { b(mut xs, n - 1) } }\nfn b(mut xs: List[u64], n: u64) { a(mut xs, n) }\npub fn main() {}",
                vec![("a", preservation.clone()), ("b", preservation.clone())],
            ),
        ] {
            let (mut engine, output) = check(source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
            let mut input = engine.export_native_input().unwrap();
            for (name, summary) in claimed {
                let mut path = entry_path(&default_context());
                path.0.name = name.into();
                let signature = input.signatures.get_mut(&path).unwrap();
                let id = signature.definition;
                std::sync::Arc::make_mut(signature).metadata.inferred_unique = summary.clone();
                let body = engine
                    .lower_to_tir(id.path())
                    .unwrap_or_else(|e| panic!("{source}\n{name}: {e:?}"))
                    .value;
                let definition = input.definitions.entry(id.path().clone()).or_insert(body);
                std::sync::Arc::make_mut(definition).inferred_unique = summary;
            }
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            let refused = input.definitions.values().any(|definition| {
                !hirsel_validate::validate(definition, &input.types, &signatures, &mut vec![])
                    .violations
                    .is_empty()
            });
            assert!(refused, "V accepted forged summary: {source}");
        }
    });
}

#[test]
fn private_helper_uniqueness_59_checked_receipts_keep_settled_headers() {
    with_compiler_stack(|| {
        let (mut engine, output) = check(
            "fn epoch_day() -> i128 { 1 }\nfn format() -> i128 { epoch_day() }\npub fn main() { let _ = format() }",
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let entry = entry_path(&default_context());
        engine.checked_products(&entry).unwrap();
    });
}

#[test]
fn private_helper_uniqueness_59_stored_headers_settle_before_body_receipts() {
    with_compiler_stack(|| {
        use std::sync::Arc;
        let store = Arc::new(hirsel_engine::store::MemStore::default());
        let mut engine =
            QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store);
        engine.set_retention(hirsel_engine::memo::Retention::Batch);
        let source = "fn epoch_day() -> i128 { 1 }\nfn format() -> i128 { epoch_day() }\npub fn main() { let _ = format() }";
        let file = engine.sources.add("stored.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        engine
            .checked_products(&entry_path(&default_context()))
            .unwrap();
    });
}

#[test]
fn private_helper_uniqueness_59_precompute_keeps_published_header_receipts() {
    with_compiler_stack(|| {
        let dir = std::env::temp_dir().join(format!(
            "hirsel-private-summary-precompute-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("days.hir"),
            "@noalloc fn epoch_day() -> i128 uses {} { 1 }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("format.hir"),
            "pub fn format() -> i128 uses {} { epoch_day() }\n",
        )
        .unwrap();
        std::fs::write(dir.join("hirsel.json"), r#"{"schema":1,"package":{"name":"summary_test","version":"1"},"sources":[{"path":"days.hir","module":["time"]},{"path":"format.hir","module":["time"]}],"dependencies":[]}"#).unwrap();
        COMPILER
            .precompute_package(&dir.join("hirsel.json"), &dir.join("products"))
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn private_helper_uniqueness_59_option_and_tuple_rows_survive_shared_siblings() {
    with_compiler_stack(|| {
        let source = r#"
const NAME: String = "shared"
fn pair() -> (String, List[u64]) { (NAME, [1]) }
fn maybe(flag: bool) -> Option[List[u64]] {
  if flag { Some([1]) } else { None() }
}
fn need(take xs: unique List[u64]) {}
pub fn main() {
  let (_, values) = pair()
  need(take values)
  match maybe(true) { Some(payload) => need(take payload), None() => () }
}
"#;
        let (mut engine, output) = check(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let pair = input
            .signatures
            .values()
            .find(|s| s.definition.path().0.name == "pair")
            .unwrap();
        assert_eq!(
            pair.metadata.inferred_unique.results,
            [UniqueResultRow::Tuple(1)]
        );
        let maybe = input
            .signatures
            .values()
            .find(|s| s.definition.path().0.name == "maybe")
            .unwrap();
        assert!(
            maybe
                .metadata
                .inferred_unique
                .results
                .contains(&UniqueResultRow::Payload {
                    variant: 1,
                    field: 0
                })
        );
        engine.import_native_input(input).unwrap();
    });
}

#[test]
fn private_helper_uniqueness_59_body_edit_invalidates_summary_and_caller() {
    with_compiler_stack(|| {
        let good = "const EMPTY: List[u64] = []\nfn add(mut xs: List[u64]) { list.push(mut xs, 1) }\nfn need(take xs: unique List[u64]) {}\npub fn main() { let mut xs: unique List[u64] = []; add(mut xs); need(take xs) }";
        let (mut engine, output) = check(good);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let file = engine
            .sources
            .add_snapshot(
                hirsel_span::FileId(0),
                good.replace("list.push(mut xs, 1)", "xs = EMPTY"),
            )
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|d| d.rule.as_str() == "MODE-2")
            .unwrap_or_else(|| panic!("{:?}", output.diagnostics));
        assert!(
            diagnostic
                .secondary
                .iter()
                .any(|l| l.message.contains("add")),
            "{diagnostic:?}"
        );
    });
}

#[test]
fn private_helper_uniqueness_59_closure_calls_share_through_borrowed_views() {
    with_compiler_stack(|| {
        for (body, preserves) in [
            ("{let v=view(x);let kept=v;()};()", true),
            ("{let v=view(x);let kept=(fn(){v})();()};()", false),
        ] {
            let source = format!(
                "fn view(x:Bytes)->Bytes in x{{copy x}}\nfn probe(mut x:Bytes){{{body}}}\npub fn main(){{}}"
            );
            let (mut engine, output) = check(&source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
            let mut probe = entry_path(&default_context());
            probe.0.name = "probe".into();
            engine.lower_to_tir(&probe).unwrap();
            let signature = engine
                .signature(hirsel_syntax::DefId::intern(&probe))
                .unwrap();
            assert_eq!(
                signature
                    .metadata
                    .inferred_unique
                    .preserves_unique
                    .contains(&hirsel_types::ParamId(0)),
                preserves,
                "{source}"
            );
        }
    });
}

#[test]
fn private_helper_forwards_declared_unique_result_payload() {
    with_compiler_stack(|| {
        let source = "pub fn produced()->Result[unique List[u8],()] { Ok([]) }\nfn execute()->Result[List[u8],()] { produced() }\npub fn main()->Result[(),()] { let work = fn()->Result[unique List[u8],()] { execute() }; let _ = work()?; Ok(()) }\n";
        let (mut engine, output) = check(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let header = input
            .signatures
            .values()
            .find(|s| s.definition.path().0.name == "execute")
            .unwrap();
        assert!(
            header
                .metadata
                .inferred_unique
                .results
                .contains(&UniqueResultRow::Payload {
                    variant: 0,
                    field: 0
                })
        );
        engine.import_native_input(input).unwrap();
    });
}
