//! `if let` and `while let` (LANG-13), desugared here into an ordinary match, so
//! no later phase has a conditional-let form.
use super::*;

impl Parser<'_> {
    /// After `if let`: `if let P = e A else B` becomes `match e { P => A, _ => B }`,
    /// with `{}` for an omitted else.
    pub(super) fn if_let(&mut self, start: usize) -> Parsed<ExprId> {
        let (pattern, value) = self.let_condition()?;
        let header = self.origin(start);
        let then_branch = self.block()?;
        let else_branch = if self.eat("else") {
            Some(if self.at("if") {
                self.expr(0)?
            } else {
                self.block()?
            })
        } else {
            None
        };
        let refused = else_branch.unwrap_or_else(|| {
            self.synthetic_expr(
                header,
                ExprKind::Block {
                    label: None,
                    statements: vec![],
                    tail: None,
                },
            )
        });
        let matching = self.refusing(start, pattern, value, then_branch, refused);
        self.tree.conditions.push(LetCondition::If {
            matching,
            otherwise: else_branch.is_some(),
        });
        Ok(matching)
    }
    /// After `while let`: `'l: while let P = e A` becomes
    /// `'l: while true { match e { P => A, _ => break } }`, so `e` is evaluated
    /// before each iteration and `break` and `continue` target the loop.
    pub(super) fn while_let(&mut self, label: Option<Symbol>, start: usize) -> Parsed<StmtKind> {
        let (pattern, value) = self.let_condition()?;
        let header = self.origin(start);
        let body = self.block()?;
        let exit = self.synthetic_expr(
            header,
            ExprKind::Break {
                label: None,
                value: None,
            },
        );
        let matching = self.refusing(start, pattern, value, body, exit);
        let whole = self.origin(start);
        let block = self.synthetic_expr(
            whole,
            ExprKind::Block {
                label: None,
                statements: vec![],
                tail: Some(matching),
            },
        );
        let condition = self.synthetic_expr(header, ExprKind::Literal(Literal::Bool(true)));
        self.tree.conditions.push(LetCondition::While {
            body: block,
            matching,
        });
        Ok(StmtKind::While {
            label,
            condition,
            body: block,
        })
    }
    /// `P = e` of a conditional let. The scrutinee cannot be an unparenthesized
    /// `&&` or `||`: there are no let chains.
    fn let_condition(&mut self) -> Parsed<(PatternId, ExprId)> {
        let pattern = self.pattern()?;
        self.expect("=", const { RuleId::of("LANG-13") })?;
        let value = self.required(|p| p.expr(0))?;
        if let ExprKind::Binary {
            op: BinaryOp::And | BinaryOp::Or,
            ..
        } = self.tree.expressions[value.0 as usize].kind
            && !self.tree.groups.iter().any(|g| g.expression == value)
        {
            return self.fail_at(
                const { RuleId::of("LANG-13") },
                self.tree.expressions[value.0 as usize].origin.span,
                "a conditional let does not chain; nest an if, or parenthesize a boolean scrutinee",
            );
        }
        Ok((pattern, value))
    }
    /// `match value { pattern => matched, _ => refused }`.
    fn refusing(
        &mut self,
        start: usize,
        pattern: PatternId,
        value: ExprId,
        matched: ExprId,
        refused: ExprId,
    ) -> ExprId {
        let refused_origin = self.tree.expressions[refused.0 as usize].origin;
        let wildcard = self.synthetic_pattern(refused_origin, PatternKind::Wildcard);
        self.push_expr(
            start,
            ExprKind::Match {
                value,
                arms: vec![
                    MatchArm {
                        pattern,
                        guard: None,
                        body: matched,
                    },
                    MatchArm {
                        pattern: wildcard,
                        guard: None,
                        body: refused,
                    },
                ],
            },
        )
    }
}
