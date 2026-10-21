//! S1's depth-first task worker. Shared inputs and successful constant products
//! are immutable; only the short success-memo operations hold a lock. Query
//! execution never borrows an engine or holds a shared query lock.
use super::*;
use crate::comptime::{
    ComptimeValueSource, EvaluatedConstant, EvaluationInput, StagedDeclarationInput,
    StagedRegistration, prepare_staged_types, prepare_values,
};
use crate::memo::{DepthEvidence, Entered, S1Coordinator, S1Worker};
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
    Staged(DefPath),
    Receipt(ReadKey),
    Generate(DefPath),
    ConstantInstance(DefPath, Vec<u64>),
    Instance(DefPath, Vec<GenericArgument>),
}

pub(crate) struct TaskInputs {
    pub(crate) snapshot: Arc<SignatureSnapshot>,
    pub(crate) memo: Arc<hirsel_types::TypeMemo>,
    pub(crate) lowering: Arc<dyn LoweringPhase>,
    pub(crate) runtime: Arc<RuntimeTypes>,
    pub(crate) sources: SourceMap,
    pub(crate) limits: hirsel_interp::Limits,
    pub(crate) verify: Verify,
}

pub(crate) struct TaskBody {
    pub(crate) analyzed: Arc<QueryOutput<AnalyzedBody>>,
    pub(crate) reads: Reads,
    pub(crate) dependencies: Vec<SignatureDependency>,
}

/// Immutable source inputs, with task-local requests supplied explicitly.
/// A source may compute a demanded body through immutable phase handles; it
/// must return its complete read record on failure as well as on success.
pub(crate) trait TirSource: Sync {
    fn inputs(&self) -> &TaskInputs;
    fn implementation(&self, path: &DefPath) -> Option<ContentHash>;
    fn site(&self, path: &DefPath) -> Result<Span, QueryError>;
    fn body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
    ) -> DependencyProduct<Arc<TaskBody>>;
    fn dictionary_constant(&self, path: &DefPath) -> Result<Option<Materialized>, QueryError>;
    fn supports_staged_declarations(&self) -> bool {
        true
    }
    fn cached_constant(&self, _: &DefPath) -> Option<EvaluatedConstant> {
        None
    }
    fn cached_lowering(&self, _: &DefPath) -> Option<(DefinitionRef, Reads, DepthEvidence<Span>)> {
        None
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

pub(crate) struct TaskShared {
    coordinator: Arc<S1Coordinator<DefPath>>,
    constants: Mutex<BTreeMap<DefPath, ConstantProduct>>,
}
impl TaskShared {
    pub(crate) fn new(coordinator: Arc<S1Coordinator<DefPath>>) -> Self {
        Self {
            coordinator,
            constants: Mutex::new(BTreeMap::new()),
        }
    }
    pub(super) fn into_constants(
        self,
    ) -> Vec<(DefPath, EvaluatedConstant, Vec<StagedRegistration>)> {
        self.constants
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .into_iter()
            .map(|(path, product)| (path, product.constant, product.registrations))
            .collect()
    }
}

struct Lowered {
    definition: DefinitionRef,
    reads: Reads,
    depth: DepthEvidence<Span>,
    registrations: Vec<StagedRegistration>,
}

pub(crate) struct TaskWorker<'a, S: TirSource + ?Sized> {
    source: &'a S,
    shared: &'a TaskShared,
    stack: S1Worker<DefPath, Span>,
    /// Clone once per worker; nested evaluation reuses this recorder stack.
    runtime: Arc<RuntimeTypes>,
    reads: Vec<ReadsBuilder>,
    lowered: BTreeMap<DefPath, Arc<Lowered>>,
    pub(crate) registrations: Vec<StagedRegistration>,
    pub(crate) limit: Option<hirsel_tir::ToolingLimit>,
    pub(super) pending: Vec<Requirement>,
}
impl<'a, S: TirSource + ?Sized> TaskWorker<'a, S> {
    pub(crate) fn new(source: &'a S, shared: &'a TaskShared) -> Self {
        Self {
            source,
            shared,
            stack: shared.coordinator.worker(),
            runtime: Arc::new((*source.inputs().runtime).clone()),
            reads: vec![],
            lowered: BTreeMap::new(),
            registrations: vec![],
            limit: None,
            pending: vec![],
        }
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
        self.capture(|worker| {
            let site = worker.source.site(path)?;
            worker
                .constant_at(path, site)
                .map(|constant| constant.value)
        })
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
    fn implementation(&mut self, path: &DefPath) {
        if let Some(hash) = self.source.implementation(path) {
            self.record(
                ReadKey::ExecutedBody(DefId::intern(path)),
                memos::optional(Some(hash)),
            );
        }
    }
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        let inputs = self.source.inputs();
        let mut reads = ReadsBuilder::new();
        let result = FrozenProvider::new(&inputs.snapshot, path.0.package.clone(), &mut reads)
            .signature(DefId::intern(path));
        self.merge(&reads.finish().map_err(|_| QueryError::NotImplemented)?);
        result
    }
    fn body(&mut self, path: &DefPath) -> Result<Arc<TaskBody>, QueryError> {
        let source = self.source;
        let product = source.body(self, path);
        let body = self.merge_product(product)?;
        self.merge(&body.reads);
        for dependency in &body.dependencies {
            let id = DefId::intern(&dependency.definition);
            if let Some(signature) = self.source.inputs().snapshot.signature(id) {
                let fingerprint = signature.ok().map(|signature| signature.fingerprint);
                self.record(
                    ReadKey::ExecutedSignature(id),
                    memos::signature_value(fingerprint),
                );
            }
        }
        Ok(body)
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
            self.admit(&value.depth, site)?;
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
                self.merge(&cached.constant.executed);
                self.registrations.extend(cached.registrations);
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
        let registration_start = self.registrations.len();
        let pending_start = self.pending.len();
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
            let constants = prepare_values(&types, &body.analyzed.value, &[], &signature, worker)?;
            let value = if let Some((_, value)) = constants.iter().find(|(id, _)| *id == value) {
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
            Ok((value, constants))
        });
        let depth = self.stack.composed(self.depth_limit(), true);
        let result = product
            .result
            .map(|(value, substitutions)| EvaluatedConstant {
                value,
                substitutions,
                executed: product.reads,
                depth,
            });
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
            if !self.source.supports_staged_declarations() {
                self.pending.push(Requirement::Staged(owner.clone()));
                return Err(QueryError::NotImplemented);
            }
            let signature = self.signature(owner)?;
            self.registrations.push(
                StagedDeclarationInput {
                    analyzed,
                    expression: row.expression,
                    ty: row.ty,
                    inputs,
                }
                .prepare(&types, &memo, &signature)?,
            );
        }
        Ok(())
    }
    fn validate_lowered(
        &mut self,
        path: &DefPath,
        definition: &hirsel_tir::Definition,
    ) -> Result<(), QueryError> {
        let inputs = self.source.inputs();
        if inputs.verify == Verify::Off {
            return Ok(());
        }
        let mut reads = ReadsBuilder::new();
        FrozenProvider::new(&inputs.snapshot, path.0.package.clone(), &mut reads)
            .validation_universe();
        self.merge(&reads.finish().expect("one immutable validation universe"));
        let _time = timings::phase(Phase::Verify);
        timings::count(Count::VRuns);
        let mut observed = vec![];
        let report = hirsel_validate::validate(
            definition,
            inputs.snapshot.types(),
            inputs.snapshot.validation(&path.0.package),
            &mut observed,
        );
        if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
            return Err(error);
        }
        if report.violations.is_empty() {
            Ok(())
        } else {
            Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "TIR validation".into(),
                message: format!("{:?}", report.violations),
            }))
        }
    }
    pub(super) fn lower(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        if let Some((definition, reads, depth)) = self.source.cached_lowering(path) {
            self.merge(&reads);
            self.validate_lowered(path, &definition)?;
            self.stack
                .replay_requests(&depth)
                .map_err(|site| self.overflow(site))?;
            return Ok(definition);
        }
        if let Some(hit) = self.lowered.get(path).cloned() {
            self.stack
                .replay_requests(&hit.depth)
                .map_err(|site| self.overflow(site))?;
            self.merge(&hit.reads);
            self.registrations.extend(hit.registrations.iter().cloned());
            return Ok(hit.definition.clone());
        }
        self.stack.begin_requests();
        let registration_start = self.registrations.len();
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
            let definition = inputs
                .lowering
                .lower(
                    &types,
                    &inputs.memo,
                    &body.analyzed.value,
                    &Comptime {
                        materialized,
                        staged,
                    },
                    &signature,
                )?
                .value;
            worker.validate_lowered(path, &definition)?;
            Ok(Arc::new(definition))
        });
        let depth = self.stack.finish_requests(self.depth_limit());
        self.merge(&product.reads);
        let definition = product.result?;
        self.lowered.insert(
            path.clone(),
            Arc::new(Lowered {
                definition: definition.clone(),
                reads: product.reads,
                depth,
                registrations: self.registrations[registration_start..].to_vec(),
            }),
        );
        Ok(definition)
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
        let inputs = self.source.inputs();
        let package = input.signature.definition.path().0.package.clone();
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
        if product.limit.is_some() {
            self.limit = product.limit;
        }
        let (value, ty) = product.value.result?;
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
    }
    impl TirSource for Source {
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
        let mut limits = hirsel_interp::Limits::default();
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
            },
            bodies,
        }
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
                            (name, result, worker.limit)
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
    #[test]
    fn worker_helper_lowering_hits_keep_the_same_overflow_site() {
        crate::with_compiler_stack(|| {
            let text = "const LEAF: () = ()\nfn helper()->(){comptime LEAF}\nconst C2: () = comptime helper()\nconst C1: () = comptime C2\nconst C0: () = comptime C1\n";
            let source = source(text, 3);
            for jobs in [1, 2, 8] {
                for warm in [false, true] {
                    let shared = TaskShared::new(S1Coordinator::new());
                    let results = jobs::map_with_state(
                        Jobs::new(jobs),
                        vec![(); 8],
                        |_| 1,
                        || {
                            let mut worker = TaskWorker::new(&source, &shared);
                            if warm {
                                worker.lower(&path("helper")).unwrap();
                            }
                            worker
                        },
                        |worker, ()| {
                            let result =
                                worker.constant_at(&path("C0"), source.site(&path("C0")).unwrap());
                            assert!(
                                matches!(result, Err(QueryError::Operational(_))),
                                "jobs{jobs} warm{warm}: {result:?}"
                            );
                            let limit = worker.limit.take().unwrap();
                            source.inputs.sources.resolve(limit.site.unwrap()).start
                        },
                    );
                    assert!(
                        results
                            .iter()
                            .all(|&site| site == text.find("comptime LEAF").unwrap() as u32 + 9),
                        "jobs{jobs} warm{warm}: {results:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn real_constant_worker_cycles_are_identical_with_partial_hits() {
        crate::with_compiler_stack(|| {
            let source = source(
                "const LEAF: () = ()\nconst A: () = comptime B\nconst B: () = comptime A\n",
                8,
            );
            let mut expected = None;
            for jobs in [1, 2, 8] {
                let shared = TaskShared::new(S1Coordinator::new());
                TaskWorker::new(&source, &shared)
                    .constant_at(&path("LEAF"), source.site(&path("LEAF")).unwrap())
                    .unwrap();
                let results = jobs::map(
                    Jobs::new(jobs),
                    vec!["A", "B", "A", "B"],
                    |_| 1,
                    |name| {
                        let mut worker = TaskWorker::new(&source, &shared);
                        let result =
                            worker.constant_at(&path(name), source.site(&path(name)).unwrap());
                        let Err(QueryError::Diagnostics(diagnostics)) = result else {
                            panic!("expected CT4");
                        };
                        diagnostics
                            .into_iter()
                            .map(|d| (d.rule, source.inputs.sources.resolve(d.primary)))
                            .collect::<Vec<_>>()
                    },
                );
                for diagnostics in results {
                    assert_eq!(diagnostics.len(), 1);
                    if let Some(expected) = &expected {
                        assert_eq!(&diagnostics, expected);
                    } else {
                        expected = Some(diagnostics);
                    }
                }
                let memo = shared.constants.lock().unwrap();
                assert!(memo.contains_key(&path("LEAF")));
                assert!(!memo.contains_key(&path("A")) && !memo.contains_key(&path("B")));
            }
        });
    }
}
