//! Isolated lowering fixtures with explicit declarations and analyzed facts.
use super::*;
use hirsel_intern::Symbol;
use hirsel_span::Anchor;
use hirsel_syntax::*;
use hirsel_types::*;
/// The session interner these tests' rows share.
pub fn session_arc() -> &'static std::sync::Arc<TypeInterner> {
    static SESSION: std::sync::OnceLock<std::sync::Arc<TypeInterner>> = std::sync::OnceLock::new();
    SESSION.get_or_init(|| TypeInterner::new(None))
}
pub fn session() -> &'static TypeInterner {
    session_arc()
}
pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
/// The anchor fixture sites name; the fixture's sources place it.
pub fn fixture_anchor() -> Anchor {
    static ANCHOR: std::sync::OnceLock<Anchor> = std::sync::OnceLock::new();
    *ANCHOR.get_or_init(Anchor::fresh)
}
pub fn site(line: u32) -> Span {
    Span::new(fixture_anchor(), (line - 1) * 2, (line - 1) * 2 + 1).unwrap()
}
pub fn origin(line: u32) -> hirsel_syntax::Origin {
    hirsel_syntax::Origin {
        span: site(line),
        tokens: TokenRange::default(),
    }
}
pub struct Fixture {
    /// The lowered item: its tree holds the fixture's syntax.
    pub syntax: ItemSyntax,
    pub checked: CheckedBody,
    pub plans: Plans,
    pub ownership: OwnershipFacts,
    pub result_types: Vec<(NameSite, TypeId)>,
    pub frames: Vec<(NameSite, TypeId)>,
    pub unit: TypeId,
    pub boolean: TypeId,
    pub scalar: TypeId,
    pub owner: NameSite,
    pub next_local: u32,
    pub declarations: hirsel_validate::SignatureMap,
    pub parameters: Vec<hirsel_types::Parameter>,
}
impl Fixture {
    /// A site of the lowered item, qualified by its owner.
    pub fn scope(&self, site: NameSite) -> SiteRef {
        self.syntax.owner().site(site)
    }
    pub fn new(primitive: Primitive) -> Self {
        let types = session();
        let unit = types.intern(ty(TypeKind::Tuple(vec![].into())));
        let boolean = types.intern(ty(TypeKind::Primitive(Primitive::Bool)));
        let scalar = types.intern(ty(TypeKind::Primitive(primitive)));
        let syntax = ItemSyntax {
            item: Item {
                name_span: Span::default(),
                origin: origin(1),
                name: Symbol::intern("fixture"),
                definition: path("fixture"),
                visibility: Visibility::Private,
                attributes: vec![],
                kind: ItemKind::Function {
                    signature: FunctionSyntax::default(),
                    body: None,
                    abi: None,
                },
                generated: None,
            },
            tree: SyntaxTree::default(),
            anchor: fixture_anchor(),
            text_hash: ContentHash::default(),
            occurrence: 0,
        };
        Self {
            checked: CheckedBody {
                lexical: syntax.owner(),
                tails: vec![],
                places: vec![],
                calls: vec![],
                bindings: vec![],
                patterns: vec![],
                resolution: Resolution::default(),
                expressions: vec![],
                instantiations: vec![],
                closures: vec![],
            },
            plans: Plans::default(),
            ownership: OwnershipFacts::default(),
            result_types: vec![],
            frames: vec![],
            unit,
            boolean,
            scalar,
            owner: NameSite::Item,
            syntax,
            next_local: 0,
            declarations: hirsel_validate::SignatureMap::new(session_arc().clone()),
            parameters: vec![],
        }
    }
    pub fn type_(&mut self, kind: TypeKind) -> TypeId {
        session().intern(ty(kind))
    }
    pub fn expr(&mut self, kind: ExprKind, ty: TypeId, usage: UseKind, line: u32) -> ExprId {
        let id = ExprId(self.syntax.tree.expressions.len() as u32);
        self.syntax.tree.expressions.push(Expr {
            origin: origin(line),
            kind,
        });
        self.checked.expressions.push(Elaboration {
            construction_destination: None,
            expression: id,
            ty,
            usage,
            instantiated_arguments: vec![],
        });
        let root = hirsel_types::SourceRoot::Value(id);
        let mut place = hirsel_types::SourcePlace {
            expression: id,
            root,
            projections: vec![].into(),
            access: hirsel_types::SourceAccess::Read,
        };
        match &self.syntax.tree.expressions[id.0 as usize].kind {
            ExprKind::Field { base, field } => {
                place = self
                    .checked
                    .places
                    .iter()
                    .find(|p| p.expression == *base)
                    .cloned()
                    .unwrap();
                place.expression = id;
                place.projections.push(hirsel_types::SourceProjection {
                    expression: id,
                    // Fixture fields are spelled by their ordinal.
                    kind: hirsel_types::SourceProjectionKind::Field(hirsel_types::FieldId(
                        field.as_str().parse().expect("a fixture field ordinal"),
                    )),
                });
            }
            ExprKind::Index { base, index } => {
                place = self
                    .checked
                    .places
                    .iter()
                    .find(|p| p.expression == *base)
                    .cloned()
                    .unwrap();
                place.expression = id;
                place.projections.push(hirsel_types::SourceProjection {
                    expression: id,
                    kind: hirsel_types::SourceProjectionKind::Index(*index),
                });
            }
            ExprKind::Unary { op, value }
                if matches!(
                    op,
                    UnaryOp::Copy | UnaryOp::Take | UnaryOp::Mut | UnaryOp::Unique
                ) =>
            {
                place = self
                    .checked
                    .places
                    .iter()
                    .find(|p| p.expression == *value)
                    .cloned()
                    .unwrap();
                place.expression = id;
                place.access = match op {
                    UnaryOp::Copy => hirsel_types::SourceAccess::Copy,
                    UnaryOp::Take => hirsel_types::SourceAccess::Take,
                    UnaryOp::Mut => hirsel_types::SourceAccess::Mut,
                    _ => hirsel_types::SourceAccess::Unique,
                };
            }
            _ => (),
        }
        self.checked.places.push(place);
        id
    }
    pub fn integer(&mut self, n: i128, line: u32) -> ExprId {
        if n < 0 {
            let value = self.integer(-n, line);
            self.expr(
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    value,
                },
                self.scalar,
                UseKind::Take,
                line,
            )
        } else {
            self.expr(
                ExprKind::Literal(Literal::Integer(n.to_string())),
                self.scalar,
                UseKind::Take,
                line,
            )
        }
    }
    pub fn bool_(&mut self, b: bool) -> ExprId {
        self.expr(
            ExprKind::Literal(Literal::Bool(b)),
            self.boolean,
            UseKind::Take,
            1,
        )
    }
    pub fn unit_(&mut self) -> ExprId {
        self.expr(ExprKind::Tuple(vec![]), self.unit, UseKind::Take, 1)
    }
    pub fn binary(&mut self, op: BinaryOp, left: ExprId, right: ExprId, line: u32) -> ExprId {
        let ty = if matches!(
            op,
            BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::And
                | BinaryOp::Or
        ) {
            self.boolean
        } else {
            self.scalar
        };
        self.expr(
            ExprKind::Binary { op, left, right },
            ty,
            UseKind::Take,
            line,
        )
    }
    pub fn stmt(&mut self, kind: StmtKind, line: u32) -> StmtId {
        let id = StmtId(self.syntax.tree.statements.len() as u32);
        self.syntax.tree.statements.push(Stmt {
            origin: origin(line),
            kind,
        });
        id
    }
    pub fn binding(&mut self, ty: TypeId, usage: UseKind) -> (PatternId, ResolvedName) {
        let pattern = PatternId(self.syntax.tree.patterns.len() as u32);
        let local = hirsel_types::LocalId(self.next_local);
        self.next_local += 1;
        self.syntax.tree.patterns.push(Pattern {
            origin: origin(1),
            kind: PatternKind::Binding(Symbol::intern(&format!("local{}", local.0))),
        });
        self.checked.patterns.push(PatternDecision {
            pattern,
            ty,
            usage,
            kind: PatternDecisionKind::Binding(local),
            fields: vec![],
            annotation: None,
        });
        let binding = ResolvedName::Local(local);
        self.checked.bindings.push(BindingDecision {
            owner: self.owner.clone(),
            binding: binding.clone(),
            site: NameSite::Pattern(pattern),
            ty,
            usage,
            mutable: true,
        });
        (pattern, binding)
    }
    pub fn let_(&mut self, value: ExprId) -> (StmtId, ResolvedName) {
        let ty = self.checked.expressions[value.0 as usize].ty;
        let (pattern, binding) = self.binding(ty, UseKind::Take);
        (
            self.stmt(
                StmtKind::Let {
                    mutable: true,
                    pattern,
                    ty: None,
                    value,
                    otherwise: None,
                },
                1,
            ),
            binding,
        )
    }
    pub fn read(&mut self, binding: ResolvedName, ty: TypeId, usage: UseKind) -> ExprId {
        let id = self.expr(
            ExprKind::Path(Path {
                package: None,
                segments: vec![],
            }),
            ty,
            usage,
            1,
        );
        self.checked
            .resolution
            .expressions
            .push((id, binding.clone()));
        self.checked
            .places
            .iter_mut()
            .find(|p| p.expression == id)
            .unwrap()
            .root = hirsel_types::SourceRoot::Binding {
            owner: self.owner.clone(),
            binding,
        };
        self.checked
            .resolution
            .reference_scopes
            .push((NameSite::Expression(id), self.owner.clone()));
        id
    }
    pub fn parameter(&mut self, ordinal: u32, ty: TypeId, usage: UseKind) -> ResolvedName {
        assert_eq!(ordinal as usize, self.parameters.len());
        self.parameters.push(hirsel_types::Parameter {
            name: None,
            label: None,
            passing: passing(usage),
            ty,
        });
        let binding = ResolvedName::Parameter(ParamId(ordinal));
        self.checked.bindings.push(BindingDecision {
            owner: self.owner.clone(),
            binding: binding.clone(),
            site: self.owner.clone(),
            ty,
            usage,
            mutable: usage == UseKind::Mut,
        });
        binding
    }
    pub fn block(&mut self, statements: Vec<StmtId>, tail: ExprId) -> ExprId {
        let ty = self.checked.expressions[tail.0 as usize].ty;
        self.expr(
            ExprKind::Block {
                label: None,
                statements,
                tail: Some(tail),
            },
            ty,
            UseKind::Take,
            1,
        )
    }
    pub fn named_call(
        &mut self,
        name: &str,
        arguments: Vec<ExprId>,
        parameters: Vec<(TypeId, UseKind)>,
        result: TypeId,
        line: u32,
    ) -> ExprId {
        if !self.declarations.contains_key(&path(name)) {
            self.declare(name, parameters.clone(), result);
        }
        let callee = self.expr(
            ExprKind::Path(Path {
                package: None,
                segments: vec![],
            }),
            self.unit,
            UseKind::Borrow,
            line,
        );
        self.checked
            .resolution
            .expressions
            .push((callee, ResolvedName::Definition(path(name))));
        let uses: Vec<_> = arguments
            .iter()
            .copied()
            .zip(parameters.iter().map(|(_, usage)| *usage))
            .collect();
        let call = self.expr(
            ExprKind::Call {
                callee,
                arguments: arguments
                    .into_iter()
                    .map(|value| Argument { label: None, value })
                    .collect(),
            },
            result,
            UseKind::Take,
            line,
        );
        self.checked.calls.push(hirsel_types::CheckedCall {
            expression: call,
            target: hirsel_types::CallTarget::Definition(path(name)),
            arguments: uses
                .iter()
                .enumerate()
                .map(|(i, (expression, _))| hirsel_types::CheckedArgument {
                    expression: *expression,
                    parameter: Some((
                        i as u32,
                        hirsel_types::Parameter {
                            name: None,
                            label: None,
                            passing: passing(parameters[i].1),
                            ty: parameters[i].0,
                        },
                    )),
                })
                .collect(),
            generic_arguments: vec![],
            contextual_contracts: vec![],
            result,
            state: hirsel_types::CallState::Complete,
        });
        self.ownership.call_arguments.extend(
            uses.into_iter()
                .map(|(argument, usage)| (call, argument, usage)),
        );
        call
    }
    pub fn assign(
        &mut self,
        place: ExprId,
        op: AssignOp,
        value: ExprId,
        content: TypeId,
        line: u32,
    ) -> StmtId {
        self.ownership
            .replacements
            .push((place, content, Some(true)));
        self.stmt(StmtKind::Assign { place, op, value }, line)
    }
    pub fn declare(&mut self, name: &str, parameters: Vec<(TypeId, UseKind)>, result: TypeId) {
        let function = function(
            parameters
                .into_iter()
                .map(|(ty, usage)| hirsel_types::Parameter {
                    name: None,
                    label: None,
                    passing: passing(usage),
                    ty,
                })
                .collect(),
            result,
        );
        self.register(Signature {
            metadata: SignatureMetadata::default(),
            definition: DefId::intern(&path(name)),
            kind: SignatureKind::Function(function),
            fingerprint: ContentHash::default(),
        });
    }
    pub fn register(&mut self, signature: Signature) {
        self.declarations.insert(signature.definition, signature);
    }
    pub fn closure(&mut self, info: ClosureInfo, result: TypeId, capture_owner: NameSite) {
        self.frames
            .push((NameSite::Expression(info.expression), result));
        self.ownership
            .captures
            .extend(
                info.captures
                    .iter()
                    .enumerate()
                    .map(|(i, capture)| CaptureMode {
                        expression: info.expression,
                        ordinal: i as u32,
                        owner: capture_owner,
                        usage: capture.usage,
                        context: info.context,
                    }),
            );
        self.checked.closures.push(info);
    }
    pub fn input<'a>(
        &'a self,
        expression: ExprId,
        name: &str,
        result_types: &'a [(NameSite, TypeId)],
    ) -> Input<'a> {
        Input {
            types: session(),
            track_caller: false,
            syntax: &self.syntax,
            checked: &self.checked,
            plans: &self.plans,
            ownership: &self.ownership,
            result_types,
            owner: self.owner,
            expression,
            path: path(name),
            signature_hash: ContentHash::default(),
            unit: self.unit,
            boolean: self.boolean,
            arena: None,
        }
    }
    pub fn lower(
        &self,
        expression: ExprId,
        name: &str,
        result: TypeId,
    ) -> std::result::Result<Definition, LowerError> {
        let mut declarations = self.declarations.clone();
        if declarations.get(&path(name)).is_none() {
            declarations.insert(
                &path(name),
                Signature {
                    metadata: SignatureMetadata::default(),
                    definition: DefId::intern(&path(name)),
                    kind: SignatureKind::Function(function(self.parameters.clone(), result)),
                    fingerprint: ContentHash::default(),
                },
            );
        }
        let mut root = vec![(self.owner, result)];
        root.extend(self.frames.clone());
        let input = self.input(
            expression,
            name,
            if self.result_types.is_empty() {
                &root
            } else {
                &self.result_types
            },
        );
        let definition = lower(input)?;
        hirsel_validate::assert_valid(&definition, session(), &declarations);
        Ok(definition)
    }
}
pub fn ty(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
pub fn passing(usage: UseKind) -> PassingMode {
    match usage {
        UseKind::Take => PassingMode::Take,
        UseKind::Mut => PassingMode::Mut,
        UseKind::Borrow | UseKind::Copy => PassingMode::Borrow,
    }
}
pub fn function(parameters: Vec<hirsel_types::Parameter>, result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters.into(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }
}
