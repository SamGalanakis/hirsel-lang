use super::*;

impl Parser<'_> {
    pub(super) fn ty(&mut self) -> Parsed<TypeExprId> {
        self.enter()?;
        let result = self.type_inner();
        self.depth -= 1;
        self.complete(result)
    }
    fn push_type(&mut self, start: usize, kind: TypeExprKind) -> TypeExprId {
        let id = TypeExprId(self.tree.types.len() as u32);
        let height = self.heights.type_expression(&kind);
        self.check_height(height);
        self.heights.types.push(height);
        self.tree.types.push(TypeExpr {
            origin: self.origin(start),
            kind,
        });
        id
    }
    /// A field, payload, element or type argument is related as its owner
    /// and its current content are (OWN-8; spec #71), so its type writes no relation (LANG-7).
    pub(super) fn part_type(&mut self) -> Parsed<TypeExprId> {
        let ty = self.ty()?;
        self.check_part(ty)?;
        Ok(ty)
    }
    fn check_part(&mut self, ty: TypeExprId) -> Parsed<()> {
        let written = &self.tree.types[ty.0 as usize];
        if let TypeExprKind::Qualified {
            relation: Some(relation),
            ..
        } = &written.kind
        {
            return self.fail_at(
                const { RuleId::of("LANG-7") },
                relation.origin.span,
                "a field, element or type argument cannot declare a relation: it is related as its owner is",
            );
        }
        Ok(())
    }
    /// Whether `sendable` here is a modifier: a type follows it (LANG-14).
    fn modifier_follows(&self) -> bool {
        matches!(self.token(1).map(|t| &t.kind), Some(TokenKind::Ident(_)))
            || matches!(
                self.text_at(1),
                "(" | "fn" | "box" | "@" | "unique" | "once"
            )
    }
    fn type_inner(&mut self) -> Parsed<TypeExprId> {
        let start = self.pos;
        // Modifiers cover the whole type after them (LANG-7, R5).
        let mut modifiers = vec![];
        loop {
            let at = self.pos;
            let modifier = if self.eat("unique") {
                TypeModifier::Unique
            } else if self.eat("once") {
                TypeModifier::Once
            } else if self.at("sendable") && self.modifier_follows() {
                self.pos += 1;
                TypeModifier::Sendable
            } else {
                break;
            };
            // A repeated modifier is reported and recovered as written once.
            if modifiers.contains(&modifier) {
                let span = self.origin(at).span;
                self.diagnostics.push(Diagnostic::error(
                    const { RuleId::of("LANG-7") },
                    span,
                    "a modifier is written once: this one is repeated",
                ));
            } else {
                modifiers.push(modifier);
            }
        }
        let base = if self.eat("(") {
            if self.eat(")") {
                self.push_type(start, TypeExprKind::Tuple(vec![]))
            } else {
                self.whitespace += 1;
                let result = (|| {
                    let first = self.ty()?;
                    if self.eat(",") {
                        self.check_part(first)?;
                        let mut values = vec![first];
                        values.extend(self.list(")", Self::part_type)?);
                        Ok(self.push_type(start, TypeExprKind::Tuple(values)))
                    } else {
                        self.expect(")", const { RuleId::of("LANG-1") })?;
                        let height = self.heights.types[first.0 as usize] + 1;
                        self.check_height(height);
                        self.heights.types[first.0 as usize] = height;
                        Ok(first)
                    }
                })();
                self.whitespace -= 1;
                result?
            }
        } else if self.eat("box") {
            let value = self.part_type()?;
            self.push_type(start, TypeExprKind::Box(value))
        } else if self.at("fn") || self.at("@") {
            let mut attrs = vec![];
            while self.at("@") {
                let attr = self.attribute()?;
                if !matches!(
                    self.symbols.resolve(attr.name),
                    Some("noalloc" | "advance_only" | "preserves_captures" | "captures")
                ) {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "attribute cannot prefix a function type",
                    );
                }
                attrs.push(attr);
            }
            self.expect("fn", const { RuleId::of("LANG-7") })?;
            let mut signature = self.header(true, false)?;
            signature.attributes = attrs;
            self.push_type(start, TypeExprKind::Function(signature))
        } else {
            let path = self.path(true)?;
            let arguments = if self.eat("[") {
                if self.at("]") {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "type arguments cannot be empty",
                    );
                }
                self.list("]", |p| {
                    if matches!(p.token(0).map(|t| &t.kind), Some(TokenKind::Integer)) {
                        let Some(n) = size_value(p.text_at(0)) else {
                            return p.fail(
                                const { RuleId::of("LANG-7") },
                                "size argument must be an unsuffixed u64 literal",
                            );
                        };
                        p.pos += 1;
                        Ok(TypeArgument::SizeLiteral(n))
                    } else {
                        Ok(TypeArgument::Type(p.part_type()?))
                    }
                })?
            } else {
                vec![]
            };
            self.push_type(start, TypeExprKind::Named { path, arguments })
        };
        let mut brand = None;
        let mut relation: Option<RelationSyntax> = None;
        while self.continuing() {
            // A field marker terminates even a nested function/box type.
            // Only record-field parsing consumes it; other sites reject it.
            if self.at("@")
                && matches!(
                    self.text_at(1),
                    "unique" | "once" | "sendable" | "local" | "global"
                )
            {
                self.removed_qualifier(&mut modifiers, &mut relation)?;
            } else if self.at("of") {
                if brand.is_some() || relation.is_some() {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "a type has one brand, written before its relation: `T of p in x`",
                    );
                }
                self.pos += 1;
                let sibling = self.eat(".");
                brand = Some(BrandSyntax {
                    sibling,
                    path: self.path(false)?,
                });
            } else if self.at("in") {
                if relation.is_some() {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "a type has one relation; name every place in it: `in {x, y}`",
                    );
                }
                relation = Some(self.relation()?);
            } else {
                break;
            }
        }
        if modifiers.is_empty() && brand.is_none() && relation.is_none() {
            Ok(base)
        } else {
            Ok(self.push_type(
                start,
                TypeExprKind::Qualified {
                    base,
                    modifiers,
                    brand,
                    relation,
                },
            ))
        }
    }
    /// `in heap`, `in x` or `in {x, y.a, env}` (MODE-1, PROV-4; #72, #77).
    fn relation(&mut self) -> Parsed<RelationSyntax> {
        let start = self.pos;
        self.expect("in", const { RuleId::of("LANG-7") })?;
        let places = if self.eat("{") {
            self.whitespace += 1;
            let result = (|| {
                if self.at("}") {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "a relation names at least one place",
                    );
                }
                let places = self.list("}", Self::relation_place)?;
                if places.contains(&RelationPlace::Heap) {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "`heap` is the empty relation and stands alone: `in heap`",
                    );
                }
                Ok(places)
            })();
            self.whitespace -= 1;
            result?
        } else {
            vec![self.relation_place()?]
        };
        Ok(RelationSyntax {
            places,
            origin: self.origin(start),
        })
    }
    fn relation_place(&mut self) -> Parsed<RelationPlace> {
        if self.eat("heap") {
            return Ok(RelationPlace::Heap);
        }
        if self.at("env") && !matches!(self.text_at(1), "." | "/") {
            self.pos += 1;
            return Ok(RelationPlace::Environment);
        }
        Ok(RelationPlace::Path(self.path(false)?))
    }
    /// v20's `@` value qualifiers are removed syntax (LANG-7, R5). Each is
    /// reported with its v22 spelling and recovered as that spelling, so one
    /// site gives one diagnostic.
    fn removed_qualifier(
        &mut self,
        modifiers: &mut Vec<TypeModifier>,
        relation: &mut Option<RelationSyntax>,
    ) -> Parsed<()> {
        let start = self.pos;
        self.expect("@", const { RuleId::of("LANG-7") })?;
        let word = self.text_at(0).to_owned();
        self.pos += 1;
        let message = match word.as_str() {
            "unique" | "once" | "sendable" => {
                modifiers.push(match word.as_str() {
                    "unique" => TypeModifier::Unique,
                    "once" => TypeModifier::Once,
                    _ => TypeModifier::Sendable,
                });
                format!("`@ {word}` is removed: write `{word}` before the type")
            }
            "global" => {
                "`@ global` is removed: a value without a relation is global, so delete it".into()
            }
            _ => {
                let mut paths = vec![];
                if self.eat("(") {
                    if self.at(")") {
                        return self.fail(
                            const { RuleId::of("LANG-7") },
                            "local origins cannot be empty",
                        );
                    }
                    paths = self.list(")", |p| p.path(false))?;
                }
                let names: Vec<String> = paths
                    .iter()
                    .map(|p| {
                        p.segments
                            .iter()
                            .map(|s| self.symbols.resolve(*s).unwrap_or("_"))
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                    .collect();
                let mut places: Vec<RelationPlace> =
                    paths.into_iter().map(RelationPlace::Path).collect();
                let message = match names.as_slice() {
                    [] => {
                        places.push(RelationPlace::Environment);
                        "`@ local` is removed: write `in env` on a closure or function-type \
                         result, `in <name>` on a let or taken parameter, or nothing on a \
                         borrowed parameter"
                            .to_owned()
                    }
                    [one] => format!("`@ local({one})` is removed: write `in {one}`"),
                    many => format!(
                        "`@ local({})` is removed: write `in {{{}}}`",
                        many.join(", "),
                        many.join(", ")
                    ),
                };
                let origin = self.origin(start);
                match relation {
                    Some(r) => r.places.extend(places),
                    None => *relation = Some(RelationSyntax { places, origin }),
                }
                message
            }
        };
        let span = self.origin(start).span;
        self.diagnostics.push(Diagnostic::error(
            const { RuleId::of("LANG-7") },
            span,
            message,
        ));
        Ok(())
    }
    pub(super) fn pattern(&mut self) -> Parsed<PatternId> {
        self.enter()?;
        let result = self.alternatives();
        self.depth -= 1;
        self.complete(result)
    }
    /// `P | Q | ..`; a parenthesized alternation inside flattens into this one.
    fn alternatives(&mut self) -> Parsed<PatternId> {
        let start = self.pos;
        let first = self.pattern_inner()?;
        if !self.at("|") {
            return Ok(first);
        }
        let mut alternatives = vec![];
        let mut next = first;
        loop {
            match &self.tree.patterns[next.0 as usize].kind {
                PatternKind::Or(inner) => alternatives.extend(inner.iter().copied()),
                _ => alternatives.push(next),
            }
            if !self.eat("|") {
                break;
            }
            next = self.pattern_inner()?;
        }
        let kind = PatternKind::Or(alternatives);
        let id = PatternId(self.tree.patterns.len() as u32);
        let height = self.heights.pattern_expression(&kind);
        self.check_height(height);
        self.heights.patterns.push(height);
        self.tree.patterns.push(Pattern {
            origin: self.origin(start),
            kind,
        });
        Ok(id)
    }
    /// A literal pattern or range bound; a negative integer keeps its sign.
    fn pattern_literal(&mut self) -> Parsed<Option<Literal>> {
        if !self.eat("-") {
            return Ok(self.literal());
        }
        if let Some(Literal::Integer(n)) = self.literal() {
            Ok(Some(Literal::Integer(format!("-{n}"))))
        } else {
            self.fail(
                const { RuleId::of("LANG-13") },
                "negative pattern requires integer literal",
            )
        }
    }
    fn pattern_inner(&mut self) -> Parsed<PatternId> {
        if self.at("mut") {
            return self.fail(
                const { RuleId::of("LANG-13") },
                "binding-level `mut` is unsupported; put `mut` after `let`, as in `let mut (stream, _) = ...`",
            );
        }
        let start = self.pos;
        let kind = if self.eat("_") {
            PatternKind::Wildcard
        } else if let Some(literal) = self.pattern_literal()? {
            if self.eat("..=") {
                match self.pattern_literal()? {
                    Some(end) => PatternKind::Range {
                        start: literal,
                        end,
                    },
                    None => {
                        return self.fail(
                            const { RuleId::of("LANG-13") },
                            "a range pattern's bounds are literals",
                        );
                    }
                }
            } else if self.at("..") {
                return self.fail(
                    const { RuleId::of("LANG-13") },
                    "a range pattern is closed: write `a..=b`",
                );
            } else {
                PatternKind::Literal(literal)
            }
        } else if self.eat("(") {
            if self.eat(")") {
                PatternKind::Tuple(vec![])
            } else {
                self.whitespace += 1;
                let result = (|| {
                    let first = self.pattern()?;
                    if self.eat(",") {
                        let mut values = vec![first];
                        values.extend(self.list(")", Self::pattern)?);
                        Ok(PatternKind::Tuple(values))
                    } else {
                        self.expect(")", const { RuleId::of("LANG-13") })?;
                        Ok(self.tree.patterns[first.0 as usize].kind.clone())
                    }
                })();
                self.whitespace -= 1;
                result?
            }
        } else {
            let name = self.name()?;
            if self.eat("(") {
                self.whitespace += 1;
                let result = (|| {
                    let mut fields = vec![];
                    let mut rest = false;
                    while !self.eat(")") {
                        if self.eat("..") {
                            rest = true;
                            self.eat(",");
                            self.expect(")", const { RuleId::of("LANG-13") })?;
                            break;
                        }
                        let label = if self.ident(0) && self.text_at(1) == ":" {
                            let name = self.name()?;
                            self.pos += 1;
                            Some(name)
                        } else {
                            None
                        };
                        fields.push(PatternField {
                            label,
                            pattern: self.pattern()?,
                        });
                        if self.eat(")") {
                            break;
                        }
                        self.expect(",", const { RuleId::of("LANG-13") })?;
                    }
                    Ok(PatternKind::Constructor {
                        path: Path {
                            package: None,
                            segments: vec![name],
                        },
                        fields,
                        rest,
                    })
                })();
                self.whitespace -= 1;
                result?
            } else if self.at("..=") || self.at("..") {
                return self.fail(
                    const { RuleId::of("LANG-13") },
                    "a range pattern's bounds are literals",
                );
            } else {
                PatternKind::Binding(name)
            }
        };
        let id = PatternId(self.tree.patterns.len() as u32);
        let height = self.heights.pattern_expression(&kind);
        self.check_height(height);
        self.heights.patterns.push(height);
        self.tree.patterns.push(Pattern {
            origin: self.origin(start),
            kind,
        });
        Ok(id)
    }
}
