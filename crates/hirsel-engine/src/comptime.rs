//! Compile-time execution uses the runtime interpreter; lowering receives plain values.
use crate::memo::{DepthEvidence, Entered};
use crate::*;
use hirsel_interp::Outcome;
use hirsel_span::Span;
use hirsel_syntax::{DefId, ExprId, ExprKind};
use hirsel_tir::{Constant, DefinitionRef, TirProvider};
use hirsel_tir::{Materialized, ValueKind};
use hirsel_types::{
    GenericArgument, GenericBinder, NameSite, ResolvedName, SignatureKind, Size, SizeParameterRef,
    TypeId,
};
use std::sync::Arc;

/// A constant evaluation and the substitutions used to lower its source initializer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct EvaluatedConstant {
    pub value: Materialized,
    pub substitutions: Vec<(ExprId, Materialized)>,
    /// The complete reads of everything the evaluation executed or materialized.
    pub executed: crate::memo::Reads,
    /// Its dependency height and overflow table under the comptime limits.
    pub depth: DepthEvidence<Span>,
}
fn operational(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "compile-time evaluation".into(),
        message: message.into(),
    })
}
#[derive(Clone, Debug)]
pub(crate) struct StagedConstant {
    pub owner: DefPath,
    pub expression: ExprId,
    pub inputs: Vec<SizeParameterRef>,
    pub signature: hirsel_types::SignatureRef,
}
pub(crate) fn has_staged_reads(definition: &hirsel_tir::Definition) -> bool {
    definition.bodies.iter().flat_map(|body| &body.rvalues).any(|value| {
        matches!(value, hirsel_tir::Rvalue::ReadConstant { arguments, .. } if !arguments.is_empty())
    })
}
struct Evaluation<'a> {
    root: DefinitionRef,
    engine: &'a mut QueryEngine,
}
impl TirProvider for Evaluation<'_> {
    fn constant(&mut self, path: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        self.engine.materialize_value(path).map(Arc::new)
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        self.engine.constant_instance(path, arguments)
    }
    // CT-1 evaluates only the explicitly requested dependency closure.
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.engine.resolve_instance(path, arguments)
    }

    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.engine.read_implementation(path);
        if path == self.root.path.path() {
            Ok(self.root.clone())
        } else {
            let definition = self.engine.definition(path)?;
            let signature = self.engine.item_signature(path)?.value;
            let entry = match &self.engine.adapters.lowering {
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
        }
    }
}
impl QueryEngine {
    /// The views `constant`'s retained evaluation reflected or decoded
    /// (`Representation(d)`), each with the digest it saw.
    pub fn representation_reads(&self, constant: &DefPath) -> Option<Vec<(DefPath, ContentHash)>> {
        let state = self.adapters.compiler_state.as_ref()?.borrow();
        let value = state.constant_values.get(constant)?;
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
        if let Some(value) = self.image_constant(path) {
            return Ok(value);
        }
        if self.s1.is_empty() {
            self.last_comptime_limit = None;
        }
        if let Some(value) = self.registry_dictionary_value(path)? {
            return Ok(EvaluatedConstant {
                value,
                substitutions: vec![],
                executed: Default::default(),
                depth: DepthEvidence::leaf(self.depth_limit()),
            });
        }
        let syntax = self.parse_definition(path)?.value;
        self.constant_at(path, syntax.item.name_span)
    }
    fn constant_at(
        &mut self,
        path: &DefPath,
        reference: Span,
    ) -> Result<EvaluatedConstant, QueryError> {
        if let Some(value) = self.image_constant(path) {
            self.depth_admit(&value.depth, reference)?;
            self.merge_execution(&value.executed);
            self.depth_requested(&value.depth, reference);
            return Ok(value);
        }
        if let Some(value) = self.registry_dictionary_value(path)? {
            return Ok(EvaluatedConstant {
                value,
                substitutions: vec![],
                executed: Default::default(),
                depth: DepthEvidence::leaf(self.depth_limit()),
            });
        }
        let state = self
            .adapters
            .compiler_state
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let cached = state.borrow().constant_values.get(path).cloned();
        if let Some(value) = cached {
            if self.execution_valid(path, &value.executed) {
                self.depth_admit(&value.depth, reference)?;
                self.merge_execution(&value.executed);
                self.depth_requested(&value.depth, reference);
                return Ok(value);
            }
            state.borrow_mut().constant_values.remove(path);
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
        if self.s1.enter(path.clone(), reference) == Entered::Settled {
            // Another worker finished it: its success is in the memo, or this
            // stack evaluates it again.
            return self.constant_at(path, reference);
        }
        self.execution_stack.push(Default::default());
        self.read_implementation(path);
        let result = (|| {
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
            let constants = self.prepare_values(body, &[], &signature)?;
            let constant = if let Some((_, constant)) = constants.iter().find(|(e, _)| *e == value)
            {
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
            })
        })();
        self.execution_stack.pop();
        if let Ok(evaluated) = &result {
            state
                .borrow_mut()
                .constant_values
                .insert(path.clone(), evaluated.clone());
        }
        self.s1.leave();
        if let Ok(evaluated) = &result {
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
        if self.adapters.compiler_state.is_none() {
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
        let file = &analyzed.typed.syntax;
        let checked = &analyzed.typed.checked;
        let mut constants = vec![];
        self.register_staged(analyzed)?;
        for (expression, name) in &checked.resolution.expressions {
            if let ResolvedName::Definition(path) = name {
                if matches!(
                    self.item_signature(path)?.value.kind,
                    hirsel_types::SignatureKind::Const { ty: _, .. }
                ) {
                    let mut value = self
                        .constant_at(
                            path,
                            file.tree.expressions[expression.0 as usize].origin.span,
                        )?
                        .value;
                    let ty = checked
                        .expressions
                        .iter()
                        .find(|r| r.expression == *expression)
                        .ok_or_else(|| operational("missing constant reference type"))?
                        .ty;
                    // One session interner: the reference has the checked type.
                    value.root.ty = ty;
                    constants.push((*expression, value));
                }
            }
        }
        let mut expressions: Vec<_> = checked
            .expressions
            .iter()
            .filter_map(|row| {
                if let ExprKind::Comptime(child) =
                    file.tree.expressions[row.expression.0 as usize].kind
                {
                    Some((row.expression, child, row.ty))
                } else {
                    None
                }
            })
            .collect();
        expressions.sort_by_key(|(id, _, _)| {
            let s = file.tree.expressions[id.0 as usize].origin.span;
            (s.end - s.start, s.start)
        });
        for (expression, child, ty) in expressions {
            if !hirsel_check_protocol::body::comptime_inputs(&self.types, file, checked, expression)
                .is_empty()
            {
                continue;
            }
            let value = self.evaluate_value(
                analyzed, staged, signature, expression, child, ty, &constants,
            )?;
            constants.push((expression, value));
        }
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
            let valid = match &self.types.row(row.ty).kind {
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
        Ok(constants)
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
        self.adapters
            .compiler_state
            .as_ref()
            .ok_or(QueryError::NotImplemented)?;
        let file = &analyzed.typed.syntax;
        let lowering = self
            .adapters
            .lowering
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let mut definition = lowering.lower_evaluation(
            &self.types,
            &self.memo,
            analyzed,
            staged,
            signature,
            child,
            ty,
            constants,
        )?;
        // This private initializer is a zero-input artifact, not the enclosing
        // definition's exported body. Its result is H/J's checked expression type.
        definition.path = DefId::intern(&hirsel_check_protocol::body::comptime_path(
            signature.definition.path(),
            &analyzed.typed.checked,
            expression,
        ));
        specialize_sizes(&self.types, &self.memo, &mut definition, sizes)?;
        let ty = definition.bodies[definition.entry.0 as usize].result;
        let mut artifact = signature.clone();
        artifact.definition = definition.path;
        artifact.metadata = hirsel_types::SignatureMetadata {
            roles: signature.metadata.roles.clone(),
            unsafe_ffi: signature.metadata.unsafe_ffi,
            ..Default::default()
        };
        artifact.kind = hirsel_types::SignatureKind::Const {
            ty: ty,
            generics: Default::default(),
        };
        self.validate_comptime(&definition, artifact)?;
        let sources = self.sources.clone();
        let configuration = RunConfiguration {
            entry: definition.path.path().clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let limits = self.comptime_limits;
        let runtime = self.comptime_runtime_types()?;
        let mut library = hirsel_interp::library::Library::new(&configuration, Default::default())
            .map_err(|e| operational(format!("{e:?}")))?;
        runtime.serialization.record();
        let execution = hirsel_interp::run_with_runtime(
            &mut Evaluation {
                root: Arc::new(definition),
                engine: self,
            },
            &sources,
            &configuration,
            vec![],
            limits,
            &mut library,
            &runtime,
        );
        let reflected = runtime.serialization.finish();
        self.read_representations(&reflected);
        let execution = execution.map_err(|e| match e.error {
            hirsel_interp::Error::Query(e) => e,
            other => operational(format!("{other:?}")),
        })?;
        match execution.outcome {
            Outcome::Returned { value, .. } => Ok((value, ty)),
            Outcome::Failed(failure) => {
                let mut diagnostic = Diagnostic::error(
                    const { hirsel_diag::RuleId::of("CT-1") },
                    file.tree.expressions[expression.0 as usize].origin.span,
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
            // CT-1 and docs/spec/diagnostics.md own exhausted evaluation steps, including the
            // configured budget until #7 specifies source-event accounting.
            Outcome::ToolingLimit(limit) if limit.kind == hirsel_tir::ToolingLimitKind::Steps => {
                let mut diagnostic = Diagnostic::error(
                    const { hirsel_diag::RuleId::of("CT-1") },
                    file.tree.expressions[expression.0 as usize].origin.span,
                    "compile-time evaluation exhausted its step limit",
                );
                if let Some(site) = limit.site {
                    diagnostic.secondary.push(hirsel_diag::Label {
                        span: site,
                        message: "evaluation stopped here".into(),
                    });
                }
                diagnostic
                    .notes
                    .push(format!("evaluation step limit: {} (spec #7)", limit.limit));
                self.last_comptime_limit = Some(limit);
                Err(QueryError::Diagnostics(vec![diagnostic]))
            }
            Outcome::Blocked(_) => Err(operational("compile-time evaluation is Blocked")),
            Outcome::BudgetExceeded(kind) => Err(operational(format!(
                "compile-time evaluation exceeded its {} budget",
                kind.name()
            ))),
            Outcome::ToolingLimit(limit) => Err(self.comptime_limit(limit)),
            Outcome::UndefinedBehavior(detected) => Err(operational(detected.to_string())),
            Outcome::Trapped(trap) => Err(operational(trap.to_string())),
            Outcome::Environmental(cause) => {
                Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "codec-budget".into(),
                    message: cause.to_string(),
                }))
            }
        }
    }
    pub(crate) fn comptime_limit(&mut self, limit: hirsel_tir::ToolingLimit) -> QueryError {
        self.last_comptime_limit = Some(limit.clone());
        QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "compile-time tooling limit".into(),
            message: format!("{limit:?}"),
        })
    }
}

impl QueryEngine {
    /// The shared reception type of each hidden size-generic constant in `analyzed`.
    fn prepare_staged_types(
        &mut self,
        analyzed: &AnalyzedBody,
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
                    &self.types,
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
            let types = self.types.clone();
            let mut lookup = |d: DefId| self.signature(d).map(Some);
            let plain = hirsel_types::structural_property(
                &types,
                ty,
                hirsel_types::StructuralProperty::Plain,
                generics,
                &mut lookup,
            )?;
            let flat = hirsel_types::structural_flat(&types, ty, generics, &mut lookup)?;
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
                    hirsel_types::sendability_after_sharing(&types, root, generics, &mut lookup)?;
                types.intern(row)
            };
            staged.push((expression, shared));
        }
        Ok(staged)
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
            let owner = typed.syntax.item.definition.clone();
            let path =
                hirsel_check_protocol::body::comptime_path(&owner, &typed.checked, row.expression);
            let mut signature = self.item_signature(&owner)?.value.as_ref().clone();
            signature.definition = DefId::intern(&path);
            signature.metadata = hirsel_types::SignatureMetadata {
                roles: signature.metadata.roles.clone(),
                unsafe_ffi: signature.metadata.unsafe_ffi,
                ..Default::default()
            };
            let replacements: Vec<_> = inputs
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    (
                        *p,
                        Size::Parameter(SizeParameterRef::bound(hirsel_types::SizeParamId(
                            i as u32,
                        ))),
                    )
                })
                .collect();
            let ty = substitute_sizes_type(&self.types, &self.memo, row.ty, &replacements)?;
            signature.kind = SignatureKind::Const {
                ty,
                generics: inputs
                    .iter()
                    .enumerate()
                    .map(|(i, _)| hirsel_types::GenericParameter::Size {
                        name: format!("size{i}"),
                    })
                    .collect::<Vec<_>>()
                    .into(),
            };
            signature.fingerprint =
                hirsel_types::encoding::fingerprint(&self.types, &signature, &[])
                    .map_err(|e| operational(format!("staged signature: {e:?}")))?;
            if self.staged_constants.get(&path).is_some_and(|old| {
                old.signature.fingerprint == signature.fingerprint && old.inputs == inputs
            }) {
                continue;
            }
            self.retire_snapshot();
            self.staged_constants.insert(
                path,
                StagedConstant {
                    owner,
                    expression: row.expression,
                    inputs,
                    signature: Arc::new(signature),
                },
            );
        }
        Ok(())
    }
    pub(crate) fn materialize_staged(
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
        if constant.inputs.len() != arguments.len() {
            return Err(operational("staged constant arity"));
        }
        let site = self
            .parse_definition(&constant.owner)?
            .value
            .tree
            .expressions[constant.expression.0 as usize]
            .origin
            .span;
        if let Some(value) = cached {
            if self.execution_valid(&constant.owner, &value.executed) {
                self.depth_admit(&value.depth, site)?;
                self.merge_execution(&value.executed);
                self.depth_requested(&value.depth, site);
                return Ok(value);
            }
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
        self.read_implementation(&constant.owner);
        self.read_execution_signatures(&body.dependencies);
        let result = (|| {
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
            })
        })();
        self.execution_stack.pop();
        if let Ok(value) = &result {
            self.staged_values.insert(key, value.clone());
        }
        self.s1.leave();
        if let Ok(value) = &result {
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
    /// Select staged constants from checked call actuals, after signatures and
    /// GEN-5, reading each body's definition summary. Local generic frames
    /// retain their enclosing lexical size bindings. Calls inside comptime are
    /// selected by execution, preserving dead branches.
    pub(crate) fn prepare_staged_instances(&mut self) -> Result<(), QueryError> {
        use std::collections::{BTreeMap, BTreeSet, VecDeque};
        if self.staged_constants.is_empty() {
            return Ok(());
        }
        let bodies = self.instantiation_bodies()?;
        let mut pending = VecDeque::new();
        for (path, body) in &bodies {
            let signature = self.item_signature(path)?.value;
            if signature.generics().is_empty()
                && !matches!(signature.kind, SignatureKind::Const { .. })
            {
                let scope = NameSite::Item;
                pending.push_back((
                    path.clone(),
                    scope,
                    BTreeMap::<SizeParameterRef, u64>::new(),
                    crate::summary::source_site(&body.value.syntax, scope),
                ));
            }
        }
        let mut seen = BTreeSet::new();
        while let Some((path, scope, bindings, site)) = pending.pop_front() {
            if !seen.insert((path.clone(), scope, bindings.clone())) {
                continue;
            }
            if seen.len() > 65536 {
                return Err(operational("compile-time instance expansion limit"));
            }
            let analyzed = self.mode_query(&path)?;
            self.register_staged(&analyzed.value)?;
            let (summary, _) = crate::summary::summarize(self.types(), &analyzed.value.typed);
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
                        if self.imaged(&callee.0.package)
                            && !self
                                .staged_constants
                                .values()
                                .any(|constant| constant.owner.0.package == callee.0.package)
                        {
                            continue;
                        }
                        self.mode_query(&callee)?;
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
