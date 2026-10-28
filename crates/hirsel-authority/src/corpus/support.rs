pub(super) use crate::test_support::{session, session_memo};
use hirsel_diag::Diagnostic;
use hirsel_intern::Symbol;
use hirsel_resolve::{ManifestFixture, ManifestIndex};
use hirsel_span::SourceMap;
use hirsel_syntax as ast;
use hirsel_types::*;
use std::collections::BTreeMap;
use std::sync::Arc;
pub(super) struct Registry {
    pub signatures: BTreeMap<ast::DefPath, SignatureRef>,
    pub index: ManifestIndex,
    pub reads: Vec<ast::DefPath>,
}
pub(super) fn path(name: &str) -> ast::DefPath {
    let (package, name) = name
        .strip_prefix("std@1/")
        .map_or(("core", name), |name| ("std", name));
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = parts.pop().unwrap();
    ast::DefPath(ast::ItemPath {
        package: ast::PackageId {
            name: package.into(),
            version: "1".into(),
        },
        modules: parts,
        name,
    })
}
pub(super) fn context() -> ast::FileContext {
    ast::FileContext {
        package: ast::PackageId {
            name: "test".into(),
            version: "1".into(),
        },
        modules: vec![],
    }
}
impl SignatureProvider for Registry {
    fn signature(&mut self, definition: ast::DefId) -> Result<SignatureRef, QueryError> {
        let path = definition.path();
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
        let result = self
            .index
            .lookup(context, path)
            .ok()
            .or_else(|| self.signatures.contains_key(&p).then_some(p))
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
    let repr = representation.map(|r| session().intern(row(r)));
    Signature {
        definition: ast::DefId::intern(&path(name)),
        metadata: SignatureMetadata {
            visibility: ast::Visibility::Public,
            roles: NominalRoles {
                result_type: Some(path("Result")),
                unsafe_capability: Some(path("Unsafe")),
                tasks_capability: Some(path("Tasks")),
                scope_capability: Some(path("Scope")),
                heap: Some(HeapRole {
                    arena: path("Arena"),
                    operation: path("$intrinsic.heap"),
                }),
            }
            .into(),
            ..Default::default()
        },
        fingerprint: ast::ContentHash::of_bytes(name.as_bytes()),
        kind: SignatureKind::Type(TypeDefinition {
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
        }),
    }
}
pub(super) fn registry(file: &ast::SyntaxFile) -> Registry {
    let (index, _) = ManifestIndex::new(ManifestFixture::from_files(
        context().package,
        [file],
        ast::ContentHash::default(),
    ));
    let mut registry = Registry {
        signatures: BTreeMap::new(),
        index,
        reads: vec![],
    };
    for signature in
        hirsel_engine::registry::BuiltinRegistry::build(crate::test_support::session_arc())
            .signatures()
            .filter(|s| s.definition.path().0.modules == ["$intrinsic"])
    {
        registry
            .signatures
            .insert(signature.definition.path().clone(), signature.clone());
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
            path(name),
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
            path(name),
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
            path(name),
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
        let mut signature = type_signature(name, vec![], None, None);
        if matches!(name, "File" | "AtomicFile") {
            if let SignatureKind::Type(definition) = &mut signature.kind {
                definition.authority =
                    AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into());
                definition.resource = true;
            }
        }
        if matches!(name, "ScalarView" | "PositionView") {
            let core =
                hirsel_engine::registry::BuiltinRegistry::build(crate::test_support::session_arc());
            let SignatureKind::Type(published) = &core.get(&path(name)).unwrap().kind else {
                unreachable!()
            };
            let SignatureKind::Type(t) = &mut signature.kind else {
                unreachable!()
            };
            t.loop_source = published.loop_source.clone();
        }
        registry.signatures.insert(path(name), Arc::new(signature));
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
                open: path("std@1/unicode.graphemes_open"),
                next: path("std@1/unicode.graphemes_next"),
                option: path("Option"),
            });
        }
        registry.signatures.insert(path(name), Arc::new(signature));
    }
    // Bootstrap nominal headers so recursive graphs need no guessed callee bodies.
    let declarations = "type Array[T] { value: T }\ntype List[T] { value: T }\ntype Inline[T,n:size] { value: T }\ntype Iter[T] { value: T }\ntype Map[K:key,V] { key: K, value: V }\ntype Set[T:key] { value: T }\ntype SortedMap[K,V] { key: K, value: V }\ntype Ptr[T] { value: T }\ntype Option[T] = None | Some(T)\ntype Result[T,E] = Ok(T) | Error(E)\ntype CliError = Usage(ArgError) | Files(FileError) | Output(OutputError)\ntype GrantError = NotGranted()\ntype ArgError = MissingArgument()\ntype CancelCause = Requested | Parent | ChildFailed\ntype CancelError = Canceled(CancelCause)\ntype ChannelError = Closed | Canceled(CancelCause)\ntype FileError = FileFault(u32) | Canceled(CancelCause) | NotGranted()\ntype OutputError = OutputFault(u32) | Canceled(CancelCause)\ntype FloatIntError = NotFinite | OutOfRange\ntype OutOfSpace = OutOfSpace()\ntype Eq[T] { equal: fn(T,T) -> bool uses {} }\ntype Order[T] { compare: fn(T,T) -> i32 uses {} }\ntype Hasher[T] { hash: fn(T,u64) -> u64 uses {} }\n";
    let (mut types, _, ds) = parse(declarations, &[]);
    for syntax in &mut types.items {
        let item = &mut Arc::make_mut(syntax).item;
        item.definition = path(&item.definition.0.name);
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
    for signature in registry.signatures.values_mut() {
        Arc::make_mut(signature).metadata.visibility = ast::Visibility::Public;
    }
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
        let mut p = path(name);
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
        let code = if name.starts_with("sorted_map.") {
            code.replacen("[K,", "[K: key,", 1)
        } else if name.starts_with("simd.") {
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
        Arc::make_mut(registry.signatures.get_mut(&p).unwrap())
            .metadata
            .visibility = ast::Visibility::Public;
        // SEM-4 constructor semantics, supplied by the real core registry too.
        if matches!(name, "iter.from" | "iter.map") {
            let signature = Arc::make_mut(registry.signatures.get_mut(&p).unwrap());
            if let SignatureKind::Function(f) = &mut signature.kind {
                let mut result = session().row(f.result).clone();
                result.erased_captures = Some(ErasedCaptures::FromArguments(
                    vec![
                        ParameterRef::bound(ParamId(0)).into(),
                        ParameterRef::bound(ParamId(1)).into(),
                    ]
                    .into(),
                ));
                f.result = session().intern(result);
            }
        }
        // E currently over-restricts SortedMap: restore the section-22 header.
        if name.starts_with("sorted_map.") || name.starts_with("simd.") {
            let s = registry.signatures.get_mut(&p).unwrap();
            if let SignatureKind::Function(f) = &mut Arc::make_mut(s).kind {
                if let GenericParameter::Type { bounds, .. } = &mut f.generics[0] {
                    bounds.clear();
                }
            }
        }
    }
    let mut signature = type_signature("$intrinsic.heap", vec![], None, None);
    let mut result = row(TypeKind::Named {
        definition: ast::DefId::intern(&path("Arena")),
        arguments: vec![].into(),
    });
    result.allocation_target = Some(AllocationTarget::Heap);
    result.modes.sendability = Sendability::Sendable;
    let result = session().intern(result);
    signature.kind = SignatureKind::Function(FunctionType {
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
    registry
        .signatures
        .insert(signature.definition.path().clone(), Arc::new(signature));
    registry
}
pub(super) fn normalize_file(
    file: &ast::SyntaxFile,
    registry: &mut Registry,
) -> Result<(), String> {
    let metadata = Metadata {
        tasks_capability: Some(path("Tasks")),
        scope_capability: Some(path("Scope")),
        heap: Some(HeapRole {
            arena: path("Arena"),
            operation: path("$intrinsic.heap"),
        }),
        result_type: Some(path("Result")),
        unsafe_capability: Some(path("Unsafe")),
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
                    memo: session_memo(),
                    syntax,
                    context: &ctx,
                    resolution: &r,
                    metadata: &metadata,
                    declaration_sites: &metadata.declarations,
                    stage: None,
                },
                registry,
            ) {
                Ok(out) => {
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
