use super::super::*;
use hirsel_intern::Symbol;
use hirsel_syntax::{self as s, *};
use std::{collections::BTreeMap, sync::Arc};

pub struct Provider {
    pub signatures: BTreeMap<DefPath, SignatureRef>,
    pub reads: Vec<DefPath>,
    pub error: Option<QueryError>,
}
impl Default for Provider {
    fn default() -> Self {
        Self {
            signatures: BTreeMap::new(),
            reads: vec![],
            error: None,
        }
    }
}
impl SignatureProvider for Provider {
    fn signature(&mut self, path: &DefPath) -> Result<SignatureRef, QueryError> {
        self.reads.push(path.clone());
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.signatures
            .get(path)
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    fn lookup(&mut self, _: &FileContext, _: &Path) -> LookupOutput {
        panic!("ownership must not resolve names")
    }
}
pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "test".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
pub fn origin(n: u32) -> s::Origin {
    s::Origin {
        span: Span::new(hirsel_span::FileId(0), n, n + 1).unwrap(),
        tokens: TokenRange::default(),
    }
}
pub fn ty(kind: TypeKind) -> Type {
    Type::with_kind(kind)
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
pub struct Fixture {
    pub tree: SyntaxTree,
    pub body: CheckedBody,
    pub signature: Signature,
    pub provider: Provider,
    pub bytes: TypeId,
    pub scalar: TypeId,
    pub unit: TypeId,
    pub statements: Vec<StmtId>,
    pub tail: Option<ExprId>,
    pub next_local: u32,
    pub captures: Vec<CaptureMode>,
}
impl Fixture {
    pub fn new() -> Self {
        let mut types = TypeStore::default();
        let bytes = types.push(ty(TypeKind::Primitive(Primitive::Bytes)));
        let scalar = types.push(ty(TypeKind::Primitive(Primitive::U64)));
        let unit = types.push(ty(TypeKind::Tuple(vec![].into())));
        let signature = Signature {
            metadata: SignatureMetadata::default(),
            definition: path("example"),
            types: types.clone(),
            kind: SignatureKind::Function(function(vec![], unit)),
            fingerprint: Default::default(),
        };
        let name = Symbol::intern("example");
        let tree = SyntaxTree {
            items: vec![Item {
                name_span: origin(0).span,
                origin: origin(0),
                owned_tokens: Default::default(),
                name,
                definition: path("example"),
                text_hash: Default::default(),
                visibility: Visibility::Private,
                attributes: vec![],
                kind: ItemKind::Function {
                    signature: FunctionSyntax::default(),
                    body: None,
                    abi: None,
                },
                generated: None,
            }],
            ..Default::default()
        };
        Self {
            tree,
            body: CheckedBody {
                tails: vec![],
                places: vec![],
                calls: vec![],
                bindings: vec![],
                patterns: vec![],
                types,
                resolution: Resolution::default(),
                expressions: vec![],
                instantiations: vec![],
                closures: vec![],
            },
            signature,
            provider: Provider::default(),
            bytes,
            scalar,
            unit,
            statements: vec![],
            tail: None,
            next_local: 0,
            captures: vec![],
        }
    }
    pub fn new_type(&mut self, kind: TypeKind, modes: Modes) -> TypeId {
        let mut t = ty(kind);
        t.modes = modes;
        self.body.types.push(t)
    }
    pub fn parameter(&mut self, ty: TypeId, passing: PassingMode) -> usize {
        let p = if let SignatureKind::Function(f) = &mut self.signature.kind {
            let id = f.parameters.len();
            let imported = import_type(&self.body.types, ty, &mut self.signature.types).unwrap();
            f.parameters.push(hirsel_types::Parameter {
                name: Some(format!("p{id}")),
                label: None,
                passing,
                ty: imported,
            });
            id
        } else {
            unreachable!()
        };
        let index = self.body.bindings.len();
        self.body.bindings.push(BindingDecision {
            owner: NameSite::Item(ItemId(0)),
            binding: ResolvedName::Parameter(ParamId(p as u32)),
            site: NameSite::Type(TypeExprId(p as u32)),
            ty,
            usage: if passing == PassingMode::Take {
                UseKind::Take
            } else {
                UseKind::Borrow
            },
            mutable: passing == PassingMode::Mut,
        });
        index
    }
    pub fn expr(&mut self, kind: ExprKind, ty: TypeId) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        self.tree.expressions.push(Expr {
            origin: origin(id.0 * 10 + 10),
            kind,
        });
        self.body.expressions.push(Elaboration {
            expression: id,
            ty,
            usage: UseKind::Borrow,
            instantiated_arguments: vec![],
        });
        if let ExprKind::Call { callee, arguments } = &self.tree.expressions[id.0 as usize].kind {
            if let TypeKind::Function(f) = &self
                .body
                .types
                .get(expression_type(&self.body, *callee).unwrap())
                .unwrap()
                .kind
            {
                self.body.calls.push(CheckedCall {
                    expression: id,
                    target: CallTarget::Value(*callee),
                    arguments: arguments
                        .iter()
                        .enumerate()
                        .map(|(i, a)| CheckedArgument {
                            expression: a.value,
                            parameter: Some((i as u32, f.parameters[i].clone())),
                        })
                        .collect(),
                    generic_arguments: vec![],
                    result: f.result,
                    state: CallState::Complete,
                });
            }
        }
        id
    }
    pub fn read(&mut self, binding: usize) -> ExprId {
        let name = Symbol::intern(&format!("b{binding}"));
        let ty = self.body.bindings[binding].ty;
        let id = self.expr(
            ExprKind::Path(Path {
                package: None,
                segments: vec![name],
            }),
            ty,
        );
        self.body
            .resolution
            .expressions
            .push((id, self.body.bindings[binding].binding.clone()));
        self.body.resolution.reference_scopes.push((
            NameSite::Expression(id),
            self.body.bindings[binding].owner.clone(),
        ));
        id
    }
    pub fn unary(&mut self, op: UnaryOp, value: ExprId) -> ExprId {
        let ty = expression_type(&self.body, value).unwrap();
        self.expr(ExprKind::Unary { op, value }, ty)
    }
    pub fn literal(&mut self, ty: TypeId) -> ExprId {
        self.expr(ExprKind::Literal(Literal::Bytes("b\"fresh\"".into())), ty)
    }
    pub fn stmt(&mut self, kind: StmtKind) -> StmtId {
        let id = StmtId(self.tree.statements.len() as u32);
        self.tree.statements.push(Stmt {
            origin: origin(id.0 * 10 + 500),
            kind,
        });
        id
    }
    pub fn expr_stmt(&mut self, id: ExprId) {
        let stmt = self.stmt(StmtKind::Expr(id));
        self.statements.push(stmt);
    }
    pub fn bind(&mut self, value: ExprId, mutable: bool) -> usize {
        let name = Symbol::intern(&format!("local{}", self.next_local));
        let pattern = PatternId(self.tree.patterns.len() as u32);
        self.tree.patterns.push(Pattern {
            origin: origin(pattern.0 + 300),
            kind: PatternKind::Binding(name),
        });
        let ty = expression_type(&self.body, value).unwrap();
        let local = LocalId(self.next_local);
        self.next_local += 1;
        let index = self.body.bindings.len();
        self.body.bindings.push(BindingDecision {
            owner: NameSite::Item(ItemId(0)),
            binding: ResolvedName::Local(local),
            site: NameSite::Pattern(pattern),
            ty,
            usage: UseKind::Borrow,
            mutable,
        });
        self.body.patterns.push(PatternDecision {
            pattern,
            ty,
            usage: UseKind::Borrow,
            kind: PatternDecisionKind::Binding(local),
            fields: vec![],
            annotation: None,
        });
        let stmt = self.stmt(StmtKind::Let {
            mutable,
            pattern,
            ty: None,
            value,
            otherwise: None,
        });
        self.statements.push(stmt);
        index
    }
    pub fn call(
        &mut self,
        name: &str,
        args: Vec<ExprId>,
        passing: Vec<PassingMode>,
        modes: Vec<Modes>,
        result_modes: Modes,
    ) -> ExprId {
        let mut types = TypeStore::default();
        let params = args
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let id = import_type(
                    &self.body.types,
                    expression_type(&self.body, *e).unwrap(),
                    &mut types,
                )
                .unwrap();
                let mut t = types.get(id).unwrap().clone();
                t.modes = modes[i].clone();
                let id = types.push(t);
                hirsel_types::Parameter {
                    name: None,
                    label: None,
                    passing: passing[i],
                    ty: id,
                }
            })
            .collect();
        let mut result = ty(TypeKind::Tuple(vec![].into()));
        result.modes = result_modes;
        let result = types.push(result);
        let signature = Signature {
            metadata: Default::default(),
            definition: path(name),
            types,
            kind: SignatureKind::Function(function(params, result)),
            fingerprint: ContentHash::of_bytes(name.as_bytes()),
        };
        self.provider
            .signatures
            .insert(path(name), Arc::new(signature));
        let symbol = Symbol::intern(name);
        let callee = self.expr(
            ExprKind::Path(Path {
                package: None,
                segments: vec![symbol],
            }),
            self.unit,
        );
        self.body
            .resolution
            .expressions
            .push((callee, ResolvedName::Definition(path(name))));
        let call = self.expr(
            ExprKind::Call {
                callee,
                arguments: args
                    .into_iter()
                    .map(|value| Argument { label: None, value })
                    .collect(),
            },
            self.unit,
        );
        let signature = self.provider.signatures[&path(name)].clone();
        let SignatureKind::Function(f) = &signature.kind else {
            unreachable!()
        };
        let parameters = f
            .parameters
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p.ty = import_type(&signature.types, p.ty, &mut self.body.types).unwrap();
                p
            })
            .collect::<Vec<_>>();
        let result = import_type(&signature.types, f.result, &mut self.body.types).unwrap();
        let ExprKind::Call { arguments, .. } = &self.tree.expressions[call.0 as usize].kind else {
            unreachable!()
        };
        self.body.calls.push(CheckedCall {
            expression: call,
            target: CallTarget::Definition(path(name)),
            arguments: arguments
                .iter()
                .enumerate()
                .map(|(i, a)| CheckedArgument {
                    expression: a.value,
                    parameter: Some((i as u32, parameters[i].clone())),
                })
                .collect(),
            generic_arguments: vec![],
            result,
            state: CallState::Complete,
        });
        call
    }
    pub fn check(&mut self) -> QueryOutput<OwnershipFacts> {
        let root = self.expr(
            ExprKind::Block {
                label: None,
                statements: self.statements.clone(),
                tail: self.tail,
            },
            self.tail
                .map(|id| expression_type(&self.body, id).unwrap())
                .unwrap_or(self.unit),
        );
        if let ItemKind::Function { body, .. } = &mut self.tree.items[0].kind {
            *body = Some(root);
        }
        crate::authority::test_support::publish_places(&self.tree, &mut self.body);
        check_ownership_with_clauses(
            &self.tree,
            ItemId(0),
            &self.signature,
            &mut self.body,
            &self.captures,
            &[],
            &mut [],
            &[],
            &[],
            &mut self.provider,
        )
        .unwrap()
    }
    pub fn usage(&self, id: ExprId) -> UseKind {
        self.body
            .expressions
            .iter()
            .find(|e| e.expression == id)
            .unwrap()
            .usage
    }
    pub fn modes(&self, id: ExprId) -> &Modes {
        &self
            .body
            .types
            .get(expression_type(&self.body, id).unwrap())
            .unwrap()
            .modes
    }
}
fn expression_type(body: &CheckedBody, id: ExprId) -> Option<TypeId> {
    body.expressions
        .iter()
        .find(|e| e.expression == id)
        .map(|e| e.ty)
}
