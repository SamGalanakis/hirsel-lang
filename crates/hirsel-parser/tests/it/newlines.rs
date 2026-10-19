use crate::support;
use hirsel_syntax::*;

fn body(source: &str) -> (std::sync::Arc<ItemSyntax>, ExprId) {
    let (output, sources) = support::parse(source);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        output
            .diagnostics
            .iter()
            .map(|d| d.render_human(&sources))
            .collect::<String>()
    );
    let ItemKind::Function {
        body: Some(body), ..
    } = output.syntax.items[0].item.kind
    else {
        panic!("function body")
    };
    (output.syntax.items[0].clone(), body)
}
fn block(file: &ItemSyntax, id: ExprId) -> (&[StmtId], Option<ExprId>) {
    let ExprKind::Block {
        statements, tail, ..
    } = &file.tree.expressions[id.0 as usize].kind
    else {
        panic!("block")
    };
    (statements, *tail)
}

#[test]
fn newline_terminates_complete_expressions_without_reopening() {
    let (file, b) = body("fn f() {\n a\n (b)\n [c]\n}\n");
    let (statements, tail) = block(&file, b);
    assert_eq!(statements.len(), 2);
    assert!(matches!(
        file.tree.expressions[tail.unwrap().0 as usize].kind,
        ExprKind::Sequence(_)
    ));
    for next in ["+ b", ".field", "? C"] {
        let (output, _) = support::parse(&format!("fn f() {{ a\n {next} }}"));
        assert!(!output.diagnostics.is_empty(), "{next} must not reopen a");
    }
    let (file, b) = body("fn f() {\n a\n -b\n}");
    assert_eq!(block(&file, b).0.len(), 1); // Unary minus starts its own statement.
    assert!(matches!(
        file.tree.expressions[block(&file, b).1.unwrap().0 as usize].kind,
        ExprKind::Unary { .. }
    ));
}

#[test]
fn required_operands_continue_and_groups_ignore_newlines() {
    let (file, b) = body("fn f() { let x =\n a +\n b\n (x\n + c\n .member\n (d)\n [e])\n }");
    let (statements, tail) = block(&file, b);
    assert_eq!(statements.len(), 1);
    assert!(tail.is_some());
    let StmtKind::Let { value, .. } = file.tree.statements[statements[0].0 as usize].kind else {
        panic!()
    };
    assert!(matches!(
        file.tree.expressions[value.0 as usize].kind,
        ExprKind::Binary {
            op: BinaryOp::Add,
            ..
        }
    ));
}

#[test]
fn nested_closure_block_restores_statement_context() {
    let (file, _) = body("fn f() { call(fn() { a\n b\n }, c\n + d) }");
    let inner = file
        .tree
        .expressions
        .iter()
        .find(|e| matches!(&e.kind, ExprKind::Block { statements, .. } if statements.len() == 1))
        .unwrap();
    assert!(matches!(inner.kind, ExprKind::Block { tail: Some(_), .. }));
}

#[test]
fn return_break_continue_are_complete_at_statement_newlines() {
    let (file, b) = body("fn f() { return\n x }");
    let (statements, tail) = block(&file, b);
    assert_eq!(statements.len(), 1);
    let StmtKind::Expr(returned) = file.tree.statements[statements[0].0 as usize].kind else {
        panic!()
    };
    assert_eq!(
        file.tree.expressions[returned.0 as usize].kind,
        ExprKind::Return(None)
    );
    assert!(matches!(
        file.tree.expressions[tail.unwrap().0 as usize].kind,
        ExprKind::Path(_)
    ));
    for word in ["break", "continue"] {
        let (file, b) = body(&format!("fn f() {{ {word}\n x }}"));
        assert_eq!(block(&file, b).0.len(), 1);
    }
}

#[test]
fn exceptional_continuations_cross_blank_lines_and_comments() {
    let (file, b) = body(
        "fn f()\n// signature\n-> u32\nuses {}\nrequires true\nensures result > 0\n{ if true { 1 }\n\n// else\nelse if false { 2 } else { 3 } }\n",
    );
    let (_, tail) = block(&file, b);
    assert!(matches!(
        file.tree.expressions[tail.unwrap().0 as usize].kind,
        ExprKind::If {
            else_branch: Some(_),
            ..
        }
    ));
    let (output, _) = support::parse(
        "@noalloc\n// waits\npub\nfn f()\n=\ncomptime 1 + 2\nresource type R {}\n// drop belongs to R\ndrop(r) {}\ndistinct type P(x: u8)\nwhere x < 10\n",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.syntax.items.len(), 3);
    let (file, _) = body("fn f() { call(fn() {}\nuses {}) }");
    assert!(file.tree.expressions.iter().any(
        |e| matches!(&e.kind, ExprKind::Closure { signature, .. } if signature.uses == Some(vec![]))
    ));
}

#[test]
fn question_mapper_starts_on_same_physical_line() {
    for separator in ["\n", "/*\n*/", "\r\n"] {
        let (file, b) = body(&format!("fn f() {{ f()?{separator} C }}"));
        let (statements, _) = block(&file, b);
        assert_eq!(statements.len(), 1);
        assert!(file.tree.expressions.iter().any(|e| matches!(
            e.kind,
            ExprKind::Try {
                map_error: None,
                ..
            }
        )));
        let (output, _) = support::parse(&format!("fn f() {{ (f()?{separator} C) }}"));
        assert!(!output.diagnostics.is_empty());
    }
    let (file, _) = body("fn f() { f()? C(x) }");
    let callee = file
        .tree
        .expressions
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                ExprKind::Try {
                    map_error: Some(_),
                    ..
                }
            )
        })
        .unwrap();
    assert!(matches!(callee.kind, ExprKind::Try { .. }));
    let calls = file
        .tree
        .expressions
        .iter()
        .filter(|e| matches!(e.kind, ExprKind::Call { .. }))
        .count();
    assert_eq!(calls, 2); // f(), then call the propagated success value with x.
}

#[test]
fn block_tail_semicolons_and_same_line_block_expressions() {
    let (file, b) =
        body("fn f() { if true {} let x = 1; while false {} for y in xs {} fn g() {} {} x\n }");
    assert_eq!(block(&file, b).0.len(), 6);
    assert!(block(&file, b).1.is_some());
    let (file, b) = body("fn f() { 1; }");
    assert_eq!(block(&file, b).0.len(), 1);
    assert_eq!(block(&file, b).1, None);
    let (file, b) = body("fn f() { 1\n\n/* end */ }");
    assert!(block(&file, b).1.is_some());
    let (output, _) = support::parse("fn f() { a b }");
    assert_eq!(output.diagnostics[0].rule.as_str(), "LANG-1");
}

#[test]
fn list_braces_allow_newline_but_tuple_and_call_lists_require_commas() {
    let (output, _) =
        support::parse("type R { a: u32\n b: u32,\n }\nfn f() { match x { A => a\n B => b,\n } }");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for source in [
        "fn f() { g(a\n b) }",
        "fn f(a: T\n b: U) {}",
        "type R = (T\n U)",
        "type R = List[T\n U]",
        "fn f() { match x { A => a\n + b } }",
    ] {
        let (output, _) = support::parse(source);
        assert!(!output.diagnostics.is_empty(), "{source}");
    }
}

#[test]
fn controls_and_closures_cannot_complete_before_their_required_bodies() {
    for source in [
        "fn f() { if a\n && b {} }",
        "fn f() { while a\n && b {} }",
        "fn f() { for x in a\n ..b {} }",
        "fn f() { match x\n (y) { A if a\n && b => 1 } }",
        "fn f() { let closure = fn() -> T\n in env\n {} }",
        "fn f() { fn helper() -> T\n in heap\n {} }",
        "fn f() { (return\n x) }",
    ] {
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
    }
}
