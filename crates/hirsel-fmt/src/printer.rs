use crate::doc::{Doc, cat, join, list, nest, token};
use Doc::{Line, Space};
use hirsel_intern::Symbol;
use hirsel_span::SourceMap;
use hirsel_syntax::*;

type Printed = Option<Doc>;
pub(crate) struct Printer<'a> {
    file: &'a SyntaxFile,
    /// Written f-strings by the block each desugars to.
    formats: std::collections::BTreeMap<ExprId, &'a FormatString>,
}
impl<'a> Printer<'a> {
    pub fn new(file: &'a SyntaxFile, _sources: &SourceMap) -> Self {
        let formats = file
            .tree
            .formats
            .iter()
            .map(|f| (f.expression, f))
            .collect();
        Self { file, formats }
    }
    fn comment_group(&self, origin: Origin) -> bool {
        fn comment(t: &Token) -> bool {
            matches!(
                t.kind,
                TokenKind::Trivia(
                    TriviaKind::LineComment
                        | TriviaKind::DocLine
                        | TriviaKind::BlockComment
                        | TriviaKind::DocBlock
                )
            )
        }
        let range = origin.tokens;
        let inside = self
            .file
            .tokens
            .get(range.start as usize..range.end as usize);
        if inside.is_some_and(|ts| {
            let first = ts.iter().find(|t| !matches!(t.kind, TokenKind::Trivia(_)));
            let last = ts
                .iter()
                .rev()
                .find(|t| !matches!(t.kind, TokenKind::Trivia(_)));
            matches!(
                first.map(|t| &t.kind),
                Some(TokenKind::Punct(Punct::LParen))
            ) && matches!(last.map(|t| &t.kind), Some(TokenKind::Punct(Punct::RParen)))
                && ts.iter().any(comment)
        }) {
            return true;
        }
        let before = self.file.tokens.get(..range.start as usize).and_then(|ts| {
            ts.iter()
                .enumerate()
                .rev()
                .find(|(_, t)| !matches!(t.kind, TokenKind::Trivia(_)))
        });
        let after = self.file.tokens.get(range.end as usize..).and_then(|ts| {
            ts.iter()
                .enumerate()
                .find(|(_, t)| !matches!(t.kind, TokenKind::Trivia(_)))
        });
        match (before, after) {
            (
                Some((
                    start,
                    Token {
                        kind: TokenKind::Punct(Punct::LParen),
                        ..
                    },
                )),
                Some((
                    offset,
                    Token {
                        kind: TokenKind::Punct(Punct::RParen),
                        ..
                    },
                )),
            ) => self
                .file
                .tokens
                .get(start + 1..range.end as usize + offset)
                .is_some_and(|ts| ts.iter().any(comment)),
            _ => false,
        }
    }
    fn name(&self, s: Symbol) -> Printed {
        Some(token(self.file.symbols.resolve(s)?))
    }
    fn path(&self, p: &Path) -> Printed {
        let mut ds = Vec::new();
        if let Some(pkg) = &p.package {
            ds.extend([
                token(&pkg.name),
                token("@"),
                token(&pkg.version),
                token("/"),
            ]);
        }
        ds.push(join(
            p.segments
                .iter()
                .map(|s| self.name(*s))
                .collect::<Option<Vec<_>>>()?,
            token("."),
        ));
        Some(cat(ds))
    }
    pub fn file(&self) -> Printed {
        Some(join(
            self.file
                .roots
                .iter()
                .map(|id| self.item(*id))
                .collect::<Option<Vec<_>>>()?,
            cat([Line, Line]),
        ))
    }
    fn attribute(&self, a: &Attribute) -> Printed {
        let mut ds = vec![token("@"), self.name(a.name)?];
        if !a.arguments.is_empty() {
            ds.push(list(
                "(",
                a.arguments
                    .iter()
                    .map(|arg| {
                        Some(match arg {
                            AttributeArgument::Name(s) => self.name(*s)?,
                            AttributeArgument::LoopSource(s) => cat([
                                self.path(&s.open)?,
                                cat([token(","), Space]),
                                self.path(&s.next)?,
                            ]),
                            AttributeArgument::Format(s) => self.path(&s.function)?,
                            AttributeArgument::StatePromise(s) => join(
                                s.selectors
                                    .iter()
                                    .map(|p| self.path(p))
                                    .collect::<Option<Vec<_>>>()?,
                                cat([token(","), Space]),
                            ),
                            AttributeArgument::Keyword(k) => token(format!("{k:?}").to_lowercase()),
                            AttributeArgument::Integer(s) | AttributeArgument::String(s) => {
                                token(s)
                            }
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
                ")",
                false,
            ));
        }
        Some(cat(ds))
    }
    fn attributes(&self, attrs: &[Attribute]) -> Printed {
        Some(join(
            attrs
                .iter()
                .map(|a| self.attribute(a))
                .collect::<Option<Vec<_>>>()?,
            Space,
        ))
    }
    fn generics(&self, params: &[GenericParam]) -> Printed {
        if params.is_empty() {
            return Some(cat([]));
        }
        Some(list(
            "[",
            params
                .iter()
                .map(|p| {
                    let mut ds = vec![self.name(p.name)?];
                    match &p.kind {
                        GenericKind::Size => ds.extend([token(":"), Space, token("size")]),
                        GenericKind::Type(bs) if !bs.is_empty() => {
                            ds.extend([token(":"), Space, self.bounds(bs)])
                        }
                        _ => {}
                    }
                    Some(cat(ds))
                })
                .collect::<Option<Vec<_>>>()?,
            "]",
            false,
        ))
    }
    fn bounds(&self, bounds: &[Bound]) -> Doc {
        join(
            bounds.iter().map(|b| {
                token(match b {
                    Bound::NoallocDrop => "noalloc_drop".into(),
                    Bound::Serializable => "serializable".into(),
                    _ => format!("{b:?}").to_lowercase(),
                })
            }),
            cat([Space, token("+"), Space]),
        )
    }
    fn uses(&self, uses: &[Path]) -> Printed {
        Some(cat([
            token("uses"),
            Space,
            token("{"),
            join(
                uses.iter()
                    .map(|p| self.path(p))
                    .collect::<Option<Vec<_>>>()?,
                cat([token(","), Space]),
            ),
            token("}"),
        ]))
    }
    fn header(&self, sig: &FunctionSyntax) -> Printed {
        let mut ds = vec![
            self.generics(&sig.generics)?,
            list(
                "(",
                sig.parameters
                    .iter()
                    .map(|p| {
                        let mut ds = Vec::new();
                        if let Some(s) = p.label {
                            ds.extend([self.name(s)?, Space]);
                        }
                        match p.passing {
                            PassingMode::Take => ds.extend([token("take"), Space]),
                            PassingMode::Mut => ds.extend([token("mut"), Space]),
                            _ => {}
                        }
                        if let Some(s) = p.name {
                            ds.extend([self.name(s)?, token(":"), Space]);
                        }
                        ds.push(self.ty(p.ty)?);
                        Some(cat(ds))
                    })
                    .collect::<Option<Vec<_>>>()?,
                ")",
                false,
            ),
        ];
        if let Some(t) = sig.result {
            let mut result = self.ty(t)?;
            // A following uses clause must belong to this header. Function and
            // box types greedily consume it unless the result is delimited.
            if sig.uses.is_some() && self.result_accepts_uses(t)? {
                result = cat([token("("), result, token(")")]);
            }
            ds.extend([Space, token("->"), Space, result]);
        }
        if let Some(u) = &sig.uses {
            ds.extend([Space, self.uses(u)?]);
        }
        for c in &sig.contracts {
            ds.push(nest(cat([
                Line,
                token(match c.kind {
                    ContractKind::Requires => "requires",
                    ContractKind::Ensures => "ensures",
                }),
                Space,
                self.expr(c.expression, 0)?,
            ])));
        }
        Some(cat(ds))
    }
    fn result_accepts_uses(&self, id: TypeExprId) -> Option<bool> {
        match &self.file.tree.types.get(id.0 as usize)?.kind {
            TypeExprKind::Function(_) => Some(true),
            TypeExprKind::Box(inner) => self.result_accepts_uses(*inner),
            _ => Some(false),
        }
    }
    fn item(&self, id: ItemId) -> Printed {
        let item = self.file.tree.items.get(id.0 as usize)?;
        let mut ds = Vec::new();
        for a in &item.attributes {
            ds.extend([self.attribute(a)?, Line]);
        }
        if let ItemKind::Function { signature, .. } = &item.kind {
            for a in &signature.attributes {
                if !item
                    .attributes
                    .iter()
                    .any(|existing| existing.name == a.name && existing.arguments == a.arguments)
                {
                    ds.extend([self.attribute(a)?, Line]);
                }
            }
        }
        match item.visibility {
            Visibility::Public => ds.extend([token("pub"), Space]),
            Visibility::PublicOpaque => ds.extend([token("pub"), Space, token("opaque"), Space]),
            _ => {}
        }
        match &item.kind {
            ItemKind::Function {
                signature,
                body,
                abi,
            } => {
                if let Some(a) = abi {
                    ds.extend([token("extern"), Space, token(&a.literal), Space]);
                }
                ds.extend([
                    token("fn"),
                    Space,
                    self.name(item.name)?,
                    self.header(signature)?,
                ]);
                if let Some(e) = body {
                    match self.file.tree.expressions.get(e.0 as usize)?.kind {
                        ExprKind::Comptime(value) => ds.extend([
                            Space,
                            token("="),
                            Space,
                            token("comptime"),
                            Space,
                            self.expr(value, 0)?,
                        ]),
                        _ => ds.extend([Space, self.expr(*e, 0)?]),
                    }
                }
            }
            ItemKind::Const { ty, value } => ds.extend([
                token("const"),
                Space,
                self.name(item.name)?,
                token(":"),
                Space,
                self.ty(*ty)?,
                Space,
                token("="),
                Space,
                self.expr(*value, 0)?,
            ]),
            ItemKind::Type {
                flavor,
                generics,
                bounds,
                authority,
                body,
                drop,
            } => {
                match flavor {
                    TypeFlavor::Distinct => ds.extend([token("distinct"), Space]),
                    TypeFlavor::Flat => ds.extend([token("flat"), Space]),
                    TypeFlavor::Resource => ds.extend([token("resource"), Space]),
                    _ => {}
                }
                ds.extend([
                    token("type"),
                    Space,
                    self.name(item.name)?,
                    self.generics(generics)?,
                ]);
                if let Some(bounds) = bounds {
                    ds.extend([
                        token(":"),
                        Space,
                        if bounds.is_empty() {
                            cat([token("("), token(")")])
                        } else {
                            self.bounds(bounds)
                        },
                    ]);
                }
                if let Some(u) = authority {
                    ds.extend([Space, self.uses(u)?]);
                }
                match body {
                    TypeBody::Record(fields) => {
                        ds.extend([Space, token("{")]);
                        if !fields.is_empty() {
                            ds.push(nest(cat([
                                Line,
                                join(
                                    fields
                                        .iter()
                                        .map(|f| self.field(f))
                                        .collect::<Option<Vec<_>>>()?,
                                    cat([token(","), Line]),
                                ),
                                token(","),
                            ])));
                            ds.push(Line);
                        }
                        ds.push(token("}"));
                    }
                    TypeBody::Variants(vs) => {
                        ds.extend([
                            Space,
                            token("="),
                            Space,
                            join(
                                vs.iter()
                                    .map(|v| {
                                        let mut ds = vec![self.name(v.name)?];
                                        if !v.fields.is_empty() || vs.len() == 1 {
                                            ds.push(list(
                                                "(",
                                                v.fields
                                                    .iter()
                                                    .map(|(s, t)| {
                                                        let mut ds = Vec::new();
                                                        if let Some(s) = s {
                                                            ds.extend([
                                                                self.name(*s)?,
                                                                token(":"),
                                                                Space,
                                                            ]);
                                                        }
                                                        ds.push(self.ty(*t)?);
                                                        Some(cat(ds))
                                                    })
                                                    .collect::<Option<Vec<_>>>()?,
                                                ")",
                                                false,
                                            ));
                                        }
                                        Some(cat(ds))
                                    })
                                    .collect::<Option<Vec<_>>>()?,
                                cat([Space, token("|"), Space]),
                            ),
                        ]);
                    }
                    TypeBody::Alias(t) => ds.extend([Space, token("="), Space, self.ty(*t)?]),
                    TypeBody::Distinct { field, fact } => {
                        ds.extend([token("("), self.field(field)?, token(")")]);
                        if let Some(e) = fact {
                            ds.extend([Space, token("where"), Space, self.expr(*e, 0)?]);
                        }
                    }
                    TypeBody::Opaque => {}
                }
                if let Some(d) = drop {
                    ds.push(Line);
                    if d.noalloc {
                        ds.extend([token("@"), token("noalloc"), Space]);
                    }
                    ds.extend([
                        token("drop"),
                        token("("),
                        self.name(d.binding)?,
                        token(")"),
                        Space,
                        self.expr(d.body, 0)?,
                    ]);
                }
            }
        }
        Some(cat(ds))
    }
    fn field(&self, f: &FieldSyntax) -> Printed {
        let mut ds = vec![self.name(f.name)?, token(":"), Space, self.ty(f.ty)?];
        if f.growable {
            ds.extend([Space, token("@"), Space, token("growable")]);
        }
        Some(cat(ds))
    }
    fn ty(&self, id: TypeExprId) -> Printed {
        (|| {
            let node = self.file.tree.types.get(id.0 as usize)?;
            let doc = self.ty_inner(id)?;
            Some(
                if !matches!(node.kind, TypeExprKind::Tuple(_)) && self.comment_group(node.origin) {
                    cat([token("("), doc, token(")")])
                } else {
                    doc
                },
            )
        })()
    }
    fn ty_inner(&self, id: TypeExprId) -> Printed {
        Some(match &self.file.tree.types.get(id.0 as usize)?.kind {
            TypeExprKind::Named { path, arguments } => cat([
                self.path(path)?,
                if arguments.is_empty() {
                    cat([])
                } else {
                    list(
                        "[",
                        arguments
                            .iter()
                            .map(|a| match a {
                                TypeArgument::Type(t) => self.ty(*t),
                                TypeArgument::SizeLiteral(n) => Some(token(n.to_string())),
                            })
                            .collect::<Option<Vec<_>>>()?,
                        "]",
                        false,
                    )
                },
            ]),
            TypeExprKind::Tuple(ts) => list(
                "(",
                ts.iter().map(|t| self.ty(*t)).collect::<Option<Vec<_>>>()?,
                ")",
                true,
            ),
            TypeExprKind::Box(t) => cat([token("box"), Space, self.ty(*t)?]),
            TypeExprKind::Function(sig) => {
                let mut ds = Vec::new();
                if !sig.attributes.is_empty() {
                    ds.extend([self.attributes(&sig.attributes)?, Space]);
                }
                ds.extend([token("fn"), self.header(sig)?]);
                cat(ds)
            }
            TypeExprKind::Qualified {
                base,
                modifiers,
                brand,
                relation,
            } => {
                // Modifiers cover the whole type; a brand or relation binds to
                // the nearest type, so a function value without `uses`, a box
                // (whose part takes no relation) or a nested qualified type
                // is grouped to carry its own (LANG-7, R5).
                let suffixed = brand.is_some() || relation.is_some();
                let grouped = match &self.file.tree.types.get(base.0 as usize)?.kind {
                    TypeExprKind::Function(f) => suffixed && f.uses.is_none(),
                    TypeExprKind::Box(_) => suffixed,
                    TypeExprKind::Qualified { .. } => true,
                    _ => false,
                };
                let mut ds = vec![];
                for m in modifiers {
                    ds.extend([
                        token(match m {
                            TypeModifier::Unique => "unique",
                            TypeModifier::Once => "once",
                            TypeModifier::Sendable => "sendable",
                        }),
                        Space,
                    ]);
                }
                ds.push(if grouped {
                    cat([token("("), self.ty(*base)?, token(")")])
                } else {
                    self.ty(*base)?
                });
                if let Some(b) = brand {
                    ds.extend([Space, token("of"), Space]);
                    if b.sibling {
                        ds.push(token("."));
                    }
                    ds.push(self.path(&b.path)?);
                }
                if let Some(r) = relation {
                    ds.extend([Space, token("in"), Space]);
                    let places = r
                        .places
                        .iter()
                        .map(|p| match p {
                            RelationPlace::Path(p) => self.path(p),
                            RelationPlace::Heap => Some(token("heap")),
                            RelationPlace::Environment => Some(token("env")),
                        })
                        .collect::<Option<Vec<_>>>()?;
                    if places.len() == 1 {
                        ds.extend(places);
                    } else {
                        ds.push(list("{", places, "}", false));
                    }
                }
                cat(ds)
            }
            TypeExprKind::Error => return None,
        })
    }
    fn pattern(&self, id: PatternId) -> Printed {
        (|| {
            Some(match &self.file.tree.patterns.get(id.0 as usize)?.kind {
                PatternKind::Wildcard => token("_"),
                PatternKind::Binding(s) => self.name(*s)?,
                PatternKind::Literal(l) => literal(l),
                PatternKind::Tuple(ps) => list(
                    "(",
                    ps.iter()
                        .map(|p| self.pattern(*p))
                        .collect::<Option<Vec<_>>>()?,
                    ")",
                    true,
                ),
                PatternKind::Constructor { path, fields, rest } => {
                    let mut args = fields
                        .iter()
                        .map(|f| {
                            let mut ds = Vec::new();
                            if let Some(s) = f.label {
                                ds.extend([self.name(s)?, token(":"), Space]);
                            }
                            ds.push(self.pattern(f.pattern)?);
                            Some(cat(ds))
                        })
                        .collect::<Option<Vec<_>>>()?;
                    if *rest {
                        args.push(token(".."));
                    }
                    cat([self.path(path)?, list("(", args, ")", false)])
                }
                PatternKind::Error => return None,
            })
        })()
    }
    fn stmt(&self, id: StmtId, final_statement: bool) -> Printed {
        Some(match &self.file.tree.statements.get(id.0 as usize)?.kind {
            StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
            } => {
                let mut ds = vec![token("let"), Space];
                if *mutable {
                    ds.extend([token("mut"), Space]);
                }
                ds.push(self.pattern(*pattern)?);
                if let Some(t) = ty {
                    ds.extend([token(":"), Space, self.ty(*t)?]);
                }
                ds.extend([Space, token("="), Space, self.expr(*value, 0)?]);
                cat(ds)
            }
            StmtKind::Assign { place, op, value } => cat([
                self.expr(*place, 0)?,
                Space,
                token(op.0.map_or("=".to_owned(), |b| format!("{}=", binary(b).0))),
                Space,
                self.expr(*value, 0)?,
            ]),
            // Only a final expression statement requires an explicit discard.
            StmtKind::Expr(e) => cat([
                self.expr(*e, 0)?,
                if final_statement { token(";") } else { cat([]) },
            ]),
            StmtKind::While { condition, body } => cat([
                token("while"),
                Space,
                self.expr(*condition, 0)?,
                Space,
                self.expr(*body, 0)?,
            ]),
            StmtKind::For {
                pattern,
                source,
                body,
            } => cat([
                token("for"),
                Space,
                self.pattern(*pattern)?,
                Space,
                token("in"),
                Space,
                self.expr(*source, 0)?,
                Space,
                self.expr(*body, 0)?,
            ]),
            StmtKind::Function {
                name,
                signature,
                body,
            } => cat([
                token("fn"),
                Space,
                self.name(*name)?,
                self.header(signature)?,
                Space,
                self.expr(*body, 0)?,
            ]),
        })
    }
    fn expr(&self, id: ExprId, minimum: u8) -> Printed {
        self.expr_inner(id, minimum)
    }
    /// A written f-string, in place of the block the parser desugared it to.
    /// A hole starting with `{` is spaced so it cannot read as an escaped `{{`.
    fn format_string(&self, format: &FormatString) -> Printed {
        let mut parts = vec![];
        for (i, piece) in format.pieces.iter().enumerate() {
            parts.push(token(piece));
            if let Some(&hole) = format.holes.get(i) {
                let origin = self.file.tree.expressions.get(hole.0 as usize)?.origin;
                let braced = matches!(
                    self.file
                        .tokens
                        .get(origin.tokens.start as usize)
                        .map(|t| &t.kind),
                    Some(TokenKind::Punct(Punct::LBrace))
                );
                let hole = self.expr(hole, 0)?;
                parts.push(if braced {
                    cat([Space, hole, Space])
                } else {
                    hole
                });
                let spec = format.specs.get(i)?;
                if !spec.is_empty() {
                    parts.push(token(spec));
                }
            }
        }
        Some(cat(parts))
    }
    fn expr_inner(&self, id: ExprId, minimum: u8) -> Printed {
        if let Some(format) = self.formats.get(&id) {
            return self.format_string(format);
        }
        let e = self.file.tree.expressions.get(id.0 as usize)?;
        let precedence = match &e.kind {
            ExprKind::Binary { op, .. } => binary(*op).1,
            ExprKind::Unary { .. } | ExprKind::Comptime(_) => 11,
            ExprKind::Return(_) => 0,
            ExprKind::Call { .. }
            | ExprKind::Index { .. }
            | ExprKind::Field { .. }
            | ExprKind::Try { .. } => 12,
            _ => 13,
        };
        let doc = match &e.kind {
            ExprKind::Literal(l) => literal(l),
            ExprKind::Path(p) => self.path(p)?,
            ExprKind::Tuple(es) => list(
                "(",
                es.iter()
                    .map(|e| self.expr(*e, 0))
                    .collect::<Option<Vec<_>>>()?,
                ")",
                true,
            ),
            ExprKind::Sequence(es) => list(
                "[",
                es.iter()
                    .map(|e| self.expr(*e, 0))
                    .collect::<Option<Vec<_>>>()?,
                "]",
                false,
            ),
            ExprKind::Call { callee, arguments } => cat([
                self.expr(*callee, 12)?,
                list(
                    "(",
                    arguments
                        .iter()
                        .map(|a| {
                            let mut ds = Vec::new();
                            if let Some(s) = a.label {
                                ds.extend([self.name(s)?, token(":"), Space]);
                            }
                            ds.push(self.expr(a.value, 0)?);
                            Some(cat(ds))
                        })
                        .collect::<Option<Vec<_>>>()?,
                    ")",
                    false,
                ),
            ]),
            ExprKind::Field { base, field } => {
                let base_doc = self.expr(*base, 12)?;
                let base_doc = if matches!(
                    self.file.tree.expressions.get(base.0 as usize)?.kind,
                    ExprKind::Try {
                        map_error: Some(_),
                        ..
                    } | ExprKind::Path(_)
                ) {
                    cat([token("("), base_doc, token(")")])
                } else {
                    base_doc
                };
                cat([base_doc, token("."), self.name(*field)?])
            }
            ExprKind::Index { base, index } => cat([
                self.expr(*base, 12)?,
                token("["),
                self.expr(*index, 0)?,
                token("]"),
            ]),
            ExprKind::Unary { op, value } => {
                let (s, space) = match op {
                    UnaryOp::Neg => ("-", false),
                    UnaryOp::Not => ("!", false),
                    UnaryOp::BitNot => ("~", false),
                    UnaryOp::Take => ("take", true),
                    UnaryOp::Copy => ("copy", true),
                    UnaryOp::Mut => ("mut", true),
                    UnaryOp::Unique => ("unique", true),
                    UnaryOp::Box => ("box", true),
                };
                cat([
                    token(s),
                    if space { Space } else { cat([]) },
                    self.expr(*value, 11)?,
                ])
            }
            ExprKind::Binary { op, left, right } => {
                let (s, p) = binary(*op);
                let nonassoc = p == 1 || p == 4;
                let spacer = if p == 1 { cat([]) } else { Space };
                cat([
                    self.expr(*left, p + u8::from(nonassoc))?,
                    spacer.clone(),
                    token(s),
                    spacer,
                    self.expr(*right, p + 1)?,
                ])
            }
            ExprKind::Try { value, map_error } => {
                let mut ds = vec![self.expr(*value, 12)?];
                if let Some(p) = map_error {
                    ds.extend([Space, token("?"), Space, self.path(p)?]);
                } else {
                    ds.push(token("?"));
                }
                cat(ds)
            }
            ExprKind::Block { statements, tail } => {
                let mut body = Vec::new();
                for (i, s) in statements.iter().enumerate() {
                    body.push(self.stmt(*s, tail.is_none() && i + 1 == statements.len())?);
                }
                if let Some(t) = tail {
                    body.push(self.expr(*t, 0)?);
                }
                if body.is_empty() {
                    cat([token("{"), token("}")])
                } else {
                    cat([
                        token("{"),
                        nest(cat([Line, join(body, Line)])),
                        Line,
                        token("}"),
                    ])
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let mut ds = vec![
                    token("if"),
                    Space,
                    self.expr(*condition, 0)?,
                    Space,
                    self.expr(*then_branch, 0)?,
                ];
                if let Some(e) = else_branch {
                    ds.extend([Space, token("else"), Space, self.expr(*e, 0)?]);
                }
                cat(ds)
            }
            ExprKind::Match { value, arms } => {
                let mut ds = vec![
                    token("match"),
                    Space,
                    self.expr(*value, 0)?,
                    Space,
                    token("{"),
                ];
                if !arms.is_empty() {
                    ds.push(nest(cat([
                        Line,
                        join(
                            arms.iter()
                                .map(|a| {
                                    let mut ds = vec![self.pattern(a.pattern)?];
                                    if let Some(g) = a.guard {
                                        ds.extend([Space, token("if"), Space, self.expr(g, 0)?]);
                                    }
                                    ds.extend([Space, token("=>"), Space, self.expr(a.body, 0)?]);
                                    Some(cat(ds))
                                })
                                .collect::<Option<Vec<_>>>()?,
                            cat([token(","), Line]),
                        ),
                        token(","),
                    ])));
                    ds.push(Line);
                }
                ds.push(token("}"));
                cat(ds)
            }
            ExprKind::Closure { signature, body } => {
                let trailing = crate::trivia::trailing_uses(self.file, e.origin, *body)
                    .is_some_and(|(_, _, preserve)| preserve);
                if trailing {
                    let mut header = signature.clone();
                    header.uses = None;
                    let mut ds = vec![
                        token("fn"),
                        self.header(&header)?,
                        Space,
                        self.expr(*body, 0)?,
                    ];
                    if let Some(uses) = &signature.uses {
                        ds.extend([Space, self.uses(uses)?]);
                    }
                    cat(ds)
                } else {
                    cat([
                        token("fn"),
                        self.header(signature)?,
                        Space,
                        self.expr(*body, 0)?,
                    ])
                }
            }
            ExprKind::Return(value) => {
                let mut ds = vec![token("return")];
                if let Some(v) = value {
                    ds.extend([Space, self.expr(*v, 0)?]);
                }
                cat(ds)
            }
            ExprKind::Break => token("break"),
            ExprKind::Continue => token("continue"),
            ExprKind::Shape(t) => cat([token("shape"), token("("), self.ty(*t)?, token(")")]),
            ExprKind::Before(v) => {
                cat([token("before"), token("("), self.expr(*v, 0)?, token(")")])
            }
            ExprKind::Comptime(v) => cat([token("comptime"), Space, self.expr(*v, 11)?]),
            ExprKind::Heap => token("heap"),
            ExprKind::Formatter(_) => return None,
            ExprKind::Result => token("result"),
            ExprKind::Todo(s) => {
                let mut ds = vec![token("todo")];
                if let Some(s) = s {
                    ds.extend([token("("), token(s), token(")")]);
                }
                cat(ds)
            }
            ExprKind::Error => return None,
        };
        // A comment-induced newline before a postfix/binary operator must not
        // become a statement END, even when the parser omitted group delimiters.
        let grouped_comment = !matches!(e.kind, ExprKind::Tuple(_)) && self.comment_group(e.origin);
        Some(if precedence < minimum || grouped_comment {
            cat([token("("), doc, token(")")])
        } else {
            doc
        })
    }
}
fn literal(l: &Literal) -> Doc {
    match l {
        Literal::Integer(s) if s.starts_with('-') => cat([token("-"), token(&s[1..])]),
        Literal::Integer(s)
        | Literal::Float(s)
        | Literal::Character(s)
        | Literal::String(s)
        | Literal::Bytes(s) => token(s),
        Literal::Bool(b) => token(b.to_string()),
    }
}
fn binary(op: BinaryOp) -> (&'static str, u8) {
    match op {
        BinaryOp::Range => ("..", 1),
        BinaryOp::RangeInclusive => ("..=", 1),
        BinaryOp::Or => ("||", 2),
        BinaryOp::And => ("&&", 3),
        BinaryOp::Eq => ("==", 4),
        BinaryOp::Ne => ("!=", 4),
        BinaryOp::Lt => ("<", 4),
        BinaryOp::Le => ("<=", 4),
        BinaryOp::Gt => (">", 4),
        BinaryOp::Ge => (">=", 4),
        BinaryOp::BitOr => ("|", 5),
        BinaryOp::BitXor => ("^", 6),
        BinaryOp::BitAnd => ("&", 7),
        BinaryOp::Shl => ("<<", 8),
        BinaryOp::Shr => (">>", 8),
        BinaryOp::Add => ("+", 9),
        BinaryOp::Sub => ("-", 9),
        BinaryOp::Mul => ("*", 10),
        BinaryOp::Div => ("/", 10),
        BinaryOp::Rem => ("%", 10),
    }
}
#[cfg(test)]
pub(crate) fn binary_for_test(op: BinaryOp) -> (&'static str, u8) {
    binary(op)
}
