//! Demand-analysis controls on original TIR data, not V or public execution.
use super::*;
use crate::sealed_source::tests::{assign, body, operand};
use hirsel_span::Span;
use hirsel_tir::*;
use hirsel_types::{
    Allocation, AuthoritySummary, BuiltinType, FunctionType, GenericArgument, ParamId, Primitive,
    Type, TypeId, TypeInterner, TypeKind, Uses,
};

struct Types {
    _types: std::sync::Arc<TypeInterner>,
    scalar: TypeId,
    arena: TypeId,
    bytes: TypeId,
    list: TypeId,
    callable: TypeId,
}
fn types() -> Types {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let arena = types.intern(Type::with_kind(TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("Arena")),
        arguments: vec![].into(),
    }));
    let list = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(bytes)].into(),
    }));
    let callable = types.intern(Type::with_kind(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: scalar,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })));
    Types {
        _types: types,
        scalar,
        arena,
        bytes,
        list,
        callable,
    }
}
fn input(body: &Body) -> DemandBody<'_> {
    DemandBody {
        body,
        calls: BTreeMap::new(),
        closures: BTreeMap::new(),
    }
}
fn call(body: &mut Body, arguments: &[(u32, UseKind)], result: Option<u32>) {
    body.blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: hirsel_syntax::DefPath::core("independently_selected_operation"),
            arguments: vec![],
        },
        arguments: arguments
            .iter()
            .enumerate()
            .map(|(i, &(place, usage))| CallArgument {
                parameter: ParamId(i as u32),
                value: Operand {
                    place: PlaceId(place),
                    usage,
                },
            })
            .collect(),
        returns: result.map(|r| CallReturn {
            destination: PlaceId(r),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
}
fn ret(body: &mut Body, local: u32) {
    body.blocks[0].terminator = Terminator::Return {
        value: operand(local),
        cleanup: CleanupId(0),
    };
}
fn seal() -> CallDemand {
    CallDemand::External(ExternalDemand {
        parameters: vec![1],
        inventory: vec![0],
        ..Default::default()
    })
}
fn arena_origin(body: &mut Body, local: u32) -> hirsel_types::Origin {
    let origin = hirsel_types::Origin {
        root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(ParamId(
            local,
        ))),
        fields: Default::default(),
    };
    body.places[local as usize].binding = Some(origin.clone());
    origin
}

#[test]
fn no_sinks_produce_empty_demand_even_with_calls_allocations_and_aliases() {
    let t = types();
    let mut callee = body(&[t.bytes, t.bytes], 1);
    assign(&mut callee, 1, Rvalue::Use(operand(0)));
    ret(&mut callee, 1);
    let mut caller = body(&[t.arena, t.bytes, t.list, t.bytes], 2);
    let origin = arena_origin(&mut caller, 0);
    assign(
        &mut caller,
        2,
        Rvalue::Construct {
            constructor: Constructor::Sequence,
            fields: vec![ConstructField {
                field: 0,
                value: operand(1),
            }],
            destination: Destination::Arena(origin),
        },
    );
    call(&mut caller, &[(1, UseKind::Borrow)], Some(3));
    let mut caller = input(&caller);
    caller.calls.insert(0, CallDemand::Bodies(vec![1]));
    assert_eq!(
        analyze(&[caller, input(&callee)]).unwrap(),
        vec![Requirements::default(); 2]
    );
}

#[test]
fn dead_inventory_demands_all_child_occurrences_without_root_reachability() {
    let t = types();
    let mut source = body(&[t.arena, t.bytes, t.list, t.scalar], 2);
    let origin = arena_origin(&mut source, 0);
    assign(
        &mut source,
        2,
        Rvalue::Construct {
            constructor: Constructor::Sequence,
            fields: vec![
                ConstructField {
                    field: 0,
                    value: operand(1),
                },
                ConstructField {
                    field: 1,
                    value: operand(1),
                },
            ],
            destination: Destination::Arena(origin),
        },
    );
    assign(&mut source, 3, Rvalue::Constant(Constant::Integer(7)));
    source.blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Drop {
            place: PlaceId(2),
            if_live: false,
        },
        span: Span::default(),
        on_failure: CleanupId(0),
    });
    call(&mut source, &[(0, UseKind::Take), (3, UseKind::Take)], None);
    let mut source = input(&source);
    source.calls.insert(0, seal());
    let requirements = analyze(&[source]).unwrap().remove(0);
    assert_eq!(requirements.payload_locals, BTreeSet::from([1, 2, 3]));
    assert!(requirements.inventory_locals.contains(&0));
    // Demand is per selector; occurrence multiplicity stays in original fields.
    // This does not claim the B3/B4 charge or runtime journal is implemented.
}

#[test]
fn result_demand_reaches_producer_before_scope_and_every_return_alternative() {
    let t = types();
    let mut make = body(&[t.bytes, t.bytes, t.bytes], 2);
    assign(&mut make, 2, Rvalue::Use(operand(0)));
    ret(&mut make, 2);
    make.blocks.push(Block {
        instructions: vec![],
        terminator: Terminator::Return {
            value: operand(1),
            cleanup: CleanupId(0),
        },
        span: Span::default(),
    });
    let mut caller = body(&[t.arena, t.bytes, t.bytes, t.bytes], 3);
    call(
        &mut caller,
        &[(1, UseKind::Borrow), (2, UseKind::Borrow)],
        Some(3),
    );
    let mut inner = body(&[t.arena, t.bytes], 2);
    call(&mut inner, &[(0, UseKind::Take), (1, UseKind::Take)], None);
    caller.blocks.push(inner.blocks.remove(0));
    let mut caller = input(&caller);
    caller.calls.insert(0, CallDemand::Bodies(vec![1]));
    caller.calls.insert(1, seal());
    // The appended seal must select the earlier call's result (local3).
    let body = caller.body;
    let mut fixed = body.clone();
    if let Terminator::Call { arguments, .. } = &mut fixed.blocks[1].terminator {
        arguments[1].value.place = PlaceId(3);
    }
    caller.body = &fixed;
    let requirements = analyze(&[caller, input(&make)]).unwrap();
    assert!(
        requirements[0]
            .payload_locals
            .is_superset(&BTreeSet::from([1, 2, 3]))
    );
    assert!(requirements[1].result);
    assert_eq!(requirements[1].payload_locals, BTreeSet::from([0, 1, 2]));
}

#[test]
fn unit_return_and_entered_failure_keep_projected_mutation_inputs_demanded() {
    let t = types();
    let mut append = body(&[t.list, t.bytes, t.list, t.list], 2);
    assign(
        &mut append,
        3,
        Rvalue::Construct {
            constructor: Constructor::Sequence,
            fields: vec![ConstructField {
                field: 0,
                value: operand(1),
            }],
            destination: Destination::Heap,
        },
    );
    append.blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Replace {
            destination: PlaceId(0),
            value: operand(3),
            displaced: PlaceId(2),
            if_live: false,
        },
        span: Span::default(),
        on_failure: CleanupId(0),
    });
    // No return: demand of caller mut output must still reach committed input.
    let nested = t._types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(t.list)].into(),
    }));
    let mut caller = body(&[t.arena, nested, t.bytes, t.scalar], 3);
    assign(&mut caller, 3, Rvalue::Constant(Constant::Integer(0)));
    caller.places.push(Place {
        root: LocalId(1),
        projection: vec![Projection::Index(LocalId(3))].into(),
        ty: t.list,
        loan: None,
        binding: None,
    });
    call(
        &mut caller,
        &[(4, UseKind::Mut), (2, UseKind::Borrow)],
        None,
    );
    let mut seal_body = body(&[t.arena, t.list], 2);
    call(
        &mut seal_body,
        &[(0, UseKind::Take), (1, UseKind::Take)],
        None,
    );
    caller.blocks.push(seal_body.blocks.remove(0));
    let mut caller = input(&caller);
    caller.calls.insert(0, CallDemand::Bodies(vec![1]));
    caller.calls.insert(1, seal());
    let requirements = analyze(&[caller, input(&append)]).unwrap();
    assert!(requirements[0].payload_locals.contains(&2));
    assert!(requirements[1].payload_locals.contains(&1));
    assert!(!requirements[1].result);
}

#[test]
fn scalar_returning_helper_retains_arena_effects_and_external_mutation_inputs() {
    let t = types();
    let mut helper = body(&[t.arena, t.bytes, t.list, t.scalar], 3);
    call(
        &mut helper,
        &[
            (0, UseKind::Borrow),
            (1, UseKind::Borrow),
            (2, UseKind::Mut),
        ],
        Some(3),
    );
    let mut caller = body(&[t.arena, t.bytes, t.list, t.scalar], 3);
    call(
        &mut caller,
        &[
            (0, UseKind::Borrow),
            (1, UseKind::Borrow),
            (2, UseKind::Mut),
        ],
        Some(3),
    );
    let mut seal_body = body(&[t.arena, t.scalar], 2);
    call(
        &mut seal_body,
        &[(0, UseKind::Take), (1, UseKind::Take)],
        None,
    );
    caller.blocks.push(seal_body.blocks.remove(0));
    // Seal an unrelated scalar, not the helper's produced payload.
    if let Terminator::Call { arguments, .. } = &mut caller.blocks[1].terminator {
        arguments[1].value.place = PlaceId(3);
    }
    let mut caller = input(&caller);
    caller.calls.insert(0, CallDemand::Bodies(vec![1]));
    caller.calls.insert(1, seal());
    let mut helper = input(&helper);
    helper.calls.insert(
        0,
        CallDemand::External(ExternalDemand {
            retained: vec![(0, vec![2])],
            mutations: vec![(2, vec![1])],
            ..Default::default()
        }),
    );
    let requirements = analyze(&[caller, helper]).unwrap();
    assert!(requirements[0].payload_locals.contains(&1));
    assert!(requirements[1].inventory_locals.contains(&0));
    assert!(
        requirements[1]
            .payload_locals
            .is_superset(&BTreeSet::from([1, 2]))
    );
}

#[test]
fn recursive_union_has_no_iteration_budget_fallback() {
    let t = types();
    let mut source = body(&[t.arena, t.bytes, t.bytes], 2);
    call(
        &mut source,
        &[(0, UseKind::Borrow), (1, UseKind::Borrow)],
        Some(2),
    );
    let mut final_body = body(&[t.arena, t.bytes], 2);
    call(
        &mut final_body,
        &[(0, UseKind::Take), (1, UseKind::Take)],
        None,
    );
    // Demand travels through a component longer than the native clone limit.
    let mut bodies: Vec<_> = (0..257).map(|_| input(&source)).collect();
    for (i, body) in bodies.iter_mut().enumerate() {
        body.calls
            .insert(0, CallDemand::Bodies(vec![(i + 1) % 258]));
    }
    let mut sink = input(&final_body);
    sink.calls.insert(0, seal());
    bodies.push(sink);
    // Also close the recursive component without removing the path to sink.
    bodies[256]
        .calls
        .insert(0, CallDemand::Bodies(vec![0, 257]));
    let requirements = analyze(&bodies).unwrap();
    assert_eq!(requirements.len(), 258);
    assert!(
        requirements
            .iter()
            .all(|r| r.payload_locals.contains(&1) && r.inventory_locals.contains(&0))
    );
}

#[test]
fn indirect_target_union_includes_the_257th_target() {
    let t = types();
    let mut caller = body(&[t.arena, t.bytes, t.callable, t.bytes], 3);
    call(
        &mut caller,
        &[(0, UseKind::Borrow), (1, UseKind::Borrow)],
        Some(3),
    );
    if let Terminator::Call { callee, .. } = &mut caller.blocks[0].terminator {
        *callee = Callee::Indirect {
            forward_caller: false,
            value: Operand {
                place: PlaceId(2),
                usage: UseKind::Borrow,
            },
            arguments: vec![],
        };
    }
    let mut ordinary = body(&[t.arena, t.bytes], 2);
    ret(&mut ordinary, 1);
    let mut consuming = ordinary.clone();
    call(
        &mut consuming,
        &[(0, UseKind::Take), (1, UseKind::Take)],
        None,
    );
    let mut caller = input(&caller);
    caller
        .calls
        .insert(0, CallDemand::Bodies((1..=257).collect()));
    let mut bodies = vec![caller];
    bodies.extend((0..256).map(|_| input(&ordinary)));
    let mut sink = input(&consuming);
    sink.calls.insert(0, seal());
    bodies.push(sink);
    let requirements = analyze(&bodies).unwrap();
    assert!(requirements[0].payload_locals.contains(&1));
    assert!(requirements[0].inventory_locals.contains(&0));
}

#[test]
fn demanded_capture_marks_original_environment_and_its_pre_scope_producer() {
    let t = types();
    let mut caller = body(&[t.arena, t.bytes, t.bytes, t.callable], 2);
    assign(&mut caller, 2, Rvalue::Use(operand(1)));
    assign(
        &mut caller,
        3,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::Scoped,
            body: BodyId(1),
            captures: vec![
                Operand {
                    place: PlaceId(0),
                    usage: UseKind::Borrow,
                },
                Operand {
                    place: PlaceId(2),
                    usage: UseKind::Borrow,
                },
            ],
            destination: Destination::Heap,
        },
    );
    let mut nested = body(&[t.arena, t.bytes], 0);
    for local in &mut nested.locals {
        local.storage = Storage::Capture;
    }
    call(&mut nested, &[(0, UseKind::Take), (1, UseKind::Take)], None);
    let mut caller = input(&caller);
    caller.closures.insert(1, 1);
    let mut nested = input(&nested);
    nested.calls.insert(0, seal());
    let requirements = analyze(&[caller, nested]).unwrap();
    assert!(
        requirements[0]
            .payload_locals
            .is_superset(&BTreeSet::from([1, 2, 3]))
    );
    assert!(requirements[0].inventory_locals.contains(&0));
    assert!(requirements[1].payload_locals.contains(&1));
    assert!(requirements[1].inventory_locals.contains(&0));
}

#[test]
fn missing_or_incomplete_source_expansion_is_never_known_empty() {
    let t = types();
    let mut source = body(&[t.scalar], 1);
    call(&mut source, &[(0, UseKind::Borrow)], None);
    assert!(matches!(
        analyze(&[input(&source)]),
        Err(SourceError::Unsupported(_))
    ));
    let mut empty = input(&source);
    empty.calls.insert(0, CallDemand::Bodies(vec![]));
    assert!(matches!(
        analyze(&[empty]),
        Err(SourceError::Unsupported(_))
    ));
    let mut known = input(&source);
    known
        .calls
        .insert(0, CallDemand::External(ExternalDemand::default()));
    assert_eq!(analyze(&[known]).unwrap(), vec![Requirements::default()]);
    let mut forged = input(&source);
    forged.calls.insert(
        0,
        CallDemand::External(ExternalDemand {
            parameters: vec![1],
            ..Default::default()
        }),
    );
    assert!(matches!(analyze(&[forged]), Err(SourceError::Malformed(_))));
}

#[test]
fn compact_projection_outlives_source_and_keeps_original_call_ports() {
    let t = types();
    let projection = {
        let mut source = body(&[t.arena, t.bytes, t.bytes], 2);
        assign(&mut source, 2, Rvalue::Use(operand(1)));
        call(&mut source, &[(0, UseKind::Take), (2, UseKind::Take)], None);
        BodyProjection::capture(&source).unwrap()
    };
    let calls = BTreeMap::from([(0, seal())]);
    let closures = BTreeMap::new();
    assert_eq!(projection.local_count(), 3);
    assert_eq!(projection.parameters(), &[0, 1]);
    assert_eq!(projection.calls()[&0].arguments[&1].local, 2);
    assert!(
        matches!(&projection.calls()[&0].callee, Callee::Definition { path, .. }
        if path == &hirsel_syntax::DefPath::core("independently_selected_operation"))
    );
    let demanded = analyze_compact(&[CompactBody {
        source: &projection,
        calls: &calls,
        closures: &closures,
    }])
    .unwrap();
    assert_eq!(demanded[0].payload_locals, BTreeSet::from([1, 2]));
    // Bytes payload demand does not itself consume an arena grant. Only the
    // actual seal grant (local0) seeds retained-inventory history here.
    assert_eq!(demanded[0].inventory_locals, BTreeSet::from([0]));
    let missing = BTreeMap::new();
    assert!(matches!(
        analyze_compact(&[CompactBody {
            source: &projection,
            calls: &missing,
            closures: &closures
        }]),
        Err(SourceError::Unsupported(
            "demand needs complete original call expansion"
        ))
    ));
}

#[test]
fn compact_projection_preserves_reused_closure_expression_owner_ports() {
    let t = types();
    let (caller, nested) = {
        let mut caller = body(&[t.bytes, t.scalar, t.callable, t.callable], 1);
        assign(
            &mut caller,
            2,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::Scoped,
                body: BodyId(1),
                captures: vec![operand(0)],
                destination: Destination::Heap,
            },
        );
        caller.blocks[0].instructions.push(Instruction {
            kind: InstructionKind::Assign {
                destination: PlaceId(3),
                value: RvalueId(0),
            },
            span: Span::default(),
            on_failure: CleanupId(0),
        });
        let mut nested = body(&[t.arena, t.bytes], 1);
        nested.locals[1].storage = Storage::Capture;
        call(
            &mut nested,
            &[(0, UseKind::Take), (1, UseKind::Borrow)],
            None,
        );
        (
            BodyProjection::capture(&caller).unwrap(),
            BodyProjection::capture(&nested).unwrap(),
        )
    };
    let none = BTreeMap::new();
    let closures = BTreeMap::from([(0, 1usize)]);
    let calls = BTreeMap::from([(0, seal())]);
    assert_eq!(caller.closures()[&0].len(), 2);
    let demanded = analyze_compact(&[
        CompactBody {
            source: &caller,
            calls: &none,
            closures: &closures,
        },
        CompactBody {
            source: &nested,
            calls: &calls,
            closures: &BTreeMap::new(),
        },
    ])
    .unwrap();
    assert_eq!(demanded[0].payload_locals, BTreeSet::from([0, 2, 3]));
    assert_eq!(demanded[1].payload_locals, BTreeSet::from([1]));
    let missing = BTreeMap::new();
    assert!(matches!(
        analyze_compact(&[CompactBody {
            source: &caller,
            calls: &none,
            closures: &missing
        }]),
        Err(SourceError::Unsupported(
            "demand needs original closure instance"
        ))
    ));
}

#[test]
fn arena_scope_connects_fresh_grant_inventory_and_callback_capture_demand() {
    let t = types();
    let mut callback = body(&[t.arena, t.bytes], 1);
    callback.locals[1].storage = Storage::Capture;
    call(
        &mut callback,
        &[(0, UseKind::Take), (1, UseKind::Take)],
        None,
    );
    let mut scope = body(&[t.callable, t.scalar], 1);
    call(&mut scope, &[(0, UseKind::Borrow)], Some(1));
    let mut scope = input(&scope);
    scope.calls.insert(
        0,
        CallDemand::ArenaScope {
            callback: 0,
            targets: vec![1],
        },
    );
    let mut callback = input(&callback);
    callback.calls.insert(0, seal());
    let requirements = analyze(&[scope, callback]).unwrap();
    assert_eq!(requirements[0].payload_locals, BTreeSet::from([0]));
    assert!(requirements[0].inventory_locals.is_empty());
    assert_eq!(requirements[1].payload_locals, BTreeSet::from([1]));
    assert_eq!(requirements[1].inventory_locals, BTreeSet::from([0]));
}

#[test]
fn arena_scope_preserves_callback_result_dependencies_and_empty_demand() {
    let t = types();
    let mut callback = body(&[t.arena, t.bytes], 1);
    callback.locals[1].storage = Storage::Capture;
    ret(&mut callback, 1);
    let mut source = body(&[t.arena, t.callable, t.bytes], 2);
    call(&mut source, &[(1, UseKind::Borrow)], Some(2));
    let mut inner = body(&[t.arena, t.callable, t.bytes], 2);
    call(&mut inner, &[(0, UseKind::Take), (2, UseKind::Take)], None);
    source.blocks.push(inner.blocks.remove(0));
    let mut scope = input(&source);
    scope.calls.insert(
        0,
        CallDemand::ArenaScope {
            callback: 0,
            targets: vec![1],
        },
    );
    scope.calls.insert(1, seal());
    let requirements = analyze(&[scope, input(&callback)]).unwrap();
    assert!(requirements[1].result);
    assert!(requirements[1].payload_locals.contains(&1));
    assert!(
        requirements[0]
            .payload_locals
            .is_superset(&BTreeSet::from([1, 2]))
    );

    source.blocks.pop();
    let mut scope = input(&source);
    scope.calls.insert(
        0,
        CallDemand::ArenaScope {
            callback: 0,
            targets: vec![1],
        },
    );
    assert_eq!(
        analyze(&[scope, input(&callback)]).unwrap(),
        vec![Requirements::default(); 2]
    );
}

#[test]
fn arena_scope_unknown_targets_and_wrong_callback_arity_are_not_empty() {
    let t = types();
    let mut source = body(&[t.callable, t.scalar], 1);
    call(&mut source, &[(0, UseKind::Borrow)], Some(1));
    let wrong = body(&[t.arena, t.bytes], 2);
    for targets in [vec![], vec![1], vec![2]] {
        let mut scope = input(&source);
        scope.calls.insert(
            0,
            CallDemand::ArenaScope {
                callback: 0,
                targets,
            },
        );
        assert!(analyze(&[scope, input(&wrong)]).is_err());
    }
}

#[test]
fn sealed_read_result_demand_reaches_owner_without_inventing_a_grant() {
    let t = types();
    let mut callback = body(&[t.bytes], 1);
    ret(&mut callback, 0);
    let mut source = body(&[t.arena, t.list, t.callable, t.bytes], 3);
    call(
        &mut source,
        &[(1, UseKind::Mut), (2, UseKind::Borrow)],
        Some(3),
    );
    let mut sink = source.clone();
    call(&mut sink, &[(0, UseKind::Take), (3, UseKind::Take)], None);
    source.blocks.push(sink.blocks.remove(0));
    let mut read = input(&source);
    read.calls.insert(
        0,
        CallDemand::RegionRead {
            owner: 0,
            callback: 1,
            targets: vec![1],
        },
    );
    read.calls.insert(1, seal());
    let requirements = analyze(&[read, input(&callback)]).unwrap();
    assert_eq!(requirements[0].payload_locals, BTreeSet::from([1, 3]));
    assert_eq!(requirements[0].inventory_locals, BTreeSet::from([0]));
    assert_eq!(requirements[1].payload_locals, BTreeSet::from([0]));
    assert!(requirements[1].result);
    assert!(requirements[1].inventory_locals.is_empty());
    source.blocks.pop();
    let mut read = input(&source);
    read.calls.insert(
        0,
        CallDemand::RegionRead {
            owner: 0,
            callback: 1,
            targets: vec![1],
        },
    );
    assert_eq!(
        analyze(&[read, input(&callback)]).unwrap(),
        vec![Requirements::default(); 2]
    );
}

#[test]
fn task_callback_result_and_captures_keep_demand_without_creating_an_arena_grant() {
    let t = types();
    for scoped in [false, true] {
        let mut callback = if scoped {
            body(&[t.scalar, t.bytes], 1)
        } else {
            body(&[t.bytes], 0)
        };
        let capture = u32::from(scoped);
        callback.locals[capture as usize].storage = Storage::Capture;
        ret(&mut callback, capture);
        let mut source = body(&[t.arena, t.callable, t.bytes], 2);
        call(
            &mut source,
            &[(0, UseKind::Borrow), (1, UseKind::Take)],
            Some(2),
        );
        let mut sink = source.clone();
        call(&mut sink, &[(0, UseKind::Take), (2, UseKind::Take)], None);
        source.blocks.push(sink.blocks.remove(0));
        let mut caller = input(&source);
        caller.calls.insert(
            0,
            CallDemand::TaskCallback {
                callback: 1,
                scoped,
                targets: vec![1],
            },
        );
        caller.calls.insert(1, seal());
        let requirements = analyze(&[caller, input(&callback)]).unwrap();
        assert!(
            requirements[0]
                .payload_locals
                .is_superset(&BTreeSet::from([1, 2]))
        );
        assert_eq!(requirements[1].payload_locals, BTreeSet::from([capture]));
        assert!(requirements[1].result);
        assert!(requirements[1].inventory_locals.is_empty());
        if scoped {
            assert!(!requirements[1].payload_locals.contains(&0));
        }
        for targets in [vec![], vec![2]] {
            let mut caller = input(&source);
            caller.calls.insert(
                0,
                CallDemand::TaskCallback {
                    callback: 1,
                    scoped,
                    targets,
                },
            );
            caller.calls.insert(1, seal());
            assert!(analyze(&[caller, input(&callback)]).is_err());
        }
        let mut caller = input(&source);
        caller.calls.insert(
            0,
            CallDemand::TaskCallback {
                callback: 1,
                scoped: !scoped,
                targets: vec![1],
            },
        );
        caller.calls.insert(1, seal());
        assert!(analyze(&[caller, input(&callback)]).is_err());
    }
}
