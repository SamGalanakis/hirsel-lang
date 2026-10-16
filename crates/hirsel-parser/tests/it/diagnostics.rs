use crate::support;

#[test]
fn declaration_postchecks_use_the_offending_declaration() {
    for tail in ["", "\nfn later() {}"] {
        for (source, rule, start, end) in [
            (format!("pub opaque fn f() {{}}{tail}"), "PROG-2", 4, 10),
            (format!("type O{tail}"), "LANG-7", 5, 6),
        ] {
            let (output, _) = support::parse(&source);
            let diagnostic = &output.diagnostics[0];
            assert_eq!(diagnostic.rule.as_str(), rule);
            assert_eq!(
                (diagnostic.primary.start, diagnostic.primary.end),
                (start, end),
                "{source}"
            );
        }
    }
}

#[test]
fn missing_closing_delimiters_use_empty_boundary_spans() {
    for (source, close) in [
        ("fn f() { (1u8 }", ")"),
        ("fn f() { xs[0u8 }", "]"),
        ("distinct type D(value:u8\nfn later() {}", ")"),
    ] {
        let (output, _) = support::parse(source);
        let diagnostic = &output.diagnostics[0];
        assert_eq!(diagnostic.message, format!("expected `{close}`"));
        assert_eq!(diagnostic.primary.start, diagnostic.primary.end, "{source}");
    }
}

#[test]
fn malformed_forms_name_the_actual_syntax_obligation() {
    for (source, rule, message) in [
        ("let x = 1u8", "LANG-1", "top-level definition"),
        ("}", "LANG-1", "top-level definition"),
        ("pub @noalloc fn f() {}", "LANG-1", "top-level definition"),
        ("type V = A\n | B", "LANG-1", "trailing `|`"),
        ("type V = | A | B", "LANG-4", "starts with a name"),
        (
            "fn f(b:u8) { let a = b as u8 }",
            "LANG-7",
            "casts are excluded",
        ),
        (
            "fn f(b:u8) { return b as u8; }",
            "LANG-7",
            "casts are excluded",
        ),
        ("fn f() { g(a: b: c) }", "LANG-12", "one label"),
        ("fn f()->T\n in x { x }", "LANG-7", "result relation"),
        (
            "fn f(x: T @ unique) {}",
            "LANG-7",
            "`@ unique` is removed: write `unique` before the type",
        ),
        (
            "fn f(x: T @ once) {}",
            "LANG-7",
            "`@ once` is removed: write `once` before the type",
        ),
        (
            "fn f(x: T @ sendable) {}",
            "LANG-7",
            "`@ sendable` is removed: write `sendable` before the type",
        ),
        (
            "fn f(x: T @ global) {}",
            "LANG-7",
            "`@ global` is removed: a value without a relation is global",
        ),
        (
            "fn f(x: T) -> T @ local(x) { x }",
            "LANG-7",
            "`@ local(x)` is removed: write `in x`",
        ),
        (
            "fn f(x: T, y: T) -> T @ local(x, y) { x }",
            "LANG-7",
            "write `in {x, y}`",
        ),
        (
            "fn f() { let g = fn() -> T @ local { t } }",
            "LANG-7",
            "write `in env`",
        ),
        ("fn f(x: unique unique T) {}", "LANG-7", "repeated"),
    ] {
        let (output, _) = support::parse(source);
        let diagnostic = &output.diagnostics[0];
        assert_eq!(diagnostic.rule.as_str(), rule, "{source}");
        assert!(
            diagnostic.message.contains(message),
            "{source}: {}",
            diagnostic.message
        );
    }
    let (output, _) = support::parse("type T {}\nfn f()->T\n@noalloc fn g() {}");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}
