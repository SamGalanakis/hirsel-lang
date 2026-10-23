//! Reconstruct MEM-3 value edges from independently supplied type declarations.
use hirsel_lir::structural::{Shape, ShapeGraph};
use hirsel_syntax::DefId;
use hirsel_types::*;
use std::collections::HashMap;

pub fn derive(
    types: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Option<ShapeGraph>, QueryError> {
    Ok(derive_typed(types, root, lookup)?.map(|derived| derived.graph))
}

/// Transient correspondence for this derivation's interner. Numeric TypeIds
/// are never durable identities and must not be reused in another session.
pub struct DerivedShape<'a> {
    pub graph: ShapeGraph,
    pub node_types: Vec<TypeId>,
    pub types: &'a TypeInterner,
}

pub fn derive_typed<'a>(
    types: &'a TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Option<DerivedShape<'a>>, QueryError> {
    let memo = TypeMemo::new();
    let mut graph = ShapeGraph {
        root: 0,
        nodes: vec![],
    };
    let mut known = HashMap::new();
    let mut pending = vec![];
    fn node(
        ty: TypeId,
        graph: &mut ShapeGraph,
        known: &mut HashMap<TypeId, u32>,
        pending: &mut Vec<(TypeId, bool)>,
    ) -> u32 {
        *known.entry(ty).or_insert_with(|| {
            let id = graph.nodes.len() as u32;
            graph.nodes.push(Shape::Leaf);
            pending.push((ty, false));
            id
        })
    }
    node(root, &mut graph, &mut known, &mut pending);
    let mut expansions = NominalExpansions::default();
    while let Some((ty, exit)) = pending.pop() {
        if exit {
            expansions.leave(ty, 0);
            continue;
        }
        let Some(row) = types.get(ty) else {
            return Ok(None);
        };
        let child =
            |ty,
             graph: &mut ShapeGraph,
             known: &mut HashMap<TypeId, u32>,
             pending: &mut Vec<(TypeId, bool)>| node(ty, graph, known, pending);
        let shape = match &row.kind {
            TypeKind::Primitive(Primitive::Never)
            | TypeKind::Function(_)
            | TypeKind::Parameter(_)
            | TypeKind::Error => return Ok(None),
            TypeKind::Primitive(_) => Shape::Leaf,
            TypeKind::Tuple(fields) => Shape::Fields(
                fields
                    .iter()
                    .map(|&ty| child(ty, &mut graph, &mut known, &mut pending))
                    .collect(),
            ),
            TypeKind::Record(fields) => Shape::Fields(
                fields
                    .iter()
                    .map(|field| child(field.ty, &mut graph, &mut known, &mut pending))
                    .collect(),
            ),
            TypeKind::Variant(variants) => Shape::Variant(
                variants
                    .iter()
                    .map(|variant| {
                        variant
                            .fields
                            .iter()
                            .map(|(_, ty)| child(*ty, &mut graph, &mut known, &mut pending))
                            .collect()
                    })
                    .collect(),
            ),
            TypeKind::Box(payload) => {
                Shape::Box(child(*payload, &mut graph, &mut known, &mut pending))
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let Some(signature) = lookup(*definition)? else {
                    return Ok(None);
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    return Ok(None);
                };
                if declaration.resource {
                    return Ok(None);
                }
                // Path has a declared abstract scalar structural view.
                if definition.path().is_core("Path") {
                    Shape::Leaf
                } else {
                    let Some(representation) = declaration.representation else {
                        return Ok(None);
                    };
                    if !expansions.enter(ty, 0, types)? {
                        return Ok(None);
                    }
                    pending.push((ty, true));
                    let representation = substitute(
                        types,
                        &memo,
                        representation,
                        &Substitution {
                            binder: Some(GenericBinder::Bound(0)),
                            arguments,
                            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                            roles: &signature.metadata.roles,
                        },
                    )
                    .map_err(|error| {
                        QueryError::Operational(OperationalFailure {
                            operation: "logical structural shape".into(),
                            message: format!("{error:?}"),
                        })
                    })?;
                    let payload = child(representation, &mut graph, &mut known, &mut pending);
                    if declaration.distinct {
                        Shape::Distinct(payload)
                    } else {
                        Shape::Transparent(payload)
                    }
                }
            }
            TypeKind::Builtin { kind, arguments } => {
                let mut argument = |ordinal| match arguments.get(ordinal) {
                    Some(GenericArgument::Type(ty)) => {
                        Some(child(*ty, &mut graph, &mut known, &mut pending))
                    }
                    _ => None,
                };
                match kind {
                    BuiltinType::Map => {
                        let (Some(key), Some(value)) = (argument(0), argument(1)) else {
                            return Ok(None);
                        };
                        Shape::Map { key, value }
                    }
                    BuiltinType::Set => {
                        let Some(element) = argument(0) else {
                            return Ok(None);
                        };
                        Shape::Set(element)
                    }
                    BuiltinType::List
                    | BuiltinType::Array
                    | BuiltinType::Inline
                    | BuiltinType::Simd => {
                        let Some(element) = argument(0) else {
                            return Ok(None);
                        };
                        Shape::Sequence(element)
                    }
                    BuiltinType::Packed | BuiltinType::Shared => {
                        let Some(payload) = argument(0) else {
                            return Ok(None);
                        };
                        Shape::Transparent(payload)
                    }
                    _ => return Ok(None),
                }
            }
        };
        graph.nodes[known[&ty] as usize] = shape;
    }
    if graph.check().is_err() {
        return Ok(None);
    }
    let mut node_types = vec![root; graph.nodes.len()];
    for (ty, node) in known {
        node_types[node as usize] = ty;
    }
    Ok(Some(DerivedShape {
        graph,
        node_types,
        types,
    }))
}

/// Producer shape equality is checked against declarations, not against a
/// shape copied from the candidate unit or reconstructed from physical fields.
pub fn validate(
    graph: &ShapeGraph,
    types: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    Ok(graph.check().is_ok() && derive(types, root, lookup)?.as_ref() == Some(graph))
}
