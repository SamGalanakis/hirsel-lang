//! Hand-built state-promise controls preserve ordinary type and cleanup validity.
use super::*;

fn iterator(f: &mut Fixture, storage: Storage) -> u32 {
    let element = f.definition.types.push(Type::unit());
    f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::Iter,
            arguments: vec![GenericArgument::Type(element)].into(),
        },
        storage,
    )
}
fn promise(f: &mut Fixture, advance: bool) {
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        panic!()
    };
    fun.state_promises = vec![StatePromise {
        selector: ParameterRef::bound(ParamId(0)).into(),
        advance_only: advance,
        preserve_captures: true,
    }]
    .into();
}
fn entry() -> Fixture {
    let mut f = Fixture::unit();
    let state = iterator(&mut f, Storage::Parameter);
    f.body().parameters.push(LocalId(state));
    owner_signature(&mut f, PassingMode::Mut);
    f
}
fn replacement(failure: bool) -> Fixture {
    let mut f = entry();
    let new = iterator(&mut f, Storage::Parameter);
    f.body().parameters.push(LocalId(new));
    owner_signature(&mut f, PassingMode::Mut);
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        panic!()
    };
    fun.parameters[1].passing = PassingMode::Take;
    let old = iterator(&mut f, Storage::Temporary);
    f.emit(InstructionKind::Replace {
        destination: PlaceId(1),
        value: take(new),
        displaced: PlaceId(old),
        if_live: false,
    });
    f.emit(InstructionKind::Drop {
        place: PlaceId(old),
        if_live: false,
    });
    if failure {
        let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(condition, Rvalue::Constant(Constant::Bool(false)), 0);
        f.emit(InstructionKind::Check {
            condition: copy(condition),
            rule: const { hirsel_diag::RuleId::of("MEM-14") },
        });
    }
    f
}
fn state_error(f: &Fixture) {
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "iterator state preservation"
        }]
    );
}
#[test]
fn replacement_cannot_rewind_the_same_typed_state_on_normal_or_failure_exit() {
    for advance in [false, true] {
        for failure in [false, true] {
            let mut f = replacement(failure);
            f.valid(); // Identical program, without an asserted state contract.
            promise(&mut f, advance);
            state_error(&f);
        }
    }
}
#[test]
fn readonly_iterator_state_proves_both_flags() {
    for advance in [false, true] {
        let mut f = entry();
        promise(&mut f, advance);
        f.valid();
    }
}
fn call(advance: bool, callee_advance: bool, callee_preserves: bool) -> Fixture {
    let mut f = entry();
    promise(&mut f, advance);
    let mut callee = f.signatures[&path("main")].clone();
    callee.definition = path("callee");
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    fun.state_promises[0].advance_only = callee_advance;
    if !callee_preserves {
        fun.state_promises = vec![].into();
    }
    f.signatures.insert(callee.definition.clone(), callee);
    let loaned = f.through_loan(1, 0);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable: true,
        if_inactive: false,
    });
    let cleanup = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(loaned, UseKind::Mut),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(cleanup),
    };
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
    f
}
#[test]
fn calls_prove_the_same_flag_before_both_callee_outcomes() {
    call(true, true, true).valid();
    call(false, false, true).valid();
    state_error(&call(true, false, true));
    state_error(&call(false, false, false));
}
#[test]
fn preserving_calls_keep_stronger_cleanup_but_cannot_weaken_receiving_requirements() {
    let mut f = call(false, false, true);
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        panic!()
    };
    let mut row = signature.types.get(fun.parameters[0].ty).unwrap().clone();
    row.drop_allocation = DropAllocation::MayAllocate;
    fun.parameters[0].ty = signature.types.push(row);
    f.valid();

    let mut f = call(false, false, true);
    let old = f.body().locals[1].ty;
    let mut row = f.definition.types.get(old).unwrap().clone();
    row.drop_allocation = DropAllocation::MayAllocate;
    let actual = f.definition.types.push(row);
    f.body().locals[1].ty = actual;
    for place in &mut f.body().places {
        if place.root == LocalId(1) {
            place.ty = actual;
        }
    }
    owner_signature(&mut f, PassingMode::Mut);
    promise(&mut f, false);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "call argument"
    }));
}
#[test]
fn indirect_calls_require_the_same_declared_promise() {
    for promised in [true, false] {
        let mut f = call(true, true, true);
        let mut signature = f.signatures[&path("callee")].clone();
        let SignatureKind::Function(mut fun) = signature.kind else {
            panic!()
        };
        if !promised {
            fun.state_promises = vec![].into();
        }
        let row = signature.types.push(ty(TypeKind::Function(fun)));
        let imported = import_type(&signature.types, row, &mut f.definition.types).unwrap();
        let slot = f.body().locals.len() as u32;
        f.body().locals.push(Local {
            ty: imported,
            storage: Storage::Parameter,
            scope: ScopeId(0),
            span: Span::default(),
        });
        let place = f.body().places.len() as u32;
        f.body().places.push(Place {
            root: LocalId(slot),
            projection: vec![].into(),
            loan: None,
            binding: None,
            ty: imported,
        });
        f.body().parameters.push(LocalId(slot));
        owner_signature(&mut f, PassingMode::Mut);
        let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            panic!()
        };
        fun.parameters[1].passing = PassingMode::Borrow;
        promise(&mut f, true);
        let Terminator::Call { callee, .. } = &mut f.body().blocks[0].terminator else {
            panic!()
        };
        *callee = Callee::Indirect {
            value: operand(place, UseKind::Borrow),
            arguments: vec![],
        };
        if promised { f.valid() } else { state_error(&f) }
    }
}
fn captured_state(mutable: bool) -> Fixture {
    let mut f = entry();
    let replacement = mutable.then(|| iterator(&mut f, Storage::Parameter));
    if let Some(replacement) = replacement {
        f.body().parameters.push(LocalId(replacement));
        owner_signature(&mut f, PassingMode::Mut);
        let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            panic!()
        };
        fun.parameters[1].passing = PassingMode::Take;
    }
    let mut nested = f.body().clone();
    nested.parameter_scope = NameSite::Expression(hirsel_syntax::ExprId(0));
    nested.parameters.clear();
    nested.locals[1].storage = Storage::Capture;
    let mut captured = f.definition.types.get(nested.locals[1].ty).unwrap().clone();
    captured.modes.relation = Relation::environment();
    let captured = f.definition.types.push(captured);
    nested.locals[1].ty = captured;
    nested.places[1].ty = captured;
    if let Some(replacement) = replacement {
        nested.parameters.push(LocalId(replacement));
        let displaced = nested.locals.len() as u32;
        let mut local = nested.locals[replacement as usize].clone();
        local.storage = Storage::Temporary;
        nested.locals.push(local);
        let mut place = nested.places[replacement as usize].clone();
        place.root = LocalId(displaced);
        nested.places.push(place);
        nested.blocks[0].instructions.extend([
            inst(
                InstructionKind::Replace {
                    destination: PlaceId(1),
                    value: take(replacement),
                    displaced: PlaceId(displaced),
                    if_live: false,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(displaced),
                    if_live: false,
                },
                0,
            ),
        ]);
    }
    f.definition.bodies.push(nested);
    let SignatureKind::Function(mut fun) =
        function_signature("unused", vec![], TypeKind::Tuple(vec![].into())).kind
    else {
        panic!()
    };
    if let Some(replacement) = replacement {
        fun.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Take,
            ty: f.body().locals[replacement as usize].ty,
        }]
        .into();
    }
    fun.result = f.body().result;
    fun.uses = Uses::Unrestricted;
    fun.authority = AuthoritySummary::Unknown;
    let closure = f.slot(TypeKind::Function(fun), Storage::Temporary);
    let mut row = f
        .definition
        .types
        .get(f.definition.bodies[0].places[closure as usize].ty)
        .unwrap()
        .clone();
    row.modes.relation = Relation::environment();
    row.modes.sendability = Sendability::NotSendable;
    let row = f.definition.types.push(row);
    f.body().places[closure as usize].ty = row;
    f.body().locals[closure as usize].ty = row;
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable,
        if_inactive: false,
    });
    let access = f.through_loan(1, 0);
    let mut failure = vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }];
    failure.extend(replacement.map(drop));
    let failure = f.cleanup(failure);
    f.assign(
        closure,
        Rvalue::Closure {
            body: BodyId(1),
            captures: vec![operand(
                access,
                if mutable {
                    UseKind::Mut
                } else {
                    UseKind::Borrow
                },
            )],
            destination: Destination::Inferred,
        },
        failure,
    );
    let cleanup = f.cleanup(vec![
        drop(closure),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    if let Some(replacement) = replacement {
        let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Indirect {
                value: operand(closure, UseKind::Borrow),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(replacement),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(destination),
                next: BlockId(1),
            }),
            on_failure: CleanupId(cleanup),
        };
        f.body().blocks.push(block(vec![], ret(0, cleanup)));
    } else {
        f.body().blocks[0].terminator = ret(0, cleanup);
    }
    f
}
#[test]
fn a_mutable_capture_cannot_hide_replacement_of_the_promised_state() {
    for advance in [false, true] {
        let mut readonly = captured_state(false);
        promise(&mut readonly, advance);
        readonly.valid();
        let mut f = captured_state(true);
        f.valid();
        promise(&mut f, advance);
        state_error(&f);
    }
}

fn creation_return(hidden: bool) -> Fixture {
    let mut f = Fixture::unit();
    let selected = iterator(&mut f, Storage::Parameter);
    let other = iterator(&mut f, Storage::Parameter);
    f.body().parameters = vec![LocalId(selected), LocalId(other)];
    let incoming = f.body().locals[selected as usize].ty;
    let mut result = f.definition.types.get(incoming).unwrap().clone();
    result.erased_captures = Some(ErasedCaptures::FromArguments(
        vec![ParameterRef::body(f.body().parameter_scope, ParamId(0)).into()].into(),
    ));
    let result = f.definition.types.push(result);
    f.body().result = result;
    owner_signature(&mut f, PassingMode::Take);
    let scope = f.body().parameter_scope;
    let signature = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(function) = &mut signature.kind else {
        panic!()
    };
    function.result =
        map_parameter_references(&mut signature.types, function.result, |parameter, depth| {
            Ok(match parameter {
                ParameterRef::Body {
                    scope: owner,
                    parameter,
                } if owner == scope => ParameterRef::Bound { depth, parameter },
                parameter => parameter,
            })
        })
        .unwrap();
    let (returned, discarded) = if hidden {
        (other, selected)
    } else {
        (selected, other)
    };
    let cleanup = f.cleanup(vec![drop(discarded)]);
    f.body().blocks[0].terminator = ret(returned, cleanup);
    f
}

#[test]
fn a_creation_result_cannot_substitute_a_same_typed_unselected_environment() {
    creation_return(false).valid();
    assert_eq!(
        creation_return(true).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "creation capture envelope",
        }]
    );
}

fn creation_relay(hidden: bool) -> Fixture {
    let mut f = creation_return(hidden);
    let input = if hidden { 2 } else { 1 };
    let discarded = if hidden { 1 } else { 2 };
    let payload = f.body().locals[input].ty;
    let mut types = TypeStore::default();
    let parameter = types.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let representation = types.push(ty(TypeKind::Record(
        vec![Field {
            name: "state".into(),
            ty: parameter,
            growable: false,
        }]
        .into(),
    )));
    f.signatures.insert(
        path("Relay"),
        Signature {
            definition: path("Relay"),
            metadata: Default::default(),
            types,
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                generics: vec![GenericParameter::Type {
                    name: "S".into(),
                    bounds: vec![].into(),
                }],
                representation: Some(representation),
                bounds: vec![],
                authority: AuthoritySummary::Known(
                    vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                        TypeParamId(0),
                    ))]
                    .into(),
                ),
                capability: None,
                implementation_authority: None,
                serializable: false,
                layout: Default::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
                destructor_allocation: Allocation::NoAlloc,
            }),
            fingerprint: ContentHash::default(),
        },
    );
    let relay = f.slot(
        TypeKind::Named {
            definition: path("Relay"),
            arguments: vec![GenericArgument::Type(payload)].into(),
        },
        Storage::Temporary,
    );
    let failure = f.cleanup(vec![drop(2), drop(1)]);
    f.assign(
        relay,
        Rvalue::Construct {
            constructor: Constructor::Record(path("Relay")),
            fields: vec![ConstructField {
                field: 0,
                value: take(input as u32),
            }],
            destination: Destination::Inferred,
        },
        failure,
    );
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        root: LocalId(relay),
        projection: vec![Projection::Field(0)].into(),
        ty: payload,
        loan: None,
        binding: None,
    });
    let cleanup = f.cleanup(vec![drop(relay), drop(discarded)]);
    f.body().blocks[0].terminator = ret(field, cleanup);
    f
}

#[test]
fn generic_record_transport_cannot_launder_an_unselected_creation_input() {
    creation_relay(false).valid();
    assert_eq!(
        creation_relay(true).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "creation capture envelope",
        }]
    );
}

fn mutable_creation(scalar: bool) -> Fixture {
    mutable_creation_with(scalar, |_| {})
}
/// `mutable_creation` whose state row is first adjusted on one axis.
fn mutable_creation_with(scalar: bool, adjust: impl FnOnce(&mut Type)) -> Fixture {
    let mut f = Fixture::unit();
    let unit = f.body().result;
    let selected = if scalar {
        f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter)
    } else {
        iterator(&mut f, Storage::Parameter)
    };
    let initial = f.body().locals[selected as usize].ty;
    let mut row = f.definition.types.get(initial).unwrap().clone();
    adjust(&mut row);
    let selected_ty = f.definition.types.push(row);
    f.body().locals[selected as usize].ty = selected_ty;
    f.body().places[selected as usize].ty = selected_ty;
    let SignatureKind::Function(mut callback) =
        function_signature("unused", vec![], TypeKind::Tuple(vec![].into())).kind
    else {
        panic!()
    };
    callback.result = unit;
    callback.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Mut,
        ty: selected_ty,
    }]
    .into();
    let next = f.slot(TypeKind::Function(callback.clone()), Storage::Parameter);
    f.body().parameters = vec![LocalId(selected), LocalId(next)];
    let destination = iterator(&mut f, Storage::Temporary);
    let current = f.body().locals[destination as usize].ty;
    let mut result = f.definition.types.get(current).unwrap().clone();
    result.erased_captures = Some(ErasedCaptures::Known(if scalar {
        AuthoritySummary::inert()
    } else {
        AuthoritySummary::Unknown
    }));
    let destination_ty = f.definition.types.push(result.clone());
    f.body().locals[destination as usize].ty = destination_ty;
    f.body().places[destination as usize].ty = destination_ty;
    result.erased_captures = Some(ErasedCaptures::FromArguments(
        (0..2)
            .map(|ordinal| ParameterRef::body(f.body().parameter_scope, ParamId(ordinal)).into())
            .collect(),
    ));
    let result = f.definition.types.push(result);
    f.body().result = result;
    owner_signature(&mut f, PassingMode::Take);
    let scope = f.body().parameter_scope;
    let owner = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(owner_function) = &mut owner.kind else {
        panic!()
    };
    owner_function.result = map_parameter_references(
        &mut owner.types,
        owner_function.result,
        |parameter, depth| {
            Ok(match parameter {
                ParameterRef::Body {
                    scope: from,
                    parameter,
                } if from == scope => ParameterRef::Bound { depth, parameter },
                parameter => parameter,
            })
        },
    )
    .unwrap();

    let mut factory = function_signature("factory", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(mut function) = factory.kind else {
        panic!()
    };
    let unit = function.result;
    let state = factory
        .types
        .push(ty(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
    let inner = factory
        .types
        .push(ty(TypeKind::Parameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        })));
    callback.result = unit;
    callback.parameters[0].ty = inner;
    let next = factory.types.push(ty(TypeKind::Function(callback)));
    let mut result = ty(TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(unit)].into(),
    });
    result.erased_captures = Some(ErasedCaptures::FromArguments(
        (0..2)
            .map(|ordinal| ParameterRef::bound(ParamId(ordinal)).into())
            .collect(),
    ));
    function.result = factory.types.push(result);
    function.generics = vec![GenericParameter::Type {
        name: "S".into(),
        bounds: vec![].into(),
    }]
    .into();
    function.parameters = [state, next]
        .into_iter()
        .map(|ty| Parameter {
            name: None,
            label: None,
            passing: PassingMode::Take,
            ty,
        })
        .collect();
    factory.kind = SignatureKind::Function(function);
    f.signatures.insert(path("factory"), factory);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("factory"),
            arguments: vec![GenericArgument::Type(selected_ty)],
        },
        arguments: (0..2)
            .map(|ordinal| CallArgument {
                parameter: ParamId(ordinal),
                value: take(ordinal + 1),
            })
            .collect(),
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(destination, 0)));
    f
}

#[test]
fn creation_retains_callback_mutable_receipts_instead_of_initial_crossing_evidence() {
    mutable_creation(true).valid();
    // Relaying the exact incoming concrete pair keeps the caller's receipt.
    mutable_creation(false).valid();
    for promised in [false, true] {
        let f = referenced_callback(promised);
        if promised {
            f.valid();
        } else {
            assert_eq!(
                f.kinds(),
                vec![ViolationKind::TypeMismatch {
                    entity: "creation capture envelope"
                }]
            );
        }
    }
}

/// The erased state is advanced by a referenced function instead of the
/// incoming callback. Only its declared whole-slot preservation, proved where
/// it is implemented, keeps the state's snapshot on future invocations.
fn referenced_callback(promised: bool) -> Fixture {
    let mut f = mutable_creation(false);
    let SignatureKind::Function(owner) = &f.signatures[&path("main")].kind else {
        panic!()
    };
    let callback_ty = owner.parameters[1].ty;
    let types = f.signatures[&path("main")].types.clone();
    let mut stepper = function_signature("stepper", vec![], TypeKind::Tuple(vec![].into()));
    let imported = hirsel_types::import_type(&types, callback_ty, &mut stepper.types).unwrap();
    let TypeKind::Function(mut header) = stepper.types.get(imported).unwrap().kind.clone() else {
        panic!()
    };
    if promised {
        header.state_promises = vec![StatePromise {
            selector: ParameterRef::bound(ParamId(0)).into(),
            advance_only: false,
            preserve_captures: true,
        }]
        .into();
    }
    stepper.kind = SignatureKind::Function(header.clone());
    f.signatures.insert(path("stepper"), stepper);
    let local_ty = f.body().locals[2].ty;
    let mut row = f.definition.types.get(local_ty).unwrap().clone();
    let TypeKind::Function(mut local) = row.kind.clone() else {
        panic!()
    };
    local.state_promises = header.state_promises.clone();
    // A capture-free function reference is an independently inert input.
    local.authority = AuthoritySummary::inert();
    local.uses = Uses::Only(AuthoritySummary::inert());
    row.modes.relation = Relation::global();
    row.modes.multiplicity = hirsel_types::Multiplicity::Many;
    row.modes.sendability = Sendability::Sendable;
    row.drop_allocation = hirsel_types::DropAllocation::NoAlloc;
    row.kind = TypeKind::Function(local);
    let reference = f.slot(row.kind.clone(), Storage::Temporary);
    let reference_ty = f.definition.types.push(row);
    f.body().locals[reference as usize].ty = reference_ty;
    f.body().places[reference as usize].ty = reference_ty;
    f.assign(
        reference,
        Rvalue::FunctionReference {
            definition: path("stepper"),
            arguments: vec![],
        },
        0,
    );
    let failure = f.cleanup(vec![drop(2), drop(reference)]);
    let Terminator::Call {
        arguments,
        on_failure,
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    arguments[1].value = take(reference);
    *on_failure = CleanupId(failure);
    let exit = f.cleanup(vec![drop(2)]);
    let Terminator::Return { cleanup, .. } = &mut f.body().blocks[1].terminator else {
        panic!()
    };
    *cleanup = CleanupId(exit);
    f
}

fn bounded_creation(noalloc: bool) -> Fixture {
    let mut bounds = vec![Bound::Inert, Bound::Sendable];
    if noalloc {
        bounds.push(Bound::NoallocDrop);
    }
    bounded_creation_with(bounds)
}
fn bounded_creation_with(bounds: Vec<Bound>) -> Fixture {
    let mut f = mutable_creation(true);
    if !bounds.contains(&Bound::Inert) {
        // The created iterator retains the state's generic authority.
        let destination = f.body().locals[3].ty;
        let scope = f.body().parameter_scope;
        let mut row = f.definition.types.get(destination).unwrap().clone();
        row.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
            vec![AuthorityTerm::TypeParameter(TypeParameterRef::body(
                scope,
                TypeParamId(0),
            ))]
            .into(),
        )));
        let row = f.definition.types.push(row);
        f.body().locals[3].ty = row;
        f.body().places[3].ty = row;
    }
    let factory = f.signatures.get_mut(&path("factory")).unwrap();
    let SignatureKind::Function(factory_function) = &mut factory.kind else {
        panic!()
    };
    let mut row = factory
        .types
        .get(factory_function.parameters[0].ty)
        .unwrap()
        .clone();
    row.drop_allocation =
        DropAllocation::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into());
    factory_function.parameters[0].ty = factory.types.push(row);
    let scope = f.body().parameter_scope;
    let mut state = ty(TypeKind::Parameter(TypeParameterRef::body(
        scope,
        TypeParamId(0),
    )));
    if bounds.contains(&Bound::Sendable) {
        state.modes.sendability = Sendability::Sendable;
    }
    let state = f.definition.types.push(state);
    f.body().locals[1].ty = state;
    f.body().places[1].ty = state;
    let next = f.body().locals[2].ty;
    let mut next = f.definition.types.get(next).unwrap().clone();
    let TypeKind::Function(callback) = &mut next.kind else {
        panic!()
    };
    callback.parameters[0].ty = state;
    let next = f.definition.types.push(next);
    f.body().locals[2].ty = next;
    f.body().places[2].ty = next;
    let mut argument = f.definition.types.get(state).unwrap().clone();
    argument.drop_allocation =
        DropAllocation::DependsOn(vec![TypeParameterRef::body(scope, TypeParamId(0))].into());
    let argument = f.definition.types.push(argument);
    let Terminator::Call {
        callee: Callee::Definition { arguments, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    arguments[0] = GenericArgument::Type(argument);
    owner_signature(&mut f, PassingMode::Take);
    let owner = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(function) = &mut owner.kind else {
        panic!()
    };
    function.generics = vec![GenericParameter::Type {
        name: "S".into(),
        bounds: bounds.into(),
    }]
    .into();
    function.result =
        map_parameter_references(&mut owner.types, function.result, |parameter, depth| {
            Ok(match parameter {
                ParameterRef::Body {
                    scope: from,
                    parameter,
                } if from == scope => ParameterRef::Bound { depth, parameter },
                parameter => parameter,
            })
        })
        .unwrap();
    f
}

#[test]
fn generic_creation_discharges_cleanup_conjunctions_only_with_its_own_bound() {
    bounded_creation(true).valid();
    let errors = bounded_creation(false).kinds();
    assert!(
        errors.contains(&ViolationKind::TypeMismatch {
            entity: "call argument"
        }),
        "{errors:?}"
    );
}

/// `mut s: {it: Iter, spare: Iter}` whose field `replaced` is rebound from a
/// taken parameter, under a promise selecting `s.it` (field zero).
fn field_state(replaced: u32, advance: bool) -> Fixture {
    let mut f = Fixture::unit();
    let element = f.definition.types.push(Type::unit());
    let iter = || TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(element)].into(),
    };
    let part = f.definition.types.push(ty(iter()));
    let fields = ["it", "spare"]
        .into_iter()
        .map(|name| Field {
            name: name.into(),
            ty: part,
            growable: false,
        })
        .collect::<Vec<_>>();
    let state = f.slot(TypeKind::Record(fields.into()), Storage::Parameter);
    let other = f.slot(iter(), Storage::Parameter);
    f.body().parameters = vec![LocalId(state), LocalId(other)];
    owner_signature(&mut f, PassingMode::Mut);
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        panic!()
    };
    fun.parameters[1].passing = PassingMode::Take;
    let place = f.body().places.len() as u32;
    let place_ty = f.body().locals[other as usize].ty;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(state),
        projection: vec![Projection::Field(replaced)].into(),
        ty: place_ty,
    });
    let displaced = iterator(&mut f, Storage::Temporary);
    f.emit(InstructionKind::Replace {
        destination: PlaceId(place),
        value: take(other),
        displaced: PlaceId(displaced),
        if_live: false,
    });
    f.emit(InstructionKind::Drop {
        place: PlaceId(displaced),
        if_live: false,
    });
    f.valid(); // The same program without a state contract.
    select_field(&mut f, vec![FieldId(0)], advance);
    f
}
fn select_field(f: &mut Fixture, fields: Vec<FieldId>, advance: bool) {
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        panic!()
    };
    fun.state_promises = vec![StatePromise {
        selector: RelationPlace {
            parameter: ParameterRef::bound(ParamId(0)),
            fields: fields.into(),
        },
        advance_only: advance,
        preserve_captures: true,
    }]
    .into();
}
#[test]
fn a_selected_field_cannot_be_rebound_while_its_sibling_may() {
    for advance in [false, true] {
        field_state(1, advance).valid();
        state_error(&field_state(0, advance));
        // Selecting the whole record covers both fields.
        let mut f = field_state(1, advance);
        select_field(&mut f, vec![], advance);
        state_error(&f);
    }
}

/// `mut s: {it: Iter, spare: Iter}` passed whole to a callee promising
/// `callee_fields` of its parameter, under a promise selecting `s.it`.
fn field_call(callee_fields: Vec<FieldId>) -> Fixture {
    let mut f = Fixture::unit();
    let element = f.definition.types.push(Type::unit());
    let part = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(element)].into(),
    }));
    let fields = ["it", "spare"]
        .into_iter()
        .map(|name| Field {
            name: name.into(),
            ty: part,
            growable: false,
        })
        .collect::<Vec<_>>();
    let state = f.slot(TypeKind::Record(fields.into()), Storage::Parameter);
    f.body().parameters = vec![LocalId(state)];
    owner_signature(&mut f, PassingMode::Mut);
    let mut callee = f.signatures[&path("main")].clone();
    callee.definition = path("callee");
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    fun.state_promises = vec![StatePromise {
        selector: RelationPlace {
            parameter: ParameterRef::bound(ParamId(0)),
            fields: callee_fields.into(),
        },
        advance_only: true,
        preserve_captures: true,
    }]
    .into();
    f.signatures.insert(callee.definition.clone(), callee);
    let loaned = f.through_loan(state, 0);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(state),
        mutable: true,
        if_inactive: false,
    });
    let cleanup = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(loaned, UseKind::Mut),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(cleanup),
    };
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
    f.valid();
    select_field(&mut f, vec![FieldId(0)], true);
    f
}
#[test]
fn a_callee_must_promise_a_region_covering_the_selected_field() {
    field_call(vec![]).valid();
    field_call(vec![FieldId(0)]).valid();
    state_error(&field_call(vec![FieldId(1)]));
}

/// The creation body of `creation_return`, as a closure literal whose own
/// callable type selects its parameter zero.
fn closure_creation(hidden: bool) -> Fixture {
    let mut f = creation_return(hidden);
    let mut nested = f.body().clone();
    let item = nested.parameter_scope;
    let scope = NameSite::Expression(hirsel_syntax::ExprId(0));
    nested.parameter_scope = scope;
    nested.result = map_parameter_references(&mut f.definition.types, nested.result, |p, _| {
        Ok(match p {
            ParameterRef::Body {
                scope: s,
                parameter,
            } if s == item => ParameterRef::body(scope, parameter),
            p => p,
        })
    })
    .unwrap();
    let result = map_parameter_references(&mut f.definition.types, nested.result, |p, depth| {
        Ok(match p {
            ParameterRef::Body {
                scope: s,
                parameter,
            } if s == scope => ParameterRef::Bound { depth, parameter },
            p => p,
        })
    })
    .unwrap();
    let parameters = nested
        .parameters
        .iter()
        .map(|l| Parameter {
            name: None,
            label: None,
            passing: PassingMode::Take,
            ty: nested.locals[l.0 as usize].ty,
        })
        .collect();
    let mut outer = Fixture::unit().definition.bodies[0].clone();
    let unit = nested.locals[0].ty;
    outer.locals[0].ty = unit;
    outer.places[0].ty = unit;
    outer.result = unit;
    f.definition.bodies = vec![outer, nested];
    owner_signature(&mut f, PassingMode::Take);
    let SignatureKind::Function(mut fun) =
        function_signature("unused", vec![], TypeKind::Tuple(vec![].into())).kind
    else {
        panic!()
    };
    fun.parameters = parameters;
    fun.result = result;
    fun.allocation = Allocation::MayAllocate;
    fun.uses = Uses::Unrestricted;
    fun.authority = AuthoritySummary::Unknown;
    let closure = f.slot(TypeKind::Function(fun), Storage::Temporary);
    f.assign(
        closure,
        Rvalue::Closure {
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn a_closure_creation_body_proves_its_own_callable_selection() {
    closure_creation(false).valid();
    assert_eq!(
        closure_creation(true).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "creation capture envelope",
        }]
    );
}

/// Narrow a creation owner's selection to its callback parameter alone.
fn select_only_callback(f: &mut Fixture) {
    let owner = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(function) = &mut owner.kind else {
        panic!()
    };
    let mut row = owner.types.get(function.result).unwrap().clone();
    row.erased_captures = Some(ErasedCaptures::FromArguments(
        vec![ParameterRef::bound(ParamId(1)).into()].into(),
    ));
    function.result = owner.types.push(row);
    let scope = f.body().parameter_scope;
    let result = f.body().result;
    let mut row = f.definition.types.get(result).unwrap().clone();
    row.erased_captures = Some(ErasedCaptures::FromArguments(
        vec![ParameterRef::body(scope, ParamId(1)).into()].into(),
    ));
    f.body().result = f.definition.types.push(row);
}

/// Each envelope axis of an unselected retained state: the state is hidden
/// unless it is independently inert on that axis, and selecting it admits it.
#[test]
fn every_envelope_axis_of_an_unselected_retained_state_is_a_hidden_input() {
    let mut base = mutable_creation(true);
    select_only_callback(&mut base);
    base.valid();
    // Typed authority transport already refuses a result that names
    // unselected generic authority; every other axis is a flow envelope.
    let axes: Vec<(&str, &str, Box<dyn Fn() -> Fixture>)> = vec![
        (
            "once",
            "creation capture envelope",
            Box::new(|| {
                mutable_creation_with(true, |row| row.modes.multiplicity = Multiplicity::Once)
            }),
        ),
        (
            "authority",
            "return",
            Box::new(|| bounded_creation_with(vec![Bound::Sendable, Bound::NoallocDrop])),
        ),
        (
            "retained source",
            "creation capture envelope",
            Box::new(|| {
                mutable_creation_with(true, |row| row.modes.relation = Relation::environment())
            }),
        ),
        (
            "sendability",
            "creation capture envelope",
            Box::new(|| bounded_creation_with(vec![Bound::Inert, Bound::NoallocDrop])),
        ),
        (
            "allocating drop",
            "creation capture envelope",
            Box::new(|| {
                mutable_creation_with(true, |row| {
                    row.drop_allocation = DropAllocation::MayAllocate
                })
            }),
        ),
    ];
    for (axis, entity, build) in axes {
        assert_eq!(build().kinds(), vec![], "selected {axis} state");
        let mut f = build();
        select_only_callback(&mut f);
        assert_eq!(
            f.kinds(),
            vec![ViolationKind::TypeMismatch { entity }],
            "unselected {axis} state"
        );
    }
}
