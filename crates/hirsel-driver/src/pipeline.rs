//! The engine's front phases over signature, check, modes and lower.
//! Conversions retain producer decisions; missing facts are compiler errors.
use hirsel_check::body;
use hirsel_check_protocol::header::{Input, NormalizedDeclaration};
use hirsel_engine::*;
use hirsel_lower as lower;
use hirsel_syntax::ItemSyntax;
use hirsel_types::{NameSite, Resolution, Signature, TypeId, TypeKind};
use std::sync::Arc;

/// The compiler's front phases, installed by [`crate::COMPILER`].
pub struct FrontPhases;
impl FrontPhases {
    pub fn shared() -> Arc<dyn Phases> {
        Arc::new(FrontPhases)
    }
}
impl Phases for FrontPhases {
    fn normalize_signature(
        &self,
        input: Input<'_>,
        provider: &mut dyn SignatureProvider,
    ) -> Result<NormalizedDeclaration, QueryError> {
        hirsel_signature::normalize_owned(input, provider)
    }
    fn validate_loop_source(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        signature: &Signature,
        span: hirsel_span::Span,
        provider: &mut dyn SignatureProvider,
    ) -> Result<(), QueryError> {
        hirsel_signature::protocol::validate_loop_source(t, m, signature, span, provider)
    }
    fn validate_format(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        signature: &Signature,
        span: hirsel_span::Span,
        provider: &mut dyn SignatureProvider,
    ) -> Result<(), QueryError> {
        hirsel_signature::protocol::validate_format(t, m, signature, span, provider)
    }
    fn validate_serializable_check(
        &self,
        t: &hirsel_types::TypeInterner,
        _: &hirsel_types::TypeMemo,
        signature: &Signature,
        span: hirsel_span::Span,
        provider: &mut dyn SignatureProvider,
    ) -> Result<(), QueryError> {
        hirsel_signature::protocol::validate_serializable_check(t, signature, span, provider)
    }
    fn checker(&self) -> Arc<dyn CheckerPhase> {
        Arc::new(BodyAdapter)
    }
    fn modes(&self) -> Arc<dyn ModesPhase> {
        Arc::new(ModeAdapter)
    }
    fn lowering(&self) -> Arc<dyn LoweringPhase> {
        Arc::new(LowerAdapter)
    }
}

fn invariant(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "phase conversion".into(),
        message: message.into(),
    })
}
pub struct BodyAdapter;
impl CheckerPhase for BodyAdapter {
    fn match_implementation(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        family: &DefPath,
        actuals: &hirsel_types::ImplementationSignature,
        provider: &mut dyn SignatureProvider,
    ) -> Result<Option<String>, QueryError> {
        Ok(
            hirsel_check::implementation::match_implementation(t, m, family, actuals, provider)?
                .err()
                .map(|r| format!("member `{}`: [{}] {}", r.member, r.rule, r.message)),
        )
    }
    fn check(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        resolution: Resolution,
        signature: &Signature,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        let definition = signature.definition.path();
        let context = FileContext {
            package: definition.0.package.clone(),
            modules: definition.0.modules.clone(),
        };
        let protocols = hirsel_types::core_roles::loop_protocols();
        let output = body::check_with_loop_protocols(
            body::LoopInput {
                body: body::Input {
                    types: t,
                    memo: m,
                    syntax: &syntax,
                    context: &context,
                    resolution: &resolution,
                    signature,
                },
                protocols: &protocols,
            },
            provider,
        )?;
        // Lowering's native control rows: the interned unit and Bool.
        let unit = t.intern(hirsel_types::Type::unit());
        let boolean = t.intern(hirsel_types::Type::boolean());
        Ok(QueryOutput {
            value: TypedBody {
                syntax,
                checked: output.checked.value,
                plans: output.plans,
                unit,
                boolean,
            },
            diagnostics: output.checked.diagnostics,
            dependencies: output.checked.dependencies,
        })
    }
}
#[path = "private_unique.rs"]
mod private_unique;
pub struct ModeAdapter;
impl ModesPhase for ModeAdapter {
    fn infers_private_unique(&self) -> bool {
        true
    }
    fn infer_unique(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        bodies: &[(Arc<TypedBody>, hirsel_types::SignatureRef)],
        provider: &mut dyn SignatureProvider,
    ) -> Result<Vec<(DefId, hirsel_types::UniqueInference)>, QueryError> {
        private_unique::infer(self, t, m, bodies, provider)
    }

    fn storage(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        signature: &Signature,
        signatures: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<hirsel_types::storage::graph::StorageDefinition>, QueryError> {
        let clauses: Vec<_> = body
            .typed
            .plans
            .contracts
            .iter()
            .map(|clause| (clause.expression, clause.function))
            .collect();
        hirsel_ownership::storage_definition(
            t,
            m,
            &body.typed.syntax,
            signature,
            &body.typed.checked,
            &body.ownership.captures,
            &clauses,
            &body.results,
            signatures,
        )
    }
    fn check_modes(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        typed: Arc<TypedBody>,
        signature: &Signature,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
        let mut typed = Arc::unwrap_or_clone(typed);
        let syntax = typed.syntax.clone();
        let intrinsics = hirsel_types::core_roles::intrinsic_roles()
            .into_iter()
            .collect();
        let loops: Vec<_> = typed
            .plans
            .loops
            .iter()
            .filter_map(|l| l.cursor().cloned())
            .collect();
        let clauses: Vec<_> = typed
            .plans
            .contracts
            .iter()
            .map(|c| (c.expression, c.function))
            .collect();
        let mut output = hirsel_authority::check_file_with_clauses(
            t,
            m,
            &syntax,
            signature,
            typed.checked,
            provider,
            &intrinsics,
            &loops,
            &clauses,
        );
        products.mode_diagnostics.extend(
            output
                .diagnostics
                .iter()
                .cloned()
                .map(|d| (d, "authority".into())),
        );
        let captures = output.capture_modes();
        // J has formal function rows; H owns the declared constant/finalizer result.
        if !matches!(signature.kind, hirsel_types::SignatureKind::Function(_)) {
            output.result_types.extend(
                typed
                    .plans
                    .functions
                    .iter()
                    .filter(|f| f.owner == NameSite::Item)
                    .map(|f| (f.owner.clone(), f.result)),
            );
        }
        // Run I even when J reports source errors: availability can own the obligation.
        let owned = hirsel_ownership::check_ownership_with_clauses(
            t,
            m,
            &syntax,
            signature,
            &mut output.body,
            &captures,
            &clauses,
            &mut output.result_types,
            &output.identity_joins,
            &output.identity_fields,
            provider,
        );
        if let Some(error) = output.failure {
            return Err(error);
        }
        let owned = match owned {
            Ok(value) => value,
            Err(QueryError::Diagnostics(ds)) => {
                products
                    .mode_diagnostics
                    .extend(ds.iter().cloned().map(|d| (d, "ownership".into())));
                output.diagnostics.extend(ds);
                return Err(QueryError::Diagnostics(select_diagnostics(
                    output.diagnostics,
                    None,
                )));
            }
            Err(error) => return Err(error),
        };
        let clause_types = hirsel_ownership::checked_clause_types(t, &mut output.body, &clauses)
            .map_err(|error| {
                invariant(&format!("checked clause parameter transport: {error:?}"))
            })?;
        for (clause, function) in typed.plans.contracts.iter_mut().zip(clause_types) {
            clause.function = function;
        }
        // Rebuild implicit calls from declarations after I has judged the
        // source's current content. J's selected source identity stays fixed.
        if !output
            .diagnostics
            .iter()
            .any(|d| d.severity == hirsel_diag::Severity::Error)
        {
            for plan in &mut typed.plans.loops {
                let hirsel_check_protocol::body::ForPlanKind::Cursor { cursor, .. } =
                    &mut plan.kind
                else {
                    continue;
                };
                let identity = output
                    .cursor_identities
                    .get(&cursor.source)
                    .ok_or_else(|| invariant("missing cursor source identity"))?;
                output
                    .dependencies
                    .extend(hirsel_authority::refine_cursor_plan(
                        t,
                        m,
                        &mut output.body,
                        signature,
                        cursor,
                        identity,
                        provider,
                    )?);
            }
        }
        products.mode_diagnostics.extend(
            owned
                .diagnostics
                .iter()
                .cloned()
                .map(|d| (d, "ownership".into())),
        );
        output.diagnostics.extend(owned.diagnostics);
        output.dependencies.extend(owned.dependencies);
        let allocating_copies: Vec<_> = owned
            .value
            .unique
            .iter()
            .filter(|r| r.action != hirsel_types::UniqueAction::AlreadyUnique)
            .map(|r| r.expression)
            .collect();

        let audited = hirsel_check::audit::check(
            hirsel_check::audit::Input {
                types: t,
                memo: m,
                syntax: &syntax,
                signature,
                checked: &output.body,
                loops: &typed.plans.loops,
                call_allocations: &output.call_allocations,
                allocating_copies: &allocating_copies,
                allocating_mutations: &owned.value.allocating_mutations,
            },
            provider,
        )?;
        products.mode_diagnostics.extend(
            audited
                .diagnostics
                .iter()
                .cloned()
                .map(|d| (d, "allocation and representation audit".into())),
        );
        output.diagnostics.extend(audited.diagnostics);
        output.dependencies.extend(audited.dependencies);
        typed.checked = output.body;
        Ok(QueryOutput {
            value: AnalyzedBody {
                typed: Arc::new(typed),
                ownership: owned.value,
                results: output.result_types,
                call_allocations: output.call_allocations,
            },
            diagnostics: select_diagnostics(output.diagnostics, None),
            dependencies: output.dependencies,
        })
    }
}
pub struct LowerAdapter;
impl LoweringPhase for LowerAdapter {
    fn lower(
        &self,
        t: &hirsel_types::TypeInterner,
        _: &hirsel_types::TypeMemo,
        analyzed: &AnalyzedBody,
        comptime: &Comptime,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        let source = &analyzed.typed.syntax.item;
        let expression = match &source.kind {
            hirsel_syntax::ItemKind::Function { body: Some(e), .. } => *e,
            hirsel_syntax::ItemKind::Const { value, .. } => *value,
            hirsel_syntax::ItemKind::Type { drop: Some(d), .. } => d.body,
            _ => {
                return Err(invariant(format!(
                    "{:?}: no executable body",
                    signature.definition
                )));
            }
        };
        let mut value = lower_expression(
            t,
            analyzed,
            &comptime.staged,
            signature,
            expression,
            None,
            &comptime.materialized,
        )?;
        value.inferred_unique = signature.metadata.inferred_unique.clone();
        Ok(QueryOutput {
            value,
            diagnostics: vec![],
            dependencies: vec![],
        })
    }
    fn lower_evaluation(
        &self,
        t: &hirsel_types::TypeInterner,
        _: &hirsel_types::TypeMemo,
        analyzed: &AnalyzedBody,
        staged: &[(hirsel_syntax::ExprId, TypeId)],
        signature: &Signature,
        expression: hirsel_syntax::ExprId,
        result: TypeId,
        constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
    ) -> Result<hirsel_tir::Definition, QueryError> {
        lower_expression(
            t,
            analyzed,
            staged,
            signature,
            expression,
            Some(result),
            constants,
        )
    }
    fn entry_body(&self) -> hirsel_tir::BodyId {
        lower::entry_body()
    }
}

fn lower_expression(
    t: &hirsel_types::TypeInterner,
    analyzed: &AnalyzedBody,
    staged: &[(hirsel_syntax::ExprId, TypeId)],
    signature: &Signature,
    expression: hirsel_syntax::ExprId,
    evaluation_result: Option<TypeId>,
    constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
) -> Result<hirsel_tir::Definition, QueryError> {
    let typed = &analyzed.typed;
    let file = &*typed.syntax;
    let checked = &typed.checked;
    let owner = NameSite::Item;
    let result = evaluation_result.unwrap_or(
        analyzed
            .results
            .iter()
            .find(|(o, _)| *o == owner)
            .map(|(_, t)| *t)
            .ok_or_else(|| invariant("missing formal result type"))?,
    );
    let mut results = analyzed.results.clone();
    if evaluation_result.is_some() {
        for (site, ty) in &mut results {
            if *site == owner {
                *ty = result;
            }
        }
    }
    let mut plans = lower::Plans::from_body(t, &file.tree, checked, &typed.plans, expression)
        .map_err(|e| invariant(e.to_string()))?;
    plans.staged = staged.to_vec();
    let owner_span = |owner| match owner {
        NameSite::Item => Some(file.item.origin.span),
        NameSite::Statement(id) => Some(file.tree.statements[id.0 as usize].origin.span),
        NameSite::Expression(id) => Some(file.tree.expressions[id.0 as usize].origin.span),
        _ => None,
    };
    if evaluation_result.is_some() {
        plans.evaluation_frame = true;
        let frame = file.tree.expressions[expression.0 as usize].origin.span;
        // A synthetic initializer evaluates this expression alone. Enclosing
        // and unrelated declared contracts remain on their ordinary bodies.
        plans.contracts.retain(|clause| {
            owner_span(clause.owner)
                .is_some_and(|owner| owner.start >= frame.start && owner.end <= frame.end)
        });
    }
    // A materialized subtree no longer emits its nested callable bodies. Their
    // clauses were checked and executed during evaluation, not by its reader.
    let mut materialized: Vec<_> = constants
        .iter()
        .map(|(id, _)| file.tree.expressions[id.0 as usize].origin.span)
        .collect();
    materialized.sort_by_key(|span| span.start);
    let mut ranges: Vec<hirsel_span::Span> = vec![];
    for span in materialized {
        if let Some(previous) = ranges
            .last_mut()
            .filter(|previous| previous.end >= span.start)
        {
            previous.end = previous.end.max(span.end);
        } else {
            ranges.push(span);
        }
    }
    plans.contracts.retain(|clause| {
        let Some(owner) = owner_span(clause.owner) else {
            return false;
        };
        let index = ranges.partition_point(|range| range.start <= owner.start);
        index == 0 || ranges[index - 1].end < owner.end
    });
    for (expression, value) in constants {
        if let hirsel_tir::ValueKind::Constant(constant) = &value.root.kind {
            plans.constants.push((*expression, constant.clone()));
        } else {
            plans.materialized.push((*expression, value.clone()));
        }
    }
    lower::lower(lower::Input {
        types: t,
        track_caller: evaluation_result.is_none() && signature.metadata.track_caller,
        syntax: file,
        checked: &checked,
        plans: &plans,
        ownership: &analyzed.ownership,
        result_types: &results,
        owner,
        expression,
        path: signature.definition.path().clone(),
        signature_hash: signature.fingerprint,
        unit: typed.unit,
        boolean: typed.boolean,
        arena: signature
            .metadata
            .roles
            .heap
            .as_ref()
            .map(|h| h.arena.clone()),
    })
    .map_err(|e| invariant(format!("{:?}: {e}", signature.definition)))
}
