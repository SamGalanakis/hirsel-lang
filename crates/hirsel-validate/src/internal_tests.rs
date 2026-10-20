//! Tests that build the checker's private `Checker`, `Site`, declaration tables or
//! scratch stores directly. Tests through the public API are in tests/it.
use super::*;
use crate::support::*;
use hirsel_syntax::{ContentHash, PassingMode};
use hirsel_types::*;

#[test]
fn weakening_a_captured_generic_uses_its_outer_sendability_bound() {
    let mut f = captured_generic_copy(true, false, false);
    let SignatureKind::Function(owner) =
        &mut f.signatures.get_mut(&f.definition.path).unwrap().kind
    else {
        panic!()
    };
    let GenericParameter::Type { bounds, .. } = &mut owner.generics[0] else {
        panic!()
    };
    *bounds = vec![Bound::Flat].into();
    let body = &f.definition.bodies[1];
    let declarations = typing::declaration_tables(&f.definition, &f.signatures).unwrap();
    let scratch = scratch::Scratch::new(&f.definition.types, f.definition.bodies.len());
    let checker = Checker {
        definition: &f.definition,
        body,
        signatures: &f.signatures,
        declarations: &declarations,
        scratch: &scratch,
        contract: false,
        site: Site {
            body: BodyId(1),
            block: None,
            instruction: None,
        },
        errors: vec![],
        captured: BTreeMap::new(),
        returned: BTreeMap::new(),
        static_data: None,
        proof_errors: std::cell::RefCell::new(Vec::new()),
    };
    let mut types = f.definition.types.clone();
    let weakened = checker.weakened_type(
        &mut types,
        body.places[1].ty,
        &checker.generic_declarations(),
    );
    assert_eq!(
        types.get(weakened).unwrap().modes.sendability,
        Sendability::Sendable
    );
}

#[test]
fn substituted_call_rows_normalize_with_the_actual_declaring_binder() {
    let mut f = captured_generic_copy(false, true, false);
    let SignatureKind::Function(owner) =
        &mut f.signatures.get_mut(&f.definition.path).unwrap().kind
    else {
        panic!()
    };
    owner.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Data].into(),
    }]
    .into();
    let parameter = f.definition.bodies[1].places[1].ty;
    let mut row = f.definition.types.get(parameter).unwrap().clone();
    row.modes.relation = Relation::global();
    let parameter = f.definition.types.push(row);
    let mut row = ty(TypeKind::Box(parameter));
    row.modes.uniqueness = Uniqueness::Unique;
    let actual = f.definition.types.push(row);
    let body = &f.definition.bodies[1];
    let declarations = typing::declaration_tables(&f.definition, &f.signatures).unwrap();
    let scratch = scratch::Scratch::new(&f.definition.types, f.definition.bodies.len());
    let checker = Checker {
        definition: &f.definition,
        body,
        signatures: &f.signatures,
        declarations: &declarations,
        scratch: &scratch,
        contract: false,
        site: Site {
            body: BodyId(1),
            block: None,
            instruction: None,
        },
        errors: vec![],
        captured: BTreeMap::new(),
        returned: BTreeMap::new(),
        static_data: None,
        proof_errors: std::cell::RefCell::new(Vec::new()),
    };
    let mut types = f.definition.types.clone();
    let mut declaration = TypeStore::default();
    let formal = declaration.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let arguments = [GenericArgument::Type(actual)];
    let declarations = checker.generic_declarations();
    let expected = checker
        .imported_in(&declaration, formal, &mut types, &arguments, &declarations)
        .unwrap();
    assert_ne!(
        types.get(expected).unwrap().modes.sendability,
        Sendability::Sendable
    );
    let mut forged_row = types.get(parameter).unwrap().clone();
    forged_row.kind =
        TypeKind::Parameter(TypeParameterRef::body(body.parameter_scope, TypeParamId(0)));
    let forged_parameter = types.push(forged_row);
    let mut forged_row = types.get(actual).unwrap().clone();
    forged_row.kind = TypeKind::Box(forged_parameter);
    let forged_actual = types.push(forged_row);
    let forged = hirsel_types::normalize_type_properties(
        &mut types,
        forged_actual,
        &declarations,
        &mut |_| Ok::<_, hirsel_types::QueryError>(None),
    )
    .unwrap();
    assert_eq!(
        types.get(forged).unwrap().modes.sendability,
        Sendability::Sendable
    );
    assert!(!semantic_equal(&types, expected, &types, forged).unwrap());
}

#[test]
fn sorted_map_equality_and_key_validation_accounts_for_the_order_dictionary() {
    for kind in [BuiltinType::Map, BuiltinType::SortedMap] {
        let mut f = Fixture::unit();
        let scalar = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U32)));
        let source = param(
            &mut f,
            TypeKind::Builtin {
                kind,
                arguments: vec![GenericArgument::Type(scalar), GenericArgument::Type(scalar)]
                    .into(),
            },
        );
        let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(
            result,
            Rvalue::Binary {
                op: hirsel_syntax::BinaryOp::Eq,
                left: operand(source, UseKind::Borrow),
                right: operand(source, UseKind::Borrow),
            },
            0,
        );
        owner_sig(&mut f, &[PassingMode::Borrow]);
        let kinds = f.kinds();
        assert_eq!(
            kinds.contains(&ViolationKind::InvalidOperator),
            kind == BuiltinType::SortedMap,
            "{kind:?}: {kinds:?}"
        );
        let body = &f.definition.bodies[0];
        let declarations = typing::declaration_tables(&f.definition, &f.signatures).unwrap();
        let scratch = scratch::Scratch::new(&f.definition.types, f.definition.bodies.len());
        let checker = Checker {
            definition: &f.definition,
            body,
            signatures: &f.signatures,
            declarations: &declarations,
            scratch: &scratch,
            contract: false,
            site: Site {
                body: BodyId(0),
                block: None,
                instruction: None,
            },
            errors: vec![],
            captured: BTreeMap::new(),
            returned: BTreeMap::new(),
            static_data: None,
            proof_errors: std::cell::RefCell::new(Vec::new()),
        };
        assert_eq!(
            checker.bound(
                &f.definition.types,
                body.places[source as usize].ty,
                Bound::Key,
                &GenericDeclarations::default()
            ),
            kind == BuiltinType::Map
        );
    }
}

#[test]
fn erased_capture_authority_is_checked_at_its_generic_binder() {
    let mut signature = function_signature("iterator", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(f) = &mut signature.kind else {
        panic!()
    };
    f.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    let mut iterator = ty(TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(f.result)].into(),
    });
    iterator.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
        vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))]
        .into(),
    )));
    f.result = signature.types.push(iterator.clone());
    assert!(typing::signature_valid(&signature));
    iterator.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
        vec![AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        })]
        .into(),
    )));
    let forged = signature.types.push(iterator);
    let SignatureKind::Function(f) = &mut signature.kind else {
        panic!()
    };
    f.result = forged;
    assert!(!typing::signature_valid(&signature));
}

#[test]
fn callable_authority_uses_and_size_constraints_reject_foreign_binders() {
    for consumer in 0..4 {
        let mut signature = function_signature("scoped", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(f) = &mut signature.kind else {
            panic!()
        };
        f.generics = vec![
            GenericParameter::Type {
                name: "T".into(),
                bounds: vec![].into(),
            },
            GenericParameter::Size { name: "n".into() },
        ]
        .into();
        let set = |signature: &mut Signature, depth| {
            let tp = TypeParameterRef::Bound {
                depth,
                parameter: TypeParamId(0),
            };
            let sp = SizeParameterRef::Bound {
                depth,
                parameter: SizeParamId(1),
            };
            let authority = AuthoritySummary::Known(vec![AuthorityTerm::TypeParameter(tp)].into());
            let SignatureKind::Function(f) = &mut signature.kind else {
                panic!()
            };
            match consumer {
                0 => f.authority = authority,
                1 => f.uses = Uses::Only(authority),
                2 => signature.metadata.schema = vec![SchemaConstraint::PositiveSize(sp)],
                _ => {
                    f.result = signature.types.push(ty(TypeKind::Named {
                        definition: path("Sized"),
                        arguments: vec![GenericArgument::Size(Size::Parameter(sp))].into(),
                    }))
                }
            }
        };
        set(&mut signature, 0);
        assert!(
            typing::signature_valid(&signature),
            "valid consumer {consumer}"
        );
        set(&mut signature, 1);
        assert!(
            !typing::signature_valid(&signature),
            "forged consumer {consumer}"
        );
    }
}

#[test]
fn capability_authority_is_nominal_even_with_an_incomplete_snapshot() {
    for extra in [false, true] {
        let mut f = Fixture::unit();
        let source = param(
            &mut f,
            TypeKind::Named {
                definition: path("Gate"),
                arguments: vec![].into(),
            },
        );
        let mut signature = capability_signature("Gate");
        let SignatureKind::Type(declaration) = &mut signature.kind else {
            panic!();
        };
        declaration.authority = if extra {
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into())
        } else {
            AuthoritySummary::inert()
        };
        f.signatures.insert(path("Gate"), signature);
        owner_sig(&mut f, &[PassingMode::Borrow]);
        let body = &f.definition.bodies[0];
        let checker = Checker {
            definition: &f.definition,
            body,
            signatures: &f.signatures,
            declarations: &typing::declaration_tables(&f.definition, &f.signatures).unwrap(),
            scratch: &scratch::Scratch::new(&f.definition.types, f.definition.bodies.len()),
            contract: false,
            site: Site {
                body: BodyId(0),
                block: None,
                instruction: None,
            },
            errors: vec![],
            captured: BTreeMap::new(),
            returned: BTreeMap::new(),
            static_data: None,
            proof_errors: std::cell::RefCell::new(Vec::new()),
        };
        let expected =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
        assert_eq!(
            checker.authority_with_generics(
                &f.definition.types,
                body.places[source as usize].ty,
                &GenericDeclarations::default()
            ),
            expected
        );
    }
}

#[test]
fn nonregular_nominal_bound_reports_the_declared_tooling_limit() {
    let mut f = Fixture::unit();
    let mut declaration = TypeStore::default();
    let parameter = declaration.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let list = declaration.push(ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(parameter)].into(),
    }));
    let next = declaration.push(ty(TypeKind::Named {
        definition: path("Grow"),
        arguments: vec![GenericArgument::Type(list)].into(),
    }));
    let representation = declaration.push(ty(TypeKind::Box(next)));
    f.signatures.insert(
        path("Grow"),
        Signature {
            definition: path("Grow"),
            types: declaration,
            metadata: SignatureMetadata::default(),
            fingerprint: ContentHash::default(),
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                format: None,
                generics: vec![GenericParameter::Type {
                    name: "T".into(),
                    bounds: vec![].into(),
                }],
                representation: Some(representation),
                bounds: vec![],
                authority: AuthoritySummary::inert(),
                capability: None,
                implementation_authority: None,
                serializable: false,
                layout: Layout::default(),
                resource: false,
                distinct: false,
                drop: None,
                fact: None,
                captures: None,
                destructor_allocation: Allocation::NoAlloc,
            }),
        },
    );
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U32)));
    let root = f.definition.types.push(ty(TypeKind::Named {
        definition: path("Grow"),
        arguments: vec![GenericArgument::Type(scalar)].into(),
    }));
    let declarations = vec![GenericDeclarations::default()];
    let scratch = scratch::Scratch::new(&f.definition.types, 1);
    let checker = Checker {
        definition: &f.definition,
        body: &f.definition.bodies[0],
        signatures: &f.signatures,
        declarations: &declarations,
        scratch: &scratch,
        contract: false,
        site: Site {
            body: BodyId(0),
            block: None,
            instruction: None,
        },
        errors: vec![],
        captured: BTreeMap::new(),
        returned: BTreeMap::new(),
        static_data: None,
        proof_errors: std::cell::RefCell::new(Vec::new()),
    };
    assert!(!checker.bound(&f.definition.types, root, Bound::Copyable, &declarations[0]));
    let report = Validation {
        violations: checker.proof_errors.into_inner(),
    };
    assert_eq!(report.violations.len(), 1);
    assert_eq!(
        tooling_limit_error(&report),
        Some(hirsel_types::type_proof_limit())
    );
}

#[test]
fn finite_layout_cleanup_and_packed_carriers_have_no_nominal_depth_cutoff() {
    for depth in [128, 129, 200] {
        for packed in [false, true] {
            let mut f = Fixture::unit();
            let base = if packed {
                let variant = f.definition.types.push(ty(TypeKind::Variant(
                    vec![hirsel_types::Variant {
                        name: "V".into(),
                        fields: vec![].into(),
                    }]
                    .into(),
                )));
                f.definition.types.push(ty(TypeKind::Builtin {
                    kind: BuiltinType::Packed,
                    arguments: vec![GenericArgument::Type(variant)].into(),
                }))
            } else {
                f.definition
                    .types
                    .push(ty(TypeKind::Primitive(Primitive::U8)))
            };
            let mut root = base;
            for n in 0..depth {
                let definition = path(&format!("D{n}"));
                f.signatures.insert(
                    definition.clone(),
                    Signature {
                        definition: definition.clone(),
                        types: f.definition.types.clone(),
                        metadata: SignatureMetadata::default(),
                        fingerprint: ContentHash::default(),
                        kind: SignatureKind::Type(TypeDefinition {
                            loop_source: None,
                            format: None,
                            generics: vec![],
                            representation: Some(root),
                            bounds: vec![],
                            authority: AuthoritySummary::inert(),
                            capability: None,
                            implementation_authority: None,
                            serializable: false,
                            layout: Layout::default(),
                            resource: false,
                            distinct: false,
                            drop: None,
                            fact: None,
                            captures: None,
                            destructor_allocation: Allocation::NoAlloc,
                        }),
                    },
                );
                root = f.definition.types.push(ty(TypeKind::Named {
                    definition,
                    arguments: vec![].into(),
                }));
            }
            let declarations = vec![GenericDeclarations::default()];
            let scratch = scratch::Scratch::new(&f.definition.types, 1);
            let mut checker = Checker {
                definition: &f.definition,
                body: &f.definition.bodies[0],
                signatures: &f.signatures,
                declarations: &declarations,
                scratch: &scratch,
                contract: false,
                site: Site {
                    body: BodyId(0),
                    block: None,
                    instruction: None,
                },
                errors: vec![],
                captured: BTreeMap::new(),
                returned: BTreeMap::new(),
                static_data: None,
                proof_errors: std::cell::RefCell::new(Vec::new()),
            };
            let mut store = f.definition.types.clone();
            assert!(checker.bound(&store, root, Bound::NoallocDrop, &declarations[0]));
            assert_eq!(checker.packed_carrier(&mut store, root), packed);
            assert_eq!(
                checker.layout(&mut store, root),
                Some(if packed {
                    match store.get(base).unwrap().kind {
                        TypeKind::Builtin { ref arguments, .. } => store
                            .get(match arguments[0] {
                                GenericArgument::Type(t) => t,
                                _ => unreachable!(),
                            })
                            .unwrap()
                            .kind
                            .clone(),
                        _ => unreachable!(),
                    }
                } else {
                    TypeKind::Primitive(Primitive::U8)
                })
            );
            assert!(checker.errors.is_empty());
            assert!(checker.proof_errors.borrow().is_empty());
        }
    }
}
