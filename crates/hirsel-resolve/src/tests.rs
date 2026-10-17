use super::*;
use hirsel_intern::{Interner, Symbol};
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::*;
use hirsel_types::{self as ty, QueryError, ResolvedName, SignatureProvider};
use std::collections::BTreeMap;

fn package(name: &str, version: &str) -> PackageId {
    PackageId {
        name: name.into(),
        version: version.into(),
    }
}
fn definition(pkg: PackageId, modules: &[&str], name: &str) -> DefPath {
    DefPath(ItemPath {
        package: pkg,
        modules: modules.iter().map(|s| (*s).into()).collect(),
        name: name.into(),
    })
}
fn own(name: &str) -> DefPath {
    definition(package("test", "1"), &[], name)
}
fn entry(d: DefPath, namespace: Namespace, visibility: Visibility) -> DefinitionEntry {
    DefinitionEntry {
        definition: d,
        namespace,
        visibility,
        span: Span::default(),
    }
}
fn fixture(entries: Vec<DefinitionEntry>) -> ManifestFixture {
    ManifestFixture {
        package: package("test", "1"),
        definitions: entries,
        exposures: vec![],
        revision: ContentHash::of_bytes(b"manifest-v1"),
    }
}

struct Fake {
    signatures: BTreeMap<DefPath, ty::Signature>,
    lookup_index: ManifestIndex,
    reads: Vec<DefPath>,
    lookups: usize,
}
impl Fake {
    fn new(index: &ManifestIndex) -> Self {
        Self {
            signatures: BTreeMap::new(),
            lookup_index: index.clone(),
            reads: vec![],
            lookups: 0,
        }
    }
}
impl SignatureProvider for Fake {
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &Path,
        symbols: &Interner,
    ) -> hirsel_types::LookupOutput {
        hirsel_types::LookupOutput {
            manifest: Default::default(),
            result: (|| {
                self.lookups += 1;
                self.lookup_index
                    .lookup(context, path, symbols)
                    .map_err(|_| QueryError::NotImplemented)
            })(),
        }
    }
    fn signature(&mut self, definition: &DefPath) -> Result<ty::SignatureRef, QueryError> {
        (|| {
            self.reads.push(definition.clone());
            self.signatures
                .get(definition)
                .cloned()
                .ok_or(QueryError::NotImplemented)
        })()
        .map(std::sync::Arc::new)
    }
}

struct Tree {
    file: SyntaxFile,
    symbols: Interner,
    source: String,
}
impl Tree {
    fn new(source: &str) -> Self {
        let file = SourceMap::default().add("test.hir", source.into()).unwrap();
        Self {
            file: SyntaxFile {
                has_syntax_errors: false,
                symbols: Default::default(),

                file,
                tokens: vec![],
                tree: SyntaxTree::default(),
                roots: vec![],
            },
            symbols: Interner::default(),
            source: source.into(),
        }
    }
    fn origin(&self, line: u32) -> Origin {
        let start = self
            .source
            .split_inclusive('\n')
            .take(line as usize - 1)
            .map(str::len)
            .sum::<usize>() as u32;
        let end = start
            + self
                .source
                .lines()
                .nth(line as usize - 1)
                .unwrap_or("")
                .len() as u32;
        Origin {
            span: Span::new(self.file.file, start, end).unwrap(),
            tokens: TokenRange::default(),
        }
    }
    fn name(&mut self, name: &str) -> Symbol {
        self.symbols.intern(name)
    }
    fn path(&mut self, name: &str) -> Path {
        Path {
            package: None,
            segments: name.split('.').map(|s| self.name(s)).collect(),
        }
    }
    fn expr(&mut self, kind: ExprKind, line: u32) -> ExprId {
        let id = ExprId(self.file.tree.expressions.len() as u32);
        self.file.tree.expressions.push(Expr {
            kind,
            origin: self.origin(line),
        });
        id
    }
    fn value(&mut self, name: &str, line: u32) -> ExprId {
        let path = self.path(name);
        self.expr(ExprKind::Path(path), line)
    }
    fn number(&mut self, number: &str, line: u32) -> ExprId {
        self.expr(ExprKind::Literal(Literal::Integer(number.into())), line)
    }
    fn block(&mut self, statements: Vec<StmtId>, tail: Option<ExprId>, line: u32) -> ExprId {
        self.expr(ExprKind::Block { statements, tail }, line)
    }
    fn call(&mut self, name: &str, args: Vec<ExprId>, line: u32) -> ExprId {
        let callee = self.value(name, line);
        self.expr(
            ExprKind::Call {
                callee,
                arguments: args
                    .into_iter()
                    .map(|value| Argument { label: None, value })
                    .collect(),
            },
            line,
        )
    }
    fn ty(&mut self, kind: TypeExprKind, line: u32) -> TypeExprId {
        let id = TypeExprId(self.file.tree.types.len() as u32);
        self.file.tree.types.push(TypeExpr {
            kind,
            origin: self.origin(line),
        });
        id
    }
    fn named_ty(&mut self, name: &str, line: u32) -> TypeExprId {
        let path = self.path(name);
        self.ty(
            TypeExprKind::Named {
                path,
                arguments: vec![],
            },
            line,
        )
    }
    fn parameter(&mut self, name: &str, ty: TypeExprId) -> Parameter {
        Parameter {
            name: Some(self.name(name)),
            label: None,
            passing: PassingMode::Borrow,
            ty,
        }
    }
    fn pattern(&mut self, kind: PatternKind, line: u32) -> PatternId {
        let id = PatternId(self.file.tree.patterns.len() as u32);
        self.file.tree.patterns.push(Pattern {
            kind,
            origin: self.origin(line),
        });
        id
    }
    fn binding(&mut self, name: &str, line: u32) -> PatternId {
        let name = self.name(name);
        self.pattern(PatternKind::Binding(name), line)
    }
    fn stmt(&mut self, kind: StmtKind, line: u32) -> StmtId {
        let id = StmtId(self.file.tree.statements.len() as u32);
        self.file.tree.statements.push(Stmt {
            kind,
            origin: self.origin(line),
        });
        id
    }
    fn let_(&mut self, name: &str, ty: Option<TypeExprId>, value: ExprId, line: u32) -> StmtId {
        let pattern = self.binding(name, line);
        self.stmt(
            StmtKind::Let {
                mutable: false,
                pattern,
                ty,
                value,
            },
            line,
        )
    }
    fn item(&mut self, name: &str, kind: ItemKind, visibility: Visibility, line: u32) -> ItemId {
        let id = ItemId(self.file.tree.items.len() as u32);
        let symbol = self.name(name);
        self.file.tree.items.push(Item {
            name_span: self.origin(line).span,

            name: symbol,
            kind,
            visibility,
            definition: own(name),
            origin: self.origin(line),
            owned_tokens: TokenRange::default(),
            attributes: vec![],
            text_hash: ContentHash::of_bytes(name.as_bytes()),
        });
        self.file.roots.push(id);
        id
    }
    fn function(
        &mut self,
        name: &str,
        signature: FunctionSyntax,
        body: ExprId,
        visibility: Visibility,
        line: u32,
    ) -> ItemId {
        self.item(
            name,
            ItemKind::Function {
                signature,
                body: Some(body),
                abi: None,
            },
            visibility,
            line,
        )
    }
    fn type_item(
        &mut self,
        name: &str,
        body: TypeBody,
        visibility: Visibility,
        line: u32,
    ) -> ItemId {
        self.item(
            name,
            ItemKind::Type {
                flavor: TypeFlavor::Ordinary,
                generics: vec![],
                bounds: None,
                authority: None,
                body,
                drop: None,
            },
            visibility,
            line,
        )
    }
    fn index(&self) -> ManifestIndex {
        let (index, d) = ManifestIndex::new(ManifestFixture::from_files(
            package("test", "1"),
            [&self.file],
            ContentHash::of_bytes(b"manifest"),
        ));
        assert!(d.is_empty());
        index
    }
    fn run(&self, item: ItemId) -> ResolveOutput {
        let index = self.index();
        let mut fake = Fake::new(&index);
        resolve_item(&self.file, item, &self.symbols, &index, &mut fake)
    }
}
fn clean(output: &ResolveOutput) {
    assert!(
        output.query.diagnostics.is_empty(),
        "{:?}",
        output.query.diagnostics
    );
}

#[test]
fn loop_source_paths_resolve_without_operation_signature_reads() {
    let mut t = Tree::new("@loop_source(open, next) type View {}\n");
    let item = t.type_item("View", TypeBody::Record(vec![]), Visibility::Public, 1);
    let attribute = Attribute {
        name: t.name("loop_source"),
        arguments: vec![AttributeArgument::LoopSource(LoopSourceSyntax {
            open: t.path("open"),
            next: t.path("next"),
            origin: t.origin(1),
        })],
        origin: t.origin(1),
    };
    t.file.tree.items[item.0 as usize]
        .attributes
        .push(attribute);
    let mut manifest =
        ManifestFixture::from_files(package("test", "1"), [&t.file], ContentHash::default());
    for name in ["open", "next"] {
        manifest
            .definitions
            .push(entry(own(name), Namespace::Value, Visibility::Private));
    }
    let (index, diagnostics) = ManifestIndex::new(manifest);
    assert!(diagnostics.is_empty());
    for view in [
        ResolveView::File(item),
        ResolveView::Item(item),
        ResolveView::Header(item),
    ] {
        let mut fake = Fake::new(&index);
        let output = resolve_view(
            &t.file,
            view,
            &t.symbols,
            &index,
            &mut fake,
            &ExpectedTypes::default(),
        );
        clean(&output);
        assert!(fake.reads.is_empty(), "{:?}", fake.reads);
        assert!(output.query.dependencies.is_empty());
        for name in ["open", "next"] {
            assert!(
                output
                    .paths
                    .contains(&(NameSite::Item(item), ResolvedName::Definition(own(name)),))
            );
            assert!(output.reads.iter().any(|read| matches!(
                read,
                DependencyRead::Lookup { result: Ok(path), .. } if path == &own(name)
            )));
        }
    }
}

fn error_at(output: &ResolveOutput, rule: &str, span: Span) {
    let d = output
        .query
        .diagnostics
        .first()
        .expect("expected diagnostic");
    assert_eq!(d.rule.as_str(), rule);
    assert_eq!(d.primary, span);
}
fn semantic(kind: ty::TypeKind) -> ty::Type {
    ty::Type {
        drop_allocation: hirsel_types::DropAllocation::MayAllocate,
        ..ty::Type::with_kind(kind)
    }
}
fn constant(d: DefPath, kind: ty::TypeKind) -> ty::Signature {
    let mut types = ty::TypeStore::default();
    let id = types.push(semantic(kind));
    ty::Signature {
        metadata: Default::default(),

        definition: d,
        types,
        kind: ty::SignatureKind::Const(id),
        fingerprint: ContentHash::of_bytes(b"const-signature"),
    }
}

#[test]
fn exposure_is_a_module_head_not_an_unqualified_value_import() {
    let mut names = Interner::default();
    let d = definition(package("json", "1"), &["value"], "parse");
    let mut f = fixture(vec![entry(d.clone(), Namespace::Value, Visibility::Public)]);
    f.exposures.push(DependencyExposure {
        head: "v".into(),
        package: d.0.package.clone(),
        modules: vec!["value".into()],
    });
    let (index, _) = ManifestIndex::new(f);
    let context = FileContext {
        package: package("test", "1"),
        modules: vec![],
    };
    let v = names.intern("v");
    let parse = names.intern("parse");
    assert_eq!(
        index.lookup(
            &context,
            &Path {
                package: None,
                segments: vec![v, parse]
            },
            &names
        ),
        Ok(d)
    );
    assert_eq!(
        index.lookup(
            &context,
            &Path {
                package: None,
                segments: vec![parse]
            },
            &names
        ),
        Err(LookupError::Missing)
    );
}
#[test]
fn internal_paths_are_visible_only_under_the_parent_in_the_same_package() {
    let d = definition(package("test", "1"), &["a", "internal", "detail"], "T");
    let (index, _) = ManifestIndex::new(fixture(vec![entry(
        d.clone(),
        Namespace::Type,
        Visibility::Public,
    )]));
    for modules in [vec!["a".into()], vec!["a".into(), "client".into()]] {
        assert_eq!(
            index.access(
                &FileContext {
                    package: package("test", "1"),
                    modules
                },
                &d
            ),
            Ok(d.clone())
        );
    }
    for context in [
        FileContext {
            package: package("test", "1"),
            modules: vec!["b".into()],
        },
        FileContext {
            package: package("test", "2"),
            modules: vec!["a".into()],
        },
    ] {
        assert_eq!(
            index.access(&context, &d),
            Err(LookupError::Invisible(d.clone()))
        );
    }
}
#[test]
fn duplicate_paths_keep_stable_identity_and_cite_both_spans() {
    let mut a = entry(own("same"), Namespace::Value, Visibility::Private);
    a.span = Span::new(hirsel_span::FileId(0), 0, 4).unwrap();
    let mut b = a.clone();
    b.span = Span::new(hirsel_span::FileId(0), 5, 9).unwrap();
    let (index, d) = ManifestIndex::new(fixture(vec![b.clone(), a.clone()]));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].rule.as_str(), "LANG-7");
    assert_eq!(d[0].primary, b.span);
    assert_eq!(d[0].secondary[0].span, a.span);
    assert_eq!(index.entry(&own("same")).unwrap().span, a.span);
}
#[test]
fn unresolved_ambiguous_and_private_uses_have_rule_and_use_span() {
    let mut t = Tree::new("missing\njson.parse\nsecret\nfn example() {}\n");
    let missing = t.value("missing", 1);
    let ambiguous = t.value("json.parse", 2);
    let mut path = t.path("secret");
    path.package = Some(package("dep", "1"));
    let private = t.expr(ExprKind::Path(path), 3);
    let s1 = t.stmt(StmtKind::Expr(missing), 1);
    let s2 = t.stmt(StmtKind::Expr(ambiguous), 2);
    let s3 = t.stmt(StmtKind::Expr(private), 3);
    let body = t.block(vec![s1, s2, s3], None, 4);
    let item = t.function(
        "example",
        FunctionSyntax::default(),
        body,
        Visibility::Private,
        4,
    );
    let mut fixture =
        ManifestFixture::from_files(package("test", "1"), [&t.file], ContentHash::default());
    fixture.definitions.push(entry(
        definition(package("dep", "1"), &[], "secret"),
        Namespace::Value,
        Visibility::Private,
    ));
    for package in [package("dep", "1"), package("other", "1")] {
        fixture.exposures.push(DependencyExposure {
            head: "json".into(),
            package,
            modules: vec![],
        });
    }
    let (index, _) = ManifestIndex::new(fixture);
    let mut provider = Fake::new(&index);
    let output = resolve_item(&t.file, item, &t.symbols, &index, &mut provider);
    assert_eq!(output.query.diagnostics.len(), 3);
    for (d, (rule, line)) in
        output
            .query
            .diagnostics
            .iter()
            .zip([("LANG-4", 1), ("LANG-4", 2), ("PROG-2", 3)])
    {
        assert_eq!(d.rule.as_str(), rule);
        assert_eq!(d.primary, t.origin(line).span);
    }
    assert_eq!(
        output
            .reads
            .iter()
            .filter(|r| matches!(r, DependencyRead::Lookup { result: Err(_), .. }))
            .count(),
        3
    );
}
#[test]
fn successful_and_failed_signature_reads_and_failed_name_reads_survive_rejection() {
    let mut t =
        Tree::new("const good\nconst unavailable\ngood\nunavailable\nmissing\nfn example() {}\n");
    let ty = t.named_ty("u32", 1);
    let n = t.number("1", 1);
    t.item(
        "good",
        ItemKind::Const { ty, value: n },
        Visibility::Private,
        1,
    );
    let ty = t.named_ty("u32", 2);
    let n = t.number("2", 2);
    t.item(
        "unavailable",
        ItemKind::Const { ty, value: n },
        Visibility::Private,
        2,
    );
    let mut statements = vec![];
    for (name, line) in [("good", 3), ("unavailable", 4), ("missing", 5)] {
        let e = t.value(name, line);
        statements.push(t.stmt(StmtKind::Expr(e), line));
    }
    let body = t.block(statements, None, 6);
    let item = t.function(
        "example",
        FunctionSyntax::default(),
        body,
        Visibility::Private,
        6,
    );
    let index = t.index();
    let mut fake = Fake::new(&index);
    let signature = constant(own("good"), ty::TypeKind::Primitive(ty::Primitive::U32));
    fake.signatures.insert(own("good"), signature.clone());
    let output = resolve_item(&t.file, item, &t.symbols, &index, &mut fake);
    assert_eq!(
        output.query.dependencies,
        vec![ty::SignatureDependency {
            definition: own("good"),
            fingerprint: signature.fingerprint
        }]
    );
    assert_eq!(
        output.query.value.dependencies,
        vec![own("good"), own("unavailable")]
    );
    assert!(output.reads.contains(&DependencyRead::Signature {
        definition: own("unavailable"),
        fingerprint: None
    }));
    assert!(output.reads.iter().any(|r| matches!(r,DependencyRead::Lookup { segments,result: Err(LookupError::Missing),manifest,.. } if segments == &["missing"] && *manifest == index.revision)));
    assert_eq!(fake.reads.len(), 2);
}

#[test]
fn duplicate_generic_field_and_variant_names_report_rule_and_span() {
    let mut t = Tree::new("type Rec[T,T] { n: u32, n: u32 }\ntype Choice = First | First\n");
    let u32 = t.named_ty("u32", 1);
    let n = t.name("n");
    let generic = GenericParam {
        name: t.name("T"),
        kind: GenericKind::Type(vec![]),
    };
    let item = t.item(
        "Rec",
        ItemKind::Type {
            flavor: TypeFlavor::Ordinary,
            generics: vec![generic.clone(), generic],
            bounds: None,
            authority: None,
            body: TypeBody::Record(vec![
                FieldSyntax {
                    growable: false,

                    name: n,
                    ty: u32,
                    origin: t.origin(1),
                },
                FieldSyntax {
                    growable: false,

                    name: n,
                    ty: u32,
                    origin: t.origin(1),
                },
            ]),
            drop: None,
        },
        Visibility::Private,
        1,
    );
    let output = t.run(item);
    assert_eq!(output.query.diagnostics.len(), 2);
    for (d, rule) in output.query.diagnostics.iter().zip(["GEN-1", "LANG-12"]) {
        assert_eq!(d.rule.as_str(), rule);
        assert_eq!(d.primary, t.origin(1).span);
    }
    let first = t.name("First");
    let item = t.type_item(
        "Choice",
        TypeBody::Variants(vec![
            VariantSyntax {
                name: first,
                fields: vec![],
            },
            VariantSyntax {
                name: first,
                fields: vec![],
            },
        ]),
        Visibility::Private,
        2,
    );
    error_at(&t.run(item), "LANG-7", t.origin(2).span);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 256, ..Default::default() })]
    #[test]
    fn declaration_permutations_preserve_resolution(priorities in proptest::collection::vec(proptest::num::u64::ANY, 1..12)) {
        let mut t = Tree::new("fn declarations() {}\n");
        for i in 0..priorities.len() {
            let value = t.call(&format!("f{}",(i+1)%priorities.len()),vec![],1);
            let body = t.block(vec![],Some(value),1);
            t.function(&format!("f{i}"),FunctionSyntax::default(),body,Visibility::Private,1);
        }
        let before = t.file.roots.iter().map(|id| {
            let output = t.run(*id); clean(&output);
            (t.file.tree.items[id.0 as usize].definition.clone(),output.query.value)
        }).collect::<BTreeMap<_,_>>();
        let mut items = t.file.tree.items.clone().into_iter().enumerate().collect::<Vec<_>>();
        items.sort_by_key(|(i,_)| priorities[*i]);
        t.file.tree.items = items.into_iter().map(|(_,item)| item).collect();
        let after = t.file.roots.iter().map(|id| {
            let output = t.run(*id); clean(&output);
            (t.file.tree.items[id.0 as usize].definition.clone(),output.query.value)
        }).collect::<BTreeMap<_,_>>();
        proptest::prop_assert_eq!(before, after);
    }
}

#[test]
fn failed_provider_diagnostics_are_silent_for_reservations_and_emitted_once_at_use() {
    struct ReportingProvider;
    impl SignatureProvider for ReportingProvider {
        fn lookup(
            &mut self,
            _: &FileContext,
            _: &Path,
            _: &Interner,
        ) -> hirsel_types::LookupOutput {
            hirsel_types::LookupOutput {
                manifest: Default::default(),
                result: (|| {
                    Err(QueryError::Diagnostics(vec![
                        hirsel_diag::Diagnostic::error(
                            const { hirsel_diag::RuleId::of("LANG-4") },
                            Span::default(),
                            "provider unresolved name",
                        ),
                    ]))
                })(),
            }
        }
        fn signature(&mut self, _: &DefPath) -> Result<ty::SignatureRef, QueryError> {
            (|| Err(QueryError::NotImplemented))().map(std::sync::Arc::new)
        }
    }
    let mut t = Tree::new("let x = 1\nmissing\nfn example() {}\n");
    let n = t.number("1", 1);
    let sx = t.let_("x", None, n, 1);
    let missing = t.value("missing", 2);
    let body = t.block(vec![sx], Some(missing), 3);
    let item = t.function(
        "example",
        FunctionSyntax::default(),
        body,
        Visibility::Private,
        3,
    );
    let output = resolve_item(
        &t.file,
        item,
        &t.symbols,
        &t.index(),
        &mut ReportingProvider,
    );
    assert_eq!(output.query.diagnostics.len(), 1);
    error_at(&output, "LANG-4", t.origin(2).span);
    assert_eq!(
        output
            .reads
            .iter()
            .filter(|r| matches!(
                r,
                DependencyRead::Lookup {
                    result: Err(LookupError::Provider(_)),
                    ..
                }
            ))
            .count(),
        2
    );
}

#[test]
fn mixed_generic_parameter_ids_use_full_declaration_ordinals() {
    let mut t = Tree::new("fn example[T, n: size, U, m: size](a: T, b: U) { (n,m) }\n");
    let t_ty = t.named_ty("T", 1);
    let u_ty = t.named_ty("U", 1);
    let a = t.parameter("a", t_ty);
    let b = t.parameter("b", u_ty);
    let n = t.value("n", 1);
    let m = t.value("m", 1);
    let body = t.expr(ExprKind::Tuple(vec![n, m]), 1);
    let generics = [
        ("T", GenericKind::Type(vec![])),
        ("n", GenericKind::Size),
        ("U", GenericKind::Type(vec![])),
        ("m", GenericKind::Size),
    ]
    .into_iter()
    .map(|(name, kind)| GenericParam {
        name: t.name(name),
        kind,
    })
    .collect();
    let item = t.function(
        "example",
        FunctionSyntax {
            generics,
            parameters: vec![a, b],
            ..Default::default()
        },
        body,
        Visibility::Private,
        1,
    );
    let output = t.run(item);
    clean(&output);
    assert!(output.types.contains(&(
        t_ty,
        ResolvedName::TypeParameter(ty::TypeParameterRef::body(
            NameSite::Item(item),
            ty::TypeParamId(0)
        ))
    )));
    assert!(output.types.contains(&(
        u_ty,
        ResolvedName::TypeParameter(ty::TypeParameterRef::body(
            NameSite::Item(item),
            ty::TypeParamId(2)
        ))
    )));
    assert!(output.query.value.expressions.contains(&(
        n,
        ResolvedName::Size(ty::SizeParameterRef::body(
            NameSite::Item(item),
            ty::SizeParamId(1)
        ))
    )));
    assert!(output.query.value.expressions.contains(&(
        m,
        ResolvedName::Size(ty::SizeParameterRef::body(
            NameSite::Item(item),
            ty::SizeParamId(3)
        ))
    )));
    assert_eq!(
        output
            .generics
            .iter()
            .map(|g| g.parameter.clone())
            .collect::<Vec<_>>(),
        vec![
            GenericParameterName::Type(ty::TypeParameterRef::body(
                NameSite::Item(item),
                ty::TypeParamId(0)
            )),
            GenericParameterName::Size(ty::SizeParameterRef::body(
                NameSite::Item(item),
                ty::SizeParamId(1)
            )),
            GenericParameterName::Type(ty::TypeParameterRef::body(
                NameSite::Item(item),
                ty::TypeParamId(2)
            )),
            GenericParameterName::Size(ty::SizeParameterRef::body(
                NameSite::Item(item),
                ty::SizeParamId(3)
            ))
        ]
    );
}

#[test]
fn invalid_manifest_paths_have_a_prog1_diagnostic_at_the_manifest_site() {
    let mut invalid = entry(own("bad.name"), Namespace::Value, Visibility::Public);
    invalid.span = Span::new(hirsel_span::FileId(0), 3, 11).unwrap();
    let (index, diagnostics) = ManifestIndex::new(fixture(vec![invalid.clone()]));
    assert!(index.entry(&invalid.definition).is_none());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].rule.as_str(), "PROG-1");
    assert_eq!(diagnostics[0].primary, invalid.span);
}
