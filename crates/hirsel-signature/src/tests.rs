use super::*;
use hirsel_intern::Symbol;
use hirsel_run_schema::hex;
use hirsel_types::encoding::{EncodingError, canonical_encoding as encode};
/// The session interner and structural memo these tests share.
fn session() -> &'static TypeInterner {
    static SESSION: std::sync::OnceLock<std::sync::Arc<TypeInterner>> = std::sync::OnceLock::new();
    SESSION.get_or_init(|| TypeInterner::new(None))
}
fn memo() -> &'static TypeMemo {
    static MEMO: std::sync::OnceLock<TypeMemo> = std::sync::OnceLock::new();
    MEMO.get_or_init(TypeMemo::new)
}
/// Every row a signature's roots reach, in first-encounter order.
fn rows(s: &Signature) -> Vec<(TypeId, Type)> {
    let roots: Vec<TypeId> = match &s.kind {
        SignatureKind::Function(f) => f
            .parameters
            .iter()
            .map(|p| p.ty)
            .chain([f.result])
            .chain(f.error)
            .collect(),
        SignatureKind::Type(t) => t.representation.into_iter().collect(),
        SignatureKind::Const { ty, .. } => vec![*ty],
        SignatureKind::Implementation(i) => i.types(),
        SignatureKind::Family(_) => vec![],
    };
    closure(session(), roots)
        .iter()
        .map(|t| (*t, session().row(*t).clone()))
        .collect()
}
fn canonical_encoding(
    s: &Signature,
    deps: &[SignatureDependency],
) -> Result<Vec<u8>, EncodingError> {
    encode(session(), s, deps)
}
fn def(name: &str) -> ast::DefPath {
    ast::DefPath(ast::ItemPath {
        package: ast::PackageId {
            name: "test".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
fn origin() -> ast::Origin {
    ast::Origin {
        span: Span::default(),
        tokens: ast::TokenRange::default(),
    }
}
#[derive(Default)]
struct Fake {
    signatures: BTreeMap<ast::DefPath, Signature>,
    reads: Vec<ast::DefPath>,
}
impl SignatureProvider for Fake {
    fn signature(&mut self, d: DefId) -> Result<SignatureRef, QueryError> {
        (|| {
            let d = d.path();
            self.reads.push(d.clone());
            self.signatures
                .get(d)
                .cloned()
                .ok_or(QueryError::NotImplemented)
        })()
        .map(std::sync::Arc::new)
    }
    fn lookup(&mut self, c: &ast::FileContext, p: &ast::Path) -> hirsel_types::LookupOutput {
        hirsel_types::LookupOutput {
            manifest: Default::default(),
            result: (|| {
                let mut segments = p
                    .segments
                    .iter()
                    .map(|s| s.as_str().to_owned())
                    .collect::<Vec<_>>();
                let name = segments.pop().unwrap();
                Ok(ast::DefPath(ast::ItemPath {
                    package: p.package.clone().unwrap_or_else(|| c.package.clone()),
                    modules: if segments.is_empty() {
                        c.modules.clone()
                    } else {
                        segments
                    },
                    name,
                }))
            })(),
        }
    }
}
struct Fixture {
    syntax: ast::ItemSyntax,
    resolution: Resolution,
    metadata: Metadata,
    provider: Fake,
    context: ast::FileContext,
}
impl Fixture {
    fn new() -> Self {
        let name = Symbol::intern("example");
        Self {
            syntax: ast::ItemSyntax {
                item: ast::Item {
                    name_span: Default::default(),
                    origin: origin(),
                    name,
                    definition: def("example"),
                    visibility: ast::Visibility::Private,
                    attributes: vec![],
                    kind: ast::ItemKind::Function {
                        signature: Default::default(),
                        body: None,
                        abi: None,
                    },
                    generated: None,
                },
                tree: Default::default(),
                anchor: Default::default(),
                text_hash: ast::ContentHash::default(),
                occurrence: 0,
            },
            resolution: Default::default(),
            metadata: Default::default(),
            provider: Default::default(),
            context: ast::FileContext {
                package: def("example").0.package,
                modules: vec![],
            },
        }
    }
    fn ty(&mut self, kind: ast::TypeExprKind) -> ast::TypeExprId {
        let id = ast::TypeExprId(self.syntax.tree.types.len() as u32);
        self.syntax.tree.types.push(ast::TypeExpr {
            origin: origin(),
            kind,
        });
        id
    }
    fn qualified(
        &mut self,
        base: ast::TypeExprId,
        modifiers: Vec<ast::TypeModifier>,
        brand: Option<ast::BrandSyntax>,
        places: Option<Vec<ast::RelationPlace>>,
    ) -> ast::TypeExprId {
        self.ty(ast::TypeExprKind::Qualified {
            base,
            modifiers,
            brand,
            relation: places.map(|places| ast::RelationSyntax {
                places,
                origin: origin(),
            }),
        })
    }
    fn named(&mut self, n: &str) -> ast::TypeExprId {
        let s = Symbol::intern(n);
        self.ty(ast::TypeExprKind::Named {
            path: ast::Path {
                package: None,
                segments: vec![s],
            },
            arguments: vec![],
        })
    }
    fn application(&mut self, n: &str, args: Vec<ast::TypeArgument>) -> ast::TypeExprId {
        let s = Symbol::intern(n);
        self.ty(ast::TypeExprKind::Named {
            path: ast::Path {
                package: None,
                segments: vec![s],
            },
            arguments: args,
        })
    }
    fn f(&mut self) -> &mut ast::FunctionSyntax {
        match &mut self.syntax.item.kind {
            ast::ItemKind::Function { signature, .. } => signature,
            _ => panic!(),
        }
    }
    fn param(&mut self, n: &str, t: ast::TypeExprId, passing: PassingMode) {
        let n = Symbol::intern(n);
        self.f().parameters.push(ast::Parameter {
            name: Some(n),
            label: None,
            ty: t,
            passing,
        });
    }
    fn attr(&mut self, n: &str) {
        let n = Symbol::intern(n);
        self.syntax.item.attributes.push(ast::Attribute {
            name: n,
            arguments: vec![],
            origin: origin(),
        });
    }
    fn result(&mut self, t: ast::TypeExprId) {
        self.f().result = Some(t);
    }
    fn run(&mut self) -> Result<QueryOutput<Signature>, QueryError> {
        normalize(
            Input {
                types: session(),
                memo: memo(),
                syntax: &self.syntax,
                context: &self.context,
                resolution: &self.resolution,
                metadata: &self.metadata,
                declaration_sites: &self.metadata.declarations,
                stage: None,
            },
            &mut self.provider,
        )
    }
    fn sig(&mut self) -> Signature {
        self.run().unwrap().value
    }
    fn declaration(&mut self, body: ast::TypeBody, flavor: ast::TypeFlavor) {
        self.syntax.item.kind = ast::ItemKind::Type {
            flavor,
            generics: vec![],
            bounds: None,
            authority: None,
            body,
            drop: None,
        };
    }
    fn expression(&mut self, kind: ast::ExprKind) -> ast::ExprId {
        let id = ast::ExprId(self.syntax.tree.expressions.len() as u32);
        self.syntax.tree.expressions.push(ast::Expr {
            origin: origin(),
            kind,
        });
        id
    }
    fn dependency(
        &mut self,
        n: &str,
        capability: Option<CapabilityKind>,
        public: bool,
    ) -> ast::DefPath {
        let d = def(n);
        let s = Signature {
            metadata: SignatureMetadata {
                visibility: if public {
                    ast::Visibility::Public
                } else {
                    ast::Visibility::Private
                },
                ..Default::default()
            },
            definition: DefId::intern(&d),
            kind: SignatureKind::Type(Box::new(TypeDefinition {
                loop_source: None,
                format: None,
                destructor_allocation: hirsel_types::Allocation::MayAllocate,

                generics: vec![],
                representation: None,
                bounds: vec![],
                authority: AuthoritySummary::inert(),
                capability,
                implementation_authority: None,
                serializable: false,
                serializable_check: None,
                layout: Default::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
                captures: None,
            })),
            fingerprint: ast::ContentHash::of_bytes(n.as_bytes()),
        };
        self.provider.signatures.insert(d.clone(), s);
        self.metadata.definitions.insert(
            d.clone(),
            DefinitionMetadata {
                public,
                alias: false,
            },
        );
        d
    }
    fn field(&mut self, n: &str, t: ast::TypeExprId) -> ast::FieldSyntax {
        ast::FieldSyntax {
            growable: false,

            name: Symbol::intern(n),
            ty: t,
            origin: origin(),
        }
    }
}
fn function(s: &Signature) -> &FunctionType {
    if let SignatureKind::Function(f) = &s.kind {
        f
    } else {
        panic!()
    }
}
fn typedef(s: &Signature) -> &TypeDefinition {
    if let SignatureKind::Type(t) = &s.kind {
        t
    } else {
        panic!()
    }
}
fn error(f: &mut Fixture, rule: &str, span: Span) {
    let Err(QueryError::Diagnostics(ds)) = f.run() else {
        panic!("expected {rule}")
    };
    assert_eq!(ds[0].rule.as_str(), rule);
    assert_eq!(ds[0].primary, span);
}
#[test]
fn signatures_share_session_rows_and_import_aliases() {
    let mut source = Fixture::new();
    let u = source.named("u32");
    let t = source.ty(ast::TypeExprKind::Tuple(vec![u, u]));
    source.declaration(ast::TypeBody::Alias(t), ast::TypeFlavor::Ordinary);
    let mut s = source.sig();
    s.definition = DefId::intern(&def("Pair"));
    let mut f = Fixture::new();
    f.provider.signatures.insert(def("Pair"), s.clone());
    f.metadata.definitions.insert(
        def("Pair"),
        DefinitionMetadata {
            public: true,
            alias: true,
        },
    );
    let junk = f.named("String");
    f.param("padding", junk, PassingMode::Take);
    let pair = f.named("Pair");
    f.result(pair);
    let a = f.sig();
    let b = f.sig();
    assert_eq!(canonical_encoding(&a, &[]), canonical_encoding(&b, &[]));
    // One session row per content: the alias's tuple is the result's tuple.
    assert_eq!(typedef(&s).representation.unwrap(), function(&a).result);
    let TypeKind::Tuple(ts) = &session().row(function(&b).result).kind else {
        panic!()
    };
    for t in ts.iter() {
        assert_eq!(session().row(*t).kind, TypeKind::Primitive(Primitive::U32));
    }
}

#[test]
fn diagnostic_lang7_repeated_modifier_cites_type_span() {
    for repeated in [true, false] {
        let mut f = Fixture::new();
        let ty = f.named("String");
        let modifiers = if repeated {
            vec![ast::TypeModifier::Unique, ast::TypeModifier::Unique]
        } else {
            vec![ast::TypeModifier::Unique]
        };
        let ty = f.qualified(ty, modifiers, None, None);
        let span = Span::new(hirsel_span::Anchor(1), 10, 15).unwrap();
        f.syntax.tree.types[ty.0 as usize].origin.span = span;
        f.result(ty);
        if repeated {
            error(&mut f, "LANG-7", span);
        } else {
            f.sig();
        }
    }
}
#[test]
fn diagnostic_gen1_size_as_type_cites_type_span() {
    let mut f = Fixture::new();
    let ty = f.named("n");
    let name = Symbol::intern("n");
    f.f().generics.push(ast::GenericParam {
        name,
        kind: ast::GenericKind::Size,
    });
    let span = Span::new(hirsel_span::Anchor(3), 4, 9).unwrap();
    f.syntax.tree.types[ty.0 as usize].origin.span = span;
    f.result(ty);
    let Err(QueryError::Diagnostics(errors)) = f.run() else {
        panic!("expected size-as-type diagnostic")
    };
    let [error] = &errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(error.rule.as_str(), "GEN-1");
    assert_eq!(error.primary, span);
    assert_eq!(error.message, "size parameter cannot be used as a type");
    assert_eq!(
        error.notes,
        [
            "help: `n` is a size parameter; use it in a size argument, for example `Inline[u8, n]`. A type position needs a type parameter: declare a separate `T` with `[T, n: size]` and use `T` here, or change the binder to `[n]` if it was intended as a type parameter"
        ]
    );
    assert!(error.fix.is_none());
}
#[test]
fn diagnostic_gen4_generic_ffi_cites_header_span() {
    let mut f = Fixture::new();
    let name = Symbol::intern("T");
    f.f().generics.push(ast::GenericParam {
        name,
        kind: ast::GenericKind::Type(vec![]),
    });
    if let ast::ItemKind::Function { abi, .. } = &mut f.syntax.item.kind {
        *abi = Some(ast::ForeignAbi {
            literal: "\"c\"".into(),
            contract: None,
        });
    }
    let span = Span::new(hirsel_span::Anchor(3), 10, 20).unwrap();
    f.syntax.item.origin.span = span;
    error(&mut f, "GEN-4", span);
}
#[test]
fn diagnostic_prog4_extern_requires_package_metadata() {
    let mut f = Fixture::new();
    if let ast::ItemKind::Function { abi, .. } = &mut f.syntax.item.kind {
        *abi = Some(ast::ForeignAbi {
            literal: "\"c\"".into(),
            contract: None,
        });
    }
    let span = Span::new(hirsel_span::Anchor(4), 10, 12).unwrap();
    f.syntax.item.origin.span = span;
    error(&mut f, "PROG-4", span);
}
#[test]
fn diagnostics_validate_variant_labels_and_generic_bounds() {
    let mut f = Fixture::new();
    let ty = f.named("u8");
    let name = Symbol::intern("Some");
    let label = Symbol::intern("x");
    f.declaration(
        ast::TypeBody::Variants(vec![ast::VariantSyntax {
            name,
            fields: vec![(Some(label), ty), (None, ty)],
        }]),
        ast::TypeFlavor::Ordinary,
    );
    error(&mut f, "LANG-12", Span::default());
    let mut f = Fixture::new();
    let d = f.dependency("NeedsKey", None, true);
    if let SignatureKind::Type(t) = &mut f.provider.signatures.get_mut(&d).unwrap().kind {
        t.generics.push(GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Key].into(),
        });
    }
    let ty = f.named("f64");
    let ty = f.application("NeedsKey", vec![ast::TypeArgument::Type(ty)]);
    f.result(ty);
    error(&mut f, "GEN-1", Span::default());
}
#[test]
fn opaque_public_signatures_require_authority_and_hide_representation() {
    let mut f = Fixture::new();
    f.declaration(ast::TypeBody::Opaque, ast::TypeFlavor::Ordinary);
    f.syntax.item.visibility = ast::Visibility::PublicOpaque;
    error(&mut f, "LANG-6", Span::default());
    if let ast::ItemKind::Type {
        authority, bounds, ..
    } = &mut f.syntax.item.kind
    {
        *authority = Some(vec![]);
        *bounds = Some(vec![]);
    }
    let a = f.sig();
    let ty = f.named("u32");
    if let ast::ItemKind::Type { body, .. } = &mut f.syntax.item.kind {
        *body = ast::TypeBody::Alias(ty);
    }
    let b = f.sig();
    assert_eq!(a.fingerprint, b.fingerprint);
    assert!(typedef(&b).representation.is_none());
}
fn foreign_fixture() -> Fixture {
    let mut f = Fixture::new();
    if let ast::ItemKind::Function { abi, .. } = &mut f.syntax.item.kind {
        *abi = Some(ast::ForeignAbi {
            literal: "\"c\"".into(),
            contract: None,
        });
    }
    f
}
fn unsafe_ffi_package(f: &mut Fixture) {
    let package = f.syntax.item.definition.0.package.clone();
    f.metadata.unsafe_ffi.insert(package, UnsafeFfi::default());
}
fn foreign_contract(f: &mut Fixture, text: &str) {
    if let ast::ItemKind::Function { abi: Some(abi), .. } = &mut f.syntax.item.kind {
        abi.contract = Some(text.into());
    }
}
fn unsafe_parameter(f: &mut Fixture) {
    f.metadata.unsafe_capability = Some(f.dependency("Unsafe", Some(CapabilityKind::Global), true));
    let ty = f.named("Unsafe");
    f.param("u", ty, PassingMode::Borrow);
}
#[test]
fn foreign_signature_requires_an_unsafe_parameter_and_records_its_contract() {
    let mut f = foreign_fixture();
    unsafe_ffi_package(&mut f);
    foreign_contract(&mut f, "reads nothing");
    error(&mut f, "PROG-4", Span::default());
    let mut f = foreign_fixture();
    unsafe_ffi_package(&mut f);
    unsafe_parameter(&mut f);
    foreign_contract(&mut f, "borrow does not retain");
    f.attr("blocking");
    // The parser attaches a function's attributes to its signature as well, which is
    // checked as the extern's own.
    f.f().attributes.push(ast::Attribute {
        name: Symbol::intern("blocking"),
        arguments: vec![],
        origin: origin(),
    });
    let s = f.sig();
    assert!(s.metadata.unsafe_ffi);
    let foreign = function(&s).foreign.as_ref().unwrap();
    assert!(foreign.blocking);
    assert_eq!(foreign.aliasing_contract, "borrow does not retain");
}
#[test]
fn foreign_rejects_a_bytes_result_and_accepts_numerics_and_pointers() {
    let foreign = || {
        let mut f = foreign_fixture();
        unsafe_ffi_package(&mut f);
        unsafe_parameter(&mut f);
        foreign_contract(&mut f, "explicit parameter ownership");
        f
    };
    let mut f = foreign();
    let ty = f.named("Bytes");
    f.param("x", ty, PassingMode::Borrow);
    f.result(ty);
    error(&mut f, "PROG-4", Span::default());
    let mut f = foreign();
    let bytes = f.named("Bytes");
    f.param("x", bytes, PassingMode::Borrow);
    let ty = f.named("u32");
    f.param("n", ty, PassingMode::Take);
    let ptr = f.application("Ptr", vec![ast::TypeArgument::Type(ty)]);
    f.result(ptr);
    f.sig();
}

#[test]
fn hash_traverses_roots_instead_of_semantic_arena_order() {
    let mut f = Fixture::new();
    let a = f.named("String");
    let b = f.named("u8");
    f.param("x", a, PassingMode::Borrow);
    let t = f.ty(ast::TypeExprKind::Tuple(vec![a, b]));
    f.result(t);
    let a = f.sig();
    // A second interner assigns other ids (and scrambles them differently
    // after unrelated rows); the encoding traverses roots, not ids.
    let other = TypeInterner::new(None);
    other.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bool)));
    let other_memo = TypeMemo::new();
    let b = normalize(
        Input {
            types: &other,
            memo: &other_memo,
            syntax: &f.syntax,
            context: &f.context,
            resolution: &f.resolution,
            metadata: &f.metadata,
            declaration_sites: &f.metadata.declarations,
            stage: None,
        },
        &mut f.provider,
    )
    .unwrap()
    .value;
    assert_ne!(function(&a).result, function(&b).result);
    assert_eq!(
        canonical_encoding(&a, &[]).unwrap(),
        encode(&other, &b, &[]).unwrap()
    );
}
#[test]
fn hash_ignores_locations_whitespace_comments_and_text_hash() {
    let mut f = Fixture::new();
    let c = f.expression(ast::ExprKind::Literal(ast::Literal::Bool(true)));
    f.f().contracts.push(ast::ContractSyntax {
        kind: ast::ContractKind::Requires,
        expression: c,
    });
    let before = f.sig();
    f.syntax.text_hash =
        ast::ContentHash::of_bytes(b"\n// comment\nfn example() requires true { () }\n");
    f.syntax.anchor = hirsel_span::Anchor(7);
    f.syntax.item.origin.span = Span::new(hirsel_span::Anchor(5), 100, 120).unwrap();
    f.syntax.tree.expressions[0].origin.span = Span::new(hirsel_span::Anchor(9), 0, 100).unwrap();
    assert_eq!(before.fingerprint, f.sig().fingerprint);
}
#[test]
fn hash_changes_with_each_mode() {
    let mut f = Fixture::new();
    let name = Symbol::intern("T");
    f.f().generics.push(ast::GenericParam {
        name,
        kind: ast::GenericKind::Type(vec![]),
    });
    let ty = f.named("T");
    f.param("x", ty, PassingMode::Borrow);
    f.result(ty);
    let s = f.sig();
    let x = ast::Path {
        package: None,
        segments: vec![Symbol::intern("x")],
    };
    for (modifiers, places) in [
        (vec![], Some(vec![ast::RelationPlace::Path(x)])),
        (vec![ast::TypeModifier::Unique], None),
        (vec![ast::TypeModifier::Sendable], None),
    ] {
        let ty = f.qualified(ty, modifiers, None, places);
        f.result(ty);
        assert_ne!(s.fingerprint, f.sig().fingerprint);
    }
    let mut callable = f.f().clone();
    callable.generics.clear();
    let callable = f.ty(ast::TypeExprKind::Function(callable));
    f.result(callable);
    let many = f.sig();
    let once = f.qualified(callable, vec![ast::TypeModifier::Once], None, None);
    f.result(once);
    assert_ne!(many.fingerprint, f.sig().fingerprint);
}

#[test]
fn hash_changes_with_one_bound() {
    let mut f = Fixture::new();
    let name = Symbol::intern("T");
    f.f().generics.push(ast::GenericParam {
        name,
        kind: ast::GenericKind::Type(vec![]),
    });
    let before = f.sig();
    f.f().generics[0].kind = ast::GenericKind::Type(vec![Bound::Copyable]);
    assert_ne!(before.fingerprint, f.sig().fingerprint);
}
#[test]
fn hash_changes_with_capability_parameter() {
    let mut f = Fixture::new();
    let before = f.sig();
    f.dependency("Io", Some(CapabilityKind::Global), true);
    let t = f.named("Io");
    f.param("io", t, PassingMode::Borrow);
    assert_ne!(before.fingerprint, f.sig().fingerprint);
}
#[test]
fn hash_changes_with_contract_code_and_kind() {
    let mut f = Fixture::new();
    let before = f.sig();
    let c = f.expression(ast::ExprKind::Literal(ast::Literal::Bool(true)));
    f.f().contracts.push(ast::ContractSyntax {
        kind: ast::ContractKind::Requires,
        expression: c,
    });
    let a = f.sig();
    assert_ne!(before.fingerprint, a.fingerprint);
    f.syntax.tree.expressions[0].kind = ast::ExprKind::Literal(ast::Literal::Bool(false));
    let b = f.sig();
    assert_ne!(a.fingerprint, b.fingerprint);
    f.f().contracts[0].kind = ast::ContractKind::Ensures;
    assert_ne!(b.fingerprint, f.sig().fingerprint);
}
#[test]
fn hash_changes_with_noalloc() {
    let mut f = Fixture::new();
    let before = f.sig();
    f.attr("noalloc");
    assert_ne!(before.fingerprint, f.sig().fingerprint);
}
#[test]
fn hash_changes_with_used_signature() {
    let mut f = Fixture::new();
    let d = f.dependency("Visible", None, true);
    let ty = f.named("Visible");
    f.result(ty);
    let before = f.sig();
    f.provider.signatures.get_mut(&d).unwrap().fingerprint =
        ast::ContentHash::of_bytes(b"changed signature");
    assert_ne!(before.fingerprint, f.sig().fingerprint);
}
#[test]
fn contract_dependencies_are_resolved_and_hashed() {
    let mut f = Fixture::new();
    let d = f.dependency("predicate", None, true);
    let symbol = Symbol::intern("predicate");
    let callee = f.expression(ast::ExprKind::Path(ast::Path {
        package: None,
        segments: vec![symbol],
    }));
    let call = f.expression(ast::ExprKind::Call {
        callee,
        arguments: vec![],
    });
    f.f().contracts.push(ast::ContractSyntax {
        kind: ast::ContractKind::Requires,
        expression: call,
    });
    let a = f.run().unwrap();
    assert_eq!(a.dependencies.len(), 1);
    assert_eq!(*a.dependencies[0].definition.path(), d);
    let hash = function(&a.value).contracts[0].code_hash;
    f.provider.signatures.get_mut(&d).unwrap().fingerprint =
        ast::ContentHash::of_bytes(b"different");
    let b = f.sig();
    assert_ne!(hash, function(&b).contracts[0].code_hash);
    assert_ne!(a.value.fingerprint, b.fingerprint);
}
#[test]
fn contract_code_hashes_nested_blocks_patterns_and_bound_variables() {
    let mut f = Fixture::new();
    let symbol = Symbol::intern("x");
    let bind = ast::PatternId(0);
    f.syntax.tree.patterns.push(ast::Pattern {
        origin: origin(),
        kind: ast::PatternKind::Binding(symbol),
    });
    let value = f.expression(ast::ExprKind::Literal(ast::Literal::Bool(true)));
    f.syntax.tree.statements.push(ast::Stmt {
        origin: origin(),
        kind: ast::StmtKind::Let {
            mutable: false,
            pattern: bind,
            ty: None,
            value,
            otherwise: None,
        },
    });
    let tail = f.expression(ast::ExprKind::Path(ast::Path {
        package: None,
        segments: vec![symbol],
    }));
    let body = f.expression(ast::ExprKind::Block {
        label: None,
        statements: vec![ast::StmtId(0)],
        tail: Some(tail),
    });
    f.f().contracts.push(ast::ContractSyntax {
        kind: ast::ContractKind::Requires,
        expression: body,
    });
    let a = f.run().unwrap();
    assert!(a.dependencies.is_empty());
    f.syntax.tree.expressions[value.0 as usize].kind =
        ast::ExprKind::Literal(ast::Literal::Bool(false));
    assert_ne!(a.value.fingerprint, f.sig().fingerprint);
}
#[test]
fn hash_sorts_and_deduplicates_sets_and_dependencies() {
    let mut f = Fixture::new();
    let t = f.named("T");
    let name = Symbol::intern("T");
    f.f().generics.push(ast::GenericParam {
        name,
        kind: ast::GenericKind::Type(vec![Bound::Data, Bound::Copyable]),
    });
    f.result(t);
    let a = f.sig();
    f.f().generics[0].kind =
        ast::GenericKind::Type(vec![Bound::Copyable, Bound::Data, Bound::Data]);
    assert_eq!(a.fingerprint, f.sig().fingerprint);
    let a_ = SignatureDependency {
        definition: hirsel_syntax::DefId::intern(&def("a")),
        fingerprint: ast::ContentHash::of_bytes(b"a"),
    };
    let b = SignatureDependency {
        definition: hirsel_syntax::DefId::intern(&def("b")),
        fingerprint: ast::ContentHash::of_bytes(b"b"),
    };
    assert_eq!(
        fingerprint(session(), &a, &[a_.clone(), b.clone()]).unwrap(),
        fingerprint(session(), &a, &[b, a_.clone(), a_]).unwrap()
    );
}
#[test]
fn hash_rejects_invalid_graphs_and_conflicting_dependencies() {
    let mut s = Fixture::new().sig();
    let absent = TypeId::DANGLING;
    s.kind = SignatureKind::Const {
        ty: absent,
        generics: Default::default(),
    };
    assert_eq!(
        fingerprint(session(), &s, &[]),
        Err(EncodingError::MissingType(absent))
    );
    let mut s = Fixture::new().sig();
    let template = rows(&s)[0].1.clone();
    let cycle = session().intern_graph(1, |ids| {
        vec![Type {
            kind: TypeKind::Box(ids[0]),
            ..template.clone()
        }]
    })[0];
    s.kind = SignatureKind::Const {
        ty: cycle,
        generics: Default::default(),
    };
    assert_eq!(
        fingerprint(session(), &s, &[]),
        Err(EncodingError::StructuralCycle(cycle))
    );
    let recovery = session().intern(Type {
        kind: TypeKind::Error,
        ..template
    });
    s.kind = SignatureKind::Const {
        ty: recovery,
        generics: Default::default(),
    };
    assert_eq!(
        fingerprint(session(), &s, &[]),
        Err(EncodingError::RecoveryType(recovery))
    );
    let s = Fixture::new().sig();
    let d = SignatureDependency {
        definition: hirsel_syntax::DefId::intern(&def("a")),
        fingerprint: ast::ContentHash::of_bytes(b"a"),
    };
    let mut e = d.clone();
    e.fingerprint = ast::ContentHash::of_bytes(b"b");
    assert_eq!(
        fingerprint(session(), &s, &[d, e]),
        Err(EncodingError::ConflictingDependency(def("a")))
    );
}
#[test]
fn unreachable_nodes_and_stored_fingerprint_do_not_affect_hash() {
    let mut s = Fixture::new().sig();
    let h = fingerprint(session(), &s, &[]).unwrap();
    session().intern(Type {
        drop_allocation: hirsel_types::DropAllocation::MayAllocate,
        ..Type::with_kind(TypeKind::Error)
    });
    s.fingerprint = ast::ContentHash::of_bytes(b"discard");
    assert_eq!(h, fingerprint(session(), &s, &[]).unwrap());
}
#[test]
fn ordered_parameters_fields_variants_and_contracts_change_hash() {
    let mut f = Fixture::new();
    let a = f.named("u8");
    let b = f.named("u32");
    f.param("a", a, PassingMode::Borrow);
    f.param("b", b, PassingMode::Borrow);
    let h = f.sig().fingerprint;
    f.f().parameters.reverse();
    assert_ne!(h, f.sig().fingerprint);
    let a = f.field("a", a);
    let b = f.field("b", b);
    f.declaration(
        ast::TypeBody::Record(vec![a.clone(), b.clone()]),
        ast::TypeFlavor::Ordinary,
    );
    let h = f.sig().fingerprint;
    f.declaration(ast::TypeBody::Record(vec![b, a]), ast::TypeFlavor::Ordinary);
    assert_ne!(h, f.sig().fingerprint);
}
fn golden_fixtures() -> Vec<(&'static str, Signature)> {
    let unit = Fixture::new().sig();
    let mut f = Fixture::new();
    let t = f.named("u32");
    f.result(t);
    let u32_ = f.sig();
    let mut f = Fixture::new();
    f.attr("noalloc");
    let noalloc = f.sig();
    let mut f = Fixture::new();
    let t = f.named("u8");
    let field = f.field("n", t);
    f.declaration(
        ast::TypeBody::Record(vec![field]),
        ast::TypeFlavor::Ordinary,
    );
    let record = f.sig();
    let mut f = Fixture::new();
    let a = Symbol::intern("A");
    let b = Symbol::intern("B");
    f.declaration(
        ast::TypeBody::Variants(vec![
            ast::VariantSyntax {
                name: a,
                fields: vec![],
            },
            ast::VariantSyntax {
                name: b,
                fields: vec![],
            },
        ]),
        ast::TypeFlavor::Ordinary,
    );
    let variant = f.sig();
    vec![
        ("unit", unit),
        ("u32-result", u32_),
        ("noalloc", noalloc),
        ("record", record),
        ("variant", variant),
    ]
}
#[test]
fn golden_vectors() {
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        let rows = golden_fixtures()
            .into_iter()
            .map(|(name, s)| {
                format!(
                    "{name} {} {}",
                    hex(&s.fingerprint.0),
                    hex(&canonical_encoding(&s, &[]).unwrap())
                )
            })
            .collect::<Vec<_>>();
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/golden.txt"),
            format!(
                "# Canonical signature encoding v16 and SHA-256 vectors\n{}\n",
                rows.join("\n")
            ),
        )
        .unwrap();
        return;
    }
    for (name, s) in golden_fixtures() {
        let bytes = canonical_encoding(&s, &[]).unwrap();
        let row = format!("{name} {} {}", hex(&s.fingerprint.0), hex(&bytes));
        assert!(
            include_str!("golden.txt").lines().any(|l| l == row),
            "missing or changed golden {name}"
        );
    }
}

#[test]
fn library_role_detection_uses_canonical_paths_instead_of_names() {
    let mut f = Fixture::new();
    let d = f.dependency("Result", None, true);
    if let SignatureKind::Type(t) = &mut f.provider.signatures.get_mut(&d).unwrap().kind {
        t.generics = vec![
            GenericParameter::Type {
                name: "T".into(),
                bounds: vec![].into(),
            },
            GenericParameter::Type {
                name: "E".into(),
                bounds: vec![].into(),
            },
        ];
    }
    let t = f.named("u8");
    let r = f.application(
        "Result",
        vec![ast::TypeArgument::Type(t), ast::TypeArgument::Type(t)],
    );
    f.result(r);
    assert!(function(&f.sig()).error.is_none());
    f.metadata.result_type = Some(d);
    assert!(function(&f.sig()).error.is_some());
}
#[test]
fn hash_preserves_full_named_identity_and_never_uses_resolution_indices() {
    let mut f = Fixture::new();
    let d = f.dependency("Visible", None, true);
    let t = f.named("Visible");
    f.result(t);
    let a = f.sig();
    let mut other = d.clone();
    other.0.package.version = "2".into();
    let mut sig = f.provider.signatures[&d].clone();
    sig.definition = DefId::intern(&other);
    f.provider.signatures.insert(other.clone(), sig);
    f.metadata.definitions.insert(
        other.clone(),
        DefinitionMetadata {
            public: true,
            alias: false,
        },
    );
    f.resolution
        .types
        .push((t, ResolvedName::Definition(other)));
    assert_ne!(a.fingerprint, f.sig().fingerprint);
}
#[test]
fn opaque_private_representation_dependencies_are_not_public_hash_inputs() {
    let mut f = Fixture::new();
    let d = f.dependency("Hidden", None, false);
    let t = f.named("Hidden");
    f.declaration(ast::TypeBody::Alias(t), ast::TypeFlavor::Ordinary);
    f.syntax.item.visibility = ast::Visibility::PublicOpaque;
    if let ast::ItemKind::Type { bounds, .. } = &mut f.syntax.item.kind {
        *bounds = Some(vec![]);
    }
    if let ast::ItemKind::Type { authority, .. } = &mut f.syntax.item.kind {
        *authority = Some(vec![]);
    }
    let a = f.run().unwrap();
    assert!(a.dependencies.is_empty());
    f.provider.signatures.get_mut(&d).unwrap().fingerprint =
        ast::ContentHash::of_bytes(b"private change");
    assert_eq!(a.value.fingerprint, f.sig().fingerprint);
}

#[test]
fn duplicate_module_names_are_rejected_at_the_later_name_across_files() {
    let mut f = Fixture::new();
    let first = Span::new(hirsel_span::Anchor(0), 50, 55).unwrap();
    let second = Span::new(hirsel_span::Anchor(1), 70, 75).unwrap();
    f.syntax.item.name_span = second;
    f.metadata.declarations = vec![
        DeclarationSite {
            definition: def("example"),
            name_span: first,
        },
        DeclarationSite {
            definition: def("example"),
            name_span: second,
        },
    ];
    error(&mut f, "LANG-7", second);
    f.syntax.item.name_span = first;
    f.sig();
}
#[test]
fn public_opaque_authority_cannot_expose_a_private_capability() {
    let mut f = Fixture::new();
    f.dependency("PrivateCap", Some(CapabilityKind::Global), false);
    let symbol = Symbol::intern("PrivateCap");
    f.declaration(ast::TypeBody::Opaque, ast::TypeFlavor::Ordinary);
    f.syntax.item.visibility = ast::Visibility::PublicOpaque;
    if let ast::ItemKind::Type { bounds, .. } = &mut f.syntax.item.kind {
        *bounds = Some(vec![]);
    }
    if let ast::ItemKind::Type { authority, .. } = &mut f.syntax.item.kind {
        *authority = Some(vec![ast::Path {
            package: None,
            segments: vec![symbol],
        }]);
    }
    error(&mut f, "PROG-2", Span::default());
}

fn hash_change(change: impl FnOnce(&mut Signature)) {
    let mut signature = Fixture::new().sig();
    let original = signature.fingerprint;
    change(&mut signature);
    assert_ne!(original, fingerprint(session(), &signature, &[]).unwrap());
}
macro_rules! hash_component {
    ($name:ident, $change:expr) => {
        #[test]
        fn $name() {
            hash_change($change);
        }
    };
}
hash_component!(hash_changes_with_visibility, |s| s.metadata.visibility =
    ast::Visibility::Public);
hash_component!(hash_changes_with_alias_metadata, |s| s.metadata.alias =
    true);
hash_component!(hash_changes_with_unsafe_ffi_metadata, |s| s
    .metadata
    .unsafe_ffi =
    true);
hash_component!(hash_changes_with_wrapper_metadata, |s| s.metadata.wrapper =
    Some(ForeignWrapper {
        kind: WrapperKind::Pure,
        deterministic: true,
    }));
hash_component!(hash_changes_with_unsafe_nominal_role, |s| {
    std::sync::Arc::make_mut(&mut s.metadata.roles).unsafe_capability = Some(def("Unsafe"))
});
hash_component!(hash_changes_with_result_nominal_role, |s| {
    std::sync::Arc::make_mut(&mut s.metadata.roles).result_type = Some(def("Result"))
});
hash_component!(hash_changes_with_drop_allocation, |s| {
    let old = function(s).result;
    let ty = session().intern(Type {
        drop_allocation: DropAllocation::MayAllocate,
        ..session().row(old).clone()
    });
    if let SignatureKind::Function(f) = &mut s.kind {
        f.result = ty;
    }
});
hash_component!(hash_changes_with_generic_drop_obligations, |s| {
    let old = function(s).result;
    let ty = session().intern(Type {
        drop_allocation: DropAllocation::DependsOn(
            vec![TypeParameterRef::bound(TypeParamId(0))].into(),
        ),
        ..session().row(old).clone()
    });
    if let SignatureKind::Function(f) = &mut s.kind {
        f.result = ty;
    }
});

#[test]
fn hash_changes_with_growable_field_metadata() {
    let mut f = Fixture::new();
    let ty = f.named("u32");
    let field = f.field("value", ty);
    f.declaration(
        ast::TypeBody::Record(vec![field]),
        ast::TypeFlavor::Ordinary,
    );
    let original = f.sig().fingerprint;
    if let ast::ItemKind::Type {
        body: ast::TypeBody::Record(fs),
        ..
    } = &mut f.syntax.item.kind
    {
        fs[0].growable = true;
    }
    let changed = f.sig();
    assert_ne!(original, changed.fingerprint);
    let TypeKind::Record(fs) = &session()
        .get(typedef(&changed).representation.unwrap())
        .unwrap()
        .kind
    else {
        panic!()
    };
    assert!(fs[0].growable);
}

#[test]
fn hash_changes_with_destructor_allocation_promise() {
    let mut f = Fixture::new();
    f.declaration(ast::TypeBody::Record(vec![]), ast::TypeFlavor::Resource);
    let mut signature = f.sig();
    let original = signature.fingerprint;
    if let SignatureKind::Type(t) = &mut signature.kind {
        t.destructor_allocation = Allocation::NoAlloc;
    }
    assert_ne!(original, fingerprint(session(), &signature, &[]).unwrap());
}

#[test]
fn hash_changes_with_noalloc_drop_bound() {
    let mut f = Fixture::new();
    f.declaration(ast::TypeBody::Opaque, ast::TypeFlavor::Ordinary);
    f.syntax.item.visibility = ast::Visibility::PublicOpaque;
    if let ast::ItemKind::Type {
        bounds, authority, ..
    } = &mut f.syntax.item.kind
    {
        *bounds = Some(vec![]);
        *authority = Some(vec![]);
    }
    let original = f.sig().fingerprint;
    if let ast::ItemKind::Type { bounds, .. } = &mut f.syntax.item.kind {
        *bounds = Some(vec![Bound::NoallocDrop]);
    }
    let signature = f.sig();
    assert_ne!(original, signature.fingerprint);
    assert!(
        canonical_encoding(&signature, &[])
            .unwrap()
            .windows(29)
            .any(|w| w == b"hirsel.signature.encoding.v16")
    );
}

#[test]
fn hash_changes_with_declared_resource_authority() {
    let mut f = Fixture::new();
    f.dependency("Dir", Some(CapabilityKind::Global), true);
    f.declaration(ast::TypeBody::Record(vec![]), ast::TypeFlavor::Resource);
    let original = f.sig().fingerprint;
    let path = ast::Path {
        package: None,
        segments: vec![Symbol::intern("Dir")],
    };
    if let ast::ItemKind::Type { authority, .. } = &mut f.syntax.item.kind {
        *authority = Some(vec![path]);
    }
    assert_ne!(original, f.sig().fingerprint);
}

fn result_drop(f: &mut Fixture) -> DropAllocation {
    let signature = f.sig();
    session()
        .get(function(&signature).result)
        .unwrap()
        .drop_allocation
        .clone()
}

#[test]
fn generic_cleanup_conjunctions_keep_mixed_ordinals_and_bound_proofs() {
    let mut f = Fixture::new();
    let t = f.named("T");
    let u = f.named("U");
    let list = f.application("List", vec![ast::TypeArgument::Type(t)]);
    let tuple = f.ty(ast::TypeExprKind::Tuple(vec![u, list, t]));
    let ts = Symbol::intern("T");
    let ns = Symbol::intern("n");
    let us = Symbol::intern("U");
    f.f().generics = vec![
        ast::GenericParam {
            name: ts,
            kind: ast::GenericKind::Type(vec![]),
        },
        ast::GenericParam {
            name: ns,
            kind: ast::GenericKind::Size,
        },
        ast::GenericParam {
            name: us,
            kind: ast::GenericKind::Type(vec![]),
        },
    ];
    f.result(tuple);
    assert_eq!(
        result_drop(&mut f),
        DropAllocation::DependsOn(
            vec![
                TypeParameterRef::bound(TypeParamId(0)),
                TypeParameterRef::bound(TypeParamId(2))
            ]
            .into()
        )
    );
    f.f().generics[0].kind = ast::GenericKind::Type(vec![Bound::NoallocDrop]);
    assert_eq!(
        result_drop(&mut f),
        DropAllocation::DependsOn(vec![TypeParameterRef::bound(TypeParamId(2))].into())
    );
    f.f().generics[2].kind = ast::GenericKind::Type(vec![Bound::NoallocDrop]);
    assert_eq!(result_drop(&mut f), DropAllocation::NoAlloc);
}

#[test]
fn ordinary_opaque_self_references_keep_the_exported_plain_cleanup_proof() {
    let mut f = Fixture::new();
    let own = f.named("example");
    let pointer = f.application("Ptr", vec![ast::TypeArgument::Type(own)]);
    let field = f.field("back", pointer);
    f.declaration(
        ast::TypeBody::Record(vec![field]),
        ast::TypeFlavor::Ordinary,
    );
    f.syntax.item.visibility = ast::Visibility::PublicOpaque;
    if let ast::ItemKind::Type {
        bounds, authority, ..
    } = &mut f.syntax.item.kind
    {
        *bounds = Some(vec![Bound::Plain, Bound::Inert]);
        *authority = Some(vec![]);
    }
    let output = normalize_owned(
        Input {
            types: session(),
            memo: memo(),
            syntax: &f.syntax,
            context: &f.context,
            resolution: &f.resolution,
            metadata: &f.metadata,
            declaration_sites: &[],
            stage: None,
        },
        &mut f.provider,
    )
    .unwrap();
    let owner = output.owner.unwrap();
    let roots: Vec<_> = owner.declaration.representation.into_iter().collect();
    let own = closure(session(), roots).iter().map(|t| session().row(*t).clone()).find(|t| {
        matches!(&t.kind, TypeKind::Named { definition, .. } if *definition.path() == def("example"))
    }).expect("the phantom pointer argument retains the self reference");
    assert_eq!(own.drop_allocation, DropAllocation::NoAlloc);
}

hash_component!(hash_changes_with_foreign_contract, |s| {
    if let SignatureKind::Function(f) = &mut s.kind {
        f.foreign = Some(ForeignContract {
            abi: "c".into(),
            blocking: false,
            aliasing_contract: "no memory access".into(),
        });
    }
});

#[test]
fn recursive_nominal_cleanup_includes_the_nonrecursive_owned_leaves() {
    let mut f = Fixture::new();
    let next = f.named("example");
    let next = f.ty(ast::TypeExprKind::Box(next));
    let data = f.named("u32");
    let next = f.field("next", next);
    let data = f.field("data", data);
    f.declaration(
        ast::TypeBody::Record(vec![next, data]),
        ast::TypeFlavor::Ordinary,
    );
    let signature = f.sig();
    assert!(typedef(&signature).bounds.contains(&Bound::NoallocDrop));
    assert!(
        rows(&signature)
            .iter()
            .all(|(_, t)| t.drop_allocation == DropAllocation::NoAlloc)
    );
    let mut inner = Fixture::new();
    inner.syntax.item.definition = def("Allocating");
    inner.declaration(ast::TypeBody::Record(vec![]), ast::TypeFlavor::Resource);
    f.provider.signatures.insert(def("Allocating"), inner.sig());
    let resource = f.named("Allocating");
    let resource = f.field("resource", resource);
    if let ast::ItemKind::Type {
        body: ast::TypeBody::Record(fs),
        ..
    } = &mut f.syntax.item.kind
    {
        fs.push(resource);
    }
    let signature = f.sig();
    assert!(!typedef(&signature).bounds.contains(&Bound::NoallocDrop));
    let recursive = rows(&signature).into_iter().find(|(_, t)| matches!(t.kind, TypeKind::Named { ref definition, .. } if *definition.path() == def("example"))).unwrap().1;
    assert_eq!(recursive.drop_allocation, DropAllocation::MayAllocate);
}

#[test]
fn native_constructor_authority_comes_from_the_resolver_selected_declaration() {
    let mut f = Fixture::new();
    let tasks = f.dependency("GoverningTasks", Some(CapabilityKind::Global), true);
    let native = f.dependency("Sender", None, true);
    let types = session();
    let payload = types.intern(Type {
        drop_allocation: DropAllocation::DependsOn(
            vec![TypeParameterRef::bound(TypeParamId(0))].into(),
        ),
        ..Type::with_kind(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))))
    });
    let repr = types.intern(Type {
        drop_allocation: DropAllocation::DependsOn(
            vec![TypeParameterRef::bound(TypeParamId(0))].into(),
        ),
        ..Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::Sender,
            arguments: vec![GenericArgument::Type(payload)].into(),
        })
    });
    let signature = f.provider.signatures.get_mut(&native).unwrap();
    if let SignatureKind::Type(t) = &mut signature.kind {
        t.representation = Some(repr);
        t.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        }];
        t.authority = AuthoritySummary::Known(
            vec![
                AuthorityTerm::Capability(tasks.clone()),
                AuthorityTerm::TypeParameter(TypeParameterRef::bound(TypeParamId(0))),
            ]
            .into(),
        );
    }
    let payload = f.named("u32");
    let sender = f.application("Sender", vec![ast::TypeArgument::Type(payload)]);
    f.resolution
        .types
        .push((sender, ResolvedName::Definition(native.clone())));
    let sender = f.qualified(sender, vec![ast::TypeModifier::Unique], None, None);
    let field = f.field("sender", sender);
    f.declaration(
        ast::TypeBody::Record(vec![field]),
        ast::TypeFlavor::Resource,
    );
    let output = f.run().unwrap();
    assert_eq!(
        typedef(&output.value).authority,
        AuthoritySummary::Known(vec![AuthorityTerm::Capability(tasks)].into())
    );
    assert!(!typedef(&output.value).bounds.contains(&Bound::Inert));
    assert!(
        output
            .dependencies
            .iter()
            .any(|d| d.definition.path() == &native)
    );
}

#[test]
fn native_authority_proofs_apply_only_to_the_resolved_type_occurrence() {
    let mut f = Fixture::new();
    // Builtin identity comes from the core@1 declaration, not the spelling.
    let local = f.dependency("Sender", None, true);
    let mut native = local.clone();
    native.0.package = ast::PackageId {
        name: "core".into(),
        version: "1".into(),
    };
    let mut signature = f.provider.signatures.remove(&local).unwrap();
    signature.definition = DefId::intern(&native);
    f.provider.signatures.insert(native.clone(), signature);
    let metadata = f.metadata.definitions.remove(&local).unwrap();
    f.metadata.definitions.insert(native.clone(), metadata);
    let payload = f.named("u32");
    let registered = f.application("Sender", vec![ast::TypeArgument::Type(payload)]);
    let fallback = f.application("Sender", vec![ast::TypeArgument::Type(payload)]);
    f.resolution
        .types
        .push((registered, ResolvedName::Definition(native)));
    let registered = f.field("registered", registered);
    let fallback = f.field("fallback", fallback);
    f.declaration(
        ast::TypeBody::Record(vec![registered, fallback]),
        ast::TypeFlavor::Resource,
    );
    let output = f.run().unwrap();
    assert_eq!(typedef(&output.value).authority, AuthoritySummary::Unknown);
    assert!(!typedef(&output.value).bounds.contains(&Bound::Inert));
}

#[test]
fn taken_resource_property_proofs_read_transitive_nominal_declarations() {
    for branded in [false, true] {
        let mut f = Fixture::new();
        f.dependency("Dir", Some(CapabilityKind::Global), true);
        let dir = f.named("Dir");
        f.param("dir", dir, PassingMode::Borrow);
        let payload = f.dependency("Payload", None, true);
        if let SignatureKind::Type(t) = &mut f.provider.signatures.get_mut(&payload).unwrap().kind {
            t.bounds = vec![Bound::Plain, Bound::NoallocDrop];
        }
        let resource = f.dependency("Resource", None, true);
        let signature = f.provider.signatures.get_mut(&resource).unwrap();
        let representation = session().intern(Type::with_kind(TypeKind::Named {
            definition: DefId::intern(&payload),
            arguments: vec![].into(),
        }));
        if let SignatureKind::Type(t) = &mut signature.kind {
            t.representation = Some(representation);
            t.resource = true;
            t.destructor_allocation = Allocation::NoAlloc;
            t.bounds = vec![Bound::NoallocDrop];
        }
        let base = f.named("Resource");
        let base = if branded {
            let dir = Symbol::intern("dir");
            f.qualified(
                base,
                vec![],
                Some(ast::BrandSyntax {
                    sibling: false,
                    path: ast::Path {
                        package: None,
                        segments: vec![dir],
                    },
                }),
                None,
            )
        } else {
            base
        };
        f.param("resource", base, PassingMode::Take);
        let out = f.run().unwrap();
        let row = session()
            .get(function(&out.value).parameters[1].ty)
            .unwrap();
        assert_eq!(row.modes.relation, Relation::global());
        assert_eq!(row.modes.sendability, Sendability::Sendable);
        assert!(
            out.dependencies
                .iter()
                .any(|d| d.definition.path() == &payload)
        );
        assert!(f.provider.reads.contains(&payload));
    }
}

#[test]
fn parameter_projection_contracts_hash_fields_without_definition_queries() {
    let mut hashes = vec![];
    for field_name in ["low", "high"] {
        let mut f = Fixture::new();
        let base = f.named("u8");
        let field = f.field("v", base);
        let v = Symbol::intern("v");
        let member = Symbol::intern(field_name);
        let projection = f.expression(ast::ExprKind::Path(ast::Path {
            package: None,
            segments: vec![v, member],
        }));
        let fact = f.expression(ast::ExprKind::Binary {
            op: ast::BinaryOp::Eq,
            left: projection,
            right: projection,
        });
        f.declaration(
            ast::TypeBody::Distinct {
                field,
                fact: Some(fact),
            },
            ast::TypeFlavor::Distinct,
        );
        let out = f.run().unwrap();
        hashes.push(typedef(&out.value).fact.as_ref().unwrap().code_hash);
        assert!(f.provider.reads.is_empty());
    }
    assert_ne!(hashes[0], hashes[1]);
}

#[test]
fn nested_authority_names_its_lexical_binder_and_shadowing_is_local() {
    for shadow in [false, true] {
        let mut f = Fixture::new();
        f.dependency("Io", Some(CapabilityKind::Global), true);
        let io = f.named("Io");
        f.param("outer", io, PassingMode::Borrow);
        let name = Symbol::intern(if shadow { "outer" } else { "inner" });
        let outer = Symbol::intern("outer");
        let callback = f.ty(ast::TypeExprKind::Function(ast::FunctionSyntax {
            parameters: vec![ast::Parameter {
                name: Some(name),
                label: None,
                ty: io,
                passing: PassingMode::Borrow,
            }],
            uses: Some(vec![ast::Path {
                package: None,
                segments: vec![outer],
            }]),
            ..Default::default()
        }));
        f.param("action", callback, PassingMode::Borrow);
        let signature = f.sig();
        let TypeKind::Function(callback) = &session()
            .get(function(&signature).parameters[1].ty)
            .unwrap()
            .kind
        else {
            panic!()
        };
        let authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(ParameterRef::Bound {
                depth: u32::from(!shadow),
                parameter: ParamId(0),
            })]
            .into(),
        );
        assert_eq!(callback.authority, authority);
        assert_eq!(callback.uses, Uses::Only(authority));
        assert!(canonical_encoding(&signature, &[]).is_ok());
    }
}

#[test]
fn signature_encoding_rejects_body_parameter_references() {
    let mut signature = Fixture::new().sig();
    let SignatureKind::Function(function) = &mut signature.kind else {
        panic!()
    };
    let reference = ParameterRef::body(
        LexicalOwner::first(&def("example")).site(NameSite::Item),
        ParamId(0),
    );
    function.authority = AuthoritySummary::Known(vec![AuthorityTerm::Parameter(reference)].into());
    assert_eq!(
        canonical_encoding(&signature, &[]),
        Err(EncodingError::BodyParameter(reference))
    );
}

#[test]
fn builtin_handle_authority_follows_the_supplied_nominal_roles() {
    let mut f = Fixture::new();
    let scheduler = f.dependency("Scheduler", Some(CapabilityKind::Global), true);
    f.metadata.tasks_capability = Some(scheduler.clone());
    let scalar = f.named("u32");
    let sender = f.application("Sender", vec![ast::TypeArgument::Type(scalar)]);
    let field = f.field("sender", sender);
    f.declaration(
        ast::TypeBody::Record(vec![field]),
        ast::TypeFlavor::Resource,
    );
    let signature = f.sig();
    assert_eq!(
        typedef(&signature).authority,
        AuthoritySummary::Known(vec![AuthorityTerm::Capability(scheduler)].into())
    );
    f.metadata.tasks_capability = None;
    assert_eq!(typedef(&f.sig()).authority, AuthoritySummary::Unknown);
}

impl Fixture {
    fn held_authority(&mut self, root: TypeId) -> AuthoritySummary {
        let input = Input {
            types: session(),
            memo: memo(),
            syntax: &self.syntax,
            context: &self.context,
            resolution: &self.resolution,
            metadata: &self.metadata,
            declaration_sites: &self.metadata.declarations,
            stage: None,
        };
        let snapshots = self
            .provider
            .signatures
            .iter()
            .map(|(p, s)| (DefId::intern(p), std::sync::Arc::new(s.clone())))
            .collect();
        let n = Normalizer {
            proof_failure: &std::cell::RefCell::new(None),
            input: &input,
            provider: &mut self.provider,
            types: session(),
            memo: memo(),
            own_representation: None,
            generics: vec![],
            parameters: vec![],
            parameter_syntax: vec![],
            outer_parameters: vec![],
            outer_generics: vec![],
            siblings: vec![],
            dependencies: DefMap::default(),
            snapshots,
            group: BTreeMap::new(),
            group_key: None,
            group_drop: BTreeMap::new(),
            builtin_declarations: Default::default(),
            active: BTreeSet::new(),
            aliases: BTreeSet::new(),
            public: false,
            span: Span::default(),
            arena_places: BTreeSet::new(),
            current_parameter: None,
        };
        n.type_authority(Some(root))
    }
}

#[test]
fn recovery_rows_cannot_prove_empty_held_authority() {
    for recovery in [false, true] {
        for nested in [false, true] {
            let mut f = Fixture::new();
            let types = session();
            let row = Type::with_kind(if recovery {
                TypeKind::Error
            } else {
                TypeKind::Primitive(Primitive::U32)
            });
            let mut root = types.intern(row.clone());
            if nested {
                let mut row = row;
                row.kind = TypeKind::Box(root);
                root = types.intern(row);
            }
            assert_eq!(
                f.held_authority(root),
                if recovery {
                    AuthoritySummary::Unknown
                } else {
                    AuthoritySummary::inert()
                }
            );
        }
    }
}

#[test]
fn typed_ambient_heap_has_empty_authority_without_erasing_arena_authority() {
    for heap in [false, true] {
        let mut f = Fixture::new();
        let arena = f.dependency("Arena", Some(CapabilityKind::Scoped), true);
        f.metadata.heap = Some(HeapRole {
            arena: arena.clone(),
            operation: def("heap"),
        });
        let root = session().intern(Type {
            allocation_target: heap.then_some(AllocationTarget::Heap),
            ..Type::with_kind(TypeKind::Named {
                definition: DefId::intern(&arena),
                arguments: vec![].into(),
            })
        });
        assert_eq!(
            f.held_authority(root),
            if heap {
                AuthoritySummary::inert()
            } else {
                AuthoritySummary::Known(vec![AuthorityTerm::Capability(arena)].into())
            }
        );
    }
}

#[test]
fn instantiation_bound_schema_fingerprints_and_scoped_references() {
    let mut signature = Fixture::new().sig();
    let SignatureKind::Function(function) = &mut signature.kind else {
        unreachable!()
    };
    function.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    });
    let constraint = |parameter, bound| hirsel_types::SchemaConstraint::Bound {
        parameter,
        bound,
        rule: const { RuleId::of("GEN-1") },
    };
    signature.metadata.schema = vec![constraint(
        TypeParameterRef::bound(TypeParamId(0)),
        Bound::Copyable,
    )];
    let before = canonical_encoding(&signature, &[]).unwrap();
    signature.metadata.schema = vec![constraint(
        TypeParameterRef::bound(TypeParamId(0)),
        Bound::Plain,
    )];
    assert_ne!(before, canonical_encoding(&signature, &[]).unwrap());
    for parameter in [
        TypeParameterRef::Body {
            scope: LexicalOwner::first(&def("example")).site(NameSite::Item),
            parameter: TypeParamId(0),
        },
        TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        },
        TypeParameterRef::bound(TypeParamId(99)),
    ] {
        signature.metadata.schema = vec![constraint(parameter, Bound::Copyable)];
        assert!(canonical_encoding(&signature, &[]).is_err());
    }
}

#[test]
fn signature_recovery_22_provider_failures_keep_their_disposition() {
    struct Failing(QueryError);
    impl SignatureProvider for Failing {
        fn signature(&mut self, _: DefId) -> Result<SignatureRef, QueryError> {
            Err(self.0.clone())
        }
        fn lookup(&mut self, c: &ast::FileContext, p: &ast::Path) -> LookupOutput {
            Fake::default().lookup(c, p)
        }
    }
    let span = Span::new(hirsel_span::Anchor(22), 3, 9).unwrap();
    for failure in [
        QueryError::NotImplemented,
        QueryError::Operational(OperationalFailure {
            operation: "signature fixture".into(),
            message: "provider unavailable".into(),
        }),
        diagnostic(const { RuleId::of("LANG-4") }, span, "missing dependency"),
    ] {
        for nested in [false, true] {
            let mut f = Fixture::new();
            let root = f.named("Dependency");
            let root = if nested {
                f.ty(ast::TypeExprKind::Tuple(vec![root]))
            } else {
                root
            };
            f.result(root);
            let result = normalize(
                Input {
                    types: session(),
                    memo: memo(),
                    syntax: &f.syntax,
                    context: &f.context,
                    resolution: &f.resolution,
                    metadata: &f.metadata,
                    declaration_sites: &[],
                    stage: None,
                },
                &mut Failing(failure.clone()),
            );
            assert_eq!(result.unwrap_err(), failure);
            // A repaired dependency has a normal signature, in the same session.
            f.dependency("Dependency", None, true);
            assert!(f.run().unwrap().diagnostics.is_empty());
        }
    }
}

#[test]
fn signature_recovery_22_dependency_recovery_rows_never_publish_proofs() {
    for nested in [false, true] {
        for recovery in [true, false] {
            let mut f = Fixture::new();
            let dependency = f.dependency("Alias", None, true);
            let root = session().intern(Type::with_kind(if recovery {
                TypeKind::Error
            } else {
                TypeKind::Primitive(Primitive::U8)
            }));
            let root = if nested {
                session().intern(Type::with_kind(TypeKind::Tuple(vec![root].into())))
            } else {
                root
            };
            let signature = f.provider.signatures.get_mut(&dependency).unwrap();
            signature.metadata.alias = true;
            let SignatureKind::Type(declaration) = &mut signature.kind else {
                unreachable!()
            };
            declaration.representation = Some(root);
            // A stale claimed bound and empty authority cannot bless a recovery row.
            declaration.bounds = vec![Bound::Plain, Bound::Inert];
            let root = f.named("Alias");
            f.declaration(ast::TypeBody::Alias(root), ast::TypeFlavor::Ordinary);
            match f.run() {
                Err(QueryError::Diagnostics(errors)) if recovery => {
                    assert!(!errors.is_empty());
                    assert_eq!(errors[0].rule.as_str(), "LANG-6");
                }
                Ok(output) if !recovery => {
                    assert!(output.diagnostics.is_empty());
                    assert!(typedef(&output.value).bounds.contains(&Bound::Plain));
                    assert_eq!(typedef(&output.value).authority, AuthoritySummary::inert());
                }
                result => panic!("nested={nested}, recovery={recovery}: {result:?}"),
            }
        }
    }
}
