use crate::support;
use hirsel_syntax::*;
use proptest::prelude::*;

#[test]
fn damaged_statements_and_items_keep_later_definitions() {
    let (output, _) = support::parse(
        "fn broken(x) { 0 }\nfn usable() {\nlet x = ;\nlet y = 2\ny\n}\nconst Z: u32 = 3\n",
    );
    assert_eq!(output.syntax.roots.len(), 2);
    assert_eq!(output.syntax.tree.items[0].definition.0.name, "usable");
    assert_eq!(output.syntax.tree.items[1].definition.0.name, "Z");
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-6");
    assert!(
        output
            .syntax
            .tree
            .expressions
            .iter()
            .any(|e| e.kind == ExprKind::Error)
    );
}

#[test]
fn nesting_is_an_incomplete_tooling_outcome() {
    support::with_compiler_stack(|| {
        for source in [
            format!(
                "fn deep() {{ {}0{} }}",
                "(".repeat(10000),
                ")".repeat(10000)
            ),
            format!("fn deep() {{ {}0 }}", "- ".repeat(10000)),
            format!(
                "fn deep() {{ {}0{} }}",
                "{".repeat(10000),
                "}".repeat(10000)
            ),
            format!("type Deep = {}T", "box ".repeat(10000)),
            format!(
                "fn deep() {{ let {}x{} = 0 }}",
                "Ctor(".repeat(10000),
                ")".repeat(10000)
            ),
        ] {
            let (output, _) = support::parse(&source);
            assert!(output.nesting_limit.is_some(), "{source}");
            assert!(output.syntax.has_syntax_errors);
            assert!(
                !output
                    .diagnostics
                    .iter()
                    .any(|d| d.message.contains("nesting limit"))
            );
        }
    });
}

#[test]
fn stale_spans_and_missing_keyword_symbols_are_diagnosed_without_panicking() {
    use hirsel_intern::Interner;
    use hirsel_span::{FileId, SourceMap, Span};
    let mut sources = SourceMap::default();
    let file = sources
        .add("stale.hir", "fn f() { x.type }".into())
        .unwrap();
    let mut names = Interner::default();
    let mut tokens = support::tokenize(file, sources.file(file).unwrap().text(), &mut names);
    for token in &mut tokens {
        token.span.file = FileId(u32::MAX);
        token.span.start = u32::MAX;
        token.span.end = 0;
    }
    let out = hirsel_parser::parse(file, tokens, &support::context(), &sources, &names);
    assert!(!out.diagnostics.is_empty());
    let mut names = Interner::default();
    let f = names.intern("f");
    let x = names.intern("x");
    let kinds = [
        TokenKind::Keyword(Keyword::Fn),
        TokenKind::Ident(f),
        TokenKind::Punct(Punct::LParen),
        TokenKind::Punct(Punct::RParen),
        TokenKind::Punct(Punct::LBrace),
        TokenKind::Ident(x),
        TokenKind::Punct(Punct::Dot),
        TokenKind::Keyword(Keyword::Type),
        TokenKind::Punct(Punct::RBrace),
    ];
    let ranges = [
        (0, 2),
        (3, 4),
        (4, 5),
        (5, 6),
        (7, 8),
        (9, 10),
        (10, 11),
        (11, 15),
        (16, 17),
    ];
    let tokens = kinds
        .into_iter()
        .zip(ranges)
        .map(|(kind, (start, end))| Token {
            kind,
            span: Span { file, start, end },
        })
        .collect();
    let out = hirsel_parser::parse(file, tokens, &support::context(), &sources, &names);
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.message.contains("symbol snapshot"))
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]
    #[test]
    fn arbitrary_token_sequences_never_panic(words in prop::collection::vec(0usize..99, 0..512), corrupt in any::<bool>()) {
        use hirsel_intern::Interner;
        use hirsel_span::{FileId, SourceMap};
        // Covers every punctuation/keyword category, trivia, invalids, literals,
        // identifiers and EOF at arbitrary positions, independent of lexical validity.
        const WORDS: &[&str] = &["fn", "let", "mut", "match", "if", "else", "for", "in", "while", "break",
            "continue", "return", "take", "copy", "unique", "type", "distinct", "flat", "resource", "drop",
            "box", "pub", "opaque", "const", "comptime", "shape", "heap", "before", "requires", "ensures",
            "result", "where", "uses", "of", "local", "global", "once", "todo", "extern", "unsafe", "true", "false",
            "(", ")", "[", "]", "{", "}", ",", ":", ";", ".", "..", "..=", "@", "?", "->", "=>", "+", "-", "*",
            "/", "%", "!", "~", "&", "|", "^", "&&", "||", "<<", ">>", "=", "==", "!=", "<", "<=", ">", ">=",
            "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=", "x", "0", "1.5", "\"s\"", "b\"b\"",
            "\n", "/*\n*/", "// hi\n", "$", "_", ""];
        let source = words.iter().map(|&i| WORDS[i]).collect::<Vec<_>>().join(" ");
        let mut sources = SourceMap::default();
        let file = sources.add("random.hir", source.clone()).unwrap();
        let mut names = Interner::default();
        // Build each entry individually so concatenation cannot change token categories.
        let mut tokens = vec![];
        let mut offset = 0u32;
        for &i in &words {
            let mut entry = support::tokenize(file, WORDS[i], &mut names);
            if !WORDS[i].is_empty() { entry.pop(); }
            for mut token in entry {
                token.span.start += offset; token.span.end += offset;
                if corrupt {
                    token.span.file = FileId(u32::MAX);
                    if matches!(token.kind, TokenKind::Ident(_)) { token.kind = TokenKind::Ident(hirsel_intern::Symbol(u32::MAX)); }
                }
                tokens.push(token);
            }
            offset += WORDS[i].len() as u32 + 1;
        }
        let original = tokens.clone();
        let output = hirsel_parser::parse(file, tokens, &support::context(), &sources, &names);
        prop_assert_eq!(&output.syntax.tokens, &original);
        for expr in &output.syntax.tree.expressions {
            prop_assert!(expr.origin.tokens.start <= expr.origin.tokens.end);
            prop_assert!(expr.origin.tokens.end as usize <= original.len());
        }
    }
}

#[test]
fn recovery_consumes_inner_braces_and_preserves_the_enclosing_block() {
    for statement in [
        "for i in 0u8...3u8 { }",
        "match x { A => {} B => {} }",
        "let g = fn(u8) { 1 }",
        "for i in 0u8...3u8 { let bad = ; let more = ; }",
        "for i in 0u8...3u8 {\nlet bad = ;\n}\n",
    ] {
        let source = format!("fn broken() {{ {statement} }}\nfn usable() {{}}\n");
        let (output, _) = support::parse(&source);
        assert_eq!(
            output.diagnostics.len(),
            1,
            "{source}: {:?}",
            output.diagnostics
        );
        assert_eq!(output.syntax.roots.len(), 2, "{source}");
        assert_eq!(output.syntax.tree.items[1].definition.0.name, "usable");
    }
}

#[test]
fn item_recovery_does_not_publish_nested_definitions_from_a_bad_header() {
    let (output, _) = support::parse("fn broken(x) {\nfn inner() {}\n}\nfn usable() {}\n");
    assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-6");
    assert_eq!(output.syntax.roots.len(), 1);
    assert_eq!(output.syntax.tree.items[0].definition.0.name, "usable");
}
