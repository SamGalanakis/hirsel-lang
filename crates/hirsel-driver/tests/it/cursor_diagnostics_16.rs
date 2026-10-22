//! Expectations derive from LANG-15's fixed cover and owned refill contract.
use hirsel_driver::*;
use hirsel_engine::*;

const TAGGED: &str = "// λ cursor witness\ntype Value = Leaf | Tagged(value: box Value)\nfn unboxed(value: box Value) -> Value in value uses {} {\n  match value { Leaf() => Leaf(), Tagged(value: inner) => Tagged(value: copy inner) }\n}\nfn probe(value: Value) {\n  let mut cursor = value\n  match cursor { Tagged(value: inner) => { cursor = unboxed(inner) }, _ => () }\n}\n";
const PAIR: &str = "// λ computed pair\ntype Value = Leaf | Array(List[Value])\nfn probe(a: Value, b: Value) {\n  let mut left: Value in {a, b} = a\n  match (left, b) { (Array(xs), Array(_)) => { left = xs[0u64] }, _ => () }\n}\n";

fn check(
    source: &str,
    expected: &[(&str, &str)],
    labels: &[(&str, &str, &str)],
) -> Vec<serde_json::Value> {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("cursor.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert_eq!(errors.len(), expected.len(), "{source}: {errors:?}");
        for (d, (rule, primary)) in errors.iter().zip(expected) {
            assert_eq!(d.rule.as_str(), *rule, "{d:?}");
            assert_eq!(output.sources.slice(d.primary), Some(*primary));
            assert!(d.fix.is_none());
            let json = d.to_json(&output.sources);
            let human = d.render_human(&output.sources);
            let at = output.sources.resolve(d.primary);
            let start = source.find(*primary).unwrap();
            assert_eq!(
                (at.start as usize, at.end as usize),
                (start, start + primary.len())
            );
            assert_eq!(d.secondary.len(), labels.len(), "{json}");
            for (label, (needle, text, explanation)) in d.secondary.iter().zip(labels) {
                let start = source.find(*needle).unwrap() + needle.find(*text).unwrap();
                let at = output.sources.resolve(label.span);
                assert_eq!(
                    (at.start as usize, at.end as usize),
                    (start, start + text.len()),
                    "{json}"
                );
                assert_eq!(output.sources.slice(label.span), Some(*text));
                assert!(label.message.contains(explanation), "{json}");
                assert!(human.contains(&label.message), "{human}");
            }
            for note in json["notes"].as_array().unwrap() {
                let note = note.as_str().unwrap();
                assert!(
                    human.contains(note.strip_prefix("help: ").unwrap_or(note)),
                    "{human}"
                );
                assert!(
                    !note.contains("copy ")
                        && !note.contains("declare the result")
                        && !note.contains("broaden"),
                    "{json}"
                );
            }
            let warm = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                warm.diagnostics
                    .iter()
                    .map(|d| d.to_json(&warm.sources))
                    .collect::<Vec<_>>(),
                output
                    .diagnostics
                    .iter()
                    .map(|d| d.to_json(&output.sources))
                    .collect::<Vec<_>>()
            );
            eprintln!("{json}");
        }
        output
            .diagnostics
            .iter()
            .map(|d| d.to_json(&output.sources))
            .collect()
    })
}

#[test]
fn cursor_diagnostics_16_owned_and_computed_pair() {
    check(
        TAGGED,
        &[("LANG-15", "cursor = unboxed(inner)")],
        &[
            ("let mut cursor", "cursor", "read-only borrow cursor"),
            ("unboxed(inner)", "unboxed(inner)", "owned value"),
            (
                "unboxed(inner)",
                "unboxed",
                "result relation does not turn an owned return",
            ),
        ],
    );
    check(
        PAIR,
        &[("MODE-1", "left = xs[0u64]")],
        &[
            ("let mut left", "left", "rooted in `a`, `b`"),
            ("Array(xs)", "xs", "view of `xs`"),
            ("(left, b)", "(left, b)", "distinct storage origin"),
        ],
    );
    check(
        &PAIR.replace(
            "match (left, b) { (Array(xs), Array(_))",
            "match left { Array(xs)",
        ),
        &[],
        &[],
    );
}

#[test]
fn cursor_diagnostics_16_copy_take_roots_and_aliases() {
    for refill in ["copy a", "take owned"] {
        check(
            &format!("// é\nfn f(a: Bytes, take owned: Bytes) {{ let mut c = a; c = {refill} }}"),
            &[("LANG-15", &format!("c = {refill}"))],
            &[
                ("let mut c", "c", "read-only borrow cursor"),
                (&format!("c = {refill}"), refill, "owned value"),
            ],
        );
    }
    check(
        "// é\nfn f(a: Bytes, b: Bytes) { let alias = a; let mut c = alias; c = b }",
        &[("MODE-1", "c = b")],
        &[
            ("let mut c", "c", "rooted in `a`"),
            ("b: Bytes", "Bytes", "view of `b`"),
        ],
    );
    check(
        "// é\nfn f(a: Bytes, b: Bytes) { let alias = a; let mut c: Bytes in {a,b} = alias; c = b }",
        &[],
        &[],
    );
    check(
        "// é\nfn f(a: Bytes) { let alias = a; let mut c = alias; c = a }",
        &[],
        &[],
    );
}

#[test]
fn cursor_diagnostics_16_inferred_pair_and_local_signature() {
    check(
        &PAIR.replace(": Value in {a, b}", ""),
        &[("MODE-1", "left = xs[0u64]")],
        &[
            ("let mut left", "left", "rooted in `a`"),
            ("Array(xs)", "xs", "view of `xs`"),
            ("(left, b)", "(left, b)", "distinct storage origin"),
        ],
    );
    check(
        "// λ local signature\nfn probe(a: Bytes) {\n fn owned(x: Bytes) -> Bytes in x { copy x }\n let mut c = a\n c = owned(a)\n}",
        &[("LANG-15", "c = owned(a)")],
        &[
            ("let mut c", "c", "read-only borrow cursor"),
            ("owned(a)", "owned(a)", "owned value"),
            (
                "-> Bytes in x",
                "Bytes in x",
                "result relation does not turn an owned return",
            ),
        ],
    );
}

#[test]
fn cursor_diagnostics_16_deterministic_across_session_salts() {
    let Some(runs) = hirsel_intern::under_salts(
        "cursor_diagnostics_16::cursor_diagnostics_16_deterministic_across_session_salts",
        &[1, 2, 3],
        || {
            serde_json::to_string(&check(
                PAIR,
                &[("MODE-1", "left = xs[0u64]")],
                &[
                    ("let mut left", "left", "rooted in `a`, `b`"),
                    ("Array(xs)", "xs", "view of `xs`"),
                    ("(left, b)", "(left, b)", "distinct storage origin"),
                ],
            ))
            .unwrap()
        },
    ) else {
        return;
    };
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|run| run == &runs[0]));
}

fn check_recovery(source: &str, expected: &[(&str, &str)]) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("cursor.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let actual: Vec<_> = output
            .diagnostics
            .iter()
            .map(|d| (d.rule.as_str(), output.sources.slice(d.primary).unwrap()))
            .collect();
        assert_eq!(actual, expected, "{source}");
        for d in &output.diagnostics {
            let json = d.to_json(&output.sources);
            let human = d.render_human(&output.sources);
            assert_eq!(json["rule"], d.rule.as_str());
            assert_eq!(json["message"], d.message);
            assert_eq!(json["severity"], "error");
            let at = output.sources.resolve(d.primary);
            assert_eq!(json["primary"]["start"], at.start);
            assert_eq!(json["primary"]["end"], at.end);
            assert!(human.contains(d.rule.as_str()), "{human}");
            assert!(human.contains(&d.message), "{human}");
            assert!(!human.contains("rooted in )"), "{human}");
        }
        let warm = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(
            warm.diagnostics
                .iter()
                .map(|d| d.to_json(&warm.sources))
                .collect::<Vec<_>>(),
            output
                .diagnostics
                .iter()
                .map(|d| d.to_json(&output.sources))
                .collect::<Vec<_>>()
        );
    });
}

#[test]
fn cursor_diagnostics_16_invalid_cover_recovery() {
    check_recovery(
        "fn f(a:List[u64])->u64 {let mut c:List[u64] in env=a; c=a; list.len(c)}",
        &[("LANG-15", "a")],
    );
    check_recovery(
        "fn f(a:List[u64])->u64 {let alias=a; let mut c:List[u64] in env=alias; c=a; list.len(c)}",
        &[("LANG-15", "alias")],
    );
    for cover in ["env", "{env,a}", "heap"] {
        check_recovery(
            &format!(
                "fn f(a:List[u64],b:List[u64])->u64 {{let mut c:List[u64] in {cover}=a; c=b; c=a; c=copy b; list.len(c)}}"
            ),
            &[("LANG-15", "a")],
        );
    }
    check_recovery(
        "fn f(a:List[u64],b:List[u64])->u64 {let mut c:List[u64] in b=a; c=b; c=a; list.len(c)}",
        &[("MODE-1", "a")],
    );
}

#[test]
fn cursor_diagnostics_16_invalid_cover_prerequisites() {
    check_recovery(
        "fn f(a:List[u64])->u64 {let mut c:List[u64] in {heap,a}=a; c=a; list.len(c)}",
        &[("LANG-7", "=")],
    );
    for (cover, rule) in [
        ("missing", "LANG-4"),
        ("a.nope", "LANG-4"),
        ("later", "LANG-2"),
    ] {
        check_recovery(
            &format!(
                "fn f(a:List[u64])->u64 {{let mut c:List[u64] in {cover}=a; let later=a; c=a; list.len(c)}}"
            ),
            &[(rule, &format!("List[u64] in {cover}"))],
        );
    }
    check_recovery(
        "fn f(a:List[u64])->u64{let mut c:List[u64] in a=a;c=a;list.len(c)}",
        &[],
    );
    check_recovery(
        "fn f(a:List[u64],b:List[u64])->u64 {let mut c:List[u64] in env=a; c=take b; list.len(c)}",
        &[("LANG-15", "a"), ("OWN-1", "take b")],
    );
    check_recovery(
        "fn f(a:List[u64],b:List[u64])->u64 {let mut c:List[u64] in env=a; c=a; let independent=take b; list.len(c)+list.len(independent)}",
        &[("LANG-15", "a"), ("OWN-1", "take b")],
    );
}
