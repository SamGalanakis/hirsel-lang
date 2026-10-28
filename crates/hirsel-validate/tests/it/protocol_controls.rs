//! Declaration-only mutants leave ordinary executable value typing unchanged.
use super::*;

fn core_option() -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: "Option".into(),
    })
}
fn nominal(name: &str) -> Signature {
    let mut s = function_signature(name, vec![], TypeKind::Tuple(vec![].into()));
    let unit = interner().intern(Type::unit());
    s.kind = SignatureKind::Type(Box::new(TypeDefinition {
        loop_source: None,
        format: None,
        generics: vec![].into(),
        representation: Some(unit),
        bounds: vec![Bound::Plain],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        captures: None,
        destructor_allocation: Allocation::NoAlloc,
    }));
    s
}
fn association() -> Fixture {
    let mut f = Fixture::unit();
    owner_signature(&mut f, PassingMode::Borrow);
    let mut view = nominal("View");
    let mut cursor = nominal("Cursor");
    let mut option = nominal("Option");
    option.definition = DefId::intern(&core_option());
    if let SignatureKind::Type(t) = &mut option.kind {
        t.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        }]
        .into();
    }
    let mut open = function_signature(
        "open",
        vec![(
            PassingMode::Borrow,
            TypeKind::Named {
                definition: view.definition.clone(),
                arguments: vec![].into(),
            },
        )],
        TypeKind::Named {
            definition: cursor.definition.clone(),
            arguments: vec![].into(),
        },
    );
    if let SignatureKind::Function(fun) = &mut open.kind {
        let mut row = interner().get(fun.result).unwrap().clone();
        row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        fun.result = interner().intern(row);
    }
    let mut next = function_signature(
        "next",
        vec![(
            PassingMode::Mut,
            TypeKind::Named {
                definition: cursor.definition.clone(),
                arguments: vec![].into(),
            },
        )],
        TypeKind::Tuple(vec![].into()),
    );
    let item = interner().intern(ty(TypeKind::Primitive(Primitive::U64)));
    if let SignatureKind::Function(fun) = &mut next.kind {
        fun.result = interner().intern(ty(TypeKind::Named {
            definition: option.definition.clone(),
            arguments: vec![GenericArgument::Type(item)].into(),
        }));
    }
    if let SignatureKind::Type(t) = &mut view.kind {
        t.loop_source = Some(LoopSourceDeclaration {
            open: open.definition.path().clone(),
            next: next.definition.path().clone(),
            option: option.definition.path().clone(),
        });
    }
    // Only the operation result is Local; the cursor declaration's shape is Plain.
    cursor.metadata.visibility = hirsel_syntax::Visibility::Public;
    for s in [view, cursor, option, open, next] {
        f.signatures.insert(s.definition.clone(), s);
    }
    f
}
#[test]
fn unused_association_rejects_foreign_option_with_unchanged_executable_typing() {
    let mut f = association();
    f.valid();
    let mut foreign = f.signatures.get(&core_option()).unwrap().clone();
    foreign.definition = id("ForeignOption");
    f.signatures.insert(foreign.definition.clone(), foreign);
    let next = f.signatures.get_mut(&path("next")).unwrap();
    let SignatureKind::Function(fun) = &mut next.kind else {
        panic!()
    };
    let mut row = interner().get(fun.result).unwrap().clone();
    let TypeKind::Named { definition, .. } = &mut row.kind else {
        panic!()
    };
    *definition = id("ForeignOption");
    fun.result = interner().intern(row);
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::InvalidSignature(path("View"))]
    );
}
#[test]
fn unused_association_rejects_next_receiver_mode() {
    let mut f = association();
    f.valid();
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("next")).unwrap().kind
    else {
        panic!()
    };
    fun.parameters[0].passing = PassingMode::Borrow;
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::InvalidSignature(path("View"))]
    );
}
#[test]
fn unused_association_rejects_a_next_mode_the_open_result_does_not_supply() {
    let mut f = association();
    let cursor = f.signatures.get_mut(&path("Cursor")).unwrap();
    let string = interner().intern(ty(TypeKind::Primitive(Primitive::String)));
    let record = interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "source".into(),
            ty: string,
            growable: false,
        }]
        .into(),
    )));
    let SignatureKind::Type(nominal) = &mut cursor.kind else {
        panic!()
    };
    nominal.representation = Some(record);
    nominal.bounds.clear();
    f.valid();
    let next = f.signatures.get_mut(&path("next")).unwrap();
    let SignatureKind::Function(fun) = &next.kind else {
        panic!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let parameter = interner().intern(row);
    let SignatureKind::Function(fun) = &mut next.kind else {
        panic!()
    };
    fun.parameters[0].ty = parameter;
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::InvalidSignature(path("View"))]
    );
}
#[test]
fn selector_fields_are_checked_on_declaration_only_boundaries() {
    let mut f = Fixture::unit();
    owner_signature(&mut f, PassingMode::Borrow);
    let mut boundary = function_signature(
        "boundary",
        vec![(PassingMode::Mut, TypeKind::Record(vec![].into()))],
        TypeKind::Tuple(vec![].into()),
    );
    let SignatureKind::Function(fun) = &mut boundary.kind else {
        panic!()
    };
    fun.state_promises = vec![StatePromise {
        selector: ParameterRef::bound(ParamId(0)).into(),
        advance_only: false,
        preserve_captures: true,
        preserve_unique: false,
        unretained: Default::default(),
    }]
    .into();
    f.signatures.insert(boundary.definition.clone(), boundary);
    f.valid();
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("boundary")).unwrap().kind
    else {
        panic!()
    };
    fun.state_promises[0].selector.fields = vec![FieldId(0)].into();
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::InvalidSignature(path("boundary"))]
    );
}
#[test]
fn executable_state_claims_on_any_parameter_are_proved_by_flow() {
    for (advance, write) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut f = Fixture::unit();
        let p = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
        f.body().parameters.push(LocalId(p));
        owner_signature(&mut f, PassingMode::Mut);
        if write {
            let value = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
            let displaced = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
            f.assign(value, Rvalue::Constant(Constant::Integer(7)), 0);
            f.emit(InstructionKind::Replace {
                destination: PlaceId(p),
                value: take(value),
                displaced: PlaceId(displaced),
                if_live: false,
            });
            f.emit(InstructionKind::Drop {
                place: PlaceId(displaced),
                if_live: false,
            });
        }
        f.valid();
        let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            panic!()
        };
        fun.state_promises = vec![StatePromise {
            selector: ParameterRef::bound(ParamId(0)).into(),
            advance_only: advance,
            preserve_captures: true,
            preserve_unique: false,
            unretained: Default::default(),
        }]
        .into();
        // Closed content may be rewritten under preservation; advance-only
        // forbids replacing the whole selected state.
        if advance && write {
            assert_eq!(
                f.kinds(),
                vec![ViolationKind::TypeMismatch {
                    entity: "iterator state preservation"
                }]
            );
        } else {
            f.valid();
        }
    }
}
#[test]
fn stale_protocol_receipt_is_not_an_unchecked_hint() {
    let mut f = association();
    f.valid();
    f.definition
        .protocol_associations
        .push(ProtocolAssociationReceipt {
            association: ProtocolAssociation {
                source: path("View"),
                fingerprint: ContentHash([1; 32]),
            },
            body: BodyId(0),
            open: BlockId(0),
            next: BlockId(0),
        });
    assert_eq!(f.kinds(), vec![ViolationKind::InvalidProtocolAssociation]);
}

fn calls_with_receipt() -> Fixture {
    let mut f = association();
    let view = f.signatures.get_mut(&path("View")).unwrap();
    let SignatureKind::Type(t) = &mut view.kind else {
        panic!()
    };
    t.representation = Some(interner().intern(ty(TypeKind::Record(vec![].into()))));
    let source = f.slot(
        TypeKind::Named {
            definition: id("View"),
            arguments: vec![].into(),
        },
        Storage::Local,
    );
    f.assign(
        source,
        Rvalue::Construct {
            constructor: Constructor::Record(path("View")),
            fields: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cursor = f.slot(
        TypeKind::Named {
            definition: id("Cursor"),
            arguments: vec![].into(),
        },
        Storage::Local,
    );
    let next = &f.signatures.get(id("next")).unwrap();
    let SignatureKind::Function(fun) = &next.kind else {
        panic!()
    };
    let result = fun.result;
    let option = f.slot(
        interner().get(result).unwrap().kind.clone(),
        Storage::Temporary,
    );
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("open"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(source, UseKind::Borrow),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(cursor),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    let access = f.through_loan(cursor, 0);
    let failed = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(cursor),
                mutable: true,
                if_inactive: false,
            },
            0,
        )],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("next"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(access, UseKind::Mut),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(option),
                next: BlockId(2),
            }),
            on_failure: CleanupId(failed),
        },
    ));
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        )],
        ret(0, 0),
    ));
    f.definition
        .protocol_associations
        .push(ProtocolAssociationReceipt {
            association: ProtocolAssociation {
                source: path("View"),
                fingerprint: ContentHash::default(),
            },
            body: BodyId(0),
            open: BlockId(0),
            next: BlockId(1),
        });
    f
}
#[test]
fn exact_call_receipt_has_an_accepted_twin_and_rejects_a_changed_fingerprint() {
    let mut f = calls_with_receipt();
    f.valid();
    f.definition.protocol_associations[0]
        .association
        .fingerprint = ContentHash([1; 32]);
    assert_eq!(f.kinds(), vec![ViolationKind::InvalidProtocolAssociation]);
}

#[test]
fn unused_association_rejects_private_signature_types_but_grants_private_operations() {
    let mut f = association();
    for definition in [path("View"), path("Cursor"), core_option()] {
        f.signatures
            .get_mut(&definition)
            .unwrap()
            .metadata
            .visibility = hirsel_syntax::Visibility::Public;
    }
    assert_eq!(
        f.signatures.get(id("open")).unwrap().metadata.visibility,
        hirsel_syntax::Visibility::Private
    );
    f.valid();
    f.signatures
        .get_mut(&path("Cursor"))
        .unwrap()
        .metadata
        .visibility = hirsel_syntax::Visibility::Private;
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::InvalidSignature(path("View"))]
    );
}
#[test]
fn unused_association_rejects_an_unrelated_same_shape_cursor() {
    let mut f = association();
    f.valid();
    let other = nominal("OtherCursor");
    f.signatures.insert(other.definition.clone(), other);
    let next = f.signatures.get_mut(&path("next")).unwrap();
    let SignatureKind::Function(fun) = &mut next.kind else {
        panic!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    let TypeKind::Named { definition, .. } = &mut row.kind else {
        panic!()
    };
    *definition = id("OtherCursor");
    fun.parameters[0].ty = interner().intern(row);
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::InvalidSignature(path("View"))]
    );
}

fn retained_source_calls() -> Fixture {
    let mut f = association();
    for name in ["View", "Cursor"] {
        let declaration = f.signatures.get_mut(&path(name)).unwrap();
        let string = interner().intern(ty(TypeKind::Primitive(Primitive::String)));
        let record = interner().intern(ty(TypeKind::Record(
            vec![Field {
                name: "source".into(),
                growable: false,
                ty: string,
            }]
            .into(),
        )));
        let SignatureKind::Type(t) = &mut declaration.kind else {
            panic!()
        };
        t.representation = Some(record);
        t.bounds = vec![Bound::Copyable];
    }
    let option = f.signatures.get_mut(&core_option()).unwrap();
    let parameter = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let representation = interner().intern(ty(TypeKind::Variant(
        vec![
            Variant {
                name: "None".into(),
                fields: vec![].into(),
            },
            Variant {
                name: "Some".into(),
                fields: vec![(None, parameter)].into(),
            },
        ]
        .into(),
    )));
    let SignatureKind::Type(t) = &mut option.kind else {
        panic!()
    };
    t.representation = Some(representation);
    t.bounds = vec![];
    let next = f.signatures.get_mut(&path("next")).unwrap();
    let string = interner().intern(ty(TypeKind::Primitive(Primitive::String)));
    let mut result = ty(TypeKind::Named {
        definition: DefId::intern(&core_option()),
        arguments: vec![GenericArgument::Type(string)].into(),
    });
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    let result = interner().intern(result);
    let SignatureKind::Function(fun) = &mut next.kind else {
        panic!()
    };
    fun.result = result;
    let localize = |f: &mut Fixture, p: u32| {
        let ty = f.body().places[p as usize].ty;
        let mut row = interner().get(ty).unwrap().clone();
        row.modes.relation = Relation::environment();
        let ty = f.definition.types.push(row);
        let root = f.body().places[p as usize].root;
        f.body().places[p as usize].ty = ty;
        f.body().locals[root.0 as usize].ty = ty;
    };
    let source = f.slot(
        TypeKind::Named {
            definition: id("View"),
            arguments: vec![].into(),
        },
        Storage::Parameter,
    );
    localize(&mut f, source);
    f.body().parameters.push(LocalId(source));
    owner_signature(&mut f, PassingMode::Borrow);
    let cursor = f.slot(
        TypeKind::Named {
            definition: id("Cursor"),
            arguments: vec![].into(),
        },
        Storage::Local,
    );
    localize(&mut f, cursor);
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let option = f.slot(
        TypeKind::Named {
            definition: DefId::intern(&core_option()),
            arguments: vec![GenericArgument::Type(string)].into(),
        },
        Storage::Temporary,
    );
    localize(&mut f, option);
    let read = function_signature("read", vec![], TypeKind::Tuple(vec![].into()));
    // Import the Option item into the independent callee store.
    let mut read = read;
    let item = interner().intern(ty(TypeKind::Primitive(Primitive::String)));
    let SignatureKind::Function(fun) = &mut read.kind else {
        panic!()
    };
    let ty = interner().intern(ty(TypeKind::Named {
        definition: DefId::intern(&core_option()),
        arguments: vec![GenericArgument::Type(item)].into(),
    }));
    fun.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Borrow,
        ty,
    }]
    .into();
    f.signatures.insert(read.definition.clone(), read);
    let output = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let source_access = f.through_loan(source, 0);
    let cursor_access = f.through_loan(cursor, 1);
    let option_access = f.through_loan(option, 2);
    let end = |loan| CleanupAction::EndLoan {
        loan: LoanId(loan),
        if_active: false,
    };
    let open_failure = f.cleanup(vec![end(0)]);
    let next_failure = f.cleanup(vec![end(1), drop(cursor), end(0)]);
    let read_failure = f.cleanup(vec![end(2), drop(option), end(1), drop(cursor), end(0)]);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(source),
        mutable: false,
        if_inactive: false,
    });
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("open"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(source_access, UseKind::Borrow),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(cursor),
            next: BlockId(1),
        }),
        on_failure: CleanupId(open_failure),
    };
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::BeginLoan {
                id: LoanId(1),
                place: PlaceId(cursor),
                mutable: true,
                if_inactive: false,
            },
            open_failure,
        )],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("next"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(cursor_access, UseKind::Mut),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(option),
                next: BlockId(2),
            }),
            on_failure: CleanupId(next_failure),
        },
    ));
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::BeginLoan {
                id: LoanId(2),
                place: PlaceId(option),
                mutable: false,
                if_inactive: false,
            },
            next_failure,
        )],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("read"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(option_access, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(output),
                next: BlockId(3),
            }),
            on_failure: CleanupId(read_failure),
        },
    ));
    let cleanup = f.cleanup(vec![drop(option), end(1), drop(cursor), end(0)]);
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(2),
                if_active: false,
            },
            0,
        )],
        ret(output, cleanup),
    ));
    f.definition
        .protocol_associations
        .push(ProtocolAssociationReceipt {
            association: ProtocolAssociation {
                source: path("View"),
                fingerprint: ContentHash::default(),
            },
            body: BodyId(0),
            open: BlockId(0),
            next: BlockId(1),
        });
    f
}
#[test]
fn borrowed_protocol_keeps_its_source_loan_through_next_and_yield_reads() {
    let mut f = retained_source_calls();
    f.valid();
    f.body().blocks[1].instructions.insert(
        0,
        inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        ),
    );
    for node in &mut f.body().cleanup_nodes {
        if let CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active,
        } = &mut node.action
        {
            *if_active = true
        }
    }
    let violations = f.kinds();
    assert!(!violations.is_empty());
    assert!(
        violations
            .iter()
            .all(|kind| matches!(kind, ViolationKind::ExpiredLoan { .. })),
        "{violations:?}"
    );
}

#[test]
fn borrowed_protocol_rejects_ending_retained_source_before_a_yield_read() {
    let mut f = retained_source_calls();
    f.valid();
    f.body().blocks[2].instructions.insert(
        0,
        inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        ),
    );
    for node in &mut f.body().cleanup_nodes {
        if let CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active,
        } = &mut node.action
        {
            *if_active = true
        }
    }
    let violations = f.kinds();
    assert!(!violations.is_empty());
    assert!(
        violations
            .iter()
            .all(|kind| matches!(kind, ViolationKind::ExpiredLoan { .. })),
        "{violations:?}"
    );
}

fn returned_source_calls() -> Fixture {
    let mut f = retained_source_calls();
    let option = 3;
    let cursor = 2;
    let option_type = f.body().places[option].ty;
    let mut result = interner().get(option_type).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    f.body().result = f.definition.types.push(result);
    owner_signature(&mut f, PassingMode::Borrow);
    let scope = f.body().parameter_scope;
    let result = f.body().result;
    f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
    let cleanup = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        drop(cursor),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    // The read call and its observation loan are absent on the return path.
    // Unused cleanup roots still require structurally declared loan IDs.
    for node in &mut f.body().cleanup_nodes {
        if let CleanupAction::EndLoan {
            loan: LoanId(2), ..
        } = node.action
        {
            node.action = CleanupAction::EndLoan {
                loan: LoanId(1),
                if_active: true,
            };
        }
    }
    for place in &mut f.body().places {
        if place.loan == Some(LoanId(2)) {
            place.loan = None;
        }
    }
    f.body().blocks.truncate(3);
    f.body().blocks[2] = block(vec![], ret(option as u32, cleanup));
    f
}

#[test]
fn an_owned_return_ends_frame_access_but_keeps_its_selected_source() {
    let f = returned_source_calls();
    f.valid();
}

#[test]
fn an_owned_return_cannot_erase_its_original_source_bound() {
    let mut f = returned_source_calls();
    f.valid();
    let other = f.slot(
        TypeKind::Named {
            definition: id("View"),
            arguments: vec![].into(),
        },
        Storage::Parameter,
    );
    let source_type = f.body().places[1].ty;
    let mut parameter = interner().get(source_type).unwrap().clone();
    parameter.modes.relation = Relation::environment();
    let ty = f.definition.types.push(parameter);
    let other_root = f.body().places[other as usize].root;
    f.body().locals[other_root.0 as usize].ty = ty;
    f.body().places[other as usize].ty = ty;
    f.body().parameters.push(other_root);
    let result = f.body().result;
    let mut wrong = interner().get(result).unwrap().clone();
    wrong.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(1))]);
    f.body().result = f.definition.types.push(wrong);
    owner_signature(&mut f, PassingMode::Borrow);
    let scope = f.body().parameter_scope;
    let result = f.body().result;
    f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::InvalidResultSource(PlaceId(3))),
        "{kinds:?}"
    );
}

#[test]
fn a_returned_borrow_does_not_acquire_an_owned_sharing_credit() {
    let mut f = returned_source_calls();
    f.valid();
    let Terminator::Return { value, .. } = &mut f.body().blocks[2].terminator else {
        panic!()
    };
    value.usage = UseKind::Borrow;
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::BorrowedMove(PlaceId(3))),
        "{kinds:?}"
    );
}

#[test]
fn an_ordinary_direct_call_keeps_its_local_binding_home() {
    // MODE-1, spec #100: the composition grant belongs to for protocols.
    let mut f = returned_source_calls();
    f.valid();
    f.definition.protocol_associations.clear();
    assert!(f.kinds().contains(&ViolationKind::LoanOutlivesSource {
        loan: LoanId(1),
        place: PlaceId(2),
    }));
}
