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
    if !d.serializable && !s.metadata.alias
        || d.capability.is_some()
        || d.resource
        || d.drop.is_some()
        || d.fact.is_some()
        || s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
        || d.authority != AuthoritySummary::inert()
    {
        return Ok(false);
    }
    let Some(root) = d.representation else {
        return Ok(false);
    };
    eligible(
        &s.types,
        root,
        &GenericDeclarations::signature(&d.generics),
        signatures,
    )
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
                // SER-1 enumerates these transparent core data constructors;
                // their ordinary generic declarations must still accept File,
                // resources, etc. Eligibility is proved after substitution.
                let builtin = definition.0.package.name == "core"
                    && definition.0.package.version == "1"
                    && definition.0.modules.is_empty()
                    && matches!(definition.0.name.as_str(), "Option" | "Result");
                let data_authority = d.authority == AuthoritySummary::inert()
                    || builtin
                        && matches!(&d.authority, AuthoritySummary::Known(terms)
                        if terms.iter().all(|t| matches!(t, AuthorityTerm::TypeParameter(_))));
                if d.capability.is_some()
                    || d.resource
                    || d.drop.is_some()
                    || d.fact.is_some()
                    || s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
                    || !data_authority
                    || (!s.metadata.alias && !d.serializable && !builtin)
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
                if !expansions.enter(
                    id,
                    0,
                    |a, b| semantic_equal(&store, a, &store, b).unwrap_or(false),
                    |a, b| nominal_arguments_grow(&store, a, &store, b),
                )? {
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
