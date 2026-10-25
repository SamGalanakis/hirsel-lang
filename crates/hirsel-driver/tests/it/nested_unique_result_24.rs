//! A deep copy changes value ownership, not a fixed nested element contract.
use hirsel_driver::*;
use hirsel_engine::*;

#[path = "../support/history.rs"]
#[allow(
    dead_code,
    reason = "shared history support also serves reduction and cleanup tests"
)]
mod history;

const RETURNING: &str = r#"fn relay[T](take source: unique List[unique T]) -> unique List[unique T] { take source }
fn probe(source: List[List[String]]) {
  let owned: unique List[unique List[String]] = unique source
  let rows = relay(take owned)
}
pub fn main() {}
"#;

fn checked(source: &str) -> CheckOutput {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("nested-copy.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}

#[test]
fn nested_unique_result_24_explains_copy_contract_for_returning_concrete_and_unit() {
    with_compiler_stack(|| {
        let concrete = RETURNING
            .replace("relay[T]", "relay")
            .replace("unique T", "unique List[String]");
        let unit = RETURNING.replace(
            " -> unique List[unique T] { take source }",
            " { let _ = take source }",
        );
        for text in [RETURNING, &concrete, &unit] {
            let output = checked(text);
            let [d] = &output.diagnostics[..] else {
                panic!("{text}\n{:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "MODE-2");
            assert_eq!(output.sources.slice(d.primary), Some("take owned"));
            assert!(
                d.message
                    .contains("`unique` copies the value but cannot add"),
                "{d:?}"
            );
            assert!(d.message.contains("unique List[String]"));
            assert!(d.fix.is_none());
            assert!(
                d.secondary.iter().any(|l| {
                    output.sources.slice(l.span) == Some("unique List[unique List[String]]")
                        && l.message.contains("annotation")
                }),
                "{d:?}"
            );
            assert!(
                d.notes
                    .iter()
                    .any(|n| n.contains("element-by-element") && n.contains("fresh literal")),
                "{d:?}"
            );
            let human = d.render_human(&output.sources);
            let json = d.to_json(&output.sources);
            assert_eq!(json["message"], d.message);
            for label in &d.secondary {
                assert!(human.contains(&label.message));
            }
            for note in &d.notes {
                assert!(human.contains(note));
            }
        }
    });
}

#[test]
fn nested_unique_result_24_direct_copy_and_mutable_binding_controls() {
    with_compiler_stack(|| {
        let direct = RETURNING.replace(
            "  let owned: unique List[unique List[String]] = unique source\n  let rows = relay(take owned)",
            "  let rows = relay(unique source)",
        );
        let output = checked(&direct);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert_eq!(output.sources.slice(d.primary), Some("unique source"));
        assert!(
            d.message
                .contains("`unique` copies the value but cannot add")
        );
        assert!(
            !d.secondary
                .iter()
                .any(|l| l.message.contains("binding annotation"))
        );

        // A mutable binding has no retained immutable initializer receipt.
        // Its declaration must not be presented as the current value's cause.
        let mutable = RETURNING.replace("let owned:", "let mut owned:");
        let output = checked(&mutable);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert!(!d.message.contains("cannot add"));
        assert!(
            !d.secondary
                .iter()
                .any(|l| l.message.contains("binding annotation"))
        );
    });
}

// Rust Vec clone vectors, f6c1107f: alloc/src/vec/mod.rs:4054-4092 and
// alloctests/tests/vec.rs:257-292. Compare values and retained snapshots;
// no pointer inequality, implicit Clone bound or buffer-reuse promise.
const RECIPE: &str = r#"
fn relay[T](take source: unique List[unique T]) -> unique List[unique T] { take source }
fn verify(source: List[List[String]], untouched: List[List[String]], changed: List[List[String]]) -> bool {
  let snapshot = copy source
  let mut owned: unique List[unique List[String]] = []
  for row in source { list.push(mut owned, unique row) }
  let mut rows = relay(take owned)
  if list.len(rows) > 0 { list.push(mut rows[0], "changed") }
  rows == changed && snapshot == untouched && source == untouched
}
pub fn main() -> Result[(), ()] {
  // The fresh-literal alternative supplies its complete contract at construction.
  let literal: unique List[unique List[String]] = [["fresh"]]
  let received = relay(take literal)
  if received != [["fresh"]] { return Error(()) }
  if !verify([], [], []) { return Error(()) }
  if !verify([["a"]], [["a"]], [["a", "changed"]]) { return Error(()) }
  if !verify([["a"], ["b"]], [["a"], ["b"]], [["a", "changed"], ["b"]]) { return Error(()) }
  if !verify([["a"], ["b", "c"]], [["a"], ["b", "c"]], [["a", "changed"], ["b", "c"]]) { return Error(()) }
  Ok(())
}
"#;

#[test]
fn nested_unique_result_24_legal_recipes_execute() {
    let scratch =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("nested-unique-result-24");
    let runner = history::Runner::new(&scratch).unwrap();
    let report = runner.run("recipes", RECIPE);
    assert!(report.agrees(), "{report:?}");
    assert_eq!(report.cells.len(), 4);
    assert!(report.disagreements(b"", false).is_empty(), "{report:?}");
    eprintln!("{report:?}");
}
