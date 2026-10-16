//! Flow-sensitive ownership judgments. Only declared signatures are queried.
mod cfg;
mod flow;
mod liveness;
mod properties;
#[cfg(test)]
mod tests;

use hirsel_diag::Diagnostic;
use hirsel_intern::work::{self, Counter};
use hirsel_span::Span;
use hirsel_syntax::{ExprId, ItemId, SyntaxTree};
use hirsel_types::*;

/// A binding row is the storage identity; nested parameter ordinals are scoped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Place {
    pub binding: usize,
    pub projections: Vec<Projection>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Projection {
    Field(u32),
    VariantField {
        variant: u32,
        field: u32,
    },
    /// Every index of the same sequence aliases for OWN-3.
    Elements,
}
impl Place {
    pub fn overlaps(&self, other: &Self) -> bool {
        if self.binding != other.binding {
            return false;
        }
        for (a, b) in self.projections.iter().zip(&other.projections) {
            if matches!((a, b), (Projection::VariantField { variant: a, .. }, Projection::VariantField { variant: b, .. }) if a != b)
            {
                return true;
            }
            if a != b {
                return false;
            }
        }
        true
    }
}
/// Refine shared tables in place. Reads, including failed reads, remain in
/// `body.resolution.reads` even when an operational provider error is returned.
/// Recovery/error types suppress dependent ownership judgments (§23).
/// Clause roots already have H's distinct borrowed parameter rows.
/// `results` are the authority pass's formal frame results: they are derived
/// summaries of the same rows as the callable types, and are normalised with them.
pub fn check_ownership_with_clauses(
    tree: &SyntaxTree,
    symbols: &hirsel_intern::Interner,
    item: ItemId,
    signature: &Signature,
    body: &mut CheckedBody,
    captures: &[CaptureMode],
    clauses: &[(ExprId, TypeId)],
    results: &mut [(hirsel_types::NameSite, TypeId)],
    identity_joins: &[(NameSite, NameSite, Origin)],
    identity_fields: &[(NameSite, NameSite, Origin, Vec<(FieldId, TypeId)>)],
    provider: &mut dyn SignatureProvider,
) -> Result<QueryOutput<OwnershipFacts>, QueryError> {
    let rows = source_rows::SourceRows::new(body);
    let arenas = crate::arenas::Arenas::new(body, signature);
    let mut introductions: std::collections::BTreeMap<NameSite, Vec<(NameSite, Origin, usize)>> =
        std::collections::BTreeMap::new();
    for (site, owner, origin) in identity_joins {
        let binding = match origin.root {
            OriginRoot::Local(local) => ResolvedName::Local(local),
            OriginRoot::Parameter(ParameterRef::Body { parameter, .. }) => {
                ResolvedName::Parameter(parameter)
            }
            _ => continue,
        };
        if let Some(ordinal) = rows.binding(body, Some(*owner), &binding) {
            introductions
                .entry(*site)
                .or_default()
                .push((*owner, origin.clone(), ordinal));
        }
    }
    let mut signatures = properties::Signatures::default();
    let mut graphs = cfg::build(
        tree, item, signature, body, captures, clauses, results, &rows,
    )?;
    let projection_lifetimes =
        properties::projection_lifetimes(body, signature, &mut signatures, provider)?;
    let pattern_declarations =
        properties::pattern_declarations(body, signature, &mut signatures, provider)?;
    let field_loan_places =
        properties::field_loan_places(body, signature, &graphs, &mut signatures, provider, &rows)?;
    let mut properties = properties::classify(body, signature, &mut signatures, provider)?;
    let result_sharing =
        properties::result_sharing(body, signature, &graphs, &mut signatures, provider, &rows)?;
    let mut mutation_sites = Vec::new();
    let mut facts = OwnershipFacts {
        captures: captures.to_vec(),
        call_arguments: cfg::call_arguments(body, &properties, &rows),
        ..OwnershipFacts::default()
    };
    let mut diagnostics: Vec<Diagnostic> = vec![];
    let mut replacement_live = std::collections::BTreeMap::new();
    let mut initial_storage = flow::InitialStorage::new(tree, body, &properties);
    // A postcondition observes continuing exits, including modes lost in the body.
    graphs.sort_by_key(|graph| graph.clause.is_some());
    let frame_results: std::collections::BTreeMap<_, _> = results.iter().cloned().collect();
    let capture_crossing_uses =
        flow::capture_crossing_uses(body, &graphs, &rows, signature, &signatures)?;
    for graph in &graphs {
        let checked = flow::check(
            tree,
            symbols,
            signature,
            body,
            &signatures,
            &mut properties,
            &mut initial_storage,
            &rows,
            &result_sharing,
            &projection_lifetimes,
            &pattern_declarations,
            &field_loan_places,
            graph.clone(),
            &capture_crossing_uses,
            &mut facts,
            &mut diagnostics,
            &frame_results,
            &introductions,
        )?;
        initial_storage.postcondition_inputs(
            tree,
            graph.owner,
            body,
            &mut properties,
            &checked.return_uniqueness,
            &checked.return_sendability,
        );
        mutation_sites.extend(checked.mutation_sites);
        for (expression, live) in checked.replacements {
            replacement_live
                .entry(expression)
                .and_modify(|old| {
                    if *old != live {
                        *old = None;
                    }
                })
                .or_insert(live);
        }
    }
    // CLO-1: a closure over a closure local to its environment returns
    // results local to the same environment.
    for expression in initial_storage.environment_frames() {
        if let Some((_, result)) = results
            .iter_mut()
            .find(|(site, _)| *site == hirsel_types::NameSite::Expression(*expression))
        {
            let mut row = body.types.get(*result).unwrap().clone();
            row.modes.relation = arenas.relocate(&row.modes.relation, &Relation::environment());
            properties.push(properties[result.index()]);
            *result = body.types.push(row);
        }
    }
    properties::normalize(
        body,
        signature,
        results,
        &mut facts.physical_projections,
        &mut signatures,
        provider,
    )?;
    let targets = mutation_sites;
    for expression in targets {
        let place = cfg::place(body, expression, &rows);
        // Owned local storage retains its receiving lifetime through fields.
        // Borrowed accesses to caller storage instead describe global content.
        let owned_receiver = place.as_ref().is_some_and(|place| {
            matches!(
                body.bindings[place.binding].usage,
                UseKind::Take | UseKind::Copy
            ) && (place.projections.is_empty()
                || body
                    .types
                    .get(body.bindings[place.binding].ty)
                    .unwrap()
                    .modes
                    .relation
                    .has_environment())
        });
        let root = place
            .filter(|place| place.projections.is_empty())
            .map(|place| body.bindings[place.binding].ty)
            .unwrap_or_else(|| {
                if owned_receiver {
                    if let Some((_, physical)) = facts
                        .physical_projections
                        .iter()
                        .find(|(source, _)| *source == expression)
                    {
                        return *physical;
                    }
                }
                body.expressions
                    .iter()
                    .find(|e| e.expression == expression)
                    .unwrap()
                    .ty
            });
        let mut store = body.types.clone();
        let content = if owned_receiver {
            root
        } else {
            storage_content_type(
                &mut store,
                root,
                &body.resolution.generic_declarations.clone(),
                &mut |path| signatures.read(path, body, provider).map(Some),
            )?
        };
        body.types = store;
        let live = replacement_live[&expression];
        facts.replacements.push((expression, content, live));
    }
    let initialized: std::collections::BTreeSet<_> = std::mem::take(&mut facts.identity_joins)
        .into_iter()
        .collect();
    facts.identity_joins = identity_joins
        .iter()
        .filter(|row| initialized.contains(*row))
        .cloned()
        .collect();
    facts.identity_fields = identity_fields
        .iter()
        .filter(|(site, owner, origin, _)| {
            facts.identity_joins.iter().any(|(at, binding, root)| {
                at == site && binding == owner && root.root == origin.root
            })
        })
        .cloned()
        .collect();
    properties::identity_projections(
        body,
        signature,
        &mut facts.identity_fields,
        &mut signatures,
        provider,
    )?;
    // Flow refines modes without changing Plain. Reuse those classified rows;
    // normalization and replacement storage can append further structural rows.
    let declarations = body.resolution.generic_declarations.clone();
    let store = body.types.clone();
    for (id, row) in store.iter() {
        let mut lookup = |path: &hirsel_syntax::DefPath| {
            if path == &signature.definition {
                Ok(Some(std::sync::Arc::new(signature.clone())))
            } else {
                signatures.read(path, body, provider).map(Some)
            }
        };
        let (plain, copyable) = if let Some(property) = properties.get(id.index()) {
            (property.plain, property.copyable)
        } else {
            (
                structural_property(
                    &store,
                    id,
                    StructuralProperty::Plain,
                    &declarations,
                    &mut lookup,
                )?,
                structural_property(
                    &store,
                    id,
                    StructuralProperty::Copyable,
                    &declarations,
                    &mut lookup,
                )?,
            )
        };
        if plain {
            facts.plain_types.insert(id);
        }
        if copyable && row.modes.multiplicity != Multiplicity::Once {
            facts.silent_drop_types.insert(id);
        }
    }
    diagnostics.sort_by(|a, b| {
        (
            a.primary.start,
            a.primary.end,
            a.rule.prefix(),
            suffix(a.rule.as_str()),
        )
            .cmp(&(
                b.primary.start,
                b.primary.end,
                b.rule.prefix(),
                suffix(b.rule.as_str()),
            ))
    });
    diagnostics.dedup();
    facts.call_arguments.sort_by_key(|r| (r.0, r.1));
    facts.call_arguments.dedup();
    facts.unique.sort_by_key(|d| d.expression);
    facts.unique.dedup();
    facts.guarded_patterns.sort_by_key(|d| d.pattern);
    facts.guarded_patterns.dedup();
    facts.allocating_mutations.sort();
    facts.allocating_mutations.dedup();
    Ok(QueryOutput {
        value: facts,
        diagnostics,
        dependencies: signatures.dependencies(),
    })
}
fn suffix(rule: &str) -> u32 {
    rule.split_once('-').unwrap().1.parse().unwrap()
}

/// Build internal clause headers from independently checked formal rows.
/// Entry clauses keep entry modes; postconditions carry continuing-exit modes.
pub fn checked_clause_types(
    body: &mut CheckedBody,
    clauses: &[(ExprId, TypeId)],
) -> Result<Vec<TypeId>, GraphError> {
    let formals: std::collections::BTreeMap<_, _> = body
        .bindings
        .iter()
        .filter_map(|binding| {
            let ResolvedName::Parameter(parameter) = binding.binding else {
                return None;
            };
            Some(((binding.owner, parameter), binding.ty))
        })
        .collect();
    clauses
        .iter()
        .map(|(expression, original)| {
            let scope = NameSite::Expression(*expression);
            let mut row = body.types.get(*original).unwrap().clone();
            let TypeKind::Function(function) = &mut row.kind else {
                return Ok(*original);
            };
            let mut parameters = function.parameters.to_vec();
            for (ordinal, parameter) in parameters.iter_mut().enumerate() {
                if let Some(&ty) = formals.get(&(scope, ParamId(ordinal as u32))) {
                    parameter.ty = ty;
                }
            }
            function.parameters = parameters.into();
            let row = body.types.push(row);
            abstract_parameter_scope(&mut body.types, row, scope)
        })
        .collect()
}

mod source_rows;
