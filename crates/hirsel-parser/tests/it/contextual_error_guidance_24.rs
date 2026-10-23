use crate::support;

#[test]
fn contextual_error_guidance_24_inline_position_keeps_rule_and_span() {
    for gap in [" ", " /* context */ "] {
        let source = format!(
            "// café 🐑\nfn f() {{ let n = parse()?{gap}fn(take e: E) -> F {{ wrap(e) }} }}"
        );
        let (output, sources) = support::parse(&source);
        let d = &output.diagnostics[0];
        assert_eq!(d.rule.as_str(), "LANG-1");
        assert_eq!(
            d.message,
            "expected newline or semicolon after complete statement/item"
        );
        assert_eq!(sources.slice(d.primary), Some("fn"));
        let notes = d.notes.join("\n");
        assert!(notes.contains("unary named path"));
        assert!(notes.contains("results.map_error(operation(), fn(take error:"));
        assert!(notes.contains("immutable local"));
        assert!(notes.contains("explicit match"));
        assert!(d.fix.is_none());
    }
}

#[test]
fn contextual_error_guidance_24_legal_separators_and_call_arguments() {
    for body in [
        "let n = parse()?; n",
        "let n = parse()? convert; n",
        "let n = parse()? errors.convert; n",
        "let n = parse()? /* same line */ errors.convert; n",
        "let n = parse()?\nlet mapper = fn(take e: E) -> F { wrap(e) }; n",
        "let n = parse()? // comment\nlet mapper = fn(take e: E) -> F { wrap(e) }; n",
        "let n = parse()? /*\n comment */ let mapper = fn(take e: E) -> F { wrap(e) }; n",
        "let n = parse()?; fn helper() {}",
        "results.map_error(parse(), fn(take e: E) -> F { wrap(e) })?",
        "call(parse()?, fn(take e: E) -> F { wrap(e) })",
    ] {
        let (output, _) = support::parse(&format!("fn f() {{ {body} }}"));
        assert!(
            output.diagnostics.is_empty(),
            "{body}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn contextual_error_guidance_24_unrelated_missing_separator_has_no_recipe() {
    for body in [
        "let n = parse() fn(take e: E) -> F { wrap(e) }",
        "let n = parse()? convert fn(take e: E) -> F { wrap(e) }",
        "let n = 1 let other = 2",
        "let n = parse()? fn helper() {}",
    ] {
        let (output, _) = support::parse(&format!("fn f() {{ {body} }}"));
        let d = &output.diagnostics[0];
        assert_eq!(d.rule.as_str(), "LANG-1");
        assert_eq!(
            d.message,
            "expected newline or semicolon after complete statement/item"
        );
        assert!(d.notes.is_empty(), "{d:?}");
    }
}
