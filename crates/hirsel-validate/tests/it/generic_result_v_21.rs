//! Hand-built callback receipts: no source checker or source proof metadata.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn slot(f: &mut Fixture, ty: TypeId, storage: Storage) -> u32 {
    let p = f.slot(interner().get(ty).unwrap().kind.clone(), storage);
    let root = f.body().places[p as usize].root;
    f.body().locals[root.0 as usize].ty = ty;
    f.body().places[p as usize].ty = ty;
    f.definition.types.adopt(ty);
    p
}

fn callback_result(bound: Option<Bound>, shape: &str, unique: bool) -> Fixture {
    let mut f = Fixture::unit();
    let reference = TypeParameterRef::body(f.body().parameter_scope, TypeParamId(0));
    let mut result = ty(TypeKind::Parameter(reference));
    // Keep the signature's conditional cleanup spelling on the callback result.
    // The declaring binder independently proves this is NoAlloc when bounded.
    result.drop_allocation = DropAllocation::DependsOn(vec![reference].into());
    if unique {
        result.modes.uniqueness = Uniqueness::Unique;
    }
    let callback_result = interner().intern(result.clone());
    if bound.is_some() {
        result.drop_allocation = DropAllocation::NoAlloc;
    }
    let received = interner().intern(result);
    let callback = interner().intern(ty(TypeKind::Function(fn_type(vec![], callback_result))));
    let read = slot(&mut f, callback, Storage::Parameter);
    let read_root = f.body().places[read as usize].root;
    f.body().parameters.push(read_root);
    let value = slot(&mut f, received, Storage::Temporary);
    let (kind, constructor) = if shape == "tuple" {
        (TypeKind::Tuple(vec![received].into()), Constructor::Tuple)
    } else {
        let mut declaration = capability_signature(shape);
        let SignatureKind::Type(header) = &mut declaration.kind else {
            unreachable!()
        };
        header.capability = None;
        header.authority = AuthoritySummary::inert();
        header.bounds = vec![];
        header.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        }];
        let parameter = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        header.representation = Some(
            interner().intern(ty(TypeKind::Variant(
                vec![
                    Variant {
                        name: "Some".into(),
                        fields: vec![(None, parameter)].into(),
                    },
                    Variant {
                        name: "None".into(),
                        fields: vec![].into(),
                    },
                ]
                .into(),
            ))),
        );
        f.signatures.insert(id(shape), declaration);
        (
            TypeKind::Named {
                definition: id(shape),
                arguments: vec![GenericArgument::Type(received)].into(),
            },
            Constructor::Variant {
                definition: path(shape),
                variant: 0,
            },
        )
    };
    let aggregate = slot(&mut f, interner().intern(ty(kind)), Storage::Temporary);
    f.body().blocks[0].instructions.clear();
    f.body().rvalues.clear();
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Indirect {
            value: operand(read, UseKind::Borrow),
            arguments: vec![],
            forward_caller: false,
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(value),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    let rv = RvalueId(0);
    f.body().rvalues.push(Rvalue::Construct {
        constructor,
        fields: vec![ConstructField {
            field: 0,
            value: take(value),
        }],
        destination: Destination::Inferred,
    });
    let failure = f.cleanup(vec![drop(value)]);
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::Assign {
                destination: PlaceId(aggregate),
                value: rv,
            },
            failure,
        )],
        ret(aggregate, 0),
    ));
    let result = f.body().places[aggregate as usize].ty;
    f.body().result = result;
    owner_sig(&mut f, &[PassingMode::Borrow]);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(id("main")).unwrap().kind else {
        unreachable!()
    };
    owner.allocation = Allocation::MayAllocate;
    owner.generics = vec![GenericParameter::Type {
        name: "R".into(),
        bounds: bound.into_iter().collect(),
    }]
    .into();
    f
}

#[test]
fn generic_result_v_21_direct_receipts_and_forgeries() {
    for bound in [Some(Bound::Plain), Some(Bound::NoallocDrop), None] {
        for shape in ["Option", "Packet", "tuple"] {
            callback_result(bound, shape, false).valid();
        }
    }
}

fn refresh_owner(f: &mut Fixture, bound: Option<Bound>) {
    owner_sig(f, &[PassingMode::Borrow]);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(id("main")).unwrap().kind else {
        unreachable!()
    };
    owner.allocation = Allocation::MayAllocate;
    owner.generics = vec![
        GenericParameter::Type {
            name: "R".into(),
            bounds: bound.into_iter().collect(),
        },
        GenericParameter::Type {
            name: "S".into(),
            bounds: vec![Bound::Plain].into(),
        },
    ]
    .into();
}

fn replace_type(f: &mut Fixture, old: TypeId, new: TypeId) {
    for local in &mut f.body().locals {
        if local.ty == old {
            local.ty = new;
        }
    }
    for place in &mut f.body().places {
        if place.ty == old {
            place.ty = new;
        }
    }
    if f.body().result == old {
        f.body().result = new;
    }
    f.definition.types.adopt(new);
}

fn receiving_part(f: &mut Fixture, shape: &str, change: impl FnOnce(&mut Type)) {
    let result = f.body().result;
    let mut row = interner().get(result).unwrap().clone();
    let part = match &row.kind {
        TypeKind::Tuple(parts) => parts[0],
        TypeKind::Named { arguments, .. } => match arguments[0] {
            GenericArgument::Type(t) => t,
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let mut part = interner().get(part).unwrap().clone();
    change(&mut part);
    let part = interner().intern(part);
    if shape == "tuple" {
        row.kind = TypeKind::Tuple(vec![part].into());
    } else {
        row.kind = TypeKind::Named {
            definition: id(shape),
            arguments: vec![GenericArgument::Type(part)].into(),
        };
    }
    let received = interner().intern(row);
    replace_type(f, result, received);
}

fn rejected(f: &Fixture, entity: &'static str) {
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::TypeMismatch { entity }),
        "expected {entity}: {kinds:?}"
    );
}

#[test]
fn generic_result_v_21_changed_argument_and_unique_forgeries_fail() {
    let mut count = 0;
    for bound in [Some(Bound::Plain), Some(Bound::NoallocDrop), None] {
        for shape in ["Option", "Packet", "tuple"] {
            let mut changed = callback_result(bound, shape, false);
            let scope = changed.body().parameter_scope;
            receiving_part(&mut changed, shape, |row| {
                row.kind = TypeKind::Parameter(TypeParameterRef::body(scope, TypeParamId(1)));
                row.drop_allocation = DropAllocation::NoAlloc;
            });
            refresh_owner(&mut changed, bound);
            rejected(&changed, "constructor field");
            count += 1;
            // Plain values have no counted blocks and already prove uniqueness.
            if bound == Some(Bound::Plain) {
                continue;
            }
            let mut supplier = callback_result(bound, shape, true);
            supplier.valid();
            let read = supplier.body().parameters[0];
            let previous = supplier.body().locals[read.0 as usize].ty;
            let mut callback = interner().get(previous).unwrap().clone();
            let TypeKind::Function(header) = &mut callback.kind else {
                unreachable!()
            };
            let mut result = interner().get(header.result).unwrap().clone();
            result.modes.uniqueness = Uniqueness::Aliased;
            header.result = interner().intern(result);
            replace_type(&mut supplier, previous, interner().intern(callback));
            refresh_owner(&mut supplier, bound);
            rejected(&supplier, "call result");
            count += 1;
            let mut forged = callback_result(bound, shape, false);
            receiving_part(&mut forged, shape, |row| {
                row.modes.uniqueness = Uniqueness::Unique
            });
            refresh_owner(&mut forged, bound);
            rejected(&forged, "constructor field");
            count += 1;
        }
    }
    assert_eq!(count, 21);
    eprintln!("generic_result_v_21: {count} independent generic/unique forgeries rejected");
}

#[test]
fn generic_result_v_21_unrelated_bound_cannot_discharge_cleanup() {
    for shape in ["Option", "Packet", "tuple"] {
        let mut f = callback_result(None, shape, false);
        // A second, Plain parameter cannot prove nonallocating cleanup for R.
        receiving_part(&mut f, shape, |row| {
            row.drop_allocation = DropAllocation::NoAlloc
        });
        refresh_owner(&mut f, None);
        rejected(&f, "constructor field");
    }
}

#[test]
fn generic_result_v_21_retained_callback_cleanup_uses_its_own_bound() {
    for bound in [Some(Bound::Plain), Some(Bound::NoallocDrop), None] {
        let mut f = callback_result(bound, "tuple", false);
        let flag = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(flag, Rvalue::Constant(Constant::Bool(true)), 0);
        let value = match &f.body().blocks[0].terminator {
            Terminator::Call {
                returns: Some(receipt),
                ..
            } => receipt.destination,
            _ => unreachable!(),
        };
        let failure = f.cleanup(vec![drop(value.0)]);
        f.body().blocks[1].instructions.insert(
            0,
            inst(
                InstructionKind::Check {
                    condition: copy(flag),
                    rule: const { hirsel_diag::RuleId::of("SEM-1") },
                },
                failure,
            ),
        );
        // V's executable noalloc cleanup contract is carried by a closure
        // header. Keep R in the enclosing binder and make this an independently
        // typed executable child, as the existing cleanup-model controls do.
        let mut child = std::mem::replace(f.body(), Fixture::unit().body().clone());
        child.parameter_scope = scoped(NameSite::Statement(hirsel_syntax::StmtId(0)));
        let parameters = child
            .parameters
            .iter()
            .map(|p| Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: child.locals[p.0 as usize].ty,
            })
            .collect();
        let header = fn_type(parameters, child.result);
        f.definition.bodies.push(child);
        let closure = f.slot(TypeKind::Function(header), Storage::Temporary);
        f.assign(
            closure,
            Rvalue::Closure {
                context: CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![],
                destination: Destination::Inferred,
            },
            0,
        );
        let cleanup = f.cleanup(vec![drop(closure)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        owner_sig(&mut f, &[]);
        let SignatureKind::Function(owner) = &mut f.signatures.get_mut(id("main")).unwrap().kind
        else {
            unreachable!()
        };
        owner.generics = vec![GenericParameter::Type {
            name: "R".into(),
            bounds: bound.into_iter().collect(),
        }]
        .into();
        if bound.is_some() {
            f.valid();
        } else {
            rejected(&f, "body destructor allocation");
        }
    }
}
