//! These are source extractor unit controls, not V or public execution evidence.
use super::*;
use hirsel_span::Span;
use hirsel_tir::*;
use hirsel_types::{
    Allocation, AuthoritySummary, BuiltinType, FunctionType, Primitive, Type, TypeKind, UseKind,
    Uses,
};

pub(super) fn body(types: &[TypeId], parameters: usize) -> Body {
    Body {
        arena_scope_grant: None,
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
pub(super) fn assign(body: &mut Body, destination: u32, value: Rvalue) {
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
pub(super) fn operand(place: u32) -> Operand {
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

#[test]
fn semantic_destination_33_arena_retains_original_parameter_lineage() {
    let types = TypeInterner::new(None);
    let arena = types.intern(Type::with_kind(TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("Arena")),
        arguments: vec![].into(),
    }));
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let tuple = types.intern(Type::with_kind(TypeKind::Tuple(vec![scalar].into())));
    let mut body = body(&[arena, scalar, tuple], 2);
    let origin = hirsel_types::Origin {
        root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(
            hirsel_types::ParamId(0),
        )),
        fields: Default::default(),
    };
    body.places[0].binding = Some(origin.clone());
    assign(
        &mut body,
        2,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: operand(1),
            }],
            destination: Destination::Arena(origin),
        },
    );
    let recipes = extract(&types, &body, &[4, 5]).unwrap();
    let PayloadOperation::Allocate {
        destination: PayloadDestination::Arena(arena),
    } = &recipes[0].recipe.operation
    else {
        panic!("source arena destination")
    };
    assert_eq!(arena.place, 0);
    assert_eq!(arena.producer, PayloadProducer::Parameter(4));
    assert_eq!(arena.capture_slot, None);
}

pub(super) fn witness_definition(body: Body) -> DefinitionRef {
    std::sync::Arc::new(hirsel_tir::Definition {
        caller_bodies: vec![BodyId(0)],
        protocol_associations: vec![],
        types: vec![].into(),
        path: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core(
            "seal_source_witness_fixture",
        )),
        inferred_unique: Default::default(),
        signature_hash: hirsel_syntax::ContentHash([0; 32]),
        bodies: vec![body],
        entry: BodyId(0),
        contracts: vec![],
    })
}
fn witness_input(tag: u8) -> hirsel_syntax::ContentHash {
    hirsel_syntax::ContentHash([tag; 32])
}
pub(super) fn witness_key(
    types: &TypeInterner,
    definition: &DefinitionRef,
) -> hirsel_layout::instance::InstanceKey {
    hirsel_layout::instance::InstanceKey::new(definition.path.path(), 0, &[], types, None, None)
        .unwrap()
}

#[test]
fn source_witness_33_expected_empty_is_distinct_from_unavailable_provider() {
    use hirsel_lir::operation::OperationDeclarations;
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let definition = witness_definition(body(&[scalar], 1));
    let key = witness_key(&types, &definition);
    let source = SourceInput::capture(
        witness_input(1),
        hirsel_syntax::ContentHash([2; 32]),
        definition,
    );
    let expected = source.derive(&key, &types, &[0], Ok).unwrap();
    let mut declarations = SourceDeclarations::new(hirsel_layout::operation::OperationRegistry {
        seal_sources: Default::default(),
        receipt_sources: Default::default(),
        types,
        signatures: Default::default(),
        owners: Default::default(),
        type_declarations: Default::default(),
        invocation_bodies: vec![],
        invocations: Default::default(),
        pretransfer_adapters: vec![],
        pretransfer_actuals: Vec::new(),
    });
    assert!(
        declarations
            .seal_source_witness(key.definition(), key.digest())
            .is_none()
    );
    assert!(
        declarations
            .seal_events(key.definition(), key.digest())
            .is_none()
    );
    declarations.insert(expected.clone()).unwrap();
    assert_eq!(
        declarations
            .seal_source_witness(key.definition(), key.digest())
            .as_ref(),
        Some(expected.witness())
    );
    assert_eq!(
        declarations.seal_events(key.definition(), key.digest()),
        Some(vec![])
    );
    assert_eq!(
        declarations.seal_payloads(key.definition(), key.digest()),
        Some(vec![])
    );
}

#[test]
fn source_witness_33_private_revision_and_complete_input_invalidate_same_shape() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let definition = witness_definition(body(&[scalar], 1));
    let key = witness_key(&types, &definition);
    let observe = |input, revision| {
        SourceInput::capture(input, revision, definition.clone())
            .derive(&key, &types, &[0], Ok)
            .unwrap()
    };
    let before = observe(witness_input(1), hirsel_syntax::ContentHash([2; 32]));
    let private_edit = observe(witness_input(1), hirsel_syntax::ContentHash([3; 32]));
    let input_edit = observe(witness_input(4), hirsel_syntax::ContentHash([2; 32]));
    assert_eq!(
        before.witness().expectations,
        private_edit.witness().expectations
    );
    assert_ne!(before.witness(), private_edit.witness());
    assert_ne!(before.witness(), input_edit.witness());
}

#[test]
fn source_witness_33_capture_keeps_original_lower_before_body_rewrite() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let tuple = types.intern(Type::with_kind(TypeKind::Tuple(vec![scalar].into())));
    let mut original = body(&[scalar, tuple], 0);
    assign(&mut original, 0, Rvalue::Constant(Constant::Integer(1)));
    assign(
        &mut original,
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
    let mut definition = witness_definition(original);
    let key = witness_key(&types, &definition);
    let captured = SourceInput::capture(
        witness_input(1),
        hirsel_syntax::ContentHash([2; 32]),
        definition.clone(),
    );
    std::sync::Arc::make_mut(&mut definition).bodies[0].rvalues[0] =
        Rvalue::Constant(Constant::Integer(9));
    let later = SourceInput::capture(
        witness_input(1),
        hirsel_syntax::ContentHash([3; 32]),
        definition,
    );
    let before = captured.derive(&key, &types, &[], Ok).unwrap();
    let after = later.derive(&key, &types, &[], Ok).unwrap();
    assert_ne!(before.witness().expectations, after.witness().expectations);
}

#[test]
fn source_witness_33_unexpanded_call_is_unavailable_not_empty() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let mut original = body(&[scalar], 1);
    original.blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: hirsel_syntax::DefPath::core("unexpanded_source_operation"),
            arguments: vec![],
        },
        arguments: vec![],
        returns: None,
        on_failure: CleanupId(0),
    };
    let definition = witness_definition(original);
    let key = witness_key(&types, &definition);
    let source = SourceInput::capture(
        witness_input(1),
        hirsel_syntax::ContentHash([2; 32]),
        definition,
    );
    assert!(matches!(
        source.derive(&key, &types, &[0], Ok),
        Err(SourceError::Unsupported(
            "source witness needs reviewed call event expansion"
        ))
    ));
}

#[test]
fn source_witness_33_provider_refuses_mixed_source_snapshots() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let definition = witness_definition(body(&[scalar], 1));
    let key = witness_key(&types, &definition);
    let observe = |tag| {
        SourceInput::capture(
            witness_input(tag),
            hirsel_syntax::ContentHash([2; 32]),
            definition.clone(),
        )
        .derive(&key, &types, &[0], Ok)
        .unwrap()
    };
    let one = observe(1);
    let two = observe(2);
    let mut declarations = SourceDeclarations::new(hirsel_layout::operation::OperationRegistry {
        seal_sources: Default::default(),
        receipt_sources: Default::default(),
        types,
        signatures: Default::default(),
        owners: Default::default(),
        type_declarations: Default::default(),
        invocation_bodies: vec![],
        invocations: Default::default(),
        pretransfer_adapters: vec![],
        pretransfer_actuals: Vec::new(),
    });
    declarations.insert(one).unwrap();
    assert!(declarations.insert(two).is_err());
}

#[test]
fn source_body_33_inline_construction_is_not_an_extra_semantic_allocation() {
    let types = TypeInterner::new(None);
    let arena = types.intern(Type::with_kind(TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("Arena")),
        arguments: vec![].into(),
    }));
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    for boxed in [false, true] {
        let root = types.intern(Type::with_kind(if boxed {
            TypeKind::Box(scalar)
        } else {
            TypeKind::Tuple(vec![scalar].into())
        }));
        let mut original = body(&[arena, scalar, root], 2);
        let origin = hirsel_types::Origin {
            root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(
                hirsel_types::ParamId(0),
            )),
            fields: Default::default(),
        };
        original.places[0].binding = Some(origin.clone());
        assign(
            &mut original,
            2,
            Rvalue::Construct {
                constructor: if boxed {
                    Constructor::Box
                } else {
                    Constructor::Tuple
                },
                fields: vec![ConstructField {
                    field: 0,
                    value: operand(1),
                }],
                destination: Destination::Arena(origin),
            },
        );
        let definition = witness_definition(original);
        let key = witness_key(&types, &definition);
        let expected = SourceInput::capture(
            witness_input(1),
            hirsel_syntax::ContentHash([2; 32]),
            definition,
        )
        .derive(&key, &types, &[0, 1], Ok)
        .unwrap();
        assert_eq!(expected.events().len(), usize::from(boxed));
        assert_eq!(expected.payloads().len(), 1);
        let source = expected.source();
        assert_eq!(source.reference.body, key.body());
        source.check().unwrap();
    }
}

#[test]
fn source_demand_33_uses_original_lower_snapshot_after_native_body_rewrite() {
    use super::demand::{DemandBody, analyze};
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let mut original = body(&[scalar], 0);
    assign(&mut original, 0, Rvalue::Constant(Constant::Integer(7)));
    let mut definition = witness_definition(original);
    let key = witness_key(&types, &definition);
    let captured = SourceInput::capture(witness_input(1), witness_input(2), definition.clone());
    std::sync::Arc::make_mut(&mut definition).bodies[0].rvalues[0] =
        Rvalue::Constant(Constant::Integer(9));
    let retained = captured.demand_body(&key).unwrap();
    assert_eq!(retained.rvalues[0], Rvalue::Constant(Constant::Integer(7)));
    assert_ne!(retained, &definition.bodies[0]);
    let demand = analyze(&[DemandBody {
        body: retained,
        calls: Default::default(),
        closures: Default::default(),
    }])
    .unwrap();
    assert_eq!(demand, vec![super::demand::Requirements::default()]);
}

#[test]
fn source_demand_projection_33_binds_original_instance_and_releases_definition() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let definition = witness_definition(body(&[scalar], 1));
    let key = witness_key(&types, &definition);
    let source = SourceInput::capture(witness_input(1), witness_input(2), definition.clone());
    for (path, body, error) in [
        (
            hirsel_syntax::DefPath::core("another_original_source"),
            0,
            "source witness definition identity",
        ),
        (key.definition().clone(), 1, "source witness body identity"),
    ] {
        let wrong = hirsel_layout::instance::InstanceKey::new(&path, body, &[], &types, None, None)
            .unwrap();
        assert_eq!(
            source.demand_projection(&wrong).err(),
            Some(SourceError::Malformed(error))
        );
    }
    let projection = source.demand_projection(&key).unwrap();
    assert_eq!(projection.reference().definition, *key.definition());
    assert_eq!(projection.reference().body, key.body());
    assert_eq!(projection.reference().instance, key.digest());
    assert_eq!(projection.source_identity(), witness_input(1));
    assert_eq!(projection.implementation_revision(), witness_input(2));
    let weak = std::sync::Arc::downgrade(&definition);
    drop(source);
    drop(definition);
    assert!(
        weak.upgrade().is_none(),
        "compact projection must retain no source DefinitionRef"
    );
    let calls = std::collections::BTreeMap::new();
    let closures = std::collections::BTreeMap::new();
    let requirements = demand::analyze_compact(&[demand::CompactBody {
        source: projection.projection(),
        calls: &calls,
        closures: &closures,
    }])
    .unwrap();
    assert_eq!(requirements, vec![demand::Requirements::default()]);
}

#[test]
fn source_nominal_121_classifies_current_representations_and_refuses_missing_declarations() {
    use hirsel_syntax::{DefId, DefPath};
    use hirsel_types::{Signature, SignatureKind, TypeDefinition};
    use std::sync::Arc;
    let types = TypeInterner::new(None);
    let arena = types.intern(Type::with_kind(TypeKind::Named {
        definition: DefId::intern(&DefPath::core("Arena")),
        arguments: vec![].into(),
    }));
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let nominal = DefId::intern(&DefPath::core("source_nominal_121"));
    let root = types.intern(Type::with_kind(TypeKind::Named {
        definition: nominal,
        arguments: vec![].into(),
    }));
    let mut original = body(&[arena, scalar, root], 2);
    let origin = hirsel_types::Origin {
        root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(
            hirsel_types::ParamId(0),
        )),
        fields: Default::default(),
    };
    original.places[0].binding = Some(origin.clone());
    assign(
        &mut original,
        2,
        Rvalue::Construct {
            constructor: Constructor::Record(nominal.path().clone()),
            fields: vec![ConstructField {
                field: 0,
                value: operand(1),
            }],
            destination: Destination::Arena(origin),
        },
    );
    let definition = witness_definition(original);
    let key = witness_key(&types, &definition);
    let source = SourceInput::capture(witness_input(1), witness_input(2), definition);
    assert!(source.derive(&key, &types, &[0, 1], Ok).is_err());
    for boxed in [false, true] {
        let shape = types.intern(Type::with_kind(if boxed {
            TypeKind::Box(scalar)
        } else {
            TypeKind::Tuple(vec![scalar].into())
        }));
        let declaration = Arc::new(Signature {
            definition: nominal,
            metadata: Default::default(),
            fingerprint: witness_input(3),
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                format: None,
                destructor_allocation: Allocation::NoAlloc,
                generics: vec![],
                representation: Some(shape),
                bounds: vec![],
                authority: AuthoritySummary::inert(),
                capability: None,
                implementation_authority: None,
                serializable: false,
                serializable_check: None,
                layout: Default::default(),
                distinct: true,
                resource: false,
                drop: None,
                fact: None,
                captures: None,
            }),
        });
        let expected = source
            .clone()
            .with_nominals([(nominal, declaration.clone())].into())
            .derive(&key, &types, &[0, 1], Ok)
            .unwrap();
        assert!(expected.events().is_empty());
        assert_eq!(expected.payloads().len(), 1);
        expected.source().check().unwrap();
        let mut opaque = declaration;
        let SignatureKind::Type(ty) = &mut Arc::make_mut(&mut opaque).kind else {
            unreachable!()
        };
        ty.representation = None;
        assert!(
            source
                .clone()
                .with_nominals([(nominal, opaque)].into())
                .derive(&key, &types, &[0, 1], Ok)
                .is_err()
        );
    }
}

#[test]
fn semantic_source_147_projected_aliases_keep_path_and_index_producers() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let variant = types.intern(Type::with_kind(TypeKind::Variant(
        vec![
            hirsel_types::Variant {
                name: "A".into(),
                fields: vec![].into(),
            },
            hirsel_types::Variant {
                name: "B".into(),
                fields: vec![].into(),
            },
            hirsel_types::Variant {
                name: "C".into(),
                fields: vec![].into(),
            },
            hirsel_types::Variant {
                name: "D".into(),
                fields: vec![(None, scalar), (None, scalar)].into(),
            },
        ]
        .into(),
    )));
    let list = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(variant)].into(),
    }));
    let root = types.intern(Type::with_kind(TypeKind::Tuple(
        vec![list, list, list].into(),
    )));
    let tuple = types.intern(Type::with_kind(TypeKind::Tuple(vec![scalar].into())));
    let mut original = body(&[root, scalar, variant, scalar, tuple], 2);
    original.places.push(Place {
        root: LocalId(0),
        projection: vec![Projection::Field(2), Projection::Index(LocalId(1))].into(),
        ty: variant,
        loan: None,
        binding: None,
    });
    original.places.push(Place {
        root: LocalId(2),
        projection: vec![Projection::VariantField {
            variant: 3,
            field: 1,
        }]
        .into(),
        ty: scalar,
        loan: None,
        binding: None,
    });
    assign(&mut original, 2, Rvalue::Use(operand(5)));
    assign(&mut original, 3, Rvalue::Use(operand(6)));
    assign(
        &mut original,
        4,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: operand(3),
            }],
            destination: Destination::Heap,
        },
    );
    let expected = PayloadProducer::Projected {
        root: Box::new(PayloadProducer::Parameter(8)),
        path: vec![
            PayloadProjection::Field(2),
            PayloadProjection::Index(Box::new(PayloadProducer::Parameter(9))),
            PayloadProjection::VariantField {
                variant: 3,
                field: 1,
            },
        ],
    };
    let recipes = extract(&types, &original, &[8, 9]).unwrap();
    assert_eq!(recipes[0].recipe.inputs[0].producer, expected);
    let text = hirsel_codec::to_text(&expected);
    assert_eq!(
        hirsel_codec::from_text::<PayloadProducer>(&text).unwrap(),
        expected
    );
    original.places[5].projection =
        vec![Projection::Field(1), Projection::Index(LocalId(1))].into();
    assert_ne!(
        extract(&types, &original, &[8, 9]).unwrap()[0]
            .recipe
            .inputs[0]
            .producer,
        expected
    );
    assign(&mut original, 1, Rvalue::Constant(Constant::Integer(0)));
    assert!(
        extract(&types, &original, &[8, 9]).is_err(),
        "ambiguous index source"
    );
}

#[test]
fn semantic_source_147_projected_writes_invalidate_the_whole_source() {
    let types = TypeInterner::new(None);
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let tuple = types.intern(Type::with_kind(TypeKind::Tuple(vec![scalar].into())));
    let mut original = body(&[tuple, tuple], 1);
    original.places.push(Place {
        root: LocalId(0),
        projection: vec![Projection::Field(0)].into(),
        ty: scalar,
        loan: None,
        binding: None,
    });
    assign(&mut original, 2, Rvalue::Constant(Constant::Integer(1)));
    assign(
        &mut original,
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
        extract(&types, &original, &[0]).err(),
        Some(SourceError::Unsupported(
            "payload operand needs reaching source alternatives"
        ))
    );
}

#[test]
fn semantic_source_147_projected_construction_cannot_disappear_from_original_inventory() {
    let types = TypeInterner::new(None);
    let arena = types.intern(Type::with_kind(TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("Arena")),
        arguments: vec![].into(),
    }));
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let list = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(scalar)].into(),
    }));
    let wrapper = types.intern(Type::with_kind(TypeKind::Tuple(vec![list].into())));
    let mut original = body(&[arena, scalar, wrapper, list], 3);
    let origin = hirsel_types::Origin {
        root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(
            hirsel_types::ParamId(0),
        )),
        fields: Default::default(),
    };
    original.places[0].binding = Some(origin.clone());
    original.places.push(Place {
        root: LocalId(2),
        projection: vec![Projection::Field(0)].into(),
        ty: list,
        loan: None,
        binding: None,
    });
    assign(
        &mut original,
        4,
        Rvalue::Construct {
            constructor: Constructor::Sequence,
            fields: vec![ConstructField {
                field: 0,
                value: operand(1),
            }],
            destination: Destination::Arena(origin),
        },
    );
    assert_eq!(
        extract(&types, &original, &[0, 1, 2]).err(),
        Some(SourceError::Unsupported(
            "projected payload construction needs source destination transport"
        ))
    );
    let InstructionKind::Assign { destination, .. } = &mut original.blocks[0].instructions[0].kind
    else {
        unreachable!()
    };
    *destination = PlaceId(3);
    let accepted = extract(&types, &original, &[0, 1, 2]).unwrap();
    assert_eq!(accepted.len(), 1);
    assert!(
        matches!(&accepted[0].recipe.operation, PayloadOperation::Allocate {
        destination: PayloadDestination::Arena(arena) } if arena.producer == PayloadProducer::Parameter(0))
    );
}
