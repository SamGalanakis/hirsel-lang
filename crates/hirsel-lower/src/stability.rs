//! A bounded, conservative source-effect proof. V still independently checks
//! the ordinary read loan; no elision flag or trusted IR evidence is published.
use super::builder::Builder;
use hirsel_syntax::{Argument, ExprKind, StmtKind};
use hirsel_tir::PlaceId;
use hirsel_types::{SourceAccess, UseKind};

impl Builder<'_, '_> {
    pub(super) fn callee_is_stable(&self, place: PlaceId, arguments: &[Argument]) -> bool {
        let root = self.root(place);
        // An already retained exclusive alias (for example a scoped callback)
        // can write the root without naming it in the argument syntax.
        // Large frames fall back too, keeping the proof's work bounded.
        if self.state.loans.len() > 256 || self.has_mutable_loan_on(root) {
            return false;
        }
        let mut pending: Vec<_> = arguments.iter().map(|a| a.value).collect();
        let mut budget = 256;
        while let Some(id) = pending.pop() {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            // Every explicit source access and consumer is checked. Reading
            // sibling fields is safe; moving or mutating any part is not.
            if self.resolved_place(id).is_ok_and(|p| self.root(p) == root) {
                let Ok(source) = self.source_place(id) else {
                    return false;
                };
                let Ok(decision) = self.decision(id) else {
                    return false;
                };
                if matches!(
                    source.access,
                    SourceAccess::Take | SourceAccess::Mut | SourceAccess::Unique
                ) || matches!(decision.usage, UseKind::Take | UseKind::Mut)
                {
                    return false;
                }
            }
            match &self.input.tree.expressions[id.0 as usize].kind {
                ExprKind::Literal(_)
                | ExprKind::Path(_)
                | ExprKind::Formatter(_)
                | ExprKind::Heap
                | ExprKind::Break(_)
                | ExprKind::Continue(_)
                | ExprKind::Todo(_) => {}
                ExprKind::Tuple(xs) | ExprKind::Sequence(xs) => pending.extend(xs),
                ExprKind::Field { base, .. } => pending.push(*base),
                ExprKind::Index { base, index } => pending.extend([*base, *index]),
                ExprKind::Unary { value, .. }
                | ExprKind::Before(value)
                | ExprKind::Comptime(value) => pending.push(*value),
                ExprKind::Binary { left, right, .. } => pending.extend([*left, *right]),
                ExprKind::Call { callee, arguments } => {
                    pending.push(*callee);
                    for argument in arguments {
                        if self
                            .resolved_place(argument.value)
                            .is_ok_and(|p| self.root(p) == root)
                            && self
                                .decisions
                                .arguments
                                .get(&(id, argument.value))
                                .iter()
                                .any(|row| matches!(row.2, UseKind::Take | UseKind::Mut))
                        {
                            return false;
                        }
                        pending.push(argument.value);
                    }
                }
                ExprKind::Try {
                    value,
                    map_error: None,
                } => pending.push(*value),
                ExprKind::Block { statements, tail } => {
                    pending.extend(tail);
                    for statement in statements {
                        match &self.input.tree.statements[statement.0 as usize].kind {
                            StmtKind::Let {
                                value, otherwise, ..
                            } => pending.extend(std::iter::once(*value).chain(*otherwise)),
                            StmtKind::Expr(value) => pending.push(*value),
                            StmtKind::Assign { place, value, .. } => {
                                // Unknown storage is not a proof of disjointness.
                                let Ok(target) = self.resolved_place(*place) else {
                                    return false;
                                };
                                if self.root(target) == root {
                                    return false;
                                }
                                pending.extend([*place, *value]);
                            }
                            // Hidden loop calls and new capture frames need a
                            // separate effect proof. Keep their snapshot.
                            StmtKind::While { .. }
                            | StmtKind::For { .. }
                            | StmtKind::Function { .. } => return false,
                        }
                    }
                }
                ExprKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    pending.extend([*condition, *then_branch]);
                    pending.extend(else_branch);
                }
                ExprKind::Match { value, arms } => {
                    pending.push(*value);
                    for arm in arms {
                        pending.extend(arm.guard);
                        pending.push(arm.body);
                    }
                }
                ExprKind::Return(value) => pending.extend(value),
                // Captures, implicit mapper inputs and materialized shape code
                // are deliberately outside this small proof.
                ExprKind::Closure { .. }
                | ExprKind::Try { .. }
                | ExprKind::Shape(_)
                | ExprKind::Result
                | ExprKind::Error => return false,
            }
        }
        true
    }
}
