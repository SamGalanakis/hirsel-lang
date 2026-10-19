use crate::support;
use hirsel_syntax::*;

#[test]
fn recovered_lexical_tokens_and_missing_tokens_mark_the_shared_status() {
    let (output, _) = support::parse("fn f() {}\n$");
    assert!(output.syntax.has_syntax_errors);
    let (output, _) = support::parse("fn f(x u32) {}");
    assert!(output.syntax.has_syntax_errors);
}

#[test]
fn comparison_range_assignment_and_return_binding_follow_the_table() {
    for source in [
        "fn f() { a < b < c }",
        "fn f() { a..b..c }",
        "fn f() { a = b = c }",
        "fn f() { ..b }",
        "fn f() { a.. }",
        "fn f() { (a = b) }",
    ] {
        assert!(!support::parse(source).0.diagnostics.is_empty(), "{source}");
    }
    for expression in ["(a < b) == c", "a..(b..c)", "(a..b)..c", "a < b && c < d"] {
        assert!(
            support::parse(&format!("fn f() {{ {expression} }}"))
                .0
                .diagnostics
                .is_empty(),
            "{expression}"
        );
    }
    let (output, _) = support::parse("fn f() { return a..b || c && d } ");
    assert!(output.diagnostics.is_empty());
    let returned = output
        .syntax
        .items
        .iter()
        .flat_map(|i| &i.tree.expressions)
        .find_map(|e| match e.kind {
            ExprKind::Return(Some(e)) => Some(e),
            _ => None,
        })
        .unwrap();
    assert!(matches!(
        output.syntax.items[0].tree.expressions[returned.0 as usize].kind,
        ExprKind::Binary {
            op: BinaryOp::Range,
            ..
        }
    ));
}

#[test]
fn function_type_modifiers_cover_what_follows_and_relations_bind_the_nearest_type() {
    let (output, _) = support::parse(
        "type A = fn() -> once T\ntype B = once fn() -> T\ntype C = fn() -> T in x\ntype D = fn() -> T uses {} in x",
    );
    assert!(output.diagnostics.is_empty());
    // Each alias's type, in its own item's arenas.
    let alias = |i: usize| {
        let syntax = &output.syntax.items[i];
        match syntax.item.kind {
            ItemKind::Type {
                body: TypeBody::Alias(ty),
                ..
            } => (&syntax.tree, ty),
            _ => panic!(),
        }
    };
    let (tree, ty) = alias(0);
    let TypeExprKind::Function(sig) = &tree.types[ty.0 as usize].kind else {
        panic!()
    };
    assert!(matches!(
        tree.types[sig.result.unwrap().0 as usize].kind,
        TypeExprKind::Qualified { .. }
    ));
    let (tree, ty) = alias(1);
    let TypeExprKind::Qualified { base, .. } = tree.types[ty.0 as usize].kind else {
        panic!()
    };
    let TypeExprKind::Function(sig) = &tree.types[base.0 as usize].kind else {
        panic!()
    };
    assert!(matches!(
        tree.types[sig.result.unwrap().0 as usize].kind,
        TypeExprKind::Named { .. }
    ));
    // A relation after a result binds the result; after `uses`, the function.
    let (tree, ty) = alias(2);
    let TypeExprKind::Function(sig) = &tree.types[ty.0 as usize].kind else {
        panic!()
    };
    assert!(matches!(
        tree.types[sig.result.unwrap().0 as usize].kind,
        TypeExprKind::Qualified { .. }
    ));
    let (tree, ty) = alias(3);
    let TypeExprKind::Qualified { base, .. } = tree.types[ty.0 as usize].kind else {
        panic!()
    };
    assert!(matches!(
        tree.types[base.0 as usize].kind,
        TypeExprKind::Function(_)
    ));
}

#[test]
fn numeric_size_arguments_are_unsuffixed_u64_literals_without_arithmetic() {
    for ty in [
        "Inline[T,0]",
        "Inline[T,18_446_744_073_709_551_615]",
        "Inline[T,0xff]",
        "Inline[T,n]",
    ] {
        assert!(
            support::parse(&format!("type A = {ty}"))
                .0
                .diagnostics
                .is_empty(),
            "{ty}"
        );
    }
    for ty in [
        "Inline[T,-1]",
        "Inline[T,1u64]",
        "Inline[T,18446744073709551616]",
        "Inline[T,n+1]",
        "Inline[]",
    ] {
        assert!(
            !support::parse(&format!("type A = {ty}"))
                .0
                .diagnostics
                .is_empty(),
            "{ty}"
        );
    }
}

#[test]
fn package_paths_are_adjacent_and_unversioned_slash_is_division() {
    let (output, _) = support::parse("fn f() { a/b }\nfn g() { json@1/value.parse(x) }");
    assert!(output.diagnostics.is_empty());
    assert!(
        output
            .syntax
            .items
            .iter()
            .flat_map(|i| &i.tree.expressions)
            .any(|e| matches!(
                e.kind,
                ExprKind::Binary {
                    op: BinaryOp::Div,
                    ..
                }
            ))
    );
    assert!(
        output
            .syntax
            .items
            .iter()
            .flat_map(|i| &i.tree.expressions)
            .any(|e| matches!(&e.kind, ExprKind::Path(path) if path.package.is_some()))
    );
    for expression in [
        "json @1/value.parse(x)",
        "json@ 1/value.parse(x)",
        "json@1 /value.parse(x)",
        "json@1u8/value.parse(x)",
    ] {
        assert!(
            !support::parse(&format!("fn f() {{ {expression} }}"))
                .0
                .diagnostics
                .is_empty(),
            "{expression}"
        );
    }
}

#[test]
fn incomplete_bindings_continue_types_but_complete_type_aliases_do_not_reopen() {
    for source in [
        "fn f() { let x: T\n in x\n = y }",
        "const C: T\n in heap\n = y",
    ] {
        assert!(support::parse(source).0.diagnostics.is_empty(), "{source}");
    }
    for source in [
        "type A = T\n in env",
        "type A = fn()\n -> T",
        "type A = fn() -> T\n uses {}",
        "type A = A\n | B",
        "type A = A | B\n | C",
    ] {
        assert!(!support::parse(source).0.diagnostics.is_empty(), "{source}");
    }
}
