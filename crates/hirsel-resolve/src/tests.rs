use super::*;
use hirsel_intern::Symbol;
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::*;
use hirsel_types::{self as ty, QueryError, ResolvedName, SignatureProvider};
use std::collections::BTreeMap;

/// The session interner these tests' signatures and resolutions share.
fn session() -> &'static ty::TypeInterner {
    static SESSION: std::sync::OnceLock<std::sync::Arc<ty::TypeInterner>> =
        std::sync::OnceLock::new();
    SESSION.get_or_init(|| ty::TypeInterner::new(None))
}

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
    fn lookup(&mut self, context: &FileContext, path: &Path) -> hirsel_types::LookupOutput {
        hirsel_types::LookupOutput {
            manifest: Default::default(),
            result: (|| {
                self.lookups += 1;
                self.lookup_index
                    .lookup(context, path)
                    .map_err(|_| QueryError::NotImplemented)
            })(),
        }
    }
    fn signature(&mut self, definition: DefId) -> Result<ty::SignatureRef, QueryError> {
        (|| {
            let definition = definition.path();
            self.reads.push(definition.clone());
            self.signatures
                .get(definition)
                .cloned()
                .ok_or(QueryError::NotImplemented)
        })()
        .map(std::sync::Arc::new)
    }
}

/// An item's index in the built file.
type ItemId = usize;
struct Tree {
    file: SyntaxFile,
    /// The arenas of the item being built: the next `item` takes them.
    tree: SyntaxTree,
    /// Every span is file-relative: each item's anchor is the file's.
    anchor: hirsel_span::Anchor,
    source: String,
}
impl Tree {
    fn new(source: &str) -> Self {
        let mut sources = SourceMap::default();
        let file = sources.add("test.hir", source.into()).unwrap();
        Self {
            file: SyntaxFile {
                has_syntax_errors: false,
                file,
                items: vec![],
                tokens: vec![],
            },
            tree: SyntaxTree::default(),
            anchor: sources.file_anchor(file),
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
            span: Span::new(self.anchor, start, end).unwrap(),
            tokens: TokenRange::default(),
        }
    }
    fn name(&mut self, name: &str) -> Symbol {
        Symbol::intern(name)
    }
    fn path(&mut self, name: &str) -> Path {
        Path {
            package: None,
            segments: name.split('.').map(|s| self.name(s)).collect(),
        }
    }
    fn expr(&mut self, kind: ExprKind, line: u32) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        self.tree.expressions.push(Expr {
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
        self.expr(
            ExprKind::Block {
                label: None,
                statements,
                tail,
            },
            line,
        )
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
        let id = TypeExprId(self.tree.types.len() as u32);
        self.tree.types.push(TypeExpr {
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
        let id = PatternId(self.tree.patterns.len() as u32);
        self.tree.patterns.push(Pattern {
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
        let id = StmtId(self.tree.statements.len() as u32);
        self.tree.statements.push(Stmt {
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
                otherwise: None,
            },
            line,
        )
    }
    fn item(&mut self, name: &str, kind: ItemKind, visibility: Visibility, line: u32) -> ItemId {
        let id = self.file.items.len();
        let symbol = self.name(name);
        let item = Item {
            name_span: self.origin(line).span,
            name: symbol,
            kind,
            visibility,
            definition: own(name),
            origin: self.origin(line),
            attributes: vec![],
            generated: None,
        };
        self.file.items.push(std::sync::Arc::new(ItemSyntax {
            item,
            tree: std::mem::take(&mut self.tree),
            anchor: self.anchor,
            text_hash: ContentHash::of_bytes(name.as_bytes()),
            occurrence: 0,
        }));
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
        resolve_item(
            &self.file,
            &self.file.items[item],
            &index,
            session(),
            &mut fake,
        )
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
fn recursive_header_uses_own_syntax_without_a_failed_signature_read() {
    let mut t = Tree::new("pub type Value { next: Value }\n");
    let ty = t.named_ty("Value", 1);
    let field = FieldSyntax {
        growable: false,
        name: t.name("next"),
        ty,
        origin: t.origin(1),
    };
    let item = t.type_item(
        "Value",
        TypeBody::Record(vec![field]),
        Visibility::Public,
        1,
    );
    let index = t.index();
    for view in [ResolveView::Header, ResolveView::Item] {
        let mut fake = Fake::new(&index);
        let output = resolve_view(
            &t.file.items[item],
            &t.file.items,
            view,
            &index,
            session(),
            &mut fake,
            &ExpectedTypes::default(),
        );
        clean(&output);
        assert!(
            output
                .types
                .contains(&(ty, ResolvedName::Definition(own("Value"))))
        );
        let self_read = output.reads.iter().any(|read| matches!(
            (read).as_ref(), DependencyRead::Signature { definition, .. } if definition.path() == &own("Value")
        ));
        assert_eq!(self_read, matches!(view, ResolveView::Item));
        assert_eq!(
            fake.reads,
            if self_read {
                vec![own("Value")]
            } else {
                vec![]
            }
        );
    }
}

#[test]
fn recursive_header_retains_other_declarations_failed_signature_reads() {
    let mut t = Tree::new("pub type Value { next: Other }\n");
    let ty = t.named_ty("Other", 1);
    let name = t.name("next");
    let item = t.type_item(
        "Value",
        TypeBody::Record(vec![FieldSyntax {
            growable: false,
            name,
            ty,
            origin: t.origin(1),
        }]),
        Visibility::Public,
        1,
    );
    let mut manifest =
        ManifestFixture::from_files(package("test", "1"), [&t.file], ContentHash::default());
    manifest
        .definitions
        .push(entry(own("Other"), Namespace::Type, Visibility::Public));
    let (index, diagnostics) = ManifestIndex::new(manifest);
    assert!(diagnostics.is_empty());
    let mut fake = Fake::new(&index);
    let output = resolve_view(
        &t.file.items[item],
        &t.file.items,
        ResolveView::Header,
        &index,
        session(),
        &mut fake,
        &ExpectedTypes::default(),
    );
    clean(&output);
    assert_eq!(fake.reads, vec![own("Other")]);
    assert!(output.reads.iter().any(|read| {
        read == &DependencyRead::Signature {
            definition: DefId::intern(&own("Other")),
            fingerprint: None,
        }
        .shared()
    }));
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
    std::sync::Arc::make_mut(&mut t.file.items[item])
        .item
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
    for view in [ResolveView::File, ResolveView::Item, ResolveView::Header] {
        let mut fake = Fake::new(&index);
        let output = resolve_view(
            &t.file.items[item],
            &t.file.items,
            view,
            &index,
            session(),
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
                    .contains(&(NameSite::Item, ResolvedName::Definition(own(name)),))
            );
            assert!(output.reads.iter().any(|read| matches!(
                (read).as_ref(),
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
    let id = session().intern(semantic(kind));
    ty::Signature {
        metadata: Default::default(),

        definition: DefId::intern(&d),
        kind: ty::SignatureKind::Const {
            ty: id,
            generics: Default::default(),
        },
        fingerprint: ContentHash::of_bytes(b"const-signature"),
    }
}

#[test]
fn exposure_is_a_module_head_not_an_unqualified_value_import() {
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
    let v = Symbol::intern("v");
    let parse = Symbol::intern("parse");
    assert_eq!(
        index.lookup(
            &context,
            &Path {
                package: None,
                segments: vec![v, parse]
            }
        ),
        Ok(d)
    );
    assert_eq!(
        index.lookup(
            &context,
            &Path {
                package: None,
                segments: vec![parse]
            }
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
    a.span = Span::new(hirsel_span::Anchor(0), 0, 4).unwrap();
    let mut b = a.clone();
    b.span = Span::new(hirsel_span::Anchor(0), 5, 9).unwrap();
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
    let output = resolve_item(
        &t.file,
        &t.file.items[item],
        &index,
        session(),
        &mut provider,
    );
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
            .filter(|r| matches!((r).as_ref(), DependencyRead::Lookup { result: Err(_), .. }))
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
    let output = resolve_item(&t.file, &t.file.items[item], &index, session(), &mut fake);
    assert_eq!(
        output.query.dependencies,
        vec![ty::SignatureDependency {
            definition: hirsel_syntax::DefId::intern(&own("good")),
            fingerprint: signature.fingerprint
        }]
    );
    assert_eq!(
        output.query.value.dependencies,
        vec![own("good"), own("unavailable")]
    );
    assert!(output.reads.iter().any(|read| {
        read == &DependencyRead::Signature {
            definition: hirsel_syntax::DefId::intern(&own("unavailable")),
            fingerprint: None,
        }
        .shared()
    }));
    assert!(output.reads.iter().any(|r| matches!((r).as_ref(),DependencyRead::Lookup { segments,result: Err(LookupError::Missing),manifest,.. } if segments == &["missing"] && *manifest == index.revision)));
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
        let resolved = |t: &Tree| (0..t.file.items.len()).map(|id| {
            let output = t.run(id); clean(&output);
            (t.file.items[id].item.definition.clone(),output.query.value)
        }).collect::<BTreeMap<_,_>>();
        let before = resolved(&t);
        let mut items = t.file.items.clone().into_iter().enumerate().collect::<Vec<_>>();
        items.sort_by_key(|(i,_)| priorities[*i]);
        t.file.items = items.into_iter().map(|(_,item)| item).collect();
        let after = resolved(&t);
        proptest::prop_assert_eq!(before, after);
    }
}

#[test]
fn failed_provider_diagnostics_are_silent_for_reservations_and_emitted_once_at_use() {
    struct ReportingProvider;
    impl SignatureProvider for ReportingProvider {
        fn lookup(&mut self, _: &FileContext, _: &Path) -> hirsel_types::LookupOutput {
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
        fn signature(&mut self, _: DefId) -> Result<ty::SignatureRef, QueryError> {
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
        &t.file.items[item],
        &t.index(),
        session(),
        &mut ReportingProvider,
    );
    assert_eq!(output.query.diagnostics.len(), 1);
    error_at(&output, "LANG-4", t.origin(2).span);
    assert_eq!(
        output
            .reads
            .iter()
            .filter(|r| matches!(
                (r).as_ref(),
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
    let binder = LexicalOwner::first(&own("example")).site(NameSite::Item);
    assert!(output.types.contains(&(
        t_ty,
        ResolvedName::TypeParameter(ty::TypeParameterRef::body(binder, ty::TypeParamId(0)))
    )));
    assert!(output.types.contains(&(
        u_ty,
        ResolvedName::TypeParameter(ty::TypeParameterRef::body(binder, ty::TypeParamId(2)))
    )));
    assert!(output.query.value.expressions.contains(&(
        n,
        ResolvedName::Size(ty::SizeParameterRef::body(binder, ty::SizeParamId(1)))
    )));
    assert!(output.query.value.expressions.contains(&(
        m,
        ResolvedName::Size(ty::SizeParameterRef::body(binder, ty::SizeParamId(3)))
    )));
    assert_eq!(
        output
            .generics
            .iter()
            .map(|g| g.parameter.clone())
            .collect::<Vec<_>>(),
        vec![
            GenericParameterName::Type(ty::TypeParameterRef::body(binder, ty::TypeParamId(0))),
            GenericParameterName::Size(ty::SizeParameterRef::body(binder, ty::SizeParamId(1))),
            GenericParameterName::Type(ty::TypeParameterRef::body(binder, ty::TypeParamId(2))),
            GenericParameterName::Size(ty::SizeParameterRef::body(binder, ty::SizeParamId(3)))
        ]
    );
}

#[test]
fn invalid_manifest_paths_have_a_prog1_diagnostic_at_the_manifest_site() {
    let mut invalid = entry(own("bad.name"), Namespace::Value, Visibility::Public);
    invalid.span = Span::new(hirsel_span::Anchor(0), 3, 11).unwrap();
    let (index, diagnostics) = ManifestIndex::new(fixture(vec![invalid.clone()]));
    assert!(index.entry(&invalid.definition).is_none());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].rule.as_str(), "PROG-1");
    assert_eq!(diagnostics[0].primary, invalid.span);
}

#[test]
fn near_miss_candidates_follow_exposure_visibility_and_module_precedence() {
    let dep = package("library", "1");
    let mut f = fixture(vec![
        entry(own("length"), Namespace::Value, Visibility::Private),
        entry(
            definition(dep.clone(), &["text"], "len"),
            Namespace::Value,
            Visibility::Public,
        ),
        entry(
            definition(dep.clone(), &["text"], "secret"),
            Namespace::Value,
            Visibility::Private,
        ),
        entry(
            definition(dep.clone(), &["internal"], "hidden"),
            Namespace::Value,
            Visibility::Public,
        ),
        entry(
            definition(dep.clone(), &["other"], "length"),
            Namespace::Value,
            Visibility::Public,
        ),
    ]);
    f.exposures.push(DependencyExposure {
        head: "words".into(),
        package: dep.clone(),
        modules: vec!["text".into()],
    });
    f.exposures.push(DependencyExposure {
        head: "hidden".into(),
        package: dep.clone(),
        modules: vec!["internal".into()],
    });
    let (index, errors) = ManifestIndex::new(f);
    assert!(errors.is_empty());
    let context = FileContext {
        package: package("test", "1"),
        modules: vec![],
    };
    assert_eq!(
        index.candidate_names(&context, None, &["words".into(), "lne".into()], 1),
        ["len"]
    );
    assert_eq!(
        index.candidate_names(&context, None, &["lne".into()], 0),
        ["length"]
    );
    assert!(
        index
            .candidate_names(&context, None, &["hidden".into(), "hiden".into()], 1)
            .is_empty()
    );
    let mut exposures = vec![DependencyExposure {
        head: "words".into(),
        package: dep,
        modules: vec!["text".into()],
    }];
    exposures.push(DependencyExposure {
        head: "words".into(),
        package: package("other", "1"),
        modules: vec![],
    });
    let ambiguous = index.with_exposures(&exposures);
    assert!(
        ambiguous
            .candidate_names(&context, None, &["words".into(), "lne".into()], 1)
            .is_empty()
    );
}

#[test]
fn missing_lookup_candidates_ignore_clients_but_track_accessible_repairs() {
    let library = package("library", "1");
    let context = FileContext {
        package: library.clone(),
        modules: vec![],
    };
    let segments = ["lenght".into()];
    let revision = |candidate: &str, visibility, client: &str| {
        let mut f = fixture(vec![
            entry(
                definition(library.clone(), &[], candidate),
                Namespace::Value,
                visibility,
            ),
            entry(
                definition(package(client, "1"), &[], "length"),
                Namespace::Value,
                Visibility::Public,
            ),
        ]);
        f.revision = ContentHash::of_bytes(client.as_bytes());
        let (index, errors) = ManifestIndex::new(f);
        assert!(errors.is_empty());
        assert_eq!(
            index.lookup_spelling(&context, None, &segments),
            Err(LookupError::Missing)
        );
        index.lookup_candidates_revision(&context, None, &segments)
    };
    let before = revision("length", Visibility::Private, "imager");
    assert_eq!(before, revision("length", Visibility::Private, "client"));
    assert_ne!(before, revision("size", Visibility::Private, "client"));
    // The owner's private names remain visible in its own diagnostic input.
    assert_eq!(before, revision("length", Visibility::Public, "client"));
}

#[test]
fn bounded_candidates_match_the_exhaustive_spelling_scan() {
    let own_package = package("test", "1");
    let library = package("library", "1");
    let mut f = fixture(vec![]);
    for (pkg, modules, name, visibility) in [
        (own_package.clone(), vec![], "length", Visibility::Private),
        (
            own_package.clone(),
            vec!["nested"],
            "length",
            Visibility::Public,
        ),
        (
            own_package.clone(),
            vec!["nested", "deep"],
            "value",
            Visibility::Public,
        ),
        (library.clone(), vec!["text"], "len", Visibility::Public),
        (library.clone(), vec!["text"], "secret", Visibility::Private),
        (
            library.clone(),
            vec!["internal"],
            "hidden",
            Visibility::Public,
        ),
    ] {
        f.definitions.push(entry(
            definition(pkg, &modules, name),
            Namespace::Value,
            visibility,
        ));
    }
    f.exposures = vec![
        DependencyExposure {
            head: "words".into(),
            package: library.clone(),
            modules: vec!["text".into()],
        },
        DependencyExposure {
            head: "hidden".into(),
            package: library.clone(),
            modules: vec!["internal".into()],
        },
    ];
    let mut names = std::collections::BTreeSet::new();
    for entry in &f.definitions {
        names.insert(entry.definition.0.name.clone());
        names.extend(entry.definition.0.modules.iter().cloned());
    }
    names.extend(f.exposures.iter().map(|e| e.head.clone()));
    let (index, errors) = ManifestIndex::new(f);
    assert!(errors.is_empty());
    let ambiguous = index.with_exposures(&[
        DependencyExposure {
            head: "words".into(),
            package: library.clone(),
            modules: vec!["text".into()],
        },
        DependencyExposure {
            head: "words".into(),
            package: own_package.clone(),
            modules: vec![],
        },
    ]);
    for index in [&index, &ambiguous] {
        for modules in [vec![], vec!["nested".into()]] {
            let context = FileContext {
                package: own_package.clone(),
                modules,
            };
            for qualifier in [None, Some(&own_package), Some(&library)] {
                for path in [
                    vec!["lenght"],
                    vec!["words", "lne"],
                    vec!["wrods", "len"],
                    vec!["nested", "deep", "vlaue"],
                    vec!["hidden", "hiden"],
                    vec!["text", "lne"],
                ] {
                    let segments: Vec<String> = path.into_iter().map(String::from).collect();
                    for segment in 0..segments.len() {
                        let expected: Vec<_> = names
                            .iter()
                            .filter(|name| {
                                let mut candidate = segments.clone();
                                candidate[segment] = (*name).clone();
                                index
                                    .lookup_spelling(&context, qualifier, &candidate)
                                    .is_ok()
                            })
                            .cloned()
                            .collect();
                        assert_eq!(
                            index.candidate_names(&context, qualifier, &segments, segment),
                            expected,
                            "{context:?} {qualifier:?} {segments:?} {segment}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn shadow_binding_sites_locate_module_declaration() {
    let mut t = Tree::new("fn counts() {}\nlet counts = 0u64\n");
    let body = t.block(vec![], None, 1);
    t.function(
        "counts",
        FunctionSyntax::default(),
        body,
        Visibility::Private,
        1,
    );
    let zero = t.number("0u64", 2);
    let local = t.let_("counts", None, zero, 2);
    let body = t.block(vec![local], None, 2);
    let main = t.function(
        "main",
        FunctionSyntax::default(),
        body,
        Visibility::Public,
        2,
    );
    let output = t.run(main);
    assert_eq!(output.query.diagnostics.len(), 1);
    let diagnostic = &output.query.diagnostics[0];
    assert_eq!(diagnostic.rule.as_str(), "LANG-4");
    assert_eq!(diagnostic.primary, t.origin(2).span);
    assert_eq!(diagnostic.secondary.len(), 1);
    assert_eq!(diagnostic.secondary[0].span, t.origin(1).span);
    assert!(diagnostic.secondary[0].message.contains("test@1"));
    assert!(diagnostic.fix.is_none());
}

#[test]
fn shadow_binding_sites_unavailable_source_has_qualified_note() {
    let mut t = Tree::new("let counts = 0u64\n");
    let zero = t.number("0u64", 1);
    let local = t.let_("counts", None, zero, 1);
    let body = t.block(vec![local], None, 1);
    let main = t.function(
        "main",
        FunctionSyntax::default(),
        body,
        Visibility::Public,
        1,
    );
    // Both source-less indexed declarations and provider-only declarations
    // retain their stable identity without a new signature/source query.
    for indexed in [true, false] {
        let counts = own("counts");
        let entries = vec![entry(counts.clone(), Namespace::Value, Visibility::Private)];
        let (provider_index, _) = ManifestIndex::new(fixture(entries.clone()));
        let (index, _) = ManifestIndex::new(fixture(if indexed { entries } else { vec![] }));
        let mut fake = Fake::new(&provider_index);
        fake.signatures.insert(
            counts.clone(),
            constant(counts, ty::TypeKind::Primitive(ty::Primitive::U64)),
        );
        let output = resolve_item(&t.file, &t.file.items[main], &index, session(), &mut fake);
        assert_eq!(output.query.diagnostics.len(), 1);
        let d = &output.query.diagnostics[0];
        assert_eq!(d.primary, t.origin(1).span);
        assert!(d.secondary.is_empty());
        assert!(d.notes.iter().any(|n| n.contains("test@1/counts")));
        assert!(d.notes.iter().any(|n| n.contains("unavailable")));
        assert!(d.fix.is_none());
        assert_eq!(fake.reads.len(), usize::from(!indexed));
    }
}

#[test]
fn shadow_binding_sites_nested_local_retains_primary_order() {
    let mut t = Tree::new("outer counts\ninner counts\n");
    let zero = t.number("0u64", 1);
    let outer = t.let_("counts", None, zero, 1);
    let zero = t.number("0u64", 2);
    let inner = t.let_("counts", None, zero, 2);
    let inner = t.block(vec![inner], None, 2);
    let body = t.block(vec![outer], Some(inner), 1);
    let main = t.function(
        "main",
        FunctionSyntax::default(),
        body,
        Visibility::Public,
        1,
    );
    let output = t.run(main);
    assert_eq!(output.query.diagnostics.len(), 1);
    let d = &output.query.diagnostics[0];
    assert_eq!(d.rule.as_str(), "LANG-4");
    assert_eq!(d.message, "a local cannot shadow another local");
    assert_eq!(d.primary, t.origin(2).span);
    assert_eq!(d.secondary[0].span, t.origin(1).span);
    assert_eq!(d.secondary[0].message, "earlier local");
}

#[test]
fn shadow_binding_sites_imported_heads_and_values_remain_allowed() {
    let mut t = Tree::new("let counts = 0u64\nlet helpers = 0u64\n");
    let zero = t.number("0u64", 1);
    let counts = t.let_("counts", None, zero, 1);
    let zero = t.number("0u64", 2);
    let helpers = t.let_("helpers", None, zero, 2);
    let body = t.block(vec![counts, helpers], None, 1);
    let main = t.function(
        "main",
        FunctionSyntax::default(),
        body,
        Visibility::Public,
        1,
    );
    let dependency = definition(package("library", "1"), &[], "counts");
    let mut fixture = fixture(vec![entry(
        dependency.clone(),
        Namespace::Value,
        Visibility::Public,
    )]);
    fixture.exposures.push(DependencyExposure {
        head: "helpers".into(),
        package: dependency.0.package,
        modules: vec![],
    });
    let (index, diagnostics) = ManifestIndex::new(fixture);
    assert!(diagnostics.is_empty());
    let mut fake = Fake::new(&index);
    clean(&resolve_item(
        &t.file,
        &t.file.items[main],
        &index,
        session(),
        &mut fake,
    ));
}

/// The driver oracle covers source manifests; these vectors exercise the
/// resolver's provider fallback and the immutable importer-view boundary.
#[test]
fn package_head_22_ambiguity_does_not_read_provider_signatures() {
    let mut t = Tree::new("vendor.value\nfn example() {}\n");
    let value = t.value("vendor.value", 1);
    let body = t.block(vec![], Some(value), 2);
    let item = t.function(
        "example",
        FunctionSyntax::default(),
        body,
        Visibility::Public,
        2,
    );
    let target = definition(package("dep", "1"), &["api"], "value");
    let absent = definition(package("dep", "2"), &["api"], "value");
    let mut fixture =
        ManifestFixture::from_files(package("test", "1"), [&t.file], ContentHash::default());
    fixture
        .definitions
        .push(entry(target.clone(), Namespace::Value, Visibility::Public));
    fixture.exposures = [&absent, &target, &target]
        .map(|d| DependencyExposure {
            head: "vendor".into(),
            package: d.0.package.clone(),
            modules: d.0.modules.clone(),
        })
        .into();
    for reversed in [false, true] {
        if reversed {
            fixture.definitions.reverse();
            fixture.exposures.reverse();
        }
        let (index, diagnostics) = ManifestIndex::new(fixture.clone());
        assert!(diagnostics.is_empty());
        let mut provider = Fake::new(&index);
        let output = resolve_item(
            &t.file,
            &t.file.items[item],
            &index,
            session(),
            &mut provider,
        );
        error_at(&output, "LANG-4", t.origin(1).span);
        assert_eq!(output.query.diagnostics.len(), 1);
        assert!(
            output.query.diagnostics[0]
                .message
                .contains("dep@1/api/value, dep@2/api/value")
        );
        assert_eq!(provider.lookups, 0);
        assert!(provider.reads.is_empty());
        assert!(output.query.value.dependencies.is_empty());
    }
}

#[test]
fn package_head_22_importer_views_keep_aliases_and_versions_separate() {
    let first = definition(package("dep", "1"), &["api", "nested"], "value");
    let second = definition(package("dep", "2"), &["api", "nested"], "value");
    let (index, diagnostics) = ManifestIndex::new(fixture(vec![
        entry(first.clone(), Namespace::Value, Visibility::Public),
        entry(second.clone(), Namespace::Value, Visibility::Public),
    ]));
    assert!(diagnostics.is_empty());
    let exposure = |target: &DefPath| DependencyExposure {
        head: "vendor".into(),
        package: target.0.package.clone(),
        modules: vec!["api".into()],
    };
    let left = index.with_exposures(&[exposure(&first), exposure(&first)]);
    let right = index
        .for_package(package("wrapper", "1"))
        .with_exposures(&[exposure(&second)]);
    let spelling = ["vendor".into(), "nested".into(), "value".into()];
    // Interleave views: neither selecting an importer nor replacing its exposure
    // set mutates the shared rows or another importer's aliases.
    for selected in [&left, &right, &left, &index, &right] {
        let context = FileContext {
            package: selected.package.clone(),
            modules: vec![],
        };
        let expected = if std::ptr::eq(selected, &left) {
            Ok(first.clone())
        } else if std::ptr::eq(selected, &right) {
            Ok(second.clone())
        } else {
            Err(LookupError::Missing)
        };
        assert_eq!(
            selected.lookup_spelling(&context, None, &spelling),
            expected
        );
        for target in [&first, &second] {
            assert_eq!(
                selected.lookup_spelling(
                    &context,
                    Some(&target.0.package),
                    &["api".into(), "nested".into(), "value".into()]
                ),
                Ok(target.clone())
            );
        }
        assert_eq!(
            selected.lookup_spelling(
                &context,
                Some(&package("dep", "3")),
                &["api".into(), "nested".into(), "value".into()]
            ),
            Err(LookupError::Missing)
        );
    }
}

#[test]
fn package_head_22_core_std_merge_requires_matching_module_targets() {
    let core = definition(package("core", "1"), &["string"], "byte_len");
    let std = definition(package("std", "1"), &["string"], "find");
    let (index, diagnostics) = ManifestIndex::new(fixture(vec![
        entry(core.clone(), Namespace::Value, Visibility::Public),
        entry(std.clone(), Namespace::Value, Visibility::Public),
    ]));
    assert!(diagnostics.is_empty());
    let context = FileContext {
        package: package("test", "1"),
        modules: vec![],
    };
    let expose = |p, module: &str| DependencyExposure {
        head: "string".into(),
        package: p,
        modules: vec![module.into()],
    };
    let defaults = [
        expose(package("core", "1"), "string"),
        expose(package("std", "1"), "string"),
    ];
    for reversed in [false, true] {
        let mut exposures = defaults.to_vec();
        if reversed {
            exposures.reverse();
        }
        let merged = index.with_exposures(&exposures);
        for target in [&core, &std] {
            assert_eq!(
                merged.lookup_spelling(&context, None, &["string".into(), target.0.name.clone()]),
                Ok(target.clone())
            );
        }
        assert_eq!(
            merged.lookup_spelling(&context, None, &["string".into(), "absent".into()]),
            Err(LookupError::MissingMember(vec![
                definition(package("core", "1"), &["string"], "absent"),
                definition(package("std", "1"), &["string"], "absent"),
            ]))
        );
        // A mismatched module and a second std version are distinct targets,
        // even though only core declares byte_len.
        for extra in [
            expose(package("std", "1"), "other"),
            expose(package("std", "2"), "string"),
        ] {
            let mut exposures = vec![defaults[0].clone()];
            if extra.package.version == "2" {
                exposures.push(defaults[1].clone());
            }
            exposures.push(extra.clone());
            let selected = index.with_exposures(&exposures);
            assert_eq!(
                selected.lookup_spelling(&context, None, &["string".into(), "byte_len".into()]),
                Err(LookupError::Ambiguous(
                    exposures
                        .iter()
                        .map(|e| definition(e.package.clone(), &[&e.modules[0]], "byte_len"))
                        .collect()
                ))
            );
        }
    }
}
