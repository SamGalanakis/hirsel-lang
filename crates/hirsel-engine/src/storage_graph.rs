//! Whole-program source storage proof. Body products are read only after their
//! definition queries have completed, as for GEN-5; signature lookup never
//! recursively asks for a callee's mode result.
use crate::{Diagnostic, QueryEngine, QueryError, RuleId};
use hirsel_type_analysis::storage::{StorageDomain, program};
use hirsel_types::storage::graph::{StorageCallee, StorageOp, StorageValue};
use hirsel_types::storage::{CallableStorageProfile, StorageProfile};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn check(engine: &mut QueryEngine) -> Result<Vec<Diagnostic>, QueryError> {
    derive(engine).map(|(_, diagnostics)| diagnostics)
}
pub(crate) fn profiles(
    engine: &mut QueryEngine,
) -> Result<Vec<CallableStorageProfile>, QueryError> {
    let (profiles, diagnostics) = derive(engine)?;
    if !diagnostics.is_empty() {
        return Err(QueryError::Diagnostics(diagnostics));
    }
    Ok(profiles)
}
fn derive(
    engine: &mut QueryEngine,
) -> Result<(Vec<CallableStorageProfile>, Vec<Diagnostic>), QueryError> {
    let checked = engine.checked_definition_summaries()?;
    if !checked.iter().any(|(_, summary)| {
        summary
            .summary
            .calls
            .iter()
            .any(|call| call.callee.path().is_core("region.with"))
    }) {
        return Ok((vec![], vec![]));
    }
    let mut pending: BTreeSet<_> = checked
        .iter()
        .filter(|(_, summary)| {
            summary
                .summary
                .calls
                .iter()
                .any(|call| call.callee.path().is_core("region.with"))
        })
        .map(|(definition, _)| *definition)
        .collect();
    let mut seen = BTreeSet::new();
    // Bring in actual callers of a demanded definition so its callable/owner
    // inputs retain their original producers. This scans compact call rows;
    // it does not retain all unrelated standard-library CFGs.
    loop {
        let mut added = false;
        for (definition, summary) in &checked {
            if summary
                .summary
                .calls
                .iter()
                .any(|call| pending.contains(&call.callee))
            {
                added |= pending.insert(*definition);
            }
        }
        if !added {
            break;
        }
    }
    let selected = engine.select_generated_instances(
        pending
            .iter()
            .map(|definition| definition.path().clone())
            .collect::<Vec<_>>(),
    )?;
    pending.extend(selected.iter().map(hirsel_syntax::DefId::intern));
    let mut bodies = BTreeMap::new();
    while let Some(definition) = pending.pop_first() {
        if !seen.insert(definition) {
            continue;
        }
        // Reviewed bodyless operations have independently checked recipes in
        // the product. A missing source body is never invented for a core name.
        if definition.path().0.package.name == "core" || engine.instance_bodied(definition.path()) {
            continue;
        }
        let summary = engine.storage_definition_summary(definition)?;
        let mut storage = (**summary
            .storage
            .as_ref()
            .ok_or_else(|| invalid("missing demanded source storage product"))?)
        .clone();
        for body in &mut storage.bodies {
            for node in &body.nodes {
                match &node.operation {
                    StorageOp::Call {
                        callee:
                            StorageCallee::Apply {
                                definition,
                                arguments,
                            },
                        ..
                    } => {
                        pending.insert(*definition);
                        if let Some(selected) = selected_definition(engine, *definition, arguments)?
                        {
                            pending.insert(selected);
                        }
                    }
                    StorageOp::Call {
                        callee: StorageCallee::Definition(callee),
                        ..
                    } => {
                        pending.insert(*callee);
                    }
                    StorageOp::Assign(assignments) => {
                        for (_, value) in assignments {
                            if let StorageValue::Reference {
                                definition,
                                arguments,
                            } = value
                            {
                                pending.insert(*definition);
                                if let Some(selected) =
                                    selected_definition(engine, *definition, arguments)?
                                {
                                    pending.insert(selected);
                                }
                            }
                            if let StorageValue::Callable { target, .. } = value {
                                pending.insert(target.definition);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if bodies.insert(body.callable, body.clone()).is_some() {
                return Err(invalid("duplicate actual callable association"));
            }
        }
    }
    let bodies: Vec<_> = bodies.into_values().collect();
    let domains: Vec<_> = bodies.iter().map(StorageDomain::for_body).collect();
    let instances = engine.instance_bodies.clone();
    let select = |definition, arguments: &[hirsel_types::GenericArgument]| {
        instances
            .resolve(definition, &engine.types, arguments)
            .ok()
            .map(|selected| selected.unwrap_or(definition))
    };
    let proof = program::analyze(&engine.types, &select, &bodies, &domains)
        .map_err(|error| invalid(format!("malformed source storage product: {error:?}")))?;
    let indices: BTreeMap<_, _> = bodies.iter().map(|body| (body.callable, body)).collect();
    let mut diagnostics = Vec::new();
    let mut reported = BTreeSet::new();
    let allocation_reads: BTreeSet<_> = proof
        .violations
        .iter()
        .filter(|violation| {
            matches!(
                violation.failure,
                program::StorageReadFailure::AllocatesFromRoot(_)
            )
        })
        .map(|violation| (violation.callable, violation.node))
        .collect();
    for violation in proof.violations {
        if matches!(
            violation.failure,
            program::StorageReadFailure::ReturnsRoot(_)
        ) && allocation_reads.contains(&(violation.callable, violation.node))
        {
            continue;
        }
        let operation = violation.origin.unwrap_or(program::StorageOperation {
            callable: violation.callable,
            node: violation.node,
        });
        let body = indices
            .get(&operation.callable)
            .ok_or_else(|| invalid("missing storage witness body"))?;
        let site = body
            .nodes
            .get(operation.node)
            .ok_or_else(|| invalid("missing storage witness operation"))?
            .site;
        if !reported.insert((site.anchor, site.start, site.end)) {
            continue;
        }
        let message = match violation.failure {
            program::StorageReadFailure::UnknownCallback => {
                "sealed-root callback has no complete storage provenance"
            }
            program::StorageReadFailure::AllocatesFromRoot(_) => {
                "unique or allocation retains the sealed root's storage placement"
            }
            program::StorageReadFailure::ReturnsRoot(_) => {
                "callback result retains the sealed root's storage placement"
            }
        };
        diagnostics.push(Diagnostic::error(
            const { RuleId::of("MODE-1") },
            site,
            message,
        ));
    }
    let mut profiles = Vec::new();
    if diagnostics.is_empty() {
        for (body, behavior) in bodies.iter().zip(proof.behaviors) {
            if !proof.demanded.contains(&body.callable) {
                continue;
            }
            let source_type = body
                .source_type
                .ok_or_else(|| invalid("demanded callable has no canonical source type"))?;
            let profile = StorageProfile::new(source_type, behavior)
                .map_err(|error| invalid(format!("invalid source storage profile: {error:?}")))?;
            profiles.push(CallableStorageProfile {
                callable: body.callable,
                profile,
            });
        }
    }
    Ok((profiles, diagnostics))
}
fn invalid(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "source storage proof".into(),
        message: message.into(),
    })
}

fn selected_definition(
    engine: &mut QueryEngine,
    declaration: hirsel_syntax::DefId,
    arguments: &[hirsel_types::GenericArgument],
) -> Result<Option<hirsel_syntax::DefId>, QueryError> {
    if declaration.path().0.package.name == "core" {
        return Ok(None);
    }
    if !engine.instance_bodied(declaration.path()) {
        return Ok(Some(declaration));
    }
    let signature = hirsel_types::SignatureProvider::signature(engine, declaration)?;
    let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
        return Ok(None);
    };
    if function.generics.len() != arguments.len()
        || !function
            .generics
            .iter()
            .zip(arguments)
            .all(|(parameter, argument)| {
                matches!(
                    (parameter, argument),
                    (
                        hirsel_types::GenericParameter::Type { .. },
                        hirsel_types::GenericArgument::Type(_)
                    ) | (
                        hirsel_types::GenericParameter::Size { .. },
                        hirsel_types::GenericArgument::Size(_)
                    )
                )
            })
    {
        return Ok(None);
    }
    // An open generic reference has no selected executable body in this
    // source context. It must not fall back to the declaration's signature.
    if hirsel_tir::InstanceBodies::key(&engine.types, arguments).is_none() {
        return Ok(None);
    }
    engine
        .resolve_instance(declaration.path(), arguments)
        .map(|body| body.map(|path| hirsel_syntax::DefId::intern(&path)))
}
