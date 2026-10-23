//! These are source extractor unit controls, not V or public execution evidence.
use super::*;
use hirsel_span::Span;
use hirsel_tir::*;
use hirsel_types::{
    Allocation, AuthoritySummary, BuiltinType, FunctionType, Primitive, Type, TypeKind, UseKind,
    Uses,
};

fn body(types: &[TypeId], parameters: usize) -> Body {
    Body {
        parameter_scope: hirsel_syntax::LexicalOwner::first(&hirsel_syntax::DefPath::core(
            "sealed_source_fixture",
        ))
        .site(hirsel_types::NameSite::Item),
        locals: types
            .iter()
            .enumerate()
            .map(|(index, &ty)| Local {
                ty,
                storage: if index < parameters {
                    Storage::Parameter
                } else {
                    Storage::Local
                },
                scope: ScopeId(0),
                span: Span::default(),
            })
            .collect(),
        places: types
            .iter()
            .enumerate()
            .map(|(index, &ty)| Place {
                loan: None,
                binding: None,
                root: LocalId(index as u32),
                projection: Default::default(),
                ty,
            })
            .collect(),
        scopes: vec![Scope {
            parent: None,
            span: Span::default(),
        }],
        blocks: vec![Block {
            instructions: vec![],
            terminator: Terminator::Unreachable,
            span: Span::default(),
        }],
        rvalues: vec![],
        cleanups: vec![],
        cleanup_nodes: vec![],
        entry: BlockId(0),
        parameters: (0..parameters as u32).map(LocalId).collect(),
        result: *types.last().unwrap(),
        borrow_cursors: vec![],
        tail_sites: vec![],
        matches: vec![],
        scoped_parameters: vec![],
    }
}
fn assign(body: &mut Body, destination: u32, value: Rvalue) {
    let id = RvalueId(body.rvalues.len() as u32);
    body.rvalues.push(value);
    body.blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Assign {
            destination: PlaceId(destination),
            value: id,
        },
        span: Span::default(),
        on_failure: CleanupId(0),
    });
}
fn operand(place: u32) -> Operand {
    Operand {
        place: PlaceId(place),
        usage: UseKind::Copy,
    }
}
fn extract(
    types: &TypeInterner,
    body: &Body,
    parameters: &[u32],
) -> Result<Vec<SourcePayload>, SourceError> {
    payloads(
        &Context {
            body,
            body_id: BodyId(9),
            types,
            source_anchors: &[Anchor(0)],
            parameters,
        },
        Ok,
    )
}

#[test]
fn semantic_source_33_aliases_preserve_original_typed_operands_and_instance_parameters() {
    let types = TypeInterner::new(None);
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let list = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(bytes)].into(),
    }));
    let mut body = body(&[bytes, bytes, list], 1);
    assign(&mut body, 1, Rvalue::Use(operand(0)));
    assign(
        &mut body,
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
                    value: operand(0),
                },
            ],
            destination: Destination::Heap,
        },
    );
    let recipes = extract(&types, &body, &[2]).unwrap();
    let [recipe] = recipes.as_slice() else {
        panic!("one source construction");
    };
    assert_eq!(recipe.recipe.source.body, 9);
    assert_eq!(recipe.recipe.source.block, 0);
    assert_eq!(recipe.recipe.source.statement, 1);
    assert_eq!(
        recipe
            .recipe
            .inputs
            .iter()
            .map(|input| input.place)
            .collect::<Vec<_>>(),
        vec![1, 0]
    );
    for input in &recipe.recipe.inputs {
        assert_eq!(input.producer, PayloadProducer::Parameter(2));
        assert_eq!(
            input.ty,
            NativeType::Primitive(hirsel_lir::operation::Primitive::Bytes)
        );
    }
    assert_eq!(
        recipe.recipe.operation,
        PayloadOperation::Allocate {
            destination: PayloadDestination::Heap
        }
    );
}

#[test]
fn semantic_source_33_literal_string_and_bytes_never_invent_destination_events() {
    let types = TypeInterner::new(None);
    for (primitive, value) in [
        (Primitive::Bytes, Constant::Bytes(b"abc".to_vec())),
        (Primitive::String, Constant::String("abc".into())),
    ] {
        let ty = types.intern(Type::with_kind(TypeKind::Primitive(primitive)));
        let mut body = body(&[ty], 0);
        assign(&mut body, 0, Rvalue::Constant(value));
        assert!(extract(&types, &body, &[]).unwrap().is_empty());
    }
}

#[test]
fn semantic_source_33_closed_capture_slots_use_actual_capture_types() {
    let types = TypeInterner::new(None);
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let callable = types.intern(Type::with_kind(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: scalar,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })));
    let mut body = body(&[bytes, callable], 1);
    assign(
        &mut body,
        1,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::Escaping,
            body: BodyId(1),
            captures: vec![operand(0)],
            destination: Destination::Heap,
        },
    );
    let recipes = extract(&types, &body, &[0]).unwrap();
    assert_eq!(recipes[0].recipe.inputs.len(), 1);
    assert_eq!(recipes[0].recipe.inputs[0].capture_slot, Some(0));
    assert_eq!(
        recipes[0].recipe.inputs[0].ty,
        NativeType::Primitive(hirsel_lir::operation::Primitive::Bytes)
    );
    assert_eq!(
        recipes[0].recipe.inputs[0].producer,
        PayloadProducer::Parameter(0)
    );
}

#[test]
fn semantic_source_33_ambiguous_reaching_producers_fail_closed() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let tuple = types.intern(Type::with_kind(TypeKind::Tuple(vec![scalar].into())));
    let mut body = body(&[scalar, tuple], 0);
    assign(&mut body, 0, Rvalue::Constant(Constant::Integer(1)));
    assign(&mut body, 0, Rvalue::Constant(Constant::Integer(2)));
    assign(
        &mut body,
        1,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: operand(0),
            }],
            destination: Destination::Heap,
        },
    );
    assert_eq!(
        extract(&types, &body, &[]),
        Err(SourceError::Unsupported(
            "payload operand needs reaching source alternatives"
        ))
    );
}

// Extractor controls only: the call blocks are built directly, independently
// of candidate LIR. V acceptance is outside this helper's test boundary.
fn call_transport(
    projected: bool,
    indirect: bool,
    usage: UseKind,
) -> Result<Vec<SourcePayload>, SourceError> {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let tuple = types.intern(Type::with_kind(TypeKind::Tuple(vec![scalar].into())));
    let mut body = body(&[tuple, tuple], 1);
    let place = if projected {
        body.places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(0),
            projection: vec![Projection::Field(0)].into(),
            ty: scalar,
        });
        PlaceId(2)
    } else {
        PlaceId(0)
    };
    let value = Operand { place, usage };
    body.blocks[0].terminator = Terminator::Call {
        callee: if indirect {
            Callee::Indirect {
                forward_caller: false,
                value: value.clone(),
                arguments: vec![],
            }
        } else {
            Callee::Definition {
                path: hirsel_syntax::DefPath::core("source_mutation_control"),
                arguments: vec![],
            }
        },
        arguments: if indirect {
            vec![]
        } else {
            vec![CallArgument {
                parameter: hirsel_types::ParamId(0),
                value,
            }]
        },
        returns: None,
        on_failure: CleanupId(0),
    };
    assign(
        &mut body,
        1,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: operand(0),
            }],
            destination: Destination::Heap,
        },
    );
    extract(&types, &body, &[0])
}

#[test]
fn semantic_source_33_mut_call_invalidates_old_root_producer() {
    assert!(matches!(
        call_transport(false, false, UseKind::Mut),
        Err(SourceError::Unsupported(_))
    ));
}

#[test]
fn semantic_source_33_projected_mut_call_invalidates_whole_root_producer() {
    assert!(matches!(
        call_transport(true, false, UseKind::Mut),
        Err(SourceError::Unsupported(_))
    ));
}

#[test]
fn semantic_source_33_mut_indirect_call_invalidates_old_environment_producer() {
    assert!(matches!(
        call_transport(false, true, UseKind::Mut),
        Err(SourceError::Unsupported(_))
    ));
}

#[test]
fn semantic_source_33_borrow_call_preserves_source_producer() {
    assert!(call_transport(false, false, UseKind::Borrow).is_ok());
}
