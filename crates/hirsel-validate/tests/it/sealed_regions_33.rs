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
fn sealed_regions_33_scope_entry_control() {
    scope().valid();
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
fn sealed_regions_33_taken_helper_control() {
    taken_seal(TypeKind::Primitive(Primitive::U64)).0.valid();
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
