//! The existing GEN-5 parameter-edge criterion, applied before closing storage
//! contexts. Mirrors engine/summary.rs::parameters and generic_graph's strict
//! SCC edges; a reset to a closed argument contributes no dependency edge.
//! No iteration/depth cutoff substitutes for the finite-context proof.
use hirsel_syntax::NameSite;
use hirsel_types::storage::graph::*;
use hirsel_types::*;
use std::collections::{BTreeMap, BTreeSet, HashSet};

type Node = (SiteRef, bool, u32);
fn owned<P: Copy>(reference: BinderRef<P>, depth: u32, owner: SiteRef) -> Option<(SiteRef, P)> {
    match reference {
        BinderRef::Body { scope, parameter } => Some((scope, parameter)),
        BinderRef::Bound {
            depth: bound,
            parameter,
        } if bound == depth => Some((owner, parameter)),
        _ => None,
    }
}
fn parameters(
    types: &TypeInterner,
    argument: &GenericArgument,
    owner: SiteRef,
) -> Option<BTreeSet<(Node, bool)>> {
    let mut result = BTreeSet::new();
    let size = |size: &Size, depth, nested, result: &mut BTreeSet<_>| {
        if let Size::Parameter(parameter) = size {
            if let Some((scope, parameter)) = owned(*parameter, depth, owner) {
                result.insert(((scope, true, parameter.0), nested));
            }
        }
    };
    let root = match argument {
        GenericArgument::Type(ty) => *ty,
        GenericArgument::Size(value) => {
            size(value, 0, false, &mut result);
            return Some(result);
        }
    };
    let mut todo = vec![(root, false, 0)];
    let mut seen = HashSet::new();
    while let Some((ty, nested, depth)) = todo.pop() {
        if !seen.insert((ty, nested, depth)) {
            continue;
        }
        match &types.get(ty)?.kind {
            TypeKind::Parameter(parameter) => {
                if let Some((scope, parameter)) = owned(*parameter, depth, owner) {
                    result.insert(((scope, false, parameter.0), nested));
                }
            }
            TypeKind::Tuple(parts) => todo.extend(parts.iter().map(|ty| (*ty, true, depth))),
            TypeKind::Record(fields) => {
                todo.extend(fields.iter().map(|field| (field.ty, true, depth)))
            }
            TypeKind::Variant(variants) => {
                todo.extend(variants.iter().flat_map(|variant| {
                    variant.fields.iter().map(move |(_, ty)| (*ty, true, depth))
                }))
            }
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    match argument {
                        GenericArgument::Type(ty) => todo.push((*ty, true, depth)),
                        GenericArgument::Size(value) => size(value, depth, true, &mut result),
                    }
                }
            }
            TypeKind::Box(ty) => todo.push((*ty, true, depth)),
            TypeKind::Function(function) => {
                todo.extend(
                    function
                        .parameters
                        .iter()
                        .map(|parameter| (parameter.ty, true, depth + 1)),
                );
                todo.push((function.result, true, depth + 1));
            }
            _ => (),
        }
    }
    Some(result)
}
pub(super) fn non_growing(types: &TypeInterner, bodies: &[StorageBody]) -> bool {
    let definitions: BTreeMap<_, _> = bodies
        .iter()
        .filter(|body| body.callable.scope.site == NameSite::Item)
        .map(|body| (body.callable.definition, body))
        .collect();
    let mut edges = vec![];
    for body in bodies {
        let mut uses = vec![];
        for node in &body.nodes {
            match &node.operation {
                StorageOp::Call {
                    callee:
                        StorageCallee::Apply {
                            definition,
                            arguments,
                        },
                    ..
                } => uses.push((*definition, arguments)),
                StorageOp::Assign(assignments) => {
                    for (_, value) in assignments {
                        if let StorageValue::Reference {
                            definition,
                            arguments,
                        } = value
                        {
                            uses.push((*definition, arguments));
                        }
                    }
                }
                _ => (),
            }
        }
        for (definition, arguments) in uses {
            let Some(target) = definitions.get(&definition) else {
                continue;
            };
            for (ordinal, argument) in arguments.iter().enumerate() {
                let destination = (
                    target.callable.scope,
                    matches!(argument, GenericArgument::Size(_)),
                    ordinal as u32,
                );
                let Some(parameters) = parameters(types, argument, body.callable.scope) else {
                    return false;
                };
                for (parameter, grows) in parameters {
                    edges.push((parameter, destination, grows));
                }
            }
        }
    }
    let nodes: BTreeMap<_, _> = edges
        .iter()
        .flat_map(|(from, to, _)| [*from, *to])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, node)| (node, index))
        .collect();
    let mut graph = vec![vec![]; nodes.len()];
    for (from, to, _) in &edges {
        graph[nodes[from]].push(nodes[to]);
    }
    let mut component = vec![0; nodes.len()];
    for (index, members) in hirsel_graph::components(&graph).into_iter().enumerate() {
        for member in members {
            component[member] = index;
        }
    }
    !edges
        .iter()
        .any(|(from, to, grows)| *grows && component[nodes[from]] == component[nodes[to]])
}
