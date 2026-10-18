//! Bottom-up heights: every arena edge is charged, including nodes built in loops.
use super::*;

#[derive(Default)]
pub(super) struct Heights {
    pub expressions: Vec<usize>,
    pub statements: Vec<usize>,
    pub types: Vec<usize>,
    pub patterns: Vec<usize>,
}
impl Heights {
    fn expr(&self, id: ExprId) -> usize {
        self.expressions[id.0 as usize]
    }
    fn ty(&self, id: TypeExprId) -> usize {
        self.types[id.0 as usize]
    }
    fn pattern(&self, id: PatternId) -> usize {
        self.patterns[id.0 as usize]
    }
    fn signature(&self, sig: &FunctionSyntax) -> usize {
        sig.parameters
            .iter()
            .map(|p| self.ty(p.ty))
            .chain(sig.result.map(|id| self.ty(id)))
            .chain(sig.contracts.iter().map(|c| self.expr(c.expression)))
            .max()
            .unwrap_or(0)
    }
    pub fn expression(&self, kind: &ExprKind) -> usize {
        let child = match kind {
            ExprKind::Tuple(xs) | ExprKind::Sequence(xs) => {
                xs.iter().map(|id| self.expr(*id)).max().unwrap_or(0)
            }
            ExprKind::Call { callee, arguments } => arguments
                .iter()
                .map(|a| self.expr(a.value))
                .chain([self.expr(*callee)])
                .max()
                .unwrap(),
            ExprKind::Field { base, .. } => self.expr(*base),
            ExprKind::Index { base, index } => self.expr(*base).max(self.expr(*index)),
            ExprKind::Unary { value, .. }
            | ExprKind::Try { value, .. }
            | ExprKind::Before(value)
            | ExprKind::Comptime(value) => self.expr(*value),
            ExprKind::Binary { left, right, .. } => self.expr(*left).max(self.expr(*right)),
            ExprKind::Block { statements, tail } => statements
                .iter()
                .map(|id| self.statements[id.0 as usize])
                .chain(tail.map(|id| self.expr(id)))
                .max()
                .unwrap_or(0),
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self
                .expr(*condition)
                .max(self.expr(*then_branch))
                .max(else_branch.map_or(0, |id| self.expr(id))),
            ExprKind::Match { value, arms } => arms
                .iter()
                .map(|a| {
                    self.pattern(a.pattern)
                        .max(a.guard.map_or(0, |id| self.expr(id)))
                        .max(self.expr(a.body))
                })
                .chain([self.expr(*value)])
                .max()
                .unwrap(),
            ExprKind::Closure { signature, body } => {
                self.signature(signature).max(self.expr(*body))
            }
            ExprKind::Return(value) => value.map_or(0, |id| self.expr(id)),
            ExprKind::Shape(id) => self.ty(*id),
            ExprKind::Literal(_)
            | ExprKind::Path(_)
            | ExprKind::Formatter(_)
            | ExprKind::Break(_)
            | ExprKind::Continue(_)
            | ExprKind::Heap
            | ExprKind::Result
            | ExprKind::Todo(_)
            | ExprKind::Error => 0,
        };
        child + 1
    }
    pub fn statement(&self, kind: &StmtKind) -> usize {
        let child = match kind {
            StmtKind::Let {
                pattern, ty, value, ..
            } => self
                .pattern(*pattern)
                .max(ty.map_or(0, |id| self.ty(id)))
                .max(self.expr(*value)),
            StmtKind::Assign { place, value, .. } => self.expr(*place).max(self.expr(*value)),
            StmtKind::Expr(id) => return self.expr(*id), // statement wrapper is transparent
            StmtKind::While {
                condition, body, ..
            } => self.expr(*condition).max(self.expr(*body)),
            StmtKind::For {
                pattern,
                source,
                body,
                ..
            } => self
                .pattern(*pattern)
                .max(self.expr(*source))
                .max(self.expr(*body)),
            StmtKind::Function {
                signature, body, ..
            } => self.signature(signature).max(self.expr(*body)),
        };
        child + 1
    }
    pub fn type_expression(&self, kind: &TypeExprKind) -> usize {
        let child = match kind {
            TypeExprKind::Named { arguments, .. } => arguments
                .iter()
                .filter_map(|a| match a {
                    TypeArgument::Type(id) => Some(self.ty(*id)),
                    _ => None,
                })
                .max()
                .unwrap_or(0),
            TypeExprKind::Tuple(xs) => xs.iter().map(|id| self.ty(*id)).max().unwrap_or(0),
            TypeExprKind::Function(sig) => self.signature(sig),
            TypeExprKind::Box(id) | TypeExprKind::Qualified { base: id, .. } => self.ty(*id),
            TypeExprKind::Error => 0,
        };
        child + 1
    }
    pub fn pattern_expression(&self, kind: &PatternKind) -> usize {
        1 + match kind {
            PatternKind::Tuple(xs) => xs.iter().map(|id| self.pattern(*id)).max().unwrap_or(0),
            PatternKind::Constructor { fields, .. } => fields
                .iter()
                .map(|f| self.pattern(f.pattern))
                .max()
                .unwrap_or(0),
            _ => 0,
        }
    }
}
