//! Compile-time execution uses the runtime interpreter; lowering receives plain values.
use crate::*;
use hirsel_interp::Outcome;
use hirsel_span::Span;
use hirsel_syntax::{ExprId, ExprKind};
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
    pub executed: crate::queries::ExecutionReads,
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
        types: &hirsel_types::TypeStore,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.engine.resolve_instance(path, types, arguments)
    }

    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.engine.read_implementation(path);
        if *path == self.root.path {
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
        if let Some(value) = self.prechecked_constant(path)? {
            return Ok(value);
        }
        if self.comptime_stack.is_empty() {
            self.last_comptime_limit = None;
        }
        if let Some(value) = self.registry_dictionary_value(path)? {
            return Ok(EvaluatedConstant {
                value,
                substitutions: vec![],
                executed: Default::default(),
            });
        }
        let (file, item) = self.parse_definition(path)?.value;
        self.constant_at(path, file.tree.items[item.0 as usize].name_span)
    }
    fn constant_at(
        &mut self,
        path: &DefPath,
        reference: Span,
    ) -> Result<EvaluatedConstant, QueryError> {
        if let Some(value) = self.prechecked_constant(path)? {
            return Ok(value);
        }
        if let Some(value) = self.registry_dictionary_value(path)? {
            return Ok(EvaluatedConstant {
                value,
                substitutions: vec![],
                executed: Default::default(),
            });
        }
        let state = self
            .adapters
            .compiler_state
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let cached = state.borrow().constant_values.get(path).cloned();
        if let Some(value) = cached {
            if self.executions_valid(&value.executed) {
                self.inherit_execution(&value.executed);
                return Ok(value);
            }
            state.borrow_mut().constant_values.remove(path);
        }
        if let Some(start) = self.comptime_stack.iter().position(|(p, _)| p == path) {
            let mut diagnostics: Vec<_> = self.comptime_stack[start + 1..]
                .iter()
                .map(|(_, s)| *s)
                .chain([reference])
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
        let dependency_depth = self.comptime_limits.call_depth.min(32);
        if self.comptime_stack.len() >= dependency_depth {
            let limit = hirsel_tir::ToolingLimit {
                kind: hirsel_tir::ToolingLimitKind::CallDepth,
                limit: dependency_depth as u64,
                site: Some(reference),
            };
            return Err(self.comptime_limit(limit));
        }
        self.comptime_stack.push((path.clone(), reference));
        self.execution_stack.push(Default::default());
        self.read_implementation(path);
        let result = (|| {
            let (file, item) = self.parse_definition(path)?.value;
            if !matches!(
                file.tree.items[item.0 as usize].kind,
                hirsel_syntax::ItemKind::Const { .. }
            ) {
                return Err(operational("materialization requested for a non-constant"));
            }
            let signature = self.item_signature(path)?.value;
            let body = self.mode_query(path)?;
            self.read_execution_signatures(&body.dependencies);
            let body = &body.value;
            let hirsel_syntax::ItemKind::Const { value, .. } =
                body.typed.file.tree.items[body.typed.item.0 as usize].kind
            else {
                return Err(operational("constant artifact has a non-constant owner"));
            };
            let constants = self.prepare_values(body, &signature)?;
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
                self.evaluate_value(body, &signature, value, value, ty, &constants)?
            };
            let evaluated = EvaluatedConstant {
                value: constant,
                substitutions: constants,
                executed: self
                    .execution_stack
                    .last()
                    .expect("constant execution frame")
                    .clone(),
            };
            state
                .borrow_mut()
                .constant_values
                .insert(path.clone(), evaluated.clone());
            Ok(evaluated)
        })();
        self.comptime_stack.pop();
        self.execution_stack.pop();
        result
    }
    pub(crate) fn prepare_comptime(
        &mut self,
        analyzed: &mut AnalyzedBody,
        signature: &Signature,
    ) -> Result<(), QueryError> {
        self.prepare_materialization_environment();
        self.prepare_staged_types(analyzed)?;
        if self.adapters.compiler_state.is_none() {
            return Ok(());
        }
        if matches!(
            signature.kind,
            hirsel_types::SignatureKind::Const { ty: _, .. }
        ) {
            analyzed.materialized = self.evaluate_constant(&signature.definition)?.substitutions;
            Ok(())
        } else {
            analyzed.materialized = self.prepare_values(analyzed, signature)?;
            Ok(())
        }
    }
    pub(crate) fn prepare_values(
        &mut self,
        analyzed: &AnalyzedBody,
        signature: &Signature,
    ) -> Result<Vec<(ExprId, Materialized)>, QueryError> {
        let file = &analyzed.typed.file;
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
                    value.root.ty = hirsel_types::import_type(&checked.types, ty, &mut value.types)
                        .map_err(|e| operational(format!("constant reference import: {e:?}")))?;
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
            if !hirsel_check_protocol::body::comptime_inputs(&file.tree, checked, expression)
                .is_empty()
            {
                continue;
            }
            let value =
                self.evaluate_value(analyzed, signature, expression, child, ty, &constants)?;
            constants.push((expression, value));
        }
        Ok(constants)
    }

    fn evaluate_value(
        &mut self,
        analyzed: &AnalyzedBody,
        signature: &Signature,
        expression: ExprId,
        child: ExprId,
        ty: TypeId,
        constants: &[(ExprId, Materialized)],
    ) -> Result<Materialized, QueryError> {
        self.evaluate_sized_value(analyzed, signature, expression, child, ty, constants, &[])
    }
    fn evaluate_sized_value(
        &mut self,
        analyzed: &AnalyzedBody,
        signature: &Signature,
        expression: ExprId,
        child: ExprId,
        ty: TypeId,
        constants: &[(ExprId, Materialized)],
        sizes: &[(SizeParameterRef, u64)],
    ) -> Result<Materialized, QueryError> {
        let (value, types, ty) =
            self.run_evaluation(analyzed, signature, expression, child, ty, constants, sizes)?;
        self.reify_value(
            &signature.definition.0.package,
            value,
            &types,
            ty,
            analyzed.typed.file.tree.expressions[expression.0 as usize]
                .origin
                .span,
        )
    }
    /// Run one checked compile-time expression to its interpreter value, with
    /// its TIR type store and result type; evaluation failures are CT-1.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_evaluation(
        &mut self,
        analyzed: &AnalyzedBody,
        signature: &Signature,
        expression: ExprId,
        child: ExprId,
        ty: TypeId,
        constants: &[(ExprId, Materialized)],
        sizes: &[(SizeParameterRef, u64)],
    ) -> Result<(hirsel_interp::Value, hirsel_types::TypeStore, TypeId), QueryError> {
        self.adapters
            .compiler_state
            .as_ref()
            .ok_or(QueryError::NotImplemented)?;
        let file = &analyzed.typed.file;
        let lowering = self
            .adapters
            .lowering
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let mut definition =
            lowering.lower_evaluation(analyzed, signature, child, ty, constants)?;
        // This private initializer is a zero-input artifact, not the enclosing
        // definition's exported body. Its result is H/J's checked expression type.
        definition.path = hirsel_check_protocol::body::comptime_path(
            &signature.definition,
            &analyzed.typed.checked,
            expression,
        );
        specialize_sizes(&mut definition, sizes)?;
        let ty = definition.bodies[definition.entry.0 as usize].result;
        let types = definition.types.clone();
        let mut artifact = signature.clone();
        artifact.definition = definition.path.clone();
        artifact.metadata = hirsel_types::SignatureMetadata {
            roles: signature.metadata.roles.clone(),
            unsafe_ffi: signature.metadata.unsafe_ffi,
            ..Default::default()
        };
        artifact.types = types.clone();
        artifact.kind = hirsel_types::SignatureKind::Const {
            ty: ty,
            generics: Default::default(),
        };
        self.validate_comptime(&definition, artifact)?;
        let sources = self.sources.clone();
        let configuration = RunConfiguration {
            entry: definition.path.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let limits = self.comptime_limits;
        let runtime = self.comptime_runtime_types()?;
        let mut library = hirsel_interp::library::Library::new(&configuration, Default::default())
            .map_err(|e| operational(format!("{e:?}")))?;
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
        )
        .map_err(|e| match e.error {
            hirsel_interp::Error::Query(e) => e,
            other => operational(format!("{other:?}")),
        })?;
        match execution.outcome {
            Outcome::Returned { value, .. } => Ok((value, types, ty)),
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
            Outcome::ToolingLimit(limit) => Err(self.comptime_limit(limit)),
            Outcome::UndefinedBehavior(detected) => Err(operational(detected.to_string())),
            Outcome::Trapped(trap) => Err(operational(trap.to_string())),
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
    fn prepare_staged_types(&mut self, analyzed: &mut AnalyzedBody) -> Result<(), QueryError> {
        let checked = &mut analyzed.typed.checked;
        analyzed.staged.clear();
        let expressions: Vec<_> = checked
            .expressions
            .iter()
            .filter(|row| {
                matches!(
                    analyzed.typed.file.tree.expressions[row.expression.0 as usize].kind,
                    ExprKind::Comptime(_)
                ) && !hirsel_check_protocol::body::comptime_inputs(
                    &analyzed.typed.file.tree,
                    checked,
                    row.expression,
                )
                .is_empty()
            })
            .map(|row| (row.expression, row.ty))
            .collect();
        for (expression, ty) in expressions {
            let generics = &checked.resolution.generic_declarations;
            let mut lookup = |path: &DefPath| self.signature(path).map(Some);
            let plain = hirsel_types::structural_property(
                &checked.types,
                ty,
                hirsel_types::StructuralProperty::Plain,
                generics,
                &mut lookup,
            )?;
            let flat = hirsel_types::structural_flat(&checked.types, ty, generics, &mut lookup)?;
            let protected = matches!(
                checked.types.get(ty).unwrap().kind,
                hirsel_types::TypeKind::Builtin {
                    kind: hirsel_types::BuiltinType::Shared,
                    ..
                }
            );
            let shared = if plain || flat || protected {
                ty
            } else {
                let mut row = checked.types.get(ty).unwrap().clone();
                row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
                let root = checked.types.push(row.clone());
                row.modes.sendability = hirsel_types::sendability_after_sharing(
                    &checked.types,
                    root,
                    generics,
                    &mut lookup,
                )?;
                checked.types.push(row)
            };
            analyzed.staged.push((expression, shared));
        }
        Ok(())
    }
    pub(crate) fn register_staged(&mut self, analyzed: &AnalyzedBody) -> Result<(), QueryError> {
        let typed = &analyzed.typed;
        for row in &typed.checked.expressions {
            if !matches!(
                typed.file.tree.expressions[row.expression.0 as usize].kind,
                ExprKind::Comptime(_)
            ) {
                continue;
            }
            let inputs = hirsel_check_protocol::body::comptime_inputs(
                &typed.file.tree,
                &typed.checked,
                row.expression,
            );
            if inputs.is_empty() {
                continue;
            }
            let owner = typed.file.tree.items[typed.item.0 as usize]
                .definition
                .clone();
            let path =
                hirsel_check_protocol::body::comptime_path(&owner, &typed.checked, row.expression);
            let mut signature = self.item_signature(&owner)?.value.as_ref().clone();
            signature.definition = path.clone();
            signature.metadata = hirsel_types::SignatureMetadata {
                roles: signature.metadata.roles.clone(),
                unsafe_ffi: signature.metadata.unsafe_ffi,
                ..Default::default()
            };
            signature.types = typed.checked.types.clone();
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
            let ty = substitute_sizes_type(&mut signature.types, row.ty, &replacements)?;
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
            signature.fingerprint = hirsel_types::encoding::fingerprint(&signature, &[])
                .map_err(|e| operational(format!("staged signature: {e:?}")))?;
            if self.staged_constants.get(&path).is_some_and(|old| {
                old.signature.fingerprint == signature.fingerprint && old.inputs == inputs
            }) {
                continue;
            }
            self.invalidate_signature_input();
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
        if self.comptime_stack.is_empty() {
            self.last_comptime_limit = None;
        }
        let key = (path.clone(), arguments.to_vec());
        if let Some(value) = self.staged_values.get(&key).cloned() {
            if self.executions_valid(&value.executed) {
                self.inherit_execution(&value.executed);
                return Ok(value);
            }
        }
        let constant = self
            .staged_constants
            .get(path)
            .cloned()
            .ok_or_else(|| operational("missing staged constant declaration"))?;
        if constant.inputs.len() != arguments.len() {
            return Err(operational("staged constant arity"));
        }
        let mut body = (*self.mode_query(&constant.owner)?).clone();
        self.prepare_staged_types(&mut body.value)?;
        let file = &body.value.typed.file;
        let site = file.tree.expressions[constant.expression.0 as usize]
            .origin
            .span;
        let instance = hirsel_tir::constant_instance_path(path, arguments);
        if self.comptime_stack.iter().any(|(p, _)| *p == instance) {
            return Err(QueryError::Diagnostics(vec![Diagnostic::error(
                const { hirsel_diag::RuleId::of("CT-4") },
                site,
                "compile-time dependency cycle",
            )]));
        }
        if self.comptime_stack.len() >= self.comptime_limits.call_depth.min(32) {
            return Err(self.comptime_limit(hirsel_tir::ToolingLimit {
                kind: hirsel_tir::ToolingLimitKind::CallDepth,
                limit: self.comptime_limits.call_depth.min(32) as u64,
                site: Some(site),
            }));
        }
        self.comptime_stack.push((instance, site));
        self.execution_stack.push(Default::default());
        self.read_implementation(&constant.owner);
        self.read_execution_signatures(&body.dependencies);
        let result = (|| {
            let signature = self.item_signature(&constant.owner)?.value;
            let constants = self.prepare_values(&body.value, &signature)?;
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
                executed: self.execution_stack.last().unwrap().clone(),
            })
        })();
        self.comptime_stack.pop();
        let executed = self.execution_stack.pop().unwrap();
        self.inherit_execution(&executed);
        if let Ok(value) = &result {
            self.staged_values.insert(key, value.clone());
        }
        result
    }
}

fn substitute_sizes_type(
    store: &mut hirsel_types::TypeStore,
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
        ty = hirsel_types::substitute_type_in_place(
            store,
            ty,
            hirsel_types::Substitution {
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
    let mut memo = std::collections::BTreeMap::new();
    let mut ty = |root: &mut TypeId| -> Result<(), QueryError> {
        *root = match memo.get(root) {
            Some(mapped) => *mapped,
            None => {
                let mapped = substitute_sizes_type(&mut definition.types, *root, &replacements)?;
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
    // Keep one memo for every rooted graph transported into the private store.
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
    Ok(())
}

fn source_site(tree: &hirsel_syntax::SyntaxTree, site: NameSite) -> Span {
    match site {
        NameSite::Item(id) => tree.items[id.0 as usize].origin.span,
        NameSite::Statement(id) => tree.statements[id.0 as usize].origin.span,
        NameSite::Expression(id) => tree.expressions[id.0 as usize].origin.span,
        NameSite::Pattern(id) => tree.patterns[id.0 as usize].origin.span,
        NameSite::Type(id) => tree.types[id.0 as usize].origin.span,
    }
}

impl QueryEngine {
    /// Select staged constants from checked call actuals, after signatures and
    /// GEN-5. Local generic frames retain their enclosing lexical size bindings.
    /// Calls inside comptime are selected by execution, preserving dead branches.
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
                let scope = NameSite::Item(body.value.item);
                pending.push_back((
                    path.clone(),
                    scope,
                    BTreeMap::<SizeParameterRef, u64>::new(),
                    source_site(&body.value.file.tree, scope),
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
            let typed = &analyzed.value.typed;
            self.register_staged(&analyzed.value)?;
            let tree = &typed.file.tree;
            let checked = &typed.checked;
            let frames: BTreeSet<_> = typed
                .plans
                .functions
                .iter()
                .map(|frame| frame.owner)
                .chain([NameSite::Item(typed.item)])
                .collect();
            let contains =
                |outer: Span, inner: Span| outer.start <= inner.start && outer.end >= inner.end;
            let frame_at = |at: Span, exclude: Option<NameSite>| {
                frames
                    .iter()
                    .copied()
                    .filter(|frame| {
                        Some(*frame) != exclude && contains(source_site(tree, *frame), at)
                    })
                    .min_by_key(|frame| {
                        let span = source_site(tree, *frame);
                        span.end - span.start
                    })
            };
            let staged_regions: Vec<_> = checked
                .expressions
                .iter()
                .filter(|row| {
                    matches!(
                        tree.expressions[row.expression.0 as usize].kind,
                        ExprKind::Comptime(_)
                    )
                })
                .map(|row| source_site(tree, NameSite::Expression(row.expression)))
                .collect();
            let staged_at = |at| staged_regions.iter().any(|region| contains(*region, at));
            for frame in &frames {
                if *frame == scope {
                    continue;
                }
                let at = source_site(tree, *frame);
                if frame_at(at, Some(*frame)) == Some(scope)
                    && !staged_at(at)
                    && !checked
                        .resolution
                        .generics
                        .iter()
                        .any(|generic| generic.owner == *frame)
                {
                    pending.push_back((path.clone(), *frame, bindings.clone(), site));
                }
            }
            let constants: Vec<_> = self
                .staged_constants
                .iter()
                .filter(|(_, constant)| {
                    constant.owner == path
                        && frame_at(
                            source_site(tree, NameSite::Expression(constant.expression)),
                            None,
                        ) == Some(scope)
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
            let mut calls: Vec<_> = checked
                .instantiations
                .iter()
                .map(|call| {
                    (
                        call.expression,
                        hirsel_types::CallTarget::Definition(call.callee.clone()),
                        call.arguments.clone(),
                    )
                })
                .collect();
            calls.extend(checked.calls.iter().map(|call| {
                (
                    call.expression,
                    call.target.clone(),
                    call.generic_arguments.clone(),
                )
            }));
            for (expression, target, arguments) in calls {
                let at = source_site(tree, NameSite::Expression(expression));
                if frame_at(at, None) != Some(scope) || staged_at(at) {
                    continue;
                }
                let (callee, binder, mut actuals) = match target {
                    hirsel_types::CallTarget::Definition(callee) => {
                        if self.definition_items(&callee).is_err() {
                            continue;
                        }
                        // A precheck is closed over its own package graph. Without
                        // staged declarations there, its calls need no new walk.
                        if self.prechecked(&callee.0.package)
                            && !self
                                .staged_constants
                                .values()
                                .any(|constant| constant.owner.0.package == callee.0.package)
                        {
                            continue;
                        }
                        let body = self.mode_query(&callee)?;
                        (
                            callee,
                            NameSite::Item(body.value.typed.item),
                            BTreeMap::new(),
                        )
                    }
                    hirsel_types::CallTarget::Value(value) => {
                        let local = checked
                            .resolution
                            .expressions
                            .iter()
                            .find_map(|(id, name)| {
                                if *id == value {
                                    if let ResolvedName::Local(id) = name {
                                        Some(*id)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });
                        let binder = local
                            .and_then(|local| {
                                checked
                                    .resolution
                                    .local_functions
                                    .iter()
                                    .find(|(_, id)| *id == local)
                                    .map(|(statement, _)| NameSite::Statement(*statement))
                            })
                            .or_else(|| {
                                frames
                                    .contains(&NameSite::Expression(value))
                                    .then_some(NameSite::Expression(value))
                            });
                        let Some(binder) = binder else { continue };
                        (path.clone(), binder, bindings.clone())
                    }
                    _ => continue,
                };
                for (ordinal, argument) in arguments.iter().enumerate() {
                    if let GenericArgument::Size(size) = argument {
                        let value = match size {
                            Size::Constant(n) => Some(*n),
                            Size::Parameter(parameter) => bindings.get(parameter).copied(),
                        }
                        .ok_or_else(|| operational("selected call has an unbound size"))?;
                        actuals.insert(
                            SizeParameterRef::body(
                                binder,
                                hirsel_types::SizeParamId(ordinal as u32),
                            ),
                            value,
                        );
                    }
                }
                pending.push_back((callee, binder, actuals, at));
            }
        }
        Ok(())
    }
}
