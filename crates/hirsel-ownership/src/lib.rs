//! Flow-sensitive ownership judgments. Only declared signatures are queried.

#[cfg(test)]
hirsel_guard::install!();

mod arenas;
mod cfg;
mod flow;
mod liveness;
mod properties;
mod storage;
#[cfg(test)]
mod tests;

use hirsel_diag::Diagnostic;
use hirsel_intern::work::{self, Counter};
use hirsel_span::Span;
use hirsel_syntax::DefId;
use hirsel_syntax::{ExprId, ItemSyntax};
use hirsel_types::*;
use std::sync::Arc;

/// A binding row is the storage identity; nested parameter ordinals are scoped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Place {
    pub binding: usize,
    pub projections: Vec<Projection>,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Projection {
    Field(u32),
    VariantField {
        variant: u32,
        field: u32,
    },
    /// Every index of the same sequence aliases for OWN-3.
    Elements,
}

/// Build a demanded compact product from an already checked body. No callee
/// body is read here; program scheduling resolves all helper products later.
pub fn storage_definition(
    t: &TypeInterner,
    m: &TypeMemo,
    syntax: &ItemSyntax,
    signature: &Signature,
    checked: &CheckedBody,
    captures: &[CaptureMode],
    clauses: &[(ExprId, TypeId)],
    results: &[(NameSite, TypeId)],
    provider: &mut dyn SignatureProvider,
) -> Result<QueryOutput<hirsel_types::storage::graph::StorageDefinition>, QueryError> {
    let mut body = checked.clone();
    let rows = source_rows::SourceRows::new(&body);
    let graphs = cfg::build(
        t, syntax, signature, &mut body, captures, clauses, results, &rows,
    )?;
    let generics = body.resolution.generic_declarations.clone();
    let reader = hirsel_modes::reader::Reader::new(provider, signature);
    let mut read = |definition| reader.borrow_mut().read(definition).map(Some);
    let mut flow_read = |definition| reader.borrow_mut().read(definition).map(Some);
    let memo = ContextMemo::new(t, &generics, &mut read);
    let flow_memo = ContextMemo::new(t, &generics, &mut flow_read);
    let mut signatures = properties::Signatures::new(m, &reader, memo, flow_memo);
    let properties =
        properties::classify(t, &mut body, cfg::graph_types(&graphs), &mut signatures)?;
    let value = storage::derive(
        t,
        syntax,
        signature,
        &mut body,
        &graphs,
        &rows,
        &properties,
        &mut signatures,
    )?;
    Ok(QueryOutput {
        value,
        diagnostics: vec![],
        dependencies: signatures.dependencies(),
    })
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
/// Recovery/error types suppress dependent ownership judgments (docs/spec/diagnostics.md).
/// Clause roots already have H's distinct borrowed parameter rows.
/// `results` are the authority pass's formal frame results: they are derived
/// summaries of the same rows as the callable types, and are normalised with them.
pub fn check_ownership_with_clauses(
    t: &TypeInterner,
    m: &TypeMemo,
    syntax: &ItemSyntax,
    signature: &Signature,
    body: &mut CheckedBody,
    captures: &[CaptureMode],
    clauses: &[(ExprId, TypeId)],
    results: &mut [(hirsel_types::NameSite, TypeId)],
    identity_joins: &[(NameSite, NameSite, Origin)],
    identity_fields: &[(NameSite, NameSite, Origin, Vec<(FieldId, TypeId)>)],
    provider: &mut dyn SignatureProvider,
) -> Result<QueryOutput<OwnershipFacts>, QueryError> {
    let tree = &syntax.tree;
    let rows = source_rows::SourceRows::new(body);
    let arenas = crate::arenas::Arenas::new(t, body, signature);
    let mut introductions: std::collections::BTreeMap<NameSite, Vec<(NameSite, Origin, usize)>> =
        std::collections::BTreeMap::new();
    for (site, owner, origin) in identity_joins {
        let binding = match origin.root {
            OriginRoot::Local(_, local) => ResolvedName::Local(local),
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
    // Both proof memos read through one recording Reader. Flow refinements
    // can need a nested declaration that classification never reached.
    let generics = body.resolution.generic_declarations.clone();
    let reader = hirsel_modes::reader::Reader::new(&mut *provider, signature);
    let mut read = |d: DefId| reader.borrow_mut().read(d).map(Some);
    let mut flow_read = |d: DefId| reader.borrow_mut().read(d).map(Some);
    let memo = ContextMemo::new(t, &generics, &mut read);
    let flow_memo = ContextMemo::new(t, &generics, &mut flow_read);
    let mut signatures = properties::Signatures::new(m, &reader, memo, flow_memo);
    let mut graphs = cfg::build(
        t, syntax, signature, body, captures, clauses, results, &rows,
    )?;
    // Direct-call evidence lives in the declaration metadata, independently
    // of the callable row. Read these headers even for structurally plain calls.
    let direct: Vec<_> = body
        .resolution
        .expressions
        .iter()
        .filter_map(|(_, name)| match name {
            ResolvedName::Definition(path) => Some(DefId::intern(path)),
            _ => None,
        })
        .collect();
    for definition in direct {
        signatures.read(definition, Arc::make_mut(&mut body.resolution.reads))?;
    }
    let projection_lifetimes =
        properties::projection_lifetimes(t, body, signature, &mut signatures)?;
    let pattern_declarations = properties::pattern_declarations(t, body, &mut signatures)?;
    let field_loan_places =
        properties::field_loan_places(t, body, &graphs, &mut signatures, &rows)?;
    // The rows the body's store held when main classified it: the tables'
    // rows and those the declaration-layout passes above derived.
    let derived = projection_lifetimes
        .values()
        .map(|p| p.physical)
        .chain(pattern_declarations.values().copied())
        .chain(field_loan_places.values().map(|p| p.declared))
        .chain(cfg::graph_types(&graphs))
        .chain(results.iter().map(|(_, ty)| *ty))
        .chain(clauses.iter().map(|(_, ty)| *ty))
        .chain(
            identity_fields
                .iter()
                .flat_map(|(_, _, _, parts)| parts.iter().map(|(_, ty)| *ty)),
        )
        .collect::<Vec<_>>();
    let mut properties = properties::classify(t, body, derived, &mut signatures)?;
    // MODE-2: an error mapper's parameter mode is checked on the failure edge.
    for graph in &graphs {
        for node in &graph.nodes {
            let cfg::Op::MapError { expression, .. } = node.op else {
                continue;
            };
            let mapper = body
                .resolution
                .paths
                .iter()
                .find_map(|(site, name)| match name {
                    ResolvedName::Definition(path) if *site == NameSite::Expression(expression) => {
                        Some(path.clone())
                    }
                    _ => None,
                });
            // J reports an unreadable mapper; this read only feeds the check.
            if let Some(path) = mapper {
                let _ = signatures.read(
                    DefId::intern(&path),
                    Arc::make_mut(&mut body.resolution.reads),
                );
            }
        }
    }
    // MODE-1: the caller-side `mut` content bound asks which storage can hold
    // a part of which content; read the nominal representations it needs.
    let mut held = Vec::new();
    for graph in &graphs {
        for node in &graph.nodes {
            let cfg::Op::Call {
                expression,
                arguments,
                requirements,
                ..
            } = &node.op
            else {
                continue;
            };
            if requirements
                .iter()
                .flatten()
                .any(|r| r.passing == hirsel_syntax::PassingMode::Mut)
            {
                held.extend(
                    arguments
                        .iter()
                        .map(|a| a.value)
                        .chain([*expression])
                        .filter_map(|e| cfg::expression_type(body, e, &rows)),
                );
            }
        }
    }
    if !held.is_empty() {
        let mut seen = std::collections::HashSet::new();
        for ty in held {
            if !seen.insert(ty) {
                continue;
            }
            hirsel_type_analysis::representation::may_hold_content(t, m, ty, ty, &mut |path| {
                signatures
                    .read(path, Arc::make_mut(&mut body.resolution.reads))
                    .ok()
            });
        }
    }
    let result_sharing = properties::result_sharing(t, body, &graphs, &mut signatures, &rows)?;
    let mut mutation_sites = Vec::new();
    let mut facts = OwnershipFacts {
        physical_patterns: pattern_declarations
            .iter()
            .map(|(&p, &ty)| (p, ty))
            .collect(),
        captures: captures.to_vec(),
        call_arguments: cfg::call_arguments(body, &properties, &rows),
        ..OwnershipFacts::default()
    };
    let mut diagnostics: Vec<Diagnostic> = vec![];
    let mut replacement_live = std::collections::BTreeMap::new();
    let mut initial_storage = flow::InitialStorage::new(t, tree, body, &properties);
    // A postcondition observes continuing exits, including modes lost in the body.
    graphs.sort_by_key(|graph| graph.clause.is_some());
    let frame_results: std::collections::BTreeMap<_, _> = results.iter().cloned().collect();
    let capture_crossing_uses =
        flow::capture_crossing_uses(t, body, &graphs, &rows, signature, &signatures)?;
    // MODE-2: a `@preserves_unique` body is checked again with its selected
    // parameter and its taken and `mut` inputs entering unique; only whether
    // the selected parameter leaves unique is kept from that run.
    let association = match &signature.kind {
        SignatureKind::Function(f) => {
            properties::format_association(t, f, signature.definition.path(), &signatures)
        }
        _ => None,
    };
    let mut promised: Vec<ParamId> = match &signature.kind {
        SignatureKind::Function(f) => f
            .state_promises
            .iter()
            .filter(|p| p.preserve_unique && p.selector.fields.is_empty())
            .filter_map(|p| match p.selector.parameter {
                ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } => Some(parameter),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if association.is_some() && !promised.contains(&ParamId(1)) {
        promised.push(ParamId(1));
    }
    let declared_promises = promised.clone();
    let infer = syntax.item.visibility == hirsel_syntax::Visibility::Private
        && !syntax
            .item
            .attributes
            .iter()
            .any(|a| a.name.as_str() == "test")
        && matches!(
            &syntax.item.kind,
            hirsel_syntax::ItemKind::Function { body: Some(_), .. }
        )
        && matches!(&signature.kind, SignatureKind::Function(f) if f.foreign.is_none());
    if infer && let SignatureKind::Function(f) = &signature.kind {
        for (i, p) in f.parameters.iter().enumerate() {
            let id = ParamId(i as u32);
            if p.passing == PassingMode::Mut && !promised.contains(&id) {
                promised.push(id);
            }
        }
    }
    for parameter in promised {
        for graph in graphs
            .iter()
            .filter(|g| g.owner == NameSite::Item && g.clause.is_none())
        {
            let mut proof_body = body.clone();
            let mut proof_properties = properties.clone();
            let mut proof_storage = initial_storage.clone();
            let mut proof_facts = facts.clone();
            let mut proof_diagnostics = Vec::new();
            let checked = flow::check(
                t,
                syntax,
                signature,
                &mut proof_body,
                &signatures,
                &mut proof_properties,
                &mut proof_storage,
                &rows,
                &result_sharing,
                &projection_lifetimes,
                &pattern_declarations,
                &field_loan_places,
                graph.clone(),
                &capture_crossing_uses,
                &mut proof_facts,
                &mut proof_diagnostics,
                &frame_results,
                &introductions,
                Some(parameter),
            );
            signatures.record(Arc::make_mut(&mut body.resolution.reads));
            checked?;
            if infer
                && let Some(d) = proof_diagnostics
                    .iter()
                    .find(|d| d.message.starts_with(flow::UNIQUE_PROOF_FAILED))
            {
                let cause = d
                    .secondary
                    .iter()
                    .find(|l| l.message.starts_with("provided:") && l.span != d.primary);
                facts.unique_failures.push(UniqueFailure {
                    claim: UniqueClaim::Parameter(parameter),
                    span: cause.map_or(d.primary, |l| l.span),
                    message: cause.map_or_else(|| d.message.clone(), |l| l.message.clone()),
                });
            }
            if infer
                && !proof_diagnostics
                    .iter()
                    .any(|d| d.message.starts_with(flow::UNIQUE_PROOF_FAILED))
            {
                facts.inferred_unique.preserves_unique.push(parameter);
            }
            diagnostics.extend(
                proof_diagnostics
                    .into_iter()
                    .filter(|d| declared_promises.contains(&parameter) && d.message.starts_with(flow::UNIQUE_PROOF_FAILED))
                    .map(|mut d| {
                        if let Some(nominal) = &association {
                            d.primary = syntax.item.name_span;
                            d.notes.push(format!(
                                "the @format association on {nominal} implies @preserves_unique on the output builder"
                            ));
                        }
                        d
                    }),
            );
        }
    }
    for graph in &graphs {
        let checked = flow::check(
            t,
            syntax,
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
            None,
        );
        signatures.record(Arc::make_mut(&mut body.resolution.reads));
        let checked = checked?;
        initial_storage.postcondition_inputs(
            t,
            syntax,
            graph.owner,
            body,
            &mut properties,
            &checked.return_uniqueness,
            &checked.return_sendability,
        );
        if infer && graph.owner == NameSite::Item && graph.clause.is_none() {
            facts.inferred_unique.results = checked.unique_results;
            facts.unique_failures.extend(checked.unique_failures);
        }
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
            let mut row = t.get(*result).unwrap().clone();
            row.modes.relation = arenas.relocate(&row.modes.relation, &Relation::environment());
            let old = *result;
            *result = t.intern(row);
            properties.inherit(*result, old);
        }
    }
    properties::normalize(
        t,
        body,
        results,
        &mut facts.physical_projections,
        &mut signatures,
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
                || t.get(body.bindings[place.binding].ty)
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
        let content = if owned_receiver {
            root
        } else {
            storage_content_type(
                t,
                root,
                &body.resolution.generic_declarations.clone(),
                &mut |d| {
                    signatures
                        .read(d, Arc::make_mut(&mut body.resolution.reads))
                        .map(Some)
                },
            )?
        };
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
    properties::identity_projections(t, body, &mut facts.identity_fields, &mut signatures)?;
    // Flow refines modes without changing Plain. Reuse those classified rows;
    // normalization and replacement storage can derive further structural rows.
    // The published rows are the final tables' graphs and the facts' rows.
    let mut published = hirsel_modes::rows::BodyRows::new(t, body);
    for &(_, ty, _) in &facts.replacements {
        published.note(t, ty);
    }
    for &(_, ty) in facts.physical_projections.iter().chain(&facts.field_loans) {
        published.note(t, ty);
    }
    for cursor in &facts.borrow_cursors {
        for (_, fields) in cursor.cover.iter().flatten() {
            for &ty in fields {
                published.note(t, ty);
            }
        }
    }
    for &(_, ty) in &facts.physical_patterns {
        published.note(t, ty);
    }
    for (_, _, _, parts) in &facts.identity_fields {
        for &(_, ty) in parts {
            published.note(t, ty);
        }
    }
    for &(_, ty) in results.iter() {
        published.note(t, ty);
    }
    for &id in published.ids() {
        let (plain, copyable) = match properties.get(id) {
            Some(property) => (property.plain, property.copyable),
            None => {
                let reads = Arc::make_mut(&mut body.resolution.reads);
                let mut proof = |property| {
                    signatures.prove(reads, |memo| {
                        hirsel_modes::proofs::property(memo, id, property)
                    })
                };
                (
                    proof(StructuralProperty::Plain)?,
                    proof(StructuralProperty::Copyable)?,
                )
            }
        };
        if plain {
            facts.plain_types.insert(id);
        }
        if copyable && t.row(id).modes.multiplicity != Multiplicity::Once {
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
    facts.borrow_cursors.sort_by_key(|cursor| cursor.binding);
    facts.borrow_cursors.dedup();
    facts.cursor_rebindings.sort();
    facts.cursor_rebindings.dedup();
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
    t: &TypeInterner,
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
            let mut row = t.get(*original).unwrap().clone();
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
            let row = t.intern(row);
            abstract_parameter_scope(t, row, body.lexical.site(scope))
        })
        .collect()
}

mod source_rows;
