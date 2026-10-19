use crate::support;
use hirsel_intern::Symbol;
use hirsel_span::SourceMap;
use hirsel_syntax::*;

#[test]
fn identity_ignores_file_positions_and_symbol_order_while_hashes_include_comments() {
    fn item(source: &str, path: &str, prefix_symbols: &[&str]) -> Item {
        let mut sources = SourceMap::default();
        let file = sources.add(path, source.into()).unwrap();
        for name in prefix_symbols {
            Symbol::intern(name);
        }
        let tokens = support::tokenize(file, source);
        let output = hirsel_parser::parse(file, tokens, &support::context(), &sources);
        assert!(output.diagnostics.is_empty());
        output
            .syntax
            .tree
            .items
            .into_iter()
            .find(|i| i.definition.0.name == "same")
            .unwrap()
    }
    let a = item("fn same() {}", "a.hir", &[]);
    let b = item(
        "fn unrelated() {}\nfn same() {}",
        "moved/b.hir",
        &["other", "symbols"],
    );
    assert_eq!(a.definition, b.definition);
    assert_eq!(a.definition.fingerprint(), b.definition.fingerprint());
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
    let owned = |range: TokenRange| {
        file.tokens[range.start as usize..range.end as usize]
            .iter()
            .map(|t| sources.slice(t.span).unwrap())
            .collect::<String>()
    };
    assert_eq!(
        owned(file.tree.items[0].owned_tokens),
        "// initial\r\nfn a() {} // a\r\n"
    );
    assert_eq!(
        owned(file.tree.items[1].owned_tokens),
        "\r\n/// docs for b\r\n@noalloc\r\nfn b() {}"
    );
    assert_eq!(
        owned(TokenRange {
            start: file.tree.items[1].owned_tokens.end,
            end: file.tokens.len() as u32
        }),
        "\r\n// final file trivia\r\n"
    );
    for item in &file.tree.items {
        assert_eq!(
            item.text_hash,
            ContentHash::of_bytes(owned(item.owned_tokens).as_bytes())
        );
        assert!(item.origin.tokens.start >= item.owned_tokens.start);
        assert!(item.origin.tokens.end <= item.owned_tokens.end);
    }
    let (out, _) = support::parse("// orphan only\n");
    assert!(out.syntax.roots.is_empty());
    assert_eq!(out.syntax.tokens.len(), 3);
}

#[test]
fn changing_manifest_context_changes_identity_without_changing_text_hash() {
    let source = "fn f() {}";
    let mut sources = SourceMap::default();
    let file = sources.add("f.hir", source.into()).unwrap();
    let tokens = support::tokenize(file, source);
    let mut context = support::context();
    let a = hirsel_parser::parse(file, tokens.clone(), &context, &sources)
        .syntax
        .tree
        .items
        .remove(0);
    context.package.version = "2".into();
    context.modules.push("nested".into());
    let b = hirsel_parser::parse(file, tokens, &context, &sources)
        .syntax
        .tree
        .items
        .remove(0);
    assert_ne!(a.definition, b.definition);
    assert_eq!(a.text_hash, b.text_hash);
}

#[test]
fn explicit_item_terminators_do_not_detach_trailing_comments() {
    let source = "fn a() {}; // trailing a\n/// b\nfn b() {}; // trailing b";
    let (output, sources) = support::parse(source);
    assert!(output.diagnostics.is_empty());
    let text = |item: &Item| {
        output.syntax.tokens[item.owned_tokens.start as usize..item.owned_tokens.end as usize]
            .iter()
            .map(|t| sources.slice(t.span).unwrap())
            .collect::<String>()
    };
    assert_eq!(
        text(&output.syntax.tree.items[0]),
        "fn a() {}; // trailing a\n"
    );
    assert_eq!(
        text(&output.syntax.tree.items[1]),
        "/// b\nfn b() {}; // trailing b"
    );
}
