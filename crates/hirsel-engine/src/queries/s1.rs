//! Task-local dependency/read bridge for S1 evaluation. Shared sources expose
//! immutable inputs; the requestor owns its coordinator stack and read frames.
//! Like rustc's body tasks (rustc_hir_analysis/src/lib.rs:166-220), a dependency
//! returns products for its caller to merge, including when evaluation fails.
use super::*;
mod source;
pub use source::{LoweringResult, S1LowerWork};
mod worker;
use hirsel_tir::{DefinitionRef, MaterializedRef, TirProvider};
use hirsel_types::GenericArgument;

/// A dependency's complete read record survives failure and cache hits alike.
/// The source owns depth admission and registration requests; this bridge only
/// transports its result and evidence to the enclosing evaluation.
pub(crate) struct DependencyProduct<T> {
    pub(crate) result: Result<T, QueryError>,
    pub(crate) reads: Reads,
}

/// A worker's request interface. Implementations keep mutable request state
/// local, over immutable shared phase/snapshot handles. No engine is required
/// by the evaluation provider, and no lock spans interpreter execution.
pub(crate) trait DependencySource {
    fn definition(&mut self, path: &DefPath) -> DependencyProduct<DefinitionRef>;
    fn constant(&mut self, path: &DefPath) -> DependencyProduct<MaterializedRef>;
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> DependencyProduct<MaterializedRef>;
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> DependencyProduct<Option<DefPath>>;
}

/// Saved dependency futures retain the same source request and read evidence
/// across suspension. Their output confers no additional verifier authority.
pub(crate) trait AsyncDependencySource {
    fn definition(
        &mut self,
        path: &DefPath,
    ) -> impl std::future::Future<Output = DependencyProduct<DefinitionRef>>;
    fn constant(
        &mut self,
        path: &DefPath,
    ) -> impl std::future::Future<Output = DependencyProduct<MaterializedRef>>;
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> impl std::future::Future<Output = DependencyProduct<MaterializedRef>>;
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> impl std::future::Future<Output = DependencyProduct<Option<DefPath>>>;
}
struct ReadyDependencies<'a, S>(&'a mut S);
impl<S: DependencySource> AsyncDependencySource for ReadyDependencies<'_, S> {
    async fn definition(&mut self, path: &DefPath) -> DependencyProduct<DefinitionRef> {
        self.0.definition(path)
    }
    async fn constant(&mut self, path: &DefPath) -> DependencyProduct<MaterializedRef> {
        self.0.constant(path)
    }
    async fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> DependencyProduct<MaterializedRef> {
        self.0.constant_instance(path, arguments)
    }
    async fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> DependencyProduct<Option<DefPath>> {
        self.0.instance_body(path, arguments)
    }
}

/// An immutable private artifact after V admission. It can execute with any
/// task-local dependency source and a worker's runtime; it owns no session.
/// Lowering stays in EvaluationInput::lower, so a failed lower never forces
/// snapshot construction or runtime preparation in the serial delegate.
pub(crate) struct ValidatedEvaluation {
    definition: DefinitionRef,
    ty: hirsel_types::TypeId,
    site: Span,
    acceptance: Option<hirsel_validate::AcceptedArtifact>,
}

pub(crate) struct EvaluationOutcome {
    pub(crate) acceptance: Option<hirsel_validate::AcceptedArtifact>,
    pub(crate) value: DependencyProduct<(hirsel_interp::Value, hirsel_types::TypeId)>,
    pub(crate) limit: Option<hirsel_tir::ToolingLimit>,
}
impl ValidatedEvaluation {
    /// A worker may publish across later registration epochs. The package's
    /// complete base V universe identifies every input of the private overlay,
    /// including formerly valid declarations whose overlay verdict can change.
    /// Record before admission so failures keep the same proof as successes.
    pub(crate) fn recorded(
        prepared: crate::comptime::PreparedEvaluation,
        snapshot: &SignatureSnapshot,
        verify: Verify,
        site: Span,
    ) -> DependencyProduct<Self> {
        let mut reads = ReadsBuilder::new();
        if verify == Verify::On {
            FrozenProvider::new(
                snapshot,
                prepared.definition.path.path().0.package.clone(),
                &mut reads,
            )
            .validation_universe();
        }
        DependencyProduct {
            result: Self::new(prepared, Some(snapshot), verify, site),
            reads: reads.finish().expect("one immutable validation universe"),
        }
    }
    pub(crate) fn new(
        prepared: crate::comptime::PreparedEvaluation,
        snapshot: Option<&SignatureSnapshot>,
        verify: Verify,
        site: Span,
    ) -> Result<Self, QueryError> {
        let acceptance = if verify == Verify::On {
            crate::comptime::validate_evaluation(
                snapshot.expect("verification requires a frozen snapshot"),
                &prepared.definition,
                prepared.signature,
                verify,
                prepared.origin.as_ref(),
            )?
        } else {
            None
        };
        Ok(Self {
            acceptance,
            definition: Arc::new(prepared.definition),
            ty: prepared.ty,
            site,
        })
    }
    /// The caller clones RuntimeTypes once per worker. Nested evaluations
    /// reuse that runtime so Serialization's recorder stack remains nested.
    pub(crate) fn run(
        self,
        sources: &SourceMap,
        limits: crate::ComptimeLimits,
        runtime: &hirsel_interp::aggregate::RuntimeTypes,
        source: &mut impl DependencySource,
    ) -> EvaluationOutcome {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let mut source = ReadyDependencies(source);
        let mut execution = std::pin::pin!(self.run_async(sources, limits, runtime, &mut source));
        match execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(product) => product,
            Poll::Pending => unreachable!("synchronous S1 dependency source suspended"),
        }
    }
    pub(crate) async fn run_async(
        self,
        sources: &SourceMap,
        limits: crate::ComptimeLimits,
        runtime: &hirsel_interp::aggregate::RuntimeTypes,
        source: &mut impl AsyncDependencySource,
    ) -> EvaluationOutcome {
        let configuration = RunConfiguration {
            entry: self.definition.path.path().clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut provider = EvaluationProvider::new(self.definition, source);
        let product = crate::comptime::EvaluationTask {
            sources,
            configuration,
            limits,
            runtime,
            site: self.site,
            ty: self.ty,
        }
        .run_async(&mut provider)
        .await;
        let mut reads = ReadsBuilder::new();
        let recorded = provider.finish();
        if let Ok(recorded) = &recorded {
            reads.merge(recorded);
        }
        for (path, digest) in product.reflected {
            reads.record(ReadKey::Representation(DefId::intern(&path)), digest);
        }
        let reads = reads.finish();
        let (result, reads) = match (recorded, reads) {
            (Ok(_), Ok(reads)) => (product.result, reads),
            _ => (Err(QueryError::NotImplemented), Reads::empty()),
        };
        EvaluationOutcome {
            acceptance: self.acceptance,
            value: DependencyProduct { result, reads },
            limit: product.limit,
        }
    }
}

/// A private evaluation root plus its task-local dependency requestor. Every
/// request merges its complete reads before returning its success or failure.
/// Constants remain lazy: interpreting a root never enumerates other constants.
pub(crate) struct EvaluationProvider<'a, S> {
    root: DefinitionRef,
    source: &'a mut S,
    reads: ReadsBuilder,
}
impl<'a, S> EvaluationProvider<'a, S> {
    pub(crate) fn new(root: DefinitionRef, source: &'a mut S) -> Self {
        Self {
            root,
            source,
            reads: ReadsBuilder::new(),
        }
    }
    fn merge<T>(&mut self, product: DependencyProduct<T>) -> Result<T, QueryError> {
        self.reads.merge(&product.reads);
        product.result
    }
    pub(crate) fn finish(self) -> Result<Reads, QueryError> {
        self.reads.finish().map_err(|_| QueryError::NotImplemented)
    }
}
impl<S: DependencySource> TirProvider for EvaluationProvider<'_, S> {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        if path == self.root.path.path() {
            return Ok(self.root.clone());
        }
        let product = self.source.definition(path);
        self.merge(product)
    }
    fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError> {
        let product = self.source.constant(path);
        self.merge(product)
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        let product = self.source.constant_instance(path, arguments);
        self.merge(product)
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        let product = self.source.instance_body(path, arguments);
        self.merge(product)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
    }
}

impl<S: AsyncDependencySource> hirsel_interp::AsyncTirProvider for EvaluationProvider<'_, S> {
    async fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        if path == self.root.path.path() {
            return Ok(self.root.clone());
        }
        let product = self.source.definition(path).await;
        self.merge(product)
    }
    async fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError> {
        let product = self.source.constant(path).await;
        self.merge(product)
    }
    async fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        let product = self.source.constant_instance(path, arguments).await;
        self.merge(product)
    }
    async fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        let product = self.source.instance_body(path, arguments).await;
        self.merge(product)
    }
    async fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_tir::BodyId;

    struct Failed {
        reads: Reads,
        calls: usize,
    }
    impl Failed {
        fn product<T>(&mut self) -> DependencyProduct<T> {
            self.calls += 1;
            DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: self.reads.clone(),
            }
        }
    }
    impl DependencySource for Failed {
        fn definition(&mut self, _: &DefPath) -> DependencyProduct<DefinitionRef> {
            self.product()
        }
        fn constant(&mut self, _: &DefPath) -> DependencyProduct<MaterializedRef> {
            self.product()
        }
        fn constant_instance(
            &mut self,
            _: &DefPath,
            _: &[u64],
        ) -> DependencyProduct<MaterializedRef> {
            self.product()
        }
        fn instance_body(
            &mut self,
            _: &DefPath,
            _: &[GenericArgument],
        ) -> DependencyProduct<Option<DefPath>> {
            self.product()
        }
    }

    /// A real private constant artifact that reads a dependency through the
    /// interpreter. V admission and the returned type must remain independent
    /// of whether the provider later succeeds or fails.
    pub(super) fn reading_artifact(
        types: &hirsel_types::TypeInterner,
        root: &DefPath,
        dependency: &DefPath,
    ) -> crate::comptime::PreparedEvaluation {
        use hirsel_tir::*;
        let ty = types.intern(hirsel_types::Type::unit());
        let site = Span::default();
        let body = Body {
            arena_scope_grant: None,
            scoped_parameters: Vec::new(),
            parameter_scope: hirsel_syntax::LexicalOwner::first(root)
                .site(hirsel_types::NameSite::Item),
            locals: vec![Local {
                ty,
                storage: Storage::Temporary,
                scope: ScopeId(0),
                span: site,
            }],
            places: vec![Place {
                loan: None,
                binding: None,
                root: LocalId(0),
                projection: vec![].into(),
                ty,
            }],
            scopes: vec![Scope {
                parent: None,
                span: site,
            }],
            blocks: vec![Block {
                instructions: vec![
                    Instruction {
                        kind: InstructionKind::StorageLive(LocalId(0)),
                        span: site,
                        on_failure: CleanupId(0),
                    },
                    Instruction {
                        kind: InstructionKind::Assign {
                            destination: PlaceId(0),
                            value: RvalueId(0),
                        },
                        span: site,
                        on_failure: CleanupId(0),
                    },
                ],
                terminator: Terminator::Return {
                    value: Operand {
                        place: PlaceId(0),
                        usage: hirsel_types::UseKind::Take,
                    },
                    cleanup: CleanupId(0),
                },
                span: site,
            }],
            rvalues: vec![Rvalue::ReadConstant {
                definition: dependency.clone(),
                arguments: vec![],
            }],
            cleanups: vec![Cleanup::default()],
            cleanup_nodes: vec![],
            entry: BlockId(0),
            parameters: vec![],
            result: ty,
            borrow_cursors: vec![],
            tail_sites: vec![],
            matches: vec![],
        };
        let mut definition = Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: Arc::from([]),
            path: DefId::intern(root),
            inferred_unique: Default::default(),
            signature_hash: ContentHash::default(),
            bodies: vec![body],
            entry: BodyId(0),
            contracts: vec![],
        };
        definition.types = definition_types(types, &definition);
        crate::comptime::PreparedEvaluation {
            definition,
            origin: None,
            signature: Signature {
                definition: DefId::intern(root),
                kind: hirsel_types::SignatureKind::Const {
                    ty,
                    generics: Default::default(),
                },
                metadata: Default::default(),
                fingerprint: ContentHash::default(),
            },
            ty,
        }
    }

    struct DeferredSource {
        dependencies: crate::runtime::QueryReads,
        query: crate::runtime::QueryId,
        types: Arc<hirsel_types::TypeInterner>,
        reads: Reads,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AsyncDependencySource for DeferredSource {
        async fn definition(&mut self, _: &DefPath) -> DependencyProduct<DefinitionRef> {
            DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            }
        }
        async fn constant(&mut self, _: &DefPath) -> DependencyProduct<MaterializedRef> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let result = self
                .dependencies
                .read(self.query)
                .await
                .map(|_| {
                    Arc::new(hirsel_tir::Materialized {
                        root: hirsel_tir::ValueNode {
                            ty: self.types.intern(hirsel_types::Type::unit()),
                            kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                        },
                    })
                })
                .map_err(|_| QueryError::NotImplemented);
            DependencyProduct {
                result,
                reads: self.reads.clone(),
            }
        }
        async fn constant_instance(
            &mut self,
            path: &DefPath,
            _: &[u64],
        ) -> DependencyProduct<MaterializedRef> {
            self.constant(path).await
        }
        async fn instance_body(
            &mut self,
            _: &DefPath,
            _: &[GenericArgument],
        ) -> DependencyProduct<Option<DefPath>> {
            DependencyProduct {
                result: Ok(None),
                reads: Reads::empty(),
            }
        }
    }
    fn reference(text: &[u8]) -> crate::store::Reference {
        crate::store::Domain {
            kind: 0x5300_00ff,
            schema: 1,
        }
        .reference(text)
        .unwrap()
    }
    fn errors() -> crate::runtime::RuntimeErrors {
        crate::runtime::RuntimeErrors {
            resource: reference(b"resource"),
            protocol: reference(b"protocol"),
            cycle: reference(b"cycle"),
            panicked: reference(b"panicked"),
            cancelled: reference(b"cancelled"),
        }
    }
    fn query(kind: u32) -> crate::runtime::QueryId {
        crate::runtime::QueryId {
            kind,
            address: crate::store::Address(ContentHash::of_bytes(&kind.to_le_bytes())),
        }
    }
    fn product(query: crate::runtime::QueryId) -> crate::runtime::ProductRef {
        crate::runtime::test_product(
            query.address,
            crate::store::Action(query.address.0),
            crate::store::Domain { kind: 1, schema: 1 },
            &query.address.0.0,
        )
    }
    struct Leaf(bool);
    impl crate::runtime::QueryFactory for Leaf {
        fn create(
            &self,
            query: crate::runtime::QueryId,
        ) -> Result<Box<dyn crate::runtime::QueryTask>, crate::store::Reference> {
            Ok(Box::new(LeafTask {
                query,
                fail: self.0,
            }))
        }
    }
    struct LeafTask {
        query: crate::runtime::QueryId,
        fail: bool,
    }
    impl crate::runtime::QueryTask for LeafTask {
        fn id(&self) -> crate::runtime::QueryId {
            self.query
        }
        fn weight(&self) -> crate::runtime::MemoryWeight {
            Default::default()
        }
        fn poll(&mut self, _: &mut dyn crate::runtime::QueryContext) -> crate::runtime::QueryPoll {
            if self.fail {
                crate::runtime::QueryPoll::Failed(errors().protocol)
            } else {
                crate::runtime::QueryPoll::Complete(product(self.query))
            }
        }
    }
    struct EvaluationFactory {
        types: Arc<hirsel_types::TypeInterner>,
        snapshot: Arc<SignatureSnapshot>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    }
    impl crate::runtime::LocalQueryFactory for EvaluationFactory {
        fn weight(
            &self,
            _: crate::runtime::QueryId,
        ) -> Result<crate::runtime::MemoryWeight, crate::store::Reference> {
            Ok(crate::runtime::MemoryWeight {
                retained: 1 << 20,
                scratch: 1 << 20,
                stack: 0,
            })
        }
        fn create(
            &self,
            query: crate::runtime::QueryId,
        ) -> Result<Box<dyn crate::runtime::LocalQueryTask>, crate::store::Reference> {
            let types = self.types.clone();
            let snapshot = self.snapshot.clone();
            let calls = self.calls.clone();
            let fail = self.fail;
            let task = crate::runtime::LocalFutureTask::new(
                query,
                self.weight(query)?,
                1,
                errors(),
                |dependencies| async move {
                    let root = DefPath::core("saved-private-root");
                    let dependency = DefPath::core("saved-private-dependency");
                    let prepared = reading_artifact(&types, &root, &dependency);
                    let admitted = ValidatedEvaluation::recorded(
                        prepared,
                        &snapshot,
                        Verify::On,
                        Span::default(),
                    );
                    assert!(
                        admitted
                            .reads
                            .get(&ReadKey::ValidationUniverse(crate::memo::PackageKey::of(
                                &root.0.package
                            )))
                            .is_some()
                    );
                    let validated = admitted.result.unwrap();
                    let runtime = hirsel_interp::aggregate::RuntimeTypes::new(types.clone());
                    let expected = Reads::from_vec(vec![(
                        ReadKey::ExecutedBody(DefId::intern(&dependency)),
                        ContentHash::of_bytes(b"selected leaf"),
                    )])
                    .unwrap();
                    let mut source = DeferredSource {
                        dependencies,
                        query: super::tests::query(1),
                        types,
                        reads: expected.clone(),
                        calls,
                    };
                    let outcome = validated
                        .run_async(
                            &SourceMap::default(),
                            Default::default(),
                            &runtime,
                            &mut source,
                        )
                        .await;
                    assert_eq!(outcome.value.reads, expected);
                    assert!(outcome.limit.is_none());
                    if fail {
                        assert!(matches!(
                            outcome.value.result,
                            Err(QueryError::NotImplemented)
                        ));
                    } else {
                        assert!(matches!(
                            outcome.value.result,
                            Ok((hirsel_interp::Value::Unit, _))
                        ));
                    }
                    Ok(product(query))
                },
            )?;
            Ok(Box::new(task))
        }
    }
    #[test]
    fn validated_private_evaluation_suspends_on_s5_with_one_worker_and_complete_failure_reads() {
        use crate::runtime::{AdmittedRuntime, ProcessBudget, QueryRuntime};
        crate::with_compiler_stack(|| {
            for fail in [false, true] {
                let types = hirsel_types::TypeInterner::new(None);
                let dependency = DefPath::core("saved-private-dependency");
                let prepared =
                    reading_artifact(&types, &DefPath::core("saved-private-root"), &dependency);
                let mut signature = prepared.signature;
                signature.definition = DefId::intern(&dependency);
                let snapshot = Arc::new(SignatureSnapshot::new(
                    Revision::FIRST,
                    types.clone(),
                    [(signature.definition, Ok(Arc::new(signature)))],
                    [],
                    None,
                ));
                let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let budget = ProcessBudget::new(256 << 20);
                let mut runtime = AdmittedRuntime::new(
                    hirsel_intern::jobs::Jobs::new(1),
                    budget.reserve(256 << 20).unwrap(),
                    errors(),
                )
                .unwrap();
                runtime.register(1, Arc::new(Leaf(fail))).unwrap();
                runtime
                    .register_local(
                        2,
                        Arc::new(EvaluationFactory {
                            types,
                            snapshot,
                            calls: calls.clone(),
                            fail,
                        }),
                    )
                    .unwrap();
                let request = runtime.request(&[query(2)]);
                assert_eq!(runtime.wait(request), Ok(vec![product(query(2))]));
                assert_eq!(runtime.stats().claims, 2);
                assert_eq!(runtime.stats().waits, 1);
                assert_eq!(
                    calls.load(std::sync::atomic::Ordering::SeqCst),
                    1,
                    "resume must not reissue or replay the initializer's source demand"
                );
            }
        });
    }
    #[test]
    fn immutable_evaluation_keeps_dependency_and_reflection_reads_after_failure() {
        crate::with_compiler_stack(|| {
            let types = hirsel_types::TypeInterner::new(None);
            let root = DefPath::core("private-evaluation");
            let dependency = DefPath::core("constant-dependency");
            let prepared = reading_artifact(&types, &root, &dependency);
            let mut signature = prepared.signature.clone();
            signature.definition = DefId::intern(&dependency);
            let snapshot = SignatureSnapshot::new(
                Revision::FIRST,
                types.clone(),
                [(signature.definition, Ok(Arc::new(signature)))],
                [],
                None,
            );
            let runtime = hirsel_interp::aggregate::RuntimeTypes::new(types);
            let mut source = Failed {
                calls: 0,
                reads: Reads::from_vec(vec![(
                    ReadKey::ExecutedBody(DefId::intern(&dependency)),
                    ContentHash::of_bytes(b"body"),
                )])
                .unwrap(),
            };
            let admitted =
                ValidatedEvaluation::new(prepared, Some(&snapshot), Verify::On, Span::default())
                    .unwrap();
            let result = admitted.run(
                &SourceMap::default(),
                Default::default(),
                &runtime,
                &mut source,
            );
            assert!(matches!(
                result.value.result,
                Err(QueryError::NotImplemented)
            ));
            assert_eq!(source.calls, 1);
            assert_eq!(result.value.reads, source.reads);
            assert!(result.limit.is_none());
            assert!(runtime.serialization.finish().is_empty());
        });
    }

    #[test]
    fn failed_private_admission_keeps_its_originating_universe_receipt() {
        crate::with_compiler_stack(|| {
            let types = hirsel_types::TypeInterner::new(None);
            let root = DefPath::core("private-admission");
            let dependency = DefPath::core("constant-dependency");
            let snapshot = SignatureSnapshot::new(Revision::FIRST, types.clone(), [], [], None);
            let mut artifact = reading_artifact(&types, &root, &dependency);
            artifact.definition.entry = BodyId(100);
            let product =
                ValidatedEvaluation::recorded(artifact, &snapshot, Verify::On, Span::default());
            assert!(matches!(product.result, Err(QueryError::Operational(_))));
            assert_eq!(
                product
                    .reads
                    .get(&ReadKey::ValidationUniverse(crate::memo::PackageKey::of(
                        &root.0.package
                    ))),
                Some(snapshot.validation_input_digest(&root.0.package))
            );
            let artifact = reading_artifact(&types, &root, &dependency);
            let unchecked =
                ValidatedEvaluation::recorded(artifact, &snapshot, Verify::Off, Span::default());
            assert!(unchecked.result.is_ok());
            assert!(
                unchecked.reads.is_empty(),
                "verification off creates no acceptance proof"
            );
        });
    }

    #[test]
    fn dependency_failures_preserve_reads_and_private_roots_are_local() {
        let path = DefPath::core("task-root");
        let dependency = DefPath::core("dependency");
        let root = Arc::new(hirsel_tir::Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: Arc::from([]),
            path: DefId::intern(&path),
            inferred_unique: Default::default(),
            signature_hash: ContentHash::default(),
            bodies: vec![],
            entry: BodyId(0),
            contracts: vec![],
        });
        let reads = Reads::from_vec(vec![(
            ReadKey::ExecutedBody(DefId::intern(&dependency)),
            ContentHash::of_bytes(b"body"),
        )])
        .unwrap();
        let mut source = Failed {
            reads: reads.clone(),
            calls: 0,
        };
        let mut provider = EvaluationProvider::new(root.clone(), &mut source);
        assert!(Arc::ptr_eq(&provider.definition(&path).unwrap(), &root));
        assert!(provider.constant_definitions().unwrap().is_empty());
        assert!(provider.definition(&dependency).is_err());
        assert!(provider.constant(&dependency).is_err());
        assert!(provider.constant_instance(&dependency, &[3]).is_err());
        assert!(provider.instance_body(&dependency, &[]).is_err());
        assert_eq!(provider.finish().unwrap(), reads);
        assert_eq!(source.calls, 4);
    }
}
