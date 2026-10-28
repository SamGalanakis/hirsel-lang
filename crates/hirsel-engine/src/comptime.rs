//! Compile-time execution uses the runtime interpreter; lowering receives plain values.
use crate::memo::{DepthEvidence, Entered};
use crate::*;
use hirsel_intern::timings::{self, Phase};
use hirsel_interp::Outcome;
use hirsel_span::Span;
use hirsel_syntax::{DefId, ExprId, ExprKind, ItemKind};
use hirsel_tir::{Constant, DefinitionRef, TirProvider};
use hirsel_tir::{Materialized, ValueKind};
use hirsel_types::{
    GenericArgument, GenericBinder, NameSite, ResolvedName, SignatureKind, Size, SizeParameterRef,
    TypeId,
};
use std::sync::Arc;

/// Exact demand order across staged values and generated source selections.
/// This is replay metadata, never verification authority.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) enum Selection {
    Staged(DefId, Vec<u64>),
    Generated(DefId),
}

/// A constant evaluation and the substitutions used to lower its source initializer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct EvaluatedConstant {
    pub value: Materialized,
    pub substitutions: Vec<(ExprId, Materialized)>,
    /// The complete reads of everything the evaluation executed or materialized.
    pub executed: crate::memo::Reads,
    /// Its dependency height and overflow table under the comptime limits.
    pub depth: DepthEvidence<Span>,
    /// Exact ordered staged selections, including transitive value hits.
    pub selections: Vec<crate::comptime::Selection>,
    /// Memory-only acceptance contexts, recorded while private/source V ran.
    /// Producer serialization cannot carry this authority. Workers instead
    /// retain their existing explicit ValidationUniverse witnesses.
    #[serde(skip)]
    pub validation: Option<Arc<[(hirsel_syntax::PackageId, hirsel_syntax::ContentHash)]>>,
    #[serde(skip)]
    pub acceptance: Option<hirsel_validate::ExecutionReceipt>,
}
/// The default source budget of one compile-time evaluation, in charged units (CT-4).
pub const DEFAULT_COMPTIME_BUDGET: u64 = 1_000_000;
/// Compile-time evaluation limits: each evaluation's source budget (CT-4) and
/// the call-depth tooling safeguard. Both are explicit query inputs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ComptimeLimits {
    /// Charged source units each evaluation may spend.
    pub budget: u64,
    pub call_depth: usize,
}
impl Default for ComptimeLimits {
    fn default() -> Self {
        Self {
            budget: DEFAULT_COMPTIME_BUDGET,
            call_depth: usize::MAX,
        }
    }
}
impl ComptimeLimits {
    /// The interpreter limits of one evaluation: its source budget, and no
    /// execution-step bound (PROG-3 steps are not compile-time accounting).
    pub(crate) fn interpreter(self) -> hirsel_interp::Limits {
        hirsel_interp::Limits {
            steps: None,
            call_depth: self.call_depth,
            source: Some(self.budget),
        }
    }
}
fn operational(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "compile-time evaluation".into(),
        message: message.into(),
    })
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct StagedConstant {
    pub owner: DefPath,
    pub expression: ExprId,
    pub inputs: Vec<SizeParameterRef>,
    pub signature: hirsel_types::SignatureRef,
}
/// One size-generic constant declaration's immutable construction input.
/// Workers return its registration; the owning engine publishes registrations
/// only after the task's dependencies settle.
pub(crate) struct StagedDeclarationInput<'a> {
    pub(crate) analyzed: &'a AnalyzedBody,
    pub(crate) expression: ExprId,
    pub(crate) ty: TypeId,
    pub(crate) inputs: Vec<SizeParameterRef>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct StagedRegistration {
    pub(crate) path: DefPath,
    pub(crate) constant: StagedConstant,
}

impl StagedDeclarationInput<'_> {
    pub(crate) fn prepare(
        self,
        types: &hirsel_types::TypeInterner,
        memo: &hirsel_types::TypeMemo,
        owner_signature: &Signature,
    ) -> Result<StagedRegistration, QueryError> {
        let owner = self.analyzed.typed.syntax.item.definition.clone();
        let path = hirsel_check_protocol::body::comptime_path(
            &owner,
            &self.analyzed.typed.checked,
            self.expression,
        );
        let mut signature = owner_signature.clone();
        signature.definition = DefId::intern(&path);
        signature.metadata = hirsel_types::SignatureMetadata {
            roles: signature.metadata.roles.clone(),
            unsafe_ffi: signature.metadata.unsafe_ffi,
            ..Default::default()
        };
        let replacements: Vec<_> = self
            .inputs
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    *p,
                    Size::Parameter(SizeParameterRef::bound(hirsel_types::SizeParamId(i as u32))),
                )
            })
            .collect();
        let ty = substitute_sizes_type(types, memo, self.ty, &replacements)?;
        signature.kind = SignatureKind::Const {
            ty,
            generics: self
                .inputs
                .iter()
                .enumerate()
                .map(|(i, _)| hirsel_types::GenericParameter::Size {
                    name: format!("size{i}"),
                })
                .collect::<Vec<_>>()
                .into(),
        };
        signature.fingerprint = hirsel_types::encoding::fingerprint(types, &signature, &[])
            .map_err(|e| operational(format!("staged signature: {e:?}")))?;
        Ok(StagedRegistration {
            path,
            constant: StagedConstant {
                owner,
                expression: self.expression,
                inputs: self.inputs,
                signature: Arc::new(signature),
            },
        })
    }
}

pub(crate) fn has_staged_reads(definition: &hirsel_tir::Definition) -> bool {
    definition.bodies.iter().flat_map(|body| &body.rvalues).any(|value| {
        matches!(value, hirsel_tir::Rvalue::ReadConstant { arguments, .. } if !arguments.is_empty())
    })
}
/// The definition-local inputs to lowering an evaluation. Its phase handle,
/// interner and memo may be shared, while the lowered artifact is task-owned.
/// It reads no QueryEngine state and publishes no registration.
pub(crate) struct EvaluationInput<'a> {
    pub(crate) analyzed: &'a AnalyzedBody,
    pub(crate) staged: &'a [(ExprId, TypeId)],
    pub(crate) signature: &'a Signature,
    pub(crate) expression: ExprId,
    pub(crate) child: ExprId,
    pub(crate) ty: TypeId,
    pub(crate) constants: &'a [(ExprId, Materialized)],
    pub(crate) sizes: &'a [(SizeParameterRef, u64)],
}

pub(crate) struct PreparedEvaluation {
    pub(crate) definition: hirsel_tir::Definition,
    pub(crate) signature: Signature,
    pub(crate) ty: TypeId,
    pub(crate) origin: Option<(hirsel_span::Anchor, DefPath, u32)>,
}

impl EvaluationInput<'_> {
    pub(crate) fn lower(
        self,
        types: &hirsel_types::TypeInterner,
        memo: &hirsel_types::TypeMemo,
        lowering: &dyn LoweringPhase,
    ) -> Result<PreparedEvaluation, QueryError> {
        let mut definition = lowering.lower_evaluation(
            types,
            memo,
            self.analyzed,
            self.staged,
            self.signature,
            self.child,
            self.ty,
            self.constants,
        )?;
        // This private initializer is a zero-input artifact, not the enclosing
        // definition's exported body. Its result is H/J's checked expression type.
        definition.path = DefId::intern(&hirsel_check_protocol::body::comptime_path(
            self.signature.definition.path(),
            &self.analyzed.typed.checked,
            self.expression,
        ));
        specialize_sizes(types, memo, &mut definition, self.sizes)?;
        let ty = definition.bodies[definition.entry.0 as usize].result;
        let mut artifact = self.signature.clone();
        artifact.definition = definition.path;
        artifact.metadata = hirsel_types::SignatureMetadata {
            roles: self.signature.metadata.roles.clone(),
            unsafe_ffi: self.signature.metadata.unsafe_ffi,
            ..Default::default()
        };
        artifact.kind = hirsel_types::SignatureKind::Const {
            ty,
            generics: Default::default(),
        };

        Ok(PreparedEvaluation {
            definition,
            signature: artifact,
            ty,
            origin: Some((
                self.analyzed.typed.syntax.anchor,
                self.analyzed.typed.syntax.item.definition.clone(),
                self.analyzed.typed.syntax.occurrence,
            )),
        })
    }
}

/// V over an evaluation's private artifact and the frozen signature universe.
/// Both the serial caller and S1 workers use this immutable entry point.
pub(crate) fn validate_evaluation(
    snapshot: &SignatureSnapshot,
    definition: &hirsel_tir::Definition,
    artifact: Signature,
    verify: Verify,
    origin: Option<&(hirsel_span::Anchor, DefPath, u32)>,
) -> Result<Option<hirsel_validate::AcceptedArtifact>, QueryError> {
    let (report, acceptance) = match verify {
        Verify::On => {
            let _timing = timings::phase(Phase::Verify);
            snapshot.validate_private_artifact(definition, Arc::new(artifact), origin)
        }
        Verify::Off => (hirsel_validate::Validation::default(), None),
    };
    if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
        return Err(error);
    }
    if report.violations.is_empty() {
        Ok(acceptance)
    } else {
        Err(QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "compile-time TIR validation".into(),
            message: format!("{:?}", report.violations),
        }))
    }
}

/// One checked, validated compile-time artifact's interpreter task. All inputs
/// are immutable; `provider` owns its dependency requests and read recording.
/// `runtime` belongs to this worker: nested tasks reuse its recording stack,
/// while different workers clone RuntimeTypes (sharing only immutable views).
/// This is the same task at jobs=1 and on a worker; it never borrows an engine.
/// Like rustc's body-owner tasks (rustc_hir_analysis/src/lib.rs:166-220),
/// execution returns products for publication by the caller.
pub(crate) struct EvaluationTask<'a> {
    pub(crate) sources: &'a hirsel_span::SourceMap,
    pub(crate) configuration: RunConfiguration,
    pub(crate) limits: crate::ComptimeLimits,
    pub(crate) runtime: &'a hirsel_interp::aggregate::RuntimeTypes,
    pub(crate) site: Span,
    pub(crate) ty: TypeId,
}

/// Execution evidence survives failure as well as success. The caller merges
/// `reflected` with all reads made by the dependency provider before deciding
/// whether to cache the result; a context-dependent failure stays local.
pub(crate) struct EvaluationProduct {
    pub(crate) result: Result<(hirsel_interp::Value, TypeId), QueryError>,
    pub(crate) reflected: std::collections::BTreeMap<DefPath, ContentHash>,
    pub(crate) limit: Option<hirsel_tir::ToolingLimit>,
}

/// Close task-local reflection recording even when an admitted continuation
/// is cancelled and dropped while a dependency is pending.
struct EvaluationRecording<'a> {
    serialization: &'a hirsel_interp::aggregate::Serialization,
    active: bool,
}
impl<'a> EvaluationRecording<'a> {
    fn new(serialization: &'a hirsel_interp::aggregate::Serialization) -> Self {
        serialization.record();
        Self {
            serialization,
            active: true,
        }
    }
    fn finish(mut self) -> std::collections::BTreeMap<DefPath, ContentHash> {
        self.active = false;
        self.serialization.finish()
    }
}
impl Drop for EvaluationRecording<'_> {
    fn drop(&mut self) {
        if self.active {
            self.serialization.finish();
        }
    }
}

impl EvaluationTask<'_> {
    pub(crate) fn run(self, provider: &mut dyn TirProvider) -> EvaluationProduct {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let mut provider = hirsel_interp::ImmediateTirProvider::new(provider);
        let mut evaluation = std::pin::pin!(self.run_async(&mut provider));
        match evaluation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(product) => product,
            Poll::Pending => unreachable!("synchronous compile-time source suspended"),
        }
    }
    /// This future owns the real saved interpreter continuation. A producer
    /// supplies QueryReads-backed dependencies and polls/drops it only at home.
    pub(crate) async fn run_async(
        self,
        provider: &mut (impl hirsel_interp::AsyncTirProvider + ?Sized),
    ) -> EvaluationProduct {
        let mut library =
            match hirsel_interp::library::Library::new(&self.configuration, Default::default()) {
                Ok(library) => library,
                Err(error) => {
                    return EvaluationProduct {
                        result: Err(operational(format!("{error:?}"))),
                        reflected: Default::default(),
                        limit: None,
                    };
                }
            };
        let recording = EvaluationRecording::new(&self.runtime.serialization);
        let execution = hirsel_interp::run_with_runtime_async(
            provider,
            self.sources,
            &self.configuration,
            vec![],
            self.limits.interpreter(),
            &mut library,
            self.runtime,
        )
        .await;
        let reflected = recording.finish();
        let mut limit_reached = None;
        let result = (|| {
            let execution = execution.map_err(|e| match e.error {
                hirsel_interp::Error::Query(e) => e,
                other => operational(format!("{other:?}")),
            })?;
            match execution.outcome {
                Outcome::Returned { value, .. } => Ok((value, self.ty)),
                Outcome::Failed(failure) => {
                    let mut diagnostic = Diagnostic::error(
                        const { hirsel_diag::RuleId::of("CT-1") },
                        self.site,
                        "compile-time evaluation failed",
                    );
                    diagnostic.secondary.push(hirsel_diag::Label {
                        span: failure.site,
                        message: format!("{}: {}", failure.rule, failure.kind),
                    });
                    diagnostic.notes.push(format!(
                        "underlying failure {}: {}",
                        failure.rule, failure.kind
                    ));
                    Err(QueryError::Diagnostics(vec![diagnostic]))
                }
                // CT-4: the source event past the budget was denied before it
                // executed; CT-1 reports it at the enclosing evaluation.
                Outcome::Exhausted(exhaustion) => {
                    let mut diagnostic = Diagnostic::error(
                        const { hirsel_diag::RuleId::of("CT-1") },
                        self.site,
                        "compile-time evaluation exhausted its budget",
                    );
                    diagnostic.secondary.push(hirsel_diag::Label {
                        span: exhaustion.site,
                        message: match exhaustion.event.kind {
                            hirsel_tir::SourceEventKind::Call => "this call exceeds the budget",
                            hirsel_tir::SourceEventKind::Iteration => {
                                "this loop iteration exceeds the budget"
                            }
                        }
                        .into(),
                    });
                    diagnostic.notes.push(format!(
                        "compile-time budget: {} units, one per source call and loop iteration",
                        exhaustion.budget
                    ));
                    Err(QueryError::Diagnostics(vec![diagnostic]))
                }
                Outcome::Blocked(_) => Err(operational("compile-time evaluation is Blocked")),
                Outcome::Signaled(kind) => Err(operational(format!(
                    "compile-time evaluation ended by {}",
                    kind.name()
                ))),
                Outcome::BudgetExceeded(kind) => Err(operational(format!(
                    "compile-time evaluation exceeded its {} budget",
                    kind.name()
                ))),
                Outcome::ToolingLimit(limit) => {
                    let error = operational_limit(&limit);
                    limit_reached = Some(limit);
                    Err(error)
                }
                Outcome::UndefinedBehavior(detected) => Err(operational(detected.to_string())),
                Outcome::Trapped(trap) => Err(operational(trap.to_string())),
                Outcome::Environmental(cause) => {
                    Err(QueryError::Operational(hirsel_types::OperationalFailure {
                        operation: "codec-budget".into(),
                        message: cause.to_string(),
                    }))
                }
            }
        })();
        EvaluationProduct {
            result,
            reflected,
            limit: limit_reached,
        }
    }
}

/// Dependency requests made while preparing one body's compile-time values.
/// Implementations own their request stack, reads and local registrations;
/// this algorithm never accesses a QueryEngine or publishes a shared memo.
pub(crate) trait ComptimeValueSource {
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError>;
    fn constant(&mut self, path: &DefPath, site: Span) -> Result<Materialized, QueryError>;
    fn evaluate(&mut self, input: EvaluationInput<'_>) -> Result<Materialized, QueryError>;
}

struct EngineComptimeValues<'a>(&'a mut QueryEngine);
impl ComptimeValueSource for EngineComptimeValues<'_> {
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        self.0.item_signature(path).map(|output| output.value)
    }
    fn constant(&mut self, path: &DefPath, site: Span) -> Result<Materialized, QueryError> {
        self.0.constant_at(path, site).map(|output| output.value)
    }
    fn evaluate(&mut self, input: EvaluationInput<'_>) -> Result<Materialized, QueryError> {
        self.0.evaluate_sized_value(
            input.analyzed,
            input.staged,
            input.signature,
            input.expression,
            input.child,
            input.ty,
            input.constants,
            input.sizes,
        )
    }
}

/// Prepare in deterministic request order: constant references first, then
/// innermost compile-time expressions. The caller installs this body's staged
/// declarations in its local source before entering. Serial and frozen task
/// sources use the same repeat validation and dependency-selection algorithm.
pub(crate) fn prepare_values(
    types: &hirsel_types::TypeInterner,
    analyzed: &AnalyzedBody,
    staged: &[(ExprId, TypeId)],
    signature: &Signature,
    source: &mut impl ComptimeValueSource,
) -> Result<Vec<(ExprId, Materialized)>, QueryError> {
    let mut preparation = ValuePreparation::new(analyzed);
    match preparation.poll(types, analyzed, staged, signature, &mut ReadyValues(source)) {
        std::task::Poll::Ready(Ok(_)) => Ok(preparation.constants),
        std::task::Poll::Ready(Err(error)) => Err(error),
        std::task::Poll::Pending => unreachable!("synchronous value source suspended"),
    }
}

/// A dependency source that can release the compiler worker while a constant or
/// private evaluation is pending. Implementations retain their own evaluation
/// continuation; polling again must never restart an initializer.
pub(crate) trait PreparingSource {
    fn signature(
        &mut self,
        path: &DefPath,
    ) -> std::task::Poll<Result<hirsel_types::SignatureRef, QueryError>>;
    fn constant(
        &mut self,
        path: &DefPath,
        site: Span,
    ) -> std::task::Poll<Result<Materialized, QueryError>>;
    fn evaluate(
        &mut self,
        input: EvaluationInput<'_>,
    ) -> std::task::Poll<Result<Materialized, QueryError>>;
}
struct ReadyValues<'a, S>(&'a mut S);
impl<S: ComptimeValueSource> PreparingSource for ReadyValues<'_, S> {
    fn signature(
        &mut self,
        path: &DefPath,
    ) -> std::task::Poll<Result<hirsel_types::SignatureRef, QueryError>> {
        std::task::Poll::Ready(self.0.signature(path))
    }
    fn constant(
        &mut self,
        path: &DefPath,
        site: Span,
    ) -> std::task::Poll<Result<Materialized, QueryError>> {
        std::task::Poll::Ready(self.0.constant(path, site))
    }
    fn evaluate(
        &mut self,
        input: EvaluationInput<'_>,
    ) -> std::task::Poll<Result<Materialized, QueryError>> {
        std::task::Poll::Ready(self.0.evaluate(input))
    }
}

/// Saved preparation cursor over one immutable checked body. A pending read
/// keeps preceding substitutions and the selected signature, in their original
/// order, so suspension repeats neither completed work nor observable reads.
/// This is the same preparation algorithm used by synchronous callers.
pub(crate) struct ValuePreparation {
    reference: usize,
    selected_constant: Option<(ExprId, DefPath, TypeId, Span)>,
    expressions: Vec<(ExprId, ExprId, TypeId)>,
    expression: usize,
    constants: Vec<(ExprId, Materialized)>,
    terminal: Option<Result<(), QueryError>>,
}
impl ValuePreparation {
    pub(crate) fn new(analyzed: &AnalyzedBody) -> Self {
        let file = &analyzed.typed.syntax;
        let mut expressions: Vec<_> = analyzed
            .typed
            .checked
            .expressions
            .iter()
            .filter_map(
                |row| match file.tree.expressions[row.expression.0 as usize].kind {
                    ExprKind::Comptime(child) => Some((row.expression, child, row.ty)),
                    _ => None,
                },
            )
            .collect();
        expressions.sort_by_key(|(id, _, _)| {
            let span = file.tree.expressions[id.0 as usize].origin.span;
            (span.end - span.start, span.start)
        });
        Self {
            reference: 0,
            selected_constant: None,
            expressions,
            expression: 0,
            constants: vec![],
            terminal: None,
        }
    }
    pub(crate) fn poll(
        &mut self,
        types: &hirsel_types::TypeInterner,
        analyzed: &AnalyzedBody,
        staged: &[(ExprId, TypeId)],
        signature: &Signature,
        source: &mut impl PreparingSource,
    ) -> std::task::Poll<Result<&[(ExprId, Materialized)], QueryError>> {
        if let Some(result) = &self.terminal {
            return std::task::Poll::Ready(result.clone().map(|()| self.constants.as_slice()));
        }
        let result = self.advance(types, analyzed, staged, signature, source);
        match result {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(result) => {
                self.terminal = Some(result.clone());
                std::task::Poll::Ready(result.map(|()| self.constants.as_slice()))
            }
        }
    }
    fn advance(
        &mut self,
        types: &hirsel_types::TypeInterner,
        analyzed: &AnalyzedBody,
        staged: &[(ExprId, TypeId)],
        signature: &Signature,
        source: &mut impl PreparingSource,
    ) -> std::task::Poll<Result<(), QueryError>> {
        use std::task::{Poll, ready};
        let file = &analyzed.typed.syntax;
        let checked = &analyzed.typed.checked;
        while self.reference < checked.resolution.expressions.len() {
            if let Some((expression, path, ty, site)) = &self.selected_constant {
                let mut value = ready!(source.constant(path, *site))?;
                value.root.ty = *ty;
                self.constants.push((*expression, value));
                self.selected_constant = None;
                self.reference += 1;
                continue;
            }
            let (expression, name) = &checked.resolution.expressions[self.reference];
            if let ResolvedName::Definition(path) = name
                && matches!(
                    ready!(source.signature(path))?.kind,
                    SignatureKind::Const { .. }
                )
            {
                let ty = checked
                    .expressions
                    .iter()
                    .find(|row| row.expression == *expression)
                    .ok_or_else(|| operational("missing constant reference type"))?
                    .ty;
                self.selected_constant = Some((
                    *expression,
                    path.clone(),
                    ty,
                    file.tree.expressions[expression.0 as usize].origin.span,
                ));
            } else {
                self.reference += 1;
            }
        }
        while let Some(&(expression, child, ty)) = self.expressions.get(self.expression) {
            if !hirsel_check_protocol::body::comptime_inputs(types, file, checked, expression)
                .is_empty()
            {
                self.expression += 1;
                continue;
            }
            let value = ready!(source.evaluate(EvaluationInput {
                analyzed,
                staged,
                signature,
                expression,
                child,
                ty,
                constants: &self.constants,
                sizes: &[],
            }))?;
            self.constants.push((expression, value));
            self.expression += 1;
        }
        validate_repeats(types, analyzed, &self.constants)?;
        Poll::Ready(Ok(()))
    }
}

fn validate_repeats(
    types: &hirsel_types::TypeInterner,
    analyzed: &AnalyzedBody,
    constants: &[(ExprId, Materialized)],
) -> Result<(), QueryError> {
    let file = &analyzed.typed.syntax;
    let checked = &analyzed.typed.checked;
    for repeat in &file.tree.repeats {
        let Some(row) = checked
            .expressions
            .iter()
            .find(|r| r.expression == repeat.sequence)
        else {
            continue;
        };
        let Some((_, value)) = constants
            .iter()
            .find(|(id, _)| *id == repeat.evaluated_count)
        else {
            return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                const { hirsel_diag::RuleId::of("CT-2") },
                file.tree.expressions[repeat.count.0 as usize].origin.span,
                "repeat length must be known at definition time",
            )]));
        };
        let ValueKind::Constant(Constant::Integer(length)) = value.root.kind else {
            return Err(operational("repeat count is not an evaluated u64"));
        };
        let valid = match &types.row(row.ty).kind {
            hirsel_types::TypeKind::Builtin {
                kind: hirsel_types::BuiltinType::Array,
                ..
            } => true,
            hirsel_types::TypeKind::Builtin {
                kind: hirsel_types::BuiltinType::Inline,
                arguments,
            } => {
                matches!(arguments.get(1), Some(GenericArgument::Size(Size::Constant(n))) if u128::from(*n) == length)
            }
            _ => false,
        };
        if !valid {
            return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                const { hirsel_diag::RuleId::of("LANG-7") },
                file.tree.expressions[repeat.expression.0 as usize]
                    .origin
                    .span,
                "repeat literal requires Array or an Inline of the same length",
            )]));
        }
    }
    Ok(())
}

/// Reception types for hidden size-generic constants, using only immutable
/// checked input, structural type operations and a task-local signature reader.
pub(crate) fn prepare_staged_types(
    types: &hirsel_types::TypeInterner,
    analyzed: &AnalyzedBody,
    provider: &mut dyn SignatureProvider,
) -> Result<Vec<(ExprId, TypeId)>, QueryError> {
    let checked = &analyzed.typed.checked;
    let mut staged = vec![];
    let expressions: Vec<_> = checked
        .expressions
        .iter()
        .filter(|row| {
            matches!(
                analyzed.typed.syntax.tree.expressions[row.expression.0 as usize].kind,
                ExprKind::Comptime(_)
            ) && !hirsel_check_protocol::body::comptime_inputs(
                types,
                &analyzed.typed.syntax,
                checked,
                row.expression,
            )
            .is_empty()
        })
        .map(|row| (row.expression, row.ty))
        .collect();
    for (expression, ty) in expressions {
        let generics = &checked.resolution.generic_declarations;
        let mut lookup = |d: DefId| provider.signature(d).map(Some);
        let plain = hirsel_types::structural_property(
            types,
            ty,
            hirsel_types::StructuralProperty::Plain,
            generics,
            &mut lookup,
        )?;
        let flat = hirsel_types::structural_flat(types, ty, generics, &mut lookup)?;
        let protected = matches!(
            types.row(ty).kind,
            hirsel_types::TypeKind::Builtin {
                kind: hirsel_types::BuiltinType::Shared,
                ..
            }
        );
        let shared = if plain || flat || protected {
            ty
        } else {
            let mut row = types.row(ty).clone();
            row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
            let root = types.intern(row.clone());
            row.modes.sendability =
                hirsel_types::sendability_after_sharing(types, root, generics, &mut lookup)?;
            types.intern(row)
        };
        staged.push((expression, shared));
    }
    Ok(staged)
}

fn operational_limit(limit: &hirsel_tir::ToolingLimit) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "compile-time tooling limit".into(),
        message: format!("{limit:?}"),
    })
}

/// Serial request adapter for the same read/product bridge S1 workers use.
/// Only this adapter borrows an engine; evaluation itself sees immutable TIR
/// plus a task-local dependency source.
struct EngineDependencies<'a>(&'a mut QueryEngine);
impl EngineDependencies<'_> {
    fn recorded<T>(
        &mut self,
        request: impl FnOnce(&mut QueryEngine) -> Result<T, QueryError>,
    ) -> crate::queries::s1::DependencyProduct<T> {
        self.0.execution_stack.push(ReadsBuilder::new());
        let result = request(self.0);
        let reads = self
            .0
            .execution_stack
            .pop()
            .expect("dependency read frame")
            .finish();
        match reads {
            Ok(reads) => crate::queries::s1::DependencyProduct { result, reads },
            Err(_) => crate::queries::s1::DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            },
        }
    }
}
impl crate::queries::s1::DependencySource for EngineDependencies<'_> {
    fn constant(
        &mut self,
        path: &DefPath,
    ) -> crate::queries::s1::DependencyProduct<hirsel_tir::MaterializedRef> {
        self.recorded(|engine| engine.materialize_value(path).map(Arc::new))
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> crate::queries::s1::DependencyProduct<hirsel_tir::MaterializedRef> {
        self.recorded(|engine| engine.constant_instance(path, arguments))
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> crate::queries::s1::DependencyProduct<Option<DefPath>> {
        self.recorded(|engine| engine.resolve_instance(path, arguments))
    }
    fn definition(
        &mut self,
        path: &DefPath,
    ) -> crate::queries::s1::DependencyProduct<DefinitionRef> {
        self.recorded(|engine| {
            engine.read_implementation(path);
            let definition = engine.definition(path)?;
            if !engine.evaluation_acceptance.is_empty() && engine.verify == Verify::On {
                let evidence = engine.ensure_lowered_verdict(&definition)?;
                engine.record_evaluation_acceptance(
                    evidence.and_then(|evidence| evidence.accepted_artifact()),
                );
            }
            if engine.verify == Verify::On && !engine.evaluation_validation.is_empty() {
                let digest = engine
                    .signature_snapshot()
                    .validation_input_digest(&path.0.package);
                engine.record_evaluation_validation(path.0.package.clone(), digest);
            }
            let signature = engine.item_signature(path)?.value;
            let entry = match &engine.adapters.lowering {
                Some(lowering) => lowering.entry_body(),
                None => return Err(QueryError::NotImplemented),
            };
            if matches!(&signature.kind, hirsel_types::SignatureKind::Type(t) if t.resource)
                && definition.entry != entry
            {
                return Err(operational(
                    "finalizer entry disagrees with the producer layout",
                ));
            }
            Ok(definition)
        })
    }
}
impl QueryEngine {
    pub(crate) fn record_evaluation_validation(
        &mut self,
        package: hirsel_syntax::PackageId,
        digest: ContentHash,
    ) {
        for frame in &mut self.evaluation_validation {
            if let Some(rows) = frame {
                if rows
                    .insert(package.clone(), digest)
                    .is_some_and(|old| old != digest)
                {
                    *frame = None;
                }
            }
        }
    }
    pub(crate) fn record_evaluation_acceptance(
        &mut self,
        acceptance: Option<hirsel_validate::AcceptedArtifact>,
    ) {
        for frame in &mut self.evaluation_acceptance {
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
    pub(crate) fn begin_evaluation_validation(&mut self) {
        self.evaluation_selections.push(vec![]);
        self.evaluation_validation
            .push((self.verify == Verify::On).then(Default::default));
        self.evaluation_acceptance
            .push((self.verify == Verify::On).then(Vec::new));
    }
    pub(crate) fn end_evaluation_validation(
        &mut self,
    ) -> (
        Option<Arc<[(hirsel_syntax::PackageId, ContentHash)]>>,
        Option<hirsel_validate::ExecutionReceipt>,
        Vec<Selection>,
    ) {
        let validation = self
            .evaluation_validation
            .pop()
            .expect("scalar validation frame")
            .map(|rows| Arc::from(rows.into_iter().collect::<Vec<_>>()));
        let acceptance = self
            .evaluation_acceptance
            .pop()
            .expect("scalar acceptance frame");
        let receipt =
            validation
                .as_deref()
                .zip(acceptance.as_deref())
                .and_then(|(contexts, artifacts)| {
                    hirsel_validate::ExecutionReceipt::compose(contexts, artifacts)
                });
        let staged = self
            .evaluation_selections
            .pop()
            .expect("evaluation selection frame");
        (validation, receipt, staged)
    }
    fn finish_evaluation_validation(&mut self, result: &mut Result<EvaluatedConstant, QueryError>) {
        let (validation, acceptance, selections) = self.end_evaluation_validation();
        if let Ok(value) = result {
            value.validation = validation;
            value.acceptance = acceptance;
            value.selections = selections;
        }
    }
    pub(crate) fn lower_acceptance_holds(
        &mut self,
        receipt: Option<&hirsel_validate::ExecutionReceipt>,
    ) -> bool {
        if self.verify == Verify::Off {
            return true;
        }
        receipt.is_none_or(|receipt| {
            let snapshot = self.signature_snapshot();
            receipt
                .contexts()
                .iter()
                .all(|(package, digest)| snapshot.validation_input_digest(package) == *digest)
        })
    }
    pub(crate) fn merge_lower_acceptance(
        &mut self,
        receipt: Option<&hirsel_validate::ExecutionReceipt>,
        required: bool,
    ) {
        if let Some(receipt) = receipt {
            for artifact in receipt.artifacts() {
                self.record_evaluation_acceptance(Some(artifact.clone()));
            }
            for (package, digest) in receipt.contexts() {
                self.record_evaluation_validation(package.clone(), *digest);
            }
        } else if required {
            self.evaluation_acceptance.fill(None);
            self.evaluation_validation.fill(None);
        }
    }
    pub(crate) fn evaluation_validation_holds(&mut self, value: &EvaluatedConstant) -> bool {
        self.evaluation_context_holds(value.validation.as_deref(), &value.executed)
    }
    pub(crate) fn evaluation_context_holds(
        &mut self,
        validation: Option<&[(hirsel_syntax::PackageId, hirsel_syntax::ContentHash)]>,
        executed: &crate::memo::Reads,
    ) -> bool {
        if self.verify == Verify::Off {
            return true;
        }
        if let Some(rows) = validation {
            let snapshot = self.signature_snapshot();
            return rows
                .iter()
                .all(|(package, digest)| snapshot.validation_input_digest(package) == *digest);
        }
        executed
            .iter()
            .any(|(key, _)| matches!(key, crate::ReadKey::ValidationUniverse(_)))
    }
    fn merge_evaluation_validation(&mut self, value: &EvaluatedConstant) {
        self.record_selections(&value.selections);
        if let Some(receipt) = &value.acceptance {
            for artifact in receipt.artifacts() {
                self.record_evaluation_acceptance(Some(artifact.clone()));
            }
        } else {
            self.evaluation_acceptance.fill(None);
        }
        if let Some(rows) = &value.validation {
            for (package, digest) in rows.iter() {
                self.record_evaluation_validation(package.clone(), *digest);
            }
        } else {
            let mut witnessed = false;
            for (key, digest) in value.executed.iter() {
                if let crate::ReadKey::ValidationUniverse(package) = key {
                    witnessed = true;
                    self.record_evaluation_validation(package.id(), *digest);
                }
            }
            if !witnessed {
                self.evaluation_validation.fill(None);
            }
        }
    }
    pub(crate) fn record_staged_selection(&mut self, path: DefId, arguments: &[u64]) {
        for frame in &mut self.evaluation_selections {
            frame.push(Selection::Staged(path, arguments.to_vec()));
        }
    }
    pub(crate) fn record_generated_selection(&mut self, path: DefId) {
        for frame in &mut self.evaluation_selections {
            frame.push(Selection::Generated(path));
        }
    }
    pub(crate) fn record_selections(&mut self, selections: &[Selection]) {
        for frame in &mut self.evaluation_selections {
            frame.extend_from_slice(selections);
        }
    }
    /// Complete execution evidence of a retained constant materialization.
    /// This includes lookup, citation, private-view and transitive body reads;
    /// graph memo counters alone cannot establish that closure.
    pub fn materialization_reads(&self, constant: &DefPath) -> Option<&crate::Reads> {
        self.constant_values
            .get(constant)
            .map(|value| &value.executed)
    }
    /// The views `constant`'s retained evaluation reflected or decoded
    /// (`Representation(d)`), each with the digest it saw.
    pub fn representation_reads(&self, constant: &DefPath) -> Option<Vec<(DefPath, ContentHash)>> {
        let value = self.constant_values.get(constant)?;
        Some(
            value
                .executed
                .iter()
                .filter_map(|(read, digest)| match read {
                    crate::memo::ReadKey::Representation(d) => Some((d.path().clone(), *digest)),
                    _ => None,
                })
                .collect(),
        )
    }
    /// Evaluate a declared constant in this source snapshot. Implementation changes
    /// invalidate materializations, even when every exported signature is unchanged.
    pub fn materialize_constant(&mut self, path: &DefPath) -> Result<Constant, QueryError> {
        match self.materialize_value(path)?.root.kind {
            ValueKind::Constant(value) => Ok(value),
            _ => Err(operational(
                "constant is aggregate data; use materialize_value",
            )),
        }
    }
    pub fn materialize_value(&mut self, path: &DefPath) -> Result<Materialized, QueryError> {
        self.evaluate_constant(path).map(|c| c.value)
    }
    pub(crate) fn evaluate_constant(
        &mut self,
        path: &DefPath,
    ) -> Result<EvaluatedConstant, QueryError> {
        self.prepare_materialization_environment();

        if self.s1.is_empty() {
            self.last_comptime_limit = None;
        }
        if let Some(value) = self.registry_dictionary_value(path)? {
            return Ok(EvaluatedConstant {
                value,
                substitutions: vec![],
                executed: Default::default(),
                depth: DepthEvidence::leaf(self.depth_limit()),
                selections: vec![],
                validation: None,
                acceptance: None,
            });
        }
        let reference = match self.declarations.sites(path).first() {
            Some(site) => site.name_span,
            None => self.parse_definition(path)?.value.item.name_span,
        };
        self.constant_at(path, reference)
    }
    fn constant_at(
        &mut self,
        path: &DefPath,
        reference: Span,
    ) -> Result<EvaluatedConstant, QueryError> {
        let result = self.constant_query(path, reference);
        let released = self.release_batch_front(path);
        result.and_then(|output| released.map(|()| output))
    }
    fn constant_query(
        &mut self,
        path: &DefPath,
        reference: Span,
    ) -> Result<EvaluatedConstant, QueryError> {
        if let Some(value) = self.registry_dictionary_value(path)? {
            return Ok(EvaluatedConstant {
                value,
                substitutions: vec![],
                executed: Default::default(),
                depth: DepthEvidence::leaf(self.depth_limit()),
                selections: vec![],
                validation: None,
                acceptance: None,
            });
        }
        let cached = self.constant_values.get(path).cloned();
        if let Some(value) = cached {
            if self.evaluation_validation_holds(&value)
                && self.execution_valid(path, &value.executed)
            {
                self.depth_admit(&value.depth, reference)?;
                self.merge_execution(&value.executed);
                self.merge_evaluation_validation(&value);
                self.depth_requested(&value.depth, reference);
                return Ok(value);
            }
            self.constant_values.remove(path);
        }
        if let Some(value) = self.load_evaluation(path) {
            self.depth_admit(&value.depth, reference)?;
            self.merge_execution(&value.executed);
            self.merge_evaluation_validation(&value);
            self.depth_requested(&value.depth, reference);
            self.constant_values.insert(path.clone(), value.clone());
            return Ok(value);
        }
        if let Some(start) = self.s1.on_stack(path) {
            self.s1.fail_cycle();
            let mut diagnostics: Vec<_> = self
                .s1
                .cycle_sites(start, reference)
                .map(|site| {
                    Diagnostic::error(
                        const { hirsel_diag::RuleId::of("CT-4") },
                        site,
                        "compile-time dependency cycle",
                    )
                })
                .collect();
            diagnostics = crate::select_diagnostics(diagnostics, Some(&self.sources));
            return Err(QueryError::Diagnostics(vec![diagnostics.remove(0)]));
        }
        // Keep recursive compiler queries bounded separately from the interpreter's guard.
        self.depth_admit(&DepthEvidence::leaf(self.depth_limit()), reference)?;
        self.restore_execution_mode(path);
        if self.s1.enter(path.clone(), reference) == Entered::Settled {
            // Another worker finished it: its success is in the memo, or this
            // stack evaluates it again.
            return self.constant_at(path, reference);
        }
        self.execution_stack.push(Default::default());
        self.begin_evaluation_validation();
        self.read_implementation(path);
        let mut result = (|| {
            let syntax = self.parse_definition(path)?.value;
            if !matches!(syntax.item.kind, hirsel_syntax::ItemKind::Const { .. }) {
                return Err(operational("materialization requested for a non-constant"));
            }
            let signature = self.item_signature(path)?.value;
            let body = self.mode_query(path)?;
            self.read_execution_signatures(&body.dependencies);
            let body = &body.value;
            let hirsel_syntax::ItemKind::Const { value, .. } = body.typed.syntax.item.kind else {
                return Err(operational("constant artifact has a non-constant owner"));
            };
            let mut constants = self.prepare_values(body, &[], &signature)?;
            let dictionary = crate::dictionaries::declaration(
                &self.types.clone(),
                &self.memo.clone(),
                body,
                &signature,
                &constants,
                &mut EngineComptimeValues(self),
            )?;
            let is_dictionary = dictionary.is_some();
            let constant = if let Some(dictionary) = dictionary {
                dictionary
            } else if let Some((_, constant)) = constants.iter().find(|(e, _)| *e == value) {
                constant.clone()
            } else {
                let ty = body
                    .typed
                    .checked
                    .expressions
                    .iter()
                    .find(|r| r.expression == value)
                    .ok_or_else(|| operational("missing constant expression type"))?
                    .ty;
                self.evaluate_value(body, &[], &signature, value, value, ty, &constants)?
            };
            if is_dictionary && !constants.iter().any(|(id, _)| *id == value) {
                constants.push((value, constant.clone()));
            }
            let executed = self
                .execution_stack
                .last()
                .expect("constant execution frame")
                .clone()
                .finish()
                .map_err(|_| QueryError::NotImplemented)?;
            Ok(EvaluatedConstant {
                value: constant,
                substitutions: constants,
                executed,
                depth: self.depth_composed(true),
                selections: vec![],
                validation: None,
                acceptance: None,
            })
        })();
        self.execution_stack.pop();
        self.finish_evaluation_validation(&mut result);
        if let Ok(evaluated) = &result {
            self.constant_values.insert(path.clone(), evaluated.clone());
        }
        self.s1.leave();
        if let Ok(evaluated) = &result {
            self.store_evaluation(path, evaluated)?;
            self.depth_requested(&evaluated.depth, reference);
        }
        result
    }
    /// The compile-time dependency limit L.
    pub(crate) fn depth_limit(&self) -> u32 {
        crate::memo::dependency_limit(self.comptime_limits.call_depth)
    }
    /// Requests a result with `evidence` at `site` from the current stack
    /// length: the cache-free evaluation's verdict, whether the result was
    /// computed, stored or came from another worker. A leaf's evidence decides
    /// only the zero-budget case, before computing.
    pub(crate) fn depth_admit(
        &mut self,
        evidence: &DepthEvidence<Span>,
        site: Span,
    ) -> Result<(), QueryError> {
        self.s1.admit(evidence, site).map_err(|site| {
            self.comptime_limit(hirsel_tir::ToolingLimit {
                kind: hirsel_tir::ToolingLimitKind::CallDepth,
                limit: evidence.limit() as u64,
                site: Some(site),
            })
        })
    }
    /// Replay a lowering's requests at this caller's depth. Lowering itself
    /// adds no S1 frame; its cached substitutions still consume dependency
    /// budget exactly where their source evaluation would have done so.
    pub(crate) fn depth_replay_requests(
        &mut self,
        evidence: &DepthEvidence<Span>,
    ) -> Result<(), QueryError> {
        self.s1.replay_requests(evidence).map_err(|site| {
            self.comptime_limit(hirsel_tir::ToolingLimit {
                kind: hirsel_tir::ToolingLimitKind::CallDepth,
                limit: evidence.limit() as u64,
                site: Some(site),
            })
        })
    }
    /// Records a completed request in the requester's frame, in request order.
    pub(crate) fn depth_requested(&mut self, evidence: &DepthEvidence<Span>, site: Span) {
        self.s1.requested(evidence, site);
    }
    /// The evidence of the evaluation whose frame is on top; `bounded` for a
    /// constant or staged evaluation, not for a generator.
    pub(crate) fn depth_composed(&self, bounded: bool) -> DepthEvidence<Span> {
        self.s1.composed(self.depth_limit(), bounded)
    }
    /// The compile-time products lowering `analyzed` reads.
    pub fn prepare_comptime(
        &mut self,
        analyzed: &AnalyzedBody,
        signature: &Signature,
    ) -> Result<Comptime, QueryError> {
        self.prepare_materialization_environment();
        let staged = self.prepare_staged_types(analyzed)?;
        if self.adapters.phases.is_none() {
            return Ok(Comptime {
                materialized: vec![],
                staged,
            });
        }
        let materialized = if matches!(
            signature.kind,
            hirsel_types::SignatureKind::Const { ty: _, .. }
        ) {
            self.evaluate_constant(signature.definition.path())?
                .substitutions
        } else {
            self.prepare_values(analyzed, &staged, signature)?
        };
        Ok(Comptime {
            materialized,
            staged,
        })
    }
    pub(crate) fn prepare_values(
        &mut self,
        analyzed: &AnalyzedBody,
        staged: &[(ExprId, TypeId)],
        signature: &Signature,
    ) -> Result<Vec<(ExprId, Materialized)>, QueryError> {
        self.register_staged(analyzed)?;
        let types = self.types.clone();
        prepare_values(
            &types,
            analyzed,
            staged,
            signature,
            &mut EngineComptimeValues(self),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_value(
        &mut self,
        analyzed: &AnalyzedBody,
        staged: &[(ExprId, TypeId)],
        signature: &Signature,
        expression: ExprId,
        child: ExprId,
        ty: TypeId,
        constants: &[(ExprId, Materialized)],
    ) -> Result<Materialized, QueryError> {
        self.evaluate_sized_value(
            analyzed,
            staged,
            signature,
            expression,
            child,
            ty,
            constants,
            &[],
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn evaluate_sized_value(
        &mut self,
        analyzed: &AnalyzedBody,
        staged: &[(ExprId, TypeId)],
        signature: &Signature,
        expression: ExprId,
        child: ExprId,
        ty: TypeId,
        constants: &[(ExprId, Materialized)],
        sizes: &[(SizeParameterRef, u64)],
    ) -> Result<Materialized, QueryError> {
        let (value, ty) = self.run_evaluation(
            analyzed, staged, signature, expression, child, ty, constants, sizes,
        )?;
        self.reify_value(
            &signature.definition.path().0.package,
            value,
            ty,
            analyzed.typed.syntax.tree.expressions[expression.0 as usize]
                .origin
                .span,
        )
    }
    /// Run one checked compile-time expression to its interpreter value and
    /// result type; evaluation failures are CT-1.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_evaluation(
        &mut self,
        analyzed: &AnalyzedBody,
        staged: &[(ExprId, TypeId)],
        signature: &Signature,
        expression: ExprId,
        child: ExprId,
        ty: TypeId,
        constants: &[(ExprId, Materialized)],
        sizes: &[(SizeParameterRef, u64)],
    ) -> Result<(hirsel_interp::Value, TypeId), QueryError> {
        let _timing = hirsel_intern::timings::phase(hirsel_intern::timings::Phase::Comptime);
        let file = &analyzed.typed.syntax;
        let lowering = self
            .adapters
            .lowering
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let prepared = EvaluationInput {
            analyzed,
            staged,
            signature,
            expression,
            child,
            ty,
            constants,
            sizes,
        }
        .lower(&self.types, &self.memo, &*lowering)?;
        let snapshot = (self.verify == Verify::On).then(|| self.signature_snapshot());
        let site = file.tree.expressions[expression.0 as usize].origin.span;
        if let Some(snapshot) = &snapshot {
            let package = &prepared.definition.path.path().0.package;
            self.record_evaluation_validation(
                package.clone(),
                snapshot.validation_input_digest(package),
            );
        }
        let validated = crate::queries::s1::ValidatedEvaluation::new(
            prepared,
            snapshot.as_deref(),
            self.verify,
            site,
        )?;
        let sources = self.sources.clone();
        let limits = self.comptime_limits;
        let runtime = self.comptime_runtime_types()?;
        let product = validated.run(&sources, limits, &runtime, &mut EngineDependencies(self));
        self.merge_execution(&product.value.reads);
        self.record_evaluation_acceptance(product.acceptance);
        if let Some(limit) = product.limit {
            self.last_comptime_limit = Some(limit);
        }
        product.value.result
    }
    pub(crate) fn comptime_limit(&mut self, limit: hirsel_tir::ToolingLimit) -> QueryError {
        self.last_comptime_limit = Some(limit.clone());
        operational_limit(&limit)
    }
}

impl QueryEngine {
    /// The shared reception type of each hidden size-generic constant in `analyzed`.
    fn prepare_staged_types(
        &mut self,
        analyzed: &AnalyzedBody,
    ) -> Result<Vec<(ExprId, TypeId)>, QueryError> {
        let types = self.types.clone();
        prepare_staged_types(&types, analyzed, self)
    }
    /// Demand the declarations introduced by a source definition's staged
    /// expressions. Publication uses the same complete observed inputs as its
    /// analyzed body; a durable hit does not restore that body.
    pub fn register_staged_definition(&mut self, path: &DefPath) -> Result<(), QueryError> {
        self.prepare_materialization_environment();
        if self.load_registrations(path).is_some() {
            return Ok(());
        }
        let analyzed = self.mode_query(path)?;
        self.register_staged(&analyzed.value)
    }
    pub(crate) fn register_staged(&mut self, analyzed: &AnalyzedBody) -> Result<(), QueryError> {
        let typed = &analyzed.typed;
        for row in &typed.checked.expressions {
            if !matches!(
                typed.syntax.tree.expressions[row.expression.0 as usize].kind,
                ExprKind::Comptime(_)
            ) {
                continue;
            }
            let inputs = hirsel_check_protocol::body::comptime_inputs(
                &self.types,
                &typed.syntax,
                &typed.checked,
                row.expression,
            );
            if inputs.is_empty() {
                continue;
            }
            let signature = self.item_signature(&typed.syntax.item.definition)?.value;
            let registration = StagedDeclarationInput {
                analyzed,
                expression: row.expression,
                ty: row.ty,
                inputs,
            }
            .prepare(&self.types, &self.memo, &signature)?;
            self.install_staged(registration);
        }
        self.store_registrations(&typed.syntax.item.definition)
    }
    pub(crate) fn install_staged(&mut self, registration: StagedRegistration) {
        let StagedRegistration { path, constant } = registration;
        if self.staged_constants.get(&path).is_some_and(|old| {
            old.signature.fingerprint == constant.signature.fingerprint
                && old.inputs == constant.inputs
                && old.expression == constant.expression
                && old.owner == constant.owner
        }) {
            return;
        }
        self.retire_snapshot();
        self.staged_constants.insert(path, constant);
    }
    pub(crate) fn materialize_staged(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<EvaluatedConstant, QueryError> {
        let owner = self
            .staged_constants
            .get(path)
            .map(|constant| constant.owner.clone());
        let result = self.materialize_staged_query(path, arguments);
        let released = owner
            .as_ref()
            .map_or(Ok(()), |owner| self.release_batch_front(owner));
        result.and_then(|output| released.map(|()| output))
    }
    fn materialize_staged_query(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<EvaluatedConstant, QueryError> {
        self.prepare_materialization_environment();
        if self.s1.is_empty() {
            self.last_comptime_limit = None;
        }
        let key = (path.clone(), arguments.to_vec());
        let cached = self.staged_values.get(&key).cloned();
        let constant = self
            .staged_constants
            .get(path)
            .cloned()
            .ok_or_else(|| operational("missing staged constant declaration"))?;
        // The staged declaration is part of execution, even on a value hit.
        // If indexing retires it, enclosing cached constants/lowerings must
        // miss and reconstruct the declaration and its selected instances.
        self.merge_execution(
            &crate::memo::Reads::from_vec(vec![(
                crate::memo::ReadKey::Signature(DefId::intern(path)),
                crate::queries::memos::signature_value(Some(constant.signature.fingerprint)),
            )])
            .expect("one staged declaration"),
        );
        if constant.inputs.len() != arguments.len() {
            return Err(operational("staged constant arity"));
        }
        let site = self.comptime_site(&constant.owner, constant.expression)?;
        self.record_staged_selection(DefId::intern(path), arguments);
        if let Some(value) = cached {
            if self.evaluation_validation_holds(&value)
                && self.execution_valid(&constant.owner, &value.executed)
            {
                self.depth_admit(&value.depth, site)?;
                self.merge_execution(&value.executed);
                self.merge_evaluation_validation(&value);
                self.depth_requested(&value.depth, site);
                return Ok(value);
            }
        }
        if let Some(value) = self.load_staged_evaluation(path, arguments) {
            self.depth_admit(&value.depth, site)?;
            self.merge_execution(&value.executed);
            self.merge_evaluation_validation(&value);
            self.depth_requested(&value.depth, site);
            self.staged_values.insert(key, value.clone());
            return Ok(value);
        }
        let body = self.mode_query(&constant.owner)?;
        let staged = self.prepare_staged_types(&body.value)?;
        let file = &body.value.typed.syntax;
        let instance = hirsel_tir::constant_instance_path(path, arguments);
        if self.s1.on_stack(&instance).is_some() {
            self.s1.fail_cycle();
            return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                const { hirsel_diag::RuleId::of("CT-4") },
                site,
                "compile-time dependency cycle",
            )]));
        }
        self.depth_admit(&DepthEvidence::leaf(self.depth_limit()), site)?;
        if self.s1.enter(instance, site) == Entered::Settled {
            return self.materialize_staged(path, arguments);
        }
        self.execution_stack.push(Default::default());
        self.begin_evaluation_validation();
        self.read_implementation(&constant.owner);
        self.merge_mode_execution(&constant.owner);
        self.read_execution_signatures(&body.dependencies);
        let mut result = (|| {
            let signature = self.item_signature(&constant.owner)?.value;
            let constants = self.prepare_values(&body.value, &staged, &signature)?;
            let ExprKind::Comptime(child) =
                file.tree.expressions[constant.expression.0 as usize].kind
            else {
                return Err(operational("staged expression no longer exists"));
            };
            let ty = body
                .value
                .typed
                .checked
                .expressions
                .iter()
                .find(|r| r.expression == constant.expression)
                .ok_or_else(|| operational("missing staged result"))?
                .ty;
            let sizes: Vec<_> = constant
                .inputs
                .iter()
                .copied()
                .zip(arguments.iter().copied())
                .collect();
            let value = self.evaluate_sized_value(
                &body.value,
                &staged,
                &signature,
                constant.expression,
                child,
                ty,
                &constants,
                &sizes,
            )?;
            Ok(EvaluatedConstant {
                value,
                substitutions: constants,
                executed: self
                    .execution_stack
                    .last()
                    .unwrap()
                    .clone()
                    .finish()
                    .map_err(|_| QueryError::NotImplemented)?,
                depth: self.depth_composed(true),
                selections: vec![],
                validation: None,
                acceptance: None,
            })
        })();
        self.execution_stack.pop();
        self.finish_evaluation_validation(&mut result);
        if let Ok(value) = &result {
            self.staged_values.insert(key, value.clone());
        }
        self.s1.leave();
        if let Ok(value) = &result {
            self.store_staged_evaluation(path, arguments, value)?;
            self.merge_execution(&value.executed);
            self.depth_requested(&value.depth, site);
        }
        result
    }
}

fn substitute_sizes_type(
    t: &hirsel_types::TypeInterner,
    m: &hirsel_types::TypeMemo,
    mut ty: TypeId,
    replacements: &[(SizeParameterRef, Size)],
) -> Result<TypeId, QueryError> {
    let scopes: std::collections::BTreeSet<_> =
        replacements.iter().map(|(p, _)| p.binder()).collect();
    for scope in scopes {
        let count = replacements
            .iter()
            .filter(|(p, _)| p.binder() == scope)
            .map(|(p, _)| p.parameter().0 as usize + 1)
            .max()
            .unwrap();
        let arguments: Vec<_> = (0..count)
            .map(|i| {
                GenericArgument::Size(
                    replacements
                        .iter()
                        .find(|(p, _)| p.binder() == scope && p.parameter().0 as usize == i)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| {
                            Size::Parameter(match scope {
                                GenericBinder::Body(owner) => SizeParameterRef::body(
                                    owner,
                                    hirsel_types::SizeParamId(i as u32),
                                ),
                                GenericBinder::Bound(depth) => SizeParameterRef::Bound {
                                    depth,
                                    parameter: hirsel_types::SizeParamId(i as u32),
                                },
                            })
                        }),
                )
            })
            .collect();
        ty = hirsel_types::substitute(
            t,
            m,
            ty,
            &hirsel_types::Substitution {
                binder: Some(scope),
                arguments: &arguments,
                ..Default::default()
            },
        )
        .map_err(|e| operational(format!("compile-time size substitution: {e:?}")))?;
    }
    Ok(ty)
}

/// Specialize the private evaluation artifact, leaving published generic TIR
/// shared. Only its selected size inputs change; ordinary calls still execute.
fn specialize_sizes(
    t: &hirsel_types::TypeInterner,
    m: &hirsel_types::TypeMemo,
    definition: &mut hirsel_tir::Definition,
    sizes: &[(SizeParameterRef, u64)],
) -> Result<(), QueryError> {
    if sizes.is_empty() {
        return Ok(());
    }
    let replacements: Vec<_> = sizes
        .iter()
        .map(|(p, n)| (*p, Size::Constant(*n)))
        .collect();
    let mut memo = std::collections::HashMap::new();
    let mut ty = |root: &mut TypeId| -> Result<(), QueryError> {
        *root = match memo.get(root) {
            Some(mapped) => *mapped,
            None => {
                let mapped = substitute_sizes_type(t, m, *root, &replacements)?;
                memo.insert(*root, mapped);
                mapped
            }
        };
        Ok(())
    };
    let size = |n: &mut Size| {
        if let Size::Parameter(p) = n {
            if let Some((_, value)) = sizes.iter().find(|(parameter, _)| parameter == p) {
                *n = Size::Constant(*value);
            }
        }
    };
    let mut arguments = |args: &mut Vec<GenericArgument>| -> Result<(), QueryError> {
        for a in args {
            match a {
                GenericArgument::Type(t) => ty(t)?,
                GenericArgument::Size(n) => size(n),
            }
        }
        Ok(())
    };
    // Keep one memo for every rooted graph.
    for body in &mut definition.bodies {
        // These roots use the same substitution as call/type-directed operands.
        for root in body
            .locals
            .iter_mut()
            .map(|l| &mut l.ty)
            .chain(body.places.iter_mut().map(|p| &mut p.ty))
            .chain(std::iter::once(&mut body.result))
        {
            let mut a = vec![GenericArgument::Type(*root)];
            arguments(&mut a)?;
            let GenericArgument::Type(t) = a[0] else {
                unreachable!()
            };
            *root = t;
        }
        for value in &mut body.rvalues {
            match value {
                hirsel_tir::Rvalue::Size(parameter) => {
                    if let Some((_, n)) = sizes.iter().find(|(p, _)| p == parameter) {
                        *value = hirsel_tir::Rvalue::Constant(Constant::Integer(u128::from(*n)));
                    }
                }
                hirsel_tir::Rvalue::ReadConstant { arguments, .. } => {
                    for n in arguments {
                        size(n);
                    }
                }
                hirsel_tir::Rvalue::FunctionReference { arguments: a, .. } => arguments(a)?,
                hirsel_tir::Rvalue::Shape(t) => {
                    let mut a = vec![GenericArgument::Type(*t)];
                    arguments(&mut a)?;
                    let GenericArgument::Type(root) = a[0] else {
                        unreachable!()
                    };
                    *t = root;
                }
                _ => (),
            }
        }
        for block in &mut body.blocks {
            if let hirsel_tir::Terminator::Call { callee, .. } = &mut block.terminator {
                match callee {
                    hirsel_tir::Callee::Definition { arguments: a, .. }
                    | hirsel_tir::Callee::Indirect { arguments: a, .. } => arguments(a)?,
                }
            }
        }
    }
    // The private artifact's persisted table is the closure of its new roots.
    definition.types = hirsel_tir::definition_types(t, definition);
    Ok(())
}

impl QueryEngine {
    /// Select staged constants from final-mode call actuals, after signatures and
    /// GEN-5, reading each body's definition summary. Local generic frames
    /// retain their enclosing lexical size bindings. Calls inside comptime are
    /// selected by execution, preserving dead branches.
    pub(crate) fn prepare_staged_instances(&mut self) -> Result<(), QueryError> {
        use std::collections::{BTreeMap, BTreeSet, VecDeque};
        if self.staged_constants.is_empty() {
            return Ok(());
        }
        let roots: Vec<_> = self
            .items
            .iter()
            .filter_map(|(path, items)| {
                items
                    .first()
                    .filter(|syntax| !matches!(syntax.item.kind, ItemKind::Const { .. }))
                    .map(|syntax| (path.clone(), syntax.item.origin.span))
            })
            .collect();
        let mut pending = VecDeque::new();
        for (path, site) in roots {
            let published = self.front_summary(DefId::intern(&path))?;
            if !published.summary.frames[0].generic {
                let scope = NameSite::Item;
                pending.push_back((
                    path.clone(),
                    scope,
                    BTreeMap::<SizeParameterRef, u64>::new(),
                    site,
                ));
            }
        }
        let mut seen = BTreeSet::new();
        let mut registered = BTreeSet::new();
        while let Some((path, scope, bindings, site)) = pending.pop_front() {
            if !seen.insert((path.clone(), scope, bindings.clone())) {
                continue;
            }
            if seen.len() > 65536 {
                return Err(operational("compile-time instance expansion limit"));
            }
            let published = self.front_summary(DefId::intern(&path))?;
            let summary = &published.summary;
            // Registration demands its own compact product; traversal retains only
            // the published summary, including on repeated size instantiations.
            if !summary.staged.is_empty() && registered.insert(path.clone()) {
                let registered = self.register_staged_definition(&path);
                let released = self.release_batch_front(&path);
                registered?;
                released?;
            }
            let frames = &summary.frames;
            let frame_of = |site: NameSite| {
                frames
                    .iter()
                    .position(|frame| frame.site == site)
                    .map(|i| i as u32)
            };
            // The function frame a frame belongs to, and whether a comptime
            // region encloses it.
            let function = |mut frame: u32| {
                while frames[frame as usize].kind == FrameKind::ComptimeRegion {
                    frame = frames[frame as usize].parent.expect("region parent");
                }
                frame
            };
            let staged = |mut frame: u32| loop {
                if frames[frame as usize].kind == FrameKind::ComptimeRegion {
                    return true;
                }
                match frames[frame as usize].parent {
                    Some(parent) => frame = parent,
                    None => return false,
                }
            };
            let Some(current) = frame_of(scope) else {
                continue;
            };
            let mut nested: Vec<NameSite> = frames
                .iter()
                .enumerate()
                .filter(|(i, frame)| {
                    let i = *i as u32;
                    frame.kind != FrameKind::ComptimeRegion
                        && i != current
                        && frame.parent.map(function) == Some(current)
                        && !staged(i)
                        && !frame.generic
                })
                .map(|(_, frame)| frame.site)
                .collect();
            nested.sort();
            for frame in nested {
                pending.push_back((path.clone(), frame, bindings.clone(), site));
            }
            let constants: Vec<_> = self
                .staged_constants
                .iter()
                .filter(|(_, constant)| {
                    constant.owner == path
                        && frame_of(NameSite::Expression(constant.expression))
                            .and_then(|region| frames[region as usize].parent)
                            .map(function)
                            == Some(current)
                })
                .map(|(path, constant)| (path.clone(), constant.inputs.clone()))
                .collect();
            for (constant, inputs) in constants {
                let arguments = inputs
                    .iter()
                    .map(|input| bindings.get(input).copied())
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| operational("selected comptime frame has an unbound size"))?;
                if let Err(mut error) = self.materialize_staged(&constant, &arguments) {
                    if let QueryError::Diagnostics(diagnostics) = &mut error {
                        for diagnostic in diagnostics {
                            diagnostic.secondary.push(hirsel_diag::Label {
                                span: site,
                                message: format!("selected size instantiation {arguments:?}"),
                            });
                        }
                    }
                    return Err(error);
                }
            }
            for call in summary.calls.iter() {
                if function(call.owning_frame) != current
                    || call.class == CallClass::ComptimeExecuted
                {
                    continue;
                }
                // The binder's owner: the callee's item, or this body's for
                // a local function.
                let (callee, binder, mut actuals) = match call.target_frame {
                    None => {
                        let callee = call.callee.path().clone();
                        if !self.declared(&callee) {
                            continue;
                        }
                        // An image is closed over its own package graph. Without
                        // staged declarations there, its calls need no new walk.

                        (callee, NameSite::Item, BTreeMap::new())
                    }
                    Some(_) => (path.clone(), call.binder.site, bindings.clone()),
                };
                for (ordinal, argument) in call.arguments.iter().enumerate() {
                    if let GenericArgument::Size(size) = argument {
                        let value = match size {
                            Size::Constant(n) => Some(*n),
                            Size::Parameter(parameter) => bindings.get(parameter).copied(),
                        }
                        .ok_or_else(|| operational("selected call has an unbound size"))?;
                        actuals.insert(
                            SizeParameterRef::body(
                                call.binder,
                                hirsel_types::SizeParamId(ordinal as u32),
                            ),
                            value,
                        );
                    }
                }
                pending.push_back((callee, binder, actuals, call.site));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod preparation_tests;

#[cfg(test)]
mod task_tests {
    use super::*;
    use hirsel_interp::aggregate::RuntimeTypes;
    use std::sync::Barrier;

    /// Fail after a view read, before the entry body exists. Evidence must
    /// survive provider errors too, and must not consume an outer recording.
    struct Missing<'a> {
        runtime: &'a RuntimeTypes,
        path: DefPath,
        together: Option<&'a Barrier>,
    }
    impl TirProvider for Missing<'_> {
        fn definition(&mut self, _: &DefPath) -> Result<DefinitionRef, QueryError> {
            self.runtime.serialization.get(&self.path);
            if let Some(together) = self.together {
                together.wait();
            }
            Err(QueryError::NotImplemented)
        }
        fn constant(&mut self, _: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
            Ok(vec![])
        }
    }
    fn task(runtime: &RuntimeTypes, together: Option<&Barrier>, name: &str) -> EvaluationProduct {
        let path = DefPath::core(name);
        let sources = SourceMap::default();
        let ty = runtime.types.intern(hirsel_types::Type::unit());
        EvaluationTask {
            sources: &sources,
            configuration: RunConfiguration {
                entry: path.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            limits: Default::default(),
            runtime,
            site: Span::default(),
            ty,
        }
        .run(&mut Missing {
            runtime,
            path,
            together,
        })
    }
    struct Deferred<'a> {
        runtime: &'a RuntimeTypes,
        path: DefPath,
        ready: std::rc::Rc<std::cell::Cell<bool>>,
        entered: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl hirsel_interp::AsyncTirProvider for Deferred<'_> {
        async fn definition(&mut self, _: &DefPath) -> Result<DefinitionRef, QueryError> {
            self.entered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.runtime.serialization.get(&self.path);
            std::future::poll_fn(|_| {
                if self.ready.get() {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            })
            .await;
            Err(QueryError::NotImplemented)
        }
        async fn constant(
            &mut self,
            _: &DefPath,
        ) -> Result<hirsel_tir::MaterializedRef, QueryError> {
            Err(QueryError::NotImplemented)
        }
        async fn constant_instance(
            &mut self,
            _: &DefPath,
            _: &[u64],
        ) -> Result<hirsel_tir::MaterializedRef, QueryError> {
            Err(QueryError::NotImplemented)
        }
        async fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
            Ok(vec![])
        }
        async fn instance_body(
            &mut self,
            _: &DefPath,
            _: &[GenericArgument],
        ) -> Result<Option<DefPath>, QueryError> {
            Ok(None)
        }
    }
    #[test]
    fn saved_evaluation_keeps_reflection_reads_and_closes_recording_on_cancel() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        for cancel in [false, true] {
            let runtime = RuntimeTypes::new(hirsel_types::TypeInterner::new(None));
            let outer = DefPath::core("outer");
            let inner = DefPath::core("inner");
            let sources = SourceMap::default();
            let ready = std::rc::Rc::new(std::cell::Cell::new(false));
            let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let mut provider = Deferred {
                runtime: &runtime,
                path: inner.clone(),
                ready: ready.clone(),
                entered: entered.clone(),
            };
            runtime.serialization.record();
            runtime.serialization.get(&outer);
            let mut evaluation = Box::pin(
                EvaluationTask {
                    sources: &sources,
                    configuration: RunConfiguration {
                        entry: inner.clone(),
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    limits: Default::default(),
                    runtime: &runtime,
                    site: Span::default(),
                    ty: runtime.types.intern(hirsel_types::Type::unit()),
                }
                .run_async(&mut provider),
            );
            let mut cx = Context::from_waker(Waker::noop());
            assert!(evaluation.as_mut().poll(&mut cx).is_pending());
            assert!(evaluation.as_mut().poll(&mut cx).is_pending());
            assert_eq!(entered.load(std::sync::atomic::Ordering::SeqCst), 1);
            if !cancel {
                ready.set(true);
                let Poll::Ready(product) = evaluation.as_mut().poll(&mut cx) else {
                    panic!("saved dependency did not resume");
                };
                assert!(matches!(product.result, Err(QueryError::NotImplemented)));
                assert_eq!(product.reflected.keys().collect::<Vec<_>>(), [&inner]);
            }
            drop(evaluation);
            let enclosing = runtime.serialization.finish();
            assert_eq!(enclosing.len(), 2);
            assert!(enclosing.contains_key(&outer));
            assert!(enclosing.contains_key(&inner));
            assert!(
                runtime.serialization.finish().is_empty(),
                "pending recording outlived its cancelled task"
            );
        }
    }
    #[test]
    fn failed_evaluation_returns_reads_and_preserves_the_outer_recording() {
        let runtime = RuntimeTypes::new(hirsel_types::TypeInterner::new(None));
        let outer = DefPath::core("outer");
        let inner = DefPath::core("inner");
        runtime.serialization.record();
        runtime.serialization.get(&outer);
        let product = task(&runtime, None, "inner");
        assert!(matches!(product.result, Err(QueryError::NotImplemented)));
        assert_eq!(
            product.reflected.keys().cloned().collect::<Vec<_>>(),
            [inner.clone()]
        );
        assert!(product.limit.is_none());
        let enclosing = runtime.serialization.finish();
        assert_eq!(enclosing.len(), 2);
        assert!(enclosing.contains_key(&outer));
        assert!(enclosing.contains_key(&inner));
        assert!(runtime.serialization.finish().is_empty());
    }
    #[test]
    fn simultaneous_workers_keep_their_reflection_reads_separate() {
        let runtime = RuntimeTypes::new(hirsel_types::TypeInterner::new(None));
        let together = Barrier::new(2);
        let products = std::thread::scope(|scope| {
            let workers: Vec<_> = ["left", "right"]
                .into_iter()
                .map(|name| {
                    let runtime = runtime.clone();
                    let together = &together;
                    scope.spawn(move || (name, task(&runtime, Some(together), name)))
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        for (name, product) in products {
            assert!(matches!(product.result, Err(QueryError::NotImplemented)));
            assert_eq!(
                product.reflected.keys().cloned().collect::<Vec<_>>(),
                [DefPath::core(name)]
            );
        }
        assert!(runtime.serialization.finish().is_empty());
    }
}
