//! Source-data reconstruction controls, not CT5/V or provider authority.
use super::*;
use hirsel_core_contract::Passing;
use hirsel_layout::instance::InstanceKey;
use hirsel_lir::sealed_payload_source::{ClosedCapture, ClosedEnvironment};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::*;
use std::sync::Arc;

fn scalar(types: &TypeInterner) -> TypeId {
    types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)))
}
fn function(types: &TypeInterner, result: TypeId) -> TypeId {
    types.intern(Type::with_kind(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: Default::default(),
        parameters: Default::default(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: Default::default(),
        incomplete: false,
        foreign: None,
    })))
}
fn case(types: &TypeInterner, label: &str, captures: Option<Vec<TypeId>>) -> CallableCase {
    let path = DefPath::core(label);
    let key = InstanceKey::new(&path, 0, &[], types, None, None).unwrap();
    CallableCase {
        case: ClosedStorageCase {
            body: hirsel_lir::invocations::ReferenceBody {
                definition: path,
                instance: key.digest(),
                body: 0,
            },
            // This fixture's synthetic recipe is data, never a source witness.
            recipe: ContentHash::of_parts("profile-test-data", [label.as_bytes()]),
            environment: captures.as_ref().map(|captures| ClosedEnvironment {
                captures: captures
                    .iter()
                    .map(|&ty| ClosedCapture {
                        ty: closed_storage_type(types, ty).unwrap(),
                        passing: Passing::Take,
                    })
                    .collect(),
                sizes: vec![],
                live: (0..captures.len() as u32).collect(),
            }),
        },
        captures: captures.unwrap_or_default(),
    }
}
fn signature(
    definition: DefId,
    representation: Option<TypeId>,
    resource: bool,
    generic: bool,
) -> SignatureRef {
    Arc::new(Signature {
        definition,
        metadata: Default::default(),
        fingerprint: ContentHash::of_bytes(b"profile nominal fixture"),
        kind: SignatureKind::Type(TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::NoAlloc,
            generics: if generic {
                vec![GenericParameter::Type {
                    name: "T".into(),
                    bounds: Default::default(),
                }]
            } else {
                vec![]
            },
            representation,
            bounds: vec![],
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Default::default(),
            distinct: false,
            resource,
            drop: None,
            fact: None,
            captures: None,
        }),
    })
}

#[test]
fn complete_noncallable_shapes_reconstruct_without_callable_queries() {
    let types = TypeInterner::new(None);
    let leaf = scalar(&types);
    let sequence = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(leaf)].into(),
    }));
    let boxed = types.intern(Type::with_kind(TypeKind::Box(sequence)));
    let tuple = types.intern(Type::with_kind(TypeKind::Tuple(vec![leaf, boxed].into())));
    let profile = derive(&types, tuple, &mut |_| Ok(None), &mut |_| {
        panic!("no callable value edge")
    })
    .unwrap();
    assert_eq!(profile.root, closed_storage_type(&types, tuple).unwrap());
    assert!(profile.functions.is_empty());
    assert!(
        hirsel_layout::structural::derive(&types, tuple, &mut |_| Ok(None))
            .unwrap()
            .is_some()
    );
}

#[test]
fn capture_graphs_recurse_without_following_callable_signature_types() {
    let types = TypeInterner::new(None);
    let leaf = scalar(&types);
    let inner = function(&types, leaf);
    // A function in the result type is not an owned environment edge.
    let outer = function(&types, inner);
    let mut queried = Vec::new();
    let only_code = derive(&types, outer, &mut |_| Ok(None), &mut |ty| {
        queried.push(ty);
        Ok(vec![case(&types, "outer_code_only", None)])
    })
    .unwrap();
    assert_eq!(queried, [outer]);
    assert_eq!(only_code.functions.len(), 1);
    assert!(
        hirsel_layout::structural::derive(&types, outer, &mut |_| Ok(None))
            .unwrap()
            .is_none()
    );

    let profile = derive(&types, outer, &mut |_| Ok(None), &mut |ty| {
        Ok(if ty == outer {
            vec![case(&types, "outer_owned_capture", Some(vec![inner]))]
        } else {
            assert_eq!(ty, inner);
            // Recursive environment type equations terminate without a clone cutoff.
            vec![case(&types, "inner_owned_capture", Some(vec![outer]))]
        })
    })
    .unwrap();
    assert_eq!(profile.functions.len(), 2);
    for row in &profile.functions {
        assert_eq!(row.cases[0].environment.as_ref().unwrap().captures.len(), 1);
    }
}

#[test]
fn callable_alternatives_are_canonical_but_unknown_or_empty_is_refused() {
    let types = TypeInterner::new(None);
    let callable = function(&types, scalar(&types));
    let profile = derive(&types, callable, &mut |_| Ok(None), &mut |_| {
        Ok(vec![
            case(&types, "profile_b", Some(vec![])),
            case(&types, "profile_a", None),
        ])
    })
    .unwrap();
    let reversed = derive(&types, callable, &mut |_| Ok(None), &mut |_| {
        Ok(vec![
            case(&types, "profile_a", None),
            case(&types, "profile_b", Some(vec![])),
        ])
    })
    .unwrap();
    assert_eq!(profile, reversed);
    assert_eq!(profile.functions[0].cases.len(), 2);
    assert!(
        profile.functions[0]
            .cases
            .iter()
            .any(|c| c.environment.is_none())
    );
    assert!(
        profile.functions[0]
            .cases
            .iter()
            .any(|c| c.environment.is_some())
    );
    assert_eq!(
        derive(&types, callable, &mut |_| Ok(None), &mut |_| Ok(vec![])),
        Err(SourceError::Unsupported(
            "storage profile needs complete original callable cases"
        ))
    );
    assert_eq!(
        derive(&types, callable, &mut |_| Ok(None), &mut |_| Err(
            SourceError::Unsupported("missing original code observation")
        )),
        Err(SourceError::Unsupported(
            "missing original code observation"
        ))
    );
}

#[test]
fn erased_callable_capture_types_cannot_supply_the_original_environment() {
    let types = TypeInterner::new(None);
    let leaf = scalar(&types);
    let inner = function(&types, leaf);
    let outer = function(&types, inner);
    assert_eq!(
        closed_storage_type(&types, inner).unwrap().ty,
        closed_storage_type(&types, outer).unwrap().ty
    );
    let result = derive(&types, outer, &mut |_| Ok(None), &mut |_| {
        let mut observed = case(&types, "profile_forged_capture", Some(vec![inner]));
        observed.captures[0] = outer;
        Ok(vec![observed])
    });
    assert_eq!(
        result,
        Err(SourceError::Malformed(
            "callable environment differs from original capture type"
        ))
    );
    let result = derive(&types, outer, &mut |_| Ok(None), &mut |_| {
        let mut observed = case(&types, "profile_missing_capture", Some(vec![inner]));
        observed.captures.clear();
        Ok(vec![observed])
    });
    assert_eq!(
        result,
        Err(SourceError::Malformed(
            "callable environment source capture arity"
        ))
    );
    let result = derive(&types, outer, &mut |_| Ok(None), &mut |_| {
        let mut observed = case(&types, "profile_borrowed_capture", Some(vec![leaf]));
        observed.case.environment.as_mut().unwrap().captures[0].passing = Passing::Borrow;
        Ok(vec![observed])
    });
    assert_eq!(
        result,
        Err(SourceError::Malformed(
            "closed environment lacks exact owned capture ordinals"
        ))
    );
}

#[test]
fn generic_nominal_profiles_reuse_substitution_and_refuse_hidden_resources() {
    let types = TypeInterner::new(None);
    let leaf = scalar(&types);
    let callable = function(&types, leaf);
    let definition = DefId::intern(&DefPath::core("ProfileWrap"));
    let parameter = types.intern(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::bound(TypeParamId(0)),
    )));
    let representation = types.intern(Type::with_kind(TypeKind::Box(parameter)));
    let wrap = |payload| {
        types.intern(Type::with_kind(TypeKind::Named {
            definition,
            arguments: vec![GenericArgument::Type(payload)].into(),
        }))
    };
    let declaration = signature(definition, Some(representation), false, true);
    let profile = derive(
        &types,
        wrap(callable),
        &mut |d| Ok((d == definition).then(|| declaration.clone())),
        &mut |ty| {
            assert_eq!(ty, callable);
            Ok(vec![case(&types, "profile_generic_inner", None)])
        },
    )
    .unwrap();
    assert_eq!(profile.functions.len(), 1);
    let resource_def = DefId::intern(&DefPath::core("ProfileResource"));
    let resource = types.intern(Type::with_kind(TypeKind::Named {
        definition: resource_def,
        arguments: Default::default(),
    }));
    let resource_decl = signature(resource_def, Some(leaf), true, false);
    let mut lookup = |d| {
        Ok(if d == definition {
            Some(declaration.clone())
        } else if d == resource_def {
            Some(resource_decl.clone())
        } else {
            None
        })
    };
    let expected = Err(SourceError::Unsupported(
        "storage profile needs a complete resource-free source shape",
    ));
    assert_eq!(
        derive(&types, wrap(resource), &mut lookup, &mut |_| panic!(
            "resource isn't callable"
        )),
        expected
    );
    assert_eq!(
        derive(&types, callable, &mut lookup, &mut |_| Ok(vec![case(
            &types,
            "profile_hidden_resource",
            Some(vec![resource])
        )])),
        expected
    );
    // The same nominal's unknown or opaque representation cannot become empty.
    assert_eq!(
        derive(&types, wrap(leaf), &mut |_| Ok(None), &mut |_| Ok(vec![])),
        expected
    );
    let opaque = signature(definition, None, false, true);
    assert_eq!(
        derive(
            &types,
            wrap(leaf),
            &mut |_| Ok(Some(opaque.clone())),
            &mut |_| Ok(vec![])
        ),
        expected
    );
}

#[test]
fn boundary_handles_keep_payload_profiles_separate_from_seal_admission() {
    let types = TypeInterner::new(None);
    let value = scalar(&types);
    let callable = function(&types, value);
    let region = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(callable)].into(),
    }));
    let profile = boundary(&types, region, &mut |_| Ok(None), &mut |ty| {
        assert_eq!(ty, callable);
        Ok(vec![case(&types, "region_profile_owned_code", None)])
    })
    .unwrap();
    assert_eq!(profile.root, closed_storage_type(&types, region).unwrap());
    assert_eq!(profile.functions.len(), 1);
    assert!(derive(&types, region, &mut |_| Ok(None), &mut |_| Ok(vec![])).is_err());
    let definition = DefId::intern(&DefPath::core("Arena"));
    let arena = types.intern(Type::with_kind(TypeKind::Named {
        definition,
        arguments: Default::default(),
    }));
    let mut declaration = signature(definition, None, false, false);
    let SignatureKind::Type(ty) = &mut Arc::make_mut(&mut declaration).kind else {
        unreachable!()
    };
    ty.capability = Some(CapabilityKind::Scoped);
    let profile = boundary(
        &types,
        arena,
        &mut |_| Ok(Some(declaration.clone())),
        &mut |_| panic!("grant has no callable payload"),
    )
    .unwrap();
    assert!(profile.functions.is_empty());
    assert_eq!(profile.root, closed_storage_type(&types, arena).unwrap());
    assert!(
        derive(
            &types,
            arena,
            &mut |_| Ok(Some(declaration.clone())),
            &mut |_| Ok(vec![])
        )
        .is_err()
    );
    assert_eq!(
        boundary(&types, arena, &mut |_| Ok(None), &mut |_| Ok(vec![])),
        Err(SourceError::Unsupported(
            "boundary Arena needs its original declaration"
        ))
    );
    let SignatureKind::Type(ty) = &mut Arc::make_mut(&mut declaration).kind else {
        unreachable!()
    };
    ty.capability = Some(CapabilityKind::Global);
    assert_eq!(
        boundary(
            &types,
            arena,
            &mut |_| Ok(Some(declaration.clone())),
            &mut |_| Ok(vec![])
        ),
        Err(SourceError::Malformed("boundary Arena declaration differs"))
    );
}

#[test]
fn boundary_nested_region_variants_keep_callable_profiles_without_payload_admission() {
    let types = TypeInterner::new(None);
    let leaf = scalar(&types);
    let callable = function(&types, leaf);
    let region = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(callable)].into(),
    }));
    // The same graph shape as a Result[Region[fn...], error] representation.
    let wrapper = types.intern(Type::with_kind(TypeKind::Variant(
        vec![
            Variant {
                name: "Ok".into(),
                fields: vec![(Some("value".into()), region)].into(),
            },
            Variant {
                name: "Error".into(),
                fields: vec![(Some("error".into()), leaf)].into(),
            },
        ]
        .into(),
    )));
    let profile = boundary(&types, wrapper, &mut |_| Ok(None), &mut |ty| {
        assert_eq!(ty, callable);
        Ok(vec![case(&types, "nested_boundary_callable", None)])
    })
    .unwrap();
    assert_eq!(profile.root, closed_storage_type(&types, wrapper).unwrap());
    assert_eq!(profile.functions.len(), 1);
    assert!(derive(&types, wrapper, &mut |_| Ok(None), &mut |_| Ok(vec![])).is_err());
    assert!(
        hirsel_layout::structural::derive(&types, wrapper, &mut |_| Ok(None))
            .unwrap()
            .is_none()
    );
    // Allowing a handle in a boundary does not make it a permitted owned
    // capture inside a resource-free callable payload.
    assert!(
        boundary(&types, callable, &mut |_| Ok(None), &mut |_| {
            Ok(vec![case(
                &types,
                "forged_resource_capture",
                Some(vec![region]),
            )])
        })
        .is_err()
    );
}

#[test]
fn test_capability_has_a_boundary_profile_but_remains_refused_as_sealed_contents() {
    let types = TypeInterner::new(None);
    let definition = DefId::intern(&DefPath::core("Test"));
    let root = types.intern(Type::with_kind(TypeKind::Named {
        definition,
        arguments: vec![].into(),
    }));
    let mut declaration = signature(definition, None, false, false);
    let SignatureKind::Type(ty) = &mut Arc::make_mut(&mut declaration).kind else {
        unreachable!()
    };
    ty.capability = Some(CapabilityKind::Scoped);
    let accepted = boundary(
        &types,
        root,
        &mut |_| Ok(Some(declaration.clone())),
        &mut |_| panic!("Test has no held code"),
    )
    .unwrap();
    assert_eq!(accepted.root, closed_storage_type(&types, root).unwrap());
    assert!(accepted.functions.is_empty());
    assert!(
        derive(
            &types,
            root,
            &mut |_| Ok(Some(declaration.clone())),
            &mut |_| Ok(vec![])
        )
        .is_err()
    );
    let SignatureKind::Type(ty) = &mut Arc::make_mut(&mut declaration).kind else {
        unreachable!()
    };
    ty.capability = Some(CapabilityKind::Global);
    assert_eq!(
        boundary(
            &types,
            root,
            &mut |_| Ok(Some(declaration.clone())),
            &mut |_| Ok(vec![])
        ),
        Err(SourceError::Malformed("boundary Test declaration differs"))
    );
}
