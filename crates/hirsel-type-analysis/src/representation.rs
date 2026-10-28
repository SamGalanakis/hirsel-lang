//! Nominal representation expansion for stage-owned layout walks.
use hirsel_syntax::DefId;
use hirsel_types::*;
use std::collections::HashSet;

/// Why a representation walk stopped before reaching a structural row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepresentationFailure {
    MissingType,
    MissingDeclaration,
    NotTypeDeclaration,
    MissingRepresentation,
    RecursiveRepresentation,
    Graph(GraphError),
    Query(QueryError),
}

/// The last row reached, plus the exact failure. A caller may retain that row
/// conservatively, but operational query failures must still propagate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepresentationError {
    pub root: TypeId,
    pub failure: RepresentationFailure,
}

/// Expand successive nominal representations in caller-owned storage.
/// `steps: Some(1)` performs one substitution; `None` reaches a structural row.
/// Zero steps returns the input without a declaration read. One phase-owned
/// callback supplies the declaration and actual argument authorities, so both
/// can use the same mutable query context without competing borrows. It receives
/// the scratch store rather than requiring a store clone. This preserves J's
/// evaluated authority and physical layout walks' conservative Unknown authority.
/// The guard spends its budget only on active polymorphic constructor growth.
pub fn expand_representation(
    store: &TypeInterner,
    memo: &TypeMemo,
    mut root: TypeId,
    lookup: &mut impl FnMut(
        &TypeInterner,
        DefId,
        &[GenericArgument],
    ) -> Result<Option<(SignatureRef, Vec<AuthoritySummary>)>, QueryError>,
    steps: Option<usize>,
) -> Result<TypeId, RepresentationError> {
    let mut expansions = NominalExpansions::default();
    let mut remaining = steps;
    loop {
        let error = |failure| RepresentationError { root, failure };
        let row = store
            .get(root)
            .ok_or_else(|| error(RepresentationFailure::MissingType))?;
        if remaining == Some(0) {
            return Ok(root);
        }
        let TypeKind::Named {
            definition,
            arguments,
        } = &row.kind
        else {
            return Ok(root);
        };
        let definition = *definition;
        let arguments = arguments.clone();
        if !expansions
            .enter(root, 0, store)
            .map_err(|e| error(RepresentationFailure::Query(e)))?
        {
            return Err(error(RepresentationFailure::RecursiveRepresentation));
        }
        let (signature, authorities) = lookup(store, definition, &arguments)
            .map_err(|e| error(RepresentationFailure::Query(e)))?
            .ok_or_else(|| error(RepresentationFailure::MissingDeclaration))?;
        let SignatureKind::Type(declaration) = &signature.kind else {
            return Err(error(RepresentationFailure::NotTypeDeclaration));
        };
        let representation = declaration
            .representation
            .ok_or_else(|| error(RepresentationFailure::MissingRepresentation))?;
        root = substitute(
            store,
            memo,
            representation,
            &Substitution {
                binder: Some(GenericBinder::Bound(0)),
                arguments: &arguments,
                authorities: &authorities,
                roles: &signature.metadata.roles,
            },
        )
        .map_err(|e| error(RepresentationFailure::Graph(e)))?;
        remaining = remaining.map(|n| n - 1);
    }
}

/// MODE-1, PROV-4: whether storage of type `holder` could come to hold a part
/// of a value of type `content`, by type: some counted part of each has the
/// same shape. Nominal parts are read through their representations. A
/// generic, callable, erased or unexpandable part could be anything, and so
/// may hold anything; plain scalars hold no storage.
pub fn may_hold_content(
    store: &TypeInterner,
    memo: &TypeMemo,
    holder: TypeId,
    content: TypeId,
    lookup: &mut impl FnMut(DefId) -> Option<SignatureRef>,
) -> bool {
    let (Some(holders), Some(contents)) = (
        counted_parts(store, memo, holder, lookup),
        counted_parts(store, memo, content, lookup),
    ) else {
        return true;
    };
    holders
        .iter()
        .any(|h| contents.iter().any(|c| same_shape(store, *h, *c, 0)))
}
/// Every counted part of `root`, nominal representations included, or None
/// when some part could be anything.
fn counted_parts(
    store: &TypeInterner,
    memo: &TypeMemo,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Option<SignatureRef>,
) -> Option<Vec<TypeId>> {
    const LIMIT: usize = 256;
    let mut parts = vec![];
    let mut pending = vec![root];
    let mut seen = HashSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) {
            continue;
        }
        if seen.len() > LIMIT {
            return None;
        }
        let kind = store.get(ty)?.kind.clone();
        match &kind {
            TypeKind::Primitive(Primitive::String | Primitive::Bytes) => parts.push(ty),
            TypeKind::Primitive(_) => {}
            TypeKind::Parameter(_)
            | TypeKind::Function(_)
            | TypeKind::Error
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            } => return None,
            TypeKind::Named { definition, .. } => {
                let signature = lookup(*definition)?;
                let SignatureKind::Type(declaration) = &signature.kind else {
                    return None;
                };
                // A checked public Plain bound proves there is no counted
                // storage. An opaque export need not reveal its representation
                // to preserve this proof (for example std's checked Date).
                if declaration.bounds.contains(&Bound::Plain) {
                    continue;
                }
                parts.push(ty);
                let representation = expand_representation(
                    store,
                    memo,
                    ty,
                    &mut |_, path, arguments| {
                        Ok(lookup(path).map(|signature| {
                            (signature, vec![AuthoritySummary::Unknown; arguments.len()])
                        }))
                    },
                    None,
                )
                .ok()?;
                let row = store.get(representation)?;
                if matches!(row.kind, TypeKind::Named { .. }) {
                    return None;
                }
                pending.push(representation);
            }
            _ => {
                parts.push(ty);
                pending.extend(kind.value_parts());
            }
        }
    }
    Some(parts)
}
/// Structural equality of two types, ignoring modes, brands and relations.
fn same_shape(store: &TypeInterner, a: TypeId, b: TypeId, depth: u32) -> bool {
    if a == b || depth > 16 {
        return true;
    }
    let (Some(x), Some(y)) = (store.get(a), store.get(b)) else {
        return true;
    };
    let arguments = |p: &[GenericArgument], q: &[GenericArgument]| {
        p.len() == q.len()
            && p.iter().zip(q).all(|pair| match pair {
                (GenericArgument::Type(p), GenericArgument::Type(q)) => {
                    same_shape(store, *p, *q, depth + 1)
                }
                (p, q) => p == q,
            })
    };
    let all = |p: &[TypeId], q: &[TypeId]| {
        p.len() == q.len()
            && p.iter()
                .zip(q)
                .all(|(p, q)| same_shape(store, *p, *q, depth + 1))
    };
    match (&x.kind, &y.kind) {
        (TypeKind::Parameter(_) | TypeKind::Function(_) | TypeKind::Error, _)
        | (_, TypeKind::Parameter(_) | TypeKind::Function(_) | TypeKind::Error) => true,
        (TypeKind::Primitive(p), TypeKind::Primitive(q)) => p == q,
        (
            TypeKind::Builtin {
                kind: p,
                arguments: a,
            },
            TypeKind::Builtin {
                kind: q,
                arguments: b,
            },
        ) => p == q && arguments(a, b),
        (
            TypeKind::Named {
                definition: p,
                arguments: a,
            },
            TypeKind::Named {
                definition: q,
                arguments: b,
            },
        ) => p == q && arguments(a, b),
        (TypeKind::Box(p), TypeKind::Box(q)) => same_shape(store, *p, *q, depth + 1),
        (TypeKind::Tuple(_) | TypeKind::Record(_) | TypeKind::Variant(_), _) => {
            std::mem::discriminant(&x.kind) == std::mem::discriminant(&y.kind)
                && all(&x.kind.value_parts(), &y.kind.value_parts())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::{ContentHash, DefPath};

    fn row(kind: TypeKind) -> Type {
        Type::with_kind(kind)
    }
    fn path(name: &str) -> DefPath {
        DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "proof".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        })
    }
    fn nominal(representation: Option<TypeId>, bounds: Vec<Bound>) -> SignatureRef {
        std::sync::Arc::new(Signature {
            definition: DefId::intern(&path("Wrap")),
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
    fn with_unknown_authorities(
        signature: Option<SignatureRef>,
        arguments: &[GenericArgument],
    ) -> Option<(SignatureRef, Vec<AuthoritySummary>)> {
        signature.map(|signature| (signature, vec![AuthoritySummary::Unknown; arguments.len()]))
    }

    #[test]
    fn storage_holds_content_by_counted_shape_through_representations() {
        let list = |store: &TypeInterner, element| {
            store.intern(row(TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(element)].into(),
            }))
        };
        // `Wrap` is a record holding one String.
        let declaration = TypeInterner::new(None);
        let field = declaration.intern(row(TypeKind::Primitive(Primitive::String)));
        let record = declaration.intern(row(TypeKind::Record(
            vec![Field {
                growable: false,
                name: "text".into(),
                ty: field,
            }]
            .into(),
        )));
        let signature = nominal(Some(record), vec![]);
        let store = declaration.clone();
        let bytes = store.intern(row(TypeKind::Primitive(Primitive::U8)));
        let text = store.intern(row(TypeKind::Primitive(Primitive::String)));
        let wrap = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Wrap")),
            arguments: vec![].into(),
        }));
        let generic = store.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let byte_list = list(&store, bytes);
        let text_list = list(&store, text);
        let mut lookup = |_: DefId| Some(signature.clone());
        let memo = TypeMemo::new();
        let mut holds = |store: &TypeInterner, holder, content| {
            may_hold_content(store, &memo, holder, content, &mut lookup)
        };
        assert!(!holds(&store, byte_list, text_list));
        assert!(!holds(&store, byte_list, wrap));
        assert!(holds(&store, text, text_list));
        assert!(holds(&store, text_list, wrap));
        assert!(holds(&store, byte_list, generic));
        let mut missing = |_: DefId| None;
        assert!(may_hold_content(
            &store,
            &memo,
            byte_list,
            wrap,
            &mut missing
        ));
    }

    #[test]
    fn opaque_plain_parts_cannot_carry_mutable_content() {
        let store = TypeInterner::new(None);
        let bytes = store.intern(row(TypeKind::Primitive(Primitive::Bytes)));
        let opaque = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Wrap")),
            arguments: vec![].into(),
        }));
        let holder = store.intern(row(TypeKind::Tuple(vec![opaque].into())));
        let memo = TypeMemo::new();
        // A public Plain contract proves no counted storage, even when the
        // opaque representation is unavailable (as for std's checked Date).
        let plain = nominal(None, vec![Bound::Plain]);
        assert!(!may_hold_content(&store, &memo, holder, bytes, &mut |_| {
            Some(plain.clone())
        }));
        assert!(!may_hold_content(&store, &memo, bytes, holder, &mut |_| {
            Some(plain.clone())
        }));
        let unknown = nominal(None, vec![]);
        assert!(may_hold_content(&store, &memo, holder, bytes, &mut |_| {
            Some(unknown.clone())
        }));
        assert!(may_hold_content(&store, &memo, bytes, holder, &mut |_| {
            Some(unknown.clone())
        }));
    }

    #[test]
    fn representation_steps_preserve_nested_actuals() {
        let declaration = TypeInterner::new(None);
        let parameter = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let signature = nominal(Some(parameter), vec![]);
        let store = std::sync::Arc::clone(&declaration);
        let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
        let mut root = scalar;
        for _ in 0..200 {
            root = store.intern(row(TypeKind::Named {
                definition: DefId::intern(&path("Wrap")),
                arguments: vec![GenericArgument::Type(root)].into(),
            }));
        }
        assert_eq!(
            expand_representation(
                &store,
                &TypeMemo::new(),
                root,
                &mut |_, _, _| panic!("zero-step lookup"),
                Some(0)
            ),
            Ok(root)
        );
        let mut reads = 0;
        let mut lookup = |_: &TypeInterner, _: DefId, arguments: &[GenericArgument]| {
            reads += 1;
            Ok(with_unknown_authorities(Some(signature.clone()), arguments))
        };
        let one =
            expand_representation(&store, &TypeMemo::new(), root, &mut lookup, Some(1)).unwrap();
        assert!(matches!(
            store.get(one).unwrap().kind,
            TypeKind::Named { .. }
        ));
        let full = expand_representation(&store, &TypeMemo::new(), one, &mut lookup, None).unwrap();
        assert_eq!(
            store.get(full).unwrap().kind,
            store.get(scalar).unwrap().kind
        );
        assert_eq!(reads, 200);
    }

    #[test]
    fn representation_stops_keep_the_last_row_and_exact_failure() {
        let store = TypeInterner::new(None);
        let root = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Wrap")),
            arguments: vec![].into(),
        }));
        let opaque = nominal(None, vec![]);
        for (declaration, failure) in [
            (None, RepresentationFailure::MissingDeclaration),
            (Some(opaque), RepresentationFailure::MissingRepresentation),
        ] {
            assert_eq!(
                expand_representation(
                    &store,
                    &TypeMemo::new(),
                    root,
                    &mut |_, _, arguments| Ok(with_unknown_authorities(
                        declaration.clone(),
                        arguments
                    )),
                    None
                ),
                Err(RepresentationError { root, failure })
            );
        }
        assert_eq!(
            expand_representation(
                &store,
                &TypeMemo::new(),
                root,
                &mut |_, _, _| Err(type_proof_limit()),
                None
            ),
            Err(RepresentationError {
                root,
                failure: RepresentationFailure::Query(type_proof_limit())
            })
        );
        let declaration = std::sync::Arc::clone(&store);
        let parameter = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let malformed = nominal(Some(parameter), vec![]);
        assert_eq!(
            expand_representation(
                &store,
                &TypeMemo::new(),
                root,
                &mut |_, _, arguments| Ok(with_unknown_authorities(
                    Some(malformed.clone()),
                    arguments
                )),
                None
            ),
            Err(RepresentationError {
                root,
                failure: RepresentationFailure::Graph(GraphError::MissingArgument(0))
            })
        );
        // The accepted twin supplies the missing argument.
        let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
        let supplied = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Wrap")),
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        assert!(
            expand_representation(
                &store,
                &TypeMemo::new(),
                supplied,
                &mut |_, _, arguments| Ok(with_unknown_authorities(
                    Some(malformed.clone()),
                    arguments
                )),
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn representation_cycles_stop_and_polymorphic_growth_remains_operational() {
        for growing in [false, true] {
            let declaration = TypeInterner::new(None);
            let parameter = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let argument = if growing {
                declaration.intern(row(TypeKind::Box(parameter)))
            } else {
                parameter
            };
            let next = declaration.intern(row(TypeKind::Named {
                definition: DefId::intern(&path("Wrap")),
                arguments: vec![GenericArgument::Type(argument)].into(),
            }));
            let signature = nominal(Some(next), vec![]);
            let store = std::sync::Arc::clone(&declaration);
            let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
            let root = store.intern(row(TypeKind::Named {
                definition: DefId::intern(&path("Wrap")),
                arguments: vec![GenericArgument::Type(scalar)].into(),
            }));
            // A single step is useful even for a recursive nominal declaration.
            let one = expand_representation(
                &store,
                &TypeMemo::new(),
                root,
                &mut |_, _, arguments| {
                    Ok(with_unknown_authorities(Some(signature.clone()), arguments))
                },
                Some(1),
            )
            .unwrap();
            let error = expand_representation(
                &store,
                &TypeMemo::new(),
                one,
                &mut |_, _, arguments| {
                    Ok(with_unknown_authorities(Some(signature.clone()), arguments))
                },
                None,
            )
            .unwrap_err();
            assert_eq!(
                error.failure,
                if growing {
                    RepresentationFailure::Query(type_proof_limit())
                } else {
                    RepresentationFailure::RecursiveRepresentation
                }
            );
            assert!(matches!(
                store.get(error.root).unwrap().kind,
                TypeKind::Named { .. }
            ));
        }
    }

    #[test]
    fn representation_substitution_preserves_caller_supplied_authority() {
        let declaration = TypeInterner::new(None);
        let unit = declaration.intern(row(TypeKind::Tuple(vec![].into())));
        let callable = declaration.intern(row(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result: unit,
            error: None,
            allocation: Allocation::NoAlloc,
            authority: AuthoritySummary::Known(
                vec![AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
                    depth: 1,
                    parameter: TypeParamId(0),
                })]
                .into(),
            ),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        })));
        let signature = nominal(Some(callable), vec![]);
        let store = std::sync::Arc::clone(&declaration);
        let argument = store.intern(row(TypeKind::Primitive(Primitive::Bytes)));
        let root = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path("Wrap")),
            arguments: vec![GenericArgument::Type(argument)].into(),
        }));
        for authority in [AuthoritySummary::Unknown, AuthoritySummary::inert()] {
            let expanded = expand_representation(
                &store,
                &TypeMemo::new(),
                root,
                &mut |store, _, arguments| {
                    // The callback reads actuals from caller-owned scratch.
                    assert!(
                        matches!(arguments[0], GenericArgument::Type(t) if store.get(t).is_some())
                    );
                    Ok(Some((
                        signature.clone(),
                        vec![authority.clone(); arguments.len()],
                    )))
                },
                None,
            )
            .unwrap();
            let TypeKind::Function(function) = &store.get(expanded).unwrap().kind else {
                panic!("expanded callable");
            };
            assert_eq!(function.authority, authority);
        }
    }
}
