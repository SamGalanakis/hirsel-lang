//! Direct V witnesses: retained values at a pre-entry failure, without J.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn resource(f: &mut Fixture, allocation: Allocation) -> TypeId {
    let mut signature = capability_signature("R");
    let repr = interner().intern(ty(TypeKind::Tuple(vec![].into())));
    let SignatureKind::Type(declaration) = &mut signature.kind else {
        unreachable!()
    };
    declaration.capability = None;
    declaration.authority = AuthoritySummary::inert();
    declaration.resource = true;
    declaration.representation = Some(repr);
    declaration.destructor_allocation = allocation;
    f.signatures.insert(id("R"), signature);
    // Deliberately forged cached cleanup: V must use the declaration above.
    f.definition.types.push(ty(TypeKind::Named {
        definition: id("R"),
        arguments: vec![].into(),
    }))
}

fn literal(f: &mut Fixture, child: Body, parameters: Vec<Parameter>) {
    let header = fn_type(parameters, child.result);
    f.definition.bodies.push(child);
    let c = f.slot(TypeKind::Function(header), Storage::Temporary);
    f.assign(
        c,
        Rvalue::Closure {
            context: CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let exit = f.cleanup(vec![drop(c)]);
    f.body().blocks[0].terminator = ret(0, exit);
}

fn parameter(child: &mut Body, ty: TypeId) -> u32 {
    let p = add_capture(child, ty);
    child.locals[p as usize].storage = Storage::Parameter;
    child.parameters.push(LocalId(p));
    p
}
fn formal(ty: TypeId, passing: PassingMode) -> Parameter {
    Parameter {
        name: None,
        label: None,
        passing,
        ty,
    }
}

#[test]
fn retained_pair_checks_declared_cleanup_at_the_failure_boundary() {
    let mut count = 0;
    for allocation in [Allocation::NoAlloc, Allocation::MayAllocate] {
        for fail_before_entry in [false, true] {
            for nested in [false, true] {
                let mut f = Fixture::unit();
                let leaf = resource(&mut f, allocation);
                let value = if nested {
                    let inner = f
                        .definition
                        .types
                        .push(ty(TypeKind::Tuple(vec![leaf].into())));
                    f.definition
                        .types
                        .push(ty(TypeKind::Tuple(vec![inner].into())))
                } else {
                    leaf
                };
                let boolean = f
                    .definition
                    .types
                    .push(ty(TypeKind::Primitive(Primitive::Bool)));
                let mut child = empty_body(&mut f);
                let a = parameter(&mut child, value);
                let b = parameter(&mut child, value);
                let flag = parameter(&mut child, boolean);
                let cleanup = child.add_cleanup(vec![drop(b), drop(a)]);
                if fail_before_entry {
                    child.blocks[0].instructions.push(inst(
                        InstructionKind::Check {
                            condition: operand(flag, UseKind::Borrow),
                            rule: const { hirsel_diag::RuleId::of("SEM-1") },
                        },
                        cleanup.0,
                    ));
                }
                // The consumer receives both operands only after the check.
                let pair = f
                    .definition
                    .types
                    .push(ty(TypeKind::Tuple(vec![value, value].into())));
                let result = add_capture(&mut child, pair);
                child.locals[result as usize].storage = Storage::Temporary;
                child.result = pair;
                let rv = child.rvalues.len() as u32;
                child.rvalues.push(Rvalue::Construct {
                    constructor: Constructor::Tuple,
                    fields: vec![
                        ConstructField {
                            field: 0,
                            value: take(a),
                        },
                        ConstructField {
                            field: 1,
                            value: take(b),
                        },
                    ],
                    destination: Destination::Inferred,
                });
                child.blocks[0].instructions.push(inst(
                    InstructionKind::Assign {
                        destination: PlaceId(result),
                        value: RvalueId(rv),
                    },
                    cleanup.0,
                ));
                let exit = child.add_cleanup(vec![drop(result), drop(b), drop(a)]);
                child.blocks[0].terminator = ret(result, exit.0);
                literal(
                    &mut f,
                    child,
                    vec![
                        formal(value, PassingMode::Take),
                        formal(value, PassingMode::Take),
                        formal(boolean, PassingMode::Borrow),
                    ],
                );
                let expected = allocation == Allocation::NoAlloc || !fail_before_entry;
                let violations = f.kinds();
                if expected {
                    assert!(
                        violations.is_empty(),
                        "allocation={allocation:?}, pre_entry={fail_before_entry}, nested={nested}: {violations:?}"
                    );
                } else {
                    assert!(!violations.is_empty());
                    assert!(
                        violations.iter().all(|v| *v
                            == ViolationKind::TypeMismatch {
                                entity: "body destructor allocation"
                            }),
                        "{violations:?}"
                    );
                }
                count += 1;
            }
        }
    }
    assert_eq!(count, 8);
}

#[test]
fn generic_bound_checks_both_actual_resources_after_a_later_check() {
    for allocation in [Allocation::NoAlloc, Allocation::MayAllocate] {
        let mut f = Fixture::unit();
        let actual = resource(&mut f, allocation);
        let kind = interner().get(actual).unwrap().kind.clone();
        let a = param(&mut f, kind.clone());
        let b = param(&mut f, kind.clone());
        owner_sig(&mut f, &[PassingMode::Take, PassingMode::Take]);
        let flag = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(flag, Rvalue::Constant(Constant::Bool(true)), 0);
        let cleanup = f.cleanup(vec![drop(b), drop(a)]);
        f.body().blocks[0].instructions.push(inst(
            InstructionKind::Check {
                condition: copy(flag),
                rule: const { hirsel_diag::RuleId::of("SEM-1") },
            },
            cleanup,
        ));
        let mut signature = function_signature(
            "callee",
            vec![(PassingMode::Take, kind.clone()), (PassingMode::Take, kind)],
            TypeKind::Tuple(vec![].into()),
        );
        let t = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let SignatureKind::Function(header) = &mut signature.kind else {
            unreachable!()
        };
        header.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::NoallocDrop].into(),
        }]
        .into();
        for p in &mut header.parameters {
            p.ty = t;
        }
        f.signatures.insert(id("callee"), signature);
        let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("callee"),
                arguments: vec![GenericArgument::Type(actual)],
            },
            arguments: vec![
                CallArgument {
                    parameter: ParamId(0),
                    value: take(a),
                },
                CallArgument {
                    parameter: ParamId(1),
                    value: take(b),
                },
            ],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(vec![], ret(result, 0)));
        if allocation == Allocation::NoAlloc {
            f.valid();
        } else {
            assert_eq!(
                f.kinds(),
                vec![ViolationKind::TypeMismatch {
                    entity: "generic bound"
                }]
            );
        }
    }
}

#[test]
fn erased_cleanup_summary_does_not_follow_the_invocation_effect() {
    for invocation in [Allocation::NoAlloc, Allocation::MayAllocate] {
        for cleanup in [DropAllocation::NoAlloc, DropAllocation::MayAllocate] {
            let mut f = Fixture::unit();
            let unit = unit_ty(&mut f);
            let mut call = fn_type(vec![], unit);
            call.allocation = invocation;
            let mut row = ty(TypeKind::Function(call));
            row.drop_allocation = cleanup.clone();
            let erased = f.definition.types.push(row);
            let mut child = empty_body(&mut f);
            let g = parameter(&mut child, erased);
            let exit = child.add_cleanup(vec![drop(g)]);
            child.blocks[0].terminator = ret(0, exit.0);
            literal(&mut f, child, vec![formal(erased, PassingMode::Take)]);
            if cleanup == DropAllocation::NoAlloc {
                f.valid();
            } else {
                assert_eq!(
                    f.kinds(),
                    vec![ViolationKind::TypeMismatch {
                        entity: "body destructor allocation"
                    }]
                );
            }
        }
    }
}

#[test]
fn captured_resource_cleanup_cannot_be_forged_from_noalloc_invocation() {
    for allocation in [Allocation::NoAlloc, Allocation::MayAllocate] {
        for claimed in [DropAllocation::NoAlloc, DropAllocation::MayAllocate] {
            let mut f = Fixture::unit();
            let r = resource(&mut f, allocation);
            let source = param(&mut f, interner().get(r).unwrap().kind.clone());
            let mut child = empty_body(&mut f);
            let capture = add_capture(&mut child, r);
            child.result = r;
            let exit = child.add_cleanup(vec![drop(capture)]);
            child.blocks[0].terminator = ret(capture, exit.0);
            // Invocation transfers r on every exit, so it has no cleanup
            // obligation. Dropping the uninvoked environment still destroys r.
            f.definition.bodies.push(child);
            let mut row = ty(TypeKind::Function(fn_type(vec![], r)));
            row.modes.multiplicity = Multiplicity::Once;
            row.drop_allocation = claimed.clone();
            let closure_ty = f.definition.types.push(row);
            let c = f.slot(
                interner().get(closure_ty).unwrap().kind.clone(),
                Storage::Local,
            );
            f.body().locals[c as usize].ty = closure_ty;
            f.body().places[c as usize].ty = closure_ty;
            let fail = f.cleanup(vec![drop(source)]);
            f.assign(
                c,
                Rvalue::Closure {
                    context: CaptureContext::OwningScoped,
                    body: BodyId(1),
                    captures: vec![take(source)],
                    destination: Destination::Inferred,
                },
                fail,
            );
            let exit = f.cleanup(vec![drop(c), drop(source)]);
            f.body().blocks[0].terminator = ret(0, exit);
            owner_sig(&mut f, &[PassingMode::Take]);
            let SignatureKind::Function(owner) =
                &mut f.signatures.get_mut(&path("main")).unwrap().kind
            else {
                unreachable!()
            };
            owner.allocation = Allocation::MayAllocate;
            if allocation == Allocation::NoAlloc || claimed == DropAllocation::MayAllocate {
                f.valid();
            } else {
                assert_eq!(
                    f.kinds(),
                    vec![ViolationKind::TypeMismatch {
                        entity: "closure cleanup"
                    }]
                );
            }
        }
    }
}

#[test]
fn indexed_tuple_operand_keeps_the_retained_resource_cleanup() {
    for allocation in [Allocation::NoAlloc, Allocation::MayAllocate] {
        let mut f = Fixture::unit();
        let r = resource(&mut f, allocation);
        let number = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U64)));
        let array = f.definition.types.push(ty(TypeKind::Builtin {
            kind: BuiltinType::Array,
            arguments: vec![GenericArgument::Type(number)].into(),
        }));
        let pair = f
            .definition
            .types
            .push(ty(TypeKind::Tuple(vec![r, number].into())));
        let mut child = empty_body(&mut f);
        let a = parameter(&mut child, r);
        let xs = parameter(&mut child, array);
        let index = parameter(&mut child, number);
        let element = child.places.len() as u32;
        child.places.push(Place {
            root: LocalId(xs),
            projection: vec![Projection::Index(LocalId(index))].into(),
            ty: number,
            loan: None,
            binding: None,
        });
        let result = add_capture(&mut child, pair);
        let result_root = child.places[result as usize].root;
        child.locals[result_root.0 as usize].storage = Storage::Temporary;
        child.result = pair;
        let cleanup = child.add_cleanup(vec![drop(a)]);
        let rv = child.rvalues.len() as u32;
        child.rvalues.push(Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![
                ConstructField {
                    field: 0,
                    value: take(a),
                },
                ConstructField {
                    field: 1,
                    value: take(element),
                },
            ],
            destination: Destination::Inferred,
        });
        child.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(result),
                value: RvalueId(rv),
            },
            cleanup.0,
        ));
        let exit = child.add_cleanup(vec![drop(result), drop(a)]);
        child.blocks[0].terminator = ret(result, exit.0);
        literal(
            &mut f,
            child,
            vec![
                formal(r, PassingMode::Take),
                formal(array, PassingMode::Borrow),
                formal(number, PassingMode::Borrow),
            ],
        );
        if allocation == Allocation::NoAlloc {
            f.valid();
        } else {
            assert_eq!(
                f.kinds(),
                vec![ViolationKind::TypeMismatch {
                    entity: "body destructor allocation"
                }]
            );
        }
    }
}
