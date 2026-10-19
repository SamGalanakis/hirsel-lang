hirsel_guard::install!();

use hirsel_span::{SourceMap, Span};
use hirsel_syntax::*;
fn parse(source: &str) -> (SyntaxFile, SourceMap, bool) {
    let mut sources = SourceMap::default();
    let file = sources.add("test.hir", source.into()).unwrap();
    let lex = hirsel_lexer::lex(file, &sources);
    let context = FileContext {
        package: PackageId {
            name: "formatter_test".into(),
            version: "1".into(),
        },
        modules: vec![],
    };
    let parsed = hirsel_parser::parse(file, lex.tokens, &context, &mut sources);
    (
        parsed.syntax,
        sources,
        !lex.diagnostics.is_empty() || !parsed.diagnostics.is_empty(),
    )
}
fn comments(file: &SyntaxFile, sources: &SourceMap) -> Vec<String> {
    file.tape()
        .filter(|t| {
            matches!(
                t.kind,
                TokenKind::Trivia(
                    TriviaKind::LineComment
                        | TriviaKind::DocLine
                        | TriviaKind::BlockComment
                        | TriviaKind::DocBlock
                )
            )
        })
        .map(|t| sources.slice(t.span).unwrap().into())
        .collect()
}
/// Each item with its source coordinates, tokens and hash erased.
fn normalized(file: &SyntaxFile) -> Vec<(Item, SyntaxTree)> {
    file.items
        .iter()
        .map(|syntax| normalized_item(syntax))
        .collect()
}
fn normalized_item(syntax: &ItemSyntax) -> (Item, SyntaxTree) {
    let mut item = syntax.item.clone();
    let mut tree = syntax.tree.clone();
    let origin = Origin {
        span: Span::default(),
        tokens: TokenRange::default(),
    };
    fn attrs(attrs: &mut [Attribute], origin: Origin) {
        for a in attrs {
            a.origin = origin;
            for argument in &mut a.arguments {
                match argument {
                    AttributeArgument::LoopSource(s) => s.origin = origin,
                    AttributeArgument::StatePromise(s) => s.origin = origin,
                    AttributeArgument::Format(s) => s.origin = origin,
                    _ => (),
                }
            }
        }
    }
    fn sig(s: &mut FunctionSyntax, origin: Origin) {
        attrs(&mut s.attributes, origin);
    }
    for a in &mut tree.analyses {
        a.origin = origin;
    }
    tree.tokens.clear();
    {
        item.origin = origin;
        item.name_span = Span::default();
        attrs(&mut item.attributes, origin);
        match &mut item.kind {
            ItemKind::Function { signature, .. } => sig(signature, origin),
            ItemKind::Type { body, .. } => match body {
                TypeBody::Record(fields) => {
                    for f in fields {
                        f.origin = origin;
                    }
                }
                TypeBody::Distinct { field, .. } => field.origin = origin,
                _ => {}
            },
            _ => {}
        }
    }
    for e in &mut tree.expressions {
        e.origin = origin;
        if let ExprKind::Closure { signature, .. } = &mut e.kind {
            sig(signature, origin);
        }
    }
    for s in &mut tree.statements {
        s.origin = origin;
        if let StmtKind::Function { signature, .. } = &mut s.kind {
            sig(signature, origin);
        }
    }
    for p in &mut tree.patterns {
        p.origin = origin;
    }
    // Written parentheses are source detail; the formatter keeps only those the
    // tree needs.
    tree.groups.clear();
    for t in &mut tree.types {
        t.origin = origin;
        match &mut t.kind {
            TypeExprKind::Function(signature) => sig(signature, origin),
            TypeExprKind::Qualified {
                relation: Some(relation),
                ..
            } => relation.origin = origin,
            _ => {}
        }
    }
    (item, tree)
}
#[test]
fn adversarial_comment_newline_and_projection_boundaries_roundtrip() {
    for source in [
        "resource type Token { values: unique List[u32] @ growable }\n/* drop */ @noalloc drop(x) { () }",
        "pub opaque type Marker: () uses {}\nfn f[T: noalloc_drop](take x: T) {}",
        "pub opaque type Omitted uses {}",
        "fn main() { (a // left\n + b) }",
        "fn main() { (a /*\nblock */ + b) }",
        "fn main() { (x).type }",
        "fn main() { ((x).type) }",
        "fn main() { (f() ? Err).field }",
        "fn main() { f(fn() { // body\n x } uses {}) }",
        "fn main() { f(fn() uses {} { // body\n x }) }",
        "fn main() -> (T // result\n) in env",
        "fn main() -> (T // relation\n in env)",
        "fn main() -> once (fn() -> T // closure type\n)",
        "type Alias = (T // alias\n)",
        "fn main() { (a+b)[0] }",
        "fn main() { (a // base\n)[0] }",
        "fn main() { (a // callee\n)(x) }",
        "fn main() { (a // value\n)? }",
        "fn main() { (a // member\n).type }",
        "fn main() { f((a // nested\n + b)) }",
        "fn main() { let x = f()?\n C }",
        "fn main() { return\n x }",
        "fn main() { x; }",
        "fn main() { if let Some(x) = (a || b) { x } else if let None = y {} else { 1 } }",
        "fn main() { if let A(x) | B(x) = take t { x; } }",
        "fn main() { 'l: while let Some(x) = list.pop(mut xs) { continue 'l } }",
        "fn main() { let Some(x) = a + (if c { a } else { b }) else { return } }",
        "fn main() { let Some(x) = (match c { _ => a }) else { return } }",
        "/// doc\nfn a() {} // end\n\n/// next\nfn b() {}\n// orphan",
        // Leading analysis annotations precede the item's span (PS-19).
        "/// doc\n@analysis(a) fn f() {}",
        "@analysis(a)\nfn f(@analysis(u: m) x: u64) {}",
        "fn g() {}\n\n// lead\n@analysis(a) // after\n@noalloc\nfn f() {}",
        "// c\n@analysis(a)\n/// d\ntype R {\n  @analysis(u) x: u64,\n}",
    ] {
        let (original, sources, errors) = parse(source);
        assert!(!errors, "{source}");
        let first = hirsel_fmt::format(&original, &sources);
        assert!(!first.unchanged, "{source}");
        let (reparsed, new_sources, errors) = parse(&first.source);
        assert!(!errors, "{}", first.source);
        let before_comments = comments(&original, &sources);
        assert_eq!(
            normalized(&original),
            normalized(&reparsed),
            "{}",
            first.source
        );
        assert_eq!(before_comments, comments(&reparsed, &new_sources));
        let second = hirsel_fmt::format(&reparsed, &new_sources);
        assert!(!second.unchanged);
        assert_eq!(first.source, second.source);
    }
}

#[test]
fn real_spec_examples_reparse_to_equal_trees_and_match_the_hand_built_goldens() {
    let source = include_str!("fixtures/spec-overview-original.hir");
    let (original, sources, errors) = parse(source);
    assert!(!errors);
    let formatted = hirsel_fmt::format(&original, &sources);
    assert!(!formatted.unchanged);
    assert_eq!(
        formatted.source,
        format!(
            "{}\n",
            include_str!("fixtures/spec-overview.hir").trim_end()
        )
    );
    let (reparsed, new_sources, errors) = parse(&formatted.source);
    assert!(!errors);
    assert_eq!(
        comments(&original, &sources),
        comments(&reparsed, &new_sources)
    );
    assert_eq!(normalized(&original), normalized(&reparsed));
    let again = hirsel_fmt::format(&reparsed, &new_sources);
    assert!(!again.unchanged);
    assert_eq!(formatted.source, again.source);
}

#[test]
fn outer_uses_survives_function_typed_results() {
    for source in [
        "fn f() -> (fn()) uses {} { g }",
        "fn outer() { fn f() -> (fn()) uses {a} { g } }",
        "fn outer() { let f = fn() -> (fn()) uses {a} { g } }",
        "fn outer() { let f = fn() -> (fn()) { g } uses {a} }",
        "type F = fn() -> (fn()) uses {a}",
        "type F = fn() -> box (fn()) uses {a}",
        "type F = fn() -> (fn() -> fn()) uses {a}",
        "type F = fn() -> (fn() -> fn() uses {}) uses {a}",
    ] {
        let (original, sources, errors) = parse(source);
        assert!(!errors, "{source}");
        let first = hirsel_fmt::format(&original, &sources);
        assert!(!first.unchanged, "{source}");
        let (reparsed, new_sources, errors) = parse(&first.source);
        assert!(!errors, "{}", first.source);
        assert_eq!(
            normalized(&original),
            normalized(&reparsed),
            "{}",
            first.source
        );
        let second = hirsel_fmt::format(&reparsed, &new_sources);
        assert!(!second.unchanged);
        assert_eq!(first.source, second.source);
    }
}

#[test]
fn leading_header_blank_line_matches_golden_and_is_idempotent() {
    let source = include_str!("fixtures/leading-header-original.hir");
    let expected = include_str!("fixtures/leading-header.hir");
    let (file, sources, errors) = parse(source);
    assert!(!errors);
    let formatted = hirsel_fmt::format(&file, &sources);
    assert!(!formatted.unchanged);
    assert_eq!(formatted.source, expected);
    let (file, sources, errors) = parse(expected);
    assert!(!errors);
    assert_eq!(hirsel_fmt::format(&file, &sources).source, expected);
    for source in [
        "// A file header.\n\npub fn one() -> u64 uses {} {\n  1\n}\n",
        "// Attached comment.\npub fn one() -> u64 uses {} {\n  1\n}\n",
        "/// Item documentation.\npub fn one() -> u64 uses {} {\n  1\n}\n",
    ] {
        let (file, sources, errors) = parse(source);
        assert!(!errors);
        assert_eq!(hirsel_fmt::format(&file, &sources).source, source);
    }
}

#[test]
fn repeat_literals_retain_written_form_and_comments() {
    let source = "fn f(){let a: Inline[u8, 64]=[0u8 /* value */; /* count */ 64]}";
    let expected = "fn f() {\n  let a: Inline[u8, 64] = [0u8 /* value */; /* count */ 64]\n}\n";
    let (file, sources, errors) = parse(source);
    assert!(!errors);
    let formatted = hirsel_fmt::format(&file, &sources);
    assert!(!formatted.unchanged);
    assert_eq!(formatted.source, expected);
    let (file, sources, errors) = parse(expected);
    assert!(!errors);
    assert_eq!(hirsel_fmt::format(&file, &sources).source, expected);
}
