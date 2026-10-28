//! Manufactured revision-1 checked bodies. These fixtures do not implement a checker.
use super::*;
use hirsel_intern::Symbol;
use hirsel_syntax::{
    ContentHash, Expr, ExprKind, FunctionSyntax, Item, ItemKind, ItemPath, PackageId, Pattern,
    PatternKind, Stmt, StmtKind, TokenRange, UnaryOp, Visibility,
};
use std::sync::Arc;

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
/// The lexical owner of the fixture item's sites.
pub fn lexical() -> hirsel_syntax::LexicalOwner {
    hirsel_syntax::LexicalOwner::first(&path("example"))
}
/// A site of the fixture item, qualified by its owner.
pub fn scoped(site: NameSite) -> hirsel_syntax::SiteRef {
    lexical().site(site)
}
pub fn origin(root: OriginRoot) -> Origin {
    Origin {
        root,
        fields: vec![].into(),
    }
}
pub fn brand(root: OriginRoot) -> Brand {
    Brand {
        alternatives: vec![origin(root)].into(),
    }
}
pub fn node_origin(n: u32) -> hirsel_syntax::Origin {
    hirsel_syntax::Origin {
        span: Span {
            anchor: hirsel_span::Anchor(0),
            start: n,
            end: n + 1,
        },
        tokens: TokenRange { start: 0, end: 0 },
    }
}
pub fn function(result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }
}
pub fn syntax_function() -> FunctionSyntax {
    FunctionSyntax {
        attributes: vec![],
        generics: vec![],
        parameters: vec![],
        result: None,
        uses: None,
        contracts: vec![],
    }
}
pub fn type_row(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
#[derive(Default)]
pub struct Provider {
    pub signatures: BTreeMap<DefId, SignatureRef>,
    pub failures: BTreeMap<DefId, QueryError>,
    pub calls: Vec<DefId>,
}
impl SignatureProvider for Provider {
    fn signature(&mut self, d: DefId) -> Result<SignatureRef, QueryError> {
        self.calls.push(d);
        if let Some(e) = self.failures.get(&d) {
            return Err(e.clone());
        }
        self.signatures
            .get(&d)
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    fn lookup(&mut self, _: &hirsel_syntax::FileContext, _: &hirsel_syntax::Path) -> LookupOutput {
        panic!("authority must not perform name lookup")
    }
}
pub struct Fixture {
    pub types: &'static std::sync::Arc<TypeInterner>,
    pub memo: TypeMemo,
    pub tree: SyntaxTree,
    pub body: CheckedBody,
    pub signature: Signature,
    pub provider: Provider,
    pub intrinsics: Intrinsics,
    pub unit: TypeId,
}
impl Fixture {
    pub fn new() -> Self {
        let types = session_arc();
        let unit = types.intern(type_row(TypeKind::Tuple(vec![].into())));
        let signature = Signature {
            definition: DefId::intern(&path("example")),
            metadata: SignatureMetadata::default(),
            kind: SignatureKind::Function(function(unit)),
            fingerprint: ContentHash([1; 32]),
        };
        let tree = SyntaxTree::default();
        let body = CheckedBody {
            lexical: lexical(),
            tails: vec![],
            places: vec![],
            calls: vec![],
            bindings: vec![],
            patterns: vec![],
            resolution: Resolution::default(),
            expressions: vec![],
            instantiations: vec![],
            closures: vec![],
        };
        Self {
            types,
            memo: TypeMemo::new(),
            tree,
            body,
            signature,
            provider: Provider::default(),
            intrinsics: Intrinsics::new(),
            unit,
        }
    }
    /// The fixture item: a function with `body`, over the arenas built so far.
    pub fn syntax(&self, body: Option<ExprId>) -> hirsel_syntax::ItemSyntax {
        hirsel_syntax::ItemSyntax {
            item: Item {
                name: Symbol::intern("example"),
                name_span: node_origin(1).span,
                origin: node_origin(1),
                definition: self.signature.definition.path().clone(),
                visibility: Visibility::Private,
                attributes: vec![],
                kind: ItemKind::Function {
                    signature: syntax_function(),
                    body,
                    abi: None,
                },
                generated: None,
            },
            tree: self.tree.clone(),
            anchor: hirsel_span::Anchor(0),
            text_hash: ContentHash([0; 32]),
            occurrence: 0,
        }
    }
    pub fn ty(&mut self, kind: TypeKind) -> TypeId {
        self.types.intern(type_row(kind))
    }
    pub fn adjusted(&mut self, id: TypeId, change: impl FnOnce(&mut Type)) -> TypeId {
        let mut t = self.types.row(id).clone();
        change(&mut t);
        self.types.intern(t)
    }
    pub fn expr(&mut self, kind: ExprKind, ty: TypeId) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        self.tree.expressions.push(Expr {
            kind,
            origin: node_origin(100 + id.0 * 10),
        });
        self.body.expressions.push(Elaboration {
            construction_destination: None,
            expression: id,
            ty,
            usage: UseKind::Borrow,
            instantiated_arguments: vec![],
        });
        id
    }
    pub fn binding(
        &mut self,
        name: ResolvedName,
        ty: TypeId,
        owner: NameSite,
        usage: UseKind,
        mutable: bool,
    ) -> ExprId {
        let ty = bind_parameter_scopes(&self.types, ty, &[scoped(owner)]).unwrap();
        let symbol = Symbol::intern(&format!("binding{}", self.body.bindings.len()));
        let p = hirsel_syntax::PatternId(self.tree.patterns.len() as u32);
        self.tree.patterns.push(Pattern {
            origin: node_origin(50 + p.0),
            kind: PatternKind::Binding(symbol),
        });
        self.body.bindings.push(BindingDecision {
            owner: owner.clone(),
            site: NameSite::Pattern(p),
            binding: name.clone(),
            ty,
            usage,
            mutable,
        });
        self.body.patterns.push(PatternDecision {
            pattern: p,
            ty,
            usage,
            kind: match name {
                ResolvedName::Local(l) => PatternDecisionKind::Binding(l),
                _ => PatternDecisionKind::Wildcard,
            },
            fields: vec![],
            annotation: None,
        });
        self.reference(name, ty, owner)
    }
    pub fn reference(&mut self, name: ResolvedName, ty: TypeId, owner: NameSite) -> ExprId {
        let symbol = Symbol::intern("ref");
        let e = self.expr(
            ExprKind::Path(hirsel_syntax::Path {
                package: None,
                segments: vec![symbol],
            }),
            ty,
        );
        self.body.resolution.expressions.push((e, name));
        self.body
            .resolution
            .reference_scopes
            .push((NameSite::Expression(e), owner));
        e
    }
    pub fn closure(&mut self, value: ExprId, f: FunctionType) -> ExprId {
        let ty = self.ty(TypeKind::Function(f));
        self.expr(
            ExprKind::Closure {
                signature: syntax_function(),
                body: value,
            },
            ty,
        )
    }
    pub fn call(
        &mut self,
        name: &str,
        f: FunctionType,
        args: Vec<ExprId>,
        intrinsic: Option<Intrinsic>,
    ) -> ExprId {
        let definition = path(name);
        self.provider.signatures.insert(
            DefId::intern(&definition),
            Arc::new(Signature {
                definition: DefId::intern(&definition),
                metadata: SignatureMetadata::default(),
                kind: SignatureKind::Function(f.clone()),
                fingerprint: ContentHash([2; 32]),
            }),
        );
        if let Some(role) = intrinsic {
            self.intrinsics.insert(definition.clone(), role);
        }
        let fty = self.ty(TypeKind::Function(f.clone()));
        let callee = self.reference(ResolvedName::Definition(definition), fty, NameSite::Item);
        self.expr(
            ExprKind::Call {
                callee,
                arguments: args
                    .into_iter()
                    .map(|value| hirsel_syntax::Argument { label: None, value })
                    .collect(),
            },
            f.result,
        )
    }
    pub fn statement(&mut self, kind: StmtKind) -> hirsel_syntax::StmtId {
        let id = hirsel_syntax::StmtId(self.tree.statements.len() as u32);
        self.tree.statements.push(Stmt {
            kind,
            origin: node_origin(500 + id.0 * 10),
        });
        id
    }
    pub fn block(&mut self, expressions: Vec<ExprId>, tail: Option<ExprId>, ty: TypeId) -> ExprId {
        let statements = expressions
            .into_iter()
            .map(|e| self.statement(StmtKind::Expr(e)))
            .collect();
        self.expr(
            ExprKind::Block {
                label: None,
                statements,
                tail,
            },
            ty,
        )
    }
    pub fn capability(&mut self, name: &str, kind: CapabilityKind) -> TypeId {
        self.nominal(
            name,
            Some(kind),
            vec![Bound::Plain],
            AuthoritySummary::inert(),
            None,
            false,
        )
    }
    pub fn nominal(
        &mut self,
        name: &str,
        capability: Option<CapabilityKind>,
        bounds: Vec<Bound>,
        authority: AuthoritySummary,
        representation: Option<TypeId>,
        resource: bool,
    ) -> TypeId {
        let definition = DefId::intern(&path(name));
        self.provider.signatures.insert(
            definition,
            Arc::new(Signature {
                definition,
                metadata: SignatureMetadata::default(),
                fingerprint: ContentHash([3; 32]),
                kind: SignatureKind::Type(Box::new(TypeDefinition {
                    loop_source: None,
                    format: None,
                    generics: vec![],
                    representation,
                    bounds,
                    authority,
                    capability,
                    implementation_authority: None,
                    serializable: false,
                    serializable_check: None,
                    layout: Layout::default(),
                    distinct: false,
                    resource,
                    drop: None,
                    fact: None,
                    captures: None,
                    destructor_allocation: Allocation::NoAlloc,
                })),
            }),
        );
        self.ty(TypeKind::Named {
            definition,
            arguments: vec![].into(),
        })
    }
    pub fn finish(mut self, root: ExprId) -> AuthorityOutput {
        for (index, expression) in self.tree.expressions.iter().enumerate() {
            let ExprKind::Call { callee, arguments } = &expression.kind else {
                continue;
            };
            let id = ExprId(index as u32);
            let definition = self
                .body
                .resolution
                .expressions
                .iter()
                .find(|(e, _)| e == callee)
                .and_then(|(_, n)| {
                    if let ResolvedName::Definition(d) = n {
                        Some(d.clone())
                    } else {
                        None
                    }
                });
            let target = definition
                .clone()
                .map(CallTarget::Definition)
                .unwrap_or(CallTarget::Value(*callee));
            let ty = self
                .body
                .expressions
                .iter()
                .find(|r| r.expression == *callee)
                .unwrap()
                .ty;
            let (target, parameters, result) = match self.types.row(ty).kind.clone() {
                TypeKind::Function(f) => (target, f.parameters, f.result),
                TypeKind::Named { definition, .. } => {
                    let signature = &self.provider.signatures[&definition];
                    let SignatureKind::Type(t) = &signature.kind else {
                        panic!("constructor fixture")
                    };
                    let TypeKind::Record(fields) = &self.types.row(t.representation.unwrap()).kind
                    else {
                        panic!("record fixture")
                    };
                    (
                        CallTarget::Constructor(ContextualName::Constructor(
                            definition.path().clone(),
                        )),
                        fields
                            .iter()
                            .map(|f| parameter(f.ty, PassingMode::Take))
                            .collect(),
                        ty,
                    )
                }
                _ => panic!("call fixture needs callable type"),
            };
            let generic_arguments = self
                .body
                .instantiations
                .iter()
                .find(|r| r.expression == id && definition.as_ref() == Some(&r.callee))
                .map(|r| r.arguments.clone())
                .unwrap_or_default();
            self.body.calls.push(CheckedCall {
                expression: id,
                target,
                arguments: arguments
                    .iter()
                    .enumerate()
                    .map(|(i, a)| CheckedArgument {
                        expression: a.value,
                        parameter: parameters.get(i).cloned().map(|p| (i as u32, p)),
                    })
                    .collect(),
                generic_arguments,
                contextual_contracts: vec![],
                result,
                state: CallState::Complete,
            });
        }
        publish_places(&self.types, &self.tree, &mut self.body);
        let syntax = self.syntax(Some(root));
        check_file_with_clauses(
            &self.types,
            &self.memo,
            &syntax,
            &self.signature,
            self.body,
            &mut self.provider,
            &self.intrinsics,
            &[],
            &[],
        )
    }
}
/// The session interner every fixture's rows share.
pub fn session_arc() -> &'static std::sync::Arc<TypeInterner> {
    static SESSION: std::sync::OnceLock<std::sync::Arc<TypeInterner>> = std::sync::OnceLock::new();
    SESSION.get_or_init(|| TypeInterner::new(None))
}
pub fn session() -> &'static TypeInterner {
    session_arc()
}
/// The structural memo shared with the session interner.
pub fn session_memo() -> &'static TypeMemo {
    static MEMO: std::sync::OnceLock<TypeMemo> = std::sync::OnceLock::new();
    MEMO.get_or_init(TypeMemo::new)
}
pub fn parameter(ty: TypeId, passing: PassingMode) -> hirsel_types::Parameter {
    hirsel_types::Parameter {
        name: None,
        label: None,
        ty,
        passing,
    }
}
pub fn clean(out: &AuthorityOutput) {
    assert!(out.failure.is_none(), "{:?}", out.failure);
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
}
pub fn rejected(out: &AuthorityOutput, rule: &str) {
    assert!(out.failure.is_none(), "{:?}", out.failure);
    assert_eq!(
        out.diagnostics.first().map(|d| d.rule.as_str()),
        Some(rule),
        "{:?}",
        out.diagnostics
    );
}

/// Manufacture the checked storage rows specified by these typed test fixtures.
pub fn publish_places(t: &TypeInterner, tree: &SyntaxTree, body: &mut CheckedBody) {
    let specified = body.places.clone();
    body.places.clear();
    for row in &body.expressions.clone() {
        let id = row.expression;
        let root = body
            .resolution
            .expressions
            .iter()
            .find(|(e, _)| *e == id)
            .map(|(_, n)| n);
        let root = match root {
            Some(n @ (ResolvedName::Local(_) | ResolvedName::Parameter(_))) => {
                let scope = body
                    .resolution
                    .reference_scopes
                    .iter()
                    .find(|(s, _)| *s == NameSite::Expression(id))
                    .map(|(_, s)| s);
                let b = body
                    .bindings
                    .iter()
                    .find(|b| {
                        b.binding == *n
                            && (matches!(n, ResolvedName::Local(_)) || scope == Some(&b.owner))
                    })
                    .unwrap();
                SourceRoot::Binding {
                    owner: b.owner.clone(),
                    binding: n.clone(),
                }
            }
            Some(ResolvedName::Definition(d)) => SourceRoot::Definition(d.clone()),
            _ => SourceRoot::Value(id),
        };
        body.places.push(SourcePlace {
            expression: id,
            root,
            projections: specified
                .iter()
                .find(|p| p.expression == id)
                .map(|p| p.projections.clone())
                .unwrap_or_default(),
            access: SourceAccess::Read,
        });
    }
    for (index, expression) in tree.expressions.iter().enumerate() {
        let id = ExprId(index as u32);
        let Some(position) = body.places.iter().position(|p| p.expression == id) else {
            continue;
        };
        let (base, kind, access) = match &expression.kind {
            ExprKind::Unary {
                op: op @ (UnaryOp::Copy | UnaryOp::Take | UnaryOp::Mut | UnaryOp::Unique),
                value,
            } => (
                *value,
                None,
                match op {
                    UnaryOp::Copy => SourceAccess::Copy,
                    UnaryOp::Take => SourceAccess::Take,
                    UnaryOp::Mut => SourceAccess::Mut,
                    _ => SourceAccess::Unique,
                },
            ),
            ExprKind::Index { base, index } => (
                *base,
                Some(SourceProjectionKind::Index(*index)),
                SourceAccess::Read,
            ),
            ExprKind::Field { base, field } => {
                let ty = body
                    .expressions
                    .iter()
                    .find(|r| r.expression == *base)
                    .unwrap()
                    .ty;
                let contextual =
                    body.resolution
                        .contextual
                        .iter()
                        .find_map(|(site, name)| match (site, name) {
                            (
                                NameSite::Expression(e),
                                ContextualName::Field { field, variant, .. },
                            ) if *e == id => Some(match variant {
                                Some(variant) => SourceProjectionKind::VariantField {
                                    variant: *variant,
                                    field: *field,
                                },
                                None => SourceProjectionKind::Field(*field),
                            }),
                            _ => None,
                        });
                let supplied = specified
                    .iter()
                    .find(|p| p.expression == id)
                    .and_then(|p| p.projections.iter().find(|p| p.expression == id))
                    .map(|p| p.kind.clone());
                let kind = supplied.or(contextual).unwrap_or_else(|| {
                    let field = match &t.row(ty).kind {
                        TypeKind::Record(fields) => fields
                            .iter()
                            .position(|f| f.name.as_str() == field.as_str())
                            .unwrap() as u32,
                        TypeKind::Tuple(_) => field.as_str().parse().unwrap(),
                        _ => panic!("fixture field needs checked ordinal"),
                    };
                    SourceProjectionKind::Field(FieldId(field))
                });
                (*base, Some(kind), SourceAccess::Read)
            }
            _ => continue,
        };
        let mut place = body
            .places
            .iter()
            .find(|p| p.expression == base)
            .cloned()
            .unwrap();
        place.expression = id;
        place.access = access;
        if let Some(kind) = kind {
            place.projections.push(SourceProjection {
                expression: id,
                kind,
            });
        }
        body.places[position] = place;
    }
}
