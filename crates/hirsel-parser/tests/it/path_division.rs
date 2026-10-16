use crate::support;
use hirsel_syntax::*;

#[test]
fn versioned_value_paths_stop_before_unambiguous_division() {
    for source in [
        "fn f() { pkg@1/m.f / x }",
        "fn f() { pkg@1/m / 2u8 }",
        "fn f() { pkg@1/m/n.f / x }",
        "fn f() { pkg@1/m.type / x }",
    ] {
        let (output, _) = support::parse(source);
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
        let ItemKind::Function {
            body: Some(body), ..
        } = output.syntax.tree.items[output.syntax.roots[0].0 as usize].kind
        else {
            panic!()
        };
        let ExprKind::Block {
            tail: Some(tail), ..
        } = output.syntax.tree.expressions[body.0 as usize].kind
        else {
            panic!()
        };
        assert!(
            matches!(
                output.syntax.tree.expressions[tail.0 as usize].kind,
                ExprKind::Binary {
                    op: BinaryOp::Div,
                    ..
                }
            ),
            "{source}"
        );
    }
}

#[test]
fn type_paths_and_ambiguous_versioned_value_paths_keep_their_separators() {
    for source in ["type A = pkg@1/m.T/U", "fn f() { pkg@1/a / b }"] {
        let (output, _) = support::parse(source);
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
        assert!(!output.syntax.tree.expressions.iter().any(|e| matches!(
            e.kind,
            ExprKind::Binary {
                op: BinaryOp::Div,
                ..
            }
        )));
    }
}
