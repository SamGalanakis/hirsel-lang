//! Declaration proofs for equality and held authority (GEN-1/CAP-2).
type IdSet<T> = std::collections::HashSet<T, hirsel_intern::IdHasher>;
use crate::*;

/// Data/Key are representation properties. Parameter evidence belongs to the
/// occurrence's lexical scope; a bare ordinal cannot select that scope.
/// Missing declarations and recovery prove nothing. Non-regular polymorphic
/// recursion is an operational tooling failure, never negative evidence.
pub fn structural_equality(
    source: &TypeInterner,
    root: TypeId,
    bound: Bound,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    assert!(matches!(bound, Bound::Data | Bound::Key));
    let proves = |bounds: &[Bound]| {
        bounds.contains(&bound) || (bound == Bound::Data && bounds.contains(&Bound::Key))
    };
    if source.get(root).is_none() {
        return Ok(false);
    }
    let store = source;
    let mut pending = vec![(root, false)];
    let mut seen = IdSet::default();
    let mut nominals = NominalExpansions::default();
    while let Some((id, exit)) = pending.pop() {
        if exit {
            nominals.leave(id, 0);
            continue;
        }
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = store.get(id).cloned() else {
            return Ok(false);
        };
        match row.kind {
            TypeKind::Error | TypeKind::Function(_) | TypeKind::Primitive(Primitive::Never) => {
                return Ok(false);
            }
            TypeKind::Primitive(Primitive::F32 | Primitive::F64) if bound == Bound::Key => {
                return Ok(false);
            }
            TypeKind::Primitive(_) => (),
            // A declaration parameter without a supplied actual has no proof.
            TypeKind::Parameter(p) => {
                if !proves(declarations.bounds(p)) {
                    return Ok(false);
                }
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let Some(signature) = lookup(definition)? else {
                    return Ok(false);
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    return Ok(false);
                };
                if declaration.resource || declaration.capability.is_some() {
                    return Ok(false);
                }
                if proves(&declaration.bounds) {
                    continue;
                }
                if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                    return Ok(false);
                }
                if !nominals.enter(id, 0, store)? {
                    continue;
                }
                pending.push((id, true));
                let Some(representation) = declaration.representation else {
                    return Ok(false);
                };
                let Ok(representation) = crate::graph::expand(
                    store,
                    representation,
                    &arguments,
                    &signature.metadata.roles,
                ) else {
                    return Ok(false);
                };
                pending.push((representation, false));
            }
            TypeKind::Builtin { kind, arguments } => {
                // GEN-1, spec #41: Shared/Slots/TypeShape lack structural operations;
                // SortedMap owns its function-bearing Order dictionary.
                if !matches!(
                    kind,
                    BuiltinType::Array
                        | BuiltinType::List
                        | BuiltinType::Inline
                        | BuiltinType::Simd
                        | BuiltinType::Packed
                        | BuiltinType::Map
                        | BuiltinType::Set
                ) {
                    return Ok(false);
                }
                pending.extend(arguments.into_iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some((t, false)),
                    GenericArgument::Size(_) => None,
                }));
            }
            TypeKind::Tuple(ts) => pending.extend(ts.into_iter().map(|t| (t, false))),
            TypeKind::Record(fs) => pending.extend(fs.into_iter().map(|f| (f.ty, false))),
            TypeKind::Variant(vs) => pending.extend(
                vs.into_iter()
                    .flat_map(|v| v.fields.into_iter().map(|f| (f.1, false))),
            ),
            TypeKind::Box(t) => pending.push((t, false)),
        }
    }
    Ok(true)
}

/// These bounds exclude held authority, independently of copy/sendable modes.
pub fn bounds_imply_inert(bounds: &[Bound]) -> bool {
    bounds.iter().any(|b| {
        matches!(
            b,
            Bound::Inert | Bound::Flat | Bound::Data | Bound::Key | Bound::Serializable
        )
    })
}

/// Held authority comes from declarations. The producer publishes field joins
/// union explicit uses; consumers specialize that summary, never re-open fields.
/// A capability intrinsically contributes itself (CAP-2); an ambient heap token
/// is the declared exception. Phantom arguments do not contribute authority.
///
/// `lookup` receives the occurrence and its nominal path, or None for a builtin
/// with an optional resolver-selected declaration. `declarations` supplies
/// exact lexical evidence. Callable parameter references remain symbolic: evaluating
/// them against checked value bindings belongs to the authority pass.
pub fn structural_authority<E>(
    store: &TypeInterner,
    root: TypeId,
    roles: &NominalRoles,
    declarations: &GenericDeclarations,
    lookup: &mut impl FnMut(TypeId, Option<DefId>) -> Result<Option<SignatureRef>, E>,
) -> Result<AuthoritySummary, E> {
    let mut pending = vec![root];
    let mut seen = IdSet::default();
    let mut summary = AuthoritySummary::inert();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = store.get(id) else {
            return Ok(AuthoritySummary::Unknown);
        };
        if row.is_heap_target(roles) {
            continue;
        }
        match &row.kind {
            TypeKind::Error => return Ok(AuthoritySummary::Unknown),
            TypeKind::Primitive(_) => (),
            TypeKind::Parameter(p) => {
                if !bounds_imply_inert(declarations.bounds(*p)) {
                    summary = summary.join(AuthoritySummary::Known(
                        vec![AuthorityTerm::TypeParameter(*p)].into(),
                    ));
                }
            }
            TypeKind::Function(f) => summary = summary.join(f.authority.clone()),
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let Some(signature) = lookup(id, Some(*definition))? else {
                    return Ok(AuthoritySummary::Unknown);
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    return Ok(AuthoritySummary::Unknown);
                };
                if declaration.capability.is_some() {
                    summary = summary.join(AuthoritySummary::Known(
                        vec![AuthorityTerm::Capability(definition.path().clone())].into(),
                    ));
                    // CAP-5: actuals and owner ceilings do not refine the
                    // nominal permission of a whole seal.
                    continue;
                }
                if !specialize_authority(
                    &declaration.authority,
                    arguments,
                    &mut summary,
                    &mut pending,
                ) {
                    return Ok(AuthoritySummary::Unknown);
                }
                // CAP-2: an erased carrier also holds its value's own erased
                // evidence, unknown for a written type, as an Iter does.
                if declaration.captures.is_some() {
                    summary = summary.join(
                        row.erased_captures
                            .as_ref()
                            .map(ErasedCaptures::authority)
                            .unwrap_or(AuthoritySummary::Unknown),
                    );
                }
            }
            TypeKind::Builtin { kind, arguments } => {
                if *kind == BuiltinType::Iter {
                    summary = summary.join(
                        row.erased_captures
                            .as_ref()
                            .map(ErasedCaptures::authority)
                            .unwrap_or(AuthoritySummary::Unknown),
                    );
                } else if let Some(signature) = lookup(id, None)? {
                    let SignatureKind::Type(declaration) = &signature.kind else {
                        return Ok(AuthoritySummary::Unknown);
                    };
                    if !specialize_authority(
                        &declaration.authority,
                        arguments,
                        &mut summary,
                        &mut pending,
                    ) {
                        return Ok(AuthoritySummary::Unknown);
                    }
                } else {
                    summary = summary.join(builtin_held_authority(*kind, roles));
                    if !matches!(
                        kind,
                        BuiltinType::Ptr
                            | BuiltinType::FnPtr
                            | BuiltinType::SlotId
                            | BuiltinType::TypeShape
                    ) {
                        pending.extend(arguments.iter().filter_map(|a| match a {
                            GenericArgument::Type(t) => Some(*t),
                            GenericArgument::Size(_) => None,
                        }));
                    }
                }
            }
            TypeKind::Tuple(ts) => pending.extend(ts),
            TypeKind::Record(fs) => pending.extend(fs.iter().map(|f| f.ty)),
            TypeKind::Variant(vs) => {
                pending.extend(vs.iter().flat_map(|v| v.fields.iter().map(|f| f.1)))
            }
            TypeKind::Box(t) => pending.push(*t),
        }
    }
    Ok(summary)
}
fn specialize_authority(
    declaration: &AuthoritySummary,
    arguments: &[GenericArgument],
    summary: &mut AuthoritySummary,
    pending: &mut Vec<TypeId>,
) -> bool {
    let AuthoritySummary::Known(terms) = declaration else {
        return false;
    };
    for term in terms {
        match term {
            AuthorityTerm::TypeParameter(p) if p.binder() == GenericBinder::Bound(0) => {
                match arguments.get(p.parameter().0 as usize) {
                    Some(GenericArgument::Type(t)) => pending.push(*t),
                    _ => return false,
                }
            }
            term => {
                *summary = summary
                    .clone()
                    .join(AuthoritySummary::Known(vec![term.clone()].into()))
            }
        }
    }
    true
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn row(kind: TypeKind) -> Type {
        Type::with_kind(kind)
    }
    /// The binder of the `n`th test item.
    pub(crate) fn item(n: u32) -> SiteRef {
        hirsel_syntax::LexicalOwner::first(&path(&format!("item{n}"))).site(NameSite::Item)
    }
    pub(crate) fn path(name: &str) -> DefPath {
        DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "proof".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        })
    }
    pub(crate) fn def(name: &str) -> DefId {
        DefId::intern(&path(name))
    }
    pub(crate) fn nominal(representation: Option<TypeId>, bounds: Vec<Bound>) -> SignatureRef {
        std::sync::Arc::new(Signature {
            definition: def("Wrap"),
            metadata: SignatureMetadata::default(),
            fingerprint: ContentHash::default(),
            kind: SignatureKind::Type(Box::new(TypeDefinition {
                loop_source: None,
                format: None,
                generics: vec![],
                representation,
                bounds,
                authority: AuthoritySummary::Known(
                    vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                        TypeParamId(0),
                    ))]
                    .into(),
                ),
                capability: None,
                implementation_authority: None,
                serializable: false,
                serializable_check: None,
                layout: Layout::default(),
                resource: false,
                distinct: false,
                drop: None,
                fact: None,
                captures: None,
                destructor_allocation: Allocation::NoAlloc,
            })),
        })
    }
    #[test]
    fn scoped_evidence_and_intrinsic_capability_authority_are_independent() {
        let store = TypeInterner::new(None);
        let a = store.intern(row(TypeKind::Parameter(TypeParameterRef::body(
            item(0),
            TypeParamId(0),
        ))));
        let b = store.intern(row(TypeKind::Parameter(TypeParameterRef::body(
            item(1),
            TypeParamId(0),
        ))));
        let mut declarations = GenericDeclarations::default();
        declarations.insert(
            item(0),
            vec![GenericParameter::Type {
                name: "A".into(),
                bounds: vec![Bound::Data].into(),
            }],
        );
        declarations.insert(
            item(1),
            vec![GenericParameter::Type {
                name: "B".into(),
                bounds: vec![].into(),
            }],
        );
        let root = store.intern(row(TypeKind::Tuple(vec![a, b].into())));
        assert_eq!(
            structural_authority(
                &store,
                root,
                &NominalRoles::default(),
                &declarations,
                &mut |_, _| Ok::<_, QueryError>(None)
            )
            .unwrap(),
            AuthoritySummary::Known(
                vec![AuthorityTerm::TypeParameter(TypeParameterRef::body(
                    item(1),
                    TypeParamId(0)
                ))]
                .into()
            )
        );
        assert!(
            !structural_equality(&store, root, Bound::Data, &declarations, &mut |_| Ok::<
                _,
                QueryError,
            >(
                None
            ))
            .unwrap()
        );
        let mut signature = nominal(None, vec![]).as_ref().clone();
        let SignatureKind::Type(d) = &mut signature.kind else {
            unreachable!()
        };
        d.capability = Some(CapabilityKind::Global);
        d.authority = AuthoritySummary::inert();
        let root = store.intern(row(TypeKind::Named {
            definition: def("Wrap"),
            arguments: vec![].into(),
        }));
        assert_eq!(
            structural_authority(
                &store,
                root,
                &NominalRoles::default(),
                &GenericDeclarations::default(),
                &mut |_, _| Ok::<_, QueryError>(Some(std::sync::Arc::new(signature.clone())))
            )
            .unwrap(),
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Wrap"))].into())
        );
        let error = store.intern(row(TypeKind::Error));
        assert_eq!(
            structural_authority(
                &store,
                error,
                &NominalRoles::default(),
                &GenericDeclarations::default(),
                &mut |_, _| Ok::<_, QueryError>(None)
            )
            .unwrap(),
            AuthoritySummary::Unknown
        );
    }
    #[test]
    fn nominal_transport_preserves_scoped_parameter_proofs() {
        let declaration = TypeInterner::new(None);
        let p = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let signature = nominal(Some(p), vec![]);
        for composite in [false, true] {
            for both in [false, true] {
                let store = std::sync::Arc::clone(&declaration);
                let bad = store.intern(row(TypeKind::Parameter(TypeParameterRef::body(
                    item(0),
                    TypeParamId(0),
                ))));
                let good = store.intern(row(TypeKind::Parameter(TypeParameterRef::body(
                    item(1),
                    TypeParamId(0),
                ))));
                let mut roots = vec![];
                for argument in [bad, good] {
                    let argument = if composite {
                        store.intern(row(TypeKind::Builtin {
                            kind: BuiltinType::Array,
                            arguments: vec![GenericArgument::Type(argument)].into(),
                        }))
                    } else {
                        argument
                    };
                    roots.push(store.intern(row(TypeKind::Named {
                        definition: def("Wrap"),
                        arguments: vec![GenericArgument::Type(argument)].into(),
                    })));
                }
                let root = store.intern(row(TypeKind::Tuple(roots.into())));
                for bound in [Bound::Data, Bound::Key] {
                    let actual = structural_equality(
                        &store,
                        root,
                        bound,
                        &GenericDeclarations::from_binders([
                            (
                                GenericBinder::Body(item(0)),
                                vec![GenericParameter::Type {
                                    name: "Bad".into(),
                                    bounds: if both {
                                        vec![bound].into()
                                    } else {
                                        vec![].into()
                                    },
                                }],
                            ),
                            (
                                GenericBinder::Body(item(1)),
                                vec![GenericParameter::Type {
                                    name: "Good".into(),
                                    bounds: vec![bound].into(),
                                }],
                            ),
                        ]),
                        &mut |_| Ok::<_, QueryError>(Some(signature.clone())),
                    )
                    .unwrap();
                    assert_eq!(actual, both, "composite={composite}, bound={bound:?}");
                }
            }
        }
    }
}

#[cfg(test)]
mod finite_nominal_tests {
    use super::tests::{def, nominal, row};
    use super::*;
    #[test]
    fn finite_nominal_depth_is_not_a_property_failure() {
        for depth in [128, 129, 200] {
            let mut signatures = std::collections::BTreeMap::new();
            let store = TypeInterner::new(None);
            for level in 0..depth {
                let parameter = store.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
                    TypeParamId(0),
                ))));
                let representation = if level == 0 {
                    parameter
                } else {
                    store.intern(row(TypeKind::Named {
                        definition: def(&format!("D{}", level - 1)),
                        arguments: vec![GenericArgument::Type(parameter)].into(),
                    }))
                };
                let mut signature = nominal(Some(representation), vec![]).as_ref().clone();
                signature.definition = def(&format!("D{level}"));
                signatures.insert(signature.definition, std::sync::Arc::new(signature));
            }
            let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
            let root = store.intern(row(TypeKind::Named {
                definition: def(&format!("D{}", depth - 1)),
                arguments: vec![GenericArgument::Type(scalar)].into(),
            }));
            let generics = GenericDeclarations::default();
            let mut lookup = |p: hirsel_syntax::DefId| Ok(signatures.get(&p).cloned());
            for property in [
                StructuralProperty::Plain,
                StructuralProperty::Copyable,
                StructuralProperty::Sendable,
            ] {
                assert!(
                    structural_property(&store, root, property, &generics, &mut lookup).unwrap(),
                    "{depth} levels: {property:?}"
                );
            }
            for bound in [Bound::Data, Bound::Key] {
                assert!(
                    structural_equality(&store, root, bound, &generics, &mut lookup).unwrap(),
                    "{depth} levels: {bound:?}"
                );
            }
            assert!(structural_flat(&store, root, &generics, &mut lookup).unwrap());
        }
    }
    #[test]
    fn structural_properties_accept_finished_siblings_at_the_old_limit() {
        let declaration = TypeInterner::new(None);
        let parameter = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let signature = nominal(Some(parameter), vec![]);
        for width in [128, 129] {
            let store = std::sync::Arc::clone(&declaration);
            let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
            let fields = (0..width)
                .map(|size| {
                    let actual = store.intern(row(TypeKind::Builtin {
                        kind: BuiltinType::Inline,
                        arguments: vec![
                            GenericArgument::Type(scalar),
                            GenericArgument::Size(Size::Constant(size)),
                        ]
                        .into(),
                    }));
                    store.intern(row(TypeKind::Named {
                        definition: def("Wrap"),
                        arguments: vec![GenericArgument::Type(actual)].into(),
                    }))
                })
                .collect::<Vec<_>>();
            let root = store.intern(row(TypeKind::Tuple(fields.into())));
            for property in [
                StructuralProperty::Plain,
                StructuralProperty::Copyable,
                StructuralProperty::Sendable,
            ] {
                assert!(
                    structural_property(
                        &store,
                        root,
                        property,
                        &GenericDeclarations::default(),
                        &mut |_| Ok::<_, QueryError>(Some(signature.clone()))
                    )
                    .unwrap(),
                    "{width} siblings: {property:?}"
                );
            }
            assert!(
                structural_flat(
                    &store,
                    root,
                    &GenericDeclarations::default(),
                    &mut |_| Ok::<_, QueryError>(Some(signature.clone()))
                )
                .unwrap()
            );
        }
    }
    #[test]
    fn finite_sibling_nominals_do_not_spend_recursive_expansion_depth() {
        let declaration = TypeInterner::new(None);
        let parameter = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let signature = nominal(Some(parameter), vec![]);
        for rejected in [false, true] {
            let store = std::sync::Arc::clone(&declaration);
            let scalar = store.intern(row(if rejected {
                TypeKind::Error
            } else {
                TypeKind::Primitive(Primitive::U8)
            }));
            let mut fields = vec![];
            for width in 0..130 {
                let actual = store.intern(row(TypeKind::Builtin {
                    kind: BuiltinType::Inline,
                    arguments: vec![
                        GenericArgument::Type(scalar),
                        GenericArgument::Size(Size::Constant(width)),
                    ]
                    .into(),
                }));
                fields.push(store.intern(row(TypeKind::Named {
                    definition: def("Wrap"),
                    arguments: vec![GenericArgument::Type(actual)].into(),
                })));
            }
            let root = store.intern(row(TypeKind::Tuple(fields.into())));
            for bound in [Bound::Data, Bound::Key] {
                assert_eq!(
                    structural_equality(
                        &store,
                        root,
                        bound,
                        &GenericDeclarations::default(),
                        &mut |_| Ok::<_, QueryError>(Some(signature.clone()))
                    )
                    .unwrap(),
                    !rejected
                );
            }
        }
    }
}
