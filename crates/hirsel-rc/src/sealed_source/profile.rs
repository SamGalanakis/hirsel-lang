//! Reconstruct one complete profile from original types and source-selected
//! callable observations. No layout, native ABI or candidate LIR is read.
use super::{SourceError, closed_storage_type};
use hirsel_lir::sealed_payload_source::{
    ClosedStorageCase, ClosedStorageFunction, ClosedStorageProfile,
};
use hirsel_syntax::DefId;
use hirsel_types::{QueryError, SignatureRef, TypeId, TypeInterner, TypeKind};
use std::collections::{BTreeMap, HashSet};

/// One independently selected original callable case. Capture TypeIds belong
/// to this query's interner and retain the full original types in slot order.
/// The case's recipe must be the actual observed CT5 recipe, never a signature
/// hash or a recipe synthesized for a direct named call.
pub struct CallableCase {
    pub case: ClosedStorageCase,
    pub captures: Vec<TypeId>,
}

/// `callables` must return every case for the given full callable type in this
/// original context. Unknown and empty answers cannot stand for completeness.
/// These callbacks provide source data; the query owner still binds its current
/// source/V observations independently before publishing any provider result.
pub fn derive(
    types: &TypeInterner,
    root: TypeId,
    declarations: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
    callables: &mut impl FnMut(TypeId) -> Result<Vec<CallableCase>, SourceError>,
) -> Result<ClosedStorageProfile, SourceError> {
    derive_impl(types, root, declarations, callables, false)
}

fn derive_impl(
    types: &TypeInterner,
    root: TypeId,
    declarations: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
    callables: &mut impl FnMut(TypeId) -> Result<Vec<CallableCase>, SourceError>,
    boundary: bool,
) -> Result<ClosedStorageProfile, SourceError> {
    let root_type = closed_storage_type(types, root)?;
    let mut pending = vec![(root, boundary)];
    let mut visited = HashSet::new();
    let mut functions = BTreeMap::new();
    while let Some((ty, boundary)) = pending.pop() {
        if !visited.insert((ty, boundary)) {
            continue;
        }
        let shape = if boundary {
            hirsel_layout::structural::derive_boundary_profile_shape(types, ty, declarations)
        } else {
            hirsel_layout::structural::derive_profile_shape(types, ty, declarations)
        }
        .map_err(|error| SourceError::Type(format!("{error:?}")))?
        .ok_or(SourceError::Unsupported(
            "storage profile needs a complete resource-free source shape",
        ))?;
        for ty in shape.node_types {
            let Some(row) = types.get(ty) else {
                return Err(SourceError::Malformed("storage profile source type"));
            };
            let TypeKind::Function(function) = &row.kind else {
                continue;
            };
            if function.incomplete || !function.generics.is_empty() {
                return Err(SourceError::Unsupported(
                    "storage profile needs a fully selected original callable type",
                ));
            }
            let closed = closed_storage_type(types, ty)?;
            if functions.contains_key(&closed.source_type) {
                continue;
            }
            let observed = callables(ty)?;
            if observed.is_empty() {
                return Err(SourceError::Unsupported(
                    "storage profile needs complete original callable cases",
                ));
            }
            let mut cases = Vec::with_capacity(observed.len());
            for observed in observed {
                match &observed.case.environment {
                    None if !observed.captures.is_empty() => {
                        return Err(SourceError::Malformed("code-only case has source captures"));
                    }
                    None => (),
                    Some(environment) => {
                        if environment.captures.len() != observed.captures.len() {
                            return Err(SourceError::Malformed(
                                "callable environment source capture arity",
                            ));
                        }
                        for (capture, &ty) in environment.captures.iter().zip(&observed.captures) {
                            if capture.ty != closed_storage_type(types, ty)? {
                                return Err(SourceError::Malformed(
                                    "callable environment differs from original capture type",
                                ));
                            }
                        }
                    }
                }
                // Owned environment payloads remain resource-free even when the
                // callable itself is a boundary parameter/result.
                pending.extend(observed.captures.into_iter().map(|ty| (ty, false)));
                cases.push(observed.case);
            }
            functions.insert(
                closed.source_type,
                ClosedStorageFunction { ty: closed, cases },
            );
        }
    }
    ClosedStorageProfile {
        schema: ClosedStorageProfile::SCHEMA,
        root: root_type,
        functions: functions.into_values().collect(),
    }
    .canonical()
    .map_err(SourceError::Malformed)
}

/// Complete boundary type profile for receipt context identity. Arena is a
/// scoped grant with no owned callable payload; Region owns the profile of T.
/// These handle rules do not authorize either handle inside sealed contents:
/// payload admission continues to use the resource-free `derive` above.
pub fn boundary(
    types: &TypeInterner,
    root: TypeId,
    declarations: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
    callables: &mut impl FnMut(TypeId) -> Result<Vec<CallableCase>, SourceError>,
) -> Result<ClosedStorageProfile, SourceError> {
    use hirsel_types::{BuiltinType, CapabilityKind, GenericArgument, SignatureKind};
    let original = closed_storage_type(types, root)?;
    let mut payload = root;
    loop {
        let row = types
            .get(payload)
            .ok_or(SourceError::Malformed("boundary profile type"))?;
        match &row.kind {
            TypeKind::Builtin {
                kind: BuiltinType::Region,
                arguments,
            } => {
                let [GenericArgument::Type(ty)] = arguments.as_slice() else {
                    return Err(SourceError::Malformed("boundary Region type arguments"));
                };
                payload = *ty;
            }
            TypeKind::Named {
                definition,
                arguments,
            } if (definition.path().is_core("Arena") || definition.path().is_core("Test"))
                && arguments.is_empty() =>
            {
                let (missing, changed) = if definition.path().is_core("Arena") {
                    (
                        "boundary Arena needs its original declaration",
                        "boundary Arena declaration differs",
                    )
                } else {
                    (
                        "boundary Test needs its original declaration",
                        "boundary Test declaration differs",
                    )
                };
                let declaration = declarations(*definition)
                    .map_err(|error| SourceError::Type(format!("{error:?}")))?
                    .ok_or(SourceError::Unsupported(missing))?;
                if declaration.definition != *definition
                    || !matches!(&declaration.kind, SignatureKind::Type(ty) if ty.generics.is_empty()
                        && ty.representation.is_none() && !ty.resource && ty.capability == Some(CapabilityKind::Scoped))
                {
                    return Err(SourceError::Malformed(changed));
                }
                return ClosedStorageProfile {
                    schema: ClosedStorageProfile::SCHEMA,
                    root: original,
                    functions: vec![],
                }
                .canonical()
                .map_err(SourceError::Malformed);
            }
            _ => {
                let mut profile = derive_impl(types, payload, declarations, callables, true)?;
                profile.root = original;
                return profile.canonical().map_err(SourceError::Malformed);
            }
        }
    }
}

#[cfg(test)]
mod tests;
