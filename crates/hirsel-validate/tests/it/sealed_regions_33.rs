//! Independent entry proofs: every control reaches public validate.
use crate::support::*;
use hirsel_syntax::{DefPath, PassingMode};
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn scope() -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let a = arena_slot(&mut f, Storage::Temporary);
    let arena = f.body().locals[a as usize].ty;
    if let SignatureKind::Type(declaration) = &mut f.signatures.get_mut(id("Arena")).unwrap().kind {
        declaration.bounds.push(Bound::NoallocDrop);
    }
    let mut nested = empty_body(&mut f);
    nested.arena_scope_grant = Some(ArenaScopeGrant {
        intrinsic: DefPath::core("arena.with"),
        callback_parameter: ParamId(0),
        arena_parameter: ParamId(0),
    });
    nested.locals.push(Local {
        ty: arena,
        storage: Storage::Parameter,
        scope: ScopeId(0),
        span: Default::default(),
    });
    nested.places.push(Place {
        root: LocalId(1),
        projection: vec![].into(),
        loan: None,
        binding: None,
        ty: arena,
    });
    nested.parameters.push(LocalId(1));
    edit_cleanup(&mut nested, 0, |actions| *actions = vec![drop(1)]);
    f.definition.bodies.push(nested);
    let callback = fn_type(
        vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: arena,
        }],
        unit,
    );
    let c = f.slot(TypeKind::Function(callback.clone()), Storage::Temporary);
    f.assign(
        c,
        Rvalue::Closure {
            context: CaptureContext::Scoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Heap,
        },
        0,
    );
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let cleanup = f.cleanup(vec![drop(c)]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: DefPath::core("arena.with"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: copy(c),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(cleanup),
    };
    f.body().blocks.push(block(vec![], ret(0, cleanup)));
    let mut signature = function_signature(
        "scope",
        vec![(PassingMode::Borrow, TypeKind::Function(callback))],
        TypeKind::Tuple(vec![].into()),
    );
    signature.definition = (&DefPath::core("arena.with")).into();
    f.signatures.insert(signature.definition, signature);
    f
}

fn rejected(f: &Fixture, entity: &'static str) {
    let kinds = f.kinds();
    assert!(
        kinds
            .iter()
            .any(|k| *k == ViolationKind::TypeMismatch { entity }),
        "{kinds:?}"
    );
}

#[test]
fn sealed_regions_33_forged_grant_on_nonintrinsic_entry() {
    let mut f = scope();
    f.valid();
    f.definition.bodies[0].arena_scope_grant = f.definition.bodies[1].arena_scope_grant.clone();
    rejected(&f, "arena scope grant entry");
}

#[test]
fn sealed_regions_33_same_spelling_is_not_intrinsic() {
    let mut f = scope();
    f.valid();
    f.definition.bodies[1]
        .arena_scope_grant
        .as_mut()
        .unwrap()
        .intrinsic = path("arena.with");
    rejected(&f, "arena scope grant entry");
}

#[test]
fn sealed_regions_33_forged_callback_parameter() {
    let mut f = scope();
    f.valid();
    f.definition.bodies[1]
        .arena_scope_grant
        .as_mut()
        .unwrap()
        .callback_parameter = ParamId(1);
    rejected(&f, "arena scope grant entry");
}

#[test]
fn sealed_regions_33_forged_arena_parameter() {
    let mut f = scope();
    f.valid();
    f.definition.bodies[1]
        .arena_scope_grant
        .as_mut()
        .unwrap()
        .arena_parameter = ParamId(1);
    rejected(&f, "arena scope grant entry");
}

#[test]
fn sealed_regions_33_grant_closure_cannot_also_escape() {
    let mut f = scope();
    f.valid();
    let c = 2;
    f.body().blocks[1].terminator = Terminator::Return {
        value: copy(c),
        cleanup: CleanupId(0),
    };
    rejected(&f, "arena scope grant entry");
}

#[test]
fn sealed_regions_33_ordinary_borrow_cannot_destroy_grant() {
    let mut f = scope();
    f.valid();
    f.definition.bodies[1].arena_scope_grant = None;
    assert!(
        f.kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::BorrowedMove(_)))
    );
}

#[test]
fn sealed_regions_33_grant_cleanup_is_required() {
    let mut f = scope();
    f.valid();
    edit_cleanup(&mut f.definition.bodies[1], 0, |actions| actions.clear());
    assert!(
        f.kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::CleanupMismatch { .. }))
    );
}

#[test]
fn sealed_regions_33_double_consumption() {
    let mut f = scope();
    f.valid();
    f.definition.bodies[1].blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(1),
            if_live: false,
        },
        0,
    ));
    f.definition.bodies[1].blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(1),
            if_live: false,
        },
        0,
    ));
    assert!(f.kinds().iter().any(|kind| matches!(
        kind,
        ViolationKind::UseAfterMove(_) | ViolationKind::ReadBeforeInit(_)
    )));
}

#[test]
fn sealed_regions_33_missing_grant_at_indirect_entry() {
    let mut f = scope();
    f.valid();
    let returned = f.body().blocks[1].terminator.clone();
    f.body().blocks[1].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: copy(2),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: copy(1),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(0),
            next: BlockId(2),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], returned));
    rejected(&f, "arena scope grant entry");
}

#[test]
fn sealed_regions_33_erased_grant_closure() {
    let mut f = scope();
    f.valid();
    let callback_ty = f.definition.bodies[0].locals[2].ty;
    let wrapped = f.slot(
        TypeKind::Tuple(vec![callback_ty].into()),
        Storage::Temporary,
    );
    f.assign(
        wrapped,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: copy(2),
            }],
            destination: Destination::Heap,
        },
        0,
    );
    rejected(&f, "arena scope grant entry");
}

fn taken_seal(root_kind: TypeKind) -> (Fixture, u32, u32, u32) {
    let mut f = Fixture::unit();
    let arena = arena_slot(&mut f, Storage::Parameter);
    if let SignatureKind::Type(declaration) = &mut f.signatures.get_mut(id("Arena")).unwrap().kind {
        declaration.bounds.push(Bound::NoallocDrop);
    }
    let root = f.slot(root_kind.clone(), Storage::Parameter);
    let root_ty = f.body().places[root as usize].ty;
    f.body().parameters = vec![LocalId(arena), LocalId(root)];
    let result = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::Region,
            arguments: vec![GenericArgument::Type(root_ty)].into(),
        },
        Storage::Temporary,
    );
    f.body().result = f.body().places[result as usize].ty;
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: DefPath::core("arena.seal"),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(0),
                value: take(arena),
            },
            CallArgument {
                parameter: ParamId(1),
                value: take(root),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    owner_signature(&mut f, PassingMode::Take);
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    let arena_kind = interner()
        .row(f.body().places[arena as usize].ty)
        .kind
        .clone();
    let result_kind = interner().row(f.body().result).kind.clone();
    let mut signature = function_signature(
        "seal",
        vec![
            (PassingMode::Take, arena_kind),
            (PassingMode::Take, root_kind),
        ],
        result_kind,
    );
    signature.definition = (&DefPath::core("arena.seal")).into();
    if let SignatureKind::Function(header) = &mut signature.kind {
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    f.signatures.insert(signature.definition, signature);
    (f, arena, root, result)
}

#[test]
fn sealed_regions_33_heap_token_cannot_be_sealed() {
    let (mut f, a, _, _) = taken_seal(TypeKind::Primitive(Primitive::U64));
    f.valid();
    let old = f.body().places[a as usize].ty;
    let mut row = interner().row(old).clone();
    row.allocation_target = Some(AllocationTarget::Heap);
    let ty = f.definition.types.push(row);
    f.body().places[a as usize].ty = ty;
    f.body().locals[a as usize].ty = ty;
    assert!(
        f.kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::InvalidUse(_)))
    );
}

#[test]
fn sealed_regions_33_seal_cannot_hide_allocation() {
    let (mut f, _, _, _) = taken_seal(TypeKind::Primitive(Primitive::U64));
    f.valid();
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.allocation = Allocation::NoAlloc;
    }
    assert!(!f.kinds().is_empty());
}

#[test]
fn sealed_regions_33_hidden_resource_rejected_independently() {
    let (mut f, _, root, result) = taken_seal(TypeKind::Primitive(Primitive::U64));
    f.valid();
    // Rebuild the complete consistent type graph around a resource payload.
    let mut resource = function_signature("R", vec![], TypeKind::Tuple(vec![].into()));
    let mut declaration = match f.signatures.get(id("Arena")).unwrap().kind.clone() {
        SignatureKind::Type(t) => t,
        _ => unreachable!(),
    };
    declaration.resource = true;
    declaration.capability = None;
    declaration.representation = Some(unit_ty(&mut f));
    declaration.authority = AuthoritySummary::inert();
    resource.kind = SignatureKind::Type(declaration);
    f.signatures.insert(id("R"), resource);
    let hidden = f
        .definition
        .types
        .push(ty(TypeKind::Box(interner().intern(ty(TypeKind::Named {
            definition: id("R"),
            arguments: vec![].into(),
        })))));
    f.body().locals[root as usize].ty = hidden;
    f.body().places[root as usize].ty = hidden;
    let owner = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(hidden)].into(),
    }));
    f.body().locals[result as usize].ty = owner;
    f.body().places[result as usize].ty = owner;
    f.body().result = owner;
    owner_signature(&mut f, PassingMode::Take);
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    if let SignatureKind::Function(header) = &mut f
        .signatures
        .get_mut(DefPath::core("arena.seal"))
        .unwrap()
        .kind
    {
        header.parameters[1].ty = hidden;
        header.result = owner;
    }
    rejected(&f, "seal resource-free payload");
}

fn second_view() -> (Fixture, u32) {
    let (mut f, arena, root, _) = taken_seal(TypeKind::Primitive(Primitive::U64));
    let number = f.body().places[root as usize].ty;
    let alias = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(number)].into(),
        },
        Storage::Parameter,
    );
    let origin = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
        fields: vec![].into(),
    };
    let mut row = interner().row(f.body().places[alias as usize].ty).clone();
    row.modes.relation = Relation::place(origin);
    let local_ty = f.definition.types.push(row);
    f.body().locals[alias as usize].ty = local_ty;
    f.body().places[alias as usize].ty = local_ty;
    f.body().parameters.push(LocalId(alias));
    let view = f.slot(interner().row(local_ty).kind.clone(), Storage::Temporary);
    f.body().locals[view as usize].ty = local_ty;
    f.body().places[view as usize].ty = local_ty;
    f.assign(view, Rvalue::Use(copy(alias)), 0);
    let before_seal = f.cleanup(vec![drop(alias), drop(arena)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(view),
            if_live: false,
        },
        before_seal,
    ));
    let cleanup = f.cleanup(vec![drop(alias)]);
    if let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator {
        *on_failure = CleanupId(cleanup);
    }
    if let Terminator::Return { cleanup: id, .. } = &mut f.body().blocks[1].terminator {
        *id = CleanupId(cleanup);
    }
    owner_signature(&mut f, PassingMode::Take);
    let scope = f.body().parameter_scope;
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.parameters[2].ty =
            map_parameter_references(interner(), header.parameters[2].ty, |p, depth| {
                Ok(match p {
                    ParameterRef::Body {
                        scope: owner,
                        parameter,
                    } if owner == scope => ParameterRef::Bound { depth, parameter },
                    p => p,
                })
            })
            .unwrap();
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    (f, alias)
}

#[test]
fn sealed_regions_33_live_second_arena_view_rejected() {
    let (mut f, _) = second_view();
    f.valid();
    let drop_view = f.body().blocks[0].instructions.pop().unwrap();
    let copy_view = f.body().blocks[0].instructions.pop().unwrap();
    f.body().blocks[1]
        .instructions
        .extend([copy_view, drop_view]);
    let kinds = f.kinds();
    assert!(
        kinds.iter().any(|kind| *kind
            == ViolationKind::TypeMismatch {
                entity: "seal live arena view"
            }),
        "{kinds:?}"
    );
}

#[test]
fn sealed_regions_33_unknown_once_cannot_borrow_another_closures_proof() {
    let mut seed = Fixture::unit();
    let unit = unit_ty(&mut seed);
    let (mut f, arena, root, result) = taken_seal(TypeKind::Function(fn_type(vec![], unit)));
    let mut row = interner().row(f.body().places[root as usize].ty).clone();
    row.modes.multiplicity = Multiplicity::Once;
    let once = f.definition.types.push(row);
    f.body().locals[root as usize].ty = once;
    f.body().locals[root as usize].storage = Storage::Temporary;
    f.body().places[root as usize].ty = once;
    let unknown = f.slot(interner().row(once).kind.clone(), Storage::Parameter);
    f.body().locals[unknown as usize].ty = once;
    f.body().places[unknown as usize].ty = once;
    f.body().parameters = vec![LocalId(arena), LocalId(unknown)];
    let owner = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(once)].into(),
    }));
    f.body().locals[result as usize].ty = owner;
    f.body().places[result as usize].ty = owner;
    f.body().result = owner;
    let nested = empty_body(&mut f);
    f.definition.bodies.push(nested);
    let construction = f.cleanup(vec![drop(unknown), drop(arena)]);
    f.assign(
        root,
        Rvalue::Closure {
            context: CaptureContext::Scoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Heap,
        },
        construction,
    );
    let cleanup = f.cleanup(vec![drop(unknown)]);
    if let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator {
        *on_failure = CleanupId(cleanup);
    }
    f.body().blocks[1].terminator = ret(result, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    if let SignatureKind::Function(header) = &mut f
        .signatures
        .get_mut(DefPath::core("arena.seal"))
        .unwrap()
        .kind
    {
        header.parameters[1].ty = once;
        header.result = owner;
    }
    f.valid();
    if let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator {
        arguments[1].value = take(unknown);
    }
    // The known closure still exists with exactly the same type. Only the
    // sealed operand changes; cleanup follows the owner that remains behind.
    edit_cleanup(f.body(), cleanup, |actions| *actions = vec![drop(root)]);
    rejected(&f, "seal resource-free payload");
}

#[test]
fn sealed_regions_33_branch_local_consumption_does_not_merge_into_a_live_grant() {
    let mut f = scope();
    let boolean = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bool)));
    let flag = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters.push(LocalId(flag));
    owner_signature(&mut f, PassingMode::Borrow);
    if let Rvalue::Closure { captures, .. } = &mut f.body().rvalues[1] {
        captures.push(copy(flag));
    }
    let body = &mut f.definition.bodies[1];
    body.locals.push(Local {
        ty: boolean,
        storage: Storage::Capture,
        scope: ScopeId(0),
        span: Default::default(),
    });
    body.places.push(Place {
        root: LocalId(2),
        projection: vec![].into(),
        loan: None,
        binding: None,
        ty: boolean,
    });
    let empty = body.cleanups.len() as u32;
    body.cleanups.push(Cleanup::default());
    body.blocks[0].terminator = Terminator::Branch {
        condition: copy(2),
        yes: BlockId(1),
        no: BlockId(2),
    };
    body.blocks.push(block(
        vec![inst(
            InstructionKind::Drop {
                place: PlaceId(1),
                if_live: false,
            },
            empty,
        )],
        Terminator::Goto(BlockId(3)),
    ));
    body.blocks
        .push(block(vec![], Terminator::Goto(BlockId(3))));
    body.blocks.push(block(vec![], ret(0, 0)));
    f.valid();
    f.definition.bodies[1].blocks[3].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(1),
            if_live: false,
        },
        empty,
    ));
    assert!(f.kinds().iter().any(|kind| matches!(
        kind,
        ViolationKind::UseAfterMove(_) | ViolationKind::ReadBeforeInit(_)
    )));
}

#[test]
fn sealed_regions_33_region_cleanup_cannot_be_omitted() {
    let (mut f, _, _, owner) = taken_seal(TypeKind::Primitive(Primitive::U64));
    let unit = unit_ty(&mut f);
    let cleanup = f.cleanup(vec![drop(owner)]);
    f.body().result = unit;
    f.body().blocks[1].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    f.valid();
    edit_cleanup(f.body(), cleanup, |actions| actions.clear());
    assert!(
        f.kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::CleanupMismatch { .. }))
    );
}
/// A real region.with call with an exclusive owner and a scoped read callback.
/// The callback's accepted result is a fresh empty heap list.
fn region_read() -> (Fixture, u32, u32) {
    let mut f = Fixture::unit();
    let number = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let list_kind = TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(number)].into(),
    };
    let list = f.definition.types.push(ty(list_kind.clone()));
    let region_kind = TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(list)].into(),
    };
    let owner = f.slot(region_kind.clone(), Storage::Parameter);
    f.body().parameters.push(LocalId(owner));
    let mut child = empty_body(&mut f);
    let root = child.locals.len() as u32;
    child.locals.push(Local {
        ty: list,
        storage: Storage::Parameter,
        scope: ScopeId(0),
        span: Default::default(),
    });
    child.places.push(Place {
        root: LocalId(root),
        projection: vec![].into(),
        loan: None,
        binding: None,
        ty: list,
    });
    child.parameters.push(LocalId(root));
    let answer = child.locals.len() as u32;
    child.locals.push(Local {
        ty: list,
        storage: Storage::Temporary,
        scope: ScopeId(0),
        span: Default::default(),
    });
    child.places.push(Place {
        root: LocalId(answer),
        projection: vec![].into(),
        loan: None,
        binding: None,
        ty: list,
    });
    child.result = list;
    let construction = RvalueId(child.rvalues.len() as u32);
    child.rvalues.push(Rvalue::Construct {
        constructor: Constructor::Sequence,
        fields: vec![],
        destination: Destination::Heap,
    });
    let active = child.add_cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(1),
        if_active: false,
    }]);
    let after = child.add_cleanup(vec![drop(answer)]);
    child.blocks[0].instructions = vec![
        inst(
            InstructionKind::Assign {
                destination: PlaceId(0),
                value: RvalueId(0),
            },
            0,
        ),
        inst(
            InstructionKind::BeginLoan {
                id: LoanId(1),
                place: PlaceId(root),
                mutable: false,
                if_inactive: false,
            },
            0,
        ),
        inst(
            InstructionKind::Assign {
                destination: PlaceId(answer),
                value: construction,
            },
            active.0,
        ),
        inst(
            InstructionKind::EndLoan {
                loan: LoanId(1),
                if_active: false,
            },
            after.0,
        ),
    ];
    child.blocks[0].terminator = ret(answer, 0);
    f.definition.bodies.push(child);
    let mut callback = fn_type(
        vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: list,
        }],
        list,
    );
    callback.allocation = Allocation::MayAllocate;
    callback.uses = Uses::Unrestricted;
    let closure = f.slot(TypeKind::Function(callback.clone()), Storage::Temporary);
    f.assign(
        closure,
        Rvalue::Closure {
            context: CaptureContext::Scoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Heap,
        },
        0,
    );
    let keep_closure = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(owner),
            mutable: true,
            if_inactive: false,
        },
        keep_closure,
    ));
    let owner_view = f.through_loan(owner, 0);
    let failure = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(closure),
    ]);
    let returned = f.slot(list_kind.clone(), Storage::Temporary);
    f.body().result = list;
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: DefPath::core("region.with"),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(0),
                value: operand(owner_view, UseKind::Mut),
            },
            CallArgument {
                parameter: ParamId(1),
                value: copy(closure),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(returned),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure),
    };
    let success = f.cleanup(vec![drop(returned), drop(closure)]);
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            success,
        )],
        ret(returned, keep_closure),
    ));
    owner_signature(&mut f, PassingMode::Mut);
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    let mut signature = function_signature(
        "read",
        vec![
            (PassingMode::Mut, region_kind),
            (PassingMode::Borrow, TypeKind::Function(callback)),
        ],
        list_kind,
    );
    signature.definition = (&DefPath::core("region.with")).into();
    if let SignatureKind::Function(header) = &mut signature.kind {
        header.allocation = Allocation::NoAlloc;
        header.uses = Uses::Unrestricted;
    }
    f.signatures.insert(signature.definition, signature);
    (f, root, answer)
}

fn region_read_local() -> (Fixture, u32, u32) {
    let (mut f, root, answer) = region_read();
    // Registry borrowed parameters default to `in env`. An explicitly global
    // borrow may be copied out and is not a witness of a sealed-root escape.
    let mut row = interner()
        .row(f.definition.bodies[1].places[root as usize].ty)
        .clone();
    row.modes.relation = Relation::environment();
    let local_root = f.definition.types.push(row);
    f.definition.bodies[1].locals[root as usize].ty = local_root;
    f.definition.bodies[1].places[root as usize].ty = local_root;
    let closure = match &f.definition.bodies[0].blocks[0].terminator {
        Terminator::Call { arguments, .. } => arguments[1].value.place,
        _ => unreachable!(),
    };
    let mut row = interner()
        .row(f.definition.bodies[0].places[closure.0 as usize].ty)
        .clone();
    let TypeKind::Function(function) = &mut row.kind else {
        unreachable!()
    };
    function.parameters[0].ty = local_root;
    let callback = f.definition.types.push(row);
    let local = f.definition.bodies[0].places[closure.0 as usize].root;
    f.definition.bodies[0].locals[local.0 as usize].ty = callback;
    f.definition.bodies[0].places[closure.0 as usize].ty = callback;
    let SignatureKind::Function(function) = &mut f
        .signatures
        .get_mut(DefPath::core("region.with"))
        .unwrap()
        .kind
    else {
        unreachable!()
    };
    function.parameters[1].ty = callback;
    (f, root, answer)
}

#[test]
fn sealed_regions_33_read_callback_cannot_escape_its_borrowed_root() {
    let (mut f, root, answer) = region_read_local();
    f.valid();
    let cleanup = f.definition.bodies[1].add_cleanup(vec![drop(answer)]);
    f.definition.bodies[1].blocks[0].terminator = Terminator::Return {
        value: copy(root),
        cleanup,
    };
    let kinds = f.kinds();
    assert!(
        kinds == vec![ViolationKind::TypeMismatch { entity: "return" }],
        "{kinds:?}"
    );
}

#[test]
fn sealed_regions_33_read_callback_cannot_forge_a_mutable_root_loan() {
    let (mut f, _, _) = region_read();
    f.valid();
    let InstructionKind::BeginLoan { mutable, .. } =
        &mut f.definition.bodies[1].blocks[0].instructions[1].kind
    else {
        unreachable!()
    };
    *mutable = true;
    let kinds = f.kinds();
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            ViolationKind::InvalidUse(_) | ViolationKind::BorrowedMove(_)
        )),
        "{kinds:?}"
    );
}

#[test]
fn sealed_regions_33_read_callback_cannot_forge_root_sendability() {
    let (mut f, root, _) = region_read();
    f.valid();
    let mut row = interner()
        .row(f.definition.bodies[1].places[root as usize].ty)
        .clone();
    row.modes.sendability = Sendability::Sendable;
    row.sendability_requirement = SendabilityRequirement::Sendable;
    let forged = f.definition.types.push(row);
    f.definition.bodies[1].locals[root as usize].ty = forged;
    f.definition.bodies[1].places[root as usize].ty = forged;
    let kinds = f.kinds();
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, ViolationKind::TypeMismatch { .. })),
        "{kinds:?}"
    );
}

#[test]
fn sealed_regions_33_foreign_arena_bound_cannot_be_erased_by_a_global_temporary() {
    let scalar = interner().intern(ty(TypeKind::Primitive(Primitive::U64)));
    let list_kind = TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(scalar)].into(),
    };
    let (mut f, _, source, _) = taken_seal(list_kind.clone());
    let foreign = arena_slot(&mut f, Storage::Parameter);
    let foreign_local = f.body().places[foreign as usize].root;
    f.body().parameters.push(foreign_local);
    let staged = f.slot(list_kind, Storage::Temporary);
    f.assign(staged, Rvalue::Use(take(source)), 0);
    let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
        unreachable!()
    };
    arguments[1].value = take(staged);
    owner_sig(
        &mut f,
        &[PassingMode::Take, PassingMode::Take, PassingMode::Borrow],
    );
    if let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind {
        header.allocation = Allocation::MayAllocate;
        header.uses = Uses::Unrestricted;
    }
    f.valid();
    // The receiving temporary still claims heap/global, while the actual
    // source parameter is now proven to be stored in the other arena.
    let mut row = interner().row(f.body().places[source as usize].ty).clone();
    row.modes.relation =
        Relation::parameters([ParameterRef::body(f.body().parameter_scope, ParamId(2))]);
    let actual = f.definition.types.push(row);
    let local = f.body().places[source as usize].root;
    f.body().places[source as usize].ty = actual;
    f.body().locals[local.0 as usize].ty = actual;
    let declared = map_parameter_references(interner(), actual, |p, depth| {
        Ok(match p {
            ParameterRef::Body { parameter, .. } => ParameterRef::Bound { depth, parameter },
            p => p,
        })
    })
    .unwrap();
    let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind
    else {
        unreachable!()
    };
    header.parameters[1].ty = declared;
    let kinds = f.kinds();
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            ViolationKind::TypeMismatch {
                entity: "assignment"
            }
        )),
        "{kinds:?}"
    );
}

#[test]
fn sealed_regions_33_omitted_alias_loan_cannot_hide_future_arena_view_use() {
    let (mut f, alias) = second_view();
    let arena = f.body().parameters[0].0;
    let before = f.cleanup(vec![drop(alias), drop(arena)]);
    let active = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(alias),
        drop(arena),
    ]);
    let copy_at = f.definition.bodies[0].blocks[0].instructions.iter().position(|i| matches!(i.kind,
        InstructionKind::Assign { value, .. } if matches!(f.definition.bodies[0].rvalues[value.0 as usize], Rvalue::Use(_)))).unwrap();
    let InstructionKind::Assign {
        destination: view,
        value,
    } = f.body().blocks[0].instructions[copy_at].kind
    else {
        unreachable!()
    };
    let borrowed = f.through_loan(alias, 0);
    let Rvalue::Use(operand) = &mut f.body().rvalues[value.0 as usize] else {
        unreachable!()
    };
    operand.place = PlaceId(borrowed);
    f.body().blocks[0].instructions[copy_at].on_failure = CleanupId(active);
    let after = f.cleanup(vec![drop(view.0), drop(alias), drop(arena)]);
    f.body().blocks[0].instructions.insert(
        copy_at,
        inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(alias),
                mutable: false,
                if_inactive: false,
            },
            before,
        ),
    );
    f.body().blocks[0].instructions.insert(
        copy_at + 2,
        inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            after,
        ),
    );
    f.valid();
    // The producer claims the access loan ended and deletes the later use's
    // LoanId. V still derives that use and its arena bound from executable TIR.
    let dropped = f.body().blocks[0].instructions.pop().unwrap();
    let mut copied = f.body().blocks[0].instructions.remove(copy_at + 1);
    f.body().blocks[0].instructions[copy_at + 1].on_failure = CleanupId(before);
    let Rvalue::Use(operand) = &mut f.body().rvalues[value.0 as usize] else {
        unreachable!()
    };
    operand.place = PlaceId(alias);
    let Terminator::Return { value: owner, .. } = f.body().blocks[1].terminator.clone() else {
        unreachable!()
    };
    let post_seal = f.cleanup(vec![drop(owner.place.0), drop(alias)]);
    copied.on_failure = CleanupId(post_seal);
    let mut dropped = dropped;
    dropped.on_failure = CleanupId(post_seal);
    f.body().blocks[1].instructions.extend([copied, dropped]);
    rejected(&f, "seal live arena view");
}

#[test]
fn sealed_regions_33_read_callback_cannot_escape_a_capture_nested_in_a_record() {
    let (mut f, root, answer) = region_read_local();
    let list = f.definition.bodies[1].places[answer as usize].ty;
    let unit = unit_ty(&mut f);
    let inner_type = f
        .definition
        .types
        .push(ty(TypeKind::Function(fn_type(vec![], unit))));
    let mut inner = empty_body(&mut f);
    add_capture(&mut inner, list);
    f.definition.bodies.push(inner);
    let representation = f.definition.types.push(ty(TypeKind::Record(
        vec![Field {
            name: "callback".into(),
            ty: inner_type,
            growable: false,
        }]
        .into(),
    )));
    let mut envelope = function_signature("Envelope", vec![], TypeKind::Tuple(vec![].into()));
    envelope.kind = SignatureKind::Type(Box::new(TypeDefinition {
        loop_source: None,
        format: None,
        destructor_allocation: Allocation::NoAlloc,
        generics: vec![],
        representation: Some(representation),
        bounds: vec![],
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
    }));
    f.signatures.insert(id("Envelope"), envelope);
    let result_kind = TypeKind::Named {
        definition: id("Envelope"),
        arguments: vec![].into(),
    };
    let result_ty = f.definition.types.push(ty(result_kind));
    let child = &mut f.definition.bodies[1];
    let inner = add_capture(child, inner_type);
    child.locals[inner as usize].storage = Storage::Temporary;
    let result = add_capture(child, result_ty);
    child.locals[result as usize].storage = Storage::Temporary;
    let answer_cleanup = child.add_cleanup(vec![drop(answer)]);
    let inner_cleanup = child.add_cleanup(vec![drop(inner), drop(answer)]);
    let capture_rvalue = RvalueId(child.rvalues.len() as u32);
    child.rvalues.push(Rvalue::Closure {
        context: CaptureContext::Escaping,
        body: BodyId(2),
        captures: vec![copy(answer)],
        destination: Destination::Heap,
    });
    let construction = RvalueId(child.rvalues.len() as u32);
    child.rvalues.push(Rvalue::Construct {
        constructor: Constructor::Record(path("Envelope")),
        fields: vec![ConstructField {
            field: 0,
            value: take(inner),
        }],
        destination: Destination::Heap,
    });
    child.blocks[0].instructions.extend([
        inst(
            InstructionKind::Assign {
                destination: PlaceId(inner),
                value: capture_rvalue,
            },
            answer_cleanup.0,
        ),
        inst(
            InstructionKind::Assign {
                destination: PlaceId(result),
                value: construction,
            },
            inner_cleanup.0,
        ),
    ]);
    child.blocks[0].terminator = ret(result, answer_cleanup.0);
    child.result = result_ty;
    let (closure, returned) = match &f.body().blocks[0].terminator {
        Terminator::Call {
            arguments,
            returns: Some(returns),
            ..
        } => (arguments[1].value.place, returns.destination),
        _ => unreachable!(),
    };
    let mut callback_row = interner()
        .row(f.body().places[closure.0 as usize].ty)
        .clone();
    let TypeKind::Function(callback) = &mut callback_row.kind else {
        unreachable!()
    };
    callback.result = result_ty;
    let callback_ty = f.definition.types.push(callback_row);
    let local = f.body().places[closure.0 as usize].root;
    f.body().places[closure.0 as usize].ty = callback_ty;
    f.body().locals[local.0 as usize].ty = callback_ty;
    let local = f.body().places[returned.0 as usize].root;
    f.body().places[returned.0 as usize].ty = result_ty;
    f.body().locals[local.0 as usize].ty = result_ty;
    f.body().result = result_ty;
    owner_signature(&mut f, PassingMode::Mut);
    let SignatureKind::Function(header) = &mut f.signatures.get_mut(id("main")).unwrap().kind
    else {
        unreachable!()
    };
    header.allocation = Allocation::MayAllocate;
    header.uses = Uses::Unrestricted;
    let SignatureKind::Function(header) = &mut f
        .signatures
        .get_mut(DefPath::core("region.with"))
        .unwrap()
        .kind
    else {
        unreachable!()
    };
    header.parameters[1].ty = callback_ty;
    header.result = result_ty;
    f.valid();
    // Keep the receiving record and closure types unchanged. Only the actual
    // captured operand changes from an independent heap list to the read view.
    let Rvalue::Closure { captures, .. } =
        &mut f.definition.bodies[1].rvalues[capture_rvalue.0 as usize]
    else {
        unreachable!()
    };
    captures[0] = copy(root);
    let kinds = f.kinds();
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            ViolationKind::TypeMismatch {
                entity: "closure capture"
            }
        )),
        "{kinds:?}"
    );
}

#[test]
fn sealed_regions_33_once_scope_formal_still_rejects_an_escaping_grant_body() {
    let mut f = scope();
    let SignatureKind::Function(signature) =
        &f.signatures.get(DefPath::core("arena.with")).unwrap().kind
    else {
        unreachable!()
    };
    let mut callback = interner().row(signature.parameters[0].ty).clone();
    callback.modes.multiplicity = Multiplicity::Once;
    let callback = f.definition.types.push(callback);
    let SignatureKind::Function(signature) = &mut f
        .signatures
        .get_mut(DefPath::core("arena.with"))
        .unwrap()
        .kind
    else {
        unreachable!()
    };
    signature.parameters[0].ty = callback;
    f.valid();
    f.body().blocks[1].terminator = Terminator::Return {
        value: copy(2),
        cleanup: CleanupId(0),
    };
    rejected(&f, "arena scope grant entry");
}
