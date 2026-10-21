//! Task-local dependency/read bridge for S1 evaluation. Shared sources expose
//! immutable inputs; the requestor owns its coordinator stack and read frames.
//! Like rustc's body tasks (rustc_hir_analysis/src/lib.rs:166-220), a dependency
//! returns products for its caller to merge, including when evaluation fails.
use super::*;
mod source;
pub use source::LoweringResult;
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

/// An immutable private artifact after V admission. It can execute with any
/// task-local dependency source and a worker's runtime; it owns no session.
/// Lowering stays in EvaluationInput::lower, so a failed lower never forces
/// snapshot construction or runtime preparation in the serial delegate.
pub(crate) struct ValidatedEvaluation {
    definition: DefinitionRef,
    ty: hirsel_types::TypeId,
    site: Span,
}

pub(crate) struct EvaluationOutcome {
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
        if verify == Verify::On {
            crate::comptime::validate_evaluation(
                snapshot.expect("verification requires a frozen snapshot"),
                &prepared.definition,
                prepared.signature,
                verify,
            )?;
        }
        Ok(Self {
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
        limits: hirsel_interp::Limits,
        runtime: &hirsel_interp::aggregate::RuntimeTypes,
        source: &mut impl DependencySource,
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
        .run(&mut provider);
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
            value: DependencyProduct { result, reads },
            limit: product.limit,
        }
    }
}

/// A private evaluation root plus its task-local dependency requestor. Every
/// request merges its complete reads before returning its success or failure.
/// Constants remain lazy: interpreting a root never enumerates other constants.
pub(crate) struct EvaluationProvider<'a, S: DependencySource> {
    root: DefinitionRef,
    source: &'a mut S,
    reads: ReadsBuilder,
}
impl<'a, S: DependencySource> EvaluationProvider<'a, S> {
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
            tail_sites: vec![],
            matches: vec![],
        };
        let mut definition = Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: Arc::from([]),
            path: DefId::intern(root),
            signature_hash: ContentHash::default(),
            bodies: vec![body],
            entry: BodyId(0),
            contracts: vec![],
        };
        definition.types = definition_types(types, &definition);
        crate::comptime::PreparedEvaluation {
            definition,
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
