use crate::support;
use hirsel_syntax::*;

fn clean(source: &str) -> SyntaxFile {
    let (output, sources) = support::parse(source);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{}",
        output
            .diagnostics
            .iter()
            .map(|d| d.render_human(&sources))
            .collect::<String>()
    );
    output.syntax
}

#[test]
fn grammar_document_examples_parse_in_their_grammatical_contexts() {
    // docs/spec/syntax.md uses fragments at their respective nonterminals. EBNF,
    // excluded spellings, metavariables and schematic ellipses are not programs.
    for declaration in [
        "fn rename(from src: Path, to dst: Path)",
        "fn append[T: copyable](into mut xs: List[T], take x: T)",
        "pub opaque type Handle: inert uses {}",
        "@capability(global) pub opaque type Dir: plain uses {Dir}",
        "@capability(local) pub opaque type Arena: inert uses {}",
        "@noalloc pub fn saxpy[n: size](x: Inline[f32,n], mut y: Inline[f32,n])",
        "@incomplete fn pending() { todo(\"pending\") }",
        "@blocking extern \"c\" fn read(u: Unsafe, n: u64)",
        "type Only = Only()",
        "type Alias = Name",
        "type Tree = Leaf | Node(box Tree)",
        "type Rec { x: T, dir: Dir, file: File of .dir }",
        "resource type R { dir: Dir }\ndrop(x) {}",
        "distinct type Percent(v: u8) where v <= 100",
    ] {
        clean(declaration);
    }
    for ty in [
        "()",
        "(T)",
        "(T,)",
        "(T,T)",
        "box T",
        "T in x",
        "T in {x,y}",
        "T in {p.a, env}",
        "T in env",
        "once T in x",
        "unique sendable T",
        "File of dir",
        "File of .dir",
        "Task[R] of s",
        "SlotId[T] of slots",
        "StrPos of s",
        "SortedMap[K,V] of by",
        "T in a",
        "T in heap",
        "@noalloc fn(T) -> U",
        "fn() -> once T",
        "once fn() -> T",
        "once fn() -> () uses {}",
        "(fn() -> T) in x",
        "fn() -> T uses {} in x",
        "fn(mut T, take U, V) -> ()",
        "fn(into mut xs: List[T]) -> T in xs",
        "fn(T) -> U uses {Dir}",
        "Packed[T]",
        "Inline[T,n]",
        "Simd[T,4]",
        "Shared[T]",
        "Region[T]",
        "Slots[T]",
        "SlotId[T]",
        "Task[T]",
        "Sender[T]",
        "Receiver[T]",
        "Atomic[T]",
        "Iter[T]",
        "FnPtr[T]",
        "Ptr[T]",
        "String",
        "Bytes",
        "json@1/Value",
        "pkg/path.name",
    ] {
        clean(&format!("type Example = {ty}"));
    }
    for expression in [
        "json@1/value.parse(x)",
        "a/b",
        "module.member(x)",
        "x.type",
        "heap",
        "result",
        "f[T]",
        "f[U]",
        "g[(T,T)]",
        "shape(T)",
        "before(list.len(xs))",
        "comptime e",
        "f()? C(x)",
        "f()? C",
        "f()?",
        "[a,b]",
        "[]",
        "()",
        "(a,b)",
        "(a,)",
        "take x",
        "copy x",
        "mut r.f",
        "mut xs[i]",
        "unique x",
        "box e",
        "-128i8",
        "a..b",
        "a..=b",
        "a + b * c",
        "fn(tx: Tx) { 0 }",
        "fn() { 0 } uses {}",
        "fn() uses {} { 0 }",
        "fn() -> T requires true ensures result == 0 { 0 }",
        "if x { a } else if y { b } else { c }",
        "match x { A => a, B(v) if v > 0 => v, }",
        "arena.with(fn(a: Arena) { x })",
        "mutex.with(tasks,m,fn(mut v: unique T) {} uses {})",
        "{ let y = x\n y }",
        "return x + y",
        "break",
        "continue",
        "'done: { 0 }",
        "'done: { break 'done x + y }",
        "todo",
        "todo(\"later\")",
    ] {
        clean(&format!("fn example() {{ {expression} }}"));
    }
    for statement in [
        "let Percent(v) = p",
        "let Rec(items: items, ..) = take r",
        "let (_, (a,b)) = pair",
        "let x: T = e",
        "let mut x = e",
        "for x in xs {}",
        "for x in mut xs {}",
        "for x in take it {}",
        "while x {}",
        "fn helper(x: T) { x }",
        "x += y",
        "x = y",
    ] {
        clean(&format!("fn example() {{ {statement} }}"));
    }
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
        for (i, item) in file.tree.items.iter().enumerate() {
            snapshot.push_str(&format!(
                "item {i} {:?} {:?} {:?} hash={:02x?}\n  {:?} {:?}\n  {:?}\n",
                item.definition,
                item.origin,
                item.owned_tokens,
                item.text_hash.0,
                item.visibility,
                item.attributes,
                item.kind
            ));
        }
        for (i, expr) in file.tree.expressions.iter().enumerate() {
            snapshot.push_str(&format!("expr {i} {:?}: {:?}\n", expr.origin, expr.kind));
        }
        for (i, stmt) in file.tree.statements.iter().enumerate() {
            snapshot.push_str(&format!("stmt {i} {:?}: {:?}\n", stmt.origin, stmt.kind));
        }
        for (i, pattern) in file.tree.patterns.iter().enumerate() {
            snapshot.push_str(&format!(
                "pattern {i} {:?}: {:?}\n",
                pattern.origin, pattern.kind
            ));
        }
        for (i, ty) in file.tree.types.iter().enumerate() {
            snapshot.push_str(&format!("type {i} {:?}: {:?}\n", ty.origin, ty.kind));
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
        let item = file.tree.items.last().unwrap();
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
