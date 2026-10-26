//! Immutable session views for S1 body demands. Tables are borrowed, never
//! cloned into workers. Ordinary misses use the existing frozen P task, and
//! complete products return for deterministic serial publication.
use super::worker::{
    CachedLowering, Requirement, SelectionEvent, TaskBody, TaskInputs, TaskWorker, TirSource,
};
use super::*;
use crate::memo::{DepthEvidence, MemoGraph};
use crate::snapshot::SourceContext;
use std::sync::Mutex;

/// Physical S1 worker lowering work, including helpers and work discarded by
/// registration retries or an earlier rejected input. Never a cache-validity
/// fact or a count of published logical misses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct S1LowerWork {
    pub query_attempts: u64,
    pub adapter_calls: u64,
    pub validation_runs: u64,
}
impl S1LowerWork {
    pub(super) fn merge(&mut self, work: Self) {
        self.query_attempts += work.query_attempts;
        self.adapter_calls += work.adapter_calls;
        self.validation_runs += work.validation_runs;
    }
}

/// One lowering root with its captured operational provenance and complete
/// execution reads. Publication of its TIR remains the caller's policy.
#[derive(Clone, Debug)]
pub struct LoweringResult<T = DefinitionRef> {
    pub verification: Option<Arc<hirsel_validate::VerifiedDefinition>>,
    pub definition: DefPath,
    pub result: Result<QueryOutput<T>, QueryError>,
    pub phase: Option<QueryKind>,
    pub limit: Option<hirsel_tir::ToolingLimit>,
    pub reads: Arc<Reads>,
    pub depth: Option<DepthEvidence<Span>>,
    /// This root missed the worker's Lower caches. Scalar fallback is already
    /// counted by the engine; this flag also covers failed worker queries.
    pub worker_recomputed: bool,
}
#[derive(Clone, Copy)]
enum RootKind {
    Constant,
    Lower,
    CheckLower,
}
#[derive(Clone)]
enum RootValue {
    Constant(hirsel_tir::Materialized),
    Lowered(
        QueryOutput<DefinitionRef>,
        DepthEvidence<Span>,
        Option<Arc<hirsel_validate::VerifiedDefinition>>,
    ),
    Checked(
        QueryOutput<()>,
        DepthEvidence<Span>,
        Option<Arc<hirsel_validate::VerifiedDefinition>>,
    ),
}
#[derive(Clone)]
struct RootOutcome {
    definition: DefPath,
    result: Result<RootValue, QueryError>,
    phase: Option<QueryKind>,
    limit: Option<hirsel_tir::ToolingLimit>,
    reads: Arc<Reads>,
    worker_recomputed: bool,
}
impl RootOutcome {
    fn lowering<T>(
        self,
        value: impl FnOnce(
            RootValue,
        ) -> (
            QueryOutput<T>,
            DepthEvidence<Span>,
            Option<Arc<hirsel_validate::VerifiedDefinition>>,
        ),
    ) -> LoweringResult<T> {
        let (result, depth, verification) = match self.result {
            Ok(root) => {
                let (output, depth, verification) = value(root);
                (Ok(output), Some(depth), verification)
            }
            Err(error) => (Err(error), None, None),
        };
        LoweringResult {
            verification,
            definition: self.definition,
            result,
            phase: self.phase,
            limit: self.limit,
            reads: self.reads,
            depth,
            worker_recomputed: self.worker_recomputed,
        }
    }
}
pub(crate) struct FrozenSource<'a> {
    pub(crate) inputs: TaskInputs,
    pub(crate) items: &'a BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    pub(crate) generated: &'a BTreeMap<DefPath, generation::GeneratedSource>,
    pub(crate) signatures: &'a Table<hirsel_types::SignatureRef>,
    pub(crate) modes: &'a Table<AnalyzedBody>,
    pub(crate) checked: &'a Table<TypedBody>,
    pub(crate) resolutions: &'a Table<Resolution>,
    pub(crate) lowered: &'a Table<DefinitionRef>,
    constants: &'a BTreeMap<DefPath, crate::comptime::EvaluatedConstant>,
    valid_constants: BTreeSet<DefPath>,
    mode_attempts: BTreeSet<DefPath>,
    pub(crate) graph: &'a MemoGraph,
    pub(crate) declarations: Arc<Declarations>,
    pub(crate) citations: SourceContext<'a>,
    pub(crate) registry: &'a crate::registry::BuiltinRegistry,
    pub(crate) instance_bodies: &'a hirsel_tir::InstanceBodies,
    pub(crate) staged: &'a BTreeMap<DefPath, crate::comptime::StagedConstant>,
    pub(crate) manifests: &'a BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    pub(crate) default_manifest: ContentHash,
    pub(crate) environment: ContentHash,
    pub(crate) signer: Option<&'a dyn SignaturePhase>,
    pub(crate) resolver: Option<&'a dyn ResolverPhase>,
    pub(crate) checker: Option<&'a dyn CheckerPhase>,
    pub(crate) mode_phase: Option<&'a dyn ModesPhase>,
    pub(crate) completed: Mutex<Vec<(parallel::CompactCompleted, ContentHash, Reads)>>,
    records: Option<Arc<persistent::TypeRecords>>,
    store: Option<Arc<dyn crate::store::ResultStore>>,
    lower_products: Mutex<BTreeMap<DefPath, persistent::CapturedLower>>,
}
impl FrozenSource<'_> {
    fn text(&self, path: &DefPath) -> Option<ContentHash> {
        let body = self.implementation(path)?;
        let manifest = self
            .manifests
            .get(&path.0.package)
            .copied()
            .unwrap_or(self.default_manifest);
        Some(ContentHash::of_parts(
            "hirsel.query.text.v1",
            [body.0.as_slice(), manifest.0.as_slice()],
        ))
    }
    fn current<T>(&self, path: &DefPath, entry: &Entry<T>) -> bool {
        entry.consistent
            && !self.graph.is_dirty(&entry.memo.key)
            && self.text(path) == Some(entry.memo.text)
    }
}
impl TirSource for FrozenSource<'_> {
    fn generator_site(&self, path: &DefPath) -> Result<Span, QueryError> {
        let syntax = self
            .items
            .get(path)
            .and_then(|items| items.first())
            .ok_or(QueryError::NotImplemented)?;
        let expression = generation::generator(syntax).ok_or(QueryError::NotImplemented)?;
        Ok(syntax.tree.expressions[expression.0 as usize].origin.span)
    }
    fn prepare_generator(
        &self,
        path: &DefPath,
    ) -> DependencyProduct<generation::task::PreparedGenerator> {
        let Some(syntax) = self
            .items
            .get(path)
            .and_then(|items| items.first())
            .cloned()
        else {
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        };
        let (Some(signature), Some(resolver), Some(checker), Some(modes)) =
            (self.signer, self.resolver, self.checker, self.mode_phase)
        else {
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        };
        generation::task::prepare(
            path,
            syntax,
            self.inputs.snapshot.types(),
            &self.inputs.memo,
            &self.inputs.snapshot,
            &self.declarations,
            &self.citations,
            self.environment,
            &generation::task::GeneratorPhases {
                signature,
                resolver,
                checker,
                modes,
            },
        )
    }

    fn inputs(&self) -> &TaskInputs {
        &self.inputs
    }
    fn anchor_owner(&self, anchor: hirsel_span::Anchor) -> Option<(DefPath, u32)> {
        super::super::persistent::artifact_anchor_owner(
            self.items,
            &self.inputs.sources,
            self.generated,
            anchor,
        )
    }
    fn implementation(&self, path: &DefPath) -> Option<ContentHash> {
        self.items
            .get(path)
            .and_then(|items| items.first())
            .map(|item| item.text_hash)
    }
    fn site(&self, path: &DefPath) -> Result<Span, QueryError> {
        self.items
            .get(path)
            .and_then(|items| items.first())
            .map(|item| item.item.name_span)
            .or_else(|| {
                self.declarations
                    .sites(path)
                    .first()
                    .map(|site| site.name_span)
            })
            .or_else(|| {
                self.registry
                    .get(DefId::intern(path))
                    .map(|_| Span::default())
            })
            .ok_or(QueryError::NotImplemented)
    }
    fn body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
    ) -> DependencyProduct<Arc<TaskBody>> {
        if self
            .items
            .get(path)
            .and_then(|items| items.first())
            .is_some_and(|syntax| {
                generation::generator(syntax).is_some() && !generation::instance_bodied(syntax)
            })
        {
            worker
                .selections
                .push(SelectionEvent::Generated(DefId::intern(path)));
        }
        // A generic generated declaration has no executable body. Its closed
        // instance must be registered before a worker checks the generator;
        // never treat its open type parameters as a private constant.
        if self
            .items
            .get(path)
            .and_then(|items| items.first())
            .is_some_and(|syntax| generation::instance_bodied(syntax))
        {
            worker.pending.push(Requirement::Generate(path.clone()));
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        }
        let id = DefId::intern(path);
        if let Some(entry) = self
            .modes
            .get(&id)
            .filter(|entry| self.current(path, entry))
        {
            if let Some(generated) = self
                .checked
                .get(&id)
                .filter(|entry| self.current(path, entry))
            {
                worker.replay_selections(&generated.selections);
            }
            if let Some(generated) = self
                .checked
                .get(&id)
                .filter(|entry| self.current(path, entry))
                && let Some(depth) = &generated.depth
                && let Err(error) =
                    worker.replay_generator(depth, self.generator_site(path).unwrap_or_default())
            {
                return DependencyProduct {
                    result: Err(error),
                    reads: entry.reads().clone(),
                };
            }
            if entry.result().is_err() {
                worker.phase = Some(QueryKind::Modes);
            }
            return DependencyProduct {
                result: entry.result().map(|output| {
                    Arc::new(TaskBody {
                        analyzed: output.clone(),
                        reads: entry.reads().clone(),
                        dependencies: output.dependencies.clone(),
                        detail: entry.detail.clone(),
                    })
                }),
                reads: entry.reads().clone(),
            };
        }
        // A nested execution demand can reach a body absent from the frozen
        // tables even when its ordinary Modes product is durable. Return the
        // demand before running P; only the controller may validate/adopt it.
        // A failed lookup is remembered for this request so cold bodies still
        // use the ordinary worker on the next round.
        if !self.mode_attempts.contains(path) {
            worker.pending.push(Requirement::Modes(path.clone()));
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        }
        let Some(syntax) = self
            .items
            .get(path)
            .and_then(|items| items.first())
            .cloned()
        else {
            worker.pending.push(Requirement::Syntax(path.clone()));
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        };
        if generation::generator(&syntax).is_some() {
            if let Some(entry) = self
                .checked
                .get(&id)
                .filter(|entry| self.current(path, entry))
            {
                worker.replay_selections(&entry.selections);
                if let Some(depth) = &entry.depth
                    && let Err(error) = worker
                        .replay_generator(depth, self.generator_site(path).unwrap_or_default())
                {
                    return DependencyProduct {
                        result: Err(error),
                        reads: entry.reads().clone(),
                    };
                }
                if let Err(error) = entry.result() {
                    worker.phase = Some(QueryKind::Check);
                    return DependencyProduct {
                        result: Err(error),
                        reads: entry.reads().clone(),
                    };
                }
            } else {
                return worker.generate_body(path);
            }
        }
        // The own header was observed during snapshot construction. Never
        // normalize/retry it in a worker after freezing the epoch.
        let signature = match self.inputs.snapshot.signature(id) {
            Some(Ok(signature)) => self
                .signatures
                .get(&id)
                .and_then(|entry| entry.result().ok())
                .map(Ok)
                .unwrap_or_else(|| Ok(Arc::new(output(signature.clone())))),
            Some(Err(error)) => Err(error.clone()),
            None => Err(QueryError::NotImplemented),
        };
        let task = parallel::Task {
            index: 0,
            definition: path.clone(),
            syntax,
            signature,
            signature_reads: self
                .signatures
                .get(&id)
                .map(|entry| entry.memo.reads.clone())
                .unwrap_or_default(),
            resolution: self
                .resolutions
                .get(&id)
                .filter(|entry| self.current(path, entry))
                .cloned(),
            checked: self
                .checked
                .get(&id)
                .filter(|entry| self.current(path, entry))
                .cloned(),
            text: self.text(path).unwrap_or_default(),
        };
        let context = parallel::Worker {
            types: self.inputs.snapshot.types(),
            memo: &self.inputs.memo,
            snapshot: &self.inputs.snapshot,
            declarations: &self.declarations,
            citations: &self.citations,
            sources: &self.inputs.sources,
            items: self.items,
            resolver: self.resolver,
            checker: self.checker,
            modes: self.mode_phase,
        };
        let done = context.run(task);
        let candidates = self.inputs.snapshot.declaration_candidates(id);
        let reads = done.complete_reads(&mut parallel::FrozenReadSource {
            declarations: &self.declarations,
            citations: &self.citations,
            candidates,
            environment: self.environment,
        });
        let result = done.result();
        if result.is_err() {
            worker.phase = Some(done.phase);
        }
        let reads = match reads {
            Ok(reads) => reads,
            Err(error) => {
                worker.pending.push(Requirement::Receipt(error.0));
                return DependencyProduct {
                    result: Err(QueryError::NotImplemented),
                    reads: Reads::empty(),
                };
            }
        };
        let text = done.text;
        let detail = done
            .modes
            .as_ref()
            .map(|mode| mode.detail().clone())
            .unwrap_or_default();
        let done = parallel::CompactCompleted::capture(
            done,
            &context,
            self.inputs.snapshot.types().clone(),
            &mut parallel::FrozenReadSource {
                declarations: &self.declarations,
                citations: &self.citations,
                candidates,
                environment: self.environment,
            },
            None,
        );
        self.completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((done, text, reads.clone()));
        DependencyProduct {
            result: result.map(|output| {
                Arc::new(TaskBody {
                    analyzed: output.clone(),
                    reads: reads.clone(),
                    dependencies: output.dependencies.clone(),
                    detail,
                })
            }),
            reads,
        }
    }
    fn completed_lower(
        &self,
        output: &QueryOutput<DefinitionRef>,
        detail: &DependencyReads,
        reads: &Reads,
        depth: &DepthEvidence<Span>,
        selections: &[crate::comptime::Selection],
        acceptance: Option<hirsel_validate::ExecutionReceipt>,
    ) {
        let Some(records) = &self.records else { return };
        let Some(store) = &self.store else { return };
        if let Some(captured) = persistent::capture_lower(
            records,
            |anchor| self.anchor_owner(anchor),
            store.as_ref(),
            output,
            detail,
            reads,
            depth,
            selections,
            acceptance,
        ) {
            self.lower_products
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(output.value.path.path().clone(), captured);
        }
    }
    fn dictionary_constant(
        &self,
        path: &DefPath,
    ) -> Result<Option<hirsel_tir::Materialized>, QueryError> {
        if self.items.contains_key(path) {
            return Ok(None);
        }
        self.registry
            .dictionary_value(self.inputs.snapshot.types(), &self.inputs.memo, path)
    }
    fn has_staged_declaration(&self, registration: &crate::comptime::StagedRegistration) -> bool {
        self.staged.get(&registration.path).is_some_and(|constant| {
            constant.signature.fingerprint == registration.constant.signature.fingerprint
                && constant.inputs == registration.constant.inputs
        })
    }
    fn staged_constant(&self, path: &DefPath) -> Option<crate::comptime::StagedConstant> {
        self.staged.get(path).cloned()
    }
    fn cached_constant(&self, path: &DefPath) -> Option<crate::comptime::EvaluatedConstant> {
        self.valid_constants
            .contains(path)
            .then(|| self.constants.get(path).cloned())
            .flatten()
    }
    fn cached_lowering(&self, path: &DefPath) -> Option<CachedLowering> {
        if let Some(entry) = self
            .lowered
            .get(&DefId::intern(path))
            .filter(|entry| self.current(path, entry))
            && let Ok(output) = entry.result()
            && !crate::comptime::has_staged_reads(&output.value)
        {
            let depth = entry.depth.clone().unwrap_or_else(|| {
                DepthEvidence::leaf(crate::memo::dependency_limit(self.inputs.limits.call_depth))
            });
            if depth.limit() == crate::memo::dependency_limit(self.inputs.limits.call_depth) {
                return Some(CachedLowering {
                    output: (*output).clone(),
                    reads: entry.reads().clone(),
                    depth,
                    verification: entry
                        .verification
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone(),
                });
            }
        }
        None
    }
    fn constant_instance(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
        arguments: &[u64],
    ) -> DependencyProduct<MaterializedRef> {
        if arguments.is_empty() {
            return DependencySource::constant(worker, path);
        }
        worker.capture(|worker| {
            worker
                .staged_at(path, arguments)
                .map(|value| Arc::new(value.value))
        })
    }
    fn instance_body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> DependencyProduct<Option<DefPath>> {
        if self
            .instance_bodies
            .declarations
            .contains(&DefId::intern(path))
            || self
                .items
                .get(path)
                .and_then(|items| items.first())
                .is_some_and(|syntax| generation::instance_bodied(syntax))
        {
            worker.implementation(path);
        }
        match self.instance_bodies.resolve(
            DefId::intern(path),
            self.inputs.snapshot.types(),
            arguments,
        ) {
            Ok(Some(body)) => {
                // Scalar selection checks an instance before returning its
                // executable identity. Keep that order and return its full
                // reads, while deferring first-selection labels to input order.
                let product = self.body(worker, body.path());
                worker.selections.push(SelectionEvent::Checked(
                    body.path().clone(),
                    matches!(product.result, Err(QueryError::Diagnostics(_))),
                ));
                DependencyProduct {
                    result: product.result.map(|_| Some(body.path().clone())),
                    reads: product.reads,
                }
            }
            Ok(None)
                if !self
                    .items
                    .get(path)
                    .and_then(|items| items.first())
                    .is_some_and(|syntax| generation::instance_bodied(syntax)) =>
            {
                DependencyProduct {
                    result: Ok(None),
                    reads: Reads::empty(),
                }
            }
            _ => {
                worker
                    .pending
                    .push(Requirement::Instance(path.clone(), arguments.to_vec()));
                DependencyProduct {
                    result: Err(QueryError::NotImplemented),
                    reads: Reads::empty(),
                }
            }
        }
    }
}

impl QueryEngine {
    /// Cumulative physical worker work for this definition. Scalar queries
    /// retain their existing counters; speculative work is included here.
    pub fn s1_lower_work(&self, definition: &DefPath) -> S1LowerWork {
        self.s1_lower_work
            .get(&DefId::intern(definition))
            .copied()
            .unwrap_or_default()
    }

    /// Freeze only handles and borrowed immutable tables. Demanded ordinary
    /// bodies use the same P worker after hold eviction; no body-table clone.
    pub(crate) fn frozen_s1_source(
        &mut self,
        lower_budget: u64,
        mode_attempts: &BTreeSet<DefPath>,
    ) -> Result<FrozenSource<'_>, QueryError> {
        self.note_environment();
        self.prepare_materialization_environment();
        let runtime = self.comptime_runtime_types()?;
        // Settle the signature universe before checking private acceptance.
        let _ = self.signature_inputs();
        let lowering = self
            .adapters
            .lowering
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        // Validate metadata one value at a time; borrow the original values
        // rather than retaining a second constant graph beside the controller.
        // Each registration round constructs a new immutable eligibility set.
        let mut valid_constants = BTreeSet::new();
        let constants: Vec<_> = self.constant_values.keys().cloned().collect();
        for path in constants {
            let Some((validation, executed)) = self
                .constant_values
                .get(&path)
                .map(|value| (value.validation.clone(), value.executed.clone()))
            else {
                continue;
            };
            if self.evaluation_context_holds(validation.as_deref(), &executed)
                && self.execution_valid(&path, &executed)
            {
                valid_constants.insert(path);
            }
        }
        let snapshot = self.signature_inputs();
        let declarations = self.frozen_declarations();
        let environment = self.environment_value();
        Ok(FrozenSource {
            inputs: TaskInputs {
                snapshot,
                memo: self.memo.clone(),
                lowering,
                runtime,
                sources: self.sources.clone(),
                limits: self.comptime_limits,
                verify: self.verify,
                lower_budget,
            },
            items: &self.items,
            generated: &self.generated,
            signatures: &self.signatures,
            modes: &self.modes,
            checked: &self.checked,
            resolutions: &self.resolutions,
            lowered: &self.lowered,
            constants: &self.constant_values,
            valid_constants,
            mode_attempts: mode_attempts.clone(),
            graph: &self.graph,
            declarations,
            citations: SourceContext {
                unique_failures: &self.private_unique_failures,
                sources: &self.sources,
                items: &self.items,
                declarations: &self.declarations,
                catalog: &self.catalog,
            },
            registry: &self.registry,
            instance_bodies: &self.instance_bodies,
            staged: &self.staged_constants,
            manifests: &self.package_manifests,
            default_manifest: self.environment.package_manifest_hash,
            environment,
            signer: self.adapters.signature.as_deref(),
            resolver: self.adapters.resolver.as_deref(),
            checker: self.adapters.checker.as_deref(),
            mode_phase: self.adapters.modes.as_deref(),
            completed: Mutex::new(vec![]),
            records: self.type_records(),
            store: self.result_store.clone(),
            lower_products: Mutex::new(BTreeMap::new()),
        })
    }
}

impl QueryEngine {
    /// Materialize a constant frontier in parallel, returning results in input
    /// order. Frozen workers resume staged/generated registration rounds;
    /// unavailable syntax or inconsistent receipts retain whole-root retry.
    /// Every returned worker product is checked against current reads before
    /// publication, including after a preceding root changed registrations.
    pub fn materialize_values(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<Result<hirsel_tir::Materialized, QueryError>> {
        self.materialize_value_outcomes(definitions, jobs)
            .into_iter()
            .map(|outcome| outcome.result)
            .collect()
    }
    /// The same worker schedule with root-local operational provenance.
    pub fn materialize_value_outcomes(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<MaterializationResult> {
        self.s1_root_outcomes(definitions, jobs, RootKind::Constant, false)
            .into_iter()
            .map(|outcome| MaterializationResult {
                definition: outcome.definition,
                result: outcome.result.map(|value| match value {
                    RootValue::Constant(value) => value,
                    _ => unreachable!("constant task"),
                }),
                phase: outcome.phase,
                limit: outcome.limit,
                reads: outcome.reads,
            })
            .collect()
    }
    /// Lower independent roots on the same frozen S1 schedule. Compile-time
    /// values, staged declarations and generated instances use the same rounds
    /// and ordered failure publication as constant materialization.
    pub fn lower_definition_outcomes(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<LoweringResult> {
        self.lower_root_results(definitions, jobs, false)
    }
    fn lower_root_results(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
        prefix: bool,
    ) -> Vec<LoweringResult> {
        self.s1_root_outcomes(definitions, jobs, RootKind::Lower, prefix)
            .into_iter()
            .map(|outcome| {
                outcome.lowering(|value| match value {
                    RootValue::Lowered(output, depth, verification) => {
                        (output, depth, verification)
                    }
                    _ => unreachable!("lower task"),
                })
            })
            .collect()
    }
    /// Lower an ordered file prefix, returning through its first rejection.
    /// Later speculative roots must not publish declarations or body products.
    pub fn lower_definition_prefix(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<LoweringResult> {
        self.lower_root_results(definitions, jobs, true)
    }
    /// Check an ordered prefix without retaining its completed root TIR.
    /// Each worker drops the body before returning its diagnostics, ordered
    /// dependencies, read receipts and transparent depth to serial publication.
    pub fn check_lower_definition_prefix(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<LoweringResult<()>> {
        self.s1_root_outcomes(definitions, jobs, RootKind::CheckLower, true)
            .into_iter()
            .map(|outcome| {
                outcome.lowering(|value| match value {
                    RootValue::Checked(output, depth, verification) => {
                        (output, depth, verification)
                    }
                    _ => unreachable!("check lower task"),
                })
            })
            .collect()
    }
    fn s1_root_outcomes(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
        kind: RootKind,
        prefix: bool,
    ) -> Vec<RootOutcome> {
        let mut selections = BTreeMap::new();
        let mut settled = vec![];
        let mut mode_attempts = BTreeSet::new();
        loop {
            let remaining = &definitions[settled.len()..];
            if let Some(mut results) = self.s1_roots_round(
                remaining,
                jobs,
                kind,
                &mut selections,
                &mut mode_attempts,
                prefix,
            ) {
                // Only admitted ordered roots publish durable values.
                if matches!(kind, RootKind::Constant) {
                    for outcome in &mut results {
                        if outcome.result.is_ok()
                            && let Some(value) =
                                self.constant_values.get(&outcome.definition).cloned()
                            && let Err(error) = self.store_evaluation(&outcome.definition, &value)
                        {
                            outcome.result = Err(error);
                        }
                    }
                }
                if let Some(last) = results.last() {
                    self.last_phase = last.phase;
                    self.last_comptime_limit = last.limit.clone();
                    self.unavailable = matches!(last.result, Err(QueryError::NotImplemented))
                        .then_some(last.phase)
                        .flatten();
                }
                let failed = results.last().is_some_and(|result| result.result.is_err());
                settled.extend(results);
                if !prefix || failed || settled.len() == definitions.len() {
                    return settled;
                }
            }
        }
    }
    fn scalar_root_outcome(&mut self, path: &DefPath, kind: RootKind) -> RootOutcome {
        timings::count(Count::S1ScalarRoots);
        self.last_phase = None;
        self.unavailable = None;
        self.last_comptime_limit = None;
        self.execution_stack.push(ReadsBuilder::new());
        let mut result = match kind {
            RootKind::Constant => self.materialize_value(path).map(RootValue::Constant),
            RootKind::Lower | RootKind::CheckLower => {
                // An image shortcut replays its stored execution, but has no
                // resident memo whose root input or V origin we can inherit.
                // Match worker admission at this outcome boundary.
                let mut root_reads = ReadsBuilder::new();
                if let Some(hash) = self.implementation_hash(path) {
                    root_reads.record(
                        ReadKey::ExecutedBody(DefId::intern(path)),
                        memos::optional(Some(hash)),
                    );
                }
                if self.verify == Verify::On {
                    let snapshot = self.signature_inputs();
                    FrozenProvider::new(&snapshot, path.0.package.clone(), &mut root_reads)
                        .validation_universe();
                }
                self.merge_execution(&root_reads.finish().expect("one root snapshot"));
                self.s1.begin_requests();
                let result = self.lower_to_tir(path);
                let depth = self.s1.finish_requests(self.depth_limit());
                result.and_then(|output| {
                    let verification = self.ensure_lowered_verdict(&output.value)?;
                    Ok(match kind {
                        RootKind::CheckLower => RootValue::Checked(
                            worker::discard_lowered_output(output),
                            depth,
                            verification,
                        ),
                        _ => RootValue::Lowered(output, depth, verification),
                    })
                })
            }
        };
        let reads = self
            .execution_stack
            .pop()
            .expect("root outcome read frame")
            .finish();
        let reads = match reads {
            Ok(reads) => reads,
            Err(_) => {
                result = Err(QueryError::NotImplemented);
                Reads::empty()
            }
        };
        self.merge_execution(&reads);
        let phase = if result.is_ok() {
            Some(QueryKind::Lower)
        } else {
            self.unavailable.or(self.last_phase)
        };
        RootOutcome {
            definition: path.clone(),
            result,
            phase,
            limit: self.last_comptime_limit.clone(),
            reads: Arc::new(reads),
            worker_recomputed: false,
        }
    }
    fn s1_roots_round(
        &mut self,
        definitions: &[DefPath],
        jobs: hirsel_intern::jobs::Jobs,
        kind: RootKind,
        selections: &mut BTreeMap<DefPath, hirsel_diag::Label>,
        mode_attempts: &mut BTreeSet<DefPath>,
        prefix: bool,
    ) -> Option<Vec<RootOutcome>> {
        if definitions.is_empty() {
            return Some(vec![]);
        }
        let prepare = timings::interval(Interval::FrontPrepare);
        self.note_environment();
        self.prepare_materialization_environment();
        // Settle only the first admitted durable candidate. An imported body
        // never supplies execution authority: replay it through ordinary Lower
        // publication when this consumer has not issued its own receipt yet.
        // Later roots cannot cross an earlier failure/registration boundary.
        if prefix
            && !matches!(kind, RootKind::Constant)
            && let Some(first) = definitions.first()
        {
            let mut replay = false;
            if let Some(entry) = self.load_lower_with_replay(first, &mut replay) {
                self.lowered.insert(DefId::intern(first), entry);
                drop(prepare);
                return Some(vec![self.scalar_root_outcome(first, kind)]);
            }
            if replay {
                drop(prepare);
                return Some(vec![self.scalar_root_outcome(first, kind)]);
            }
        }
        let mut results: Vec<Option<RootOutcome>> = vec![None; definitions.len()];
        let mut tasks = vec![];
        for (index, definition) in definitions.iter().enumerate() {
            let cached = if matches!(kind, RootKind::Constant) {
                let memory = self
                    .constant_values
                    .get(definition)
                    .cloned()
                    .filter(|value| {
                        self.evaluation_validation_holds(value)
                            && self.execution_valid(definition, &value.executed)
                    });
                memory.or_else(|| self.load_evaluation(definition))
            } else {
                None
            };
            if let Some(value) = cached {
                self.constant_values
                    .insert(definition.clone(), value.clone());
                results[index] = Some(RootOutcome {
                    definition: definition.clone(),
                    result: Ok(RootValue::Constant(value.value)),
                    phase: Some(QueryKind::Lower),
                    limit: None,
                    reads: Arc::new(value.executed),
                    worker_recomputed: false,
                });
            } else {
                // Frozen workers borrow body tables and cannot restore durable
                // products themselves. Restore only this demanded root's ordinary
                // checked body before freezing; execution and V still run under
                // this reader's authority when their own receipt is absent.
                self.restore_execution_mode(definition);
                mode_attempts.insert(definition.clone());
                tasks.push((index, definition.clone()));
            }
        }
        if tasks.is_empty() {
            self.last_comptime_limit = None;
            return Some(results.into_iter().map(Option::unwrap).collect());
        }
        let mut shared = worker::TaskShared::new(self.s1.coordinator().clone());
        let workers = jobs.worker_count(tasks.len()).max(1);
        let lower_budget = self.available_hold_bytes() / workers as u64;
        let source = match self.frozen_s1_source(lower_budget, mode_attempts) {
            Ok(source) => source,
            Err(_) => {
                drop(prepare);
                let mut outcomes = vec![];
                for path in definitions {
                    let outcome = self.scalar_root_outcome(path, kind);
                    let failed = outcome.result.is_err();
                    outcomes.push(outcome);
                    if prefix && failed {
                        break;
                    }
                }
                return Some(outcomes);
            }
        };
        drop(prepare);
        timings::count_by(Count::S1RootTasks, tasks.len() as u64);
        let mut completed = hirsel_intern::jobs::map_with_state(
            jobs,
            tasks,
            |_| 1,
            || TaskWorker::new(&source, &shared),
            |worker, (index, path)| {
                let product = match kind {
                    RootKind::Constant => {
                        let product = worker.root(&path);
                        DependencyProduct {
                            result: product.result.map(RootValue::Constant),
                            reads: product.reads,
                        }
                    }
                    RootKind::Lower => {
                        let product = worker.lower_root(&path);
                        DependencyProduct {
                            result: product.result.map(|(output, depth, verification)| {
                                RootValue::Lowered(output, depth, verification)
                            }),
                            reads: product.reads,
                        }
                    }
                    RootKind::CheckLower => {
                        let product = worker.check_lower_root(&path);
                        DependencyProduct {
                            result: product.result.map(|(output, depth, verification)| {
                                RootValue::Checked(output, depth, verification)
                            }),
                            reads: product.reads,
                        }
                    }
                };
                (
                    index,
                    path,
                    product,
                    worker.limit.take(),
                    worker.phase,
                    std::mem::take(&mut worker.registrations),
                    std::mem::take(&mut worker.pending),
                    std::mem::take(&mut worker.selections),
                )
            },
        );
        // End the frozen source borrow before updating engine-owned counters.
        let lower_products = source
            .lower_products
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());
        let bodies = source
            .completed
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());
        // Worker states have dropped after map joins. Drain physical work
        // before filtering suffixes or returning for a registration retry.
        for (id, work) in shared.take_lower_work() {
            self.s1_lower_work.entry(id).or_default().merge(work);
        }
        // A worker result is tentative until its input position settles.
        // Only the earliest incomplete/rejected root may drive registration;
        // later work cannot publish source or memo products across that barrier.
        let end = if prefix {
            completed
                .iter()
                .filter(|(_, _, product, _, _, _, pending, _)| {
                    product.result.is_err() || !pending.is_empty()
                })
                // Settle earlier successes before a later demand changes the
                // signature universe. Only a barrier at the first root may
                // publish its own required registrations in this round.
                .map(|(index, ..)| if *index == 0 { 1 } else { *index })
                .min()
                .unwrap_or(definitions.len())
        } else {
            definitions.len()
        };
        let dispatched = completed.len();
        completed.retain(|(index, ..)| *index < end);
        timings::count_by(
            Count::S1DiscardedRoots,
            (dispatched - completed.len()) as u64,
        );
        let mut demanded: BTreeSet<DefId> = definitions[..end].iter().map(DefId::intern).collect();
        for (_, _, product, _, _, _, _, _) in &completed {
            for (read, _) in product.reads.iter() {
                if let ReadKey::ExecutedBody(definition) = read {
                    demanded.insert(*definition);
                }
            }
        }
        // Hidden instance paths do not appear in ExecutedBody reads. Retain
        // exactly the requests made by admitted roots, including cached nested
        // constants/lowerings, rather than every instance of a demanded owner.
        let staged_requests: BTreeSet<_> = completed
            .iter()
            .flat_map(|(_, _, _, _, _, _, _, events)| events)
            .filter_map(|event| match event {
                SelectionEvent::StagedValue(request) => Some(request.clone()),
                _ => None,
            })
            .collect();
        let mut bodies = bodies;
        bodies.sort_by(|(left, _, _), (right, _, _)| left.definition().cmp(right.definition()));
        for (body, text, reads) in bodies {
            if (!prefix || demanded.contains(&DefId::intern(body.definition())))
                && self.key_text(body.definition(), QueryKind::Modes).ok() == Some(text)
                && self.execution_valid(body.definition(), &reads)
            {
                let _ = self.merge_compact_completed(body);
            }
        }
        // Resolve only demands from the admitted prefix, after observing
        // their complete reads. Loading Modes publishes no generated names,
        // consumes no execution fuel, and confers no V authority.
        let mut restored = false;
        for (_, path, product, _, _, _, pending, _) in &completed {
            if !self.execution_valid(path, &product.reads) {
                continue;
            }
            for requirement in pending {
                if let Requirement::Modes(path) = requirement
                    && mode_attempts.insert(path.clone())
                {
                    self.restore_execution_mode(path);
                    restored = true;
                }
            }
        }
        if restored {
            return None;
        }
        // A declaration demand suspends evaluation before V. Install only
        // registrations whose complete producer reads still hold, then begin
        // another worker round under the new signature universe.
        let mut declarations = BTreeMap::new();
        for (_, path, product, _, _, registrations, pending, _) in &completed {
            if pending
                .iter()
                .any(|requirement| matches!(requirement, Requirement::Staged(_)))
                && self.execution_valid(path, &product.reads)
            {
                for registration in registrations {
                    if !self
                        .staged_constants
                        .get(&registration.path)
                        .is_some_and(|old| {
                            old.signature.fingerprint == registration.constant.signature.fingerprint
                                && old.inputs == registration.constant.inputs
                        })
                    {
                        declarations.insert(registration.path.clone(), registration.clone());
                    }
                }
            }
        }
        if !declarations.is_empty() {
            for registration in declarations.into_values() {
                self.install_staged(registration);
            }
            timings::count(Count::S1RegistrationRounds);
            return None;
        }
        let mut instances = BTreeMap::new();
        for (_, path, product, _, _, _, pending, _) in &completed {
            if !self.execution_valid(path, &product.reads) {
                continue;
            }
            for requirement in pending {
                if let Requirement::Instance(declaration, arguments) = requirement
                    && let Some(key) = hirsel_tir::InstanceBodies::key(&self.types, arguments)
                {
                    instances
                        .entry((declaration.clone(), key))
                        .or_insert_with(|| arguments.clone());
                }
            }
        }
        let mut registered = false;
        for ((declaration, _), arguments) in instances {
            let Ok(syntax) = self.snapshot(&declaration) else {
                break;
            };
            let actuals = hirsel_types::Actuals { arguments };
            match self.register_generated_instance(&declaration, &actuals, syntax.item.name_span) {
                Ok((instance, fresh)) => {
                    if fresh {
                        let key = hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments)
                            .expect("registered closed actuals");
                        selections.insert(
                            instance,
                            hirsel_diag::Label {
                                span: syntax.item.name_span,
                                message: format!(
                                    "selects instance {}",
                                    key.instance_name(&declaration)
                                ),
                            },
                        );
                    }
                    registered |= fresh;
                }
                Err(_) => break,
            }
        }
        if registered {
            timings::count(Count::S1RegistrationRounds);
            return None;
        }
        let mut generated = shared.take_generators();
        generated.sort_by(|left, right| left.definition.cmp(&right.definition));
        let mut published = false;
        for product in generated {
            if (!prefix || demanded.contains(&DefId::intern(&product.definition)))
                && self.implementation_hash(&product.definition) == Some(product.syntax.text_hash)
                && self.execution_valid(&product.definition, &product.reads)
            {
                match self.merge_generated_text(product) {
                    Ok(()) => published = true,
                    Err(_) => break,
                }
            }
        }
        if published {
            timings::count(Count::S1RegistrationRounds);
            return None;
        }
        // Successes reached inside a failed root remain independently valid.
        // Publish only after all worker requests/claims have settled.
        let mut staged_write_errors = BTreeMap::new();
        for (path, constant, mut registrations, staged) in shared.into_constants() {
            let admitted = !prefix
                || match &staged {
                    Some((path, arguments)) => {
                        staged_requests.contains(&(DefId::intern(path), arguments.clone()))
                    }
                    None => demanded.contains(&DefId::intern(&path)),
                };
            if admitted && self.execution_valid(&path, &constant.executed) {
                registrations.sort_by(|a, b| a.path.cmp(&b.path));
                for registration in registrations {
                    self.install_staged(registration);
                }
                match staged {
                    Some(key) => {
                        // The prefix already admitted this exact selection.
                        // Publish its declaration before its selected value;
                        // compact front receipts suffice without retaining Modes.
                        let owner = self.staged_constants.get(&key.0).map(|c| c.owner.clone());
                        let written = owner
                            .map_or(Ok(()), |owner| self.store_registrations(&owner))
                            .and_then(|()| self.store_staged_evaluation(&key.0, &key.1, &constant));
                        if let Err(error) = written {
                            staged_write_errors
                                .insert((DefId::intern(&key.0), key.1.clone()), error);
                        }
                        self.staged_values.insert(key, constant);
                    }
                    None => {
                        self.constant_values.insert(path, constant);
                    }
                }
            }
        }
        let mut completed: BTreeMap<_, _> = completed
            .into_iter()
            .map(
                |(index, path, product, limit, phase, registrations, pending, selected)| {
                    (
                        index,
                        (
                            path,
                            product,
                            limit,
                            phase,
                            registrations,
                            pending,
                            selected,
                        ),
                    )
                },
            )
            .collect();
        let mut lower_products = lower_products;
        for (index, path) in definitions[..end].iter().enumerate() {
            let Some((_, product, limit, phase, mut registrations, pending, selected)) =
                completed.remove(&index)
            else {
                // A prior root may have changed registrations since preflight.
                // Recheck the hit at its sequential position and clear a limit
                // from an earlier root exactly as scalar materialization does.
                results[index] = Some(self.scalar_root_outcome(path, kind));
                continue;
            };
            let worker_recomputed = selected.iter().any(|event| {
                matches!(event, SelectionEvent::LowerComputed(id) if *id == DefId::intern(path))
            });
            if pending.is_empty() && self.execution_valid(path, &product.reads) {
                registrations.sort_by(|a, b| a.path.cmp(&b.path));
                for registration in registrations {
                    self.install_staged(registration);
                }
                self.last_comptime_limit = limit;
                let mut product = product;
                if product.result.is_ok()
                    && let Some(captured) = lower_products.remove(path)
                    && let Err(error) = self.publish_captured_lower(path, captured)
                {
                    product.result = Err(error);
                }
                results[index] = Some(RootOutcome {
                    definition: path.clone(),
                    result: product.result,
                    phase,
                    limit: self.last_comptime_limit.clone(),
                    reads: Arc::new(product.reads),
                    worker_recomputed,
                });
            } else {
                // No suspended failure becomes a cached language verdict. A
                // whole-root retry preserves the requester's original stack.
                results[index] = Some(self.scalar_root_outcome(path, kind));
            }
            // A registration's first scalar check owns its selecting label;
            // later hits return the underlying cached diagnostics. Worker
            // completion order must not decide which root receives the label.
            let mut error = results[index]
                .as_ref()
                .unwrap()
                .result
                .as_ref()
                .err()
                .cloned();
            for event in selected {
                match event {
                    SelectionEvent::StagedValue(request) => {
                        if let Some(failure) = staged_write_errors.get(&request) {
                            error = Some(failure.clone());
                        }
                    }
                    SelectionEvent::LowerComputed(_) | SelectionEvent::Generated(_) => {}
                    SelectionEvent::Failed(mut product) => {
                        let definition = product.definition.clone();
                        let cached = self.checked.get(&DefId::intern(&definition)).cloned();
                        let cached = cached.filter(|entry| {
                            self.key_text(&definition, QueryKind::Check)
                                .ok()
                                .is_some_and(|text| self.memo_valid(&definition, entry, text))
                        });
                        if let Some(cached) = cached {
                            error = cached.result().err();
                        } else if self.execution_valid(&definition, &product.reads) {
                            // Nested first-selection labels are part of the
                            // enclosing rejected check, not its child's memo.
                            if let Some(error) = &error {
                                product.text = Err(error.clone());
                            }
                            let _ = self.merge_generated_text(product);
                        }
                    }
                    SelectionEvent::Checked(instance, failed) => {
                        if let Some(label) = selections.remove(&instance)
                            && failed
                            && let Some(QueryError::Diagnostics(diagnostics)) = &mut error
                        {
                            for diagnostic in diagnostics {
                                diagnostic.secondary.push(label.clone());
                            }
                        }
                    }
                }
            }
            if let Some(error) = error {
                results[index].as_mut().unwrap().result = Err(error);
            }
            if prefix && results[index].as_ref().unwrap().result.is_err() {
                results.truncate(index + 1);
                return Some(results.into_iter().map(Option::unwrap).collect());
            }
        }
        results.truncate(end);
        Some(results.into_iter().map(Option::unwrap).collect())
    }
}
