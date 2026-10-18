//! F-strings (SEM-3): the written pieces and holes, desugared here into one
//! ordinary block, so no later phase has an f-string form.
use super::*;

impl Parser<'_> {
    pub(super) fn at_format(&self, pieces: &[FormatPiece]) -> bool {
        matches!(self.token(0).map(|t| &t.kind), Some(TokenKind::Format(p)) if pieces.contains(p))
    }
    /// `f"a{x}b"` becomes, for nesting depth N,
    /// `{ let mut $fmtN: List[u8] = []; Append("a", mut $fmtN); Append(x, mut $fmtN);
    /// Append("b", mut $fmtN); Finish(take $fmtN) }`, where each step's callee is
    /// `ExprKind::Formatter`. Empty text appends nothing.
    pub(super) fn format_string(&mut self) -> Parsed<ExprId> {
        let start = self.pos;
        self.formats += 1;
        let parts = self.format_parts();
        self.formats -= 1;
        let (pieces, holes) = parts?;
        let whole = self.origin(start);
        let names = [format!("$fmt{}", self.formats), "List".into(), "u8".into()]
            .map(|name| self.symbols.get(&name));
        let [Some(builder), Some(list), Some(byte)] = names else {
            return self.fail_at(
                const { RuleId::of("LANG-1") },
                whole.span,
                "f-string builder names are missing from the symbol snapshot",
            );
        };
        let named = |segment| TypeExprKind::Named {
            path: Path {
                package: None,
                segments: vec![segment],
            },
            arguments: vec![],
        };
        let byte = self.synthetic_type(whole, named(byte));
        let mut list = named(list);
        if let TypeExprKind::Named { arguments, .. } = &mut list {
            arguments.push(TypeArgument::Type(byte));
        }
        let list = self.synthetic_type(whole, list);
        let pattern = self.synthetic_pattern(whole, PatternKind::Binding(builder));
        let empty = self.synthetic_expr(whole, ExprKind::Sequence(vec![]));
        let mut statements = vec![self.synthetic_stmt(
            whole,
            StmtKind::Let {
                mutable: true,
                pattern,
                ty: Some(list),
                value: empty,
            },
        )];
        let mut values = vec![];
        for (i, (position, spelling)) in pieces.iter().enumerate() {
            let text = format_text(spelling);
            if !text.is_empty() {
                let origin = self.token_origin(*position);
                values.push((
                    origin,
                    self.synthetic_expr(origin, ExprKind::Literal(Literal::String(text))),
                ));
            }
            if let Some(&hole) = holes.get(i) {
                values.push((self.tree.expressions[hole.0 as usize].origin, hole));
            }
        }
        for (origin, value) in values {
            let call = self.format_step(origin, builder, FormatStep::Append, Some(value));
            statements.push(self.synthetic_stmt(origin, StmtKind::Expr(call)));
        }
        let tail = self.format_step(whole, builder, FormatStep::Finish, None);
        let block = self.synthetic_expr(
            whole,
            ExprKind::Block {
                statements,
                tail: Some(tail),
            },
        );
        self.tree.formats.push(FormatString {
            expression: block,
            pieces: pieces.into_iter().map(|(_, spelling)| spelling).collect(),
            holes,
        });
        Ok(block)
    }
    /// Pieces with their positions, and the hole between each adjacent pair.
    fn format_parts(&mut self) -> Parsed<(Vec<(usize, String)>, Vec<ExprId>)> {
        let mut pieces = vec![];
        let mut holes = vec![];
        loop {
            pieces.push((self.pos, self.text_at(0).to_owned()));
            let closed = self.at_format(&[FormatPiece::Whole, FormatPiece::Tail]);
            self.pos += 1;
            if closed {
                return Ok((pieces, holes));
            }
            if self.at_format(&[FormatPiece::Middle, FormatPiece::Tail]) {
                return self.fail(
                    const { RuleId::of("SEM-3") },
                    "an f-string hole holds an expression; write `{{` for a literal brace",
                );
            }
            // A hole is not a grouping: its expression may continue across lines
            // only where it would outside the f-string.
            let outer = self.whitespace;
            self.whitespace = 0;
            let hole = self.expr(0);
            self.whitespace = outer;
            holes.push(hole?);
            if self.at_format(&[FormatPiece::Middle, FormatPiece::Tail]) {
                if self.newline() {
                    return self.fail(
                        const { RuleId::of("SEM-3") },
                        "an f-string hole closes with `}` on the line where its expression ends",
                    );
                }
            } else if matches!(self.token(0).map(|t| &t.kind), Some(TokenKind::Invalid)) {
                // The lexer reported this quote closing the f-string inside a hole.
                return Err(());
            } else if self.at(":") {
                return self.fail(
                    const { RuleId::of("SEM-3") },
                    "f-string holes take no format specifier (`{x:08}`); format the value with a function first",
                );
            } else {
                return self.fail(
                    const { RuleId::of("SEM-3") },
                    "expected `}` to close the f-string hole",
                );
            }
        }
    }
    fn format_step(
        &mut self,
        origin: Origin,
        builder: Symbol,
        step: FormatStep,
        value: Option<ExprId>,
    ) -> ExprId {
        let place = self.synthetic_expr(
            origin,
            ExprKind::Path(Path {
                package: None,
                segments: vec![builder],
            }),
        );
        let op = match step {
            FormatStep::Append => UnaryOp::Mut,
            FormatStep::Finish => UnaryOp::Take,
        };
        let place = self.synthetic_expr(origin, ExprKind::Unary { op, value: place });
        let callee = self.synthetic_expr(origin, ExprKind::Formatter(step));
        let arguments = value
            .into_iter()
            .chain([place])
            .map(|value| Argument { label: None, value })
            .collect();
        self.synthetic_expr(origin, ExprKind::Call { callee, arguments })
    }
    fn token_origin(&self, position: usize) -> Origin {
        let index = self.significant[position];
        Origin {
            span: self.tokens[index].span,
            tokens: TokenRange {
                start: index as u32,
                end: index as u32 + 1,
            },
        }
    }
    fn synthetic_expr(&mut self, origin: Origin, kind: ExprKind) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        let height = self.heights.expression(&kind);
        self.check_height(height);
        self.heights.expressions.push(height);
        self.tree.expressions.push(Expr { origin, kind });
        id
    }
    fn synthetic_stmt(&mut self, origin: Origin, kind: StmtKind) -> StmtId {
        let id = StmtId(self.tree.statements.len() as u32);
        let height = self.heights.statement(&kind);
        self.check_height(height);
        self.heights.statements.push(height);
        self.tree.statements.push(Stmt { origin, kind });
        id
    }
    fn synthetic_type(&mut self, origin: Origin, kind: TypeExprKind) -> TypeExprId {
        let id = TypeExprId(self.tree.types.len() as u32);
        let height = self.heights.type_expression(&kind);
        self.check_height(height);
        self.heights.types.push(height);
        self.tree.types.push(TypeExpr { origin, kind });
        id
    }
    fn synthetic_pattern(&mut self, origin: Origin, kind: PatternKind) -> PatternId {
        let id = PatternId(self.tree.patterns.len() as u32);
        let height = self.heights.pattern_expression(&kind);
        self.check_height(height);
        self.heights.patterns.push(height);
        self.tree.patterns.push(Pattern { origin, kind });
        id
    }
}

/// A text piece as an ordinary string literal spelling: the piece's delimiters
/// dropped and `{{`/`}}` unescaped, its escapes kept for literal decoding.
fn format_text(piece: &str) -> String {
    let body = piece
        .strip_prefix("f\"")
        .or_else(|| piece.strip_prefix('}'))
        .unwrap_or(piece);
    let body = body
        .strip_suffix('"')
        .or_else(|| body.strip_suffix('{'))
        .unwrap_or(body);
    let mut text = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                text.push(c);
                if let Some(escaped) = chars.next() {
                    text.push(escaped);
                    if escaped == 'u' {
                        for inner in chars.by_ref() {
                            text.push(inner);
                            if inner == '}' {
                                break;
                            }
                        }
                    }
                }
            }
            '{' | '}' => {
                text.push(c);
                chars.next();
            }
            _ => text.push(c),
        }
    }
    if text.is_empty() {
        text
    } else {
        format!("\"{text}\"")
    }
}
