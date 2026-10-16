//! The engine's front phases over signature, check, modes and lower.
//! Conversions retain producer decisions; missing facts are compiler errors.
use hirsel_check::body;
use hirsel_check_protocol::header::{Input, NormalizedDeclaration};
use hirsel_engine::*;
use hirsel_lower as lower;
use hirsel_modes::{authority, ownership};
use hirsel_syntax::{ItemId, SyntaxFile};
use hirsel_types::{NameSite, Resolution, Signature, TypeId, TypeKind};
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// The compiler's front phases, installed by [`crate::COMPILER`].
pub struct FrontPhases;
impl FrontPhases {
    pub fn shared() -> Rc<dyn Phases> {
        Rc::new(FrontPhases)
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
        signature: &Signature,
        span: hirsel_span::Span,
        provider: &mut dyn SignatureProvider,
    ) -> Result<(), QueryError> {
        hirsel_signature::protocol::validate_loop_source(signature, span, provider)
    }
    fn checker(&self) -> Rc<dyn CheckerPhase> {
        Rc::new(BodyAdapter)
    }
    fn modes(&self, state: Rc<RefCell<CompilerState>>) -> Rc<dyn ModesPhase> {
        Rc::new(ModeAdapter { state })
    }
    fn lowering(&self) -> Rc<dyn LoweringPhase> {
        Rc::new(LowerAdapter)
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
    fn check(
        &self,
        file: Arc<SyntaxFile>,
        item: ItemId,
        resolution: Resolution,
        signature: &Signature,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<TypedBody>, QueryError> {
        let context = FileContext {
            package: signature.definition.0.package.clone(),
            modules: signature.definition.0.modules.clone(),
        };
        let protocols = hirsel_types::core_roles::loop_protocols();
        let mut output = body::check_with_loop_protocols(
            body::LoopInput {
                body: body::Input {
                    tree: &file.tree,
                    item,
                    symbols: &file.symbols,
                    context: &context,
                    resolution: &resolution,
                    signature,
                },
                protocols: &protocols,
            },
            provider,
        )?;
        // Retain H's first-native-row selection, including finalizer unit rows.
        let unit = output.checked.value.types.iter().find_map(|(id, ty)| {
            matches!(&ty.kind, TypeKind::Tuple(fs) if fs.is_empty()).then_some(id)
        });
        let unit =
            unit.unwrap_or_else(|| output.checked.value.types.push(hirsel_types::Type::unit()));
        let boolean = output.checked.value.types.iter().find_map(|(id, ty)| {
            (ty.kind == TypeKind::Primitive(hirsel_types::Primitive::Bool)).then_some(id)
        });
        let boolean = boolean.unwrap_or_else(|| {
            output
                .checked
                .value
                .types
                .push(hirsel_types::Type::boolean())
        });
        Ok(QueryOutput {
            value: TypedBody {
                file,
                item,
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
pub struct ModeAdapter {
    pub state: Rc<RefCell<CompilerState>>,
}
impl ModesPhase for ModeAdapter {
    fn check_modes(
        &self,
        mut typed: TypedBody,
        signature: &Signature,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
        let file = typed.file.clone();
        let item = typed.item;
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
        let mut output = authority::check_file_with_clauses(
            &file,
            item,
            signature,
            typed.checked,
            provider,
            &intrinsics,
            &loops,
            &clauses,
        );
        self.state.borrow_mut().mode_diagnostics.extend(
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
                    .filter(|f| f.owner == NameSite::Item(item))
                    .map(|f| (f.owner.clone(), f.result)),
            );
        }
        // Run I even when J reports source errors: availability can own the obligation.
        let owned = ownership::check_ownership_with_clauses(
            &file.tree,
            &file.symbols,
            item,
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
                self.state
                    .borrow_mut()
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
        let clause_types =
            ownership::checked_clause_types(&mut output.body, &clauses).map_err(|error| {
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
                output.dependencies.extend(authority::refine_cursor_plan(
                    &mut output.body,
                    signature,
                    cursor,
                    identity,
                    provider,
                )?);
            }
        }
        self.state.borrow_mut().mode_diagnostics.extend(
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
                tree: &file.tree,
                symbols: &file.symbols,
                item,
                signature,
                checked: &output.body,
                loops: &typed.plans.loops,
                call_allocations: &output.call_allocations,
                allocating_copies: &allocating_copies,
                allocating_mutations: &owned.value.allocating_mutations,
            },
            provider,
        )?;
        self.state.borrow_mut().mode_diagnostics.extend(
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
                typed,
                ownership: owned.value,
                results: output.result_types,
                call_allocations: output.call_allocations,
                materialized: vec![],
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
        analyzed: AnalyzedBody,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        let file = &analyzed.typed.file;
        let item = analyzed.typed.item;
        let source = &file.tree.items[item.0 as usize];
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
        let value = lower_expression(
            &analyzed,
            signature,
            expression,
            None,
            &analyzed.materialized,
        )?;
        Ok(QueryOutput {
            value,
            diagnostics: vec![],
            dependencies: vec![],
        })
    }
    fn lower_evaluation(
        &self,
        analyzed: &AnalyzedBody,
        signature: &Signature,
        expression: hirsel_syntax::ExprId,
        result: TypeId,
        constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
    ) -> Result<hirsel_tir::Definition, QueryError> {
        lower_expression(analyzed, signature, expression, Some(result), constants)
    }
    fn entry_body(&self) -> hirsel_tir::BodyId {
        lower::entry_body()
    }
}

fn lower_expression(
    analyzed: &AnalyzedBody,
    signature: &Signature,
    expression: hirsel_syntax::ExprId,
    evaluation_result: Option<TypeId>,
    constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
) -> Result<hirsel_tir::Definition, QueryError> {
    let typed = &analyzed.typed;
    let file = &typed.file;
    let item = typed.item;
    let checked = &typed.checked;
    let owner = NameSite::Item(item);
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
    let mut plans = lower::Plans::from_body(&file.tree, &checked, &typed.plans, expression)
        .map_err(|e| invariant(e.to_string()))?;
    let owner_span = |owner| match owner {
        NameSite::Item(id) => Some(file.tree.items[id.0 as usize].origin.span),
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
        tree: &file.tree,
        checked: &checked,
        plans: &plans,
        ownership: &analyzed.ownership,
        result_types: &results,
        owner,
        expression,
        path: signature.definition.clone(),
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
