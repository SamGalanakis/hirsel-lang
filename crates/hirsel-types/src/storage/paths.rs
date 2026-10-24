//! Finite structural paths for original value types. This is a lattice domain,
//! never evidence of a code producer or of a callable environment's contents.
//! Follow the existing owned traversal in crossing::owned_crossing_receipts;
//! function headers and shared handles are boundaries. Recursive extensions
//! beyond the original unfolding still normalize to Unknown in the evaluator.
use crate::*;
use std::collections::BTreeSet;

#[derive(Default)]
pub struct StorageTypePaths {
    pub paths: BTreeSet<SharedPath<OwnedTypePart>>,
    pub callables: BTreeSet<SharedPath<OwnedTypePart>>,
}

pub fn storage_type_paths(
    store: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<StorageTypePaths, QueryError> {
    let mut result = StorageTypePaths::default();
    let mut active = std::collections::HashSet::new();
    let mut pending = vec![(root, SharedPath::new(), false)];
    while let Some((id, path, exit)) = pending.pop() {
        if exit {
            active.remove(&id);
            continue;
        }
        result.paths.insert(path.clone());
        if !active.insert(id) {
            continue;
        }
        pending.push((id, path.clone(), true));
        let Some(row) = store.get(id) else { continue };
        let mut push = |ty, segment| {
            let mut child = path.clone();
            child.push(segment);
            pending.push((ty, child, false));
        };
        match &row.kind {
            TypeKind::Function(_) => {
                result.callables.insert(path);
            }
            TypeKind::Tuple(parts) => {
                for (ordinal, ty) in parts.iter().enumerate() {
                    push(*ty, OwnedTypePart::Field(ordinal as u32));
                }
            }
            TypeKind::Record(fields) => {
                for (ordinal, field) in fields.iter().enumerate() {
                    push(field.ty, OwnedTypePart::Field(ordinal as u32));
                }
            }
            TypeKind::Variant(variants) => {
                for (variant, fields) in variants.iter().enumerate() {
                    for (field, (_, ty)) in fields.fields.iter().enumerate() {
                        push(
                            *ty,
                            OwnedTypePart::VariantField {
                                variant: variant as u32,
                                field: field as u32,
                            },
                        );
                    }
                }
            }
            TypeKind::Box(ty) => push(*ty, OwnedTypePart::Field(0)),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                if let Some(signature) = lookup(*definition)? {
                    if let SignatureKind::Type(declaration) = &signature.kind {
                        if let Some(representation) = declaration.representation {
                            let expanded = graph::expand(
                                store,
                                representation,
                                arguments,
                                &signature.metadata.roles,
                            )
                            .map_err(|error| {
                                QueryError::Operational(OperationalFailure {
                                    operation: "storage path domain".into(),
                                    message: format!("{error:?}"),
                                })
                            })?;
                            pending.push((expanded, path, false));
                        }
                    }
                }
            }
            TypeKind::Builtin {
                kind:
                    BuiltinType::Shared
                    | BuiltinType::Mutex
                    | BuiltinType::Atomic
                    | BuiltinType::Iter
                    | BuiltinType::Ptr
                    | BuiltinType::FnPtr
                    | BuiltinType::SlotId
                    | BuiltinType::TypeShape,
                ..
            } => (),
            TypeKind::Builtin {
                kind: BuiltinType::Map | BuiltinType::SortedMap,
                arguments,
            } => {
                for (ordinal, argument) in arguments.iter().enumerate() {
                    if let GenericArgument::Type(ty) = argument {
                        let mut child = path.clone();
                        child.push(OwnedTypePart::Elements);
                        child.push(OwnedTypePart::Field(ordinal as u32));
                        pending.push((*ty, child, false));
                    }
                }
            }
            TypeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    if let GenericArgument::Type(ty) = argument {
                        push(*ty, OwnedTypePart::Elements);
                    }
                }
            }
            _ => (),
        }
    }
    Ok(result)
}
