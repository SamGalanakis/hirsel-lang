//! Nominal representation expansion for stage-owned layout walks.
use hirsel_syntax::{DefId, DefPath};
use hirsel_types::*;

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

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::ContentHash;

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
    fn nominal(
        types: TypeInterner,
        representation: Option<TypeId>,
        bounds: Vec<Bound>,
    ) -> SignatureRef {
        std::sync::Arc::new(Signature {
            definition: path("Wrap"),
            types,
            metadata: SignatureMetadata::default(),
            fingerprint: ContentHash::default(),
            kind: SignatureKind::Type(TypeDefinition {
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
            }),
        })
    }
    fn with_unknown_authorities(
        signature: Option<SignatureRef>,
        arguments: &[GenericArgument],
    ) -> Option<(SignatureRef, Vec<AuthoritySummary>)> {
        signature.map(|signature| (signature, vec![AuthoritySummary::Unknown; arguments.len()]))
    }

    #[test]
    fn representation_steps_preserve_nested_actuals_without_copying_the_store() {
        let declaration = TypeInterner::new(None);
        let parameter = declaration.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let signature = nominal(declaration, Some(parameter), vec![]);
        let store = std::sync::Arc::clone(&declaration);
        let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
        let mut root = scalar;
        for _ in 0..200 {
            root = store.intern(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(root)].into(),
            }));
        }
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows);
        assert_eq!(
            expand_representation(
                &store,
                root,
                &mut |_, _, _| panic!("zero-step lookup"),
                Some(0)
            ),
            Ok(root)
        );
        let mut reads = 0;
        let mut lookup = |_: &TypeInterner, _: &DefPath, arguments: &[GenericArgument]| {
            reads += 1;
            Ok(with_unknown_authorities(Some(signature.clone()), arguments))
        };
        let one = expand_representation(&store, root, &mut lookup, Some(1)).unwrap();
        assert!(matches!(
            store.get(one).unwrap().kind,
            TypeKind::Named { .. }
        ));
        let full = expand_representation(&store, one, &mut lookup, None).unwrap();
        assert_eq!(
            store.get(full).unwrap().kind,
            store.get(scalar).unwrap().kind
        );
        assert_eq!(reads, 200);
        assert_eq!(
            hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows),
            before
        );
    }

    #[test]
    fn representation_stops_keep_the_last_row_and_exact_failure() {
        let store = TypeInterner::new(None);
        let root = store.intern(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![].into(),
        }));
        let opaque = nominal(TypeInterner::new(None), None, vec![]);
        for (declaration, failure) in [
            (None, RepresentationFailure::MissingDeclaration),
            (Some(opaque), RepresentationFailure::MissingRepresentation),
        ] {
            assert_eq!(
                expand_representation(
                    &store,
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
        let malformed = nominal(declaration, Some(parameter), vec![]);
        assert_eq!(
            expand_representation(
                &store,
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
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        assert!(
            expand_representation(
                &store,
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
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(argument)].into(),
            }));
            let signature = nominal(declaration, Some(next), vec![]);
            let store = std::sync::Arc::clone(&declaration);
            let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
            let root = store.intern(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(scalar)].into(),
            }));
            // A single step is useful even for a recursive nominal declaration.
            let one = expand_representation(
                &store,
                root,
                &mut |_, _, arguments| {
                    Ok(with_unknown_authorities(Some(signature.clone()), arguments))
                },
                Some(1),
            )
            .unwrap();
            let error = expand_representation(
                &store,
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
        let signature = nominal(declaration, Some(callable), vec![]);
        let store = std::sync::Arc::clone(&declaration);
        let argument = store.intern(row(TypeKind::Primitive(Primitive::Bytes)));
        let root = store.intern(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(argument)].into(),
        }));
        for authority in [AuthoritySummary::Unknown, AuthoritySummary::inert()] {
            let expanded = expand_representation(
                &store,
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
