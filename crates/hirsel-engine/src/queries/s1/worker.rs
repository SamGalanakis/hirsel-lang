//! S1's depth-first task worker. Shared inputs and successful constant products
//! are immutable; only the short success-memo operations hold a lock. Query
//! execution never borrows an engine or holds a shared query lock.
use super::*;
use crate::comptime::{
    ComptimeValueSource, EvaluatedConstant, EvaluationInput, StagedDeclarationInput,
    StagedRegistration, prepare_staged_types, prepare_values,
};
use crate::memo::{DepthEvidence, Entered, HoldStats, Holds, MemoKey, S1Coordinator, S1Worker};
use crate::queries::generation::task::{GeneratedText, PreparedGenerator};
use hirsel_interp::aggregate::RuntimeTypes;
use hirsel_syntax::ExprKind;
use hirsel_tir::Materialized;
use hirsel_types::SignatureKind;
use std::sync::Mutex;

/// A demand absent from this frozen epoch. It is returned explicitly to the
/// serial merger; it never masquerades as a final unavailable-phase verdict.
#[derive(Clone, Debug)]
pub(crate) enum Requirement {
    Syntax(DefPath),
    Modes(DefPath),
    Staged(DefPath),
    Receipt(ReadKey),
    Generate(DefPath),
    ConstantInstance(DefPath, Vec<u64>),
    Instance(DefPath, Vec<GenericArgument>),
}

pub(super) type StagedRequest = (DefId, Vec<u64>);

pub(super) enum SelectionEvent {
    LowerComputed(DefId),
    StagedValue(StagedRequest),
    Generated(DefId),
    Failed(GeneratedText),
    Checked(DefPath, bool),
}

pub(crate) struct TaskInputs {
    pub(crate) snapshot: Arc<SignatureSnapshot>,
    pub(crate) memo: Arc<hirsel_types::TypeMemo>,
    pub(crate) lowering: Arc<dyn LoweringPhase>,
    pub(crate) runtime: Arc<RuntimeTypes>,
    pub(crate) sources: SourceMap,
    pub(crate) limits: crate::ComptimeLimits,
    pub(crate) verify: Verify,
    /// This worker's share of the frontier's retained lower-body budget.
    pub(crate) lower_budget: u64,
}

pub(crate) struct TaskBody {
    pub(crate) analyzed: Arc<QueryOutput<AnalyzedBody>>,
    pub(crate) reads: Reads,
    pub(crate) dependencies: Vec<SignatureDependency>,
    pub(crate) detail: DependencyReads,
}

/// A current retained source result, including V's local weak capability.
#[derive(Clone)]
pub(super) struct CachedLowering {
    pub(super) output: QueryOutput<DefinitionRef>,
    pub(super) reads: Reads,
    pub(super) depth: DepthEvidence<Span>,
    pub(super) verification: Option<Arc<hirsel_validate::VerifiedDefinition>>,
}

/// Immutable source inputs, with task-local requests supplied explicitly.
/// A source may compute a demanded body through immutable phase handles; it
/// must return its complete read record on failure as well as on success.
pub(crate) trait TirSource: Sync {
    fn inputs(&self) -> &TaskInputs;
    fn implementation(&self, path: &DefPath) -> Option<ContentHash>;
    fn anchor_owner(&self, _: hirsel_span::Anchor) -> Option<(DefPath, u32)> {
        None
    }
    fn site(&self, path: &DefPath) -> Result<Span, QueryError>;
    fn body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
    ) -> DependencyProduct<Arc<TaskBody>>;
    fn dictionary_constant(&self, path: &DefPath) -> Result<Option<Materialized>, QueryError>;
    fn generator_site(&self, path: &DefPath) -> Result<Span, QueryError> {
        self.site(path)
    }
    fn prepare_generator(&self, _: &DefPath) -> DependencyProduct<PreparedGenerator> {
        DependencyProduct {
            result: Err(QueryError::NotImplemented),
            reads: Reads::empty(),
        }
    }

    fn has_staged_declaration(&self, _: &StagedRegistration) -> bool {
        true
    }
    fn staged_constant(&self, _: &DefPath) -> Option<crate::comptime::StagedConstant> {
        None
    }
    fn cached_constant(&self, _: &DefPath) -> Option<EvaluatedConstant> {
        None
    }
    fn cached_lowering(&self, _: &DefPath) -> Option<CachedLowering> {
        None
    }
    fn completed_lower(
        &self,
        _: &QueryOutput<DefinitionRef>,
        _: &DependencyReads,
        _: &Reads,
        _: &DepthEvidence<Span>,
        _: &[crate::comptime::Selection],
        _: Option<hirsel_validate::ExecutionReceipt>,
    ) {
    }
    fn constant_instance(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
        arguments: &[u64],
    ) -> DependencyProduct<MaterializedRef>;
    fn instance_body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> DependencyProduct<Option<DefPath>>;
}

#[derive(Clone)]
struct ConstantProduct {
    constant: EvaluatedConstant,
    registrations: Vec<StagedRegistration>,
    staged: Option<(DefPath, Vec<u64>)>,
    requests: Vec<crate::comptime::Selection>,
}

/// Reification sees the same private shape and executed implementation receipt
/// as EngineMaterialization, as well as the frozen provider's view digest.
struct TaskMaterialization<'a, S: TirSource + ?Sized> {
    provider: FrozenProvider<'a>,
    source: &'a S,
    implementation_reads: ReadsBuilder,
}
impl<S: TirSource + ?Sized> crate::materialized::MaterializationSource
    for TaskMaterialization<'_, S>
{
    fn signature(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        SignatureProvider::signature(&mut self.provider, definition)
    }
    fn body_view(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        let public = SignatureProvider::signature(&mut self.provider, definition)?;
        let private = self.provider.body_view(definition)?;
        if !Arc::ptr_eq(&public, &private)
            && let Some(hash) = self.source.implementation(definition.path())
        {
            self.implementation_reads.record(
                ReadKey::ExecutedBody(definition),
                memos::optional(Some(hash)),
            );
        }
        Ok(private)
    }
}

#[derive(Clone)]
struct GeneratorProduct {
    text: GeneratedText,
    requests: Vec<crate::comptime::Selection>,
}

pub(crate) struct TaskShared {
    coordinator: Arc<S1Coordinator<DefPath>>,
    constants: Mutex<BTreeMap<DefPath, ConstantProduct>>,
    generators: Mutex<BTreeMap<DefPath, GeneratorProduct>>,
    /// Aggregate counters; peak is the sum of per-worker insertion peaks.
    lower_holds: Mutex<HoldStats>,
    lower_work: Mutex<hirsel_syntax::DefMap<S1LowerWork>>,
}
impl TaskShared {
    pub(crate) fn new(coordinator: Arc<S1Coordinator<DefPath>>) -> Self {
        Self {
            coordinator,
            constants: Mutex::new(BTreeMap::new()),
            generators: Mutex::new(BTreeMap::new()),
            lower_holds: Mutex::new(HoldStats::default()),
            lower_work: Mutex::new(Default::default()),
        }
    }
    pub(super) fn take_lower_work(&mut self) -> hirsel_syntax::DefMap<S1LowerWork> {
        std::mem::take(
            self.lower_work
                .get_mut()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }
    pub(super) fn take_generators(&mut self) -> Vec<GeneratedText> {
        std::mem::take(
            self.generators
                .get_mut()
                .unwrap_or_else(|error| error.into_inner()),
        )
        .into_values()
        .map(|product| product.text)
        .collect()
    }
    pub(super) fn into_constants(
        self,
    ) -> Vec<(
        DefPath,
        EvaluatedConstant,
        Vec<StagedRegistration>,
        Option<(DefPath, Vec<u64>)>,
    )> {
        self.constants
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .into_iter()
            .map(|(path, product)| {
                (
                    path,
                    product.constant,
                    product.registrations,
                    product.staged,
                )
            })
            .collect()
    }
}

struct Lowered {
    definition: DefinitionRef,
    verification: Option<Arc<hirsel_validate::VerifiedDefinition>>,
    acceptance: Option<hirsel_validate::AcceptedArtifact>,
    reads: Reads,
    depth: DepthEvidence<Span>,
    registrations: Vec<StagedRegistration>,
    requests: Vec<crate::comptime::Selection>,
    diagnostics: Vec<Diagnostic>,
    dependencies: Vec<SignatureDependency>,
}

/// Check consumers retain the facts, while the active worker releases its TIR.
pub(super) fn discard_lowered_output(output: QueryOutput<DefinitionRef>) -> QueryOutput<()> {
    QueryOutput {
        value: (),
        diagnostics: output.diagnostics,
        dependencies: output.dependencies,
    }
}

pub(crate) struct TaskWorker<'a, S: TirSource + ?Sized> {
    source: &'a S,
    shared: &'a TaskShared,
    stack: S1Worker<DefPath, Span>,
    /// Clone once per worker; nested evaluation reuses this recorder stack.
    runtime: Arc<RuntimeTypes>,
    reads: Vec<ReadsBuilder>,
    acceptance: Vec<Option<Vec<hirsel_validate::AcceptedArtifact>>>,
    lowered: Holds<Arc<Lowered>>,
    lower_work: hirsel_syntax::DefMap<S1LowerWork>,
    pub(crate) registrations: Vec<StagedRegistration>,
    pub(crate) limit: Option<hirsel_tir::ToolingLimit>,
    pub(super) phase: Option<QueryKind>,
    pub(super) pending: Vec<Requirement>,
    pub(super) selections: Vec<SelectionEvent>,
}
impl<S: TirSource + ?Sized> Drop for TaskWorker<'_, S> {
    fn drop(&mut self) {
        let mut total_work = self
            .shared
            .lower_work
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for (id, work) in std::mem::take(&mut self.lower_work) {
            timings::count_by(Count::S1LowerQueryAttempts, work.query_attempts);
            timings::count_by(Count::S1LowerAdapterCalls, work.adapter_calls);
            total_work.entry(id).or_default().merge(work);
        }
        drop(total_work);
        let stats = self.lowered.stats();
        timings::count_by(Count::S1LowerHoldEvictions, stats.evictions);
        timings::count_by(Count::S1LowerHoldRecomputes, stats.recomputes);
        timings::count_by(Count::S1LowerHoldPeakBytes, stats.peak_bytes);
        let mut total = self
            .shared
            .lower_holds
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        total.held += stats.held;
        total.bytes += stats.bytes;
        total.peak_bytes += stats.peak_bytes;
        total.evictions += stats.evictions;
        total.recomputes += stats.recomputes;
    }
}
impl<'a, S: TirSource + ?Sized> TaskWorker<'a, S> {
    pub(crate) fn new(source: &'a S, shared: &'a TaskShared) -> Self {
        Self {
            source,
            shared,
            stack: shared.coordinator.worker(),
            runtime: Arc::new((*source.inputs().runtime).clone()),
            reads: vec![],
            acceptance: vec![],
            lowered: Holds::new(source.inputs().lower_budget),
            lower_work: Default::default(),
            registrations: vec![],
            limit: None,
            phase: Some(QueryKind::Lower),
            pending: vec![],
            selections: vec![],
        }
    }
    fn record_acceptance(&mut self, acceptance: Option<hirsel_validate::AcceptedArtifact>) {
        for frame in &mut self.acceptance {
            match (&mut *frame, &acceptance) {
                (Some(proofs), Some(proof)) => {
                    if !proofs.contains(proof) {
                        proofs.push(proof.clone());
                    }
                }
                (_, None) => *frame = None,
                _ => (),
            }
        }
    }
    fn merge_acceptance(&mut self, value: &EvaluatedConstant) {
        if let Some(receipt) = &value.acceptance {
            for proof in receipt.artifacts() {
                self.record_acceptance(Some(proof.clone()));
            }
        } else {
            self.acceptance.fill(None);
        }
    }
    fn finish_acceptance(&mut self, result: &mut Result<EvaluatedConstant, QueryError>) {
        let receipt =
            self.finish_execution_acceptance(result.as_ref().ok().map(|value| &value.executed));
        if let Ok(value) = result {
            value.acceptance = receipt;
        }
    }
    fn finish_execution_acceptance(
        &mut self,
        reads: Option<&Reads>,
    ) -> Option<hirsel_validate::ExecutionReceipt> {
        let proofs = self
            .acceptance
            .pop()
            .expect("worker evaluation acceptance frame");
        if let Some(reads) = reads {
            let mut contexts: Vec<_> = reads
                .iter()
                .filter_map(|(read, digest)| match read {
                    ReadKey::ValidationUniverse(package) => Some((package.id(), *digest)),
                    _ => None,
                })
                .collect();
            contexts.sort_unstable();
            return proofs
                .as_ref()
                .and_then(|proofs| hirsel_validate::ExecutionReceipt::compose(&contexts, proofs));
        }
        None
    }
    pub(super) fn capture<T>(
        &mut self,
        run: impl FnOnce(&mut Self) -> Result<T, QueryError>,
    ) -> DependencyProduct<T> {
        self.reads.push(ReadsBuilder::new());
        let result = run(self);
        match self.reads.pop().expect("task read frame").finish() {
            Ok(reads) => DependencyProduct { result, reads },
            Err(error) => {
                self.pending.push(Requirement::Receipt(error.0));
                DependencyProduct {
                    result: Err(QueryError::NotImplemented),
                    reads: Reads::empty(),
                }
            }
        }
    }
    pub(super) fn root(&mut self, path: &DefPath) -> DependencyProduct<Materialized> {
        self.limit = None;
        self.phase = Some(QueryKind::Lower);
        let product = self.capture(|worker| {
            let site = worker.source.site(path)?;
            worker
                .constant_at(path, site)
                .map(|constant| constant.value)
        });
        // Go's function worker releases task-owned working data before taking
        // the next task (gc/compile.go:166-184). Success products already own
        // complete read/depth/registration replay; helper TIR is task-local.
        self.lowered.clear();
        product
    }
    pub(super) fn lower_root(
        &mut self,
        path: &DefPath,
    ) -> DependencyProduct<(
        QueryOutput<DefinitionRef>,
        DepthEvidence<Span>,
        Option<Arc<hirsel_validate::VerifiedDefinition>>,
    )> {
        self.lower_root_with(path, |output| output)
    }
    pub(super) fn check_lower_root(
        &mut self,
        path: &DefPath,
    ) -> DependencyProduct<(
        QueryOutput<()>,
        DepthEvidence<Span>,
        Option<Arc<hirsel_validate::VerifiedDefinition>>,
    )> {
        self.lower_root_with(path, discard_lowered_output)
    }
    fn lower_root_with<T>(
        &mut self,
        path: &DefPath,
        finish: impl FnOnce(QueryOutput<DefinitionRef>) -> QueryOutput<T>,
    ) -> DependencyProduct<(
        QueryOutput<T>,
        DepthEvidence<Span>,
        Option<Arc<hirsel_validate::VerifiedDefinition>>,
    )> {
        self.limit = None;
        self.phase = Some(QueryKind::Lower);
        self.stack.begin_requests();
        let product = self.capture(|worker| {
            worker.implementation(path);
            worker.lower_output(path)
        });
        let depth = self.stack.finish_requests(self.depth_limit());
        self.lowered.clear();
        DependencyProduct {
            result: product
                .result
                .map(|(output, verification)| (finish(output), depth, verification)),
            reads: product.reads,
        }
    }
    fn execution_selections_since(&self, start: usize) -> Vec<crate::comptime::Selection> {
        self.selections[start..]
            .iter()
            .filter_map(|event| match event {
                SelectionEvent::StagedValue((id, arguments)) => {
                    Some(crate::comptime::Selection::Staged(*id, arguments.clone()))
                }
                SelectionEvent::Generated(id) => Some(crate::comptime::Selection::Generated(*id)),
                _ => None,
            })
            .collect()
    }
    pub(super) fn replay_selections(&mut self, selections: &[crate::comptime::Selection]) {
        self.selections
            .extend(selections.iter().map(|selection| match selection {
                crate::comptime::Selection::Staged(id, arguments) => {
                    SelectionEvent::StagedValue((*id, arguments.clone()))
                }
                crate::comptime::Selection::Generated(id) => SelectionEvent::Generated(*id),
            }));
    }
    fn merge(&mut self, reads: &Reads) {
        for frame in &mut self.reads {
            frame.merge(reads);
        }
    }
    fn record(&mut self, key: ReadKey, value: ContentHash) {
        for frame in &mut self.reads {
            frame.record(key, value);
        }
    }
    fn merge_product<T>(&mut self, product: DependencyProduct<T>) -> Result<T, QueryError> {
        self.merge(&product.reads);
        product.result
    }
    pub(super) fn implementation(&mut self, path: &DefPath) {
        if let Some(hash) = self.source.implementation(path) {
            self.record(
                ReadKey::ExecutedBody(DefId::intern(path)),
                memos::optional(Some(hash)),
            );
        }
    }
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        let previous = self.phase.replace(QueryKind::Signature);
        let inputs = self.source.inputs();
        let mut reads = ReadsBuilder::new();
        let result = FrozenProvider::new(&inputs.snapshot, path.0.package.clone(), &mut reads)
            .signature(DefId::intern(path));
        self.merge(&reads.finish().map_err(|_| QueryError::NotImplemented)?);
        if result.is_ok() {
            self.phase = previous;
        }
        result
    }
    fn body(&mut self, path: &DefPath) -> Result<Arc<TaskBody>, QueryError> {
        let source = self.source;
        let product = source.body(self, path);
        let body = self.merge_product(product)?;
        self.merge(&body.reads);
        for (read, _) in body.reads.iter() {
            if let ReadKey::Representation(id) = read
                && let Some(hash) = self.source.implementation(id.path())
            {
                self.record(ReadKey::LayoutBody(*id), memos::optional(Some(hash)));
            }
        }
        self.execution_signatures(&body.dependencies);
        Ok(body)
    }
    fn execution_signatures(&mut self, dependencies: &[SignatureDependency]) {
        for dependency in dependencies {
            let id = dependency.definition;
            if let Some(signature) = self.source.inputs().snapshot.signature(id) {
                let fingerprint = signature.ok().map(|signature| signature.fingerprint);
                self.record(
                    ReadKey::ExecutedSignature(id),
                    memos::signature_value(fingerprint),
                );
            }
        }
    }
    fn depth_limit(&self) -> u32 {
        crate::memo::dependency_limit(self.source.inputs().limits.call_depth)
    }
    fn overflow(&mut self, site: Span) -> QueryError {
        let limit = hirsel_tir::ToolingLimit {
            kind: hirsel_tir::ToolingLimitKind::CallDepth,
            limit: self.depth_limit() as u64,
            site: Some(site),
        };
        self.phase = Some(QueryKind::Lower);
        let error = QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "compile-time tooling limit".into(),
            message: format!("{limit:?}"),
        });
        self.limit = Some(limit);
        error
    }
    fn admit(&mut self, depth: &DepthEvidence<Span>, site: Span) -> Result<(), QueryError> {
        self.stack
            .admit(depth, site)
            .map_err(|site| self.overflow(site))
    }
    pub(crate) fn constant_at(
        &mut self,
        path: &DefPath,
        site: Span,
    ) -> Result<EvaluatedConstant, QueryError> {
        if let Some(value) = self.source.cached_constant(path) {
            self.replay_selections(&value.selections);
            self.admit(&value.depth, site)?;
            self.merge_acceptance(&value);
            self.merge(&value.executed);
            self.stack.requested(&value.depth, site);
            return Ok(value);
        }
        if let Some(value) = self.source.dictionary_constant(path)? {
            return Ok(EvaluatedConstant {
                value,
                substitutions: vec![],
                executed: Reads::empty(),
                depth: DepthEvidence::leaf(self.depth_limit()),
                selections: vec![],
                validation: None,
                acceptance: None,
            });
        }
        loop {
            let cached = self
                .shared
                .constants
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(path)
                .cloned();
            if let Some(cached) = cached {
                self.admit(&cached.constant.depth, site)?;
                self.merge_acceptance(&cached.constant);
                self.merge(&cached.constant.executed);
                self.registrations.extend(cached.registrations);
                self.replay_selections(&cached.requests);
                self.stack.requested(&cached.constant.depth, site);
                return Ok(cached.constant);
            }
            if let Some(start) = self.stack.on_stack(path) {
                self.stack.fail_cycle();
                let diagnostics = self
                    .stack
                    .cycle_sites(start, site)
                    .map(|site| {
                        Diagnostic::error(
                            const { hirsel_diag::RuleId::of("CT-4") },
                            site,
                            "compile-time dependency cycle",
                        )
                    })
                    .collect();
                let mut diagnostics =
                    crate::select_diagnostics(diagnostics, Some(&self.source.inputs().sources));
                return Err(QueryError::Diagnostics(vec![diagnostics.remove(0)]));
            }
            self.admit(&DepthEvidence::leaf(self.depth_limit()), site)?;
            if self.stack.enter(path.clone(), site) == Entered::Run {
                break;
            }
        }
        let selection_start = self.selections.len();
        let registration_start = self.registrations.len();
        let pending_start = self.pending.len();
        self.acceptance
            .push((self.source.inputs().verify == Verify::On).then(Vec::new));
        let product = self.capture(|worker| {
            worker.implementation(path);
            let signature = worker.signature(path)?;
            let body = worker.body(path)?;
            let file = &body.analyzed.value.typed.syntax;
            let hirsel_syntax::ItemKind::Const { value, .. } = file.item.kind else {
                return Err(QueryError::NotImplemented);
            };
            worker.register_staged(&body.analyzed.value)?;
            let types = worker.source.inputs().snapshot.types().clone();
            let mut constants =
                prepare_values(&types, &body.analyzed.value, &[], &signature, worker)?;
            let root_expression = value;
            let dictionary = crate::dictionaries::declaration(
                &types,
                &worker.source.inputs().memo.clone(),
                &body.analyzed.value,
                &signature,
                &constants,
                worker,
            )?;
            let is_dictionary = dictionary.is_some();
            let value = if let Some(dictionary) = dictionary {
                dictionary
            } else if let Some((_, value)) = constants.iter().find(|(id, _)| *id == value) {
                value.clone()
            } else {
                let ty = body
                    .analyzed
                    .value
                    .typed
                    .checked
                    .expressions
                    .iter()
                    .find(|row| row.expression == value)
                    .ok_or(QueryError::NotImplemented)?
                    .ty;
                worker.evaluate(EvaluationInput {
                    analyzed: &body.analyzed.value,
                    staged: &[],
                    signature: &signature,
                    expression: value,
                    child: value,
                    ty,
                    constants: &constants,
                    sizes: &[],
                })?
            };
            if is_dictionary && !constants.iter().any(|(id, _)| *id == root_expression) {
                constants.push((root_expression, value.clone()));
            }
            Ok((value, constants))
        });
        let depth = self.stack.composed(self.depth_limit(), true);
        let mut result = product
            .result
            .map(|(value, substitutions)| EvaluatedConstant {
                value,
                substitutions,
                executed: product.reads,
                depth,
                selections: vec![],
                validation: None,
                acceptance: None,
            });
        self.finish_acceptance(&mut result);
        if let Ok(value) = &mut result {
            value.selections = self.execution_selections_since(selection_start);
        }
        if self.pending.len() == pending_start
            && let Ok(value) = &result
        {
            self.shared
                .constants
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    path.clone(),
                    ConstantProduct {
                        constant: value.clone(),
                        registrations: self.registrations[registration_start..].to_vec(),
                        staged: None,
                        requests: self.execution_selections_since(selection_start),
                    },
                );
        }
        self.stack.leave();
        if let Ok(value) = &result {
            self.merge(&value.executed);
            self.stack.requested(&value.depth, site);
        }
        result
    }
    fn register_staged(&mut self, analyzed: &AnalyzedBody) -> Result<(), QueryError> {
        let types = self.source.inputs().snapshot.types().clone();
        let memo = self.source.inputs().memo.clone();
        let mut missing = false;
        for row in &analyzed.typed.checked.expressions {
            if !matches!(
                analyzed.typed.syntax.tree.expressions[row.expression.0 as usize].kind,
                ExprKind::Comptime(_)
            ) {
                continue;
            }
            let inputs = hirsel_check_protocol::body::comptime_inputs(
                &types,
                &analyzed.typed.syntax,
                &analyzed.typed.checked,
                row.expression,
            );
            if inputs.is_empty() {
                continue;
            }
            let owner = &analyzed.typed.syntax.item.definition;
            let signature = self.signature(owner)?;
            let registration = StagedDeclarationInput {
                analyzed,
                expression: row.expression,
                ty: row.ty,
                inputs,
            }
            .prepare(&types, &memo, &signature)?;
            if !self.source.has_staged_declaration(&registration) {
                self.pending.push(Requirement::Staged(owner.clone()));
                missing = true;
            }
            self.registrations.push(registration);
        }
        if missing {
            Err(QueryError::NotImplemented)
        } else {
            Ok(())
        }
    }
    /// A size-specialized constant is a normal coordinated S1 request. Its
    /// declaration was merged before this frozen epoch; execution stays local.
    pub(super) fn staged_at(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<EvaluatedConstant, QueryError> {
        let Some(constant) = self.source.staged_constant(path) else {
            self.pending.push(Requirement::ConstantInstance(
                path.clone(),
                arguments.to_vec(),
            ));
            return Err(QueryError::NotImplemented);
        };
        self.record(
            ReadKey::Signature(DefId::intern(path)),
            memos::signature_value(Some(constant.signature.fingerprint)),
        );
        if constant.inputs.len() != arguments.len() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "compile-time evaluation".into(),
                message: "staged constant arity".into(),
            }));
        }
        self.selections.push(SelectionEvent::StagedValue((
            DefId::intern(path),
            arguments.to_vec(),
        )));
        let body = self.body(&constant.owner)?;
        let site = body.analyzed.value.typed.syntax.tree.expressions
            [constant.expression.0 as usize]
            .origin
            .span;
        let instance = hirsel_tir::constant_instance_path(path, arguments);
        loop {
            let cached = self
                .shared
                .constants
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&instance)
                .cloned();
            if let Some(cached) = cached {
                self.admit(&cached.constant.depth, site)?;
                self.merge_acceptance(&cached.constant);
                self.merge(&cached.constant.executed);
                self.registrations.extend(cached.registrations);
                self.replay_selections(&cached.requests);
                self.stack.requested(&cached.constant.depth, site);
                return Ok(cached.constant);
            }
            if self.stack.on_stack(&instance).is_some() {
                self.stack.fail_cycle();
                return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                    const { hirsel_diag::RuleId::of("CT-4") },
                    site,
                    "compile-time dependency cycle",
                )]));
            }
            self.admit(&DepthEvidence::leaf(self.depth_limit()), site)?;
            if self.stack.enter(instance.clone(), site) == Entered::Run {
                break;
            }
        }
        let selection_start = self.selections.len();
        let registration_start = self.registrations.len();
        let pending_start = self.pending.len();
        self.acceptance
            .push((self.source.inputs().verify == Verify::On).then(Vec::new));
        let product = self.capture(|worker| {
            worker.implementation(&constant.owner);
            worker.merge(&body.reads);
            worker.execution_signatures(&body.dependencies);
            let signature = worker.signature(&constant.owner)?;
            let inputs = worker.source.inputs();
            let mut reads = ReadsBuilder::new();
            let staged = prepare_staged_types(
                inputs.snapshot.types(),
                &body.analyzed.value,
                &mut FrozenProvider::new(
                    &inputs.snapshot,
                    constant.owner.0.package.clone(),
                    &mut reads,
                ),
            );
            worker.merge(&reads.finish().map_err(|_| QueryError::NotImplemented)?);
            let staged = staged?;
            let constants = prepare_values(
                worker.source.inputs().snapshot.types(),
                &body.analyzed.value,
                &staged,
                &signature,
                worker,
            )?;
            let file = &body.analyzed.value.typed.syntax;
            let ExprKind::Comptime(child) =
                file.tree.expressions[constant.expression.0 as usize].kind
            else {
                return Err(QueryError::NotImplemented);
            };
            let ty = body
                .analyzed
                .value
                .typed
                .checked
                .expressions
                .iter()
                .find(|row| row.expression == constant.expression)
                .ok_or(QueryError::NotImplemented)?
                .ty;
            let sizes: Vec<_> = constant
                .inputs
                .iter()
                .copied()
                .zip(arguments.iter().copied())
                .collect();
            let value = worker.evaluate(EvaluationInput {
                analyzed: &body.analyzed.value,
                staged: &staged,
                signature: &signature,
                expression: constant.expression,
                child,
                ty,
                constants: &constants,
                sizes: &sizes,
            })?;
            Ok((value, constants))
        });
        let depth = self.stack.composed(self.depth_limit(), true);
        self.merge(&product.reads);
        let mut result = product
            .result
            .map(|(value, substitutions)| EvaluatedConstant {
                value,
                substitutions,
                executed: product.reads,
                depth,
                selections: vec![],
                validation: None,
                acceptance: None,
            });
        self.finish_acceptance(&mut result);
        if let Ok(value) = &mut result {
            value.selections = self.execution_selections_since(selection_start);
        }
        if self.pending.len() == pending_start
            && let Ok(value) = &result
        {
            self.shared
                .constants
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    instance,
                    ConstantProduct {
                        constant: value.clone(),
                        registrations: self.registrations[registration_start..].to_vec(),
                        staged: Some((path.clone(), arguments.to_vec())),
                        requests: self.execution_selections_since(selection_start),
                    },
                );
        }
        self.stack.leave();
        if let Ok(value) = &result {
            self.stack.requested(&value.depth, site);
        }
        result
    }
    pub(super) fn replay_generator(
        &mut self,
        depth: &DepthEvidence<Span>,
        site: Span,
    ) -> Result<(), QueryError> {
        self.admit(depth, site)?;
        self.stack.requested(depth, site);
        Ok(())
    }
    /// Prepare and execute a generator privately. The source cannot expose its
    /// body until its text and registrations pass the serial publication join.
    pub(super) fn generate_body(&mut self, path: &DefPath) -> DependencyProduct<Arc<TaskBody>> {
        self.capture(|worker| {
            worker.generator_text(path)?;
            worker.pending.push(Requirement::Generate(path.clone()));
            Err(QueryError::NotImplemented)
        })
    }
    fn generator_text(&mut self, path: &DefPath) -> Result<(), QueryError> {
        let site = self.source.generator_site(path)?;
        loop {
            if self.stack.on_stack(path).is_some() {
                self.stack.fail_cycle();
                return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                    const { hirsel_diag::RuleId::of("CT-4") },
                    site,
                    "compile-time dependency cycle: this generator runs its own declaration",
                )]));
            }
            let hit = self
                .shared
                .generators
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(path)
                .cloned();
            if let Some(hit) = hit {
                self.replay_generator(&hit.text.depth, site)?;
                self.merge(&hit.text.reads);
                if let Some(receipt) = &hit.text.acceptance {
                    for artifact in receipt.artifacts() {
                        self.record_acceptance(Some(artifact.clone()));
                    }
                } else {
                    self.acceptance.fill(None);
                }
                self.replay_selections(&hit.requests);
                return Ok(());
            }
            if self.stack.enter(path.clone(), site) == Entered::Run {
                break;
            }
        }
        let pending_start = self.pending.len();
        let selection_start = self.selections.len();
        let mut metadata = None;
        self.acceptance
            .push((self.source.inputs().verify == Verify::On).then(Vec::new));
        let product = self.capture(|worker| {
            worker.implementation(path);
            worker.signature(path)?;
            let prepared = worker.source.prepare_generator(path);
            let prepared = worker.merge_product(prepared)?;
            metadata = Some((
                prepared.syntax.clone(),
                prepared.body,
                prepared.products.clone(),
                prepared.detail.clone(),
            ));
            let checked = prepared.checked.as_ref().map_err(Clone::clone)?;
            worker.register_staged(&checked.analyzed)?;
            let inputs = worker.source.inputs();
            let constants = prepare_values(
                inputs.snapshot.types(),
                &checked.analyzed,
                &[],
                &checked.signature,
                worker,
            )?;
            let value = prepared.input.value;
            let (result, _) = worker.run_evaluation(EvaluationInput {
                analyzed: &checked.analyzed,
                staged: &[],
                signature: &checked.signature,
                expression: value,
                child: value,
                ty: checked.ty,
                constants: &constants,
                sizes: &[],
            })?;
            prepared.input.decode(result)
        });
        let depth = self.stack.composed(self.depth_limit(), false);
        let acceptance = self.finish_execution_acceptance(Some(&product.reads));
        self.merge(&product.reads);
        let result = product.result;
        if self.pending.len() == pending_start
            && let Some((syntax, body, products, detail)) = metadata
        {
            let completed = GeneratedText {
                definition: path.clone(),
                syntax,
                body,
                text: result.clone(),
                reads: product.reads,
                depth: depth.clone(),
                products,
                detail,
                acceptance,
                selections: self.execution_selections_since(selection_start),
            };
            if result.is_ok() {
                self.shared
                    .generators
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(
                        path.clone(),
                        GeneratorProduct {
                            text: completed,
                            requests: self.execution_selections_since(selection_start),
                        },
                    );
            } else if result.as_ref().is_err_and(|error| error.cacheable())
                && self.stack.failure_is_own()
            {
                self.selections.push(SelectionEvent::Failed(completed));
            }
        }
        let completed_request = result.is_ok()
            || (result.as_ref().is_err_and(|error| error.cacheable())
                && self.stack.failure_is_own());
        self.stack.leave();
        self.phase = Some(QueryKind::Check);
        if completed_request {
            self.stack.requested(&depth, site);
        }
        result.map(|_| ())
    }
    fn run_evaluation(
        &mut self,
        input: EvaluationInput<'_>,
    ) -> Result<(hirsel_interp::Value, hirsel_types::TypeId), QueryError> {
        self.phase = Some(QueryKind::Lower);
        let inputs = self.source.inputs();
        let site = input.analyzed.typed.syntax.tree.expressions[input.expression.0 as usize]
            .origin
            .span;
        let prepared = input.lower(inputs.snapshot.types(), &inputs.memo, &*inputs.lowering)?;
        let admission =
            ValidatedEvaluation::recorded(prepared, &inputs.snapshot, inputs.verify, site);
        self.merge(&admission.reads);
        let validated = admission.result?;
        let runtime = self.runtime.clone();
        let product = validated.run(&inputs.sources, inputs.limits, &runtime, self);
        self.merge(&product.value.reads);
        self.record_acceptance(product.acceptance);
        if product.limit.is_some() {
            self.limit = product.limit;
        }
        product.value.result
    }
    fn validate_lowered(
        &mut self,
        path: &DefPath,
        definition: &DefinitionRef,
        verification: Option<Arc<hirsel_validate::VerifiedDefinition>>,
    ) -> Result<
        (
            Option<hirsel_validate::AcceptedArtifact>,
            Option<Arc<hirsel_validate::VerifiedDefinition>>,
        ),
        QueryError,
    > {
        self.phase = Some(QueryKind::Lower);
        let inputs = self.source.inputs();
        if inputs.verify == Verify::Off {
            return Ok((None, None));
        }
        let mut reads = ReadsBuilder::new();
        FrozenProvider::new(&inputs.snapshot, path.0.package.clone(), &mut reads)
            .validation_universe();
        self.merge(&reads.finish().expect("one immutable validation universe"));
        if let Some(verification) = verification.filter(|evidence| {
            evidence.holds(
                definition,
                inputs.snapshot.types(),
                inputs.snapshot.source_verification_policy(),
                inputs
                    .snapshot
                    .validation(&definition.path.path().0.package),
            )
        }) {
            let acceptance = verification.accepted_artifact();
            self.record_acceptance(acceptance.clone());
            return Ok((acceptance, Some(verification)));
        }
        let _time = timings::phase(Phase::Verify);
        let (report, ran, acceptance, verification) = inputs
            .snapshot
            .validate_source_artifact(definition, |anchor| self.source.anchor_owner(anchor));
        self.record_acceptance(acceptance.clone());
        if ran {
            timings::count(Count::VRuns);
            self.lower_work
                .entry(DefId::intern(path))
                .or_default()
                .validation_runs += 1;
        }
        if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
            return Err(error);
        }
        if report.violations.is_empty() {
            Ok((acceptance, verification.map(Arc::new)))
        } else {
            Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "TIR validation".into(),
                message: format!("{:?}", report.violations),
            }))
        }
    }
    pub(super) fn lower(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        let (output, _) = self.lower_output(path)?;
        // Executing a cached artifact observes its signature dependencies just
        // as executing a freshly lowered body does. A scalar receipt produced
        // outside execution need not already carry ExecutedSignature reads.
        self.execution_signatures(&output.dependencies);
        Ok(output.value)
    }
    fn lower_output(
        &mut self,
        path: &DefPath,
    ) -> Result<
        (
            QueryOutput<DefinitionRef>,
            Option<Arc<hirsel_validate::VerifiedDefinition>>,
        ),
        QueryError,
    > {
        if let Some(CachedLowering {
            output,
            reads,
            depth,
            verification,
        }) = self.source.cached_lowering(path)
        {
            self.merge(&reads);
            let (_, verification) = self.validate_lowered(path, &output.value, verification)?;
            self.stack
                .replay_requests(&depth)
                .map_err(|site| self.overflow(site))?;
            return Ok((output, verification));
        }
        let key = MemoKey::new(QueryKind::Lower, DefId::intern(path));
        if let Some(hit) = self.lowered.get(&key).cloned() {
            self.stack
                .replay_requests(&hit.depth)
                .map_err(|site| self.overflow(site))?;
            self.merge(&hit.reads);
            self.record_acceptance(hit.acceptance.clone());
            self.registrations.extend(hit.registrations.iter().cloned());
            self.replay_selections(&hit.requests);
            return Ok((
                QueryOutput {
                    value: hit.definition.clone(),
                    diagnostics: hit.diagnostics.clone(),
                    dependencies: hit.dependencies.clone(),
                },
                hit.verification.clone(),
            ));
        }
        self.lowered.demand_evicted(&key);
        self.lower_work
            .entry(DefId::intern(path))
            .or_default()
            .query_attempts += 1;
        self.selections
            .push(SelectionEvent::LowerComputed(DefId::intern(path)));
        self.stack.begin_requests();
        let selection_start = self.selections.len();
        let registration_start = self.registrations.len();
        self.acceptance.push(Some(vec![]));
        let product = self.capture(|worker| {
            let body = worker.body(path)?;
            let signature = worker.signature(path)?;
            worker.register_staged(&body.analyzed.value)?;
            let inputs = worker.source.inputs();
            let types = inputs.snapshot.types().clone();
            let mut reads = ReadsBuilder::new();
            let staged = prepare_staged_types(
                &types,
                &body.analyzed.value,
                &mut FrozenProvider::new(&inputs.snapshot, path.0.package.clone(), &mut reads),
            );
            worker.merge(&reads.finish().map_err(|_| QueryError::NotImplemented)?);
            let staged = staged?;
            let materialized = if matches!(signature.kind, SignatureKind::Const { .. }) {
                worker
                    .constant_at(path, body.analyzed.value.typed.syntax.item.name_span)?
                    .substitutions
            } else {
                prepare_values(&types, &body.analyzed.value, &staged, &signature, worker)?
            };
            let inputs = worker.source.inputs();
            worker
                .lower_work
                .entry(DefId::intern(path))
                .or_default()
                .adapter_calls += 1;
            let output = inputs.lowering.lower(
                &types,
                &inputs.memo,
                &body.analyzed.value,
                &Comptime {
                    materialized,
                    staged,
                },
                &signature,
            )?;
            let definition = Arc::new(output.value);
            let (acceptance, verification) = worker.validate_lowered(path, &definition, None)?;
            if has_errors(&output.diagnostics) {
                return Err(QueryError::Diagnostics(output.diagnostics));
            }
            let mut dependencies = body.dependencies.clone();
            dependencies.extend(body.dependencies.iter().cloned());
            for dependency in &output.dependencies {
                worker.record(
                    ReadKey::Signature(dependency.definition),
                    dependency.fingerprint,
                );
            }
            dependencies.extend(output.dependencies);
            Ok((
                definition,
                output.diagnostics,
                dependencies,
                acceptance,
                verification,
                body.detail.clone(),
            ))
        });
        let depth = self.stack.finish_requests(self.depth_limit());
        let execution =
            self.finish_execution_acceptance(product.result.as_ref().ok().map(|_| &product.reads));
        self.merge(&product.reads);
        let (definition, diagnostics, dependencies, acceptance, verification, mut detail) =
            product.result?;
        if self.pending.is_empty() {
            detail.signatures = dependencies.clone();
            self.source.completed_lower(
                &QueryOutput {
                    value: definition.clone(),
                    diagnostics: diagnostics.clone(),
                    dependencies: dependencies.clone(),
                },
                &detail,
                &product.reads,
                &depth,
                &self.execution_selections_since(selection_start),
                execution,
            );
        }
        let held = Arc::new(Lowered {
            definition: definition.clone(),
            verification: verification.clone(),
            acceptance,
            reads: product.reads,
            depth,
            registrations: self.registrations[registration_start..].to_vec(),
            requests: self.execution_selections_since(selection_start),
            diagnostics: diagnostics.clone(),
            dependencies: dependencies.clone(),
        });
        let bytes = crate::memo::retained_tir_bytes(&held.definition)
            .saturating_add(crate::memo::retained_arc_bytes(&*held))
            .saturating_add(crate::memo::retained_lowering_metadata_bytes(
                &held.reads,
                &held.depth,
                &held.registrations,
            ))
            .saturating_add(crate::memo::retained_staged_request_bytes(&held.requests))
            .saturating_add(crate::memo::retained_lowering_output_bytes(
                &held.diagnostics,
                &held.dependencies,
            ));
        let bytes = bytes.saturating_add(held.verification.as_ref().map_or(0, |e| {
            std::mem::size_of::<hirsel_validate::VerifiedDefinition>() as u64
                + 2 * std::mem::size_of::<usize>() as u64
                + e.reads().capacity() as u64 * std::mem::size_of::<hirsel_validate::VRead>() as u64
        }));
        self.lowered.hold(key, held, bytes);
        Ok((
            QueryOutput {
                value: definition,
                diagnostics,
                dependencies,
            },
            verification,
        ))
    }
}
impl<S: TirSource + ?Sized> ComptimeValueSource for TaskWorker<'_, S> {
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        self.signature(path)
    }
    fn constant(&mut self, path: &DefPath, site: Span) -> Result<Materialized, QueryError> {
        self.constant_at(path, site).map(|value| value.value)
    }
    fn evaluate(&mut self, input: EvaluationInput<'_>) -> Result<Materialized, QueryError> {
        self.phase = Some(QueryKind::Lower);
        let inputs = self.source.inputs();
        let package = input.signature.definition.path().0.package.clone();
        let site = input.analyzed.typed.syntax.tree.expressions[input.expression.0 as usize]
            .origin
            .span;
        let (value, ty) = self.run_evaluation(input)?;
        let mut reads = ReadsBuilder::new();
        let mut materializer = TaskMaterialization {
            provider: FrozenProvider::new(&inputs.snapshot, package, &mut reads),
            source: self.source,
            implementation_reads: ReadsBuilder::new(),
        };
        let value = crate::materialized::reify_value(
            inputs.snapshot.types(),
            &inputs.memo,
            &mut materializer,
            value,
            ty,
            site,
        );
        let implementation_reads = materializer
            .implementation_reads
            .finish()
            .map_err(|_| QueryError::NotImplemented)?;
        self.merge(&reads.finish().map_err(|_| QueryError::NotImplemented)?);
        self.merge(&implementation_reads);
        value
    }
}
impl<S: TirSource + ?Sized> DependencySource for TaskWorker<'_, S> {
    fn definition(&mut self, path: &DefPath) -> DependencyProduct<DefinitionRef> {
        self.capture(|worker| {
            worker.implementation(path);
            let definition = worker.lower(path)?;
            let signature = worker.signature(path)?;
            if matches!(&signature.kind, SignatureKind::Type(ty) if ty.resource)
                && definition.entry != worker.source.inputs().lowering.entry_body()
            {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "compile-time evaluation".into(),
                    message: "finalizer entry disagrees with the producer layout".into(),
                }));
            }
            Ok(definition)
        })
    }
    fn constant(&mut self, path: &DefPath) -> DependencyProduct<MaterializedRef> {
        self.capture(|worker| {
            let site = worker.source.site(path)?;
            worker
                .constant_at(path, site)
                .map(|value| Arc::new(value.value))
        })
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> DependencyProduct<MaterializedRef> {
        let source = self.source;
        source.constant_instance(self, path, arguments)
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> DependencyProduct<Option<DefPath>> {
        let source = self.source;
        source.instance_body(self, path, arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_intern::jobs::{self, Jobs};
    use hirsel_types::{
        CheckedBody, Elaboration, OwnershipFacts, Resolution, TypeInterner, TypeMemo, UseKind,
    };

    /// Tiny authored H fixture: all values are unit, but source dependency
    /// selection, real lowering artifacts, V and interpretation are exercised.
    struct UnitLower;
    impl LoweringPhase for UnitLower {
        fn lower(
            &self,
            types: &TypeInterner,
            _: &TypeMemo,
            body: &AnalyzedBody,
            _: &Comptime,
            _: &Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            let mut prepared = super::super::tests::reading_artifact(
                types,
                &body.typed.syntax.item.definition,
                &DefPath::core("unused"),
            );
            prepared.definition.bodies[0].rvalues[0] =
                hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::Unit);
            Ok(output(prepared.definition))
        }
        fn lower_evaluation(
            &self,
            types: &TypeInterner,
            memo: &TypeMemo,
            body: &AnalyzedBody,
            _: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            signature: &Signature,
            expression: hirsel_syntax::ExprId,
            _: hirsel_types::TypeId,
            _: &[(hirsel_syntax::ExprId, Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            let mut definition = self
                .lower(types, memo, body, &Comptime::default(), signature)?
                .value;
            if let ExprKind::Call { callee, .. } =
                body.typed.syntax.tree.expressions[expression.0 as usize].kind
            {
                let hirsel_types::ResolvedName::Definition(path) = &body
                    .typed
                    .checked
                    .resolution
                    .expressions
                    .iter()
                    .find(|(id, _)| *id == callee)
                    .unwrap()
                    .1
                else {
                    panic!("fixture callee");
                };
                let entry = &mut definition.bodies[0];
                let mut returned = entry.blocks[0].clone();
                returned.instructions.clear();
                entry.blocks[0].instructions.truncate(1);
                entry.blocks[0].terminator = hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Definition {
                        path: path.clone(),
                        arguments: vec![],
                    },
                    arguments: vec![],
                    returns: Some(hirsel_tir::CallReturn {
                        destination: hirsel_tir::PlaceId(0),
                        next: hirsel_tir::BlockId(1),
                    }),
                    on_failure: hirsel_tir::CleanupId(0),
                };
                entry.blocks.push(returned);
                entry.rvalues.clear();
            }
            Ok(definition)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            hirsel_tir::BodyId(0)
        }
    }
    struct Source {
        inputs: TaskInputs,
        bodies: BTreeMap<DefPath, Arc<TaskBody>>,
        cached: Option<CachedLowering>,
    }
    impl TirSource for Source {
        fn cached_lowering(&self, path: &DefPath) -> Option<CachedLowering> {
            self.cached
                .as_ref()
                .filter(|hit| hit.output.value.path.path() == path)
                .cloned()
        }
        fn inputs(&self) -> &TaskInputs {
            &self.inputs
        }
        fn implementation(&self, path: &DefPath) -> Option<ContentHash> {
            self.bodies
                .get(path)
                .map(|body| body.analyzed.value.typed.syntax.text_hash)
        }
        fn site(&self, path: &DefPath) -> Result<Span, QueryError> {
            self.bodies
                .get(path)
                .map(|body| body.analyzed.value.typed.syntax.item.name_span)
                .ok_or(QueryError::NotImplemented)
        }
        fn body(
            &self,
            _: &mut TaskWorker<'_, Self>,
            path: &DefPath,
        ) -> DependencyProduct<Arc<TaskBody>> {
            DependencyProduct {
                result: self
                    .bodies
                    .get(path)
                    .cloned()
                    .ok_or(QueryError::NotImplemented),
                reads: Reads::empty(),
            }
        }
        fn dictionary_constant(&self, _: &DefPath) -> Result<Option<Materialized>, QueryError> {
            Ok(None)
        }
        fn constant_instance(
            &self,
            worker: &mut TaskWorker<'_, Self>,
            path: &DefPath,
            arguments: &[u64],
        ) -> DependencyProduct<MaterializedRef> {
            assert!(arguments.is_empty());
            DependencySource::constant(worker, path)
        }
        fn instance_body(
            &self,
            _: &mut TaskWorker<'_, Self>,
            _: &DefPath,
            _: &[GenericArgument],
        ) -> DependencyProduct<Option<DefPath>> {
            DependencyProduct {
                result: Ok(None),
                reads: Reads::empty(),
            }
        }
    }
    fn path(name: &str) -> DefPath {
        DefPath(hirsel_syntax::ItemPath {
            package: default_context().package,
            modules: vec![],
            name: name.into(),
        })
    }
    fn source(text: &str, limit: usize) -> Source {
        let mut engine =
            QueryEngine::new(default_environment(), crate::phases::engine_only_adapters());
        let file = engine.sources.add("tasks.hir", text.to_owned()).unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap();
        let types = engine.types.clone();
        let unit = types.intern(hirsel_types::Type::unit());
        let boolean = types.intern(hirsel_types::Type::boolean());
        let mut signatures = vec![];
        let mut bodies = BTreeMap::new();
        for syntax in parsed.value.items {
            let mut resolution = Resolution::default();
            for (id, expression) in syntax.tree.expressions.iter().enumerate() {
                if let ExprKind::Path(reference) = &expression.kind {
                    resolution.expressions.push((
                        hirsel_syntax::ExprId(id as u32),
                        hirsel_types::ResolvedName::Definition(path(
                            reference.segments[0].as_str(),
                        )),
                    ));
                }
            }
            let signature = Arc::new(Signature {
                definition: DefId::intern(&syntax.item.definition),
                kind: if matches!(syntax.item.kind, hirsel_syntax::ItemKind::Function { .. }) {
                    SignatureKind::Function(hirsel_types::FunctionType {
                        state_promises: Default::default(),
                        generics: Default::default(),
                        parameters: Default::default(),
                        result: unit,
                        error: None,
                        allocation: hirsel_types::Allocation::MayAllocate,
                        authority: hirsel_types::AuthoritySummary::Known(Default::default()),
                        uses: hirsel_types::Uses::Unrestricted,
                        contracts: Default::default(),
                        incomplete: false,
                        foreign: None,
                    })
                } else {
                    SignatureKind::Const {
                        ty: unit,
                        generics: Default::default(),
                    }
                },
                metadata: Default::default(),
                fingerprint: syntax.text_hash,
            });
            let reads = Reads::from_vec(vec![(
                ReadKey::Signature(signature.definition),
                signature.fingerprint,
            )])
            .unwrap();
            signatures.push((signature.definition, Ok(signature)));
            let checked = CheckedBody {
                lexical: syntax.owner(),
                places: vec![],
                calls: vec![],
                tails: vec![],
                bindings: vec![],
                patterns: vec![],
                resolution,
                expressions: syntax
                    .tree
                    .expressions
                    .iter()
                    .enumerate()
                    .map(|(id, _)| Elaboration {
                        construction_destination: None,
                        expression: hirsel_syntax::ExprId(id as u32),
                        ty: unit,
                        usage: UseKind::Take,
                        instantiated_arguments: vec![],
                    })
                    .collect(),
                instantiations: vec![],
                closures: vec![],
            };
            bodies.insert(
                syntax.item.definition.clone(),
                Arc::new(TaskBody {
                    analyzed: Arc::new(output(AnalyzedBody {
                        typed: Arc::new(TypedBody {
                            syntax,
                            checked,
                            plans: Default::default(),
                            unit,
                            boolean,
                        }),
                        ownership: OwnershipFacts::default(),
                        results: vec![],
                        call_allocations: vec![],
                    })),
                    reads,
                    dependencies: vec![],
                    detail: DependencyReads::default(),
                }),
            );
        }
        let snapshot = Arc::new(SignatureSnapshot::new(
            Revision::FIRST,
            types.clone(),
            signatures,
            [],
            None,
        ));
        let mut limits = crate::ComptimeLimits::default();
        limits.call_depth = limit;
        Source {
            inputs: TaskInputs {
                snapshot,
                memo: engine.memo.clone(),
                lowering: Arc::new(UnitLower),
                runtime: Arc::new(RuntimeTypes::new(types)),
                sources: engine.sources.clone(),
                limits,
                verify: Verify::On,
                lower_budget: 1 << 20,
            },
            bodies,
            cached: None,
        }
    }

    #[test]
    fn body_representation_reads_observe_layout_source_before_lowering() {
        crate::with_compiler_stack(|| {
            let mut source = source("fn helper()->(){()}\nfn layout()->(){()}\n", 8);
            let layout = DefId::intern(&path("layout"));
            let representation = ContentHash::of_parts("representation", [b"view".as_slice()]);
            let body = source.bodies.get_mut(&path("helper")).unwrap();
            let body = Arc::get_mut(body).unwrap();
            body.reads =
                Reads::from_vec(vec![(ReadKey::Representation(layout), representation)]).unwrap();
            let expected = memos::optional(source.implementation(layout.path()));
            assert_ne!(expected, representation);
            let shared = TaskShared::new(S1Coordinator::new());
            let mut worker = TaskWorker::new(&source, &shared);
            let product = worker.lower_root(&path("helper"));
            assert!(product.result.is_ok());
            assert!(product.reads.iter().any(|(key, value)| {
                *key == ReadKey::Representation(layout) && *value == representation
            }));
            assert!(
                product.reads.iter().any(|(key, value)| {
                    *key == ReadKey::LayoutBody(layout) && *value == expected
                })
            );
        });
    }

    fn cached_source() -> Source {
        let mut source = source("fn helper()->(){()}\n", 8);
        source.inputs.lower_budget = 0;
        let shared = TaskShared::new(S1Coordinator::new());
        let mut worker = TaskWorker::new(&source, &shared);
        let product = worker.lower_root(&path("helper"));
        let (output, depth, verification) = product.result.unwrap();
        assert_eq!(
            worker.lower_work[&DefId::intern(&path("helper"))].validation_runs,
            1
        );
        assert!(verification.is_some());
        assert_eq!(worker.lowered.stats().bytes, 0);
        drop(worker);
        source.cached = Some(CachedLowering {
            output,
            depth,
            verification,
            reads: product.reads,
        });
        source
    }

    #[test]
    fn cached_lowering_reuses_only_current_local_v_evidence() {
        crate::with_compiler_stack(|| {
            let source = cached_source();
            let expected = source.cached.as_ref().unwrap();
            let shared = TaskShared::new(S1Coordinator::new());
            let mut worker = TaskWorker::new(&source, &shared);
            let product = worker.lower_root(&path("helper"));
            let (output, depth, verification) = product.result.unwrap();
            assert!(Arc::ptr_eq(&output.value, &expected.output.value));
            assert_eq!(
                worker
                    .lower_work
                    .get(&DefId::intern(&path("helper")))
                    .map_or(0, |work| work.validation_runs),
                0,
                "a current local V capability must avoid duplicate validation"
            );
            assert!(Arc::ptr_eq(
                verification.as_ref().unwrap(),
                expected.verification.as_ref().unwrap()
            ));
            assert_eq!(product.reads, expected.reads);
            assert_eq!(depth, expected.depth);
            assert_eq!(worker.lowered.stats().bytes, 0);
        });
    }

    #[test]
    fn cached_lowering_missing_or_wrong_body_evidence_runs_v() {
        crate::with_compiler_stack(|| {
            for wrong_body in [false, true] {
                let mut source = cached_source();
                let hit = source.cached.as_mut().unwrap();
                if wrong_body {
                    hit.output.value = Arc::new((*hit.output.value).clone());
                } else {
                    hit.verification = None;
                }
                let shared = TaskShared::new(S1Coordinator::new());
                let mut worker = TaskWorker::new(&source, &shared);
                let product = worker.lower_root(&path("helper"));
                assert!(product.result.unwrap().2.is_some());
                assert_eq!(
                    worker.lower_work[&DefId::intern(&path("helper"))].validation_runs,
                    1
                );
                assert_eq!(worker.lowered.stats().bytes, 0);
            }
        });
    }

    #[test]
    fn compact_root_facts_release_tir_before_the_next_worker_task() {
        crate::with_compiler_stack(|| {
            let source = source("const LEAF:()=()\nfn helper()->(){comptime LEAF}\n", 8);
            for jobs in [1, 2, 8] {
                let shared = TaskShared::new(S1Coordinator::new());
                let live = Mutex::new(Vec::<std::sync::Weak<hirsel_tir::Definition>>::new());
                let results = jobs::map_with_state(
                    Jobs::new(jobs),
                    vec![(); 64],
                    |_| 1,
                    || TaskWorker::new(&source, &shared),
                    |worker, ()| {
                        worker.lower_root_with(&path("helper"), |output| {
                            let mut live = live.lock().unwrap();
                            live.push(Arc::downgrade(&output.value));
                            assert!(
                                live.iter().filter(|weak| weak.strong_count() > 0).count() <= jobs
                            );
                            discard_lowered_output(output)
                        })
                    },
                );
                assert!(results.iter().all(|result| result.result.is_ok()));
                assert!(
                    live.lock()
                        .unwrap()
                        .iter()
                        .all(|weak| weak.strong_count() == 0)
                );
                for result in &results[1..] {
                    assert_eq!(result.reads, results[0].reads);
                    assert_eq!(
                        result.result.as_ref().unwrap().1,
                        results[0].result.as_ref().unwrap().1
                    );
                }
            }
        });
    }

    #[test]
    fn zero_and_small_lower_budgets_release_helper_cache_ownership() {
        crate::with_compiler_stack(|| {
            for budget in [0, 1] {
                let mut source = source("const LEAF:()=()\nfn helper()->(){comptime LEAF}\n", 8);
                source.inputs.lower_budget = budget;
                let shared = TaskShared::new(S1Coordinator::new());
                let mut worker = TaskWorker::new(&source, &shared);
                let first = worker.capture(|worker| worker.lower(&path("helper")));
                let definition = first.result.unwrap();
                let weak = Arc::downgrade(&definition);
                assert_eq!(
                    Arc::strong_count(&definition),
                    1,
                    "budget={budget}: helper exceeds allowance"
                );
                drop(definition);
                assert_eq!(weak.strong_count(), 0, "cache retains evicted helper");
                assert_eq!(worker.lowered.stats().bytes, 0);
                assert_eq!(worker.lowered.stats().evictions, 1);
                let again = worker.capture(|worker| worker.lower(&path("helper")));
                assert!(again.result.is_ok());
                assert_eq!(worker.lowered.stats().recomputes, 1);
                assert_eq!(worker.lowered.stats().evictions, 2);
                assert_eq!(worker.lowered.stats().bytes, 0);
                assert!(worker.lowered.stats().peak_bytes > budget);
                assert_eq!(
                    first.reads, again.reads,
                    "recompute must preserve complete receipts"
                );
            }
        });
    }

    #[test]
    fn budgeted_helper_recompute_keeps_depth_reads_and_worker_totals() {
        crate::with_compiler_stack(|| {
            let mut source = source("const LEAF:()=()\nfn helper()->(){comptime LEAF}\n", 8);
            let control_shared = TaskShared::new(S1Coordinator::new());
            let mut control = TaskWorker::new(&source, &control_shared);
            control.stack.begin_requests();
            let expected = control.capture(|worker| worker.lower(&path("helper")));
            assert!(expected.result.is_ok());
            let expected_depth = control.stack.finish_requests(8);
            drop(control);
            for jobs in [1, 2, 8] {
                for budget in [0, 1, 4096, 1 << 20] {
                    source.inputs.lower_budget = budget / jobs;
                    let shared = TaskShared::new(S1Coordinator::new());
                    let results = jobs::map_with_state(
                        Jobs::new(jobs as usize),
                        vec![(); 16],
                        |_| 1,
                        || TaskWorker::new(&source, &shared),
                        |worker, ()| {
                            for _ in 0..2 {
                                worker.stack.begin_requests();
                                let product =
                                    worker.capture(|worker| worker.lower(&path("helper")));
                                assert!(product.result.is_ok());
                                assert_eq!(product.reads, expected.reads);
                                assert_eq!(worker.stack.finish_requests(8), expected_depth);
                                assert!(worker.lowered.stats().bytes <= source.inputs.lower_budget);
                            }
                            worker.lowered.clear();
                            worker.lowered.stats().bytes
                        },
                    );
                    assert!(results.iter().all(|&bytes| bytes == 0));
                    let stats = *shared.lower_holds.lock().unwrap();
                    assert_eq!(stats.bytes, 0, "all completed roots released");
                    assert!(stats.held > 0);
                    assert!(stats.peak_bytes > 0);
                    if budget <= 1 {
                        assert_eq!(stats.evictions, 32);
                        assert_eq!(stats.recomputes, 16);
                    }
                }
            }
        });
    }

    #[test]
    fn finite_lower_budget_evicts_real_helpers_and_recomputes_with_identical_reads() {
        crate::with_compiler_stack(|| {
            let mut source = source(
                "const LEAF:()=()\nfn helper1()->(){comptime LEAF}\nfn helper2()->(){comptime LEAF}\n",
                8,
            );
            let probe_shared = TaskShared::new(S1Coordinator::new());
            let mut probe = TaskWorker::new(&source, &probe_shared);
            drop(probe.lower(&path("helper1")).unwrap());
            let first_bytes = probe.lowered.stats().bytes;
            drop(probe.lower(&path("helper2")).unwrap());
            let second_bytes = probe.lowered.stats().bytes - first_bytes;
            drop(probe);
            source.inputs.lower_budget = first_bytes.max(second_bytes);
            let shared = TaskShared::new(S1Coordinator::new());
            let mut worker = TaskWorker::new(&source, &shared);
            let first = worker.capture(|worker| worker.lower(&path("helper1")));
            let definition = first.result.unwrap();
            let first_weak = Arc::downgrade(&definition);
            drop(definition);
            assert!(first_weak.strong_count() > 0);
            let second = worker.lower(&path("helper2")).unwrap();
            let second_weak = Arc::downgrade(&second);
            drop(second);
            assert_eq!(
                first_weak.strong_count(),
                0,
                "least-recent helper was evicted"
            );
            assert!(
                second_weak.strong_count() > 0,
                "one helper fits the finite budget"
            );
            let again = worker.capture(|worker| worker.lower(&path("helper1")));
            assert!(again.result.is_ok());
            assert_eq!(first.reads, again.reads);
            assert_eq!(second_weak.strong_count(), 0);
            let stats = worker.lowered.stats();
            assert_eq!(stats.evictions, 2);
            assert_eq!(stats.recomputes, 1);
            assert!(stats.bytes <= source.inputs.lower_budget);
        });
    }

    #[test]
    fn completed_roots_release_worker_helper_lowerings() {
        crate::with_compiler_stack(|| {
            let source = source("fn helper()->(){()}\nconst VALUE:()=comptime helper()\n", 8);
            let shared = TaskShared::new(S1Coordinator::new());
            let mut worker = TaskWorker::new(&source, &shared);
            let helper = worker.lower(&path("helper")).unwrap();
            let held = Arc::downgrade(&helper);
            drop(helper);
            assert!(
                held.strong_count() > 0,
                "helper is memoized before the root"
            );
            let first = worker.root(&path("VALUE"));
            assert!(first.result.is_ok());
            assert_eq!(
                held.strong_count(),
                0,
                "completed root retains its helper TIR"
            );
            let again = worker.root(&path("VALUE"));
            assert!(again.result.is_ok());
            assert_eq!(
                first.reads, again.reads,
                "root success replay preserves complete reads"
            );
        });
    }

    #[test]
    fn real_constant_workers_share_successes_with_serial_depth_and_reads() {
        crate::with_compiler_stack(|| {
            let text = "const LEAF: () = ()\nconst B: () = comptime LEAF\nconst C: () = comptime B\nconst D: () = comptime C\n";
            let source = source(text, 3);
            for jobs in [1, 2, 8] {
                for warm in [false, true] {
                    let coordinator = S1Coordinator::new();
                    let shared = TaskShared::new(coordinator.clone());
                    if warm {
                        TaskWorker::new(&source, &shared)
                            .constant_at(&path("B"), source.site(&path("B")).unwrap())
                            .unwrap();
                    }
                    let results = jobs::map(
                        Jobs::new(jobs),
                        vec!["C", "D", "C", "D"],
                        |_| 1,
                        |name| {
                            let mut worker = TaskWorker::new(&source, &shared);
                            let result =
                                worker.constant_at(&path(name), source.site(&path(name)).unwrap());
                            (name, result, worker.limit.take())
                        },
                    );
                    for (name, result, limit) in results {
                        if name == "C" {
                            let value = result.unwrap();
                            assert_eq!(
                                value.value.root.kind,
                                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit)
                            );
                            for name in ["LEAF", "B", "C"] {
                                assert!(
                                    value
                                        .executed
                                        .get(&ReadKey::ExecutedBody(DefId::intern(&path(name))))
                                        .is_some(),
                                    "jobs{jobs} warm{warm}: missing {name}"
                                );
                            }
                            assert!(limit.is_none());
                        } else {
                            assert!(matches!(result, Err(QueryError::Operational(_))));
                            let limit = limit.unwrap();
                            assert_eq!(limit.limit, 3);
                            let site = source.inputs.sources.resolve(limit.site.unwrap());
                            assert_eq!(site.start, text.find("comptime LEAF").unwrap() as u32 + 9);
                        }
                    }
                    assert!(coordinator.stats().claims > 0);
                }
            }
        });
    }
}
