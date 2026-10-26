//! Recheck owner declarations independently of source normalization. Public
//! opaque snapshots have no representation; private owner views do.
use super::*;
use hirsel_type_analysis::cleanup::{
    CleanupLeaf, cleanup_summary, declared_cleanup, parameter_cleanup,
};
use hirsel_types::*;

pub(super) fn valid(t: &TypeInterner, s: &Signature, signatures: &Reader<'_>) -> bool {
    if !s.metadata.inferred_unique.is_empty() {
        let SignatureKind::Function(f) = &s.kind else {
            return false;
        };
        if s.metadata.visibility != hirsel_syntax::Visibility::Private
            || f.foreign.is_some()
            || s.metadata
                .inferred_unique
                .preserves_unique
                .windows(2)
                .any(|p| p[0] >= p[1])
            || s.metadata
                .inferred_unique
                .results
                .windows(2)
                .any(|p| p[0] >= p[1])
            || s.metadata.inferred_unique.preserves_unique.iter().any(|p| {
                f.parameters
                    .get(p.0 as usize)
                    .is_none_or(|p| p.passing != hirsel_syntax::PassingMode::Mut)
            })
        {
            return false;
        }
        let rows =
            hirsel_types::unique_summary::result_rows(t, f.result, &s.metadata.roles, &mut |d| {
                signatures.get(d).cloned().map(std::sync::Arc::new)
            });
        if s.metadata
            .inferred_unique
            .results
            .iter()
            .any(|row| !rows.iter().any(|(r, _)| row == r))
        {
            return false;
        }
    }
    if s.metadata.track_caller
        && !matches!(&s.kind, SignatureKind::Function(f) if f.foreign.is_none())
    {
        return false;
    }
    let SignatureKind::Type(d) = &s.kind else {
        return true;
    };
    if d.capability == Some(CapabilityKind::Scoped)
        && d.bounds
            .iter()
            .any(|b| matches!(b, Bound::Plain | Bound::Copyable | Bound::Sendable))
    {
        return false;
    }
    if d.capability.is_some()
        && (d.authority
            != AuthoritySummary::Known(
                vec![AuthorityTerm::Capability(s.definition.path().clone())].into(),
            )
            || d.serializable
            || d.bounds.iter().any(|b| {
                matches!(
                    b,
                    Bound::Inert | Bound::Data | Bound::Key | Bound::Flat | Bound::Serializable
                )
            }))
    {
        return false;
    }
    if d.captures.is_some()
        && (d.capability.is_some()
            || d.resource
            || d.serializable
            || d.layout.flat
            || d.fact.is_some()
            || d.bounds.contains(&Bound::Inert))
    {
        return false;
    }
    let Some(root) = d.representation else {
        return d.implementation_authority.is_none();
    };
    if d.capability.is_some() && s.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque {
        return false;
    }
    if d.capability.is_some()
        && !matches!(d.implementation_authority, Some(AuthoritySummary::Known(_)))
        || d.capability.is_none() && d.implementation_authority.is_some()
    {
        return false;
    }
    let declarations = GenericDeclarations::signature(&d.generics);
    if d.capability == Some(CapabilityKind::Global)
        && !global_representation(t, root, &s.metadata.roles, &declarations, signatures)
    {
        return false;
    }
    if d.layout.flat && !flat_storage(t, root, &declarations, signatures, &mut HashSet::default()) {
        return false;
    }
    if s.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque && d.capability.is_none() {
        return true;
    }
    // CAP-2/CLO-4: a carrier's selected fields are its per-value erased
    // evidence; only the remaining fields count against the declaration.
    let roots: Vec<TypeId> = match (&d.captures, t.get(root).map(|row| &row.kind)) {
        (None, _) => vec![root],
        (Some(selected), Some(TypeKind::Record(parts))) if !d.distinct => {
            if selected.is_empty() || selected.iter().any(|field| *field as usize >= parts.len()) {
                return false;
            }
            parts
                .iter()
                .enumerate()
                .filter(|(i, _)| !selected.contains(&(*i as u32)))
                .map(|(_, part)| part.ty)
                .collect()
        }
        (Some(selected), _) if d.distinct && selected.as_slice() == [0] => vec![],
        (Some(_), _) => return false,
    };
    let mut authority = AuthoritySummary::inert();
    loop {
        let mut own = s.clone();
        let SignatureKind::Type(own_type) = &mut own.kind else {
            unreachable!()
        };
        if d.capability.is_none() {
            own_type.authority = authority.clone();
        }
        let next = roots.iter().fold(AuthoritySummary::inert(), |held, root| {
            held.join(
                structural_authority(
                    t,
                    *root,
                    &s.metadata.roles,
                    &declarations,
                    &mut |_, path| {
                        Ok::<_, QueryError>(path.and_then(|path| {
                            if path == s.definition {
                                Some(std::sync::Arc::new(own.clone()))
                            } else {
                                signatures.get(path).cloned().map(std::sync::Arc::new)
                            }
                        }))
                    },
                )
                .unwrap_or(AuthoritySummary::Unknown),
            )
        });
        if next == authority {
            break;
        }
        authority = next;
    }
    if !authority.is_subset_of(d.implementation_authority.as_ref().unwrap_or(&d.authority)) {
        return false;
    }
    let mut lookup =
        |path: DefId| Ok::<_, QueryError>(signatures.get(path).cloned().map(std::sync::Arc::new));
    d.bounds.iter().all(|bound| {
        if d.capability == Some(CapabilityKind::Scoped)
            && matches!(bound, Bound::Plain | Bound::Copyable | Bound::Sendable)
        {
            return false;
        }
        if d.resource
            && matches!(
                bound,
                Bound::Plain | Bound::Flat | Bound::Data | Bound::Key | Bound::Copyable
            )
        {
            return false;
        }
        match bound {
            Bound::Serializable => true, // Independently proved by serialization::declaration.
            Bound::Inert => d.authority == AuthoritySummary::inert() && d.captures.is_none(),
            Bound::Flat if d.layout.flat => true,
            Bound::Flat => structural_flat(t, root, &declarations, &mut lookup).unwrap_or(false),
            Bound::Plain if d.layout.flat => false,
            Bound::Sendable if d.layout.flat => true,
            Bound::Data | Bound::Key => {
                structural_equality(t, root, *bound, &declarations, &mut lookup).unwrap_or(false)
            }
            Bound::NoallocDrop => {
                (!d.resource || d.destructor_allocation == Allocation::NoAlloc)
                    && cleanup(t, root, &declarations, signatures, &mut HashSet::default())
                        == DropAllocation::NoAlloc
            }
            Bound::Plain | Bound::Copyable | Bound::Sendable => {
                let property = match bound {
                    Bound::Plain => StructuralProperty::Plain,
                    Bound::Copyable => StructuralProperty::Copyable,
                    _ => StructuralProperty::Sendable,
                };
                structural_property(t, root, property, &declarations, &mut lookup).unwrap_or(false)
            }
        }
    })
}

fn global_representation(
    store: &TypeInterner,
    root: TypeId,
    roles: &NominalRoles,
    generics: &GenericDeclarations,
    signatures: &Reader<'_>,
) -> bool {
    let held = structural_authority(store, root, roles, generics, &mut |_, path| {
        Ok::<_, QueryError>(path.and_then(|p| signatures.get(p).cloned().map(std::sync::Arc::new)))
    })
    .unwrap_or(AuthoritySummary::Unknown);
    let AuthoritySummary::Known(terms) = held else {
        return false;
    };
    if terms.iter().any(|term| match term {
        AuthorityTerm::Capability(path) => matches!(signatures.get(path).map(|s| &s.kind),
            Some(SignatureKind::Type(t)) if t.capability == Some(CapabilityKind::Scoped)),
        _ => false,
    }) {
        return false;
    }
    let mut pending = vec![root];
    let mut seen = HashSet::default();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = store.get(id) else {
            return false;
        };
        if !row.modes.relation.is_global() {
            return false;
        }
        if let TypeKind::Parameter(p) = row.kind {
            let bounds = generics.bounds(p);
            if !bounds.contains(&Bound::Plain) && !bounds.contains(&Bound::Sendable) {
                return false;
            }
        }
        if !matches!(
            row.kind,
            TypeKind::Builtin {
                kind: BuiltinType::Ptr
                    | BuiltinType::FnPtr
                    | BuiltinType::SlotId
                    | BuiltinType::TypeShape,
                ..
            }
        ) {
            pending.extend(row.kind.value_parts());
        }
    }
    true
}

fn flat_storage(
    store: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    signatures: &Reader<'_>,
    active: &mut HashSet<TypeId>,
) -> bool {
    if !active.insert(root) {
        return false;
    }
    let result = match &store.get(root).unwrap().kind {
        TypeKind::Primitive(_) => true,
        TypeKind::Parameter(p) => {
            matches!(generics.get(*p), Some(GenericParameter::Type { bounds, .. }) if bounds.contains(&Bound::Flat))
        }
        TypeKind::Tuple(ts) => ts
            .iter()
            .all(|t| flat_storage(store, *t, generics, signatures, active)),
        TypeKind::Record(fs) => fs
            .iter()
            .all(|f| flat_storage(store, f.ty, generics, signatures, active)),
        TypeKind::Variant(vs) => vs.iter().all(|v| {
            v.fields
                .iter()
                .all(|(_, t)| flat_storage(store, *t, generics, signatures, active))
        }),
        TypeKind::Named {
            definition,
            arguments,
        } => {
            // MEM-12: "no capability/function/handle/resource". Plain is
            // insufficient; prove the actual nominal layout and its arguments.
            matches!(signatures.get(*definition).map(|s| &s.kind), Some(SignatureKind::Type(d)) if d.capability.is_none() && !d.resource)
                && arguments.iter().all(|a| match a {
                    GenericArgument::Type(t) => {
                        flat_storage(store, *t, generics, signatures, active)
                    }
                    _ => true,
                })
                && structural_flat(store, root, generics, &mut |p| {
                    Ok::<_, QueryError>(signatures.get(p).cloned().map(std::sync::Arc::new))
                })
                .unwrap_or(false)
        }
        TypeKind::Builtin { kind, arguments }
            if matches!(
                kind,
                BuiltinType::List | BuiltinType::Inline | BuiltinType::Simd | BuiltinType::Packed
            ) =>
        {
            arguments.iter().all(|a| match a {
                GenericArgument::Type(t) => flat_storage(store, *t, generics, signatures, active),
                _ => true,
            })
        }
        _ => false,
    };
    active.remove(&root);
    result
}

fn cleanup(
    store: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    signatures: &Reader<'_>,
    active: &mut HashSet<TypeId>,
) -> DropAllocation {
    if !active.insert(root) {
        return DropAllocation::NoAlloc;
    }
    let result = cleanup_summary(&store.get(root).unwrap().kind, |leaf| match leaf {
        CleanupLeaf::Component(t) => cleanup(store, t, generics, signatures, active),
        CleanupLeaf::Parameter(p) => parameter_cleanup(generics, p),
        CleanupLeaf::Nominal(path, arguments) => {
            let Some(s) = signatures.get(path) else {
                return DropAllocation::MayAllocate;
            };
            let SignatureKind::Type(d) = &s.kind else {
                return DropAllocation::MayAllocate;
            };
            declared_cleanup(d, store, arguments, |t| {
                cleanup(store, t, generics, signatures, active)
            })
        }
    });
    active.remove(&root);
    result
}
