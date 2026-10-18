//! Modes, ownership, brands and closure checks.

mod arenas;
pub mod authority;
mod explain;
pub mod ownership;

use hirsel_syntax::{ExprId, ExprKind, StmtKind, SyntaxTree};
/// The direct subexpressions of `e`, statements' expressions included.
pub(crate) fn expression_children(tree: &SyntaxTree, e: ExprId) -> Vec<ExprId> {
    match &tree.expressions[e.0 as usize].kind {
        ExprKind::Tuple(es) | ExprKind::Sequence(es) => es.clone(),
        ExprKind::Call { callee, arguments } => std::iter::once(*callee)
            .chain(arguments.iter().map(|a| a.value))
            .collect(),
        ExprKind::Field { base, .. } => vec![*base],
        ExprKind::Index { base, index } => vec![*base, *index],
        ExprKind::Unary { value, .. }
        | ExprKind::Try { value, .. }
        | ExprKind::Before(value)
        | ExprKind::Comptime(value) => vec![*value],
        ExprKind::Binary { left, right, .. } => vec![*left, *right],
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => std::iter::once(*condition)
            .chain([*then_branch])
            .chain(*else_branch)
            .collect(),
        ExprKind::Match { value, arms } => std::iter::once(*value)
            .chain(
                arms.iter()
                    .flat_map(|a| a.guard.into_iter().chain([a.body])),
            )
            .collect(),
        ExprKind::Closure { body, .. } => vec![*body],
        ExprKind::Return(value) => value.iter().copied().collect(),
        ExprKind::Block { statements, tail } => statements
            .iter()
            .flat_map(|s| match &tree.statements[s.0 as usize].kind {
                StmtKind::Let { value, .. } | StmtKind::Expr(value) => vec![*value],
                StmtKind::Assign { place, value, .. } => vec![*place, *value],
                StmtKind::While {
                    condition, body, ..
                } => vec![*condition, *body],
                StmtKind::For { source, body, .. } => vec![*source, *body],
                StmtKind::Function { body, .. } => vec![*body],
            })
            .chain(*tail)
            .collect(),
        _ => vec![],
    }
}
