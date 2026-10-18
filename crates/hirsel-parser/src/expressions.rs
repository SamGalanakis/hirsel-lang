use super::*;

impl Parser<'_> {
    pub(super) fn push_expr(&mut self, start: usize, kind: ExprKind) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        let height = self.heights.expression(&kind);
        self.check_height(height);
        self.heights.expressions.push(height);
        self.tree.expressions.push(Expr {
            origin: self.origin(start),
            kind,
        });
        id
    }
    pub(super) fn expr(&mut self, minimum: u8) -> Parsed<ExprId> {
        self.enter()?;
        let result = self.expression_inner(minimum);
        self.depth -= 1;
        self.complete(result)
    }
    fn expression_inner(&mut self, minimum: u8) -> Parsed<ExprId> {
        let start = self.pos;
        let mut left = self.primary()?;
        let mut nonassoc = None;
        while self.continuing() {
            if self.nesting_limit.is_some() {
                return Err(());
            }
            if minimum <= 11 {
                if self.eat("(") {
                    let arguments = self.list(")", |p| {
                        let label = if p.ident(0) && p.text_at(1) == ":" {
                            let name = p.name()?;
                            p.pos += 1;
                            Some(name)
                        } else {
                            None
                        };
                        let value = p.expr(0)?;
                        if p.at(":") {
                            return p.fail(
                                const { RuleId::of("LANG-12") },
                                "an argument has one label before its value",
                            );
                        }
                        // #51: retain written argument order pending the label-order ruling.
                        Ok(Argument { label, value })
                    })?;
                    left = self.push_expr(
                        start,
                        ExprKind::Call {
                            callee: left,
                            arguments,
                        },
                    );
                    continue;
                }
                if self.eat("[") {
                    self.whitespace += 1;
                    let result = self.expr(0);
                    self.whitespace -= 1;
                    let index = result?;
                    self.expect("]", const { RuleId::of("LANG-1") })?;
                    left = self.push_expr(start, ExprKind::Index { base: left, index });
                    continue;
                }
                if self.eat(".") {
                    let field = self.member()?;
                    left = self.push_expr(start, ExprKind::Field { base: left, field });
                    continue;
                }
                if self.eat("?") {
                    let map_error = if !self.newline() && self.ident(0) {
                        Some(self.path(false)?)
                    } else {
                        None
                    };
                    left = self.push_expr(
                        start,
                        ExprKind::Try {
                            value: left,
                            map_error,
                        },
                    );
                    continue;
                }
            }
            let Some((precedence, op)) = binary(self.text_at(0)) else {
                break;
            };
            if precedence < minimum {
                break;
            }
            if nonassoc == Some(precedence) {
                return self.fail(
                    const { RuleId::of("LANG-1") },
                    "comparisons and ranges cannot chain without parentheses",
                );
            }
            self.pos += 1;
            let right = self.expr(precedence + 1)?;
            left = self.push_expr(start, ExprKind::Binary { op, left, right });
            if matches!(precedence, 1 | 4) {
                nonassoc = Some(precedence);
            }
        }
        if self.continuing() && self.at("as") {
            return self.fail(
                const { RuleId::of("LANG-7") },
                "casts are excluded; use named conversions",
            );
        }
        Ok(left)
    }
    fn primary(&mut self) -> Parsed<ExprId> {
        let start = self.pos;
        if self.at_format(&[FormatPiece::Whole, FormatPiece::Head]) {
            return self.format_string();
        }
        if let Some(literal) = self.literal() {
            return Ok(self.push_expr(start, ExprKind::Literal(literal)));
        }
        if let Some(op) = unary(self.text_at(0)) {
            self.pos += 1;
            let value = self.expr(11)?;
            return Ok(self.push_expr(start, ExprKind::Unary { op, value }));
        }
        if self.eat("comptime") {
            let value = self.expr(11)?;
            return Ok(self.push_expr(start, ExprKind::Comptime(value)));
        }
        if self.ident(0) {
            let path = self.path(false)?;
            return Ok(self.push_expr(start, ExprKind::Path(path)));
        }
        let kind = if self.eat("(") {
            if self.eat(")") {
                ExprKind::Tuple(vec![])
            } else {
                self.whitespace += 1;
                let result = (|| {
                    let first = self.expr(0)?;
                    if self.at(":") {
                        return self.fail(
                            const { RuleId::of("LANG-12") },
                            "tuple entries cannot have labels",
                        );
                    }
                    if self.eat(",") {
                        let mut values = vec![first];
                        values.extend(self.list(")", |p| p.expr(0))?);
                        Ok(self.push_expr(start, ExprKind::Tuple(values)))
                    } else {
                        self.expect(")", const { RuleId::of("LANG-1") })?;
                        let height = self.heights.expressions[first.0 as usize] + 1;
                        self.check_height(height);
                        self.heights.expressions[first.0 as usize] = height;
                        Ok(first)
                    }
                })();
                self.whitespace -= 1;
                return result;
            }
        } else if self.eat("[") {
            ExprKind::Sequence(self.list("]", |p| p.expr(0))?)
        } else if self.at("{") {
            return self.block_context();
        } else if self.eat("if") {
            if self.at("let") {
                return self.fail(
                    const { RuleId::of("LANG-13") },
                    "if let is excluded; use match",
                );
            }
            let condition = self.required(|p| p.expr(0))?;
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
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            }
        } else if self.eat("match") {
            let value = self.required(|p| p.expr(0))?;
            self.expect("{", const { RuleId::of("LANG-7") })?;
            let outer = self.whitespace;
            self.whitespace = 0;
            let result = (|| {
                let mut arms = vec![];
                while !self.eat("}") {
                    let pattern = self.pattern()?;
                    let guard = if self.eat("if") {
                        Some(self.required(|p| p.expr(0))?)
                    } else {
                        None
                    };
                    self.expect("=>", const { RuleId::of("LANG-7") })?;
                    let body = self.expr(0)?;
                    arms.push(MatchArm {
                        pattern,
                        guard,
                        body,
                    });
                    self.entry_end("}")?;
                }
                Ok(arms)
            })();
            self.whitespace = outer;
            ExprKind::Match {
                value,
                arms: result?,
            }
        } else if self.eat("fn") {
            let mut signature = self.required(|p| {
                if p.at("[") {
                    return p.fail(
                        const { RuleId::of("LANG-1") },
                        "closure literals cannot declare generic parameters",
                    );
                }
                p.header(false, true)
            })?;
            let body = self.block()?;
            if self.at("uses") {
                if signature.uses.is_some() {
                    return self.fail(
                        const { RuleId::of("CLO-4") },
                        "closure cannot specify uses twice",
                    );
                }
                signature.uses = self.uses()?;
            }
            ExprKind::Closure { signature, body }
        } else if self.eat("return") {
            // #53: retain grouping rule 1 (newlines are whitespace) pending its
            // precedence against the bare-return newline rule.
            let value = if (self.whitespace == 0 && self.newline())
                || self.eof()
                || matches!(self.text_at(0), ";" | "}" | ")" | "]" | ",")
            {
                None
            } else {
                Some(self.expr(0)?)
            };
            ExprKind::Return(value)
        } else if self.eat("break") {
            ExprKind::Break(self.exit_label())
        } else if self.eat("continue") {
            ExprKind::Continue(self.exit_label())
        } else if self.eat("heap") {
            ExprKind::Heap
        } else if self.eat("result") {
            ExprKind::Result
        } else if self.eat("shape") {
            self.expect("(", const { RuleId::of("CT-3") })?;
            self.whitespace += 1;
            let result = self.ty();
            self.whitespace -= 1;
            let ty = result?;
            self.expect(")", const { RuleId::of("LANG-1") })?;
            ExprKind::Shape(ty)
        } else if self.eat("before") {
            self.expect("(", const { RuleId::of("FACT-1") })?;
            self.whitespace += 1;
            let result = self.expr(0);
            self.whitespace -= 1;
            let value = result?;
            self.expect(")", const { RuleId::of("LANG-1") })?;
            ExprKind::Before(value)
        } else if self.eat("todo") {
            let message = if self.eat("(") {
                if !matches!(self.token(0).map(|t| &t.kind), Some(TokenKind::String)) {
                    return self.fail(
                        const { RuleId::of("LANG-6") },
                        "todo message must be a string literal",
                    );
                }
                let message = self.text_at(0).into();
                self.pos += 1;
                self.expect(")", const { RuleId::of("LANG-6") })?;
                Some(message)
            } else {
                None
            };
            ExprKind::Todo(message)
        } else {
            return self.fail(const { RuleId::of("LANG-1") }, "expected expression");
        };
        Ok(self.push_expr(start, kind))
    }
    pub(super) fn block(&mut self) -> Parsed<ExprId> {
        self.enter()?;
        let result = self.block_context();
        self.depth -= 1;
        self.complete(result)
    }
    fn block_context(&mut self) -> Parsed<ExprId> {
        let outer = self.whitespace;
        self.whitespace = 0;
        let result = self.block_inner();
        self.whitespace = outer;
        self.complete(result)
    }
    fn block_inner(&mut self) -> Parsed<ExprId> {
        let start = self.pos;
        self.expect("{", const { RuleId::of("LANG-1") })?;
        let mut statements = vec![];
        let mut tail = None;
        while !self.eat("}") {
            if self.nesting_limit.is_some() {
                return Err(());
            }
            if self.eof() {
                return self.fail(
                    const { RuleId::of("LANG-1") },
                    "expected closing block brace",
                );
            }
            if self.eat(";") {
                continue;
            }
            let stmt_start = self.pos;
            match self.statement() {
                Ok(kind) => {
                    let implicit = match &kind {
                        StmtKind::For { .. }
                        | StmtKind::While { .. }
                        | StmtKind::Function { .. } => true,
                        StmtKind::Expr(e) => self.block_valued(*e),
                        _ => false,
                    };
                    match self.end(implicit) {
                        Ok(semicolon) => {
                            if let StmtKind::Expr(e) = kind {
                                if !semicolon && self.at("}") {
                                    tail = Some(e);
                                    continue;
                                }
                            }
                            let id = StmtId(self.tree.statements.len() as u32);
                            let mut origin = self.origin(stmt_start);
                            if self.newline() {
                                // The logical NL terminator lives between significant tokens.
                                let end = self
                                    .significant
                                    .get(self.pos)
                                    .copied()
                                    .unwrap_or(self.tokens.len());
                                origin.tokens.end = end as u32;
                            }
                            let height = self.heights.statement(&kind);
                            self.check_height(height);
                            self.heights.statements.push(height);
                            self.tree.statements.push(Stmt { origin, kind });
                            statements.push(id);
                        }
                        Err(()) => {
                            self.recover_statement(stmt_start);
                        }
                    }
                }
                Err(()) => {
                    self.recover_statement(stmt_start);
                    let error = self.push_expr(stmt_start, ExprKind::Error);
                    let id = StmtId(self.tree.statements.len() as u32);
                    self.heights.statements.push(1);
                    self.tree.statements.push(Stmt {
                        origin: self.origin(stmt_start),
                        kind: StmtKind::Expr(error),
                    });
                    statements.push(id);
                }
            }
        }
        Ok(self.push_expr(start, ExprKind::Block { statements, tail }))
    }
    /// Whether an expression's last token closes a block, closure or match:
    /// parentheses are not retained, so `(if ..)` ends in one too.
    fn ends_in_block(&self, mut e: ExprId) -> bool {
        loop {
            match &self.tree.expressions[e.0 as usize].kind {
                ExprKind::Block { .. }
                | ExprKind::If { .. }
                | ExprKind::Match { .. }
                | ExprKind::Closure { .. } => return true,
                ExprKind::Unary { value, .. } | ExprKind::Return(Some(value)) => e = *value,
                ExprKind::Binary { right, .. } => e = *right,
                _ => return false,
            }
        }
    }
    fn block_valued(&self, e: ExprId) -> bool {
        matches!(
            self.tree.expressions[e.0 as usize].kind,
            ExprKind::Block { .. } | ExprKind::If { .. } | ExprKind::Match { .. }
        )
    }
    fn exit_label(&mut self) -> Option<Symbol> {
        if self.whitespace == 0 && self.newline() {
            return None;
        }
        let TokenKind::Label(label) = self.token(0)?.kind else {
            return None;
        };
        self.pos += 1;
        Some(label)
    }
    fn statement(&mut self) -> Parsed<StmtKind> {
        let label = if let Some(Token {
            kind: TokenKind::Label(label),
            ..
        }) = self.token(0)
        {
            let label = *label;
            self.pos += 1;
            self.expect(":", const { RuleId::of("LANG-1") })?;
            if !self.at("for") && !self.at("while") {
                return self.fail(
                    const { RuleId::of("LANG-1") },
                    "only for and while may have a loop label",
                );
            }
            Some(label)
        } else {
            None
        };
        if self.eat("let") {
            let mutable = self.eat("mut");
            let pattern = self.pattern()?;
            let ty = if self.eat(":") {
                Some(self.required(Self::ty)?)
            } else {
                None
            };
            self.expect("=", const { RuleId::of("LANG-5") })?;
            let value = self.expr(0)?;
            let otherwise = if self.at("else") {
                if self.ends_in_block(value) {
                    return self.fail(
                        const { RuleId::of("LANG-13") },
                        "a let-else initializer cannot end in a block; bind it first",
                    );
                }
                self.pos += 1;
                Some(self.block()?)
            } else {
                None
            };
            return Ok(StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
                otherwise,
            });
        }
        if self.eat("for") {
            let pattern = self.pattern()?;
            self.expect("in", const { RuleId::of("LANG-13") })?;
            let source = self.required(|p| p.expr(0))?;
            let body = self.block()?;
            return Ok(StmtKind::For {
                label,
                pattern,
                source,
                body,
            });
        }
        if self.eat("while") {
            let condition = self.required(|p| p.expr(0))?;
            let body = self.block()?;
            return Ok(StmtKind::While {
                label,
                condition,
                body,
            });
        }
        if self.at("fn") && self.ident(1) {
            self.pos += 1;
            let name = self.name()?;
            let signature = self.required(|p| p.header(false, true))?;
            let body = self.block()?;
            return Ok(StmtKind::Function {
                name,
                signature,
                body,
            });
        }
        let place = self.expr(0)?;
        if self.continuing()
            && let Some(op) = assignment(self.text_at(0))
        {
            self.pos += 1;
            let value = self.expr(0)?;
            if assignment(self.text_at(0)).is_some() {
                return self.fail(const { RuleId::of("LANG-5") }, "assignment cannot chain");
            }
            Ok(StmtKind::Assign {
                place,
                op: AssignOp(op),
                value,
            })
        } else {
            Ok(StmtKind::Expr(place))
        }
    }
}
fn unary(text: &str) -> Option<UnaryOp> {
    Some(match text {
        "-" => UnaryOp::Neg,
        "!" => UnaryOp::Not,
        "~" => UnaryOp::BitNot,
        "take" => UnaryOp::Take,
        "mut" => UnaryOp::Mut,
        "copy" => UnaryOp::Copy,
        "unique" => UnaryOp::Unique,
        "box" => UnaryOp::Box,
        _ => return None,
    })
}
fn binary(text: &str) -> Option<(u8, BinaryOp)> {
    Some(match text {
        ".." => (1, BinaryOp::Range),
        "..=" => (1, BinaryOp::RangeInclusive),
        "||" => (2, BinaryOp::Or),
        "&&" => (3, BinaryOp::And),
        "==" => (4, BinaryOp::Eq),
        "!=" => (4, BinaryOp::Ne),
        "<" => (4, BinaryOp::Lt),
        "<=" => (4, BinaryOp::Le),
        ">" => (4, BinaryOp::Gt),
        ">=" => (4, BinaryOp::Ge),
        "|" => (5, BinaryOp::BitOr),
        "^" => (6, BinaryOp::BitXor),
        "&" => (7, BinaryOp::BitAnd),
        "<<" => (8, BinaryOp::Shl),
        ">>" => (8, BinaryOp::Shr),
        "+" => (9, BinaryOp::Add),
        "-" => (9, BinaryOp::Sub),
        "*" => (10, BinaryOp::Mul),
        "/" => (10, BinaryOp::Div),
        "%" => (10, BinaryOp::Rem),
        _ => return None,
    })
}
fn assignment(text: &str) -> Option<Option<BinaryOp>> {
    Some(match text {
        "=" => None,
        "+=" => Some(BinaryOp::Add),
        "-=" => Some(BinaryOp::Sub),
        "*=" => Some(BinaryOp::Mul),
        "/=" => Some(BinaryOp::Div),
        "%=" => Some(BinaryOp::Rem),
        "<<=" => Some(BinaryOp::Shl),
        ">>=" => Some(BinaryOp::Shr),
        "&=" => Some(BinaryOp::BitAnd),
        "|=" => Some(BinaryOp::BitOr),
        "^=" => Some(BinaryOp::BitXor),
        _ => return None,
    })
}
