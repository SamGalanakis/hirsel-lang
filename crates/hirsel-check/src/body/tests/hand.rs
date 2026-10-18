use super::super::*;
use std::sync::Arc;
struct Provider {
    signatures: BTreeMap<ast::DefPath, SignatureRef>,
    failure: Option<QueryError>,
    reads: Vec<ast::DefPath>,
}
impl SignatureProvider for Provider {
    fn signature(&mut self, p: &ast::DefPath) -> Result<SignatureRef, QueryError> {
        self.reads.push(p.clone());
        if let Some(e) = &self.failure {
            return Err(e.clone());
        }
        self.signatures
            .get(p)
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    fn lookup(&mut self, _: &ast::FileContext, _: &ast::Path, _: &Interner) -> LookupOutput {
        panic!("resolved hand-built bodies must not look up names again")
    }
}
struct Fixture {
    tree: ast::SyntaxTree,
    symbols: Interner,
    resolution: Resolution,
    signature: Signature,
    provider: Provider,
}
fn origin(n: u32) -> ast::Origin {
    ast::Origin {
        span: Span::new(hirsel_span::FileId(0), n * 10, n * 10 + 5).unwrap(),
        tokens: Default::default(),
    }
}
fn row(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
fn function(result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        result,
        parameters: vec![].into(),
        generics: vec![].into(),
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }
}
impl Fixture {
    fn new(kind: TypeKind) -> Self {
        let mut store = TypeStore::default();
        let result = store.push(row(kind));
        let mut symbols = Interner::default();
        let name = symbols.intern("main");
        let tree = ast::SyntaxTree {
            items: vec![ast::Item {
                name_span: origin(0).span,
                origin: origin(0),
                owned_tokens: Default::default(),
                name,
                definition: super::support::path("main"),
                text_hash: Default::default(),
                visibility: ast::Visibility::Private,
                attributes: vec![],
                kind: ast::ItemKind::Function {
                    signature: Default::default(),
                    body: None,
                    abi: None,
                },
            }],
            ..Default::default()
        };
        Self {
            tree,
            symbols,
            resolution: Resolution::default(),
            signature: Signature {
                metadata: Default::default(),
                definition: super::support::path("main"),
                types: store,
                kind: SignatureKind::Function(function(result)),
                fingerprint: Default::default(),
            },
            provider: Provider {
                signatures: BTreeMap::new(),
                failure: None,
                reads: vec![],
            },
        }
    }
    fn ty(&mut self, kind: TypeKind) -> TypeId {
        self.signature.types.push(row(kind))
    }
    fn expr(&mut self, kind: ast::ExprKind) -> ast::ExprId {
        let id = ast::ExprId(self.tree.expressions.len() as u32);
        self.tree.expressions.push(ast::Expr {
            origin: origin(id.0 + 1),
            kind,
        });
        id
    }
    fn stmt(&mut self, kind: ast::StmtKind) -> ast::StmtId {
        let id = ast::StmtId(self.tree.statements.len() as u32);
        self.tree.statements.push(ast::Stmt {
            origin: origin(id.0 + 50),
            kind,
        });
        id
    }
    fn pattern(&mut self, kind: ast::PatternKind) -> ast::PatternId {
        let id = ast::PatternId(self.tree.patterns.len() as u32);
        self.tree.patterns.push(ast::Pattern {
            origin: origin(id.0 + 100),
            kind,
        });
        id
    }
    fn binding(&mut self, name: &str, local: u32) -> ast::PatternId {
        let name = self.symbols.intern(name);
        let p = self.pattern(ast::PatternKind::Binding(name));
        self.resolution.bindings.push((p, name, LocalId(local)));
        p
    }
    fn path(&mut self, name: &str, r: ResolvedName) -> ast::ExprId {
        let symbol = self.symbols.intern(name);
        let e = self.expr(ast::ExprKind::Path(ast::Path {
            package: None,
            segments: vec![symbol],
        }));
        self.resolution.expressions.push((e, r));
        e
    }
    fn parameter(&mut self, name: &str, ty: TypeId, passing: ast::PassingMode) {
        let symbol = self.symbols.intern(name);
        let syntax_id = ast::TypeExprId(self.tree.types.len() as u32);
        self.tree.types.push(ast::TypeExpr {
            origin: origin(200),
            kind: ast::TypeExprKind::Error,
        });
        if let SignatureKind::Function(f) = &mut self.signature.kind {
            f.parameters.push(Parameter {
                name: Some(name.into()),
                label: None,
                passing,
                ty,
            });
        }
        if let ast::ItemKind::Function { signature, .. } = &mut self.tree.items[0].kind {
            signature.parameters.push(ast::Parameter {
                name: Some(symbol),
                label: None,
                passing,
                ty: syntax_id,
            });
        }
    }
    fn finish(mut self, root: ast::ExprId) -> (Output, Vec<ast::DefPath>) {
        if let ast::ItemKind::Function { body, .. } = &mut self.tree.items[0].kind {
            *body = Some(root);
        }
        let out = check_with_loop_protocols(
            LoopInput {
                body: Input {
                    tree: &self.tree,
                    item: ast::ItemId(0),
                    symbols: &self.symbols,
                    context: &super::support::context(),
                    resolution: &self.resolution,
                    signature: &self.signature,
                },
                protocols: &hirsel_types::core_roles::loop_protocols(),
            },
            &mut self.provider,
        )
        .unwrap();
        (out, self.provider.reads)
    }
    fn block(&mut self, statements: Vec<ast::StmtId>, tail: Option<ast::ExprId>) -> ast::ExprId {
        self.expr(ast::ExprKind::Block { statements, tail })
    }
}
#[test]
fn every_integer_width_checks_min_and_overflow() {
    for (p, bits, signed) in [
        (Primitive::U8, 8, false),
        (Primitive::U16, 16, false),
        (Primitive::U32, 32, false),
        (Primitive::U64, 64, false),
        (Primitive::U128, 128, false),
        (Primitive::I8, 8, true),
        (Primitive::I16, 16, true),
        (Primitive::I32, 32, true),
        (Primitive::I64, 64, true),
        (Primitive::I128, 128, true),
    ] {
        let limit = if signed {
            1u128 << (bits - 1)
        } else if bits == 128 {
            u128::MAX
        } else {
            (1u128 << bits) - 1
        };
        let mut f = Fixture::new(TypeKind::Primitive(p));
        let value = f.expr(ast::ExprKind::Literal(ast::Literal::Integer(
            limit.to_string(),
        )));
        let root = if signed {
            f.expr(ast::ExprKind::Unary {
                op: ast::UnaryOp::Neg,
                value,
            })
        } else {
            value
        };
        let (out, _) = f.finish(root);
        assert!(out.checked.diagnostics.is_empty(), "{p:?}");
        let too_big = limit
            .checked_add(1)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "340282366920938463463374607431768211456".into());
        let mut f = Fixture::new(TypeKind::Primitive(p));
        let root = f.expr(ast::ExprKind::Literal(ast::Literal::Integer(too_big)));
        let (out, _) = f.finish(root);
        assert_eq!(out.checked.diagnostics[0].rule.as_str(), "SEM-1");
    }
}
#[test]
fn recovery_is_absorbing_and_retains_nested_binders() {
    let mut f = Fixture::new(TypeKind::Tuple(vec![].into()));
    let a = f.binding("a", 0);
    let w = f.pattern(ast::PatternKind::Error);
    let pattern = f.pattern(ast::PatternKind::Tuple(vec![a, w]));
    let value = f.expr(ast::ExprKind::Error);
    let stmt = f.stmt(ast::StmtKind::Let {
        mutable: false,
        pattern,
        ty: None,
        value,
    });
    let root = f.block(vec![stmt], None);
    let (out, _) = f.finish(root);
    assert!(out.checked.diagnostics.is_empty());
    assert_eq!(out.checked.value.patterns.len(), 3);
    assert_eq!(out.checked.value.bindings.len(), 1);
    assert!(
        out.checked
            .value
            .patterns
            .iter()
            .all(|p| matches!(p.kind, PatternDecisionKind::Error))
    );
}
#[test]
fn expression_worklist_handles_twenty_thousand_levels() {
    let mut f = Fixture::new(TypeKind::Primitive(Primitive::U8));
    let mut root = f.expr(ast::ExprKind::Literal(ast::Literal::Integer("1u8".into())));
    for _ in 0..20_000 {
        root = f.expr(ast::ExprKind::Unary {
            op: ast::UnaryOp::Copy,
            value: root,
        });
    }
    let (out, _) = f.finish(root);
    assert!(out.checked.diagnostics.is_empty());
    assert_eq!(out.checked.value.expressions.len(), 20_001);
}
#[test]
fn nonrecursive_lookup_failures_are_recorded() {
    let mut f = Fixture::new(TypeKind::Tuple(vec![].into()));
    let path = super::support::path("callee");
    f.provider.failure = Some(QueryError::Diagnostics(vec![Diagnostic::error(
        const { RuleId::of("LANG-7") },
        origin(9).span,
        "bad callee signature",
    )]));
    let root = f.path("callee", ResolvedName::Definition(path.clone()));
    let (out, reads) = f.finish(root);
    assert_eq!(reads, vec![path.clone()]);
    assert_eq!(out.checked.diagnostics.len(), 1);
    assert!(
        out.checked
            .value
            .resolution
            .reads
            .contains(&DependencyRead::Signature {
                definition: path,
                fingerprint: None
            })
    );
    assert!(out.checked.dependencies.is_empty());
}

#[test]
fn generic_foreign_calls_report_gen4_at_the_call() {
    let mut f = Fixture::new(TypeKind::Primitive(Primitive::U8));
    let mut types = TypeStore::default();
    let t = types.push(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let mut fun = function(t);
    fun.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    });
    fun.foreign = Some(ForeignContract {
        abi: "C".into(),
        blocking: false,
        aliasing_contract: "none".into(),
    });
    let path = super::support::path("foreign");
    f.provider.signatures.insert(
        path.clone(),
        Arc::new(Signature {
            definition: path.clone(),
            metadata: Default::default(),
            types,
            kind: SignatureKind::Function(fun),
            fingerprint: Default::default(),
        }),
    );
    let callee = f.path("foreign", ResolvedName::Definition(path));
    let root = f.expr(ast::ExprKind::Call {
        callee,
        arguments: vec![],
    });
    let (output, _) = f.finish(root);
    assert_eq!(output.checked.diagnostics.len(), 1);
    assert_eq!(output.checked.diagnostics[0].rule.as_str(), "GEN-4");
    assert_eq!(
        output.checked.diagnostics[0].primary,
        origin(root.0 + 1).span
    );
}
#[test]
fn manufactured_result_tags_and_constructor_calls_keep_declared_ordinals() {
    let mut f = Fixture::new(TypeKind::Tuple(vec![].into()));
    let definition = super::support::core("Result");
    let mut types = TypeStore::default();
    let a = types.push(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let b = types.push(row(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(1),
    ))));
    let representation = types.push(row(TypeKind::Variant(
        vec![
            Variant {
                name: "Error".into(),
                fields: vec![(None, b)].into(),
            },
            Variant {
                name: "Ok".into(),
                fields: vec![(None, a)].into(),
            },
        ]
        .into(),
    )));
    f.provider.signatures.insert(
        definition.clone(),
        Arc::new(Signature {
            definition: definition.clone(),
            metadata: SignatureMetadata {
                roles: NominalRoles {
                    result_type: Some(definition.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            types,
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                format: None,
                destructor_allocation: Allocation::NoAlloc,
                generics: vec![
                    GenericParameter::Type {
                        name: "T".into(),
                        bounds: vec![].into(),
                    },
                    GenericParameter::Type {
                        name: "E".into(),
                        bounds: vec![].into(),
                    },
                ],
                representation: Some(representation),
                bounds: vec![],
                authority: AuthoritySummary::inert(),
                capability: None,
                implementation_authority: None,
                serializable: false,
                layout: Default::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
            }),
            fingerprint: Default::default(),
        }),
    );
    let item = f.ty(TypeKind::Primitive(Primitive::U8));
    let error = f.ty(TypeKind::Primitive(Primitive::U16));
    let result = f.ty(TypeKind::Named {
        definition: definition.clone(),
        arguments: vec![GenericArgument::Type(item), GenericArgument::Type(error)].into(),
    });
    if let SignatureKind::Function(fun) = &mut f.signature.kind {
        fun.result = result;
        fun.error = Some(error);
    }
    f.parameter("r", result, ast::PassingMode::Take);
    let value = f.path("r", ResolvedName::Parameter(ParamId(0)));
    let propagate = f.expr(ast::ExprKind::Try {
        value,
        map_error: None,
    });
    let callee = f.path("Ok", ResolvedName::Definition(definition.clone()));
    f.resolution.contextual.push((
        NameSite::Expression(callee),
        ContextualName::Variant {
            owner: definition.clone(),
            variant: VariantId(1),
        },
    ));
    let root = f.expr(ast::ExprKind::Call {
        callee,
        arguments: vec![ast::Argument {
            label: None,
            value: propagate,
        }],
    });
    let (output, reads) = f.finish(root);
    assert!(
        output.checked.diagnostics.is_empty(),
        "{:?}",
        output.checked.diagnostics
    );
    assert_eq!(reads, vec![definition]);
    assert_eq!(output.plans.tries[0].success_variant, VariantId(1));
    assert_eq!(output.plans.tries[0].error_variant, VariantId(0));
    assert_eq!(output.plans.tries[0].propagated_error_variant, VariantId(0));
    assert!(matches!(
        output.checked.value.calls[0].target,
        CallTarget::Constructor(ContextualName::Variant {
            variant: VariantId(1),
            ..
        })
    ));
}

#[test]
fn finalizer_consumes_resolver_identity_instead_of_first_local() {
    let mut f = Fixture::new(TypeKind::Tuple(vec![].into()));
    let binder = f.symbols.intern("self_value");
    let root = f.block(vec![], None);
    f.tree.items[0].kind = ast::ItemKind::Type {
        flavor: ast::TypeFlavor::Resource,
        generics: vec![],
        bounds: None,
        authority: None,
        body: ast::TypeBody::Record(vec![]),
        drop: Some(ast::DropSyntax {
            noalloc: false,
            binding: binder,
            body: root,
        }),
    };
    f.signature.kind = SignatureKind::Type(TypeDefinition {
        loop_source: None,
        format: None,
        destructor_allocation: Allocation::MayAllocate,
        generics: vec![],
        representation: None,
        bounds: vec![],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority: None,
        serializable: false,
        layout: Layout::default(),
        distinct: false,
        resource: true,
        drop: Some(f.signature.definition.clone()),
        fact: None,
    });
    f.resolution.drop_binding = Some(LocalId(37));
    let (output, _) = f.finish(root);
    assert!(output.checked.diagnostics.is_empty());
    assert_eq!(output.checked.value.bindings.len(), 1);
    assert_eq!(
        output.checked.value.bindings[0].binding,
        ResolvedName::Local(LocalId(37))
    );
    assert_eq!(output.checked.value.bindings[0].usage, UseKind::Borrow);
}

#[test]
fn heap_requires_its_complete_private_producer_contract() {
    for mutation in 0..3 {
        let mut fixture = Fixture::new(TypeKind::Tuple(vec![].into()));
        let heap = fixture.expr(ast::ExprKind::Heap);
        let statement = fixture.stmt(ast::StmtKind::Expr(heap));
        let root = fixture.block(vec![statement], None);
        if let ast::ItemKind::Function { body, .. } = &mut fixture.tree.items[0].kind {
            *body = Some(root);
        }
        for mut signature in super::support::heap_signatures() {
            fixture.signature.metadata.roles.heap = signature.metadata.roles.heap.clone();
            if let SignatureKind::Function(f) = &mut signature.kind {
                match mutation {
                    0 => f.allocation = Allocation::MayAllocate,
                    1 => f.authority = AuthoritySummary::Unknown,
                    _ => {
                        let mut result = signature.types.get(f.result).unwrap().clone();
                        result.allocation_target = None;
                        f.result = signature.types.push(result);
                    }
                }
            }
            fixture
                .provider
                .signatures
                .insert(signature.definition.clone(), Arc::new(signature));
        }
        let result = check_with_loop_protocols(
            LoopInput {
                body: Input {
                    tree: &fixture.tree,
                    item: ast::ItemId(0),
                    symbols: &fixture.symbols,
                    context: &super::support::context(),
                    resolution: &fixture.resolution,
                    signature: &fixture.signature,
                },
                protocols: &hirsel_types::core_roles::loop_protocols(),
            },
            &mut fixture.provider,
        );
        assert!(
            matches!(result, Err(QueryError::Operational(_))),
            "heap accepted malformed producer {mutation}"
        );
    }
}
