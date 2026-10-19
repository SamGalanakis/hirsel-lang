//! Independent SER-1 proof from IR declarations. This does not consume the
//! producer's schema descriptor or invoke hirsel-types' schema builder.
use super::*;
use hirsel_types::*;

pub(super) fn declaration(
    s: &Signature,
    signatures: &dyn SignatureLookup,
) -> Result<bool, QueryError> {
    let SignatureKind::Type(d) = &s.kind else {
        return Ok(true);
    };
    if !d.serializable && !d.bounds.contains(&Bound::Serializable) {
        return Ok(true);
    }
    if s.definition.is_core("Path") {
        return Ok(text_path(d, &[]));
    }
    let parametric = matches!(&d.authority, AuthoritySummary::Known(terms)
        if terms.iter().all(|t| matches!(t, AuthorityTerm::TypeParameter(_))));
    if !d.serializable && !s.metadata.alias
        || d.capability.is_some()
        || d.resource
        || d.drop.is_some()
        || d.fact.is_some()
        || s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
        || d.authority != AuthoritySummary::inert() && !(d.serializable && parametric)
    {
        return Ok(false);
    }
    let Some(root) = d.representation else {
        return Ok(false);
    };
    // A published Serializable bound holds at every instantiation: it is proved with
    // only the declared parameter bounds.
    if d.bounds.contains(&Bound::Serializable) {
        return Ok(d.authority == AuthoritySummary::inert()
            && eligible(
                &s.types,
                root,
                &GenericDeclarations::signature(&d.generics),
                signatures,
            )?);
    }
    // An opt-in without that bound is conditional: its representation is proved with
    // every type parameter admitted, and each use proves its arguments after substitution.
    let generics: Vec<_> = d
        .generics
        .iter()
        .map(|g| match g {
            GenericParameter::Type { name, bounds } => GenericParameter::Type {
                name: name.clone(),
                bounds: bounds
                    .iter()
                    .copied()
                    .chain([Bound::Serializable])
                    .collect(),
            },
            g => g.clone(),
        })
        .collect();
    eligible(
        &s.types,
        root,
        &GenericDeclarations::signature(&generics),
        signatures,
    )
}

/// A core Path is data, not authority: an inert, nongeneric opaque handle whose schema is
/// its text. The authority it is used with stays in a Dir, which is never serializable.
fn text_path(d: &TypeDefinition, arguments: &[GenericArgument]) -> bool {
    d.serializable
        && d.generics.is_empty()
        && arguments.is_empty()
        && d.representation.is_none()
        && d.capability.is_none()
        && !d.resource
        && d.drop.is_none()
        && d.fact.is_none()
        && d.authority == AuthoritySummary::inert()
}

pub(super) fn eligible(
    source: &TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    signatures: &dyn SignatureLookup,
) -> Result<bool, QueryError> {
    let mut store = source.clone();
    let mut pending = vec![(root, false)];
    let mut seen = BTreeSet::new();
    let mut expansions = NominalExpansions::default();
    while let Some((id, exit)) = pending.pop() {
        if exit {
            expansions.leave(id, 0);
            continue;
        }
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = store.get(id).cloned() else {
            return Ok(false);
        };
        if !row.brand.alternatives.is_empty() || row.erased_captures.is_some() {
            return Ok(false);
        }
        match row.kind {
            TypeKind::Error | TypeKind::Function(_) | TypeKind::Primitive(Primitive::Never) => {
                return Ok(false);
            }
            TypeKind::Primitive(_) => (),
            TypeKind::Parameter(p) => {
                if !generics.bounds(p).contains(&Bound::Serializable) {
                    return Ok(false);
                }
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let Some(s) = signatures.get(&definition) else {
                    return Ok(false);
                };
                let SignatureKind::Type(d) = &s.kind else {
                    return Ok(false);
                };
                if definition.is_core("Path") {
                    if !text_path(d, &arguments) {
                        return Ok(false);
                    }
                    continue;
                }
                // A generic type declared serializable need not bound its parameters, so
                // its ordinary declaration still accepts File, resources, etc. Eligibility
                // is proved after substitution.
                let conditional = d.serializable
                    && matches!(&d.authority, AuthoritySummary::Known(terms)
                        if terms.iter().all(|t| matches!(t, AuthorityTerm::TypeParameter(_))));
                let data_authority = d.authority == AuthoritySummary::inert() || conditional;
                if d.capability.is_some()
                    || d.resource
                    || d.drop.is_some()
                    || d.fact.is_some()
                    || s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
                    || !data_authority
                    || (!s.metadata.alias && !d.serializable)
                    || d.generics.len() != arguments.len()
                    || !d.generics.iter().zip(&arguments).all(|(g, a)| {
                        matches!(
                            (g, a),
                            (GenericParameter::Type { .. }, GenericArgument::Type(_))
                                | (GenericParameter::Size { .. }, GenericArgument::Size(_))
                        )
                    })
                {
                    return Ok(false);
                }
                if !expansions.enter(id, 0, |_| &store)? {
                    continue;
                }
                pending.push((id, true));
                let Some(root) = d.representation else {
                    return Ok(false);
                };
                let Ok(root) = substitute_type(
                    &s.types,
                    root,
                    &mut store,
                    Substitution {
                        binder: Some(GenericBinder::Bound(0)),
                        arguments: &arguments,
                        authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                        roles: &s.metadata.roles,
                    },
                ) else {
                    return Ok(false);
                };
                pending.push((root, false));
            }
            TypeKind::Builtin { kind, arguments } => {
                let (types, size) = match kind {
                    BuiltinType::Array
                    | BuiltinType::List
                    | BuiltinType::Set
                    | BuiltinType::Packed => (1, false),
                    BuiltinType::Map => (2, false),
                    BuiltinType::Inline | BuiltinType::Simd => (1, true),
                    _ => return Ok(false),
                };
                if arguments.len() != types + usize::from(size)
                    || !arguments[..types]
                        .iter()
                        .all(|a| matches!(a, GenericArgument::Type(_)))
                    || size && !matches!(arguments[types], GenericArgument::Size(_))
                {
                    return Ok(false);
                }
                pending.extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some((*t, false)),
                    _ => None,
                }));
            }
            kind => pending.extend(kind.value_parts().into_iter().map(|t| (t, false))),
        }
    }
    Ok(true)
}
