use crate::support;
#[test]
fn closure_literals_reject_generic_binders_at_the_opening_bracket() {
    for binder in ["[T: plain]", "[n: size]", "\n[T: plain]"] {
        let source = format!("fn f() {{ let g = fn{binder}(x: u8) -> u8 {{ x }} }}");
        let (output, _) = support::parse(&source);
        let first = &output.diagnostics[0];
        assert_eq!(first.rule.as_str(), "LANG-1");
        assert_eq!(
            &source[first.primary.start as usize..first.primary.end as usize],
            "["
        );
    }
    for source in [
        "fn f[T: plain](x: T) -> T { x }",
        "fn f() { fn g[T: plain](x: T) -> T { x } }",
        "fn f() { let g = fn(x: u8) -> u8 { x } }",
    ] {
        let (output, _) = support::parse(source);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert!(output.nesting_limit.is_none());
    }
}
