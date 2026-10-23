//! Operand identity, error recovery and independent checked-heap value controls.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(engine: &mut QueryEngine, source: &str) -> CheckOutput {
    let file = engine.sources.add("concat.hir", source.into()).unwrap();
    let output = check_progress(engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}

#[test]
fn string_concat_guidance_24_resolved_operands_and_controls() {
    with_compiler_stack(|| {
        for (source, primary, guided) in [
            (
                "fn probe(a: String, b: String) { let _ = a + b }",
                "a + b",
                true,
            ),
            (
                "fn probe(a: String, b: u64) { let _ = a + { b } }",
                "a + { b }",
                true,
            ),
            (
                "fn probe(a: String, b: u64) { let _ = a + b }",
                "a + b",
                true,
            ),
            (
                "fn probe(a: u64, b: String) { let _ = a + b }",
                "a + b",
                true,
            ),
            (
                "type Text = Wrap(String)\nfn add(a: Text, b: Text) -> Text { unique a }\nfn probe(a: Text, b: Text) { let _ = a + b }",
                "a + b",
                false,
            ),
            (
                "fn probe(a: bool, b: bool) { let _ = a + b }",
                "a + b",
                false,
            ),
            (
                "fn probe(a: String, b: String) { let _ = a - b }",
                "a - b",
                false,
            ),
            (
                "fn probe(a: bool) { let _ = match a { true => 1u64 } }",
                "match a { true => 1u64 }",
                false,
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let output = check(&mut engine, source);
            let [d] = &output.diagnostics[..] else {
                panic!("{source}: {:?}", output.diagnostics)
            };
            assert_eq!(
                d.rule.as_str(),
                if source.contains("a: u64, b: String") {
                    "SEM-1"
                } else {
                    "LANG-7"
                },
                "{d:?}"
            );
            assert_eq!(output.sources.slice(d.primary), Some(primary));
            assert!(d.fix.is_none());
            let json = d.to_json(&output.sources);
            let notes = json["notes"].to_string();
            assert_eq!(notes.contains("f-strings"), guided, "{source}: {json}");
            if source.contains("match a") {
                assert_eq!(d.message, "match does not cover every value");
                assert_eq!(
                    json["notes"],
                    serde_json::json!([
                        "help: Add unguarded arms for missing patterns: `false`. Guarded arms do not establish complete coverage."
                    ])
                );
            } else {
                assert_eq!(notes.contains("non-exhaustive"), !guided, "{json}");
            }
            assert!(!notes.contains("string.concat"), "{json}");
            if guided {
                assert!(notes.contains("select std@1"), "{json}");
                assert_eq!(
                    notes.contains("convert the non-String"),
                    source.contains("u64")
                );
            }
            eprintln!("{}", d.render_human(&output.sources));
        }
        for source in [
            "fn probe(a: String) { let _ = a + missing }",
            "fn probe(a: String) { let _ = missing + a }",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let output = check(&mut engine, source);
            assert!(!output.diagnostics.is_empty());
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "LANG-4")
            );
            assert!(!output.diagnostics.iter().any(|d| {
                d.to_json(&output.sources)["notes"]
                    .to_string()
                    .contains("f-strings")
            }));
        }
        for source in [
            "fn probe() -> String { 1u64 + 2u64 }",
            "fn probe(a: String) { let _ = a + 1 }",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let output = check(&mut engine, source);
            assert!(!output.diagnostics.is_empty());
            assert!(
                !output
                    .diagnostics
                    .iter()
                    .any(|d| d.notes.iter().any(|n| n.contains("for String bindings")))
            );
        }
        for source in [
            "fn probe() { let _ = 1u64 + 2u64 }",
            "fn probe() { let _ = 1.0f64 + 2.0f64 }",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            assert!(check(&mut engine, source).diagnostics.is_empty());
        }
    });
}

#[test]
fn string_concat_guidance_24_selected_std_and_nominal_lookalike() {
    with_compiler_stack(|| {
        for (source, types, mixed) in [
            (
                "fn probe(left: String, right: String) { let _ = left + right }",
                "`String` and `String`",
                false,
            ),
            (
                "fn probe(left: String, right: u64) { let _ = left + right }",
                "`String` and `u64`",
                true,
            ),
            (
                "fn probe(left: f64, right: String) { let _ = left + right }",
                "`f64` and `String`",
                true,
            ),
            (
                "fn probe(left: u64, right: String) { let _ = left + right }",
                "`u64` and `String`",
                true,
            ),
            (
                "type Text { value: String }\nfn probe(left: String, right: Text) { let _ = left + right }",
                "`String` and `Text`",
                true,
            ),
            (
                "type Text { value: String }\nfn probe(left: Text, right: String) { let _ = left + right }",
                "`Text` and `String`",
                true,
            ),
        ] {
            let mut engine = crate::source_std::engine();
            let output = check(&mut engine, source);
            let [d] = &output.diagnostics[..] else {
                panic!("{source}: {:?}", output.diagnostics)
            };
            assert_eq!(
                d.rule.as_str(),
                if types == "`u64` and `String`" {
                    "SEM-1"
                } else if types == "`f64` and `String`" {
                    "SEM-2"
                } else {
                    "LANG-7"
                }
            );
            assert_eq!(output.sources.slice(d.primary), Some("left + right"));
            assert!(d.message.contains(types), "{d:?}");
            let notes = d.to_json(&output.sources)["notes"].to_string();
            assert!(
                notes.contains("std@1/string.concat(left, right)"),
                "{notes}"
            );
            assert!(!notes.contains("non-exhaustive"), "{notes}");
            assert_eq!(notes.contains("convert the non-String"), mixed);
            assert!(d.fix.is_none());
        }
    });
}

#[test]
fn string_concat_guidance_24_checked_heap_values_and_order() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let output = check(
            &mut engine,
            include_str!("../fixtures/string_concat_guidance_24.hir"),
        );
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let entry = entry_path(&default_context());
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let heap =
            native::run_heap(&ready, &entry, &native::Sites::new(validated.snapshot())).unwrap();
        assert_eq!(
            heap.observation.exit_code,
            Some(0),
            "{:?}",
            heap.observation
        );
        assert!(heap.observation.output.is_empty() && heap.observation.failures.is_empty());
        assert_eq!(heap.counters.live_heap_bytes, 0);
    });
}
