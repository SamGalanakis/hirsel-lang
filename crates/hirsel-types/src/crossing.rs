//! Signature-derived generic transport and owned crossing contracts (#96).
use crate::*;
use std::collections::BTreeSet;

/// Conjunction of independently required current crossing proofs.
pub fn combine_crossing_evidence(left: Sendability, right: Sendability) -> Sendability {
    match (left, right) {
        (Sendability::NotSendable, _) | (_, Sendability::NotSendable) => Sendability::NotSendable,
        (Sendability::Sendable, proof) | (proof, Sendability::Sendable) => proof,
        (Sendability::DependsOn(mut left), Sendability::DependsOn(right)) => {
            left.extend(right);
            left.sort();
            left.dedup();
            Sendability::DependsOn(left)
        }
    }
}

/// A retained callback that mutates a retained abstract state must preserve
/// that state's crossing contract on every invocation. The current initial
/// state's ownership is insufficient. These links come solely from headers.
pub fn mutable_capture_states(
    types: &TypeStore,
    function: &FunctionType,
) -> Vec<(usize, usize, usize)> {
    let Some(ErasedCaptures::FromArguments(retained)) =
        &types.get(function.result).unwrap().erased_captures
    else {
        return vec![];
    };
    let retained = |i: usize| {
        retained.contains(&RelationPlace {
            parameter: ParameterRef::bound(ParamId(i as u32)),
            fields: vec![].into(),
        })
    };
    let mut links = Vec::new();
    for (callback, parameter) in function
        .parameters
        .iter()
        .enumerate()
        .filter(|(i, _)| retained(*i))
    {
        let TypeKind::Function(header) = &types.get(parameter.ty).unwrap().kind else {
            continue;
        };
        for (slot, parameter) in header
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| p.passing == hirsel_syntax::PassingMode::Mut)
        {
            let TypeKind::Parameter(mut inner) = types.get(parameter.ty).unwrap().kind else {
                continue;
            };
            if let TypeParameterRef::Bound { depth, parameter } = inner {
                let Some(depth) = depth.checked_sub(1) else {
                    continue;
                };
                inner = TypeParameterRef::Bound { depth, parameter };
            }
            for (state, parameter) in function
                .parameters
                .iter()
                .enumerate()
                .filter(|(i, _)| retained(*i))
            {
                if matches!(types.get(parameter.ty).unwrap().kind, TypeKind::Parameter(outer) if inner == outer)
                {
                    links.push((state, callback, slot));
                }
            }
        }
    }
    links
}

/// Evidence supplied by a callback's fixed mutable parameter contract, rather
/// than by a particular value it happened to receive when constructed.
pub fn mutable_capture_state_sendability(
    source: &TypeStore,
    callback: TypeId,
    slot: usize,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    let TypeKind::Function(header) = &source.get(callback).unwrap().kind else {
        return Ok(Sendability::NotSendable);
    };
    let Some(parameter) = header.parameters.get(slot) else {
        return Ok(Sendability::NotSendable);
    };
    let mut store = source.clone();
    let root = declared_receipt_evidence(&mut store, parameter.ty).map_err(|error| {
        QueryError::Operational(OperationalFailure {
            operation: "mutable capture contract".into(),
            message: format!("{error:?}"),
        })
    })?;
    content_sendability(&store, root, declarations, lookup)
}

/// Copying permission is per declared binder. The selected result read is
/// included here without installing it for ordinary operations in the body.
pub fn sharing_type_parameters(
    types: &TypeStore,
    function: &FunctionType,
) -> BTreeSet<TypeParamId> {
    local_result_generics(types, function).iter().enumerate().filter_map(|(ordinal, parameter)| {
        matches!(parameter, GenericParameter::Type { bounds, .. } if bounds.iter().any(|b| matches!(b, Bound::Copyable | Bound::Data | Bound::Key | Bound::Flat | Bound::Plain)))
            .then_some(TypeParamId(ordinal as u32))
    }).collect()
}

fn redundant(
    store: &TypeStore,
    root: TypeId,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let row = store.get(root).unwrap();
    if structural_property(store, root, StructuralProperty::Plain, declarations, lookup)? {
        return Ok(true);
    }
    if !row.modes.relation.is_global() {
        return Ok(false);
    }
    Ok(structural_flat(store, root, declarations, lookup)?
        || matches!(
            row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared | BuiltinType::Sender | BuiltinType::Mutex,
                ..
            }
        )
        || row.modes.uniqueness == Uniqueness::Unique
            && !matches!(
                row.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            )
            && structural_sendability(store, root, declarations, lookup)? == Sendability::Sendable)
}

/// Hidden requirements copied with an actual's owned payload. Invocation
/// headers and Shared contents do not belong to that payload. Visible nominal
/// representations are checked; opaque arguments are conservative obligations.
pub fn has_owned_crossing_requirement(
    source: &TypeStore,
    root: TypeId,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let mut store = source.clone();
    let mut pending = vec![(root, false, false)];
    let mut seen = BTreeSet::new();
    let mut nominals = NominalExpansions::default();
    while let Some((id, inherited_unique, exit)) = pending.pop() {
        if exit {
            nominals.leave(id, 0);
            continue;
        }
        if !seen.insert((id, inherited_unique)) {
            continue;
        }
        let Some(mut row) = store.get(id).cloned() else {
            return Ok(true);
        };
        let unique = inherited_unique || row.modes.uniqueness == Uniqueness::Unique;
        let proof_root = if inherited_unique {
            row.modes.uniqueness = Uniqueness::Unique;
            store.push(row.clone())
        } else {
            id
        };
        if row.sendability_requirement == SendabilityRequirement::Sendable
            && !redundant(&store, proof_root, declarations, lookup)?
        {
            return Ok(true);
        }
        match row.kind {
            TypeKind::Function(_)
            | TypeKind::Builtin {
                kind:
                    BuiltinType::Shared
                    | BuiltinType::Ptr
                    | BuiltinType::FnPtr
                    | BuiltinType::TypeShape
                    | BuiltinType::SlotId
                    | BuiltinType::Iter,
                ..
            } => (),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                if !nominals.enter(id, 0, |_| &store)? {
                    continue;
                }
                pending.push((id, unique, true));
                let Some(signature) = lookup(&definition)? else {
                    return Ok(true);
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    return Ok(true);
                };
                if let Some(representation) = declaration.representation.filter(|_| {
                    signature.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque
                }) {
                    let root = substitute_type(
                        &signature.types,
                        representation,
                        &mut store,
                        Substitution {
                            binder: Some(GenericBinder::Bound(0)),
                            roles: &signature.metadata.roles,
                            arguments: &arguments,
                            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                        },
                    )
                    .map_err(|error| {
                        QueryError::Operational(OperationalFailure {
                            operation: "owned crossing refinement".into(),
                            message: format!("{error:?}"),
                        })
                    })?;
                    pending.push((root, unique, false));
                } else {
                    pending.extend(arguments.iter().filter_map(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some((*t, unique, false))
                        } else {
                            None
                        }
                    }));
                }
            }
            kind => pending.extend(kind.value_parts().into_iter().map(|t| (t, unique, false))),
        }
    }
    Ok(false)
}

/// Forget owned requirements only along read-only covariant edges. Invariant
/// nominal/builtin parts and invocation headers remain exact. Callers check
/// the entire resulting receipt and admissibility independently.
pub fn forget_read_crossing_requirements(
    store: &mut TypeStore,
    root: TypeId,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    fn visit(
        store: &mut TypeStore,
        id: TypeId,
        declarations: &GenericDeclarations,
        active: &mut BTreeSet<TypeId>,
        lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
    ) -> Result<TypeId, QueryError> {
        if !active.insert(id) {
            return Ok(id);
        }
        let mut row = store.get(id).unwrap().clone();
        if row.sendability_requirement == SendabilityRequirement::Sendable {
            row.sendability_requirement = SendabilityRequirement::None;
            if !matches!(
                row.kind,
                TypeKind::Function(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
            ) && !redundant(store, id, declarations, lookup)?
            {
                row.modes.sendability = Sendability::NotSendable;
            }
        }
        match &mut row.kind {
            TypeKind::Tuple(parts) => {
                for part in parts {
                    *part = visit(store, *part, declarations, active, lookup)?;
                }
            }
            TypeKind::Box(part) => *part = visit(store, *part, declarations, active, lookup)?,
            TypeKind::Record(fields) => {
                for field in fields {
                    field.ty = visit(store, field.ty, declarations, active, lookup)?;
                }
            }
            TypeKind::Variant(variants) => {
                for part in variants
                    .iter_mut()
                    .flat_map(|v| v.fields.iter_mut().map(|f| &mut f.1))
                {
                    *part = visit(store, *part, declarations, active, lookup)?;
                }
            }
            TypeKind::Builtin {
                kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline,
                arguments,
            } => {
                for argument in arguments {
                    if let GenericArgument::Type(part) = argument {
                        *part = visit(store, *part, declarations, active, lookup)?;
                    }
                }
            }
            _ => (),
        }
        active.remove(&id);
        Ok(store.push(row))
    }
    visit(store, root, declarations, &mut BTreeSet::new(), lookup)
}

/// Physical owned-part paths; this is a traversal result, never a new type form.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum OwnedTypePart {
    Field(u32),
    VariantField { variant: u32, field: u32 },
    Elements,
}

/// Qualified owned receipts and whether their checked content proof survives
/// ordinary sharing independently. Header and Shared boundaries remain opaque.
pub fn owned_crossing_receipts(
    source: &TypeStore,
    root: TypeId,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Vec<(Vec<OwnedTypePart>, bool)>, QueryError> {
    let mut store = source.clone();
    let mut pending = vec![(root, Vec::new(), false)];
    let mut nominals = NominalExpansions::default();
    let mut receipts = Vec::new();
    while let Some((id, path, exit)) = pending.pop() {
        if exit {
            nominals.leave(id, 0);
            continue;
        }
        let Some(row) = store.get(id).cloned() else {
            continue;
        };
        if row.sendability_requirement == SendabilityRequirement::Sendable {
            let independent = sendability_after_sharing(&store, id, declarations, lookup)?
                == Sendability::Sendable;
            receipts.push((path.clone(), independent));
        }
        let mut push = |part, segment| {
            let mut path = path.clone();
            path.push(segment);
            pending.push((part, path, false));
        };
        match row.kind {
            TypeKind::Tuple(parts) => {
                for (ordinal, part) in parts.iter().enumerate() {
                    push(*part, OwnedTypePart::Field(ordinal as u32));
                }
            }
            TypeKind::Record(fields) => {
                for (ordinal, field) in fields.iter().enumerate() {
                    push(field.ty, OwnedTypePart::Field(ordinal as u32));
                }
            }
            TypeKind::Variant(variants) => {
                for (variant, fields) in variants.iter().enumerate() {
                    for (field, part) in fields.fields.iter().enumerate() {
                        push(
                            part.1,
                            OwnedTypePart::VariantField {
                                variant: variant as u32,
                                field: field as u32,
                            },
                        );
                    }
                }
            }
            TypeKind::Box(part) => push(part, OwnedTypePart::Field(0)),
            TypeKind::Function(_)
            | TypeKind::Builtin {
                kind:
                    BuiltinType::Shared
                    | BuiltinType::Iter
                    | BuiltinType::Ptr
                    | BuiltinType::FnPtr
                    | BuiltinType::TypeShape
                    | BuiltinType::SlotId,
                ..
            } => (),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                if !nominals.enter(id, 0, |_| &store)? {
                    continue;
                }
                pending.push((id, path.clone(), true));
                let Some(signature) = lookup(&definition)? else {
                    continue;
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    continue;
                };
                if let Some(representation) = declaration.representation.filter(|_| {
                    signature.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque
                }) {
                    let expanded = substitute_type(
                        &signature.types,
                        representation,
                        &mut store,
                        Substitution {
                            binder: Some(GenericBinder::Bound(0)),
                            roles: &signature.metadata.roles,
                            arguments: &arguments,
                            authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                        },
                    )
                    .map_err(|error| {
                        QueryError::Operational(OperationalFailure {
                            operation: "owned crossing receipts".into(),
                            message: format!("{error:?}"),
                        })
                    })?;
                    pending.push((expanded, path, false));
                }
            }
            TypeKind::Builtin {
                kind: BuiltinType::Map | BuiltinType::SortedMap,
                arguments,
            } => {
                for (ordinal, argument) in arguments.iter().enumerate() {
                    if let GenericArgument::Type(part) = argument {
                        let mut path = path.clone();
                        path.push(OwnedTypePart::Elements);
                        path.push(OwnedTypePart::Field(ordinal as u32));
                        pending.push((*part, path, false));
                    }
                }
            }
            TypeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    if let GenericArgument::Type(part) = argument {
                        push(part, OwnedTypePart::Elements);
                    }
                }
            }
            _ => (),
        }
    }
    Ok(receipts)
}

/// Receiving view for independently checking Sendable bounds on every owned
/// occurrence. The overlay is a requirement in scratch storage, never evidence
/// installed on the actual or an exported signature.
pub fn generic_crossing_parameter(
    source: &TypeStore,
    function: &FunctionType,
    parameter: usize,
    target: &mut TypeStore,
    arguments: &[GenericArgument],
    roles: &NominalRoles,
) -> Result<Option<TypeId>, GraphError> {
    let Some(formal) = function.parameters.get(parameter) else {
        return Ok(None);
    };
    let checked = sendable_arguments(function, target, arguments)?;
    substitute_type(
        source,
        formal.ty,
        target,
        crossing_substitution(roles, arguments, &checked, &authorities(arguments)),
    )
    .map(Some)
}

/// `generic_crossing_parameter` over one store holding the function type's
/// rows and the arguments: the receiving view's rows are appended to `store`,
/// so a caller discards them with `TypeStore::truncate_to` instead of copying
/// the store for each call.
pub fn generic_crossing_parameter_in_place(
    store: &mut TypeStore,
    function: &FunctionType,
    parameter: usize,
    arguments: &[GenericArgument],
    roles: &NominalRoles,
) -> Result<Option<TypeId>, GraphError> {
    let Some(formal) = function.parameters.get(parameter) else {
        return Ok(None);
    };
    let checked = sendable_arguments(function, store, arguments)?;
    substitute_type_in_place(
        store,
        formal.ty,
        crossing_substitution(roles, arguments, &checked, &authorities(arguments)),
    )
    .map(Some)
}

/// The arguments, each one bound by Sendable replaced by a row requiring it.
fn sendable_arguments(
    function: &FunctionType,
    target: &mut TypeStore,
    arguments: &[GenericArgument],
) -> Result<Vec<GenericArgument>, GraphError> {
    let mut checked = arguments.to_vec();
    for (argument, declaration) in checked.iter_mut().zip(&function.generics) {
        if matches!(declaration, GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Sendable))
        {
            if let GenericArgument::Type(actual) = argument {
                let mut row = target
                    .get(*actual)
                    .ok_or(GraphError::MissingType(*actual))?
                    .clone();
                row.sendability_requirement = SendabilityRequirement::Sendable;
                *actual = target.push(row);
            }
        }
    }
    Ok(checked)
}

fn authorities(arguments: &[GenericArgument]) -> Vec<AuthoritySummary> {
    vec![AuthoritySummary::Unknown; arguments.len()]
}

fn crossing_substitution<'a>(
    roles: &'a NominalRoles,
    arguments: &[GenericArgument],
    checked: &'a [GenericArgument],
    authorities: &'a [AuthoritySummary],
) -> Substitution<'a> {
    Substitution {
        binder: (!arguments.is_empty()).then_some(GenericBinder::Bound(0)),
        roles,
        arguments: checked,
        authorities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copying_admissibility_uses_fixed_modes_and_owned_boundaries() {
        let mut store = TypeStore::default();
        let mut row = Type::unit();
        row.kind = TypeKind::Primitive(Primitive::Bytes);
        row.sendability_requirement = SendabilityRequirement::Sendable;
        row.modes.sendability = Sendability::Sendable;
        let bytes = store.push(row.clone());
        let declarations = GenericDeclarations::default();
        let mut lookup = |_: &hirsel_syntax::DefPath| Ok(None);
        assert!(has_owned_crossing_requirement(&store, bytes, &declarations, &mut lookup).unwrap());
        row.kind = TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(bytes)].into(),
        };
        let list = store.push(row.clone());
        assert!(has_owned_crossing_requirement(&store, list, &declarations, &mut lookup).unwrap());
        row.modes.uniqueness = Uniqueness::Unique;
        let unique_list = store.push(row.clone());
        assert!(
            !has_owned_crossing_requirement(&store, unique_list, &declarations, &mut lookup)
                .unwrap()
        );
        row.modes.uniqueness = Uniqueness::Aliased;
        row.sendability_requirement = SendabilityRequirement::None;
        row.kind = TypeKind::Builtin {
            kind: BuiltinType::Shared,
            arguments: vec![GenericArgument::Type(bytes)].into(),
        };
        let shared = store.push(row);
        assert!(
            !has_owned_crossing_requirement(&store, shared, &declarations, &mut lookup).unwrap()
        );
    }
    #[test]
    fn read_only_forgetting_stops_at_invariant_edges() {
        let mut store = TypeStore::default();
        let mut row = Type::unit();
        row.kind = TypeKind::Primitive(Primitive::Bytes);
        row.sendability_requirement = SendabilityRequirement::Sendable;
        let bytes = store.push(row.clone());
        let declarations = GenericDeclarations::default();
        let mut lookup = |_: &hirsel_syntax::DefPath| Ok(None);
        for (kind, forgets) in [(BuiltinType::List, true), (BuiltinType::Map, false)] {
            row.kind = TypeKind::Builtin {
                kind,
                arguments: vec![GenericArgument::Type(bytes)].into(),
            };
            row.sendability_requirement = SendabilityRequirement::None;
            let before = store.push(row.clone());
            let after =
                forget_read_crossing_requirements(&mut store, before, &declarations, &mut lookup)
                    .unwrap();
            assert_eq!(
                has_owned_crossing_requirement(&store, after, &declarations, &mut lookup).unwrap(),
                !forgets
            );
        }
    }
}
