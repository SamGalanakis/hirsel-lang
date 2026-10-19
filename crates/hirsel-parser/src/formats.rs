//! F-strings (SEM-3): the written pieces and holes, desugared here into one
//! ordinary block, so no later phase has an f-string form.
use super::*;

struct FormatParts {
    pieces: Vec<(usize, String)>,
    holes: Vec<ExprId>,
    /// Each hole's spec token position and spelling; empty for a plain hole.
    specs: Vec<(usize, String)>,
}

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
        let FormatParts {
            pieces,
            holes,
            specs,
        } = parts?;
        let whole = self.origin(start);
        let builder = Symbol::intern(&format!("$fmt{}", self.formats));
        let [list, byte] = ["List", "u8"].map(Symbol::intern);
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
                otherwise: None,
            },
        )];
        for (i, (position, spelling)) in pieces.iter().enumerate() {
            let text = format_text(spelling);
            if !text.is_empty() {
                let origin = self.token_origin(*position);
                let value = self.synthetic_expr(origin, ExprKind::Literal(Literal::String(text)));
                let call = self.format_step(origin, builder, FormatStep::Append, vec![value]);
                statements.push(self.synthetic_stmt(origin, StmtKind::Expr(call)));
            }
            if let Some(&hole) = holes.get(i) {
                let (position, spelling) = &specs[i];
                let statement = self.format_hole(builder, hole, *position, spelling)?;
                statements.push(statement);
            }
        }
        let tail = self.format_step(whole, builder, FormatStep::Finish, vec![]);
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
            specs: specs.into_iter().map(|(_, spelling)| spelling).collect(),
        });
        Ok(block)
    }
    /// Pieces with their positions, and the hole between each adjacent pair.
    fn format_parts(&mut self) -> Parsed<FormatParts> {
        let mut pieces = vec![];
        let mut holes = vec![];
        let mut specs = vec![];
        loop {
            pieces.push((self.pos, self.text_at(0).to_owned()));
            let closed = self.at_format(&[FormatPiece::Whole, FormatPiece::Tail]);
            self.pos += 1;
            if closed {
                return Ok(FormatParts {
                    pieces,
                    holes,
                    specs,
                });
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
            let spec = if self.at_format(&[FormatPiece::Spec]) {
                if self.newline() {
                    return self.fail(
                        const { RuleId::of("SEM-3") },
                        "an f-string format specifier follows its expression on the same line",
                    );
                }
                let spec = (self.pos, self.text_at(0).to_owned());
                self.pos += 1;
                spec
            } else {
                (self.pos, String::new())
            };
            specs.push(spec);
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
            } else {
                return self.fail(
                    const { RuleId::of("SEM-3") },
                    "expected `}` to close the f-string hole",
                );
            }
        }
    }
    /// One hole's statement. A plain hole is `Append(hole, mut $fmtN)`. A hole
    /// whose options are only fill, alignment and width pads its plain text:
    /// `{ let $markN = Mark($fmtN); Append(hole, mut $fmtN);
    /// Pad[hole]($markN, fill, align, width, mut $fmtN) }`. Any other options are
    /// one `AppendSpec(hole, fill, align, width, precision, flags, kind, mut $fmtN)`.
    /// A `name$` width or precision is that name's value instead of a constant.
    fn format_hole(
        &mut self,
        builder: Symbol,
        hole: ExprId,
        position: usize,
        spelling: &str,
    ) -> Parsed<StmtId> {
        let origin = self.tree.expressions[hole.0 as usize].origin;
        if spelling.is_empty() {
            let call = self.format_step(origin, builder, FormatStep::Append, vec![hole]);
            return Ok(self.synthetic_stmt(origin, StmtKind::Expr(call)));
        }
        let written = match parse_spec(spelling) {
            Ok(written) => written,
            Err(message) => {
                return self.fail_at(const { RuleId::of("SEM-3") }, origin.span, message);
            }
        };
        let FormatSpec {
            fill,
            align,
            width,
            precision,
            flags,
            kind,
        } = written.spec;
        // A named width or precision is a path at the spec token.
        let spec = self.token_origin(position);
        let count = |parser: &mut Self, name: Option<String>, value: u64| -> Parsed<ExprId> {
            Ok(match name {
                Some(name) => parser.synthetic_expr(
                    spec,
                    ExprKind::Path(Path {
                        package: None,
                        segments: vec![Symbol::intern(&name)],
                    }),
                ),
                None => parser.constant(origin, value),
            })
        };
        if flags == 0 && kind == 0 {
            let mark = Symbol::intern(&format!("$mark{}", self.formats));
            let place = self.synthetic_expr(
                origin,
                ExprKind::Path(Path {
                    package: None,
                    segments: vec![builder],
                }),
            );
            let callee = self.synthetic_expr(origin, ExprKind::Formatter(FormatStep::Mark));
            let length = self.synthetic_expr(
                origin,
                ExprKind::Call {
                    callee,
                    arguments: vec![Argument {
                        label: None,
                        value: place,
                    }],
                },
            );
            let pattern = self.synthetic_pattern(origin, PatternKind::Binding(mark));
            let bind = self.synthetic_stmt(
                origin,
                StmtKind::Let {
                    mutable: false,
                    pattern,
                    ty: None,
                    value: length,
                    otherwise: None,
                },
            );
            let append = self.format_step(origin, builder, FormatStep::Append, vec![hole]);
            let append = self.synthetic_stmt(origin, StmtKind::Expr(append));
            let start = self.synthetic_expr(
                origin,
                ExprKind::Path(Path {
                    package: None,
                    segments: vec![mark],
                }),
            );
            let fill = self.fill(origin, fill);
            let align = self.constant(origin, align);
            let width = count(self, written.width, width)?;
            let pad = self.format_step(
                origin,
                builder,
                FormatStep::Pad(hole),
                vec![start, fill, align, width],
            );
            let pad = self.synthetic_stmt(origin, StmtKind::Expr(pad));
            let block = self.synthetic_expr(
                origin,
                ExprKind::Block {
                    statements: vec![bind, append, pad],
                    tail: None,
                },
            );
            return Ok(self.synthetic_stmt(origin, StmtKind::Expr(block)));
        }
        let mut values = vec![hole, self.fill(origin, fill), self.constant(origin, align)];
        values.push(count(self, written.width, width)?);
        values.push(count(self, written.precision, precision)?);
        values.push(self.constant(origin, flags));
        values.push(self.constant(origin, kind));
        let call = self.format_step(
            origin,
            builder,
            FormatStep::AppendSpec(written.spec),
            values,
        );
        Ok(self.synthetic_stmt(origin, StmtKind::Expr(call)))
    }
    /// The fill scalar as a `char` literal.
    fn fill(&mut self, origin: Origin, scalar: u64) -> ExprId {
        self.synthetic_expr(
            origin,
            ExprKind::Literal(Literal::Character(format!("'\\u{{{scalar:x}}}'"))),
        )
    }
    fn constant(&mut self, origin: Origin, value: u64) -> ExprId {
        self.synthetic_expr(
            origin,
            ExprKind::Literal(Literal::Integer(format!("{value}u64"))),
        )
    }
    /// `step(values..., mut builder)`, or `Finish(take builder)`.
    fn format_step(
        &mut self,
        origin: Origin,
        builder: Symbol,
        step: FormatStep,
        values: Vec<ExprId>,
    ) -> ExprId {
        let place = self.synthetic_expr(
            origin,
            ExprKind::Path(Path {
                package: None,
                segments: vec![builder],
            }),
        );
        let op = match step {
            FormatStep::Finish => UnaryOp::Take,
            _ => UnaryOp::Mut,
        };
        let place = self.synthetic_expr(origin, ExprKind::Unary { op, value: place });
        let callee = self.synthetic_expr(origin, ExprKind::Formatter(step));
        let arguments = values
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
    pub(super) fn synthetic_expr(&mut self, origin: Origin, kind: ExprKind) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        let height = self.heights.expression(&kind);
        self.check_height(height);
        self.heights.expressions.push(height);
        self.tree.expressions.push(Expr { origin, kind });
        id
    }
    pub(super) fn synthetic_stmt(&mut self, origin: Origin, kind: StmtKind) -> StmtId {
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
    pub(super) fn synthetic_pattern(&mut self, origin: Origin, kind: PatternKind) -> PatternId {
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

/// Written options; a `name$` width or precision is named here, not constant.
struct WrittenSpec {
    spec: FormatSpec,
    width: Option<String>,
    precision: Option<String>,
}

/// A width or precision: decimal digits, `name$`, or absent.
enum Count {
    Absent,
    Value(u64),
    Name(String),
}

/// Rust's `[fill]align sign # 0 width .precision type` grammar, where a width
/// or precision is decimal digits or a named `name$` argument.
fn parse_spec(spelling: &str) -> Result<WrittenSpec, String> {
    let mut chars = spelling[1..].chars().peekable();
    let mut spec = FormatSpec {
        fill: 32,
        align: 0,
        width: 0,
        precision: 0,
        flags: 0,
        kind: 0,
    };
    let alignment = |c| match c {
        '<' => Some(1),
        '>' => Some(2),
        '^' => Some(3),
        _ => None,
    };
    let mut look = chars.clone();
    if let (Some(fill), Some(align)) = (look.next(), look.next())
        && let Some(align) = alignment(align)
    {
        if matches!(fill, '{' | '}') {
            return Err("a format fill cannot be a brace".into());
        }
        spec.fill = fill as u64;
        spec.align = align;
        chars.next();
        chars.next();
    }
    if spec.align == 0
        && let Some(align) = chars.peek().and_then(|&c| alignment(c))
    {
        spec.align = align;
        chars.next();
    }
    if chars.peek() == Some(&'+') {
        spec.flags |= 1;
        chars.next();
    } else if chars.peek() == Some(&'-') {
        chars.next();
    }
    if chars.peek() == Some(&'#') {
        spec.flags |= 2;
        chars.next();
    }
    if chars.peek() == Some(&'0') {
        spec.flags |= 4;
        chars.next();
    }
    let count = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| -> Result<Count, String> {
        let mut look = chars.clone();
        let mut name = String::new();
        while let Some(&c) = look.peek() {
            if c == '_' || c.is_ascii_alphabetic() || !name.is_empty() && c.is_ascii_digit() {
                name.push(c);
                look.next();
            } else {
                break;
            }
        }
        if !name.is_empty() {
            if look.peek() != Some(&'$') {
                return Ok(Count::Absent);
            }
            look.next();
            *chars = look;
            return Ok(Count::Name(name));
        }
        let mut value = None;
        while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
            chars.next();
            value = Some(
                value
                    .unwrap_or(0u64)
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(digit.into()))
                    .ok_or("format width or precision exceeds u64")?,
            );
        }
        if chars.peek() == Some(&'$') {
            return Err(
                "a positional `N$` width or precision is unsupported; write `name$` with a u64 name in scope"
                    .into(),
            );
        }
        Ok(value.map_or(Count::Absent, Count::Value))
    };
    let mut written = WrittenSpec {
        spec,
        width: None,
        precision: None,
    };
    match count(&mut chars)? {
        Count::Absent => (),
        Count::Value(width) => written.spec.width = width,
        Count::Name(name) => written.width = Some(name),
    }
    if chars.peek() == Some(&'.') {
        chars.next();
        written.spec.flags |= 8;
        match count(&mut chars)? {
            Count::Absent => {
                return Err("format precision needs decimal digits or a `name$` argument".into());
            }
            Count::Value(precision) => written.spec.precision = precision,
            Count::Name(name) => written.precision = Some(name),
        }
    }
    if let Some(kind) = chars.next() {
        if !matches!(kind, 'x' | 'X' | 'b' | 'o') || chars.next().is_some() {
            return Err(format!(
                "invalid format specifier `{spelling}`; expected fill, alignment, sign, `#`, `0`, \
                 a width and `.precision` of digits or `name$`, and type x, X, b or o"
            ));
        }
        written.spec.kind = kind as u64;
    }
    Ok(written)
}
