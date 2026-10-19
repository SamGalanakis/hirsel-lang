use hirsel_diag::{Diagnostic, RuleId};
use hirsel_engine::*;
use hirsel_intern::Symbol;
use hirsel_span::{FileId, SourceMap, Span};
use hirsel_syntax::Origin;
use hirsel_syntax::*;
use hirsel_types::*;
use std::{cell::RefCell, rc::Rc, sync::Arc};

fn out<T>(value: T) -> QueryOutput<T> {
    QueryOutput {
        value,
        diagnostics: vec![],
        dependencies: vec![],
    }
}
fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: default_context().package,
        modules: vec![],
        name: name.into(),
    })
}
fn error(span: Span) -> QueryError {
    QueryError::Diagnostics(vec![Diagnostic::error(
        const { RuleId::of("LANG-1") },
        span,
        "fixture rejection",
    )])
}
#[derive(Clone)]
struct Fake {
    events: Rc<RefCell<Vec<String>>>,
    reject: Option<&'static str>,
    missing_lookup: bool,
}
impl Fake {
    fn event(&self, phase: &str) {
        self.events.borrow_mut().push(phase.into());
    }
    fn fail(&self, phase: &str, span: Span) -> Result<(), QueryError> {
        self.event(phase);
        if self.reject == Some(phase) {
            Err(error(span))
        } else {
            Ok(())
        }
    }
}
impl LexerPhase for Fake {
    fn lex(
        &self,
        file: FileId,
        sources: &SourceMap,
    ) -> Result<QueryOutput<Vec<Token>>, QueryError> {
        self.event("lex");
        for line in sources.file(file).unwrap().text().lines() {
            Symbol::intern(line.split_whitespace().nth(1).unwrap());
        }
        let mut result = out(vec![]);
        if self.reject == Some("lex") {
            let QueryError::Diagnostics(d) = error(Span::new(file, 0, 1).unwrap()) else {
                unreachable!()
            };
            result.diagnostics = d;
        }
        Ok(result)
    }
}
impl ParserPhase for Fake {
    fn parse(
        &self,
        file: FileId,
        tokens: Vec<Token>,
        context: &FileContext,
        sources: &SourceMap,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        self.event("parse");
        let text = sources.file(file).unwrap().text();
        let mut tree = SyntaxTree::default();
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            let name = line.split_whitespace().nth(1).unwrap();
            let body_start = offset + line.find('{').unwrap();
            let body = ExprId(tree.expressions.len() as u32);
            let origin = Origin {
                span: Span::new(file, offset as u32, (offset + line.len()) as u32).unwrap(),
                tokens: TokenRange::default(),
            };
            tree.expressions.push(Expr {
                origin: Origin {
                    span: Span::new(file, body_start as u32, origin.span.end).unwrap(),
                    tokens: TokenRange::default(),
                },
                kind: ExprKind::Literal(Literal::Bool(true)),
            });
            tree.items.push(Item {
                name_span: Default::default(),

                origin,
                owned_tokens: TokenRange::default(),
                name: Symbol::intern("item"),
                definition: DefPath(ItemPath {
                    package: context.package.clone(),
                    modules: context.modules.clone(),
                    name: name.into(),
                }),
                text_hash: ContentHash::of_bytes(line.as_bytes()),
                visibility: Visibility::Private,
                attributes: vec![],
                kind: ItemKind::Function {
                    signature: FunctionSyntax::default(),
                    body: Some(body),
                    abi: Some(hirsel_syntax::ForeignAbi {
                        literal: line[..line.find('{').unwrap()].into(),
                        contract: None,
                    }),
                },
                generated: None,
            });
            offset += line.len();
        }
        let roots = (0..tree.items.len()).map(|n| ItemId(n as u32)).collect();
        let mut result = out(SyntaxFile {
            has_syntax_errors: false,

            file,
            tree,
            tokens,
            roots,
        });
        if self.reject == Some("parse") {
            let QueryError::Diagnostics(d) = error(Span::new(file, 0, 1).unwrap()) else {
                unreachable!()
            };
            result.diagnostics = d;
        }
        Ok(result)
    }
}
impl ResolverPhase for Fake {
    fn resolve(
        &self,
        file: Arc<SyntaxFile>,
        item: ItemId,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Resolution>, QueryError> {
        let item = &file.tree.items[item.0 as usize];
        self.fail("resolve", item.origin.span)?;
        if item.definition == path("b") {
            provider.signature(&path("a"))?;
        }
        if self.missing_lookup {
            let name = Symbol::intern("missing");
            let _ = provider.lookup(
                &default_context(),
                &Path {
                    package: None,
                    segments: vec![name],
                },
            );
            let _ = provider.signature(&path("missing"));
            return Err(error(item.origin.span));
        }
        Ok(out(Resolution::default()))
    }
}
impl SignaturePhase for Fake {
    fn signature(
        &self,
        file: Arc<SyntaxFile>,
        item: ItemId,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let item = &file.tree.items[item.0 as usize];
        self.fail("signature", item.origin.span)?;
        // A declared fixture signature depends on the header, never the body.
        let fingerprint = match &item.kind {
            ItemKind::Function {
                abi: Some(header), ..
            } => ContentHash::of_bytes(header.literal.as_bytes()),
            _ => unreachable!(),
        };
        let mut types = TypeStore::default();
        let unit = types.push(Type::with_kind(TypeKind::Tuple(vec![].into())));
        Ok(out(Signature {
            metadata: Default::default(),
            definition: item.definition.clone(),
            types,
            kind: SignatureKind::Const {
                ty: unit,
                generics: Default::default(),
            },
            fingerprint,
        }))
    }
}
impl CheckerPhase for Fake {
    fn check(
        &self,
        file: Arc<SyntaxFile>,
        item: ItemId,
        resolution: Resolution,
        _signature: &Signature,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        self.fail("check", file.tree.items[item.0 as usize].origin.span)?;
        let mut types = _signature.types.clone();
        let unit = match _signature.kind {
            SignatureKind::Const { ty: unit, .. } => unit,
            SignatureKind::Function(ref f) => f.result,
            _ => unreachable!(),
        };
        let boolean = types.push(Type {
            kind: TypeKind::Primitive(hirsel_types::Primitive::Bool),
            ..types.get(unit).unwrap().clone()
        });
        Ok(out(TypedBody {
            file,
            item,
            checked: CheckedBody {
                tails: vec![],
                places: vec![],
                calls: vec![],
                bindings: vec![],
                patterns: vec![],

                types,
                resolution,
                expressions: vec![],
                instantiations: vec![],
                closures: vec![],
            },
            plans: Default::default(),
            unit,
            boolean,
        }))
    }
}
impl ModesPhase for Fake {
    fn check_modes(
        &self,
        body: TypedBody,
        _signature: &Signature,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
        self.fail(
            "modes",
            body.file.tree.items[body.item.0 as usize].origin.span,
        )?;
        Ok(out(AnalyzedBody {
            typed: body,
            ownership: Default::default(),
            results: vec![],
            call_allocations: vec![],
            materialized: vec![],
            staged: vec![],
        }))
    }
}
impl LoweringPhase for Fake {
    fn lower(
        &self,
        body: AnalyzedBody,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        let file = &body.typed.file;
        let item = body.typed.item;
        self.fail("lower", file.tree.items[item.0 as usize].origin.span)?;
        Ok(out(hirsel_tir::Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: signature.types.clone(),
            path: signature.definition.clone(),
            signature_hash: signature.fingerprint,
            bodies: if self.reject == Some("invalid-tir") {
                vec![]
            } else {
                vec![unit_body(
                    signature,
                    item,
                    file.tree.items[item.0 as usize].origin.span,
                )]
            },
            entry: hirsel_tir::BodyId(0),
            contracts: vec![],
        }))
    }
    fn lower_evaluation(
        &self,
        _: &AnalyzedBody,
        _: &Signature,
        _: ExprId,
        _: TypeId,
        _: &[(ExprId, hirsel_tir::Materialized)],
    ) -> Result<hirsel_tir::Definition, QueryError> {
        Err(QueryError::NotImplemented)
    }
    fn entry_body(&self) -> hirsel_tir::BodyId {
        hirsel_tir::BodyId(0)
    }
}
fn unit_body(signature: &Signature, item: ItemId, span: Span) -> hirsel_tir::Body {
    use hirsel_tir::*;
    let unit = match &signature.kind {
        SignatureKind::Const { ty: unit, .. } => *unit,
        SignatureKind::Function(function) => {
            assert!(function.parameters.is_empty());
            function.result
        }
        _ => unreachable!(),
    };
    Body {
        parameter_scope: NameSite::Item(item),
        locals: vec![Local {
            ty: unit,
            storage: Storage::ReturnSlot,
            scope: ScopeId(0),
            span,
        }],
        places: vec![Place {
            loan: None,
            binding: None,
            root: LocalId(0),
            projection: vec![].into(),
            ty: unit,
        }],
        scopes: vec![Scope { parent: None, span }],
        blocks: vec![Block {
            instructions: vec![Instruction {
                kind: InstructionKind::Assign {
                    destination: PlaceId(0),
                    value: RvalueId(0),
                },
                span,
                on_failure: CleanupId(0),
            }],
            terminator: Terminator::Return {
                value: Operand {
                    place: PlaceId(0),
                    usage: UseKind::Take,
                },
                cleanup: CleanupId(0),
            },
            span,
        }],
        rvalues: vec![Rvalue::Constant(Constant::Unit)],
        cleanup_nodes: vec![],
        cleanups: vec![Cleanup::default()],
        entry: BlockId(0),
        parameters: vec![],
        result: unit,
        tail_sites: vec![],
        matches: vec![],
    }
}
fn make_engine(
    reject: Option<&'static str>,
    missing_lookup: bool,
) -> (QueryEngine, Rc<RefCell<Vec<String>>>) {
    let events = Rc::new(RefCell::new(vec![]));
    let fake = Rc::new(Fake {
        events: events.clone(),
        reject,
        missing_lookup,
    });
    let adapters = PhaseAdapters {
        lexer: Some(fake.clone()),
        parser: Some(fake.clone()),
        resolver: Some(fake.clone()),
        signature: Some(fake.clone()),
        checker: Some(fake.clone()),
        modes: Some(fake.clone()),
        lowering: Some(fake),
        ..Default::default()
    };
    let engine = QueryEngine::new(default_environment(), adapters);
    (engine, events)
}
fn parse(engine: &mut QueryEngine, text: &str) -> FileId {
    let file = engine.sources.add("fixture.hir", text.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    file
}
#[test]
fn invalid_tir_is_never_published_and_operational_failures_retry() {
    hirsel_engine::with_compiler_stack(
        invalid_tir_is_never_published_and_operational_failures_retry_on_compiler_thread,
    );
}
fn invalid_tir_is_never_published_and_operational_failures_retry_on_compiler_thread() {
    let (mut engine, events) = make_engine(Some("invalid-tir"), false);
    parse(&mut engine, "fn a () { 1 }\n");
    for _ in 0..2 {
        let failure = engine.lower_to_tir(&path("a")).unwrap_err();
        assert!(
            matches!(failure, QueryError::Operational(ref e) if e.operation == "TIR validation"),
            "{failure:?}"
        );
    }
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|phase| phase.as_str() == "lower")
            .count(),
        2
    );
}
#[test]
fn phases_run_in_order_and_stop_at_every_error_boundary() {
    hirsel_engine::with_compiler_stack(
        phases_run_in_order_and_stop_at_every_error_boundary_on_compiler_thread,
    );
}
fn phases_run_in_order_and_stop_at_every_error_boundary_on_compiler_thread() {
    let phases = [
        "lex",
        "parse",
        "resolve",
        "signature",
        "check",
        "modes",
        "lower",
    ];
    for stop in 0..phases.len() {
        let (mut engine, events) = make_engine(Some(phases[stop]), false);
        let file = engine
            .sources
            .add("fixture.hir", "fn a () { 1 }\n".into())
            .unwrap();
        assert!(matches!(
            check_snapshot(&mut engine, file, &default_context()),
            Err(QueryError::Diagnostics(_))
        ));
        assert_eq!(*events.borrow(), phases[..=stop]);
    }
    let (mut engine, events) = make_engine(None, false);
    let file = engine
        .sources
        .add("fixture.hir", "fn a () { 1 }\n".into())
        .unwrap();
    assert!(
        check_snapshot(&mut engine, file, &default_context())
            .unwrap()
            .is_empty()
    );
    assert_eq!(*events.borrow(), phases);
}
#[test]
fn changing_one_body_recomputes_only_that_definition() {
    hirsel_engine::with_compiler_stack(
        changing_one_body_recomputes_only_that_definition_on_compiler_thread,
    );
}
fn changing_one_body_recomputes_only_that_definition_on_compiler_thread() {
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a () { 1 }\nfn b () { 2 }\n");
    engine.lower_to_tir(&path("a")).unwrap();
    engine.lower_to_tir(&path("b")).unwrap();
    parse(&mut engine, "fn a () { 12345 }\nfn b () { 2 }\n");
    engine.lower_to_tir(&path("a")).unwrap();
    engine.lower_to_tir(&path("b")).unwrap();
    for kind in [
        QueryKind::Lex,
        QueryKind::Parse,
        QueryKind::Resolve,
        QueryKind::Check,
        QueryKind::Modes,
        QueryKind::Lower,
    ] {
        assert_eq!(engine.recompute_count(&path("a"), kind), 2, "{kind:?}");
        assert_eq!(engine.recompute_count(&path("b"), kind), 1, "{kind:?}");
    }
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Signature), 1);
    assert_eq!(engine.recompute_count(&path("b"), QueryKind::Signature), 1);
}
#[test]
fn changing_a_signature_recomputes_its_users() {
    hirsel_engine::with_compiler_stack(
        changing_a_signature_recomputes_its_users_on_compiler_thread,
    );
}
fn changing_a_signature_recomputes_its_users_on_compiler_thread() {
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a () { 1 }\nfn b () { 2 }\n");
    engine.lower_to_tir(&path("b")).unwrap();
    parse(&mut engine, "fn a (changed) { 1 }\nfn b () { 2 }\n");
    engine.lower_to_tir(&path("b")).unwrap();
    for kind in [
        QueryKind::Resolve,
        QueryKind::Check,
        QueryKind::Modes,
        QueryKind::Lower,
    ] {
        assert_eq!(engine.recompute_count(&path("b"), kind), 2, "{kind:?}");
    }
}
#[test]
fn failed_provider_reads_survive_rejection_and_manifest_changes_invalidate() {
    hirsel_engine::with_compiler_stack(
        failed_provider_reads_survive_rejection_and_manifest_changes_invalidate_on_compiler_thread,
    );
}
fn failed_provider_reads_survive_rejection_and_manifest_changes_invalidate_on_compiler_thread() {
    let (mut engine, _) = make_engine(None, true);
    parse(&mut engine, "fn a () { 1 }\n");
    assert!(engine.resolve_definition(&path("a")).is_err());
    let reads = engine
        .dependency_reads(&path("a"), QueryKind::Resolve)
        .unwrap();
    assert_eq!(reads.lookups, vec![(path("missing"), false)]);
    assert_eq!(reads.failed_signatures, vec![path("missing")]);
    assert!(engine.resolve_definition(&path("a")).is_err());
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Resolve), 1);
    parse(&mut engine, "fn a () { 1 }\nfn missing () { 3 }\n");
    assert!(engine.resolve_definition(&path("a")).is_err());
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Resolve), 2);
    let reads = engine
        .dependency_reads(&path("a"), QueryKind::Resolve)
        .unwrap();
    assert_eq!(reads.lookups, vec![(path("missing"), true)]);
    assert_eq!(reads.signatures[0].definition, path("missing"));
}
#[test]
fn missing_stages_remain_unavailable_and_diagnostic_sources_survive_edits() {
    hirsel_engine::with_compiler_stack(
        missing_stages_remain_unavailable_and_diagnostic_sources_survive_edits_on_compiler_thread,
    );
}
fn missing_stages_remain_unavailable_and_diagnostic_sources_survive_edits_on_compiler_thread() {
    let (mut engine, _) = make_engine(Some("resolve"), false);
    let old = parse(&mut engine, "fn a () { 1 }\n");
    let Err(QueryError::Diagnostics(diagnostics)) = engine.resolve_definition(&path("a")) else {
        panic!("expected errors");
    };
    parse(&mut engine, "fn a () { 123 }\n");
    let output = CheckOutput {
        completion: Default::default(),

        sources: engine.sources.clone(),
        diagnostics,
        unsafe_ffi: vec![],
    };
    assert_eq!(output.sources.file(old).unwrap().text(), "fn a () { 1 }\n");
    assert_eq!(
        output
            .sources
            .location(output.diagnostics[0].primary)
            .unwrap()
            .line,
        1
    );
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a () { 1 }\n");
    engine.adapters.checker = None;
    assert!(matches!(
        engine.check_definition(&path("a")),
        Err(QueryError::NotImplemented)
    ));
    assert_eq!(engine.unavailable, Some(QueryKind::Check));
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a () { 1 }\n");
    engine.adapters.modes = None;
    assert!(matches!(
        engine.lower_to_tir(&path("a")),
        Err(QueryError::NotImplemented)
    ));
    assert_eq!(engine.unavailable, Some(QueryKind::Modes));
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a () { 1 }\n");
    engine.adapters.lowering = None;
    assert!(matches!(
        engine.lower_to_tir(&path("a")),
        Err(QueryError::NotImplemented)
    ));
    assert_eq!(engine.unavailable, Some(QueryKind::Lower));
}

struct ContractSignatures;
impl SignaturePhase for ContractSignatures {
    fn signature(
        &self,
        file: Arc<SyntaxFile>,
        item: ItemId,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let item = &file.tree.items[item.0 as usize];
        let ItemKind::Function {
            abi: Some(header), ..
        } = &item.kind
        else {
            unreachable!()
        };
        let mut fingerprint = ContentHash::of_bytes(header.literal.as_bytes());
        let mut contracts = vec![];
        if item.definition == path("a") {
            let used = provider.signature(&path("contract"))?;
            fingerprint = ContentHash::of_parts(
                "fixture.contract.signature",
                [fingerprint.0.as_slice(), used.fingerprint.0.as_slice()],
            );
            contracts.push(hirsel_types::Contract {
                kind: hirsel_syntax::ContractKind::Requires,
                ordinal: 0,
                code_hash: fingerprint,
            });
        }
        let mut types = TypeStore::default();
        let result = types.push(hirsel_types::Type {
            drop_allocation: hirsel_types::DropAllocation::MayAllocate,
            ..hirsel_types::Type::with_kind(TypeKind::Tuple(vec![].into()))
        });
        Ok(out(Signature {
            metadata: Default::default(),

            definition: item.definition.clone(),
            types,
            fingerprint,
            kind: SignatureKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: vec![].into(),
                result,
                error: None,
                allocation: Allocation::MayAllocate,
                authority: AuthoritySummary::inert(),
                uses: Uses::Unrestricted,
                contracts: contracts.into(),
                incomplete: false,
                foreign: None,
            }),
        }))
    }
}
#[test]
fn contract_signature_reads_invalidate_the_owner_and_its_users() {
    hirsel_engine::with_compiler_stack(
        contract_signature_reads_invalidate_the_owner_and_its_users_on_compiler_thread,
    );
}
fn contract_signature_reads_invalidate_the_owner_and_its_users_on_compiler_thread() {
    let (mut engine, _) = make_engine(None, false);
    engine.adapters.signature = Some(Rc::new(ContractSignatures));
    parse(
        &mut engine,
        "fn a () { 1 }\nfn b () { 2 }\nfn contract () { 3 }\n",
    );
    engine.lower_to_tir(&path("b")).unwrap();
    let reads = engine
        .dependency_reads(&path("a"), QueryKind::Signature)
        .unwrap();
    assert_eq!(reads.signatures[0].definition, path("contract"));
    let previous = engine.item_signature(&path("a")).unwrap().value.fingerprint;
    parse(
        &mut engine,
        "fn a () { 1 }\nfn b () { 2 }\nfn contract (changed) { 3 }\n",
    );
    engine.lower_to_tir(&path("b")).unwrap();
    assert_ne!(
        engine.item_signature(&path("a")).unwrap().value.fingerprint,
        previous
    );
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Signature), 2);
    assert_eq!(engine.recompute_count(&path("b"), QueryKind::Check), 2);
}

#[test]
fn failed_signature_changes_invalidate_a_rejected_user() {
    hirsel_engine::with_compiler_stack(
        failed_signature_changes_invalidate_a_rejected_user_on_compiler_thread,
    );
}
fn failed_signature_changes_invalidate_a_rejected_user_on_compiler_thread() {
    let (mut engine, _) = make_engine(Some("signature"), false);
    parse(&mut engine, "fn a () { 1 }\nfn b () { 2 }\n");
    assert!(engine.resolve_definition(&path("b")).is_err());
    let old_key = engine
        .dependency_reads(&path("b"), QueryKind::Resolve)
        .unwrap()
        .failed_signature_keys
        .clone();
    parse(&mut engine, "fn a (changed) { 1 }\nfn b () { 2 }\n");
    assert!(engine.resolve_definition(&path("b")).is_err());
    assert_eq!(engine.recompute_count(&path("b"), QueryKind::Resolve), 2);
    assert_ne!(
        engine
            .dependency_reads(&path("b"), QueryKind::Resolve)
            .unwrap()
            .failed_signature_keys,
        old_key
    );
}
struct ObserveDuplicates;
impl SignaturePhase for ObserveDuplicates {
    fn signature(
        &self,
        _file: Arc<SyntaxFile>,
        _item: ItemId,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        unreachable!("candidate-aware entry point must be used")
    }
    fn signature_items(
        &self,
        candidates: &[(Arc<SyntaxFile>, ItemId)],
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        assert_eq!(candidates.len(), 2);
        let (later, item) = &candidates[1];
        Err(QueryError::Diagnostics(vec![Diagnostic::error(
            const { RuleId::of("LANG-7") },
            later.tree.items[item.0 as usize].origin.span,
            "fixture overloading",
        )]))
    }
}
#[test]
fn duplicate_items_keep_one_identity_and_reach_the_signature_phase() {
    hirsel_engine::with_compiler_stack(
        duplicate_items_keep_one_identity_and_reach_the_signature_phase_on_compiler_thread,
    );
}
fn duplicate_items_keep_one_identity_and_reach_the_signature_phase_on_compiler_thread() {
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a () { 1 }\n");
    let duplicate = engine
        .sources
        .add("other.hir", "fn a () { 2 }\n".into())
        .unwrap();
    engine.parse_file(duplicate, &default_context()).unwrap();
    let candidates = engine.definition_items(&path("a")).unwrap();
    assert_eq!(candidates.len(), 2);
    assert_ne!(candidates[0].0.file, candidates[1].0.file);
    for (file, item) in candidates {
        assert_eq!(file.tree.items[item.0 as usize].definition, path("a"));
    }
    engine.adapters.signature = Some(Rc::new(ObserveDuplicates));
    let Err(QueryError::Diagnostics(errors)) = engine.item_signature(&path("a")) else {
        panic!("signature phase owns overloading");
    };
    assert_eq!(errors[0].rule.as_str(), "LANG-7");
    assert_eq!(errors[0].primary.file, duplicate);
}

#[test]
fn environment_revisions_invalidate_semantic_queries() {
    hirsel_engine::with_compiler_stack(
        environment_revisions_invalidate_semantic_queries_on_compiler_thread,
    );
}
fn environment_revisions_invalidate_semantic_queries_on_compiler_thread() {
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a () { 1 }\n");
    engine.lower_to_tir(&path("a")).unwrap();
    engine.environment.interface_version += 1;
    engine.lower_to_tir(&path("a")).unwrap();
    engine.environment.compiler_version.push_str("-new");
    engine.lower_to_tir(&path("a")).unwrap();
    engine.environment.spec_hash = ContentHash::of_bytes(b"new spec");
    engine.lower_to_tir(&path("a")).unwrap();
    engine.environment.package_manifest_hash = ContentHash::of_bytes(b"new manifest");
    engine.lower_to_tir(&path("a")).unwrap();
    engine.environment.builtin_signatures_hash = ContentHash::of_bytes(b"new builtins");
    engine.lower_to_tir(&path("a")).unwrap();
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Check), 6);
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Signature), 6);
}

#[test]
fn provider_results_share_graphs_and_manifest_misses_have_revisions() {
    hirsel_engine::with_compiler_stack(
        provider_results_share_graphs_and_manifest_misses_have_revisions_on_compiler_thread,
    );
}
fn provider_results_share_graphs_and_manifest_misses_have_revisions_on_compiler_thread() {
    let (mut engine, _) = make_engine(None, false);
    parse(&mut engine, "fn a {}\n");
    let a = engine.signature(&path("a")).unwrap();
    let b = engine.signature(&path("a")).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    let a = engine.definition(&path("a")).unwrap();
    let b = engine.definition(&path("a")).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    let lookup = Path {
        package: None,
        segments: vec![Symbol::intern("missing")],
    };
    let before = engine.lookup(&default_context(), &lookup);
    assert!(before.result.is_err());
    parse(&mut engine, "fn a {}\nfn missing {}\n");
    let after = engine.lookup(&default_context(), &lookup);
    assert!(after.result.is_ok());
    assert_ne!(before.manifest, after.manifest);
}
#[test]
fn operational_signature_failures_retry_and_are_not_rejection_cache_entries() {
    hirsel_engine::with_compiler_stack(
        operational_signature_failures_retry_and_are_not_rejection_cache_entries_on_compiler_thread,
    );
}
fn operational_signature_failures_retry_and_are_not_rejection_cache_entries_on_compiler_thread() {
    struct Flaky {
        original: Rc<dyn SignaturePhase>,
        calls: std::cell::Cell<u32>,
    }
    impl SignaturePhase for Flaky {
        fn signature(
            &self,
            file: Arc<SyntaxFile>,
            item: ItemId,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<Signature>, QueryError> {
            self.calls.set(self.calls.get() + 1);
            if self.calls.get() == 1 {
                return Err(QueryError::Operational(OperationalFailure {
                    operation: "signature provider".into(),
                    message: "temporarily offline".into(),
                }));
            }
            self.original.signature(file, item, provider)
        }
    }
    let (mut engine, _) = make_engine(None, false);
    let adapter = Rc::new(Flaky {
        original: engine.adapters.signature.take().unwrap(),
        calls: std::cell::Cell::new(0),
    });
    engine.adapters.signature = Some(adapter.clone());
    parse(&mut engine, "fn a {}\n");
    assert!(matches!(
        engine.signature(&path("a")),
        Err(QueryError::Operational(_))
    ));
    assert!(engine.signature(&path("a")).is_ok());
    assert!(engine.signature(&path("a")).is_ok());
    assert_eq!(adapter.calls.get(), 2);
}
#[test]
fn partial_check_preserves_diagnostics_and_reports_the_missing_phase() {
    hirsel_engine::with_compiler_stack(
        partial_check_preserves_diagnostics_and_reports_the_missing_phase_on_compiler_thread,
    );
}
fn partial_check_preserves_diagnostics_and_reports_the_missing_phase_on_compiler_thread() {
    let (mut engine, _) = make_engine(Some("resolve"), false);
    engine.adapters.checker = None;
    let file = parse(&mut engine, "fn a {}\n");
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.diagnostics.len(), 1);
    assert_eq!(
        output.completion,
        CheckCompletion::Incomplete {
            phase: QueryKind::Check
        }
    );
    assert!(
        output
            .sources
            .location(output.diagnostics[0].primary)
            .is_some()
    );
}
