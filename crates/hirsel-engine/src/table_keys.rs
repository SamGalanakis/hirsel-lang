//! STORE-6 obligations cross ordinary generic wrappers without changing their
//! signatures. A concrete key schema cannot acquire Map/Set/Simd through a
//! wrapper that deferred its schema proof to the wrapper's caller.
use crate::QueryEngine;
use hirsel_diag::{Diagnostic, RuleId};
use hirsel_span::Span;
use hirsel_syntax::DefPath;
use hirsel_types::serialization::SchemaNode;
use hirsel_types::{
    BuiltinType, GenericArgument, GenericBinder, NameSite, QueryError, SignatureProvider,
    core_roles, serializable_schema,
};
use std::collections::{BTreeMap, BTreeSet};

struct Argument {
    forbidden: bool,
    parameters: BTreeSet<usize>,
}
struct Edge {
    owner: DefPath,
    callee: DefPath,
    site: Span,
    arguments: Vec<Option<Argument>>,
}

pub(crate) fn check(engine: &mut QueryEngine) -> Result<Vec<Diagnostic>, QueryError> {
    let bodies = engine.instantiation_bodies()?;
    // Most programs have no store operations. Avoid schema building until a
    // backward walk from a table operation identifies a relevant generic slot.
    let mut required: BTreeMap<DefPath, BTreeSet<usize>> = BTreeMap::new();
    for (_, body) in &bodies {
        for call in &body.value.checked.instantiations {
            let positions = core_roles::table_key_parameters(&call.callee);
            if !positions.is_empty() {
                required
                    .entry(call.callee.clone())
                    .or_default()
                    .extend(positions);
            }
        }
    }
    if required.is_empty() {
        return Ok(vec![]);
    }
    let mut edges = Vec::new();
    // Cache each argument's schema only when that slot becomes relevant.
    for (owner, body) in &bodies {
        for call in &body.value.checked.instantiations {
            edges.push(Edge {
                owner: owner.clone(),
                callee: call.callee.clone(),
                site: body.value.syntax.tree.expressions[call.expression.0 as usize]
                    .origin
                    .span,
                arguments: call.arguments.iter().map(|_| None).collect(),
            });
        }
    }
    let mut errors = Vec::new();
    let mut reported = BTreeSet::new();
    loop {
        let mut changed = false;
        let mut edge_index = 0;
        for (owner, body) in &bodies {
            let checked = &body.value.checked;
            for call in &checked.instantiations {
                let edge = &mut edges[edge_index];
                edge_index += 1;
                let positions = required.get(&edge.callee).cloned().unwrap_or_default();
                for position in positions {
                    let Some(GenericArgument::Type(actual)) = call.arguments.get(position) else {
                        continue;
                    };
                    if edge.arguments[position].is_none() {
                        let schema = serializable_schema(
                            &checked.types,
                            *actual,
                            &checked.resolution.generic_declarations,
                            &mut |path| engine.signature(path).map(Some),
                        )?;
                        let mut argument = Argument {
                            forbidden: false,
                            parameters: BTreeSet::new(),
                        };
                        if let Some(schema) = schema {
                            for node in schema.nodes {
                                match node {
                                    SchemaNode::Builtin {
                                        kind:
                                            BuiltinType::Map | BuiltinType::Set | BuiltinType::Simd,
                                        ..
                                    } => argument.forbidden = true,
                                    SchemaNode::Parameter(parameter) => {
                                        if matches!(parameter.binder(), GenericBinder::Bound(0))
                                            || parameter.binder()
                                                == GenericBinder::Body(
                                                    body.value.syntax.owner().site(NameSite::Item),
                                                )
                                        {
                                            argument
                                                .parameters
                                                .insert(parameter.parameter().0 as usize);
                                        }
                                    }
                                    _ => (),
                                }
                            }
                        }
                        edge.arguments[position] = Some(argument);
                    }
                    let argument = edge.arguments[position].as_ref().unwrap();
                    if argument.forbidden
                        && reported.insert((
                            owner.clone(),
                            edge.site.anchor,
                            edge.site.start,
                            edge.site.end,
                        ))
                    {
                        errors.push(Diagnostic::error(
                            const { RuleId::of("STORE-6") },
                            edge.site,
                            "generic call supplies a table key schema containing Map, Set or Simd",
                        ));
                    }
                    let owner_required = required.entry(edge.owner.clone()).or_default();
                    for parameter in &argument.parameters {
                        changed |= owner_required.insert(*parameter);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(errors)
}
