//! Compile-time execution uses the runtime interpreter; lowering receives plain values.
use crate::*;
use hirsel_interp::Outcome;
use hirsel_span::Span;
use hirsel_syntax::{ExprId, ExprKind};
use hirsel_tir::{Constant, DefinitionRef, TirProvider};
use hirsel_tir::{Materialized, ValueKind};
use hirsel_types::{ResolvedName, TypeId};
use std::sync::Arc;

/// A constant evaluation and the substitutions used to lower its source initializer.
#[derive(Clone, Debug)]
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
struct Evaluation<'a> {
    root: DefinitionRef,
    engine: &'a mut QueryEngine,
}
impl TirProvider for Evaluation<'_> {
    fn constant(&mut self, path: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        self.engine.materialize_value(path).map(Arc::new)
    }
    // CT-1 evaluates only the explicitly requested dependency closure.
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
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
                        hirsel_diag::RuleId::new("CT-4").unwrap(),
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
        if self.adapters.compiler_state.is_none() {
            return Ok(());
        }
        if matches!(signature.kind, hirsel_types::SignatureKind::Const(_)) {
            analyzed.materialized = self.evaluate_constant(&signature.definition)?.substitutions;
            Ok(())
        } else {
            analyzed.materialized = self.prepare_values(analyzed, signature)?;
            Ok(())
        }
    }
    fn prepare_values(
        &mut self,
        analyzed: &AnalyzedBody,
        signature: &Signature,
    ) -> Result<Vec<(ExprId, Materialized)>, QueryError> {
        let file = &analyzed.typed.file;
        let checked = &analyzed.typed.checked;
        let mut constants = vec![];
        for (expression, name) in &checked.resolution.expressions {
            if let ResolvedName::Definition(path) = name {
                if matches!(
                    self.item_signature(path)?.value.kind,
                    hirsel_types::SignatureKind::Const(_)
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
        self.adapters
            .compiler_state
            .as_ref()
            .ok_or(QueryError::NotImplemented)?;
        let file = &analyzed.typed.file;
        let checked = &analyzed.typed.checked;
        let lowering = self
            .adapters
            .lowering
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let mut definition =
            lowering.lower_evaluation(analyzed, signature, child, ty, constants)?;
        // This private initializer is a zero-input artifact, not the enclosing
        // definition's exported body. Its result is H/J's checked expression type.
        definition.path.0.name =
            format!("$comptime:{}:{}", signature.definition.0.name, expression.0);
        let mut artifact = signature.clone();
        artifact.definition = definition.path.clone();
        artifact.types = checked.types.clone();
        artifact.kind = hirsel_types::SignatureKind::Const(ty);
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
            Outcome::Returned { value, .. } => self.reify_value(
                &signature.definition.0.package,
                value,
                &checked.types,
                ty,
                file.tree.expressions[expression.0 as usize].origin.span,
            ),
            Outcome::Failed(failure) => {
                let mut diagnostic = Diagnostic::error(
                    hirsel_diag::RuleId::new("CT-1").unwrap(),
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
                    hirsel_diag::RuleId::new("CT-1").unwrap(),
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
