use super::super::*;
use hirsel_intern::Symbol;
use hirsel_resolve::{ManifestFixture, ManifestIndex};
use hirsel_span::SourceMap;
use std::sync::Arc;
/// Every fixture and builtin row belongs to this shared session.
pub(super) fn session() -> &'static TypeInterner {
    static TYPES: std::sync::OnceLock<Arc<TypeInterner>> = std::sync::OnceLock::new();
    TYPES.get_or_init(hirsel_engine::registry::session_types)
}
pub(super) fn memo() -> &'static TypeMemo {
    static MEMO: std::sync::OnceLock<TypeMemo> = std::sync::OnceLock::new();
    MEMO.get_or_init(TypeMemo::new)
}
pub(super) struct Registry {
    pub signatures: BTreeMap<ast::DefPath, SignatureRef>,
    pub index: ManifestIndex,
    pub reads: Vec<ast::DefPath>,
}
pub(super) fn path(name: &str) -> ast::DefPath {
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = parts.pop().unwrap();
    ast::DefPath(ast::ItemPath {
        package: ast::PackageId {
            name: "test".into(),
            version: "1".into(),
        },
        modules: parts,
        name,
    })
}
pub(super) fn core(name: &str) -> ast::DefPath {
    let (package, name) = name
        .strip_prefix("std@1/")
        .map_or(("core", name), |name| ("std", name));
    let mut p = path(name);
    p.0.package = ast::PackageId {
        name: package.into(),
        version: "1".into(),
    };
    p
}
pub(super) fn fixture(file: &ast::SyntaxFile) -> ManifestFixture {
    let mut f = ManifestFixture::from_files(context().package, [file], ast::ContentHash::default());
    for head in [
        "io",
        "out",
        "test",
        "path",
        "u64",
        "u32",
        "file",
        "dir",
        "arg",
        "bytes",
        "task",
        "mutex",
        "channel",
        "atomic",
        "sender",
        "arena",
        "region",
        "shared",
        "slots",
        "list",
        "array",
        "iter",
        "int",
        "float",
        "string",
        "unicode",
        "map",
        "set",
        "sorted_map",
        "simd",
    ] {
        f.exposures.push(hirsel_resolve::DependencyExposure {
            head: head.into(),
            package: core("Result").0.package,
            modules: vec![head.into()],
        });
    }
    f
}
pub(super) fn context() -> ast::FileContext {
    ast::FileContext {
        package: path("main").0.package,
        modules: vec![],
    }
}
impl SignatureProvider for Registry {
    fn signature(&mut self, id: ast::DefId) -> Result<SignatureRef, QueryError> {
        let path = id.path();
        self.reads.push(path.clone());
        self.signatures
            .get(path)
            .cloned()
            .ok_or(QueryError::NotImplemented)
    }
    fn lookup(&mut self, context: &ast::FileContext, path: &ast::Path) -> LookupOutput {
        let names: Vec<_> = path.segments.iter().map(|s| s.as_str()).collect();
        let mut p = self::path(&names.join("."));
        if let Some(package) = &path.package {
            p.0.package = package.clone();
        }
        let mut core = core(&names.join("."));
        if let Some(package) = &path.package {
            core.0.package = package.clone();
        }
        let result = self
            .index
            .lookup(context, path)
            .ok()
            .or_else(|| self.signatures.contains_key(&p).then_some(p))
            .or_else(|| self.signatures.contains_key(&core).then_some(core))
            .ok_or(QueryError::NotImplemented);
        LookupOutput {
            manifest: ast::ContentHash::default(),
            result,
        }
    }
}
pub(super) fn parse(
    source: &str,
    symbol_seed: &[String],
) -> (ast::SyntaxFile, SourceMap, Vec<Diagnostic>) {
    let mut map = SourceMap::default();
    let file = map.add("body.hir", source.into()).unwrap();
    for s in symbol_seed {
        Symbol::intern(s);
    }
    let lex = hirsel_lexer::lex(file, &map);
    let parsed = hirsel_parser::parse(file, lex.tokens, &context(), &mut map);
    let mut ds = lex.diagnostics;
    ds.extend(parsed.diagnostics);
    (parsed.syntax, map, ds)
}
fn row(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
fn type_signature(
    name: &str,
    bounds: Vec<ast::Bound>,
    cap: Option<CapabilityKind>,
    representation: Option<TypeKind>,
) -> Signature {
    let types = session();
    let repr = representation.map(|r| types.intern(row(r)));
    Signature {
        definition: core(name).into(),
        metadata: SignatureMetadata {
            visibility: ast::Visibility::Public,
            roles: NominalRoles {
                result_type: Some(core("Result")),
                unsafe_capability: Some(core("Unsafe")),
                tasks_capability: None,
                scope_capability: None,
                heap: Some(HeapRole {
                    arena: core("Arena"),
                    operation: core("$intrinsic.heap"),
                }),
            }
            .into(),
            ..Default::default()
        },
        fingerprint: ast::ContentHash::of_bytes(name.as_bytes()),
        kind: SignatureKind::Type(Box::new(TypeDefinition {
            loop_source: None,
            format: None,
            generics: vec![],
            representation: repr,
            bounds,
            authority: AuthoritySummary::inert(),
            capability: cap,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
            destructor_allocation: Allocation::NoAlloc,
        })),
    }
}
pub(super) fn registry(file: &ast::SyntaxFile) -> Registry {
    let (index, _) = ManifestIndex::new(fixture(file));
    let mut registry = Registry {
        signatures: BTreeMap::new(),
        index,
        reads: vec![],
    };
    let core_registry = hirsel_engine::registry::BuiltinRegistry::new();
    for signature in core_registry
        .signatures()
        .filter(|s| s.definition.path().0.modules == ["$intrinsic"])
    {
        registry
            .signatures
            .insert(signature.definition.path().clone(), signature.clone());
    }

    for name in [
        "$intrinsic.MapInsertionOrder_len",
        "$intrinsic.MapInsertionOrder_advance",
        "$intrinsic.SortedMapOrder_len",
    ] {
        let path = core(name);
        registry
            .signatures
            .insert(path.clone(), core_registry.get(&path).unwrap());
    }
    for name in [
        "Io",
        "Args",
        "Env",
        "Unsafe",
        "Dir",
        "MonotonicClock",
        "Net",
        "Random",
        "Stdout",
        "Tasks",
        "Build",
    ] {
        registry.signatures.insert(
            core(name),
            Arc::new(type_signature(
                name,
                vec![ast::Bound::Plain, ast::Bound::Copyable],
                Some(CapabilityKind::Global),
                None,
            )),
        );
    }
    for name in ["Test", "Scope", "Arena"] {
        registry.signatures.insert(
            core(name),
            Arc::new(type_signature(
                name,
                vec![],
                Some(CapabilityKind::Scoped),
                None,
            )),
        );
    }
    for name in ["Path", "StrPos", "Failure"] {
        registry.signatures.insert(
            core(name),
            Arc::new(type_signature(
                name,
                vec![
                    ast::Bound::Plain,
                    ast::Bound::Copyable,
                    ast::Bound::Data,
                    ast::Bound::Key,
                ],
                None,
                None,
            )),
        );
    }
    for name in ["File", "AtomicFile", "ScalarView", "PositionView"] {
        registry.signatures.insert(
            core(name),
            Arc::new(type_signature(name, vec![], None, None)),
        );
    }
    for name in ["ScalarView", "PositionView"] {
        let path = core(name);
        let association = match &core_registry.get(&path).unwrap().kind {
            SignatureKind::Type(t) => t.loop_source.clone(),
            _ => unreachable!(),
        };
        let SignatureKind::Type(t) =
            &mut Arc::make_mut(registry.signatures.get_mut(&path).unwrap()).kind
        else {
            unreachable!()
        };
        t.loop_source = association;
    }
    // Ordinary std fixture declarations; no core alias or privileged cursor protocol.
    for name in [
        "std@1/unicode.GraphemeView",
        "std@1/unicode.GraphemeCursor",
        "std@1/test.Source",
    ] {
        let mut signature = type_signature(
            name,
            vec![
                ast::Bound::Copyable,
                ast::Bound::Inert,
                ast::Bound::NoallocDrop,
            ],
            None,
            None,
        );
        if name.ends_with("GraphemeView") {
            let SignatureKind::Type(t) = &mut signature.kind else {
                unreachable!()
            };
            t.loop_source = Some(LoopSourceDeclaration {
                open: core("std@1/unicode.graphemes_open"),
                next: core("std@1/unicode.graphemes_next"),
                option: core("Option"),
            });
        }
        registry.signatures.insert(core(name), Arc::new(signature));
    }
    // Bootstrap nominal headers so recursive graphs need no guessed callee bodies.
    let declarations = "type SlotId[T] { value: u64 }\ntype Array[T] { value: T }\ntype List[T] { value: T }\ntype Inline[T,n:size] { value: T }\ntype Iter[T] { value: T }\ntype Map[K:key,V] { key: K, value: V }\ntype Set[T:key] { value: T }\ntype SortedMap[K,V] { key: K, value: V }\ntype Ptr[T] { value: T }\ntype Option[T] = None | Some(T)\ntype Result[T,E] = Ok(T) | Error(E)\ntype CliError = Usage(ArgError) | Files(FileError) | Output(OutputError)\ntype GrantError = NotGranted()\ntype ArgError = MissingArgument()\ntype CancelCause = Requested | Parent | ChildFailed\ntype CancelError = Canceled(CancelCause)\ntype ChannelError = Closed | Canceled(CancelCause)\ntype FileError = FileFault(u32) | Canceled(CancelCause) | NotGranted()\ntype OutputError = OutputFault(u32) | Canceled(CancelCause)\ntype FloatIntError = NotFinite | OutOfRange\ntype OutOfSpace = OutOfSpace()\ntype Eq[T] { equal: fn(T,T) -> bool uses {} }\ntype Order[T] { compare: fn(T,T) -> i32 uses {} }\ntype Hasher[T] { hash: fn(T,u64) -> u64 uses {} }\n";
    let (mut types, _, ds) = parse(declarations, &[]);
    for syntax in &mut types.items {
        let item = &mut Arc::make_mut(syntax).item;
        item.definition = core(&item.definition.0.name);
    }
    assert!(ds.is_empty(), "{ds:?}");
    for item in types.items.iter().map(|s| &s.item) {
        let mut s = type_signature(&item.definition.0.name, vec![], None, None);
        if let ast::ItemKind::Type { generics, .. } = &item.kind {
            if let SignatureKind::Type(td) = &mut s.kind {
                td.generics = generics
                    .iter()
                    .map(|g| GenericParameter::Type {
                        name: g.name.as_str().into(),
                        bounds: vec![].into(),
                    })
                    .collect();
            }
        }
        registry
            .signatures
            .insert(item.definition.clone(), Arc::new(s));
    }
    normalize_file(&types, &mut registry).unwrap();
    // Each library signature is parsed in its explicit module, without overloads.
    let source = include_str!("library.hir");
    let mut noalloc = false;
    for line in source.lines() {
        if line == "@noalloc" {
            noalloc = true;
            continue;
        }
        if !line.starts_with("fn ") {
            continue;
        }
        let declared_name = line[3..].split(['[', '(']).next().unwrap();
        let (package, name) = declared_name
            .split_once('/')
            .unwrap_or(("core@1", declared_name));
        let mut p = core(name);
        let (package_name, version) = package.split_once('@').unwrap();
        p.0.package = ast::PackageId {
            name: package_name.into(),
            version: version.into(),
        };
        let simple = "fixture_function";
        let code = format!(
            "{}{}",
            if noalloc { "@noalloc\n" } else { "" },
            line.replacen(declared_name, simple, 1)
        );
        noalloc = false;
        let code = if name.starts_with("simd.") {
            code.replacen("[A,", "[A: plain,", 1)
                .replacen("[I,", "[I: plain,", 1)
        } else {
            code
        };
        let (mut library, _, ds) = parse(&code, &[]);
        assert!(ds.is_empty(), "{code}: {ds:?}");
        Arc::make_mut(&mut library.items[0]).item.definition = p.clone();
        // Library module namespace is represented by the canonical DefPath.
        if let Err(reason) = normalize_file(&library, &mut registry) {
            panic!("library {name}: {reason}");
        }
        if let Some(builtin) = hirsel_engine::registry::BuiltinRegistry::new().get(&p) {
            Arc::make_mut(registry.signatures.get_mut(&p).unwrap())
                .metadata
                .schema = builtin.metadata.schema.clone();
        }
        // E needs a plain lane proof while core library intrinsics use scalar families.
        if name.starts_with("simd.") {
            let s = registry.signatures.get_mut(&p).unwrap();
            if let SignatureKind::Function(f) = &mut Arc::make_mut(s).kind {
                if let GenericParameter::Type { bounds, .. } = &mut f.generics[0] {
                    bounds.clear();
                }
            }
        }
    }
    for (name, primitive) in [
        ("u8", Primitive::U8),
        ("u16", Primitive::U16),
        ("u32", Primitive::U32),
        ("u64", Primitive::U64),
        ("u128", Primitive::U128),
        ("i8", Primitive::I8),
        ("i16", Primitive::I16),
        ("i32", Primitive::I32),
        ("i64", Primitive::I64),
        ("i128", Primitive::I128),
    ] {
        let types = session();
        let scalar = types.intern(row(TypeKind::Primitive(primitive)));
        let dictionary = types.intern(row(TypeKind::Named {
            definition: core("Order").into(),
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        let name = format!("{name}.order");
        registry.signatures.insert(
            core(&name),
            Arc::new(Signature {
                metadata: SignatureMetadata {
                    visibility: ast::Visibility::Public,
                    ..Default::default()
                },
                definition: core(&name).into(),
                kind: SignatureKind::Const {
                    ty: dictionary,
                    generics: Default::default(),
                },
                fingerprint: ast::ContentHash::of_bytes(name.as_bytes()),
            }),
        );
    }
    for signature in heap_signatures() {
        registry
            .signatures
            .insert(signature.definition.path().clone(), Arc::new(signature));
    }
    registry.index =
        ManifestIndex::new(fixture(file).with_dependency_signatures(registry.signatures.values()))
            .0;
    registry
}
pub(super) fn normalize_file(
    file: &ast::SyntaxFile,
    registry: &mut Registry,
) -> Result<(), String> {
    let metadata = Metadata {
        heap: Some(HeapRole {
            arena: core("Arena"),
            operation: core("$intrinsic.heap"),
        }),
        result_type: Some(core("Result")),
        unsafe_capability: Some(core("Unsafe")),
        definitions: registry
            .signatures
            .iter()
            .map(|(p, s)| {
                (
                    p.clone(),
                    DefinitionMetadata {
                        public: true,
                        alias: s.metadata.alias,
                    },
                )
            })
            .chain(file.items.iter().map(|s| &s.item).map(|i| {
                (
                    i.definition.clone(),
                    DefinitionMetadata {
                        public: true,
                        alias: matches!(
                            &i.kind,
                            ast::ItemKind::Type {
                                body: ast::TypeBody::Alias(_),
                                ..
                            }
                        ),
                    },
                )
            }))
            .collect(),
        ..Default::default()
    };
    let mut pending: Vec<_> = file.items.iter().collect();
    let mut last = String::new();
    for _ in 0..file.items.len() + 1 {
        let mut next = vec![];
        for syntax in pending {
            let r = hirsel_resolve::resolve_item(
                file,
                syntax,
                &registry.index.clone(),
                session(),
                registry,
            );
            let item = &syntax.item;
            let ctx = ast::FileContext {
                package: item.definition.0.package.clone(),
                modules: item.definition.0.modules.clone(),
            };
            match hirsel_signature::normalize(
                hirsel_check_protocol::header::Input {
                    types: session(),
                    memo: memo(),
                    syntax,
                    context: &ctx,
                    resolution: &r,
                    metadata: &metadata,
                    declaration_sites: &metadata.declarations,
                    stage: None,
                },
                registry,
            ) {
                Ok(mut out) => {
                    // These bootstrap and library declarations are public fixture exports.
                    out.value.metadata.visibility = ast::Visibility::Public;
                    registry
                        .signatures
                        .insert(item.definition.clone(), Arc::new(out.value));
                }
                Err(e) => {
                    next.push(syntax);
                    last = format!("{e:?}");
                }
            }
        }
        if next.is_empty() {
            return Ok(());
        }
        pending = next;
    }
    Err(last)
}
pub(super) fn check_source(source: &str) -> (Vec<Diagnostic>, Vec<Output>, SourceMap) {
    let (file, map, mut ds) = parse(source, &[]);
    if !ds.is_empty() {
        return (ds, vec![], map);
    }
    let mut registry = registry(&file);
    if let Err(e) = normalize_file(&file, &mut registry) {
        panic!("normalizing {source}: {e}");
    }
    let mut outputs = vec![];
    for syntax in &file.items {
        let resolution = hirsel_resolve::resolve_item(
            &file,
            syntax,
            &registry.index.clone(),
            session(),
            &mut registry,
        );
        assert!(
            resolution.query.diagnostics.is_empty(),
            "resolver {source}: {:?}",
            resolution.query.diagnostics
        );
        let sig = registry.signatures[&syntax.item.definition].clone();
        let out = check_with_loop_protocols(
            LoopInput {
                body: Input {
                    types: session(),
                    memo: memo(),
                    syntax,
                    context: &context(),
                    resolution: &resolution,
                    signature: &sig,
                },
                protocols: &hirsel_types::core_roles::loop_protocols(),
            },
            &mut registry,
        )
        .unwrap();
        ds.extend(out.checked.diagnostics.clone());
        outputs.push(out);
    }
    ds.sort_by_key(|d| {
        let at = map.resolve(d.primary);
        (at.start, at.end, d.rule.as_str().to_string())
    });
    (ds, outputs, map)
}

pub(super) fn heap_signatures() -> [Signature; 2] {
    let arena = type_signature("Arena", vec![], Some(CapabilityKind::Scoped), None);
    let mut heap = type_signature("$intrinsic.heap", vec![], None, None);
    let mut result = row(TypeKind::Named {
        definition: core("Arena").into(),
        arguments: vec![].into(),
    });
    result.allocation_target = Some(AllocationTarget::Heap);
    result.modes.sendability = Sendability::Sendable;
    let result = session().intern(result);
    heap.kind = SignatureKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    });
    [arena, heap]
}
