use super::*;

fn row(f: &mut Fixture, place: u32, change: impl FnOnce(&mut Type)) {
    let old = f.body().places[place as usize].ty;
    let mut ty = interner().get(old).unwrap().clone();
    change(&mut ty);
    let ty = f.definition.types.push(ty);
    let root = f.body().places[place as usize].root;
    f.body().locals[root.0 as usize].ty = ty;
    f.body().places[place as usize].ty = ty;
}

// Deliberately hand-built: no source checker or lowering participates.
fn unplaced_join(escape: bool) -> Fixture {
    let mut f = Fixture::unit();
    let source = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters = vec![LocalId(source), LocalId(c)];
    let destination = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    row(&mut f, destination, |t| {
        t.modes.relation = Relation::environment()
    });
    for region in 0..2 {
        let value = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        row(&mut f, value, |t| {
            t.modes.relation = t
                .modes
                .relation
                .union(&Relation::new([Reach::Region(RegionId(region))]))
        });
        let constant = RvalueId(f.body().rvalues.len() as u32);
        f.body()
            .rvalues
            .push(Rvalue::Constant(Constant::String("scoped".into())));
        let transport = RvalueId(f.body().rvalues.len() as u32);
        f.body().rvalues.push(Rvalue::Use(take(value)));
        f.body().blocks.push(block(
            vec![
                inst(
                    InstructionKind::Assign {
                        destination: PlaceId(value),
                        value: constant,
                    },
                    0,
                ),
                inst(
                    InstructionKind::Assign {
                        destination: PlaceId(destination),
                        value: transport,
                    },
                    0,
                ),
            ],
            Terminator::Goto(BlockId(3)),
        ));
    }
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(c),
        yes: BlockId(1),
        no: BlockId(2),
    };
    let cleanup = f.cleanup(vec![drop(destination)]);
    f.body().blocks.push(block(
        vec![],
        if escape {
            ret(destination, 0)
        } else {
            ret(0, cleanup)
        },
    ));
    if escape {
        let mut result = interner()
            .get(f.definition.bodies[0].places[destination as usize].ty)
            .unwrap()
            .clone();
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        let result = f.definition.types.push(result);
        f.body().result = result;
    }
    owner_signature(&mut f, PassingMode::Borrow);
    if escape {
        let scope = f.body().parameter_scope;
        let result = f.body().result;
        f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
    }
    f
}

#[test]
fn mixed_placement_join_cannot_escape_an_unplaced_holder_frame() {
    unplaced_join(false).valid();
    let f = unplaced_join(true);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(3))),
        "{:?}",
        f.kinds()
    );
}

// The same join one level down: a local tuple whose string part lost its
// region placement (PROV-4 at every depth). Deliberately hand-built.
fn unplaced_part_join(escape: bool) -> Fixture {
    let mut f = Fixture::unit();
    let source = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters = vec![LocalId(source), LocalId(c)];
    let byte = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let mut part = ty(TypeKind::Primitive(Primitive::String));
    part.modes.relation = Relation::environment();
    let part = f.definition.types.push(part);
    let destination = f.slot(TypeKind::Tuple(vec![part, byte].into()), Storage::Temporary);
    row(&mut f, destination, |t| {
        t.modes.relation = Relation::environment()
    });
    for region in 0..2 {
        let mut scoped = ty(TypeKind::Primitive(Primitive::String));
        scoped.modes.relation = scoped
            .modes
            .relation
            .union(&Relation::new([Reach::Region(RegionId(region))]));
        let scoped = f.definition.types.push(scoped);
        let text = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        row(&mut f, text, |t| {
            t.modes.relation = t
                .modes
                .relation
                .union(&Relation::new([Reach::Region(RegionId(region))]))
        });
        let number = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
        let value = f.slot(
            TypeKind::Tuple(vec![scoped, byte].into()),
            Storage::Temporary,
        );
        let first = f.body().rvalues.len() as u32;
        f.body().rvalues.extend([
            Rvalue::Constant(Constant::String("scoped".into())),
            Rvalue::Constant(Constant::Integer(1)),
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: take(text),
                    },
                    ConstructField {
                        field: 1,
                        value: take(number),
                    },
                ],
                destination: Destination::Inferred,
            },
            Rvalue::Use(take(value)),
        ]);
        let operands = f.cleanup(vec![drop(number), drop(text)]);
        let assign = |destination: u32, value: u32, cleanup| {
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(destination),
                    value: RvalueId(first + value),
                },
                cleanup,
            )
        };
        f.body().blocks.push(block(
            vec![
                assign(text, 0, 0),
                assign(number, 1, 0),
                assign(value, 2, operands),
                assign(destination, 3, 0),
            ],
            Terminator::Goto(BlockId(3)),
        ));
    }
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(c),
        yes: BlockId(1),
        no: BlockId(2),
    };
    let cleanup = f.cleanup(vec![drop(destination)]);
    f.body().blocks.push(block(
        vec![],
        if escape {
            ret(destination, 0)
        } else {
            ret(0, cleanup)
        },
    ));
    if escape {
        let mut result = interner()
            .get(f.definition.bodies[0].places[destination as usize].ty)
            .unwrap()
            .clone();
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        let result = f.definition.types.push(result);
        f.body().result = result;
    }
    owner_signature(&mut f, PassingMode::Borrow);
    if escape {
        let scope = f.body().parameter_scope;
        let result = f.body().result;
        f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
    }
    f
}

#[test]
fn a_part_joined_from_scoped_content_is_unplaced_and_bounded_by_its_frame() {
    unplaced_part_join(false).valid();
    let f = unplaced_part_join(true);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(3))),
        "{:?}",
        f.kinds()
    );
}

fn dependent_cursor(global: bool) -> Fixture {
    let mut f = Fixture::unit();
    let element = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let list = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(element)].into(),
        },
        Storage::Parameter,
    );
    row(&mut f, list, |t| t.modes.relation = Relation::environment());
    f.body().parameters.push(LocalId(list));
    let cursor = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::Iter,
            arguments: vec![GenericArgument::Type(element)].into(),
        },
        Storage::Temporary,
    );
    if !global {
        row(&mut f, cursor, |t| {
            t.modes.relation = Relation::environment()
        });
    }
    let mut signature = function_signature("consume", vec![], TypeKind::Tuple(vec![].into()));
    let parameter = f.definition.bodies[0].places[list as usize].ty;
    let mut result = interner()
        .get(f.definition.bodies[0].places[cursor as usize].ty)
        .unwrap()
        .clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    let result = interner().intern(result);
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    fun.parameters.push(Parameter {
        name: None,
        label: None,
        passing: PassingMode::Take,
        ty: parameter,
    });
    fun.result = result;
    f.signatures.insert(id("consume"), signature);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("consume"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: take(list),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(cursor),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    let cleanup = f.cleanup(vec![drop(cursor)]);
    f.body().blocks.push(block(vec![], ret(0, cleanup)));
    owner_signature(&mut f, PassingMode::Take);
    f
}

#[test]
fn consuming_cursor_cannot_forge_global_content_from_a_local_owner() {
    dependent_cursor(false).valid();
    let f = dependent_cursor(true);
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "call result"
        }),
        "{:?}",
        f.kinds()
    );
}

fn fresh_capture(refill: bool) -> Fixture {
    let mut f = copied_capture_fixture(Uniqueness::Aliased, Uniqueness::Aliased);
    f.body().blocks[0].instructions.pop(); // Replace the independent copied capture below.
    f.body().rvalues.pop();
    let closure = 2;
    row(&mut f, 1, |t| t.modes.relation = Relation::environment());
    row(&mut f, closure, |t| {
        t.modes.relation = Relation::environment()
    });
    let nested = &mut f.definition.bodies[1];
    nested.cleanups = vec![Cleanup::default()];
    nested.cleanup_nodes.clear();
    nested.blocks[0].terminator = ret(0, 0);
    // CLO-1: the borrowed capture is the body's local view of the slot, and
    // the body's local result is promised global only over global content.
    for place in [0, 1] {
        let nested = &f.definition.bodies[1];
        let mut ty = interner().get(nested.places[place].ty).unwrap().clone();
        ty.modes.relation = Relation::environment();
        let ty = f.definition.types.push(ty);
        let nested = &mut f.definition.bodies[1];
        nested.locals[place].ty = ty;
        nested.places[place].ty = ty;
        if place == 0 {
            nested.result = ty;
        }
    }
    let fresh = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(fresh, Rvalue::Constant(Constant::String("fresh".into())), 0);
    let old = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    row(&mut f, old, |t| t.modes.relation = Relation::environment());
    f.emit(InstructionKind::Replace {
        destination: PlaceId(1),
        value: take(fresh),
        displaced: PlaceId(old),
        if_live: false,
    });
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(1);
    f.emit(InstructionKind::Drop {
        place: PlaceId(old),
        if_live: false,
    });
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(1);
    if refill {
        let parameter = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
        row(&mut f, parameter, |t| {
            t.modes.relation = Relation::environment()
        });
        f.body().parameters.push(LocalId(parameter));
        f.emit(InstructionKind::Replace {
            destination: PlaceId(1),
            value: copy(parameter),
            displaced: PlaceId(old),
            if_live: false,
        });
        f.body().blocks[0]
            .instructions
            .last_mut()
            .unwrap()
            .on_failure = CleanupId(1);
        f.emit(InstructionKind::Drop {
            place: PlaceId(old),
            if_live: false,
        });
        f.body().blocks[0]
            .instructions
            .last_mut()
            .unwrap()
            .on_failure = CleanupId(1);
    }
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable: false,
        if_inactive: false,
    });
    let borrowed = f.through_loan(1, 0);
    let failure = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: true,
        },
        drop(1),
    ]);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(borrowed, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        failure,
    );
    let cleanup = f.cleanup(vec![
        drop(closure),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: true,
        },
        drop(1),
    ]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

#[test]
fn fresh_local_capture_requires_the_slots_current_global_content() {
    fresh_capture(false).valid();
    let f = fresh_capture(true);
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "closure capture"
        }),
        "{:?}",
        f.kinds()
    );
}

fn captured_name(second_carrier: bool) -> Fixture {
    let mut f = copied_capture_fixture(Uniqueness::Aliased, Uniqueness::Aliased);
    let name = Origin {
        root: OriginRoot::Expression(lexical(), hirsel_syntax::ExprId(71)),
        fields: vec![].into(),
    };
    let named = f.body().places.len() as u32;
    let mut access = f.body().places[1].clone();
    access.binding = Some(name.clone());
    f.body().places.push(access);
    f.body().blocks[0].instructions.insert(
        2,
        inst(
            InstructionKind::BindOrigin {
                source: PlaceId(1),
                place: PlaceId(named),
                kind: OriginBindingKind::Fresh,
            },
            0,
        ),
    );
    let Rvalue::Closure { captures, .. } = f.body().rvalues.last_mut().unwrap() else {
        unreachable!()
    };
    captures[0] = copy(named);
    let child = &mut f.definition.bodies[1];
    child.places[1].binding = Some(name.clone());
    child.blocks[0].instructions.push(inst(
        InstructionKind::BindOrigin {
            source: PlaceId(1),
            place: PlaceId(1),
            kind: OriginBindingKind::Fresh,
        },
        0,
    ));
    if second_carrier {
        let mut local = child.locals[1].clone();
        local.storage = Storage::Temporary;
        child.locals.push(local);
        let mut place = child.places[1].clone();
        place.root = LocalId(2);
        place.binding = None;
        child.places.push(place.clone());
        place.binding = Some(name);
        child.places.push(place);
        child.rvalues.push(Rvalue::Use(copy(1)));
        child.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(2),
                value: RvalueId(1),
            },
            0,
        ));
        child.blocks[0].instructions.push(inst(
            InstructionKind::BindOrigin {
                source: PlaceId(2),
                place: PlaceId(3),
                kind: OriginBindingKind::Fresh,
            },
            0,
        ));
        let cleanup = child.add_cleanup(vec![drop(2), drop(1)]);
        child.blocks[0].terminator = Terminator::Return {
            value: take(0),
            cleanup,
        };
    } else {
        // A second view of the SAME captured storage is not a second carrier.
        child.places.push(child.places[1].clone());
    }
    f
}

#[test]
fn fresh_capture_expression_name_requires_a_single_storage_carrier() {
    captured_name(false).valid();
    let f = captured_name(true);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidOrigin(PlaceId(3))),
        "{:?}",
        f.kinds()
    );
}

fn loop_owner_introduction(kind: OriginBindingKind) -> Fixture {
    let mut f = Fixture::unit();
    let owner = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters.push(LocalId(condition));
    let handle = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    let name = Origin {
        root: OriginRoot::Expression(lexical(), hirsel_syntax::ExprId(71)),
        fields: vec![].into(),
    };
    row(&mut f, handle, |t| t.brand.alternatives.push(name.clone()));
    let named = PlaceId(f.body().places.len() as u32);
    let mut access = f.body().places[owner as usize].clone();
    access.binding = Some(Origin {
        root: OriginRoot::Expression(lexical(), hirsel_syntax::ExprId(71)),
        fields: vec![].into(),
    });
    f.body().places.push(access);
    let observe = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    let handle_ty = f.body().places[handle as usize].ty;
    let observe_root = f.body().places[observe as usize].root;
    f.body().locals[observe_root.0 as usize].ty = handle_ty;
    f.body().places[observe as usize].ty = handle_ty;
    f.body()
        .rvalues
        .push(Rvalue::Constant(Constant::String("iteration".into())));
    f.body().rvalues.push(Rvalue::Use(copy(handle)));
    let mut signature = function_signature(
        "brand",
        vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::String))],
        TypeKind::Primitive(Primitive::U8),
    );
    let SignatureKind::Function(fun) = &signature.kind else {
        unreachable!()
    };
    let result = fun.result;
    let mut result_row = interner().get(result).unwrap().clone();
    result_row.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    let result = interner().intern(result_row);
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    fun.result = result;
    f.signatures.insert(id("brand"), signature);
    f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
    let failure = f.cleanup(vec![drop(owner)]);
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(owner),
                    value: RvalueId(1),
                },
                0,
            ),
            inst(
                InstructionKind::BindOrigin {
                    source: PlaceId(owner),
                    place: named,
                    kind,
                },
                failure,
            ),
        ],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("brand"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: copy(named.0),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(handle),
                next: BlockId(2),
            }),
            on_failure: CleanupId(failure),
        },
    ));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(observe),
                    value: RvalueId(2),
                },
                failure,
            ),
            inst(
                InstructionKind::StorageDead {
                    place: PlaceId(observe),
                    if_live: false,
                },
                failure,
            ),
            inst(
                InstructionKind::StorageDead {
                    place: PlaceId(handle),
                    if_live: false,
                },
                failure,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(owner),
                    if_live: false,
                },
                0,
            ),
        ],
        Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(1),
            no: BlockId(3),
        },
    ));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

#[test]
fn a_loop_owner_introduction_cannot_be_downgraded_to_an_identity_alias() {
    loop_owner_introduction(OriginBindingKind::Fresh).valid();
    let f = loop_owner_introduction(OriginBindingKind::Alias);
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "call result"
        }),
        "{:?}",
        f.kinds()
    );
}

fn parameter_alias(kind: OriginBindingKind) -> Fixture {
    let mut f = Fixture::unit();
    let owner = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    f.body().parameters.push(LocalId(owner));
    let scope = f.body().parameter_scope;
    f.body().places[owner as usize].binding = Some(Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    });
    let mut alias = f.body().places[owner as usize].clone();
    alias.binding = Some(Origin {
        root: OriginRoot::Expression(lexical(), hirsel_syntax::ExprId(71)),
        fields: vec![].into(),
    });
    let alias_id = PlaceId(f.body().places.len() as u32);
    f.body().places.push(alias);
    f.emit(InstructionKind::BindOrigin {
        source: PlaceId(owner),
        place: alias_id,
        kind,
    });
    let handle = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    row(&mut f, handle, |t| {
        t.brand.alternatives.push(Origin {
            root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
            fields: vec![].into(),
        })
    });
    let mut signature = function_signature(
        "brand",
        vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::String))],
        TypeKind::Primitive(Primitive::U8),
    );
    let SignatureKind::Function(fun) = &signature.kind else {
        unreachable!()
    };
    let mut result = interner().get(fun.result).unwrap().clone();
    result.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    let result = interner().intern(result);
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    fun.result = result;
    f.signatures.insert(id("brand"), signature);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("brand"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: copy(alias_id.0),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(handle),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

#[test]
fn a_value_alias_cannot_be_upgraded_to_a_fresh_owner_identity() {
    parameter_alias(OriginBindingKind::Alias).valid();
    let f = parameter_alias(OriginBindingKind::Fresh);
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "call result"
        }),
        "{:?}",
        f.kinds()
    );
}

fn unplaced_constructor(escape: bool) -> Fixture {
    let mut f = Fixture::unit();
    let source = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    f.body().parameters.push(LocalId(source));
    let scoped = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    row(&mut f, scoped, |t| {
        t.modes.relation = t
            .modes
            .relation
            .union(&Relation::new([Reach::Region(RegionId(0))]))
    });
    f.assign(
        scoped,
        Rvalue::Constant(Constant::String("scoped".into())),
        0,
    );
    let element = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let destination = f.slot(TypeKind::Tuple(vec![element].into()), Storage::Temporary);
    row(&mut f, destination, |t| {
        t.modes.relation = Relation::environment()
    });
    let failure = f.cleanup(vec![drop(scoped)]);
    f.assign(
        destination,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(scoped),
            }],
            destination: Destination::Inferred,
        },
        failure,
    );
    let cleanup = f.cleanup(vec![drop(destination)]);
    f.body().blocks[0].terminator = if escape {
        ret(destination, 0)
    } else {
        ret(0, cleanup)
    };
    if escape {
        let mut result = interner()
            .get(f.definition.bodies[0].places[destination as usize].ty)
            .unwrap()
            .clone();
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        let result = f.definition.types.push(result);
        f.body().result = result;
    }
    owner_signature(&mut f, PassingMode::Borrow);
    if escape {
        let scope = f.body().parameter_scope;
        let result = f.body().result;
        f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
    }
    f
}

#[test]
fn a_scoped_constructor_part_keeps_its_unplaced_holders_frame_bound() {
    unplaced_constructor(false).valid();
    let f = unplaced_constructor(true);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(3))),
        "{:?}",
        f.kinds()
    );
    let mut bare = unplaced_constructor(true);
    row(&mut bare, 3, |t| t.modes.relation = Relation::environment());
    let result = bare.body().places[3].ty;
    bare.body().result = result;
    owner_signature(&mut bare, PassingMode::Borrow);
    assert!(
        bare.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(3))),
        "{:?}",
        bare.kinds()
    );
}

// A call receipt owns its root without promising that every child is unique.
// Its declared unique child can move; copying the receipt invalidates that proof.
// A unique child under an unqualified wrapper may be shared with a copy of that
// wrapper made before the receipt, so it cannot move.
fn fresh_call_payload(shared: bool, parameter: bool, nested: bool) -> Fixture {
    let mut f = Fixture::unit();
    let mut unique = ty(TypeKind::Primitive(Primitive::Bytes));
    unique.modes.uniqueness = Uniqueness::Unique;
    unique.modes.sendability = Sendability::NotSendable;
    let payload_ty = f.definition.types.push(unique.clone());
    let child_ty = if nested {
        f.definition
            .types
            .push(ty(TypeKind::Tuple(vec![payload_ty].into())))
    } else {
        payload_ty
    };
    let root = f.slot(
        TypeKind::Tuple(vec![child_ty].into()),
        if parameter {
            Storage::Parameter
        } else {
            Storage::Temporary
        },
    );
    let root_ty = f.body().places[root as usize].ty;
    unique.modes.sendability = Sendability::Sendable;
    let destination = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
    row(&mut f, destination, |t| *t = unique.clone());
    let alias = shared.then(|| f.slot(TypeKind::Tuple(vec![child_ty].into()), Storage::Local));
    let payload = f.body().places.len() as u32;
    let projection = if nested {
        vec![Projection::Field(0), Projection::Field(0)]
    } else {
        vec![Projection::Field(0)]
    };
    f.body().places.push(Place {
        root: LocalId(root),
        projection: projection.into(),
        ty: payload_ty,
        loan: None,
        binding: None,
    });
    let mut instructions = vec![];
    if let Some(alias) = alias {
        let rv = RvalueId(f.body().rvalues.len() as u32);
        f.body().rvalues.push(Rvalue::Use(copy(root)));
        instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(alias),
                value: rv,
            },
            0,
        ));
    }
    let cleanup_root = f.cleanup(vec![drop(root)]);
    let rv = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(take(payload)));
    instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(destination),
            value: rv,
        },
        cleanup_root,
    ));
    let mut actions = vec![drop(destination)];
    if let Some(alias) = alias {
        actions.push(drop(alias));
    }
    actions.push(drop(root));
    let cleanup = f.cleanup(actions);
    if parameter {
        f.body().parameters.push(LocalId(root));
        f.body().blocks[0].instructions.extend(instructions);
        f.body().blocks[0].terminator = ret(0, cleanup);
        owner_signature(&mut f, PassingMode::Take);
    } else {
        let mut signature = function_signature("receipt", vec![], TypeKind::Tuple(vec![].into()));
        if let SignatureKind::Function(fun) = &mut signature.kind {
            fun.result = root_ty;
        }
        f.signatures.insert(id("receipt"), signature);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("receipt"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(root),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(instructions, ret(0, cleanup)));
    }
    f
}

#[test]
fn declared_unique_payload_requires_an_unshared_owned_call_receipt() {
    fresh_call_payload(false, false, false).valid();
    for f in [
        fresh_call_payload(true, false, false),
        fresh_call_payload(false, true, false),
        fresh_call_payload(false, false, true),
    ] {
        assert!(
            f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "value modes"
            }),
            "{:?}",
            f.kinds()
        );
    }
}

// The enclosing tuple is not Sendable; an underclaimed field row must not hide
// an independently Sendable handle. Local handles and Receiver remain controls.
fn structural_handle_payload(receiver: bool, local: bool) -> Fixture {
    let mut f = Fixture::unit();
    let item = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let kind = if receiver {
        BuiltinType::Receiver
    } else {
        BuiltinType::Sender
    };
    let mut payload = ty(TypeKind::Builtin {
        kind,
        arguments: vec![GenericArgument::Type(item)].into(),
    });
    payload.modes.sendability = Sendability::NotSendable;
    if local {
        payload.modes.relation = Relation::environment();
    }
    let payload_ty = f.definition.types.push(payload.clone());
    let other = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Receiver,
        arguments: vec![GenericArgument::Type(item)].into(),
    }));
    let root = f.slot(
        TypeKind::Tuple(vec![payload_ty, other].into()),
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(root));
    payload.modes.relation = Relation::global();
    payload.modes.sendability = Sendability::Sendable;
    let destination = f.slot(payload.kind.clone(), Storage::Temporary);
    row(&mut f, destination, |t| *t = payload);
    let part = f.body().places.len() as u32;
    f.body().places.push(Place {
        root: LocalId(root),
        projection: vec![Projection::Field(0)].into(),
        ty: payload_ty,
        loan: None,
        binding: None,
    });
    let value = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(operand(
        part,
        if local {
            UseKind::Borrow
        } else {
            UseKind::Take
        },
    )));
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(destination),
            value,
        },
        0,
    ));
    let cleanup = f.cleanup(vec![drop(destination), drop(root)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(
        &mut f,
        if local {
            PassingMode::Borrow
        } else {
            PassingMode::Take
        },
    );
    f
}

#[test]
fn projected_handle_sendability_is_structural_and_requires_global_content() {
    structural_handle_payload(false, false).valid();
    // A Receiver payload is not sendable; local content cannot become global.
    for (f, entity) in [
        (structural_handle_payload(true, false), "sendable receipt"),
        (structural_handle_payload(false, true), "assignment"),
    ] {
        assert_eq!(f.kinds(), vec![ViolationKind::TypeMismatch { entity }]);
    }
}

#[test]
fn copied_run_owned_handles_bound_a_unique_constructions_graph() {
    for kind in [BuiltinType::Mutex, BuiltinType::Atomic, BuiltinType::List] {
        let mut f = Fixture::unit();
        let payload = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U64)));
        let input = f.slot(
            TypeKind::Builtin {
                kind,
                arguments: vec![GenericArgument::Type(payload)].into(),
            },
            Storage::Parameter,
        );
        f.body().parameters = vec![LocalId(input)];
        let input_type = f.body().places[input as usize].ty;
        let output = f.slot(TypeKind::Tuple(vec![input_type].into()), Storage::Local);
        let old = f.body().places[output as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let unique = f.definition.types.push(row);
        f.body().places[output as usize].ty = unique;
        f.body().locals[output as usize].ty = unique;
        f.assign(
            output,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![ConstructField {
                    field: 0,
                    value: copy(input),
                }],
                destination: Destination::Inferred,
            },
            0,
        );
        let cleanup = f.cleanup(vec![drop(output)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        owner_signature(&mut f, PassingMode::Borrow);
        if kind == BuiltinType::List {
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "unique construction"
            }));
        } else {
            f.valid();
        }
    }
}

// Forged producer TIR: no source checker or lowering participates. A counted
// wrapper copy must not export the nested unique claim, on either alias.
fn nested_unique_export(unique: bool, retained: bool, deep: bool) -> Fixture {
    let mut f = Fixture::unit();
    let mut payload = ty(TypeKind::Primitive(Primitive::Bytes));
    if unique {
        payload.modes.uniqueness = Uniqueness::Unique;
    }
    let payload = f.definition.types.push(payload);
    let kind = TypeKind::Tuple(vec![payload].into());
    let source = param(&mut f, kind.clone());
    let alias = f.slot(kind, Storage::Temporary);
    if retained && !deep {
        let shared = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::Bytes)));
        row(&mut f, alias, |t| {
            t.kind = TypeKind::Tuple(vec![shared].into())
        });
    }
    if deep {
        row(&mut f, alias, |t| t.modes.uniqueness = Uniqueness::Unique);
    }
    let failure = f.cleanup(vec![drop(source)]);
    f.assign(
        alias,
        if deep {
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value: copy(source),
            }
        } else {
            Rvalue::Use(copy(source))
        },
        failure,
    );
    let returned = if retained { source } else { alias };
    let other = if retained { alias } else { source };
    let cleanup = f.cleanup(vec![drop(other)]);
    f.body().result = f.body().places[returned as usize].ty;
    f.body().blocks[0].terminator = ret(returned, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    f
}

#[test]
fn nested_unique_exports_require_unshared_content_without_checker_help() {
    for retained in [false, true] {
        nested_unique_export(false, retained, false).valid();
        let f = nested_unique_export(true, retained, false);
        assert!(
            f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "value modes"
            }),
            "{:?}",
            f.kinds()
        );
        nested_unique_export(true, retained, true).valid();
    }
}

#[test]
fn nested_unique_aggregate_exports_require_unshared_content() {
    for (unique, first_unique) in [(false, false), (true, false), (true, true)] {
        let mut f = Fixture::unit();
        let mut payload = ty(TypeKind::Primitive(Primitive::Bytes));
        if unique {
            payload.modes.uniqueness = Uniqueness::Unique;
        }
        let payload = f.definition.types.push(payload);
        let kind = TypeKind::Tuple(vec![payload].into());
        let source = param(&mut f, kind);
        let wrapper = f.body().places[source as usize].ty;
        let first = if first_unique {
            wrapper
        } else {
            let payload = f
                .definition
                .types
                .push(ty(TypeKind::Primitive(Primitive::Bytes)));
            f.definition
                .types
                .push(ty(TypeKind::Tuple(vec![payload].into())))
        };
        let pair = f.slot(
            TypeKind::Tuple(vec![first, wrapper].into()),
            Storage::Temporary,
        );
        let cleanup = f.cleanup(vec![drop(source)]);
        f.assign(
            pair,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: copy(source),
                    },
                    ConstructField {
                        field: 1,
                        value: take(source),
                    },
                ],
                destination: Destination::Inferred,
            },
            cleanup,
        );
        f.body().result = f.body().places[pair as usize].ty;
        f.body().blocks[0].terminator = ret(pair, 0);
        owner_signature(&mut f, PassingMode::Take);
        if unique {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    // A receiving field cannot silently change a nested type
                    // argument; exact copied claims also require flow evidence.
                    entity: if first_unique {
                        "value modes"
                    } else {
                        "constructor field"
                    }
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn shared_nested_unique_storage_can_be_borrowed_without_exporting_its_claims() {
    let mut f = nested_unique_export(true, true, false);
    let source = 1;
    let alias = 2;
    let kind = interner()
        .get(f.body().places[source].ty)
        .unwrap()
        .kind
        .clone();
    let view = f.slot(kind, Storage::Temporary);
    f.body().scopes.push(Scope {
        parent: Some(ScopeId(0)),
        span: Span::default(),
    });
    f.body().locals[view as usize].scope = ScopeId(1);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(source as u32),
        mutable: false,
        if_inactive: false,
    });
    let borrowed = f.through_loan(source as u32, 0);
    f.assign(view, Rvalue::Use(operand(borrowed, UseKind::Borrow)), 0);
    f.emit(InstructionKind::EndScope(ScopeId(1)));
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    let cleanup = f.cleanup(vec![drop(alias), drop(source as u32)]);
    f.body().result = f.body().places[0].ty;
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    f.valid();
}

#[test]
fn nested_unique_local_transports_do_not_restore_exportable_receipts() {
    for exported in [false, true] {
        let mut f = Fixture::unit();
        let mut bytes = ty(TypeKind::Primitive(Primitive::Bytes));
        bytes.modes.uniqueness = Uniqueness::Unique;
        let bytes = f.definition.types.push(bytes);
        let kind = TypeKind::Tuple(vec![bytes].into());
        let source = param(&mut f, kind.clone());
        let view = f.slot(kind.clone(), Storage::Temporary);
        let moved = f.slot(kind, Storage::Temporary);
        for slot in [view, moved] {
            row(&mut f, slot, |t| t.modes.relation = Relation::environment());
        }
        f.assign(view, Rvalue::Use(copy(source)), 0);
        f.assign(moved, Rvalue::Use(take(view)), 0);
        let cleanup = f.cleanup(vec![drop(moved)]);
        if exported {
            // Keep the view's exact physical row so lifetime typing cannot
            // pre-empt the current-content uniqueness check at the export.
            f.body().result = f.body().places[moved as usize].ty;
            f.body().blocks[0].terminator = ret(moved, 0);
        } else {
            f.body().blocks[0].terminator = ret(0, cleanup);
        }
        owner_signature(&mut f, PassingMode::Borrow);
        if exported {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "value modes"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}
