//! Tests that build the checker's private `Checker`, `Site`, declaration tables or
//! scratch stores directly. Tests through the public API are in tests/it.
use super::*;
use crate::support::*;
use hirsel_syntax::{ContentHash, PassingMode};
use hirsel_types::*;

#[test]
fn weakening_a_captured_generic_uses_its_outer_sendability_bound() {
    let mut f = captured_generic_copy(true, false, false);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(f.definition.path).unwrap().kind
    else {
        panic!()
    };
    let GenericParameter::Type { bounds, .. } = &mut owner.generics[0] else {
        panic!()
    };
    *bounds = vec![Bound::Flat].into();
    let body = &f.definition.bodies[1];
    let declarations = typing::declaration_tables(
        interner(),
        &f.definition,
        &crate::input::Reader::new(interner(), &f.signatures),
    )
    .unwrap();
    let scratch = scratch::Scratch::new(f.definition.bodies.len());
    let reader = crate::input::Reader::new(interner(), &f.signatures);
    let memo = TypeMemo::new();
    let checker = Checker {
        definition: &f.definition,
        t: interner(),
        memo: &memo,
        body,
        signatures: &reader,
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
        unique_parts_cache: Default::default(),
    };
    let types = interner();
    let weakened = checker.weakened_type(types, body.places[1].ty, &checker.generic_declarations());
    assert_eq!(
        types.get(weakened).unwrap().modes.sendability,
        Sendability::Sendable
    );
}

#[test]
fn substituted_call_rows_normalize_with_the_actual_declaring_binder() {
    let mut f = captured_generic_copy(false, true, false);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(f.definition.path).unwrap().kind
    else {
        panic!()
    };
    owner.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Data].into(),
    }]
    .into();
    let parameter = f.definition.bodies[1].places[1].ty;
    let mut row = interner().get(parameter).unwrap().clone();
    row.modes.relation = Relation::global();
    let parameter = f.definition.types.push(row);
    let mut row = ty(TypeKind::Box(parameter));
    row.modes.uniqueness = Uniqueness::Unique;
    let actual = f.definition.types.push(row);
    let body = &f.definition.bodies[1];
    let declarations = typing::declaration_tables(
        interner(),
        &f.definition,
        &crate::input::Reader::new(interner(), &f.signatures),
    )
    .unwrap();
    let scratch = scratch::Scratch::new(f.definition.bodies.len());
    let reader = crate::input::Reader::new(interner(), &f.signatures);
    let memo = TypeMemo::new();
    let checker = Checker {
        definition: &f.definition,
        t: interner(),
        memo: &memo,
        body,
        signatures: &reader,
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
        unique_parts_cache: Default::default(),
    };
    let types = interner();
    let declaration = interner();
    let formal = declaration.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let arguments = [GenericArgument::Type(actual)];
    let declarations = checker.generic_declarations();
    let expected = checker
        .imported_in(formal, &arguments, &declarations)
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
    let forged =
        hirsel_types::normalize_type_properties(types, forged_actual, &declarations, &mut |_| {
            Ok::<_, hirsel_types::QueryError>(None)
        })
        .unwrap();
    assert_eq!(
        types.get(forged).unwrap().modes.sendability,
        Sendability::Sendable
    );
    assert!(!types.semantic_eq(expected, forged));
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
        let declarations = typing::declaration_tables(
            interner(),
            &f.definition,
            &crate::input::Reader::new(interner(), &f.signatures),
        )
        .unwrap();
        let scratch = scratch::Scratch::new(f.definition.bodies.len());
        let reader = crate::input::Reader::new(interner(), &f.signatures);
        let memo = TypeMemo::new();
        let checker = Checker {
            definition: &f.definition,
            t: interner(),
            memo: &memo,
            body,
            signatures: &reader,
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
            unique_parts_cache: Default::default(),
        };
        assert_eq!(
            checker.bound(
                interner(),
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
    f.result = interner().intern(iterator.clone());
    assert!(typing::signature_valid(interner(), &signature));
    iterator.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
        vec![AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        })]
        .into(),
    )));
    let forged = interner().intern(iterator);
    let SignatureKind::Function(f) = &mut signature.kind else {
        panic!()
    };
    f.result = forged;
    assert!(!typing::signature_valid(interner(), &signature));
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
                    f.result = interner().intern(ty(TypeKind::Named {
                        definition: id("Sized"),
                        arguments: vec![GenericArgument::Size(Size::Parameter(sp))].into(),
                    }))
                }
            }
        };
        set(&mut signature, 0);
        assert!(
            typing::signature_valid(interner(), &signature),
            "valid consumer {consumer}"
        );
        set(&mut signature, 1);
        assert!(
            !typing::signature_valid(interner(), &signature),
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
                definition: id("Gate"),
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
        f.signatures.insert(id("Gate"), signature);
        owner_sig(&mut f, &[PassingMode::Borrow]);
        let body = &f.definition.bodies[0];
        let reader = crate::input::Reader::new(interner(), &f.signatures);
        let memo = TypeMemo::new();
        let checker = Checker {
            definition: &f.definition,
            t: interner(),
            memo: &memo,
            body,
            signatures: &reader,
            declarations: &typing::declaration_tables(interner(), &f.definition, &reader).unwrap(),
            scratch: &scratch::Scratch::new(f.definition.bodies.len()),
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
            unique_parts_cache: Default::default(),
        };
        let expected =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
        assert_eq!(
            checker.authority_with_generics(
                interner(),
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
    let declaration = interner();
    let parameter = declaration.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let list = declaration.push(ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(parameter)].into(),
    }));
    let next = declaration.push(ty(TypeKind::Named {
        definition: id("Grow"),
        arguments: vec![GenericArgument::Type(list)].into(),
    }));
    let representation = declaration.push(ty(TypeKind::Box(next)));
    f.signatures.insert(
        id("Grow"),
        Signature {
            definition: id("Grow"),
            metadata: SignatureMetadata::default(),
            fingerprint: ContentHash::default(),
            kind: SignatureKind::Type(Box::new(TypeDefinition {
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
                serializable_check: None,
                layout: Layout::default(),
                resource: false,
                distinct: false,
                drop: None,
                fact: None,
                captures: None,
                destructor_allocation: Allocation::NoAlloc,
            })),
        },
    );
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U32)));
    let root = f.definition.types.push(ty(TypeKind::Named {
        definition: id("Grow"),
        arguments: vec![GenericArgument::Type(scalar)].into(),
    }));
    let declarations = vec![GenericDeclarations::default()];
    let scratch = scratch::Scratch::new(1);
    let reader = crate::input::Reader::new(interner(), &f.signatures);
    let memo = TypeMemo::new();
    let checker = Checker {
        definition: &f.definition,
        t: interner(),
        memo: &memo,
        body: &f.definition.bodies[0],
        signatures: &reader,
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
        unique_parts_cache: Default::default(),
    };
    assert!(!checker.bound(interner(), root, Bound::Copyable, &declarations[0]));
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
                let definition = id(&format!("D{n}"));
                f.signatures.insert(
                    definition,
                    Signature {
                        definition,
                        metadata: SignatureMetadata::default(),
                        fingerprint: ContentHash::default(),
                        kind: SignatureKind::Type(Box::new(TypeDefinition {
                            loop_source: None,
                            format: None,
                            generics: vec![],
                            representation: Some(root),
                            bounds: vec![],
                            authority: AuthoritySummary::inert(),
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
                    },
                );
                root = f.definition.types.push(ty(TypeKind::Named {
                    definition,
                    arguments: vec![].into(),
                }));
            }
            let declarations = vec![GenericDeclarations::default()];
            let scratch = scratch::Scratch::new(1);
            let reader = crate::input::Reader::new(interner(), &f.signatures);
            let memo = TypeMemo::new();
            let mut checker = Checker {
                definition: &f.definition,
                t: interner(),
                memo: &memo,
                body: &f.definition.bodies[0],
                signatures: &reader,
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
                unique_parts_cache: Default::default(),
            };
            let store = interner();
            assert!(checker.bound(store, root, Bound::NoallocDrop, &declarations[0]));
            assert_eq!(checker.packed_carrier(store, root), packed);
            assert_eq!(
                checker.layout(store, root),
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

#[test]
fn repeated_flat_copies_keep_canonical_rows_without_reinterning() {
    let mut f = Fixture::unit();
    owner_sig(&mut f, &[]);
    let types = interner();
    let reader = crate::input::Reader::new(types, &f.signatures);
    let declarations = typing::declaration_tables(types, &f.definition, &reader).unwrap();
    let scratch = scratch::Scratch::new(f.definition.bodies.len());
    let memo = TypeMemo::new();
    let checker = Checker {
        definition: &f.definition,
        t: types,
        memo: &memo,
        body: &f.definition.bodies[0],
        signatures: &reader,
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
        unique_parts_cache: Default::default(),
    };
    let operand = copy(0);
    let original = checker.body.places[0].ty;
    let canonical = checker.copied_type(types, &operand, original);
    assert_eq!(checker.copied_type(types, &operand, canonical), canonical);
    let before = types.stats();
    for _ in 0..16 {
        assert_eq!(checker.copied_type(types, &operand, canonical), canonical);
    }
    let after = types.stats();
    assert_eq!(
        after.row_bytes, before.row_bytes,
        "copies retain no additional rows"
    );
    assert_eq!(
        after.hits, before.hits,
        "unchanged copy modes must not repeat interner/base lookups"
    );
    assert!(checker.proof_errors.borrow().is_empty());
}

#[test]
fn a_single_body_without_closures_solves_its_flow_once() {
    let mut fixture = Fixture::unit();
    owner_sig(&mut fixture, &[]);
    assert_eq!(fixture.definition.bodies.len(), 1);
    assert!(fixture.definition.contracts.is_empty());
    assert_eq!(fixture.definition.bodies[0].blocks.len(), 1);
    let before = hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFlowSteps);
    fixture.valid();
    let steps = hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateFlowSteps) - before;
    assert_eq!(
        steps, 1,
        "no closure environment needs a preliminary fixed point"
    );
}

#[test]
fn row_transformations_respect_a_foreign_generic_environment() {
    let f = Fixture::unit();
    let types = interner();
    let reader = crate::input::Reader::new(types, &f.signatures);
    let declarations = vec![GenericDeclarations::default()];
    let scratch = scratch::Scratch::new(1);
    let memo = TypeMemo::new();
    let checker = Checker {
        definition: &f.definition,
        t: types,
        memo: &memo,
        body: &f.definition.bodies[0],
        signatures: &reader,
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
        unique_parts_cache: Default::default(),
    };
    let parameter = types.intern(ty(TypeKind::Parameter(TypeParameterRef::body(
        checker.body.parameter_scope,
        hirsel_types::TypeParamId(0),
    ))));
    let mut flat = GenericDeclarations::default();
    flat.insert(
        checker.body.parameter_scope,
        vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Flat].into(),
        }],
    );
    let current = checker.weakened_type(types, parameter, checker.generic_declarations());
    let foreign = checker.weakened_type(types, parameter, &flat);
    assert_eq!(
        types.row(current).modes.sendability,
        Sendability::NotSendable
    );
    assert_eq!(types.row(foreign).modes.sendability, Sendability::Sendable);
    assert_eq!(
        checker.weakened_type(types, parameter, checker.generic_declarations()),
        current
    );
    assert_eq!(checker.weakened_type(types, parameter, &flat), foreign);
    assert!(checker.proof_errors.borrow().is_empty());
}
