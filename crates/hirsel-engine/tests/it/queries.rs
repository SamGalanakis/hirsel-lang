use hirsel_diag::{Diagnostic, RuleId};
use hirsel_engine::*;
use hirsel_intern::Symbol;
use hirsel_span::{FileId, SourceMap, Span};
use hirsel_syntax::Origin;
use hirsel_syntax::*;
use hirsel_types::*;
use std::sync::{Arc, Mutex};

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
    events: Arc<Mutex<Vec<String>>>,
    reject: Option<&'static str>,
    missing_lookup: bool,
}
impl Fake {
    fn event(&self, phase: &str) {
        self.events.lock().unwrap().push(phase.into());
    }
    fn fail(&self, phase: &str, span: Span) -> Result<(), QueryError> {
        self.event(phase);
        if self.reject == Some("resolve+signature") && matches!(phase, "resolve" | "signature") {
            return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                const { RuleId::of("LANG-1") },
                span,
                format!("{phase} fixture rejection"),
            )]));
        }
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
            let anchor = sources.file_anchor(file);
            let QueryError::Diagnostics(d) = error(Span::new(anchor, 0, 1).unwrap()) else {
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
        sources: &mut SourceMap,
    ) -> Result<QueryOutput<SyntaxFile>, QueryError> {
        self.event("parse");
        let text = sources.file(file).unwrap().text().to_owned();
        let mut items = vec![];
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            // Each line is one item, anchored at its start.
            let anchor = sources.item_anchor(file, offset as u32);
            let name = line.split_whitespace().nth(1).unwrap();
            let body_start = line.find('{').unwrap();
            let mut tree = SyntaxTree::default();
            let body = ExprId(0);
            let origin = Origin {
                span: Span::new(anchor, 0, line.len() as u32).unwrap(),
                tokens: TokenRange::default(),
            };
            tree.expressions.push(Expr {
                origin: Origin {
                    span: Span::new(anchor, body_start as u32, origin.span.end).unwrap(),
                    tokens: TokenRange::default(),
                },
                kind: ExprKind::Literal(Literal::Bool(true)),
            });
            let item = Item {
                name_span: Default::default(),
                origin,
                name: Symbol::intern("item"),
                definition: DefPath(ItemPath {
                    package: context.package.clone(),
                    modules: context.modules.clone(),
                    name: name.into(),
                }),
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
            };
            items.push(Arc::new(ItemSyntax {
                item,
                tree,
                anchor,
                text_hash: ContentHash::of_bytes(line.as_bytes()),
                occurrence: 0,
            }));
            offset += line.len();
        }
        let mut result = out(SyntaxFile {
            has_syntax_errors: false,
            file,
            items,
            tokens,
        });
        if self.reject == Some("parse") {
            let anchor = sources.file_anchor(file);
            let QueryError::Diagnostics(d) = error(Span::new(anchor, 0, 1).unwrap()) else {
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
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        _declarations: &Declarations,
        _products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Resolution>, QueryError> {
        let item = &syntax.item;
        self.fail("resolve", item.origin.span)?;
        if item.definition == path("b") {
            provider.signature(DefId::intern(&path("a")))?;
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
            let _ = provider.signature(DefId::intern(&path("missing")));
            return Err(error(item.origin.span));
        }
        Ok(out(Resolution::default()))
    }
}
impl SignaturePhase for Fake {
    fn signature(
        &self,
        t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        _: &Declarations,
        _: &mut PhaseProducts,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let item = &syntax.item;
        self.fail("signature", item.origin.span)?;
        // A declared fixture signature depends on the header, never the body.
        let fingerprint = match &item.kind {
            ItemKind::Function {
                abi: Some(header), ..
            } => ContentHash::of_bytes(header.literal.as_bytes()),
            _ => unreachable!(),
        };
        let unit = t.intern(Type::with_kind(TypeKind::Tuple(vec![].into())));
        Ok(out(Signature {
            metadata: Default::default(),
            definition: DefId::intern(&item.definition),
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
        t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        resolution: Resolution,
        _signature: &Signature,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        self.fail("check", syntax.item.origin.span)?;
        let unit = match _signature.kind {
            SignatureKind::Const { ty: unit, .. } => unit,
            SignatureKind::Function(ref f) => f.result,
            _ => unreachable!(),
        };
        let boolean = t.intern(Type {
            kind: TypeKind::Primitive(hirsel_types::Primitive::Bool),
            ..t.row(unit).clone()
        });
        Ok(out(TypedBody {
            checked: CheckedBody {
                lexical: syntax.owner(),
                tails: vec![],
                places: vec![],
                calls: vec![],
                bindings: vec![],
                patterns: vec![],
                resolution,
                expressions: vec![],
                instantiations: vec![],
                closures: vec![],
            },
            syntax,
            plans: Default::default(),
            unit,
            boolean,
        }))
    }
}
impl ModesPhase for Fake {
    fn check_modes(
        &self,
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        body: Arc<TypedBody>,
        _signature: &Signature,
        _: &mut PhaseProducts,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
        self.fail("modes", body.syntax.item.origin.span)?;
        Ok(out(AnalyzedBody {
            typed: body,
            ownership: Default::default(),
            results: vec![],
            call_allocations: vec![],
        }))
    }
}
impl LoweringPhase for Fake {
    fn lower(
        &self,
        t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        _comptime: &Comptime,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        let syntax = &body.typed.syntax;
        self.fail("lower", syntax.item.origin.span)?;
        let mut definition = hirsel_tir::Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: Arc::from([]),
            path: signature.definition,
            inferred_unique: Default::default(),
            signature_hash: signature.fingerprint,
            bodies: if self.reject == Some("invalid-tir") {
                vec![]
            } else {
                vec![unit_body(signature, syntax, syntax.item.origin.span)]
            },
            entry: hirsel_tir::BodyId(0),
            contracts: vec![],
        };
        definition.types = hirsel_tir::definition_types(t, &definition);
        Ok(out(definition))
    }
    fn lower_evaluation(
        &self,
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        _: &AnalyzedBody,
        _staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
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
fn unit_body(signature: &Signature, syntax: &ItemSyntax, span: Span) -> hirsel_tir::Body {
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
        arena_scope_grant: None,
        scoped_parameters: Vec::new(),
        parameter_scope: syntax.owner().site(NameSite::Item),
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
        borrow_cursors: vec![],
        tail_sites: vec![],
        matches: vec![],
    }
}
fn make_engine(
    reject: Option<&'static str>,
    missing_lookup: bool,
) -> (QueryEngine, Arc<Mutex<Vec<String>>>) {
    let events = Arc::new(Mutex::new(vec![]));
    let fake = Arc::new(Fake {
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
            .lock()
            .unwrap()
            .iter()
            .filter(|phase| phase.as_str() == "lower")
            .count(),
        2
    );
}
/// `Verify::Off` is the one switch that skips V: the same invalid TIR lowers.
#[test]
fn invalid_tir_lowers_without_v() {
    hirsel_engine::with_compiler_stack(|| {
        let (mut engine, _) = make_engine(Some("invalid-tir"), false);
        engine.verify = Verify::Off;
        parse(&mut engine, "fn a () { 1 }\n");
        engine.lower_to_tir(&path("a")).unwrap();
    });
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
        let expected = match stop {
            0 => vec!["lex"],
            1 => vec!["lex", "parse"],
            2 | 3 => vec!["lex", "parse", "signature", "resolve"],
            _ => [
                vec!["lex", "parse", "signature", "resolve"],
                phases[4..=stop].to_vec(),
            ]
            .concat(),
        };
        assert_eq!(
            *events.lock().unwrap(),
            expected,
            "program rejects at {}",
            phases[stop]
        );
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
    assert_eq!(
        *events.lock().unwrap(),
        [
            "lex",
            "parse",
            "signature",
            "resolve",
            "check",
            "modes",
            "lower"
        ]
    );
}
/// Direct body queries still demand resolution before the owner signature;
/// program checks freeze signatures in S0 before running the same body stages.
#[test]
fn scalar_queries_preserve_phase_order_and_stop_at_every_error_boundary() {
    with_compiler_stack(|| {
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
            let result = engine
                .parse_file(file, &default_context())
                .and_then(|parsed| {
                    if !parsed.diagnostics.is_empty() {
                        return Err(QueryError::Diagnostics(parsed.diagnostics));
                    }
                    engine.lower_to_tir(&path("a"))
                });
            assert!(matches!(result, Err(QueryError::Diagnostics(_))));
            assert_eq!(*events.lock().unwrap(), phases[..=stop]);
        }
        let (mut engine, events) = make_engine(None, false);
        parse(&mut engine, "fn a () { 1 }\n");
        engine.lower_to_tir(&path("a")).unwrap();
        assert_eq!(*events.lock().unwrap(), phases);
    });
}
#[test]
fn frozen_signature_preflight_preserves_resolution_rejection_precedence() {
    with_compiler_stack(|| {
        for jobs in [1, 2, 8] {
            let (mut engine, events) = make_engine(Some("resolve+signature"), false);
            parse(&mut engine, "fn a () { 1 }\n");
            let outcomes =
                engine.mode_definitions(vec![path("a")], hirsel_intern::jobs::Jobs::new(jobs));
            assert_eq!(outcomes[0].phase, Some(QueryKind::Resolve));
            let Err(QueryError::Diagnostics(diagnostics)) = &outcomes[0].result else {
                panic!("expected resolution rejection");
            };
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].message, "resolve fixture rejection");
            assert_eq!(
                *events.lock().unwrap(),
                ["lex", "parse", "signature", "resolve"]
            );
        }
    });
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
    assert_eq!(reads.lookups, vec![(path("missing").into(), false)]);
    assert_eq!(reads.failed_signatures, vec![path("missing")]);
    assert!(engine.resolve_definition(&path("a")).is_err());
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Resolve), 1);
    parse(&mut engine, "fn a () { 1 }\nfn missing () { 3 }\n");
    assert!(engine.resolve_definition(&path("a")).is_err());
    assert_eq!(engine.recompute_count(&path("a"), QueryKind::Resolve), 2);
    let reads = engine
        .dependency_reads(&path("a"), QueryKind::Resolve)
        .unwrap();
    assert_eq!(reads.lookups, vec![(path("missing").into(), true)]);
    assert_eq!(*reads.signatures[0].definition.path(), path("missing"));
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
        t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        _declarations: &Declarations,
        _products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let item = &syntax.item;
        let ItemKind::Function {
            abi: Some(header), ..
        } = &item.kind
        else {
            unreachable!()
        };
        let mut fingerprint = ContentHash::of_bytes(header.literal.as_bytes());
        let mut contracts = vec![];
        if item.definition == path("a") {
            let used = provider.signature(DefId::intern(&path("contract")))?;
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
        let result = t.intern(hirsel_types::Type {
            drop_allocation: hirsel_types::DropAllocation::MayAllocate,
            ..hirsel_types::Type::with_kind(TypeKind::Tuple(vec![].into()))
        });
        Ok(out(Signature {
            metadata: Default::default(),
            definition: DefId::intern(&item.definition),
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
    engine.adapters.signature = Some(Arc::new(ContractSignatures));
    parse(
        &mut engine,
        "fn a () { 1 }\nfn b () { 2 }\nfn contract () { 3 }\n",
    );
    engine.lower_to_tir(&path("b")).unwrap();
    let reads = engine
        .dependency_reads(&path("a"), QueryKind::Signature)
        .unwrap();
    assert_eq!(*reads.signatures[0].definition.path(), path("contract"));
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
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        _syntax: Arc<ItemSyntax>,
        _: &Declarations,
        _: &mut PhaseProducts,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        unreachable!("candidate-aware entry point must be used")
    }
    fn signature_items(
        &self,
        _t: &hirsel_types::TypeInterner,
        _m: &hirsel_types::TypeMemo,
        candidates: &[Arc<ItemSyntax>],
        _: &Declarations,
        _: &mut PhaseProducts,
        _provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        assert_eq!(candidates.len(), 2);
        let later = &candidates[1];
        assert_eq!(later.occurrence, 1);
        Err(QueryError::Diagnostics(vec![Diagnostic::error(
            const { RuleId::of("LANG-7") },
            later.item.origin.span,
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
    let file = |syntax: &ItemSyntax| engine.sources.anchor(syntax.anchor).unwrap().0;
    assert_ne!(file(&candidates[0]), file(&candidates[1]));
    for (occurrence, syntax) in candidates.iter().enumerate() {
        assert_eq!(syntax.item.definition, path("a"));
        assert_eq!(syntax.occurrence, occurrence as u32);
    }
    engine.adapters.signature = Some(Arc::new(ObserveDuplicates));
    let Err(QueryError::Diagnostics(errors)) = engine.item_signature(&path("a")) else {
        panic!("signature phase owns overloading");
    };
    assert_eq!(errors[0].rule.as_str(), "LANG-7");
    assert_eq!(engine.sources.resolve(errors[0].primary).file, duplicate);
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
    let a = engine.signature(DefId::intern(&path("a"))).unwrap();
    let b = engine.signature(DefId::intern(&path("a"))).unwrap();
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
        original: Arc<dyn SignaturePhase>,
        calls: std::sync::atomic::AtomicU32,
    }
    impl SignaturePhase for Flaky {
        fn signature(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            syntax: Arc<ItemSyntax>,
            _declarations: &Declarations,
            _products: &mut PhaseProducts,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<Signature>, QueryError> {
            if self
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                == 0
            {
                return Err(QueryError::Operational(OperationalFailure {
                    operation: "signature provider".into(),
                    message: "temporarily offline".into(),
                }));
            }
            self.original
                .signature(t, m, syntax, _declarations, _products, provider)
        }
    }
    let (mut engine, _) = make_engine(None, false);
    let adapter = Arc::new(Flaky {
        original: engine.adapters.signature.take().unwrap(),
        calls: std::sync::atomic::AtomicU32::new(0),
    });
    engine.adapters.signature = Some(adapter.clone());
    parse(&mut engine, "fn a {}\n");
    assert!(matches!(
        engine.signature(DefId::intern(&path("a"))),
        Err(QueryError::Operational(_))
    ));
    assert!(engine.signature(DefId::intern(&path("a"))).is_ok());
    assert!(engine.signature(DefId::intern(&path("a"))).is_ok());
    assert_eq!(adapter.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
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

#[test]
fn signature_hits_track_later_duplicate_headers_and_positions() {
    hirsel_engine::with_compiler_stack(|| {
        let (mut engine, _) = make_engine(None, false);
        parse(&mut engine, "fn a () { 1 }\n");
        let duplicate = engine
            .sources
            .add("other.hir", "fn a () { 2 }\n".into())
            .unwrap();
        engine.parse_file(duplicate, &default_context()).unwrap();
        engine.adapters.signature = Some(Arc::new(ObserveDuplicates));
        let diagnose = |engine: &mut QueryEngine| {
            let Err(QueryError::Diagnostics(errors)) = engine.item_signature(&path("a")) else {
                panic!("duplicate signature must reject");
            };
            assert_eq!(errors[0].rule.as_str(), "LANG-7");
            errors[0].primary
        };
        let old = diagnose(&mut engine);
        assert_eq!(diagnose(&mut engine), old);
        assert_eq!(engine.recompute_count(&path("a"), QueryKind::Signature), 1);
        assert!(
            engine
                .memo_reads(&path("a"), QueryKind::Signature)
                .unwrap()
                .iter()
                .any(|(key, _)| *key
                    == hirsel_engine::memo::ReadKey::DeclarationCandidates(DefId::intern(&path(
                        "a"
                    ))))
        );
        // The first candidate is byte-identical. Only the later candidate's
        // header changes and its position moves behind an unrelated item.
        let duplicate = engine
            .sources
            .add(
                "other.hir",
                "fn unrelated () { 3 }\nfn a (changed) { 2 }\n".into(),
            )
            .unwrap();
        engine.parse_file(duplicate, &default_context()).unwrap();
        let current = diagnose(&mut engine);
        assert_ne!(current, old);
        let expected = engine.definition_items(&path("a")).unwrap()[1]
            .item
            .origin
            .span;
        assert_eq!(current, expected);
        assert_eq!(engine.sources.location(current).unwrap().line, 2);
        assert_eq!(engine.recompute_count(&path("a"), QueryKind::Signature), 2);
        assert_eq!(diagnose(&mut engine), current);
        engine.environment.interface_version += 1;
        assert_eq!(diagnose(&mut engine), current);
        assert_eq!(engine.recompute_count(&path("a"), QueryKind::Signature), 3);
    });
}

/// The batch API is the same memo computation at every worker count, including
/// per-input failures, typed receipts, reversed publication/input order and hits.
#[test]
fn frozen_frontier_preserves_reads_failures_and_warm_hits() {
    with_compiler_stack(|| {
        use hirsel_intern::jobs::Jobs;
        for jobs in [1, 2, 8] {
            for reject in [
                None,
                Some("resolve"),
                Some("signature"),
                Some("check"),
                Some("modes"),
            ] {
                let (mut serial, _) = make_engine(reject, false);
                let (mut parallel, _) = make_engine(reject, false);
                let text = "fn a () { 1 }\nfn b () { 2 }\n";
                parse(&mut serial, text);
                parse(&mut parallel, text);
                let paths = vec![path("b"), path("a"), path("b")];
                let expected: Vec<_> = paths
                    .iter()
                    .map(|path| serial.mode_definition(path))
                    .collect();
                let results = parallel.mode_definitions(paths.clone(), Jobs::new(jobs));
                for ((path, expected), actual) in paths.iter().zip(expected).zip(results) {
                    assert_eq!(&actual.definition, path);
                    let phase = match reject {
                        Some("resolve") => QueryKind::Resolve,
                        Some("signature") if path.0.name == "b" => QueryKind::Resolve,
                        Some("signature") => QueryKind::Signature,
                        Some("check") => QueryKind::Check,
                        _ => QueryKind::Modes,
                    };
                    assert_eq!(actual.phase, Some(phase));
                    match (expected, actual.result) {
                        (Ok(a), Ok(b)) => {
                            assert_eq!(a.dependencies, b.dependencies);
                            assert_eq!(a.diagnostics.len(), b.diagnostics.len());
                            assert_eq!(
                                a.value.typed.checked.resolution,
                                b.value.typed.checked.resolution
                            );
                        }
                        (Err(QueryError::Diagnostics(a)), Err(QueryError::Diagnostics(b))) => {
                            assert_eq!(a.len(), b.len());
                            for (a, b) in a.iter().zip(&b) {
                                assert_eq!((&a.rule, &a.message), (&b.rule, &b.message));
                                let a = serial.sources.resolve(a.primary);
                                let b = parallel.sources.resolve(b.primary);
                                assert_eq!((a.start, a.end), (b.start, b.end));
                            }
                        }
                        (a, b) => panic!("jobs {jobs}, reject {reject:?}: {a:?} != {b:?}"),
                    }
                    for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                        assert_eq!(
                            serial.memo_reads(path, kind),
                            parallel.memo_reads(path, kind),
                            "jobs {jobs}, reject {reject:?}, {kind:?}, {path:?}"
                        );
                    }
                }
                let before: Vec<_> = paths
                    .iter()
                    .flat_map(|p| {
                        [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes]
                            .map(|k| parallel.recompute_count(p, k))
                    })
                    .collect();
                let _ = parallel.mode_definitions(paths.clone(), Jobs::new(jobs));
                let after: Vec<_> = paths
                    .iter()
                    .flat_map(|p| {
                        [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes]
                            .map(|k| parallel.recompute_count(p, k))
                    })
                    .collect();
                assert_eq!(before, after, "warm jobs {jobs}, reject {reject:?}");
            }
        }
    });
}

#[test]
fn frozen_frontier_reports_each_missing_phase_and_retries_it() {
    with_compiler_stack(|| {
        use hirsel_intern::jobs::Jobs;
        for jobs in [1, 2, 8] {
            for missing in [
                QueryKind::Resolve,
                QueryKind::Signature,
                QueryKind::Check,
                QueryKind::Modes,
            ] {
                let (mut engine, _) = make_engine(None, false);
                match missing {
                    QueryKind::Resolve => engine.adapters.resolver = None,
                    QueryKind::Signature => engine.adapters.signature = None,
                    QueryKind::Check => engine.adapters.checker = None,
                    QueryKind::Modes => engine.adapters.modes = None,
                    _ => unreachable!(),
                }
                parse(&mut engine, "fn a () { 1 }\nfn b () { 2 }\n");
                let paths = vec![path("b"), path("a")];
                for _ in 0..2 {
                    for result in engine.mode_definitions(paths.clone(), Jobs::new(jobs)) {
                        assert_eq!(result.phase, Some(missing));
                        assert!(matches!(result.result, Err(QueryError::NotImplemented)));
                        assert!(engine.memo_reads(&result.definition, missing).is_none());
                    }
                }
                for p in paths {
                    assert!(engine.recompute_count(&p, missing) >= 2);
                }
            }
        }
    });
}

#[test]
fn frozen_frontier_runs_independent_checkers_concurrently() {
    struct Together {
        fake: Arc<dyn CheckerPhase>,
        barrier: std::sync::Barrier,
    }
    impl CheckerPhase for Together {
        fn check(
            &self,
            t: &TypeInterner,
            m: &TypeMemo,
            syntax: Arc<ItemSyntax>,
            resolution: Resolution,
            signature: &Signature,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<TypedBody>, QueryError> {
            // Exactly two tasks/workers: a serialized implementation cannot
            // satisfy this rendezvous. No wall-clock threshold is involved.
            self.barrier.wait();
            self.fake
                .check(t, m, syntax, resolution, signature, provider)
        }
    }
    with_compiler_stack(|| {
        let (mut engine, _) = make_engine(None, false);
        let fake = engine.adapters.checker.take().unwrap();
        engine.adapters.checker = Some(Arc::new(Together {
            fake,
            barrier: std::sync::Barrier::new(2),
        }));
        parse(&mut engine, "fn a () { 1 }\nfn b () { 2 }\n");
        let results = engine.mode_definitions(
            vec![path("b"), path("a")],
            hirsel_intern::jobs::Jobs::new(2),
        );
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.result.is_ok()));
    });
}

#[test]
fn frozen_frontier_retries_operational_headers_only_in_the_next_snapshot() {
    struct Flaky {
        original: Arc<dyn SignaturePhase>,
        calls: std::sync::atomic::AtomicU32,
    }
    impl SignaturePhase for Flaky {
        fn signature(
            &self,
            t: &TypeInterner,
            m: &TypeMemo,
            syntax: Arc<ItemSyntax>,
            d: &Declarations,
            p: &mut PhaseProducts,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<Signature>, QueryError> {
            if syntax.item.definition.0.name == "a"
                && self
                    .calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    == 0
            {
                return Err(QueryError::Operational(OperationalFailure {
                    operation: "snapshot signature".into(),
                    message: "temporarily unavailable".into(),
                }));
            }
            self.original.signature(t, m, syntax, d, p, provider)
        }
    }
    with_compiler_stack(|| {
        for jobs in [1, 2, 8] {
            let (mut engine, _) = make_engine(None, false);
            let adapter = Arc::new(Flaky {
                original: engine.adapters.signature.take().unwrap(),
                calls: std::sync::atomic::AtomicU32::new(0),
            });
            engine.adapters.signature = Some(adapter.clone());
            parse(&mut engine, "fn a () { 1 }\nfn b () { 2 }\n");
            let results = engine.mode_definitions(
                vec![path("b"), path("a")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert!(
                results
                    .iter()
                    .all(|r| matches!(r.result, Err(QueryError::Operational(_))))
            );
            assert_eq!(
                adapter.calls.load(std::sync::atomic::Ordering::Relaxed),
                1,
                "no post-freeze retry"
            );
            for p in [path("a"), path("b")] {
                assert!(engine.memo_reads(&p, QueryKind::Modes).is_none());
            }
            let results = engine.mode_definitions(
                vec![path("b"), path("a")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert!(results.iter().all(|r| r.result.is_ok()));
            assert_eq!(adapter.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
        }
    });
}

#[test]
fn compact_release_does_not_retry_an_unavailable_phase_through_an_old_checked_body() {
    with_compiler_stack(|| {
        let (mut engine, _) = make_engine(None, false);
        parse(&mut engine, "fn a () { 1 }\n");
        engine.mode_definition(&path("a")).unwrap();
        engine.adapters.resolver = None;
        parse(&mut engine, "fn a () { 2 }\n");
        let before = engine.recompute_count(&path("a"), QueryKind::Resolve);
        let outputs = engine.publish_definitions(
            vec![path("a")],
            hirsel_intern::jobs::Jobs::new(2),
            memo::Retention::Batch,
        );
        assert!(matches!(outputs[0].result, Err(QueryError::NotImplemented)));
        assert_eq!(outputs[0].phase, Some(QueryKind::Resolve));
        assert_eq!(
            engine.recompute_count(&path("a"), QueryKind::Resolve) - before,
            1,
            "retention must not reenter a stopped body query"
        );
    });
}

mod read_names_wire {
    use hirsel_syntax::{ContentHash, DefPath, FileContext};
    use hirsel_types::*;
    #[derive(serde::Serialize, Debug)]
    enum WrittenRead {
        ProviderLookup {
            context: FileContext,
            package: Option<hirsel_syntax::PackageId>,
            segments: Vec<String>,
            output: LookupOutput,
        },
        Lookup {
            context: FileContext,
            package: Option<hirsel_syntax::PackageId>,
            segments: Vec<String>,
            manifest: ContentHash,
            result: Result<DefPath, LookupError>,
        },
        ModuleHead {
            context: FileContext,
            head: String,
            manifest: ContentHash,
            exposed: bool,
        },
    }

    #[test]
    fn interned_read_names_preserve_structural_json_bincode_and_lookup_failures() {
        let path = DefPath::core("read_names.value");
        let context = FileContext {
            package: path.0.package.clone(),
            modules: vec!["nested".into()],
        };
        let segments = vec!["nested".into(), "value".into()];
        let manifest = ContentHash::of_bytes(b"selected manifest");
        let output = LookupOutput {
            manifest,
            result: Ok(path.clone()),
        };
        let mut written = vec![WrittenRead::ProviderLookup {
            context: context.clone(),
            package: None,
            segments: segments.clone(),
            output,
        }];
        for result in [
            Ok(path.clone()),
            Err(LookupError::Missing),
            Err(LookupError::MissingMember(vec![path.clone()])),
            Err(LookupError::Invisible(path.clone())),
        ] {
            written.push(WrittenRead::Lookup {
                context: context.clone(),
                package: Some(path.0.package.clone()),
                segments: segments.clone(),
                manifest,
                result,
            });
        }
        written.push(WrittenRead::ModuleHead {
            context,
            head: "nested".into(),
            manifest,
            exposed: false,
        });
        for read in written {
            let bytes = bincode::serialize(&read).unwrap();
            let compact: DependencyRead = bincode::deserialize(&bytes).unwrap();
            assert_eq!(bincode::serialize(&compact).unwrap(), bytes);
            assert_eq!(format!("{compact:?}"), format!("{read:?}"));
            assert_eq!(
                serde_json::to_value(&compact).unwrap(),
                serde_json::to_value(&read).unwrap()
            );
            let shared: SharedRead = bincode::deserialize(&bytes).unwrap();
            assert_eq!(bincode::serialize(&shared).unwrap(), bytes);
            assert_eq!(format!("{shared:?}"), format!("{read:?}"));
        }
    }
}
