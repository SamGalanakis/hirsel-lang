use super::*;

impl Parser<'_> {
    pub(super) fn attribute(&mut self) -> Parsed<Attribute> {
        let start = self.pos;
        self.expect("@", const { RuleId::of("LANG-1") })?;
        let spelling = self.text_at(0).to_owned();
        let name = self.name()?;
        let mut arguments = vec![];
        match spelling.as_str() {
            "noalloc" | "soa" | "packed" | "blocking" | "incomplete" | "serializable" => {}
            "format" => {
                self.expect("(", const { RuleId::of("LANG-1") })?;
                let function = self.path(false)?;
                self.expect(")", const { RuleId::of("LANG-1") })?;
                arguments.push(AttributeArgument::Format(FormatAssociation {
                    function,
                    origin: self.origin(start),
                }));
            }
            "loop_source" | "advance_only" | "preserves_captures" | "preserves_unique"
            | "captures" => {
                self.expect("(", const { RuleId::of("LANG-1") })?;
                if self.at(")") {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "a declaration selector list cannot be empty",
                    );
                }
                let mut paths = self.list(")", |p| p.path(false))?;
                if spelling == "loop_source" && paths.len() != 2 {
                    return self.fail(
                        const { RuleId::of("LANG-7") },
                        "@loop_source requires open and next paths",
                    );
                }
                arguments.push(if spelling == "loop_source" {
                    let next = paths.pop().unwrap();
                    let open = paths.pop().unwrap();
                    AttributeArgument::LoopSource(LoopSourceSyntax {
                        open,
                        next,
                        origin: self.origin(start),
                    })
                } else {
                    AttributeArgument::StatePromise(StatePromiseSyntax {
                        kind: match spelling.as_str() {
                            "advance_only" => StatePromiseKind::AdvanceOnly,
                            "preserves_captures" => StatePromiseKind::PreserveCaptures,
                            "preserves_unique" => StatePromiseKind::PreserveUnique,
                            _ => StatePromiseKind::Captures,
                        },
                        selectors: paths,
                        origin: self.origin(start),
                    })
                });
            }
            "layout" | "align" | "capability" => {
                self.expect("(", const { RuleId::of("LANG-1") })?;
                match spelling.as_str() {
                    "layout" => {
                        if !self.at("c") {
                            return self.fail(const { RuleId::of("LANG-7") }, "layout requires c");
                        }
                        arguments.push(AttributeArgument::Name(self.name()?));
                    }
                    "align" => {
                        if !matches!(self.token(0).map(|t| &t.kind), Some(TokenKind::Integer)) {
                            return self
                                .fail(const { RuleId::of("LANG-7") }, "align requires an integer");
                        }
                        arguments.push(AttributeArgument::Integer(self.text_at(0).into()));
                        self.pos += 1;
                    }
                    _ => {
                        let keyword = if self.eat("global") {
                            Keyword::Global
                        } else if self.eat("local") {
                            Keyword::Local
                        } else {
                            return self.fail(
                                const { RuleId::of("MODE-6") },
                                "capability requires global or local",
                            );
                        };
                        arguments.push(AttributeArgument::Keyword(keyword));
                    }
                }
                self.expect(")", const { RuleId::of("LANG-1") })?;
            }
            _ => {
                return self.fail(
                    const { RuleId::of("LANG-7") },
                    "unknown declaration attribute",
                );
            }
        }
        Ok(Attribute {
            name,
            arguments,
            origin: self.origin(start),
        })
    }
    pub(super) fn generics(&mut self) -> Parsed<Vec<GenericParam>> {
        if !self.eat("[") {
            return Ok(vec![]);
        }
        if self.at("]") {
            return self.fail(
                const { RuleId::of("LANG-7") },
                "generic binder cannot be empty",
            );
        }
        self.list("]", |p| {
            let name = p.name()?;
            let kind = if p.eat(":") {
                if p.eat("size") {
                    GenericKind::Size
                } else {
                    GenericKind::Type(p.bounds()?)
                }
            } else {
                GenericKind::Type(vec![])
            };
            Ok(GenericParam { name, kind })
        })
    }
    fn bounds(&mut self) -> Parsed<Vec<Bound>> {
        let mut bounds = vec![];
        loop {
            let bound = match self.text_at(0) {
                "plain" => Bound::Plain,
                "flat" => Bound::Flat,
                "data" => Bound::Data,
                "key" => Bound::Key,
                "sendable" => Bound::Sendable,
                "inert" => Bound::Inert,
                "copyable" => Bound::Copyable,
                "noalloc_drop" => Bound::NoallocDrop,
                "serializable" => Bound::Serializable,
                _ => {
                    return self.fail(const { RuleId::of("GEN-1") }, "expected structural bound");
                }
            };
            self.pos += 1;
            bounds.push(bound);
            if !self.eat("+") {
                break;
            }
        }
        Ok(bounds)
    }
    pub(super) fn uses(&mut self) -> Parsed<Option<Vec<Path>>> {
        if !self.eat("uses") {
            return Ok(None);
        }
        self.expect("{", const { RuleId::of("CLO-4") })?;
        Ok(Some(self.list("}", |p| p.path(false))?))
    }
    pub(super) fn header(&mut self, anonymous: bool, contracts: bool) -> Parsed<FunctionSyntax> {
        let generics = if anonymous { vec![] } else { self.generics()? };
        self.expect("(", const { RuleId::of("LANG-1") })?;
        let parameters = self.list(")", |p| p.parameter(anonymous))?;
        let result = if (!anonymous || self.continuing()) && self.eat("->") {
            Some(self.ty()?)
        } else {
            None
        };
        let uses = if !anonymous || self.continuing() {
            self.uses()?
        } else {
            None
        };
        let mut signature = FunctionSyntax {
            generics,
            parameters,
            result,
            uses,
            ..Default::default()
        };
        if contracts {
            self.contracts(&mut signature)?;
        }
        Ok(signature)
    }
    pub(super) fn contracts(&mut self, signature: &mut FunctionSyntax) -> Parsed<()> {
        while self.at("requires") || self.at("ensures") {
            let kind = if self.eat("requires") {
                ContractKind::Requires
            } else {
                self.pos += 1;
                ContractKind::Ensures
            };
            signature.contracts.push(ContractSyntax {
                kind,
                expression: self.expr(0)?,
            });
        }
        Ok(())
    }
    fn parameter(&mut self, anonymous: bool) -> Parsed<Parameter> {
        // Colon lookahead is bounded to the parameter head; never classify names by type lookup.
        if matches!(self.text_at(0), "mut" | "take") && self.text_at(1) == ":" {
            return self.fail(
                const { RuleId::of("LANG-14") },
                "reserved keyword cannot be a parameter name",
            );
        }
        let named = (0..4).any(|i| {
            self.text_at(i) == ":"
                && (0..i).all(|j| self.ident(j) || matches!(self.text_at(j), "take" | "mut"))
        });
        if anonymous && !named {
            let passing = self.passing();
            return Ok(Parameter {
                label: None,
                name: None,
                passing,
                ty: self.ty()?,
            });
        }
        let label = if self.ident(0) && (self.ident(1) || matches!(self.text_at(1), "take" | "mut"))
        {
            Some(self.name()?)
        } else {
            None
        };
        let passing = self.passing();
        let name = self.name()?;
        self.expect(":", const { RuleId::of("LANG-6") })?;
        Ok(Parameter {
            label,
            name: Some(name),
            passing,
            ty: self.ty()?,
        })
    }
    fn passing(&mut self) -> PassingMode {
        if self.eat("take") {
            PassingMode::Take
        } else if self.eat("mut") {
            PassingMode::Mut
        } else {
            PassingMode::Borrow
        }
    }
    pub(super) fn item(&mut self, context: &FileContext) -> Parsed<Item> {
        let start = self.pos;
        let mut attributes = vec![];
        while self.at("@") {
            attributes.push(self.attribute()?);
        }
        let mut opaque_span = None;
        let visibility = if self.eat("pub") {
            let span = self.span();
            if self.eat("opaque") {
                opaque_span = Some(span);
                Visibility::PublicOpaque
            } else {
                Visibility::Public
            }
        } else {
            Visibility::Private
        };
        let (name, kind) = if self.at("fn") || self.at("extern") {
            let abi = if self.eat("extern") {
                if !matches!(self.token(0).map(|t| &t.kind), Some(TokenKind::String)) {
                    return self.fail(const { RuleId::of("PROG-4") }, "extern requires ABI string");
                }
                let literal = self.text_at(0).to_owned();
                self.pos += 1;
                Some(ForeignAbi {
                    literal,
                    contract: self.doc_comment(start),
                })
            } else {
                None
            };
            self.expect("fn", const { RuleId::of("LANG-6") })?;
            let name = self.name()?;
            let mut signature = self.header(false, true)?;
            signature.attributes = attributes.clone();
            let body = if self.at("{") {
                Some(self.block()?)
            } else if self.eat("=") {
                let expr_start = self.pos;
                self.expect("comptime", const { RuleId::of("CT-1") })?;
                let value = self.expr(0)?;
                Some(self.push_expr(expr_start, ExprKind::Comptime(value)))
            } else {
                None
            };
            if body.is_none()
                && signature.result.is_some()
                && self.newline()
                && (self.at("in") || self.at("of"))
            {
                return self.fail(
                    const { RuleId::of("LANG-7") },
                    "keep a result relation or brand with its type, or parenthesize the type",
                );
            }
            (
                name,
                ItemKind::Function {
                    signature,
                    body,
                    abi,
                },
            )
        } else if self.eat("const") {
            let name = self.name()?;
            self.expect(":", const { RuleId::of("LANG-6") })?;
            let ty = self.required(Self::ty)?;
            self.expect("=", const { RuleId::of("LANG-6") })?;
            let value = self.expr(0)?;
            (name, ItemKind::Const { ty, value })
        } else if matches!(self.text_at(0), "type" | "distinct" | "resource" | "flat") {
            self.type_item(visibility)?
        } else if self.at("|") {
            return self.fail(
                const { RuleId::of("LANG-1") },
                "a variant list continues only after a trailing `|`",
            );
        } else {
            return self.fail(
                const { RuleId::of("LANG-1") },
                "expected a top-level definition (fn, const, type)",
            );
        };
        self.validate_attributes(&attributes, opaque_span, &kind)?;
        let definition = DefPath(ItemPath {
            package: context.package.clone(),
            modules: context.modules.clone(),
            name: self.symbols.resolve(name).unwrap_or("").to_owned(),
        });
        Ok(Item {
            name_span: self.tokens[self.significant[start]
                ..self.significant[self.pos.min(self.significant.len() - 1)]]
                .iter()
                .skip_while(|t| {
                    !matches!(
                        t.kind,
                        TokenKind::Keyword(Keyword::Fn | Keyword::Type | Keyword::Const)
                    )
                })
                .find(|t| matches!(t.kind, TokenKind::Ident(s) if s == name))
                .map(|t| t.span)
                .unwrap_or(self.origin(start).span),

            origin: self.origin(start),
            owned_tokens: TokenRange::default(),
            name,
            definition,
            text_hash: ContentHash::default(),
            visibility,
            attributes,
            kind,
        })
    }
    /// The doc comments directly before the item starting at significant
    /// position `start`, without delimiters, joined by newlines (PROG-4).
    fn doc_comment(&self, start: usize) -> Option<String> {
        let first = *self.significant.get(start)?;
        let mut docs = vec![];
        for t in self.tokens[..first].iter().rev() {
            match t.kind {
                TokenKind::Trivia(TriviaKind::Space | TriviaKind::Newline) => {}
                TokenKind::Trivia(TriviaKind::DocLine) => {
                    let text = self.sources.slice(t.span)?;
                    let text = text.strip_prefix("///")?;
                    docs.push(text.strip_prefix(' ').unwrap_or(text).trim_end().to_owned());
                }
                TokenKind::Trivia(TriviaKind::DocBlock) => {
                    let text = self.sources.slice(t.span)?;
                    let text = text.strip_prefix("/**")?;
                    docs.push(text.strip_suffix("*/").unwrap_or(text).trim().to_owned());
                }
                _ => break,
            }
        }
        docs.reverse();
        let text = docs.join("\n").trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
    fn validate_attributes(
        &mut self,
        attrs: &[Attribute],
        opaque_span: Option<Span>,
        kind: &ItemKind,
    ) -> Parsed<()> {
        if let Some(span) = opaque_span
            && !matches!(kind, ItemKind::Type { .. })
        {
            return self.fail_at(
                const { RuleId::of("PROG-2") },
                span,
                "pub opaque applies only to types",
            );
        }
        for attr in attrs {
            let valid = match self.symbols.resolve(attr.name).unwrap_or("") {
                "noalloc" | "incomplete" | "advance_only" | "preserves_captures"
                | "preserves_unique" | "captures" => {
                    matches!(kind, ItemKind::Function { .. })
                }
                "blocking" => matches!(kind, ItemKind::Function { abi: Some(_), .. }),
                "capability" => opaque_span.is_some() && matches!(kind, ItemKind::Type { .. }),
                _ => matches!(kind, ItemKind::Type { .. }),
            };
            if !valid {
                self.diagnostics.push(Diagnostic::error(
                    const { RuleId::of("LANG-7") },
                    attr.origin.span,
                    "attribute is not permitted on this declaration",
                ));
                return Err(());
            }
        }
        Ok(())
    }
    fn field(&mut self, record: bool) -> Parsed<FieldSyntax> {
        let start = self.pos;
        let name = self.name()?;
        self.expect(":", const { RuleId::of("LANG-6") })?;
        let ty = self.part_type()?;
        let growable = if record && self.eat("@") {
            self.expect("growable", const { RuleId::of("LANG-7") })?;
            true
        } else {
            false
        };
        Ok(FieldSyntax {
            growable,
            name,
            ty,
            origin: self.origin(start),
        })
    }
    fn type_item(&mut self, visibility: Visibility) -> Parsed<(Symbol, ItemKind)> {
        let flavor = if self.eat("distinct") {
            TypeFlavor::Distinct
        } else if self.eat("flat") {
            TypeFlavor::Flat
        } else if self.eat("resource") {
            TypeFlavor::Resource
        } else {
            TypeFlavor::Ordinary
        };
        self.expect("type", const { RuleId::of("LANG-1") })?;
        let name_span = self.span();
        let name = self.name()?;
        let generics = self.generics()?;
        let bounds = if flavor != TypeFlavor::Distinct && self.eat(":") {
            Some(if self.eat("(") {
                self.expect(")", const { RuleId::of("GEN-1") })?;
                vec![]
            } else {
                self.bounds()?
            })
        } else {
            None
        };
        let authority = if flavor == TypeFlavor::Distinct {
            None
        } else {
            self.uses()?
        };
        let body = if flavor == TypeFlavor::Distinct {
            self.expect("(", const { RuleId::of("LANG-7") })?;
            self.whitespace += 1;
            let result = self.field(false);
            self.whitespace -= 1;
            let field = result?;
            self.expect(")", const { RuleId::of("LANG-7") })?;
            let fact = if self.eat("where") {
                Some(self.expr(0)?)
            } else {
                None
            };
            TypeBody::Distinct { field, fact }
        } else if self.eat("{") {
            let mut fields = vec![];
            while !self.eat("}") {
                fields.push(self.field(true)?);
                self.entry_end("}")?;
            }
            TypeBody::Record(fields)
        } else if self.eat("=") {
            if self.at("|") {
                return self.fail(
                    const { RuleId::of("LANG-4") },
                    "a variant list starts with a name; continuation follows a trailing `|`",
                );
            }
            let is_variant = self.ident(0)
                && matches!(self.text_at(1), "(" | "|")
                && (self.whitespace > 0 || !self.newline_at(self.pos + 1));
            if is_variant {
                let mut variants = vec![self.variant()?];
                while self.continuing() && self.eat("|") {
                    variants.push(self.variant()?);
                }
                TypeBody::Variants(variants)
            } else {
                TypeBody::Alias(self.ty()?)
            }
        } else if visibility == Visibility::PublicOpaque {
            TypeBody::Opaque
        } else {
            return self.fail_at(
                const { RuleId::of("LANG-7") },
                name_span,
                "type requires fields or a right-hand side",
            );
        };
        if self.at("where") {
            return self.fail(
                const { RuleId::of("FACT-2") },
                "where is allowed only on a distinct plain-field declaration",
            );
        }
        let annotated_drop =
            self.at("@") && self.text_at(1) == "noalloc" && self.text_at(2) == "drop";
        if (self.at("drop") || annotated_drop) && flavor != TypeFlavor::Resource {
            return self.fail(
                const { RuleId::of("DROP-1") },
                "copyable types cannot declare user-visible destruction",
            );
        }
        let noalloc = if annotated_drop {
            self.pos += 2;
            true
        } else {
            false
        };
        let drop = if self.eat("drop") {
            self.expect("(", const { RuleId::of("DROP-2") })?;
            let binding = self.name()?;
            self.expect(")", const { RuleId::of("DROP-2") })?;
            Some(DropSyntax {
                noalloc,
                binding,
                body: self.block()?,
            })
        } else {
            None
        };
        Ok((
            name,
            ItemKind::Type {
                flavor,
                generics,
                bounds,
                authority,
                body,
                drop,
            },
        ))
    }
    fn variant(&mut self) -> Parsed<VariantSyntax> {
        let name = self.name()?;
        let fields = if self.continuing() && self.eat("(") {
            self.list(")", |p| {
                let label = if p.ident(0) && p.text_at(1) == ":" {
                    let name = p.name()?;
                    p.pos += 1;
                    Some(name)
                } else {
                    None
                };
                Ok((label, p.part_type()?))
            })?
        } else {
            vec![]
        };
        if fields.iter().any(|f| f.0.is_some()) && fields.iter().any(|f| f.0.is_none()) {
            return self.fail(
                const { RuleId::of("LANG-12") },
                "variant payloads must be uniformly labelled or positional",
            );
        }
        Ok(VariantSyntax { name, fields })
    }
}
