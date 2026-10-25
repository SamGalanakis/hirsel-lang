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

/// Compact checked input. The source rule keeps generic call arguments and
/// lexical sites, without retaining a TypedBody or invoking a body controller.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct TableKeyUse {
    owner: hirsel_syntax::LexicalOwner,
    generics: hirsel_types::GenericDeclarations,
    calls: Vec<TableKeyCall>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct TableKeyCall {
    callee: DefPath,
    site: Span,
    arguments: Vec<GenericArgument>,
}
impl TableKeyUse {
    pub(crate) fn retained(&self, weight: &mut crate::memo::retained::Weight) {
        self.generics
            .visit_retained_allocations(|id, bytes| weight.allocation(id, bytes));
        weight.vec(&self.calls, |w, call| {
            w.def_path(&call.callee);
            w.flat_vec(&call.arguments);
        });
    }
    pub(crate) fn capture(body: &crate::TypedBody) -> Self {
        Self {
            owner: body.syntax.owner(),
            generics: body.checked.resolution.generic_declarations.clone(),
            calls: body
                .checked
                .instantiations
                .iter()
                .map(|call| TableKeyCall {
                    callee: call.callee.clone(),
                    site: body.syntax.tree.expressions[call.expression.0 as usize]
                        .origin
                        .span,
                    arguments: call.arguments.clone(),
                })
                .collect(),
        }
    }
}

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
    // Most programs have no store operations. Avoid schema building until a
    // backward walk from a table operation identifies a relevant generic slot.
    let mut required: BTreeMap<DefPath, BTreeSet<usize>> = BTreeMap::new();
    engine.refresh_source_growth()?;
    for path in engine.source_growth.table_calls() {
        required
            .entry(path.clone())
            .or_default()
            .extend(core_roles::table_key_parameters(path));
    }
    if required.is_empty() {
        return Ok(vec![]);
    }
    let bodies = engine
        .instantiation_bodies()?
        .into_iter()
        .map(|(owner, body)| {
            (
                owner,
                std::sync::Arc::new(TableKeyUse::capture(&body.value)),
            )
        })
        .collect();
    check_uses(&engine.types.clone(), &bodies, required, &mut |path| {
        engine.signature(path).map(Some)
    })
}

/// The existing backwards schema worklist over immutable checked products.
/// The provider supplies only signatures and records its own consumed reads.
pub(crate) fn check_uses(
    types: &hirsel_types::TypeInterner,
    bodies: &BTreeMap<DefPath, std::sync::Arc<TableKeyUse>>,
    mut required: BTreeMap<DefPath, BTreeSet<usize>>,
    signature: &mut impl FnMut(
        hirsel_syntax::DefId,
    ) -> Result<Option<hirsel_types::SignatureRef>, QueryError>,
) -> Result<Vec<Diagnostic>, QueryError> {
    if required.is_empty() {
        return Ok(vec![]);
    }
    let mut edges = Vec::new();
    // Cache each argument's schema only when that slot becomes relevant.
    for (owner, body) in bodies {
        for call in &body.calls {
            edges.push(Edge {
                owner: owner.clone(),
                callee: call.callee.clone(),
                site: call.site,
                arguments: call.arguments.iter().map(|_| None).collect(),
            });
        }
    }
    let mut errors = Vec::new();
    let mut reported = BTreeSet::new();
    loop {
        let mut changed = false;
        let mut edge_index = 0;
        for (owner, body) in bodies {
            for call in &body.calls {
                let edge = &mut edges[edge_index];
                edge_index += 1;
                let positions = required.get(&edge.callee).cloned().unwrap_or_default();
                for position in positions {
                    let Some(GenericArgument::Type(actual)) = call.arguments.get(position) else {
                        continue;
                    };
                    if edge.arguments[position].is_none() {
                        let schema =
                            serializable_schema(types, *actual, &body.generics, signature)?;
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
                                                    body.owner.site(NameSite::Item),
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
