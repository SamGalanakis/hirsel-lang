use crate::support;
use hirsel_syntax::*;

fn clean(source: &str) -> SyntaxFile {
    // Spec examples use the language's lexer, including byte and raw literals.
    let mut sources = hirsel_span::SourceMap::default();
    let file = sources.add("fixture.hir", source.into()).unwrap();
    let lexed = hirsel_lexer::lex(file, &sources);
    let output = hirsel_parser::parse(file, lexed.tokens, &support::context(), &mut sources);
    let diagnostics: Vec<_> = lexed
        .diagnostics
        .iter()
        .chain(&output.diagnostics)
        .collect();
    assert!(
        diagnostics.is_empty(),
        "{source}\n{}",
        diagnostics
            .iter()
            .map(|d| d.render_human(&sources))
            .collect::<String>()
    );
    output.syntax
}

#[test]
fn every_hirsel_block_in_the_spec_parses_cleanly() {
    // Fenced `hirsel` blocks are complete files; grammar fragments, ledgers and
    // schematic forms are `ebnf` or `text` blocks.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec");
    let mut parts: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "md"))
        .collect();
    parts.sort();
    let mut blocks = 0;
    for part in parts {
        let text = std::fs::read_to_string(&part).unwrap();
        let mut rest = text.as_str();
        while let Some((_, after)) = rest.split_once("\n```hirsel\n") {
            let (block, next) = after.split_once("\n```").unwrap();
            clean(&format!("{block}\n"));
            blocks += 1;
            rest = next;
        }
    }
    assert!(blocks > 30, "{blocks} spec examples");
    // Complete executable inline declarations from the LANG and FACT rules.
    clean("fn rename(from src: Path, to dst: Path)");
    clean("distinct type Percent(v: u8) where v <= 100");
}

#[test]
fn snapshots_pin_every_declaration_and_expression_family() {
    let fixtures = [
        (
            "functions",
            "@noalloc pub fn f[T: plain + data,n: size](from x: T, into mut y: Inline[T,n], take z: T) -> T in x uses {x} requires true ensures result == before(x) { x }\n@blocking extern \"c\" fn external(u: Unsafe)\nfn generated() = comptime 1 + 2\n@incomplete fn hole() { todo(\"later\") }",
        ),
        (
            "types",
            "@layout(c) @align(8) @packed @soa pub type Record[T] { dir: Dir\n file: File of .dir, items: Inline[T,4] }\nflat type Flat { x: u32 }\nresource type Resource { dir: Dir } drop(x) {}\ndistinct type Percent(v: u8) where v <= 100\ntype Choice = A | B(u32) | C(x: box Choice)\ntype Single = Single()\ntype Alias = Map[String,T]\n@capability(global) pub opaque type Cap: plain uses {Cap}\npub const C: u32 = comptime f()",
        ),
        (
            "function_types",
            // Record fields write no locality (LANG-7, #71), so the
            // qualifier families are parameters.
            "fn function_types(a: fn(T, mut U, take V) -> once T,\n take b: once sendable @noalloc fn(into mut x: T) -> U uses {} in x,\n c: (fn() -> T) of origin in heap,\n take d: unique T in arena)",
        ),
        (
            "patterns",
            "fn f() { let Rec(x: x, y: (_, (a, b)), ..) = take r\n match x { -1 => 0, true => 1, false => 2, \"s\" => 3, b\"b\" => 4, () => 5, (v,) => v, Single(..,) => 6, V(a,b,) if a > b => a, _ => 7 } }",
        ),
        (
            "precedence",
            "fn f() { a..=b || c && d == e | f ^ g & h << i + j * -k\n return a + b * c }",
        ),
        (
            "postfix_and_literals",
            "fn f() { (json@1/value.parse(a: x, y)?.C)[i].type? Errors.make(z)\n ((), 1u32, 1.0f64, \"s\", b\"b\", true, false, [a,b], heap, result, f[T]) }",
        ),
        (
            "prefix_and_special",
            "fn f() { (-x, !x, ~x, take x, copy x, mut x, unique x, box x, comptime x, shape(T), before(x), todo) }",
        ),
        (
            "control",
            "fn f() { let mut x: u32 = 0\n x = 1\n x += 2\n x -= 1\n x *= 2\n x /= 2\n x %= 2\n x <<= 1\n x >>= 1\n x &= 1\n x |= 1\n x ^= 1\n while x > 0 { x -= 1 } for (a,b) in xs { continue } fn helper(x: T) { return x }\n if x > 0 { { x } } else if y { break } else { match x { A => 0\n B(v) => v } } }",
        ),
        (
            "closures",
            "fn f() { call(fn(x: T) -> U uses {x} requires true ensures result > 0 { return x })\n fn() { 1 } uses {} }",
        ),
    ];
    for (name, source) in fixtures {
        let file = clean(source);
        let mut snapshot = format!("{source}\n\n");
        for (i, syntax) in file.items.iter().enumerate() {
            // Anchors are process-unique: show every span relative to anchor 0,
            // which also checks that re-anchoring reaches every span.
            let mut syntax = (**syntax).clone();
            syntax.reanchor(hirsel_span::Anchor(0));
            let debug = format!("{syntax:?}");
            assert!(
                !debug.replace("Anchor(0)", "").contains("Anchor("),
                "an item span kept its anchor: {debug}"
            );
            let (item, tree) = (&syntax.item, &syntax.tree);
            snapshot.push_str(&format!(
                "item {i} {:?} {:?} occurrence={} hash={:02x?}\n  {:?} {:?}\n  {:?}\n",
                item.definition,
                item.origin,
                syntax.occurrence,
                syntax.text_hash.0,
                item.visibility,
                item.attributes,
                item.kind
            ));
            for (i, expr) in tree.expressions.iter().enumerate() {
                snapshot.push_str(&format!("expr {i} {:?}: {:?}\n", expr.origin, expr.kind));
            }
            for (i, stmt) in tree.statements.iter().enumerate() {
                snapshot.push_str(&format!("stmt {i} {:?}: {:?}\n", stmt.origin, stmt.kind));
            }
            for (i, pattern) in tree.patterns.iter().enumerate() {
                snapshot.push_str(&format!(
                    "pattern {i} {:?}: {:?}\n",
                    pattern.origin, pattern.kind
                ));
            }
            for (i, ty) in tree.types.iter().enumerate() {
                snapshot.push_str(&format!("type {i} {:?}: {:?}\n", ty.origin, ty.kind));
            }
        }
        insta::assert_snapshot!(name, snapshot);
    }
}

#[test]
fn serializable_has_no_attribute_payload_or_value_modifier_form() {
    for source in [
        "@serializable(1) type Row { n: u8 }",
        "fn f(x: serializable u8) {}",
        "@serializable fn f() {}",
    ] {
        let (out, _) = support::parse(source);
        assert!(!out.diagnostics.is_empty(), "{source}");
    }
    // Accepted syntax twins: an identifier type and a zero-argument type attribute.
    clean("type serializable { n: u8 }\nfn f(x: serializable) {}");
    clean("@serializable type Row { n: u8 }");
}

#[test]
fn an_extern_carries_its_attached_doc_comments_as_its_contract() {
    let contract = |source: &str| {
        let file = clean(source);
        let item = &file.items.last().unwrap().item;
        let ItemKind::Function { abi: Some(abi), .. } = &item.kind else {
            panic!("expected an extern");
        };
        assert_eq!(abi.name(), "c");
        abi.contract.clone()
    };
    assert_eq!(
        contract(
            "/// Reads `p`.\n///\n/// Retains nothing.\n@blocking\nextern \"c\" fn f(u: Unsafe)"
        ),
        Some("Reads `p`.\n\nRetains nothing.".into())
    );
    assert_eq!(
        contract("/** Borrowed only. */ extern \"c\" fn f(u: Unsafe)"),
        Some("Borrowed only.".into())
    );
    // A plain comment between the doc and the item detaches the doc.
    assert_eq!(
        contract("/// Earlier.\n// note\nextern \"c\" fn f(u: Unsafe)"),
        None
    );
    assert_eq!(contract("fn g()\n///\nextern \"c\" fn f(u: Unsafe)"), None);
}
