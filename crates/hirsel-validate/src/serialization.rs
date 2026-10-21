//! Independent SER-1 proof from IR declarations. This does not consume the
//! producer's schema descriptor or invoke hirsel-types' schema builder.
use super::*;
use hirsel_types::*;

pub(super) fn declaration(
    t: &TypeInterner,
    memo: &TypeMemo,
    s: &Signature,
    signatures: &Reader<'_>,
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
    if let Some(check) = &d.serializable_check {
        return checked(t, memo, s, d, check, signatures);
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
                t,
                memo,
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
        t,
        memo,
        root,
        &GenericDeclarations::signature(&generics),
        signatures,
    )
}

/// An opaque type serializable through its check: a nongeneric, inert, opted-in opaque
/// declaration with no structural Serializable bound, whose published representation
/// qualifies, and whose check is `@noalloc fn(value: T) -> bool uses {}` in its module.
fn checked(
    t: &TypeInterner,
    memo: &TypeMemo,
    s: &Signature,
    d: &TypeDefinition,
    check: &SerializableCheck,
    signatures: &Reader<'_>,
) -> Result<bool, QueryError> {
    if !d.serializable
        || s.metadata.visibility != hirsel_syntax::Visibility::PublicOpaque
        || !d.generics.is_empty()
        || d.bounds.contains(&Bound::Serializable)
        || d.authority != AuthoritySummary::inert()
        || d.capability.is_some()
        || d.resource
        || d.drop.is_some()
        || d.fact.is_some()
        || !check_function(t, s.definition.path(), &check.function, signatures)
    {
        return Ok(false);
    }
    eligible(
        t,
        memo,
        check.representation,
        &GenericDeclarations::default(),
        signatures,
    )
}

/// The exact check header for the opaque `owner`, declared in `owner`'s module.
fn check_function(
    t: &TypeInterner,
    owner: &DefPath,
    function: &DefPath,
    signatures: &Reader<'_>,
) -> bool {
    if function.0.package != owner.0.package || function.0.modules != owner.0.modules {
        return false;
    }
    let Some(f) = signatures.get(function) else {
        return false;
    };
    let SignatureKind::Function(header) = &f.kind else {
        return false;
    };
    let [value] = header.parameters.as_slice() else {
        return false;
    };
    header.generics.is_empty()
        && value.passing == hirsel_syntax::PassingMode::Borrow
        && value.label.is_none()
        && matches!(t.get(value.ty).map(|t| &t.kind),
            Some(TypeKind::Named { definition, arguments }) if definition.path() == owner && arguments.is_empty())
        && matches!(
            t.get(header.result).map(|t| &t.kind),
            Some(TypeKind::Primitive(Primitive::Bool))
        )
        && header.error.is_none()
        && header.allocation == Allocation::NoAlloc
        && header.uses == Uses::Only(AuthoritySummary::inert())
        && header.foreign.is_none()
        && !header.incomplete
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
    t: &TypeInterner,
    memo: &TypeMemo,
    root: TypeId,
    generics: &GenericDeclarations,
    signatures: &Reader<'_>,
) -> Result<bool, QueryError> {
    let mut pending = vec![(root, false)];
    let mut seen = HashSet::default();
    let mut expansions = NominalExpansions::default();
    while let Some((id, exit)) = pending.pop() {
        if exit {
            expansions.leave(id, 0);
            continue;
        }
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = t.get(id).cloned() else {
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
                let Some(s) = signatures.get(definition) else {
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
                // An opaque type qualifies only through its check, proved with its
                // declaration; its use opens the representation it publishes for it.
                let check = d.serializable_check.as_ref();
                if d.capability.is_some()
                    || d.resource
                    || d.drop.is_some()
                    || d.fact.is_some()
                    || (s.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque)
                        != check.is_some()
                    || check.is_some_and(|c| {
                        !d.generics.is_empty()
                            || d.authority != AuthoritySummary::inert()
                            || !check_function(t, definition.path(), &c.function, signatures)
                    })
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
                if !expansions.enter(id, 0, t)? {
                    continue;
                }
                pending.push((id, true));
                let Some(root) = check.map(|c| c.representation).or(d.representation) else {
                    return Ok(false);
                };
                let Ok(root) = hirsel_types::substitute(
                    t,
                    memo,
                    root,
                    &Substitution {
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
