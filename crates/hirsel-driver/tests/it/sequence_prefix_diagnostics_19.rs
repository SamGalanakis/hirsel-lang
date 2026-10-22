//! Independent expected diagnostics and executable repairs for LANG-2 sequence prefixes.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("sequence.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    let warm = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| d.to_json(&output.sources))
            .collect::<Vec<_>>(),
        warm.diagnostics
            .iter()
            .map(|d| d.to_json(&warm.sources))
            .collect::<Vec<_>>()
    );
    output
}

fn rejected(source: &str, rule: &str, primary: &str, guidance: &[&str]) {
    let output = checked(source);
    let [d] = &output.diagnostics[..] else {
        panic!("{source}: {:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), rule, "{source}: {d:?}");
    assert_eq!(
        output.sources.slice(d.primary),
        Some(primary),
        "{source}: {d:?}"
    );
    assert_eq!(
        output.sources.resolve(d.primary).start as usize,
        source.find(primary).unwrap()
    );
    assert!(d.fix.is_none());
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    eprintln!("{source}\n{human}\n{json}");
    for text in guidance {
        assert!(human.contains(text), "missing {text}: {human}");
        assert!(
            json["message"].as_str().unwrap().contains(text)
                || json["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n.as_str().unwrap().contains(text)),
            "{json}"
        );
    }
}

#[test]
fn sequence_prefix_diagnostics_19_empty() {
    with_compiler_stack(|| {
        rejected(
            "fn rejected() { let value = [] }",
            "LANG-2",
            "[]",
            &[
                "empty sequence",
                "List[u64]",
                "Array[u64]",
                "Inline[u64, 0]",
            ],
        )
    });
}

#[test]
fn sequence_prefix_diagnostics_19_numeric() {
    with_compiler_stack(|| {
        for (literal, first) in [
            ("[1, 2u64]", "1"),
            ("[-1, 2i64]", "-1"),
            ("[1.0, 2f64]", "1.0"),
        ] {
            rejected(
                &format!("// café 🐑\nfn rejected() {{ let value = {literal} }}"),
                "LANG-2",
                first,
                &[
                    "first sequence element",
                    "later elements",
                    "numeric suffix",
                    "List[u64]",
                    "Inline[u64, 2]",
                ],
            );
        }
    });
}

#[test]
fn sequence_prefix_diagnostics_19_variant() {
    with_compiler_stack(|| {
        rejected(
            "fn rejected() { let value = [None(), Some(\"x\")] }",
            "LANG-2",
            "None()",
            &[
                "first sequence element",
                "later elements",
                "List[Option[String]]",
                "Array[Option[String]]",
                "Inline[Option[String], 2]",
            ],
        )
    });
}

#[test]
fn sequence_prefix_diagnostics_19_repeat() {
    with_compiler_stack(|| {
        for (literal, primary) in [("[1u64; 2]", "[1u64; 2]"), ("[1; 2]", "1")] {
            rejected(
                &format!("fn rejected() {{ let value = {literal} }}"),
                "LANG-2",
                primary,
                &[
                    "repeat sequence",
                    "Array[u64]",
                    "Inline[u64, 2]",
                    "suffix alone",
                ],
            );
        }
    });
}

#[test]
fn sequence_prefix_diagnostics_19_checked_repairs_and_concrete_prefixes() {
    with_compiler_stack(|| {
        for statement in [
            "let value: List[u64] = []",
            "let value: Array[u64] = []",
            "let value: Inline[u64, 0] = []",
            "let value: List[u64] = [1, 2u64]",
            "let value: Array[u64] = [1, 2u64]",
            "let value: Inline[u64, 2] = [1, 2u64]",
            "let value = [1u64, 2u64]",
            "let value = [-1i64, 2i64]",
            "let value = [1.0f64, 2f64]",
            "let value = [1u64, 2]",
            "let value = [-1i64, 2]",
            "let value = [1.0f64, 2.0]",
            "let value: List[Option[String]] = [None(), Some(\"x\")]",
            "let value: Array[Option[String]] = [None(), Some(\"x\")]",
            "let value: Inline[Option[String], 2] = [None(), Some(\"x\")]",
            "let value: Array[u64] = [1u64; 2]",
            "let value: Inline[u64, 2] = [1u64; 2]",
            "let value: Array[u64] = [1; 2]",
            "let value: Inline[u64, 2] = [1; 2]",
            "let value = [\"x\", \"y\"]",
            "let value = [true, false]",
            "let value = [Some(\"x\"), None()]",
            "let value = [1u64 + 2, 3]",
            "let value = [Item(n: 1u64), Item(n: 2)]",
            "let value: Array[Item] = [Item(n: 1), Item(n: 2)]",
            "let value: Inline[Item, 2] = [Item(n: 1), Item(n: 2)]",
        ] {
            let source = format!("type Item {{ n: u64 }}\nfn accepted() {{ {statement} }}");
            let output = checked(&source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
            eprintln!("accepted: {statement}");
        }
    });
}

#[test]
fn sequence_prefix_diagnostics_19_context_and_later_heterogeneity() {
    with_compiler_stack(|| {
        for (statement, rule, primary) in [
            ("let value = [\"x\", 2u64]", "LANG-7", "2u64"),
            ("let value = [1u64, true]", "LANG-7", "true"),
            ("let value = [1.0f64, 2]", "LANG-7", "2"),
            ("let value: Inline[u64, 3] = [1, 2]", "LANG-7", "[1, 2]"),
            ("let value: Inline[u64, 3] = [1; 2]", "LANG-7", "[1; 2]"),
            ("let value: List[u64] = [1; 2]", "LANG-7", "[1; 2]"),
            ("let value: u64 = []", "LANG-2", "[]"),
            ("let value: u64 = [1u64]", "LANG-2", "[1u64]"),
        ] {
            rejected(
                &format!("fn rejected() {{ {statement} }}"),
                rule,
                primary,
                &[],
            );
        }
    });
}

#[test]
fn sequence_prefix_diagnostics_19_prerequisite_recovery() {
    with_compiler_stack(|| {
        for source in [
            "fn rejected() { let value = [missing, 2u64] }",
            "fn rejected() { let value = [missing; 2] }",
            "fn rejected() { let value: Missing = [] }",
            "fn rejected() { let value = [1u64, missing] }",
        ] {
            let output = checked(source);
            assert_eq!(
                output.diagnostics.len(),
                1,
                "{source}: {:?}",
                output.diagnostics
            );
            assert!(
                !output.diagnostics[0]
                    .notes
                    .iter()
                    .any(|n| n.contains("sequence")),
                "{source}: {:?}",
                output.diagnostics
            );
            eprintln!("prerequisite: {source}: {:?}", output.diagnostics);
        }
    });
}
