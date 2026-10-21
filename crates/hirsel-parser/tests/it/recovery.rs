use crate::support;
use hirsel_syntax::*;
use proptest::prelude::*;

#[test]
fn damaged_statements_and_items_keep_later_definitions() {
    let (output, _) = support::parse(
        "fn broken(x) { 0 }\nfn usable() {\nlet x = ;\nlet y = 2\ny\n}\nconst Z: u32 = 3\n",
    );
    assert_eq!(output.syntax.items.len(), 2);
    assert_eq!(output.syntax.items[0].item.definition.0.name, "usable");
    assert_eq!(output.syntax.items[1].item.definition.0.name, "Z");
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-6");
    assert!(
        output
            .syntax
            .items
            .iter()
            .flat_map(|i| &i.tree.expressions)
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
fn stale_spans_are_diagnosed_and_hand_built_keyword_members_parse() {
    use hirsel_intern::Symbol;
    use hirsel_span::{Anchor, SourceMap, Span};
    let mut sources = SourceMap::default();
    let file = sources
        .add("stale.hir", "fn f() { x.type }".into())
        .unwrap();
    let anchor = sources.file_anchor(file);
    let mut tokens = support::tokenize(anchor, sources.file(file).unwrap().text());
    for token in &mut tokens {
        token.span.anchor = Anchor(u32::MAX);
        token.span.start = u32::MAX;
        token.span.end = 0;
    }
    let out = hirsel_parser::parse(file, tokens, &support::context(), &mut sources);
    assert!(!out.diagnostics.is_empty());
    let f = Symbol::intern("f");
    let x = Symbol::intern("x");
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
            span: Span { anchor, start, end },
        })
        .collect();
    // No lexer ran, so nothing interned `type`: the parser interns the member.
    let out = hirsel_parser::parse(file, tokens, &support::context(), &mut sources);
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]
    #[test]
    fn arbitrary_token_sequences_never_panic(words in prop::collection::vec(0usize..99, 0..512), corrupt in any::<bool>()) {
        use hirsel_span::{Anchor, SourceMap};
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
        let anchor = sources.file_anchor(file);
        // Build each entry individually so concatenation cannot change token categories.
        let mut tokens = vec![];
        let mut offset = 0u32;
        for &i in &words {
            let mut entry = support::tokenize(anchor, WORDS[i]);
            if !WORDS[i].is_empty() { entry.pop(); }
            for mut token in entry {
                token.span.start += offset; token.span.end += offset;
                if corrupt {
                    token.span.anchor = Anchor(u32::MAX);
                }
                tokens.push(token);
            }
            offset += WORDS[i].len() as u32 + 1;
        }
        let original = tokens.clone();
        let output = hirsel_parser::parse(file, tokens, &support::context(), &mut sources);
        // The tape keeps every token, each at its original place.
        let placed = |t: &Token| (t.kind.clone(), sources.resolve(t.span));
        prop_assert_eq!(
            output.syntax.tape().map(placed).collect::<Vec<_>>(),
            original.iter().map(placed).collect::<Vec<_>>()
        );
        for syntax in &output.syntax.items {
            for expr in &syntax.tree.expressions {
                prop_assert!(expr.origin.tokens.start <= expr.origin.tokens.end);
                prop_assert!(expr.origin.tokens.end as usize <= syntax.tree.tokens.len());
            }
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
        assert_eq!(output.syntax.items.len(), 2, "{source}");
        assert_eq!(output.syntax.items[1].item.definition.0.name, "usable");
    }
}

#[test]
fn item_recovery_does_not_publish_nested_definitions_from_a_bad_header() {
    let (output, _) = support::parse("fn broken(x) {\nfn inner() {}\n}\nfn usable() {}\n");
    assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-6");
    assert_eq!(output.syntax.items.len(), 1);
    assert_eq!(output.syntax.items[0].item.definition.0.name, "usable");
}

#[test]
fn pattern_mut_suggests_let_mut_before_the_whole_pattern() {
    let (accepted, _) = support::parse("fn probe() { let mut (stream, _) = (1u64, 2u64) }");
    assert!(
        accepted.diagnostics.is_empty(),
        "{:?}",
        accepted.diagnostics
    );
    let (rejected, _) = support::parse("fn probe() { let (mut stream, _) = (1u64, 2u64) }");
    let first = &rejected.diagnostics[0];
    assert_eq!(first.rule.as_str(), "LANG-13");
    assert!(
        first.message.contains("let mut (stream, _)"),
        "{}",
        first.message
    );
}

#[test]
fn reserved_binding_names_recover_without_followup_syntax_errors() {
    for name in ["todo", "before", "result", "distinct"] {
        let source = format!(
            "fn example() {{ let {name} = 1u64\n let next = {name}\n next }}\nfn after() {{}}\n"
        );
        let (output, _) = support::parse(&source);
        assert_eq!(
            output.diagnostics.len(),
            1,
            "{source}: {:?}",
            output.diagnostics
        );
        assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-14");
        assert!(output.diagnostics[0].message.contains("reserved for"));
        assert_eq!(output.syntax.items.len(), 2);
        assert!(output.syntax.has_syntax_errors);
        assert!(
            !output
                .syntax
                .items
                .iter()
                .flat_map(|i| &i.tree.expressions)
                .any(|e| matches!(e.kind, ExprKind::Error))
        );
    }
}

#[test]
fn reserved_rename_preserves_references_without_a_partial_edit() {
    let (output, _) =
        support::parse("fn probe() { let value = 0u64\n let result = 1u64\n result }");
    let [d] = &output.diagnostics[..] else {
        panic!("{:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "LANG-14");
    assert!(d.fix.is_none());
    assert!(d.notes[0].contains("`value_2`"));
    let (fixed, _) =
        support::parse("fn probe() { let value = 0u64\n let value_2 = 1u64\n value_2 }");
    assert!(fixed.diagnostics.is_empty(), "{:?}", fixed.diagnostics);
}
