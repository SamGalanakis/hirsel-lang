use super::*;

fn declare_owner(f: &mut Fixture) {
    owner_signature(f, PassingMode::Mut);
    let signature = f.signatures.get_mut(&path("main")).unwrap();
    if let SignatureKind::Function(fun) = &mut signature.kind {
        for parameter in &mut fun.parameters[1..] {
            parameter.passing = PassingMode::Borrow;
            parameter.ty = map_type_origins(interner(), parameter.ty, |origin, depth| {
                let mut origin = origin.clone();
                if let OriginRoot::Parameter(ParameterRef::Body { parameter, .. }) = origin.root {
                    origin.root = OriginRoot::Parameter(ParameterRef::Bound { depth, parameter });
                }
                Ok(origin)
            })
            .unwrap();
        }
    }
}

// Hand-built equivalent of replacing a sibling-branded field of a mutable
// parameter and then lending the whole record to another mutable parameter.
fn replaced_field() -> (Fixture, u32, u32) {
    let mut f = Fixture::unit();
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let mut sibling = ty(TypeKind::Primitive(Primitive::U64));
    sibling.brand.alternatives.push(Origin {
        root: OriginRoot::Sibling(FieldId(0)),
        fields: vec![].into(),
    });
    let sibling = f.definition.types.push(sibling);
    let record = TypeKind::Record(
        vec![
            Field {
                name: "owner".into(),
                ty: scalar,
                growable: false,
            },
            Field {
                name: "value".into(),
                ty: sibling,
                growable: false,
            },
        ]
        .into(),
    );
    let representation = f.definition.types.push(ty(record));
    let declaration = Signature {
        definition: id("Record"),
        metadata: Default::default(),
        fingerprint: ContentHash::default(),
        kind: SignatureKind::Type(Box::new(TypeDefinition {
            loop_source: None,
            format: None,
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
            resource: true,
            drop: None,
            fact: None,
            captures: None,
            destructor_allocation: Allocation::NoAlloc,
        })),
    };
    f.signatures.insert(id("Record"), declaration);
    let receiver = f.slot(
        TypeKind::Named {
            definition: id("Record"),
            arguments: vec![].into(),
        },
        Storage::Parameter,
    );
    let incoming = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
    let other = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
    let scope = f.body().parameter_scope;
    let origin = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    };
    f.body().places[receiver as usize].binding = Some(origin.clone());
    let mut branded = ty(TypeKind::Primitive(Primitive::U64));
    let mut owner = origin.clone();
    owner.fields.push(FieldId(0));
    branded.brand.alternatives.push(owner);
    let branded = f.definition.types.push(branded);
    f.body().locals[incoming as usize].ty = branded;
    f.body().places[incoming as usize].ty = branded;
    f.body().parameters = vec![LocalId(receiver), LocalId(incoming), LocalId(other)];
    let mut field = f.body().places[receiver as usize].clone();
    field.projection.push(Projection::Field(1));
    field.binding.as_mut().unwrap().fields.push(FieldId(1));
    field.ty = branded;
    let field_id = f.body().places.len() as u32;
    f.body().places.push(field);
    let field = field_id;
    let displaced = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    let displaced_root = f.body().places[displaced as usize].root;
    f.body().locals[displaced_root.0 as usize].ty = branded;
    f.body().places[displaced as usize].ty = branded;
    f.emit(InstructionKind::EndOrigin(PlaceId(field)));
    f.emit(InstructionKind::Replace {
        destination: PlaceId(field),
        value: copy(incoming),
        displaced: PlaceId(displaced),
        if_live: false,
    });
    f.emit(InstructionKind::StorageDead {
        place: PlaceId(displaced),
        if_live: false,
    });
    f.emit(InstructionKind::BindOrigin {
        source: PlaceId(field),
        place: PlaceId(field),
        kind: OriginBindingKind::Fresh,
    });
    declare_owner(&mut f);
    let mut callee = f.signatures.get(id("main")).unwrap().clone();
    callee.definition = id("sent");
    if let SignatureKind::Function(fun) = &mut callee.kind {
        fun.parameters.truncate(1);
    }
    f.signatures.insert(id("sent"), callee);
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("sent"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(receiver, UseKind::Mut),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    (f, incoming, other)
}

#[test]
fn a_mutable_call_preserves_the_replaced_fields_declared_sibling_brand() {
    let (f, _, _) = replaced_field();
    f.valid();
}

#[test]
fn replacement_still_requires_the_declared_sibling_owner() {
    let (mut f, incoming, _) = replaced_field();
    let scope = f.body().parameter_scope;
    let old = f.body().locals[incoming as usize].ty;
    let mut wrong = interner().get(old).unwrap().clone();
    wrong.brand.alternatives[0] = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(2))),
        fields: vec![].into(),
    };
    let wrong = f.definition.types.push(wrong);
    f.body().locals[incoming as usize].ty = wrong;
    f.body().places[incoming as usize].ty = wrong;
    declare_owner(&mut f);
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "replacement"
        }),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn a_mutable_call_still_expires_an_external_value_branded_by_its_argument() {
    let (mut f, incoming, _) = replaced_field();
    let old = f.body().locals[incoming as usize].ty;
    let destination = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    let destination_root = f.body().places[destination as usize].root;
    f.body().locals[destination_root.0 as usize].ty = old;
    f.body().places[destination as usize].ty = old;
    let value = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Use(operand(incoming, UseKind::Borrow)));
    f.body().blocks[1].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(destination),
            value,
        },
        0,
    ));
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidOrigin(PlaceId(incoming))),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn mutating_a_holder_does_not_renew_a_brand_of_an_outside_mutated_owner() {
    let (mut f, incoming, _) = replaced_field();
    let signature = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    fun.parameters[1].passing = PassingMode::Mut;
    let parameter = fun.parameters[1].clone();
    let callee = f.signatures.get_mut(&path("sent")).unwrap();
    let SignatureKind::Function(fun) = &mut callee.kind else {
        unreachable!()
    };
    fun.parameters.push(parameter);
    let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
        unreachable!()
    };
    arguments.push(CallArgument {
        parameter: ParamId(1),
        value: operand(incoming, UseKind::Mut),
    });
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidOrigin(PlaceId(incoming))),
        "{:?}",
        f.kinds()
    );
}
