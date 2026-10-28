//! Frozen per-definition front work, joined before deterministic memo publication.
//! Go's compile queue (`cmd/compile/internal/gc/compile.go:166-184`) separates
//! worker-local function work from completed products. Hirsel uses jobs::map
//! and publishes by DefPath because subscriber and diagnostic order is semantic.
use super::*;
use hirsel_intern::jobs::{self, Jobs};
use hirsel_types::SignatureRef;
mod compact;
mod scheduled;
pub(crate) use compact::CompactCompleted;
use compact::{FrontResult, WorkerCompleted};
pub(in crate::queries) use scheduled::SpecializationDependencies;
pub use scheduled::{
    FRONT_BODY_KIND, FRONT_STORAGE_KIND, FrontBodyOutput, FrontBodyQueries, FrontCarry,
    FrontCheckedOutput, FrontInputFootprint, FrontStorageOutput, FrontStorageQueries,
};

/// One input's outcome, including the phase that completed or stopped it.
/// Results from `mode_definitions` always retain the caller's input order.
#[derive(Clone, Debug)]
pub struct ModeResult {
    pub definition: DefPath,
    pub result: Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
    pub phase: Option<QueryKind>,
    pub summary: Option<Arc<PublishedSummary>>,
    pub reads: Arc<Reads>,
    pub diagnostic_phases: Vec<(Diagnostic, String)>,
}

/// A published front result retaining no checked or analyzed body.
#[derive(Clone, Debug)]
pub struct PublishedResult {
    pub definition: DefPath,
    pub result: Result<PublishedDefinition, QueryError>,
    pub phase: Option<QueryKind>,
    pub diagnostic_phases: Vec<(Diagnostic, String)>,
    pub reads: Arc<Reads>,
}
#[derive(Clone, Debug)]
pub struct PublishedDefinition {
    pub summary: Arc<PublishedSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone)]
pub(crate) struct Task {
    pub(crate) index: usize,
    pub(crate) definition: DefPath,
    pub(crate) syntax: Arc<ItemSyntax>,
    pub(crate) signature: Result<Arc<QueryOutput<SignatureRef>>, QueryError>,
    pub(crate) signature_reads: Arc<Reads>,
    pub(crate) resolution: Option<Arc<Entry<Resolution>>>,
    pub(crate) checked: Option<Arc<Entry<TypedBody>>>,
    pub(crate) text: ContentHash,
}
pub(crate) struct Stage<T> {
    result: Result<Arc<QueryOutput<T>>, QueryError>,
    detail: DependencyReads,
    observed: Observed,
    unavailable: Option<QueryKind>,
}
impl<T> Stage<T> {
    fn complete(
        &mut self,
        mut result: Result<QueryOutput<T>, QueryError>,
        products: PhaseProducts,
    ) {
        self.observed.products = Arc::new(products);
        prepare_result(&mut self.detail, &mut result, &self.observed.products);
        self.result = result.map(Arc::new);
    }
}
pub(crate) enum Product<T> {
    Hit(Arc<Entry<T>>),
    Computed(Stage<T>),
}
impl<T> Product<T> {
    pub(crate) fn result(&self) -> Result<Arc<QueryOutput<T>>, QueryError> {
        match self {
            Self::Hit(entry) => entry.result(),
            Self::Computed(stage) => stage.result.clone(),
        }
    }
    fn complete_reads(
        &self,
        owner: &DefPath,
        inherited: &Reads,
        source: &mut impl memos::ReadSource,
    ) -> Result<Reads, crate::memo::Inconsistent> {
        match self {
            Self::Hit(entry) => Ok(entry.reads().clone()),
            Self::Computed(stage) => {
                let mut merged = stage.observed.merged.clone();
                merged.merge(inherited);
                let observed = Observed {
                    products: stage.observed.products.clone(),
                    representations: stage.observed.representations.clone(),
                    merged,
                };
                memos::complete_reads(owner, &stage.detail, &observed, source)
            }
        }
    }

    pub(super) fn detail(&self) -> &DependencyReads {
        match self {
            Self::Hit(entry) => &entry.detail,
            Self::Computed(stage) => &stage.detail,
        }
    }
}
pub(crate) struct Completed {
    pub(crate) index: usize,
    pub(crate) definition: DefPath,
    pub(crate) text: ContentHash,
    pub(crate) resolution: Option<Product<Resolution>>,
    pub(crate) checked: Option<Product<TypedBody>>,
    pub(crate) modes: Option<Product<AnalyzedBody>>,
    pub(crate) result: Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
    pub(crate) phase: QueryKind,
    signature_reads: Arc<Reads>,
}

impl Completed {
    pub(super) fn recomputed(&self) -> Vec<QueryKind> {
        let mut work = Vec::new();
        if matches!(self.resolution, Some(Product::Computed(_))) {
            work.push(QueryKind::Resolve);
        }
        if matches!(self.checked, Some(Product::Computed(_))) {
            work.push(QueryKind::Check);
        }
        if matches!(self.modes, Some(Product::Computed(_))) {
            work.push(QueryKind::Modes);
        }
        work
    }
    /// Modes carries the final successful output. `result` otherwise records
    /// the earlier phase that stopped the task before modes was produced.
    pub(crate) fn result(&self) -> Result<Arc<QueryOutput<AnalyzedBody>>, QueryError> {
        self.modes
            .as_ref()
            .map_or_else(|| self.result.clone(), Product::result)
    }
    /// The task's completed receipt without mutating an engine. Inherited
    /// phase/cache reads join in the same order as merge_stage publication.
    /// The caller supplies the exact frozen environment and source inputs.
    pub(crate) fn complete_reads(
        &self,
        source: &mut impl memos::ReadSource,
    ) -> Result<Reads, crate::memo::Inconsistent> {
        let mut reads = Reads::empty();
        if let Some(product) = &self.resolution {
            reads = product.complete_reads(&self.definition, &reads, source)?;
        }
        if let Some(product) = &self.checked {
            reads = product.complete_reads(&self.definition, &reads, source)?;
        }
        if let Some(product) = &self.modes {
            reads = product.complete_reads(&self.definition, &reads, source)?;
        }
        if self.phase == QueryKind::Signature {
            reads = reads.union(&self.signature_reads)?;
        }
        Ok(reads)
    }
}

/// Read finishing over the same immutable context as a body task. Environment
/// is explicit: a manifest digest alone omits compile-time limits and compiler
/// identity. Candidate identity is supplied for the demanded owner, including
/// source-generated definitions that are absent from the declaration index.
pub(crate) struct FrozenReadSource<'a> {
    pub(crate) declarations: &'a Declarations,
    pub(crate) citations: &'a dyn CitationSource,
    pub(crate) candidates: Option<ContentHash>,
    pub(crate) environment: ContentHash,
}
impl memos::ReadSource for FrozenReadSource<'_> {
    fn namespace(
        &mut self,
        context: &FileContext,
        result: &Result<hirsel_types::ReadPath, hirsel_types::LookupError>,
    ) -> Option<hirsel_resolve::Namespace> {
        self.declarations
            .index(context.package.clone())
            .entry(result.as_ref().ok()?)
            .map(|entry| entry.namespace)
    }
    fn lookup_candidates(
        &mut self,
        context: &FileContext,
        package: Option<&hirsel_syntax::PackageId>,
        segments: &[String],
    ) -> ContentHash {
        self.declarations
            .index(context.package.clone())
            .lookup_candidates_revision(context, package, segments)
    }
    fn cited(&self, path: &DefPath) -> ContentHash {
        self.citations.cited_text(DefId::intern(path))
    }
    fn candidates(&self, _: &DefPath) -> Option<ContentHash> {
        self.candidates
    }
    fn environment(&self) -> ContentHash {
        self.environment
    }
}

/// Only immutable phase handles and inputs cross the worker boundary. In
/// particular this has neither QueryEngine nor an interpreter behind a lock.
pub(crate) struct Worker<'a> {
    pub(crate) types: &'a hirsel_types::TypeInterner,
    pub(crate) memo: &'a hirsel_types::TypeMemo,
    pub(crate) snapshot: &'a SignatureSnapshot,
    pub(crate) declarations: &'a Declarations,
    pub(crate) citations: &'a dyn CitationSource,
    pub(crate) sources: &'a SourceMap,
    pub(crate) items: &'a BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    pub(crate) resolver: Option<&'a dyn ResolverPhase>,
    pub(crate) checker: Option<&'a dyn CheckerPhase>,
    pub(crate) modes: Option<&'a dyn ModesPhase>,
}
impl Worker<'_> {
    pub(crate) fn run(&self, task: Task) -> Completed {
        self.run_to(task, false)
    }
    pub(crate) fn run_to(&self, task: Task, inference: bool) -> Completed {
        let mut done = Completed {
            index: task.index,
            definition: task.definition.clone(),
            text: task.text,
            resolution: None,
            checked: None,
            modes: None,
            result: Err(QueryError::NotImplemented),
            phase: QueryKind::Resolve,
            signature_reads: task.signature_reads.clone(),
        };
        let owner = DefId::intern(&task.definition);
        // A checked memo already includes the complete resolution reads. A
        // partial hit needs no resolver work, but remains the same task path.
        let checked = if let Some(entry) = task.checked {
            Product::Hit(entry)
        } else {
            let resolved = if let Some(entry) = task.resolution {
                Product::Hit(entry)
            } else {
                let _timing = timings::phase(Phase::Resolve);
                let mut stage = Stage {
                    result: Err(QueryError::NotImplemented),
                    detail: DependencyReads::default(),
                    observed: Observed::default(),
                    unavailable: None,
                };
                let mut products = PhaseProducts::default();
                let mut provider = WorkerRecorder::new(self, owner, &mut stage);
                let result = match self.resolver {
                    Some(adapter) => adapter.resolve(
                        self.types,
                        self.memo,
                        task.syntax.clone(),
                        self.declarations,
                        &mut products,
                        &mut provider,
                    ),
                    None => Err(QueryError::NotImplemented),
                };
                stage.complete(result, products);
                Product::Computed(stage)
            };
            let resolution = resolved.result();
            let unavailable = match &resolved {
                Product::Computed(stage) => stage.unavailable,
                _ => None,
            };
            let mut initial = resolved.detail().clone();
            done.resolution = Some(resolved);
            let resolution = match resolution {
                Ok(value) => value,
                Err(error) => {
                    if matches!(error, QueryError::NotImplemented) {
                        done.phase = unavailable.unwrap_or(done.phase);
                    }
                    done.result = Err(error);
                    return done;
                }
            };
            done.phase = QueryKind::Signature;
            let signature = match &task.signature {
                Ok(value) => value,
                Err(error) => {
                    done.result = Err(error.clone());
                    return done;
                }
            };
            done.phase = QueryKind::Check;
            initial.signatures.extend(resolution.dependencies.clone());
            initial.signatures.extend(signature.dependencies.clone());
            initial.signatures.push(SignatureDependency {
                definition: hirsel_syntax::DefId::intern(&task.definition),
                fingerprint: signature.value.fingerprint,
            });
            let _timing = timings::phase(Phase::Check);
            if !inference {
                timings::count(Count::Definitions);
            }
            let mut stage = Stage {
                result: Err(QueryError::NotImplemented),
                detail: initial,
                observed: Observed::default(),
                unavailable: None,
            };
            let mut provider = WorkerRecorder::new(self, owner, &mut stage);
            let mut result = match self.checker {
                Some(adapter) => adapter.check(
                    self.types,
                    self.memo,
                    task.syntax,
                    resolution.value.clone(),
                    &signature.value,
                    &mut provider,
                ),
                None => Err(QueryError::NotImplemented),
            };
            if let Ok(output) = &mut result {
                arm_fixes::place_with(
                    output,
                    &mut stage.observed.representations,
                    |owner, site| {
                        arm_fixes::insertion(self.sources, self.items, owner, site)
                            .or_else(|| self.citations.arm_insertion(owner, site))
                    },
                );
            }
            stage.complete(result, PhaseProducts::default());
            Product::Computed(stage)
        };
        done.phase = QueryKind::Check;
        let body = checked.result();
        let unavailable = match &checked {
            Product::Computed(stage) => stage.unavailable,
            _ => None,
        };
        let initial = checked.detail().clone();
        done.checked = Some(checked);
        let body = match body {
            Ok(value) => value,
            Err(error) => {
                if matches!(error, QueryError::NotImplemented) {
                    done.phase = unavailable.unwrap_or(done.phase);
                }
                done.result = Err(error);
                return done;
            }
        };
        if inference {
            return done;
        }
        done.phase = QueryKind::Signature;
        let signature = match task.signature {
            Ok(value) => value,
            Err(error) => {
                done.result = Err(error);
                return done;
            }
        };
        done.phase = QueryKind::Modes;
        let _timing = timings::phase(Phase::Modes);
        let mut stage = Stage {
            result: Err(QueryError::NotImplemented),
            detail: initial,
            observed: Observed::default(),
            unavailable: None,
        };
        let mut products = PhaseProducts::default();
        let mut provider = WorkerRecorder::new(self, owner, &mut stage);
        let result = match self.modes {
            Some(adapter) => adapter.check_modes(
                self.types,
                self.memo,
                Arc::new(body.value.clone()),
                &signature.value,
                &mut products,
                &mut provider,
            ),
            None => Err(QueryError::NotImplemented),
        };
        stage.complete(result, products);
        if matches!(stage.result, Err(QueryError::NotImplemented)) {
            done.phase = stage.unavailable.unwrap_or(done.phase);
        }
        let product = Product::Computed(stage);
        done.modes = Some(product);
        done
    }
}

/// Preserve adapter-level detail as well as the provider's canonical typed
/// reads. The latter is authoritative for memo verification; tooling and
/// semantic closure also consume the former.
struct WorkerRecorder<'a> {
    provider: FrozenProvider<'a>,
    snapshot: &'a SignatureSnapshot,
    detail: &'a mut DependencyReads,
    unavailable: &'a mut Option<QueryKind>,
}
impl<'a> WorkerRecorder<'a> {
    fn new<T>(worker: &'a Worker<'_>, owner: DefId, stage: &'a mut Stage<T>) -> Self {
        Self {
            provider: FrozenProvider::for_definition(
                worker.snapshot,
                owner,
                worker.citations,
                &mut stage.observed.merged,
            ),
            snapshot: worker.snapshot,
            detail: &mut stage.detail,
            unavailable: &mut stage.unavailable,
        }
    }
}
impl SignatureProvider for WorkerRecorder<'_> {
    fn signature(&mut self, definition: DefId) -> Result<SignatureRef, QueryError> {
        let result = self.provider.signature(definition);
        if matches!(result, Err(QueryError::NotImplemented))
            && self.snapshot.signature(definition).is_some()
        {
            *self.unavailable = Some(QueryKind::Signature);
        }
        if let Err(QueryError::Operational(error)) = &result {
            self.detail.operational_failure = Some(error.clone());
        }
        match &result {
            Ok(signature) => self.detail.signatures.push(SignatureDependency {
                definition,
                fingerprint: signature.fingerprint,
            }),
            Err(_) => {
                self.detail
                    .failed_signatures
                    .push(definition.path().clone());
                self.detail.failed_signature_keys.push((
                    definition.path().clone(),
                    self.snapshot.failed_signature_key(definition),
                ));
            }
        }
        result
    }
    fn cite_unique_failure(
        &mut self,
        definition: &DefPath,
        claim: hirsel_types::UniqueClaim,
    ) -> Option<hirsel_types::UniqueFailure> {
        let failure = self.provider.cite_unique_failure(definition, claim);
        self.detail
            .citations
            .push((definition.clone(), failure.as_ref().map(|f| f.span)));
        failure
    }
    fn cite_name(&mut self, definition: &DefPath) -> Option<Span> {
        let cited = self.provider.cite_name(definition);
        self.detail.citations.push((definition.clone(), cited));
        cited
    }
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
    ) -> hirsel_types::LookupOutput {
        let output = self.provider.lookup(context, path);
        self.detail.manifest_lookups.push(
            hirsel_types::DependencyRead::ProviderLookup {
                context: (context.clone()).into(),
                package: path.package.clone(),
                segments: (path
                    .segments
                    .iter()
                    .map(|s| s.as_str().to_owned())
                    .collect::<Vec<_>>())
                .into(),
                output: (output.clone()).into(),
            }
            .shared(),
        );
        if let Err(QueryError::Operational(error)) = &output.result {
            self.detail.operational_failure = Some(error.clone());
        }
        if let Some(path) = lookup_path(context, path) {
            self.detail
                .lookups
                .push((path.into(), output.result.is_ok()));
        }
        output
    }
}

impl QueryEngine {
    /// Publish compact outcomes before releasing bodies according to the
    /// selected retention policy. No returned product retains a TypedBody.
    /// Batch workers release bodies before accepting their next task; compact
    /// receipts await serial publication; encoded store bytes are drained by
    /// the producing worker before it accepts another task.
    pub fn publish_definitions(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: Jobs,
        retention: crate::memo::Retention,
    ) -> Vec<PublishedResult> {
        self.run_front(
            definitions,
            jobs,
            retention == crate::memo::Retention::Batch,
        )
        .into_iter()
        .map(|outcome| match outcome {
            FrontResult::Compact(outcome) => outcome,
            FrontResult::Resident(outcome) => self.compact_front_outcome(outcome, false),
        })
        .collect()
    }

    fn compact_front_outcome(&mut self, outcome: ModeResult, release: bool) -> PublishedResult {
        let definition = DefId::intern(&outcome.definition);
        self.remember_front_facts(definition);
        let result = outcome.result.and_then(|output| {
            outcome
                .summary
                .map(|summary| PublishedDefinition {
                    summary,
                    diagnostics: output.diagnostics.clone(),
                })
                .ok_or_else(compact::missing_summary)
        });
        if release {
            // The published summary owns the complete inherited observations;
            // a successful resolver body is no longer needed by this batch.
            if self
                .resolutions
                .get(&definition)
                .is_some_and(|e| e.memo.value.is_ok())
            {
                self.forget(&MemoKey::new(QueryKind::Resolve, definition));
            }
            // Errors are already body-free and retain their complete receipts.
            if self
                .checked
                .get(&definition)
                .is_some_and(|e| e.memo.value.is_ok())
            {
                self.forget(&MemoKey::new(QueryKind::Check, definition));
            }
            if self
                .modes
                .get(&definition)
                .is_some_and(|e| e.memo.value.is_ok())
            {
                self.forget(&MemoKey::new(QueryKind::Modes, definition));
            }
        }
        let mut published = PublishedResult {
            definition: outcome.definition,
            result,
            phase: outcome.phase,
            reads: outcome.reads,
            diagnostic_phases: outcome.diagnostic_phases,
        };
        if let Err(error) = self.store_publication(&published) {
            published.result = Err(error);
        }
        published
    }

    fn front_output(
        &mut self,
        definition: DefPath,
        result: Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
        phase: Option<QueryKind>,
        inherited: Option<Arc<Reads>>,
        compact: bool,
    ) -> FrontResult {
        let outcome = self.front_outcome(definition, result, phase, inherited);
        if compact {
            FrontResult::Compact(self.compact_front_outcome(outcome, true))
        } else {
            FrontResult::Resident(outcome)
        }
    }

    /// Select whether completed program fronts retain intermediate bodies.
    /// Explicit body-returning queries still fulfill their ordinary contract.
    pub fn set_retention(&mut self, retention: crate::memo::Retention) {
        let changed = self.retention != retention;
        self.retention = retention;
        if changed {
            self.rebuild_lower_holds();
        }
    }

    /// Serial S1 demands also publish compact facts before retiring body
    /// ownership. The active caller's Arc keeps its in-flight artifact alive.
    pub(crate) fn release_batch_front(&mut self, definition: &DefPath) -> Result<(), QueryError> {
        if self.retention != crate::memo::Retention::Batch {
            return Ok(());
        }
        let id = DefId::intern(definition);
        let Some(entry) = self.modes.get(&id).cloned() else {
            return Ok(());
        };
        let saved = (self.last_phase, self.unavailable);
        let changes = self
            .front_changes
            .is_active()
            .then(|| self.invalidate_front_definition(id));
        let outcome = self.front_outcome(
            definition.clone(),
            entry.result(),
            Some(QueryKind::Modes),
            Some(entry.memo.reads.clone()),
        );
        let outcome = self.compact_front_outcome(outcome, true);
        if let Some(changes) = changes {
            self.retain_front_completion(&outcome, &changes);
        }
        (self.last_phase, self.unavailable) = saved;
        outcome.result.map(|_| ())
    }

    fn remember_front_facts(&mut self, id: DefId) {
        if let Some(entry) = self.checked.get(&id)
            && entry.consistent
            && !self.graph.is_dirty(&entry.memo.key)
            && let Ok(output) = entry.result()
            && self.checked_summaries.get(&id).is_some_and(|summary| {
                summary.consistent && !self.graph.is_dirty(&summary.memo.key)
            })
        {
            let signature = self
                .signatures
                .get(&id)
                .and_then(|entry| entry.result().ok());
            let inputs = dead_arms::body_use(
                &self.types,
                signature.as_ref().map(|output| &*output.value),
                &output.value,
            );
            self.front_products.dead_arms.insert(id, inputs);
        }
        if let Some(entry) = self.modes.get(&id)
            && entry.consistent
            && !self.graph.is_dirty(&entry.memo.key)
            && let Ok(output) = entry.result()
            && self.summaries.get(&id).is_some_and(|summary| {
                summary.consistent && !self.graph.is_dirty(&summary.memo.key)
            })
        {
            self.front_products
                .lowering_inputs
                .insert(id, lowering::LoweringUse::capture(&output.value.typed));
        }
    }

    /// Capture this request's products before any later serial boundary can
    /// change inputs or replace the same definition's memo.
    fn front_outcome(
        &mut self,
        definition: DefPath,
        result: Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
        phase: Option<QueryKind>,
        inherited: Option<Arc<Reads>>,
    ) -> ModeResult {
        let inherited = inherited
            .or_else(|| phase.and_then(|kind| self.shared_memo_reads(&definition, kind)))
            .unwrap_or_else(|| Arc::new(Reads::empty()));
        // Most outcomes already own one complete phase read allocation. Only
        // an own-header stop adds a second, independently completed request.
        let reads = if phase == Some(QueryKind::Signature)
            && let Some(signature) = self.shared_memo_reads(&definition, QueryKind::Signature)
        {
            let mut merged = ReadsBuilder::new();
            merged.merge(&inherited);
            merged.merge(&signature);
            merged.finish().map(Arc::new)
        } else {
            Ok(inherited)
        };
        let result = self.publish_front_result(&definition, result);
        let summary = result
            .is_ok()
            .then(|| {
                self.summaries
                    .get(&DefId::intern(&definition))
                    .and_then(|entry| entry.result().ok())
                    .map(|o| o.value.clone())
            })
            .flatten();
        let diagnostics = match &result {
            Ok(output) => &output.diagnostics[..],
            Err(QueryError::Diagnostics(diagnostics)) => &diagnostics[..],
            Err(_) => &[],
        };
        let diagnostic_phases = diagnostics
            .iter()
            .filter_map(|diagnostic| {
                self.diagnostic_phase(diagnostic)
                    .map(|phase| (diagnostic.clone(), phase))
            })
            .collect();
        let (result, summary, reads) = match reads {
            Ok(reads) => (result, summary, reads),
            Err(_) => (
                Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "front receipt capture".into(),
                    message: "completed phase reads disagree".into(),
                })),
                None,
                Arc::new(Reads::empty()),
            ),
        };
        ModeResult {
            definition,
            result,
            phase,
            summary,
            reads,
            diagnostic_phases,
        }
    }
    /// Fused resolve/check/modes work for one frontier. Every worker count uses
    /// jobs::map; only preparation and memo publication mutate the engine.
    /// Declarations that execute generators keep their ordinary serial S1 path.
    pub fn mode_definitions(&mut self, definitions: Vec<DefPath>, jobs: Jobs) -> Vec<ModeResult> {
        self.run_front(definitions, jobs, false)
            .into_iter()
            .map(|outcome| {
                let FrontResult::Resident(outcome) = outcome else {
                    unreachable!()
                };
                outcome
            })
            .collect()
    }
    fn run_front(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: Jobs,
        compact: bool,
    ) -> Vec<FrontResult> {
        self.prepare_private_summaries();
        let mut outputs = Vec::with_capacity(definitions.len());
        let mut frontier = Vec::new();
        for definition in definitions {
            self.last_phase = None;
            self.unavailable = None;
            if self
                .source_facts(&definition)
                .is_some_and(|facts| !facts.generator)
            {
                frontier.push(definition);
                continue;
            }
            match self
                .materialize(&definition)
                .and_then(|_| self.snapshot(&definition))
            {
                Ok(syntax) if !generation::serial(&syntax) => frontier.push(definition),
                state => {
                    let materialization_phase = self.unavailable.or(self.last_phase);
                    let materialization_failed = state.is_err();
                    // Generators can change source/signature inputs. Publish
                    // the preceding frontier before serial S1 work, then build
                    // a fresh immutable context for the following frontier.
                    outputs.extend(self.mode_source_definitions(
                        std::mem::take(&mut frontier),
                        jobs,
                        compact,
                    ));
                    self.last_phase = None;
                    self.unavailable = None;
                    let result = state.and_then(|_| self.mode_query(&definition));
                    let phase = if materialization_failed {
                        materialization_phase
                    } else if result.is_ok() {
                        Some(QueryKind::Modes)
                    } else {
                        self.unavailable.or(self.last_phase)
                    };
                    outputs.push(self.front_output(definition, result, phase, None, compact));
                }
            }
        }
        outputs.extend(self.mode_source_definitions(frontier, jobs, compact));
        if let Some(last) = outputs.last() {
            self.last_phase = last.phase();
            self.unavailable = if last.unavailable() {
                last.phase()
            } else {
                None
            };
        }
        outputs
    }
    fn mode_source_definitions(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: Jobs,
        compact: bool,
    ) -> Vec<FrontResult> {
        self.prepare_private_summaries();
        let prepare = timings::interval(Interval::FrontPrepare);
        let mut outputs: Vec<Option<FrontResult>> = (0..definitions.len()).map(|_| None).collect();
        let mut sources = Vec::new();
        let mut first = BTreeMap::new();
        let mut duplicates = Vec::new();
        for (index, definition) in definitions.into_iter().enumerate() {
            if let Some(&previous) = first.get(&definition) {
                duplicates.push((index, previous));
                continue;
            }
            first.insert(definition.clone(), index);
            self.last_phase = None;
            self.unavailable = None;
            if compact && let Some(outcome) = self.load_publication(&definition) {
                outputs[index] = Some(FrontResult::Compact(outcome));
                continue;
            }
            let syntax = self
                .materialize(&definition)
                .and_then(|_| self.snapshot(&definition));
            match syntax {
                Ok(syntax) if !generation::serial(&syntax) => {
                    sources.push((index, definition, syntax))
                }
                Ok(_) => {
                    let result = self.mode_query(&definition);
                    outputs[index] = Some(self.front_output(
                        definition,
                        result,
                        self.unavailable.or(self.last_phase),
                        None,
                        compact,
                    ));
                }
                Err(error) => {
                    outputs[index] = Some(self.front_output(
                        definition,
                        Err(error),
                        self.unavailable.or(self.last_phase),
                        None,
                        compact,
                    ))
                }
            }
        }
        if sources.is_empty() {
            for (index, previous) in duplicates {
                outputs[index] = outputs[previous].clone();
            }
            return outputs
                .into_iter()
                .map(|result| result.expect("one outcome per input"))
                .collect();
        }
        let mut tasks = Vec::new();
        for (index, definition, syntax) in sources {
            let id = DefId::intern(&definition);
            let text = match self.key_text(&definition, QueryKind::Modes) {
                Ok(text) => text,
                Err(error) => {
                    outputs[index] =
                        Some(self.front_output(definition, Err(error), None, None, compact));
                    continue;
                }
            };
            if let Some(entry) = self.modes.get(&id).cloned()
                && self.memo_valid(&definition, &entry, text)
            {
                let reads = entry.memo.reads.clone();
                let result = self.mode_result(entry);
                outputs[index] = Some(self.front_output(
                    definition,
                    result,
                    Some(QueryKind::Modes),
                    Some(reads),
                    compact,
                ));
                continue;
            }
            if let Some(entry) = self.load_mode(&definition) {
                self.modes.insert(id, entry.clone());
                let reads = entry.memo.reads.clone();
                let result = self.mode_result(entry);
                outputs[index] = Some(self.front_output(
                    definition,
                    result,
                    Some(QueryKind::Modes),
                    Some(reads),
                    compact,
                ));
                continue;
            }
            self.syntax_views(&definition)
                .expect("preflight materialized the syntax");
            let checked = self
                .checked
                .get(&id)
                .cloned()
                .filter(|entry| self.memo_valid(&definition, entry, text));
            let resolution = if checked.is_none() {
                self.resolutions
                    .get(&id)
                    .cloned()
                    .filter(|entry| self.memo_valid(&definition, entry, text))
            } else {
                None
            };
            tasks.push(Task {
                index,
                definition,
                syntax,
                signature: Err(QueryError::NotImplemented),
                signature_reads: Arc::new(Reads::empty()),
                resolution,
                checked,
                text,
            });
        }
        // Restored and resident hits need no worker inputs. In particular,
        // freezing here would validate the entire universe for an empty batch.
        if tasks.is_empty() {
            for (index, previous) in duplicates {
                outputs[index] = outputs[previous].clone();
            }
            return outputs
                .into_iter()
                .map(|result| result.expect("one outcome per input"))
                .collect();
        }
        // Validation of dirty predecessors may normalize a signature. Freeze
        // only afterwards, and never retry a failed header after freezing: a
        // transient provider failure must not mix two universes in one task.
        let saved_phase = (self.last_phase, self.unavailable);
        let snapshot = self.signature_inputs();
        let declarations = self.frozen_declarations();
        (self.last_phase, self.unavailable) = saved_phase;
        for task in &mut tasks {
            task.signature_reads = self
                .shared_memo_reads(&task.definition, QueryKind::Signature)
                .unwrap_or_else(|| Arc::new(Reads::empty()));
            let id = DefId::intern(&task.definition);
            task.signature = match snapshot.signature(id) {
                Some(Ok(signature)) => self
                    .signatures
                    .get(&id)
                    .and_then(|entry| entry.result().ok())
                    .map(Ok)
                    .unwrap_or_else(|| Ok(Arc::new(output(signature.clone())))),
                Some(Err(error)) => Err(error.clone()),
                None => Err(QueryError::NotImplemented),
            };
        }
        let environment = if compact {
            self.environment_value()
        } else {
            ContentHash::default()
        };
        let candidates: BTreeMap<_, _> = if compact {
            tasks
                .iter()
                .map(|task| {
                    (
                        task.definition.clone(),
                        self.candidates_value(&task.definition),
                    )
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        let capture_store = compact
            && self.result_store.is_some()
            && self.s1.is_empty()
            && self.s1.failure_is_own();
        let writers: BTreeMap<_, _> = if capture_store {
            tasks
                .iter()
                .filter_map(|task| {
                    self.mode_writer(&task.definition)
                        .map(|writer| (task.definition.clone(), writer))
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        drop(prepare);
        let completed = {
            let citations = self.citation_source();
            let worker = Worker {
                types: &self.types,
                memo: &self.memo,
                snapshot: &snapshot,
                declarations: &declarations,
                citations: &citations,
                sources: &self.sources,
                items: &self.items,
                resolver: self.adapters.resolver.as_deref(),
                checker: self.adapters.checker.as_deref(),
                modes: self.adapters.modes.as_deref(),
            };
            jobs::map(
                jobs,
                tasks,
                |task| task.syntax.tree.expressions.len() as u64,
                |task| {
                    let done = worker.run(task);
                    if compact {
                        let mut source = FrozenReadSource {
                            declarations: &declarations,
                            citations: &citations,
                            candidates: candidates.get(&done.definition).copied(),
                            environment,
                        };
                        let writer = writers.get(&done.definition);
                        let mut done = CompactCompleted::capture(
                            done,
                            &worker,
                            self.types.clone(),
                            &mut source,
                            writer.map(|writer| &*writer.types),
                        );
                        done.write_payload(writer);
                        WorkerCompleted::Compact(done)
                    } else {
                        WorkerCompleted::Resident(done)
                    }
                },
            )
        };
        let mut completed = completed;
        completed.sort_by(|a, b| {
            a.definition()
                .cmp(b.definition())
                .then(a.index().cmp(&b.index()))
        });
        for done in completed {
            let index = done.index();
            outputs[index] = Some(match done {
                WorkerCompleted::Resident(done) => {
                    FrontResult::Resident(self.merge_completed(done))
                }
                WorkerCompleted::Compact(done) => {
                    FrontResult::Compact(self.merge_compact_completed(done))
                }
            });
        }
        for (index, previous) in duplicates {
            outputs[index] = outputs[previous].clone();
        }
        let outputs: Vec<_> = outputs
            .into_iter()
            .map(|result| result.expect("one outcome per input"))
            .collect();
        if let Some(last) = outputs.last() {
            self.last_phase = last.phase();
            self.unavailable = last.unavailable().then_some(last.phase()).flatten();
        }
        outputs
    }
    /// Publish one completed immutable task through the same tables, receipts,
    /// summary capture and diagnostic metadata as an ordinary P frontier.
    pub(crate) fn merge_completed(&mut self, done: Completed) -> ModeResult {
        let mut inherited = None;
        if let Some(product) = done.resolution {
            let entry = self.merge_stage(
                &done.definition,
                QueryKind::Resolve,
                done.text,
                product,
                None,
            );
            inherited = Some(entry.memo.reads.clone());
            keep(
                &mut self.resolutions,
                &mut self.graph,
                entry,
                self.s1.failure_is_own(),
            );
        }
        if let Some(product) = done.checked {
            let entry = self.merge_stage(
                &done.definition,
                QueryKind::Check,
                done.text,
                product,
                inherited.as_deref(),
            );
            inherited = Some(entry.memo.reads.clone());
            keep(
                &mut self.checked,
                &mut self.graph,
                entry,
                self.s1.failure_is_own(),
            );
        }
        let result = if let Some(product) = done.modes {
            let entry = self.merge_stage(
                &done.definition,
                QueryKind::Modes,
                done.text,
                product,
                inherited.as_deref(),
            );
            inherited = Some(entry.memo.reads.clone());
            let result = self
                .store_mode(&done.definition, &entry)
                .and_then(|_| self.mode_result(entry.clone()));
            if matches!(result, Err(QueryError::Operational(_))) {
                self.forget(&MemoKey::new(
                    QueryKind::Modes,
                    DefId::intern(&done.definition),
                ));
            } else {
                keep(
                    &mut self.modes,
                    &mut self.graph,
                    entry,
                    self.s1.failure_is_own(),
                );
            }
            result
        } else {
            done.result
        };
        self.front_outcome(done.definition, result, Some(done.phase), inherited)
    }
    fn merge_stage<T>(
        &mut self,
        definition: &DefPath,
        kind: QueryKind,
        text: ContentHash,
        product: Product<T>,
        inherited: Option<&Reads>,
    ) -> Arc<Entry<T>> {
        match product {
            Product::Hit(entry) => entry,
            Product::Computed(mut stage) => {
                // Front work cannot execute/reenter the engine. Claim the phase
                // only while publishing so ordinary counters and active-state
                // bookkeeping are shared with the serial query path.
                self.start(definition, kind)
                    .expect("front workers cannot reenter queries");
                if let Some(reads) = inherited {
                    stage.observed.merged.merge(reads);
                }
                Arc::new(self.finish_prepared(
                    MemoKey::new(kind, DefId::intern(definition)),
                    text,
                    stage.detail,
                    stage.result,
                    stage.observed,
                ))
            }
        }
    }
}

#[cfg(test)]
mod frozen_task_tests {
    use super::*;
    use crate::store::ResultStore;

    struct RejectAfterReads;
    impl CheckerPhase for RejectAfterReads {
        fn check(
            &self,
            _: &hirsel_types::TypeInterner,
            _: &hirsel_types::TypeMemo,
            syntax: Arc<ItemSyntax>,
            _: Resolution,
            _: &Signature,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<TypedBody>, QueryError> {
            provider.signature(DefId::intern(&DefPath::core("String")))?;
            let _ = provider.signature(DefId::intern(&DefPath::core("missing-task-input")));
            let _ = provider.cite_name(&syntax.item.definition);
            let _ = provider.lookup(
                &default_context(),
                &hirsel_syntax::Path {
                    package: None,
                    segments: vec![Symbol::intern("missing-task-lookup")],
                },
            );
            Err(QueryError::Diagnostics(vec![Diagnostic::error(
                const { hirsel_diag::RuleId::of("LANG-1") },
                syntax.item.name_span,
                "fixture rejection after reads",
            )]))
        }
    }
    fn fixture() -> (QueryEngine, DefPath) {
        let mut adapters = crate::phases::engine_only_adapters();
        adapters.checker = Some(Arc::new(RejectAfterReads));
        let mut engine = QueryEngine::new(default_environment(), adapters);
        let file = engine
            .sources
            .add("task.hir", "fn a()->(){()}\n".into())
            .unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap();
        let path = parsed.value.items[0].item.definition.clone();
        let id = DefId::intern(&path);
        let ty = engine.types.intern(hirsel_types::Type::unit());
        let signature = Arc::new(Signature {
            definition: id,
            kind: hirsel_types::SignatureKind::Function(hirsel_types::FunctionType {
                state_promises: Default::default(),
                generics: Default::default(),
                parameters: Default::default(),
                result: ty,
                error: None,
                allocation: hirsel_types::Allocation::MayAllocate,
                authority: hirsel_types::AuthoritySummary::Known(Default::default()),
                uses: hirsel_types::Uses::Unrestricted,
                contracts: Default::default(),
                incomplete: false,
                foreign: None,
            }),
            metadata: Default::default(),
            fingerprint: ContentHash::of_bytes(b"task header"),
        });
        let text = engine.key_text(&path, QueryKind::Signature).unwrap();
        let entry = engine.finish(
            MemoKey::new(QueryKind::Signature, id),
            text,
            DependencyReads::default(),
            Ok(output(signature)),
            Observed::default(),
        );
        engine.signatures.insert(id, Arc::new(entry));
        (engine, path)
    }
    fn runtime_errors() -> crate::runtime::RuntimeErrors {
        let diagnostic = |text: &[u8]| {
            crate::store::Domain {
                kind: 0x4600_00ff,
                schema: 1,
            }
            .reference(text)
            .unwrap()
        };
        crate::runtime::RuntimeErrors {
            resource: diagnostic(b"resource"),
            cycle: diagnostic(b"cycle"),
            cancelled: diagnostic(b"cancelled"),
            protocol: diagnostic(b"protocol"),
            panicked: diagnostic(b"panic"),
        }
    }
    #[test]
    fn admitted_front_body_failure_matches_scalar_reads_and_retains_snapshot_admission() {
        use crate::runtime::{AdmittedRuntime, MemoryWeight, ProcessBudget, QueryRuntime};
        crate::with_compiler_stack(|| {
            let (mut engine, path) = fixture();
            let store = Arc::new(crate::store::MemStore::default());
            engine = engine.with_result_store(store.clone());
            let budget = ProcessBudget::new(512 << 20);
            let mut runtime = AdmittedRuntime::new(
                Jobs::new(1),
                budget.reserve(256 << 20).unwrap(),
                runtime_errors(),
            )
            .unwrap();
            let queries = engine
                .front_body_queries(
                    &[path.clone()],
                    MemoryWeight {
                        retained: 1 << 20,
                        scratch: 1 << 20,
                        stack: 0,
                    },
                    budget.reserve(2 << 20).unwrap(),
                    runtime_errors(),
                )
                .unwrap();
            let query = queries.query(&path).unwrap();
            // Constructing a factory/task does no checking and no publication.
            let factory = queries.factory();
            let lazy = factory.create(query).unwrap();
            assert!(queries.outcomes_for_test().is_empty());
            drop(lazy);
            runtime.register(FRONT_BODY_KIND, factory).unwrap();
            let request = runtime.request(&[query, query]);
            let error = runtime
                .wait(request)
                .expect_err("authored checker rejects after its reads");
            let held = queries
                .outcome(error)
                .expect("failure metadata survives the task");
            let actual = &held.outcome;
            assert_eq!(actual.phase, Some(QueryKind::Check));
            let Err(QueryError::Diagnostics(actual_diagnostics)) = &actual.result else {
                panic!("lost language failure");
            };
            let Err(QueryError::Diagnostics(expected_diagnostics)) = engine.check_query(&path)
            else {
                panic!("scalar checker did not reject");
            };
            assert_eq!(*actual_diagnostics, expected_diagnostics);
            assert_eq!(
                &*actual.reads,
                engine.checked[&DefId::intern(&path)].reads()
            );
            assert!(store.get_content(error).is_some());
            assert_eq!(runtime.stats().claims, 1);
            assert_eq!(runtime.stats().polls, 1);
            runtime.shutdown();
            drop(queries);
            assert_eq!(
                budget.reserved(),
                2 << 20,
                "held body metadata owns its snapshot reservation"
            );
            drop(held);
            assert_eq!(budget.reserved(), 0);
        });
    }
    #[test]
    fn frozen_task_receipts_equal_scalar_failure_with_cold_and_partial_hits() {
        crate::with_compiler_stack(|| {
            for partial in [false, true] {
                let (mut engine, path) = fixture();
                if partial {
                    engine.resolve_query(&path).unwrap();
                }
                let id = DefId::intern(&path);
                let snapshot = engine.signature_inputs();
                let declarations = engine.frozen_declarations();
                let environment = engine.environment_value();
                let candidates = engine.candidates_value(&path);
                let citations = engine.citation_source();
                let worker = Worker {
                    types: &engine.types,
                    memo: &engine.memo,
                    snapshot: &snapshot,
                    declarations: &declarations,
                    citations: &citations,
                    sources: &engine.sources,
                    items: &engine.items,
                    resolver: engine.adapters.resolver.as_deref(),
                    checker: engine.adapters.checker.as_deref(),
                    modes: None,
                };
                let done = worker.run(Task {
                    index: 0,
                    definition: path.clone(),
                    syntax: engine.items[&path][0].clone(),
                    signature: engine.signatures[&id].result(),
                    signature_reads: engine.signatures[&id].memo.reads.clone(),
                    resolution: partial.then(|| engine.resolutions[&id].clone()),
                    checked: None,
                    text: ContentHash::default(),
                });
                assert_eq!(done.phase, QueryKind::Check);
                assert!(matches!(done.result, Err(QueryError::Diagnostics(_))));
                let actual = done
                    .complete_reads(&mut FrozenReadSource {
                        declarations: &declarations,
                        citations: &citations,
                        candidates: Some(candidates),
                        environment,
                    })
                    .unwrap();
                drop(citations);
                assert!(matches!(
                    engine.check_query(&path),
                    Err(QueryError::Diagnostics(_))
                ));
                let expected = engine.checked[&id].reads();
                assert_eq!(&actual, expected, "partial={partial}");
                assert!(
                    actual
                        .get(&ReadKey::FailedSignature(DefId::intern(&DefPath::core(
                            "missing-task-input"
                        ))))
                        .is_some()
                );
                assert_eq!(actual.get(&ReadKey::Environment), Some(environment));
                assert_eq!(
                    actual.get(&ReadKey::DeclarationCandidates(id)),
                    Some(candidates)
                );
                assert!(actual.get(&ReadKey::CitedText(id)).is_some());
            }
        });
    }
    #[test]
    fn frozen_task_header_stop_inherits_the_already_observed_header_receipt() {
        crate::with_compiler_stack(|| {
            let (mut engine, path) = fixture();
            let snapshot = engine.signature_inputs();
            let declarations = engine.frozen_declarations();
            let environment = engine.environment_value();
            let candidates = engine.candidates_value(&path);
            let citations = engine.citation_source();
            let worker = Worker {
                types: &engine.types,
                memo: &engine.memo,
                snapshot: &snapshot,
                declarations: &declarations,
                citations: &citations,
                sources: &engine.sources,
                items: &engine.items,
                resolver: engine.adapters.resolver.as_deref(),
                checker: None,
                modes: None,
            };
            let key = ReadKey::ExecutedBody(DefId::intern(&DefPath::core("header-read")));
            let receipt = ContentHash::of_bytes(b"frozen header dependency");
            let done = worker.run(Task {
                index: 0,
                definition: path.clone(),
                syntax: engine.items[&path][0].clone(),
                signature: Err(QueryError::NotImplemented),
                signature_reads: Arc::new(Reads::from_vec(vec![(key, receipt)]).unwrap()),
                resolution: None,
                checked: None,
                text: ContentHash::default(),
            });
            assert_eq!(done.phase, QueryKind::Signature);
            let reads = done
                .complete_reads(&mut FrozenReadSource {
                    declarations: &declarations,
                    citations: &citations,
                    candidates: Some(candidates),
                    environment,
                })
                .unwrap();
            assert_eq!(reads.get(&key), Some(receipt));
            assert_eq!(reads.get(&ReadKey::Environment), Some(environment));
        });
    }
}
