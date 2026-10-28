//! Hand-built wave-one collection, text and authority controls; no source lowering.
use super::*;

fn callback(selected: bool) -> Fixture {
    let mut f = Fixture::unit();
    let callable = FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: f.body().result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::Unknown,
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let mut parameters = Vec::new();
    for _ in 0..2 {
        let p = f.slot(TypeKind::Function(callable.clone()), Storage::Parameter);
        let old = f.body().places[p as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
        row.modes.relation = Relation::environment();
        let local = f.definition.types.push(row);
        f.body().locals[p as usize].ty = local;
        f.body().places[p as usize].ty = local;
        parameters.push(LocalId(p));
    }
    f.body().parameters = parameters.clone();
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: operand(parameters[usize::from(!selected)].0, UseKind::Borrow),
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    owner_signature(&mut f, PassingMode::Borrow);
    let SignatureKind::Function(header) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    header.uses = Uses::Only(AuthoritySummary::Known(
        vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
    ));
    header.authority = AuthoritySummary::Known(
        vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
    );
    f
}
#[test]
fn callback_invocation_requires_the_exact_selected_parameter() {
    callback(true).valid();
    assert!(
        callback(false)
            .kinds()
            .contains(&ViolationKind::TypeMismatch {
                entity: "body uses"
            })
    );
}

fn nested_once_callback(selected: Option<bool>) -> Fixture {
    let mut f = Fixture::unit();
    let parent = f.body().clone();
    let unit = f.body().result;
    f.body().blocks[0].instructions.clear();
    f.body().rvalues.clear();
    let callback = FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: unit,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::Unknown,
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let input = f.slot(TypeKind::Function(callback.clone()), Storage::Parameter);
    let old = f.body().places[input as usize].ty;
    let mut row = interner().get(old).unwrap().clone();
    row.modes.multiplicity = Multiplicity::Once;
    let once = f.definition.types.push(row);
    f.body().locals[input as usize].ty = once;
    f.body().places[input as usize].ty = once;
    let other = selected.map(|_| {
        let other = f.slot(TypeKind::Function(callback.clone()), Storage::Parameter);
        f.body().locals[other as usize].ty = once;
        f.body().places[other as usize].ty = once;
        other
    });
    f.body().parameters = std::iter::once(input).chain(other).map(LocalId).collect();
    f.body().parameter_scope = scoped(NameSite::Statement(hirsel_syntax::StmtId(1)));
    let staged = f.slot(TypeKind::Function(callback.clone()), Storage::Temporary);
    f.body().locals[staged as usize].ty = once;
    f.body().places[staged as usize].ty = once;
    let cleanup = f.cleanup(
        std::iter::once(drop(staged))
            .chain(other.map(drop))
            .chain(std::iter::once(drop(input)))
            .collect(),
    );
    let supplied = if selected == Some(false) {
        other.unwrap()
    } else {
        input
    };
    f.assign(staged, Rvalue::Use(take(supplied)), cleanup);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: take(staged),
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(0),
            next: BlockId(1),
        }),
        on_failure: CleanupId(cleanup),
    };
    f.body().blocks.push(block(vec![], ret(0, cleanup)));
    let child = std::mem::replace(f.body(), parent);
    f.definition.bodies.push(child);
    let mut header = callback;
    header.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Take,
        ty: once,
    }]
    .into();
    if other.is_some() {
        let parameter = header.parameters[0].clone();
        header.parameters.push(parameter);
    }
    header.authority = AuthoritySummary::inert();
    header.uses = Uses::Only(if selected.is_some() {
        AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
        )
    } else {
        AuthoritySummary::inert()
    });
    let closure = f.slot(TypeKind::Function(header), Storage::Local);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
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
fn a_nested_once_callback_keeps_parameter_authority_through_staging() {
    nested_once_callback(None).valid();
}

#[test]
fn nested_callback_staging_preserves_the_exact_header_selector() {
    nested_once_callback(Some(true)).valid();
    assert!(
        nested_once_callback(Some(false))
            .kinds()
            .contains(&ViolationKind::TypeMismatch {
                entity: "body uses"
            })
    );
}

fn byte_index(write: bool) -> Fixture {
    let mut f = Fixture::unit();
    let input = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let old = f.body().places[input as usize].ty;
    let mut row = interner().get(old).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let unique = f.definition.types.push(row);
    f.body().places[input as usize].ty = unique;
    f.body().locals[input as usize].ty = unique;
    f.body().parameters = vec![LocalId(input)];
    let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
    let byte = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    f.assign(byte, Rvalue::Constant(Constant::Integer(7)), 0);
    let projected = f.body().places.len() as u32;
    let mut byte_row = Type::boolean();
    byte_row.kind = TypeKind::Primitive(Primitive::U8);
    let byte_type = f.definition.types.push(byte_row);
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(input),
        projection: vec![Projection::Index(LocalId(index))].into(),
        ty: byte_type,
    });
    if write {
        let displaced = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
        f.emit(InstructionKind::Replace {
            destination: PlaceId(projected),
            value: take(byte),
            displaced: PlaceId(displaced),
            if_live: false,
        });
        f.emit(InstructionKind::StorageDead {
            place: PlaceId(displaced),
            if_live: false,
        });
    } else {
        let read = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
        f.assign(read, Rvalue::Use(copy(projected)), 0);
    }
    let cleanup = f.cleanup(vec![drop(input)]);
    // Every failure and normal exit releases the incoming owner.
    for instruction in &mut f.body().blocks[0].instructions {
        instruction.on_failure = CleanupId(cleanup);
    }
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    f
}
#[test]
fn byte_index_is_readonly_even_under_owned_unique_storage() {
    byte_index(false).valid();
    assert!(
        byte_index(true)
            .kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::InvalidUse(_)))
    );
}

fn inactive_payload(present: bool) -> Fixture {
    let mut f = Fixture::unit();
    let owner = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
    f.body().parameters = vec![LocalId(owner)];
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let mut branded = interner().get(scalar).unwrap().clone();
    branded.brand = Brand {
        alternatives: vec![Origin {
            root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
            fields: vec![].into(),
        }]
        .into(),
    };
    let branded = f.definition.types.push(branded);
    let input = f.slot(
        TypeKind::Named {
            definition: id("Choice"),
            arguments: vec![GenericArgument::Type(scalar)].into(),
        },
        Storage::Temporary,
    );
    let mut fields = Vec::new();
    if present {
        let payload = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.assign(payload, Rvalue::Constant(Constant::Integer(1)), 0);
        fields.push(ConstructField {
            field: 0,
            value: take(payload),
        });
    }
    f.assign(
        input,
        Rvalue::Construct {
            constructor: Constructor::Variant {
                definition: path("Choice"),
                variant: u32::from(present),
            },
            fields,
            destination: Destination::Inferred,
        },
        0,
    );
    let output = f.slot(
        TypeKind::Named {
            definition: id("Choice"),
            arguments: vec![GenericArgument::Type(branded)].into(),
        },
        Storage::Temporary,
    );
    f.assign(output, Rvalue::Use(take(input)), 0);
    let types = interner();
    let parameter = types.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let repr = types.push(ty(TypeKind::Variant(
        vec![
            Variant {
                name: "Absent".into(),
                fields: vec![].into(),
            },
            Variant {
                name: "Present".into(),
                fields: vec![(None, parameter)].into(),
            },
        ]
        .into(),
    )));
    f.signatures.insert(
        id("Choice"),
        Signature {
            definition: id("Choice"),
            metadata: Default::default(),
            kind: SignatureKind::Type(Box::new(TypeDefinition {
                loop_source: None,
                format: None,
                generics: vec![GenericParameter::Type {
                    name: "T".into(),
                    bounds: vec![].into(),
                }],
                representation: Some(repr),
                bounds: vec![],
                authority: AuthoritySummary::inert(),
                capability: None,
                implementation_authority: None,
                serializable: false,
                serializable_check: None,
                layout: Default::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
                captures: None,
                destructor_allocation: Allocation::NoAlloc,
            })),
            fingerprint: ContentHash::default(),
        },
    );
    owner_signature(&mut f, PassingMode::Borrow);
    f
}
#[test]
fn inactive_variant_arguments_add_no_payload_brand() {
    inactive_payload(false).valid();
    assert!(
        inactive_payload(true)
            .kinds()
            .contains(&ViolationKind::TypeMismatch {
                entity: "assignment"
            })
    );
}

fn inactive_expiry(present: bool) -> Fixture {
    let mut f = inactive_payload(false);
    if present {
        let choice = f.body().places[3].ty;
        let TypeKind::Named { arguments, .. } = &interner().get(choice).unwrap().kind else {
            unreachable!()
        };
        let GenericArgument::Type(branded) = arguments[0] else {
            unreachable!()
        };
        let payload = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
        f.body().locals[payload as usize].ty = branded;
        f.body().places[payload as usize].ty = branded;
        f.body().parameters.push(LocalId(payload));
        f.body().locals[2].ty = choice;
        f.body().places[2].ty = choice;
        f.body().rvalues[1] = Rvalue::Construct {
            constructor: Constructor::Variant {
                definition: path("Choice"),
                variant: 1,
            },
            fields: vec![ConstructField {
                field: 0,
                value: copy(payload),
            }],
            destination: Destination::Inferred,
        };
    }
    let scope = f.body().parameter_scope;
    f.body().places[1].binding = Some(Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    });
    f.emit(InstructionKind::EndOrigin(PlaceId(1)));
    let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(
        result,
        Rvalue::Binary {
            op: BinaryOp::Eq,
            left: operand(3, UseKind::Borrow),
            right: operand(3, UseKind::Borrow),
        },
        0,
    );
    owner_signature(&mut f, PassingMode::Borrow);
    if present {
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let SignatureKind::Function(header) = &mut signature.kind else {
            unreachable!()
        };
        let mut declared = interner().get(header.parameters[1].ty).unwrap().clone();
        declared.brand.alternatives[0].root =
            OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
        header.parameters[1].ty = interner().intern(declared);
    }
    f
}
#[test]
fn absent_payload_retains_no_identity_after_its_contract_owner_ends() {
    inactive_expiry(false).valid();
    let kinds = inactive_expiry(true).kinds();
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, ViolationKind::InvalidOrigin(_))),
        "{kinds:?}"
    );
}

fn captured_inactive_payload(present: bool) -> Fixture {
    let mut f = inactive_expiry(present);
    let choice = f.body().places[3].ty;
    let scope = scoped(NameSite::Statement(hirsel_syntax::StmtId(8)));
    let child = Body {
        arena_scope_grant: None,
        scoped_parameters: Vec::new(),
        parameter_scope: scope,
        locals: vec![
            Local {
                ty: choice,
                storage: Storage::Capture,
                scope: ScopeId(0),
                span: Span::default(),
            },
            Local {
                ty: choice,
                storage: Storage::Temporary,
                scope: ScopeId(0),
                span: Span::default(),
            },
        ],
        places: (0..2)
            .map(|root| Place {
                root: LocalId(root),
                projection: vec![].into(),
                ty: choice,
                loan: None,
                binding: None,
            })
            .collect(),
        scopes: vec![Scope {
            parent: None,
            span: Span::default(),
        }],
        blocks: vec![block(
            vec![inst(
                InstructionKind::Assign {
                    destination: PlaceId(1),
                    value: RvalueId(0),
                },
                0,
            )],
            ret(1, 0),
        )],
        rvalues: vec![Rvalue::Use(copy(0))],
        cleanups: vec![Cleanup::default()],
        cleanup_nodes: vec![],
        entry: BlockId(0),
        parameters: vec![],
        result: choice,
        borrow_cursors: vec![],
        tail_sites: vec![],
        matches: vec![],
    };
    f.definition.bodies.push(child);
    let function = FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: choice,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let closure = f.slot(TypeKind::Function(function), Storage::Local);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(3, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        0,
    );
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().locals[result as usize].ty = choice;
    f.body().places[result as usize].ty = choice;
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(closure),
        mutable: false,
        if_inactive: false,
    });
    let borrowed = f.body().places.len() as u32;
    let mut view = f.body().places[closure as usize].clone();
    view.loan = Some(LoanId(0));
    f.body().places.push(view);
    let failure_cleanup = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(closure),
    ]);
    let cleanup = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: operand(borrowed, UseKind::Borrow),
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure_cleanup),
    };
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            failure_cleanup,
        )],
        ret(0, cleanup),
    ));
    f
}

#[test]
fn absent_variant_identity_stays_absent_through_capture_and_return() {
    captured_inactive_payload(false).valid();
    assert!(
        captured_inactive_payload(true)
            .kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::InvalidOrigin(_)))
    );
}

#[test]
fn an_active_unbranded_call_result_cannot_acquire_a_payload_brand() {
    for forged in [false, true] {
        let mut f = captured_inactive_payload(false);
        let unbranded = f.body().places[2].ty;
        let TypeKind::Named { arguments, .. } = &interner().get(unbranded).unwrap().kind else {
            unreachable!()
        };
        let GenericArgument::Type(scalar) = arguments[0] else {
            unreachable!()
        };
        let child = &mut f.definition.bodies[1];
        child.locals[0].storage = Storage::Temporary;
        child.locals[0].ty = scalar;
        child.places[0].ty = scalar;
        child.locals[1].ty = unbranded;
        child.places[1].ty = unbranded;
        child.result = unbranded;
        child.rvalues = vec![
            Rvalue::Constant(Constant::Integer(7)),
            Rvalue::Construct {
                constructor: Constructor::Variant {
                    definition: path("Choice"),
                    variant: 1,
                },
                fields: vec![ConstructField {
                    field: 0,
                    value: copy(0),
                }],
                destination: Destination::Inferred,
            },
        ];
        child.blocks[0].instructions = vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(0),
                    value: RvalueId(0),
                },
                0,
            ),
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(1),
                    value: RvalueId(1),
                },
                0,
            ),
        ];
        let closure_value = f
            .body()
            .rvalues
            .iter()
            .position(|value| {
                matches!(
                    value,
                    Rvalue::Closure {
                        context: hirsel_types::CaptureContext::OwningScoped,
                        body: BodyId(1),
                        ..
                    }
                )
            })
            .unwrap();
        let Rvalue::Closure { captures, .. } = &mut f.body().rvalues[closure_value] else {
            unreachable!()
        };
        captures.clear();
        let closure = f.body().blocks[0]
            .instructions
            .iter()
            .find_map(|instruction| match instruction.kind {
                InstructionKind::Assign { destination, value }
                    if value.0 as usize == closure_value =>
                {
                    Some(destination)
                }
                _ => None,
            })
            .unwrap();
        let old = f.body().places[closure.0 as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
        let TypeKind::Function(function) = &mut row.kind else {
            unreachable!()
        };
        function.result = unbranded;
        let row = f.definition.types.push(row);
        let root = f.body().places[closure.0 as usize].root;
        f.body().locals[root.0 as usize].ty = row;
        for place in f
            .body()
            .places
            .iter_mut()
            .filter(|place| place.root == root)
        {
            place.ty = row;
        }
        if !forged {
            let Terminator::Call {
                returns: Some(CallReturn { destination, .. }),
                ..
            } = f.body().blocks[0].terminator
            else {
                unreachable!()
            };
            let root = f.body().places[destination.0 as usize].root;
            f.body().locals[root.0 as usize].ty = unbranded;
            f.body().places[destination.0 as usize].ty = unbranded;
            f.valid();
        } else {
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "call result"
            }));
        }
    }
}

fn scope_forwarding(selected: bool) -> Fixture {
    let mut f = callback(selected);
    let action = if let Terminator::Call {
        callee: Callee::Indirect { value, .. },
        ..
    } = &f.body().blocks[0].terminator
    {
        value.clone()
    } else {
        unreachable!()
    };
    let action_type = f.body().places[action.place.0 as usize].ty;
    let mut scope = f.signatures.get(id("main")).unwrap().clone();
    scope.definition = id("scoped");
    let SignatureKind::Function(header) = &mut scope.kind else {
        unreachable!()
    };
    header.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Borrow,
        ty: action_type,
    }]
    .into();
    header.authority = AuthoritySummary::inert();
    header.uses = Uses::Unrestricted;
    f.signatures.insert(scope.definition.clone(), scope);
    let Terminator::Call {
        callee, arguments, ..
    } = &mut f.body().blocks[0].terminator
    else {
        unreachable!()
    };
    *callee = Callee::Definition {
        path: path("scoped"),
        arguments: vec![],
    };
    *arguments = vec![CallArgument {
        parameter: ParamId(0),
        value: action,
    }];
    f
}

#[test]
fn a_scope_forwards_only_the_enclosing_declarations_selected_authority() {
    scope_forwarding(true).valid();
    assert!(
        scope_forwarding(false)
            .kinds()
            .contains(&ViolationKind::TypeMismatch {
                entity: "body uses"
            })
    );
}
