//! Rust a7c37f9f library/core/src/str/iter.rs:937-951 and
//! library/alloctests/tests/str.rs:1412-1427,2278-2282 (MIT).
use crate::source_std::tir;
use hirsel_engine::*;
use hirsel_interp::{Integer, Limits, Outcome, Value, library::Library};
use hirsel_types::Primitive;
use std::path::PathBuf;

#[test]
fn string_splitn_5_rust_oracle_and_bounded_tail() {
    with_compiler_stack(|| {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let source = r#"
pub fn bounded(take s: String, sep: String, limit: u64) -> List[String] uses {} {
  std@1/string.splitn(take s, sep, limit)
}
pub fn main() -> () { let _ = bounded("a:b", ":", 2) }
"#;
        let prepared = tir(&fixture, Some(source), Some("bounded")).unwrap();
        let runtime = &prepared.runtime;
        let sources = &prepared.sources;
        let configuration = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mary = "\nMäry häd ä little lämb\nLittle lämb\n";
        let mut vectors = vec![(mary.to_owned(), " ".to_owned()), (mary.into(), "ä".into())];
        for input in [
            "",
            "abc",
            ":",
            "::a::::",
            "foo::bar::baz",
            "ababa",
            "ééé",
            "界💚界💚界",
        ] {
            for separator in [":", "::", "aba", "éé", "💚", "absent-long-delimiter"] {
                vectors.push((input.into(), separator.into()));
            }
        }
        // Explicitly exclude Rust's empty-pattern semantics: Hirsel rejects it.
        let mut checked = 0;
        for (input, separator) in &vectors {
            for limit in [0, 1, 2, 3, 4, u64::MAX] {
                let expected = input
                    .splitn(usize::try_from(limit).unwrap(), separator.as_str())
                    .map(|s| Some(Value::String(s.into())))
                    .collect::<Vec<_>>();
                let mut library = Library::new(&configuration, Default::default()).unwrap();
                let execution = hirsel_interp::run_with_runtime(
                    &mut prepared.provider(),
                    sources,
                    &configuration,
                    vec![
                        Value::String(input.clone()),
                        Value::String(separator.clone()),
                        Value::Integer(Integer::new(Primitive::U64, u128::from(limit)).unwrap()),
                    ],
                    Limits::default(),
                    &mut library,
                    runtime,
                )
                .unwrap();
                let Outcome::Returned {
                    value: Value::Sequence { elements, .. },
                    ..
                } = execution.outcome
                else {
                    panic!("{input:?}/{separator:?}/{limit}: {:?}", execution.outcome);
                };
                assert_eq!(
                    elements.as_slice(),
                    expected.as_slice(),
                    "{input:?}/{separator:?}/{limit}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 300);
        eprintln!("string-splitn-5: {checked} Rust oracle comparisons");

        // Go ebe2feefc400f813aefdc8a036e66d2619da6649,
        // src/strings/strings_test.go:453-473 (BSD-3-Clause).
        // Negative n maps to u64::MAX; nil maps to an empty list. The eight
        // empty-pattern rows must fail in Hirsel. Two invalid UTF-8 rows are
        // unrepresentable as String and are excluded, not silently repaired.
        type GoRow = (
            &'static str,
            &'static str,
            u64,
            Option<&'static [&'static str]>,
        );
        let go_rows: &[GoRow] = &[
            ("", "", 18446744073709551615u64, None),
            ("abcd", "", 2u64, None),
            ("abcd", "", 4u64, None),
            ("abcd", "", 18446744073709551615u64, None),
            ("☺☻☹", "", 18446744073709551615u64, None),
            ("☺☻☹", "", 3u64, None),
            ("☺☻☹", "", 17u64, None),
            ("☺�☹", "", 18446744073709551615u64, None),
            ("abcd", "a", 0u64, Some(&[])),
            ("abcd", "a", 18446744073709551615u64, Some(&["", "bcd"])),
            ("abcd", "z", 18446744073709551615u64, Some(&["abcd"])),
            (
                "1,2,3,4",
                ",",
                18446744073709551615u64,
                Some(&["1", "2", "3", "4"]),
            ),
            (
                "1....2....3....4",
                "...",
                18446744073709551615u64,
                Some(&["1", ".2", ".3", ".4"]),
            ),
            ("☺☻☹", "☹", 18446744073709551615u64, Some(&["☺☻", ""])),
            ("☺☻☹", "~", 18446744073709551615u64, Some(&["☺☻☹"])),
            ("1 2 3 4", " ", 3u64, Some(&["1", "2", "3 4"])),
            ("1 2", " ", 3u64, Some(&["1", "2"])),
            ("", "T", 2305843009213693951u64, Some(&[""])),
        ];
        for &(input, separator, limit, expected) in go_rows {
            let mut library = Library::new(&configuration, Default::default()).unwrap();
            let execution = hirsel_interp::run_with_runtime(
                &mut prepared.provider(),
                sources,
                &configuration,
                vec![
                    Value::String(input.into()),
                    Value::String(separator.into()),
                    Value::Integer(Integer::new(Primitive::U64, u128::from(limit)).unwrap()),
                ],
                Limits::default(),
                &mut library,
                runtime,
            )
            .unwrap();
            match (expected, execution.outcome) {
                (
                    Some(expected),
                    Outcome::Returned {
                        value: Value::Sequence { elements, .. },
                        ..
                    },
                ) => {
                    let expected = expected
                        .iter()
                        .map(|s| Some(Value::String((*s).into())))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        elements.as_slice(),
                        expected.as_slice(),
                        "Go {input:?}/{separator:?}/{limit}"
                    );
                }
                (None, Outcome::Failed(failure)) => {
                    assert_eq!(failure.rule.as_str(), "SEM-3");
                    assert_eq!(failure.kind, "failed check");
                    assert_eq!(failure.line, 388);
                }
                (_, actual) => panic!("Go {input:?}/{separator:?}/{limit}: {actual:?}"),
            }
        }
        assert_eq!(go_rows.len(), 18);
        eprintln!("string-splitn-5: 18 Go table rows (10 returns, 8 empty-separator failures)");

        // Diagnostic only: a delimiter-rich 1 MiB suffix yields three pieces.
        // Default interpreter fuel also rules out a full split and discard.
        let tail = "detail=value|".repeat(90_000);
        let input = format!("INFO|worker|{tail}");
        let mut library = Library::new(&configuration, Default::default()).unwrap();
        let execution = hirsel_interp::run_with_runtime(
            &mut prepared.provider(),
            sources,
            &configuration,
            vec![
                Value::String(input),
                Value::String("|".into()),
                Value::Integer(Integer::new(Primitive::U64, 3).unwrap()),
            ],
            Limits::default(),
            &mut library,
            runtime,
        )
        .unwrap();
        let Outcome::Returned {
            value: Value::Sequence { elements, .. },
            ..
        } = execution.outcome
        else {
            panic!("large-tail diagnostic: {:?}", execution.outcome);
        };
        assert_eq!(
            elements.as_slice(),
            &[
                Some(Value::String("INFO".into())),
                Some(Value::String("worker".into())),
                Some(Value::String(tail))
            ]
        );
        eprintln!(
            "string-splitn-5: large tail, {} fields, {} interpreter steps",
            elements.len(),
            execution.steps
        );
    });
}
