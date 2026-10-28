use crate::support;
use hirsel_intern::Symbol;
use hirsel_span::SourceMap;
use hirsel_syntax::*;
use std::sync::Arc;

#[test]
fn identity_ignores_file_positions_and_symbol_order_while_hashes_include_comments() {
    fn item(source: &str, path: &str, prefix_symbols: &[&str]) -> Arc<ItemSyntax> {
        let mut sources = SourceMap::default();
        let file = sources.add(path, source.into()).unwrap();
        for name in prefix_symbols {
            Symbol::intern(name);
        }
        let tokens = support::tokenize(sources.file_anchor(file), source);
        let output = hirsel_parser::parse(file, tokens, &support::context(), &mut sources);
        assert!(output.diagnostics.is_empty());
        output
            .syntax
            .items
            .into_iter()
            .find(|i| i.item.definition.0.name == "same")
            .unwrap()
    }
    let a = item("fn same() {}", "a.hir", &[]);
    let b = item(
        "fn unrelated() {}\nfn same() {}",
        "moved/b.hir",
        &["other", "symbols"],
    );
    assert_eq!(a.item.definition, b.item.definition);
    assert_eq!(
        a.item.definition.fingerprint(),
        b.item.definition.fingerprint()
    );
    // Item-local syntax: the same item text after another item has the ids
    // and offsets it has alone; only its anchor differs.
    let local = |syntax: &ItemSyntax| {
        let mut syntax = syntax.clone();
        syntax.reanchor(hirsel_span::Anchor(0));
        (syntax.item, syntax.tree)
    };
    assert_ne!(a.anchor, b.anchor);
    assert_eq!(local(&a), local(&b));
    assert_eq!(a.text_hash, b.text_hash);
    let c = item("fn same() {} // comment\n", "a.hir", &[]);
    assert_ne!(a.text_hash, c.text_hash);
    assert_eq!(a.text_hash, ContentHash::of_bytes(b"fn same() {}"));
    assert_eq!(
        c.text_hash,
        ContentHash::of_bytes(b"fn same() {} // comment\n")
    );
}

#[test]
fn trivia_attaches_to_items_and_final_orphan_trivia_stays_with_file() {
    let source = "// initial\r\nfn a() {} // a\r\n\r\n/// docs for b\r\n@noalloc\r\nfn b() {}\r\n// final file trivia\r\n";
    let (output, sources) = support::parse(source);
    assert!(output.diagnostics.is_empty());
    let file = output.syntax;
    let text = |tokens: &[Token]| {
        tokens
            .iter()
            .map(|t| sources.slice(t.span).unwrap())
            .collect::<String>()
    };
    assert_eq!(
        text(&file.items[0].tree.tokens),
        "// initial\r\nfn a() {} // a\r\n"
    );
    assert_eq!(
        text(&file.items[1].tree.tokens),
        "\r\n/// docs for b\r\n@noalloc\r\nfn b() {}\r\n"
    );
    assert_eq!(text(&file.tokens), "// final file trivia\r\n");
    assert_eq!(text(&file.tape().cloned().collect::<Vec<_>>()), source);
    for syntax in &file.items {
        assert_eq!(
            syntax.text_hash,
            ContentHash::of_bytes(text(&syntax.tree.tokens).as_bytes())
        );
        // Each item's tokens start at its anchor.
        assert_eq!(syntax.tree.tokens[0].span.anchor, syntax.anchor);
        assert_eq!(syntax.tree.tokens[0].span.start, 0);
        assert!(syntax.item.origin.tokens.end as usize <= syntax.tree.tokens.len());
    }
    let (out, _) = support::parse("// orphan only\n");
    assert!(out.syntax.items.is_empty());
    assert_eq!(out.syntax.tokens.len(), 3);
}

#[test]
fn changing_manifest_context_changes_identity_without_changing_text_hash() {
    let source = "fn f() {}";
    let mut sources = SourceMap::default();
    let file = sources.add("f.hir", source.into()).unwrap();
    let tokens = support::tokenize(sources.file_anchor(file), source);
    let mut context = support::context();
    let a = hirsel_parser::parse(file, tokens.clone(), &context, &mut sources)
        .syntax
        .items
        .remove(0);
    context.package.version = "2".into();
    context.modules.push("nested".into());
    let b = hirsel_parser::parse(file, tokens, &context, &mut sources)
        .syntax
        .items
        .remove(0);
    assert_ne!(a.item.definition, b.item.definition);
    assert_eq!(a.text_hash, b.text_hash);
}

#[test]
fn explicit_item_terminators_do_not_detach_trailing_comments() {
    let source = "fn a() {}; // trailing a\n/// b\nfn b() {}; // trailing b";
    let (output, sources) = support::parse(source);
    assert!(output.diagnostics.is_empty());
    let text = |syntax: &ItemSyntax| {
        syntax
            .tree
            .tokens
            .iter()
            .map(|t| sources.slice(t.span).unwrap())
            .collect::<String>()
    };
    assert_eq!(text(&output.syntax.items[0]), "fn a() {}; // trailing a\n");
    assert_eq!(
        text(&output.syntax.items[1]),
        "/// b\nfn b() {}; // trailing b"
    );
}

#[test]
fn newline_statement_ranges_stay_item_local_after_a_prefix() {
    let item = "fn same() -> u8 {\n let value: u8 = 1u8\n value\n}\n";
    let snapshot = |source: &str| {
        let (parsed, sources) = support::parse(source);
        assert!(parsed.diagnostics.is_empty());
        let mut syntax = (*parsed.syntax.items.last().unwrap().clone()).clone();
        for statement in &syntax.tree.statements {
            assert!(statement.origin.tokens.end as usize <= syntax.tree.tokens.len());
        }
        assert!(syntax.structurally_valid(|span| sources.slice(span).is_some()));
        syntax.reanchor(hirsel_span::Anchor(0));
        syntax
    };
    let alone = snapshot(item);
    let prefixed = snapshot(&format!("fn earlier() {{}}\n{item}"));
    assert_eq!(alone, prefixed);
}

#[test]
fn reanchoring_an_opaque_decode_check_moves_all_association_spans() {
    let source = "@format(date.format_into)\n@serializable(check = decoded_date)\npub opaque type Date: plain + key + sendable + inert + noalloc_drop uses {} { year: i32, month: u8, day: u8 }\n";
    let (output, _) = support::parse(source);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let mut item = (*output.syntax.items[0]).clone();
    let original = item.anchor;
    let moved = hirsel_span::Anchor(original.0 + 1000);
    let associations: Vec<_> = item
        .item
        .attributes
        .iter()
        .flat_map(|a| a.arguments.iter())
        .map(|arg| match arg {
            AttributeArgument::Format(a) => (a.function.clone(), a.origin.span),
            AttributeArgument::SerializableCheck(a) => (a.function.clone(), a.origin.span),
            _ => panic!("unexpected association"),
        })
        .collect();
    item.reanchor(moved);
    let remapped: Vec<_> = item
        .item
        .attributes
        .iter()
        .flat_map(|a| a.arguments.iter())
        .map(|arg| match arg {
            AttributeArgument::Format(a) => (a.function.clone(), a.origin.span),
            AttributeArgument::SerializableCheck(a) => (a.function.clone(), a.origin.span),
            _ => panic!("unexpected association"),
        })
        .collect();
    assert_eq!(associations.len(), 2);
    for ((expected_function, expected_span), (function, span)) in
        associations.into_iter().zip(remapped)
    {
        assert_eq!(function, expected_function);
        assert_eq!(
            (span.start, span.end),
            (expected_span.start, expected_span.end)
        );
        assert_eq!(
            span.anchor, moved,
            "association kept its previous source generation"
        );
    }
}
