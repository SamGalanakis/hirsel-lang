use super::*;

fn declaration(
    name: &str,
    kind: TypeKind,
    bounds: Vec<Bound>,
    opaque: bool,
    flat: bool,
) -> Signature {
    let mut types = TypeStore::default();
    let root = types.push(ty(kind));
    Signature {
        definition: path(name),
        metadata: SignatureMetadata {
            visibility: if opaque {
                hirsel_syntax::Visibility::PublicOpaque
            } else {
                hirsel_syntax::Visibility::Private
            },
            ..Default::default()
        },
        types,
        kind: SignatureKind::Type(TypeDefinition {
            loop_source: None,
            generics: vec![],
            representation: Some(root),
            bounds,
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            layout: Layout {
                flat,
                ..Default::default()
            },
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            destructor_allocation: Allocation::NoAlloc,
        }),
        fingerprint: ContentHash::default(),
    }
}
fn capability() -> Signature {
    let mut s = declaration(
        "Gate",
        TypeKind::Primitive(Primitive::U32),
        vec![Bound::Plain],
        true,
        false,
    );
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.representation = None;
    d.capability = Some(CapabilityKind::Global);
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
    s
}
fn verdict(s: Signature, accepted: bool) {
    let mut f = Fixture::unit();
    f.signatures.insert(path("Gate"), capability());
    let mut base = f.signatures.clone();
    f.signatures.insert(s.definition.clone(), s.clone());
    let expected = if accepted {
        vec![]
    } else {
        vec![ViolationKind::InvalidSignature(s.definition.clone())]
    };
    f.broken(expected.clone());
    let cached = ValidatedSignatures::new(f.signatures.clone()).validate(&f.definition);
    assert_eq!(
        cached
            .violations
            .iter()
            .map(|v| v.kind.clone())
            .collect::<Vec<_>>(),
        expected
    );
    base.insert(
        s.definition.clone(),
        declaration(
            &s.definition.0.name,
            TypeKind::Primitive(Primitive::U32),
            vec![],
            true,
            false,
        ),
    );
    let overlay = ValidatedSignatures::new(base).validate_with_signature(&f.definition, &s);
    assert_eq!(
        overlay
            .violations
            .iter()
            .map(|v| v.kind.clone())
            .collect::<Vec<_>>(),
        expected
    );
}
#[test]
fn opaque_owner_bounds_have_independent_ir_controls() {
    // Hand-built unused declaration: no source checker and no executable use can
    // hide the false bound. This is accepted when declaration::valid is removed.
    verdict(
        declaration(
            "Hidden",
            TypeKind::Primitive(Primitive::String),
            vec![Bound::Plain],
            true,
            false,
        ),
        false,
    );
    verdict(
        declaration(
            "Hidden",
            TypeKind::Primitive(Primitive::String),
            vec![Bound::Copyable],
            true,
            false,
        ),
        true,
    );
}
#[test]
fn opaque_owner_authority_has_an_independent_ir_control() {
    let mut s = declaration(
        "Hidden",
        TypeKind::Named {
            definition: path("Gate"),
            arguments: vec![].into(),
        },
        vec![Bound::Plain],
        true,
        false,
    );
    verdict(s.clone(), false);
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
    verdict(s, true);
    let mut extra = declaration(
        "Hidden",
        TypeKind::Primitive(Primitive::U32),
        vec![Bound::Plain],
        true,
        false,
    );
    let SignatureKind::Type(d) = &mut extra.kind else {
        unreachable!()
    };
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
    verdict(extra, true);
}
#[test]
fn flat_storage_has_an_independent_ir_control() {
    let mut s = declaration(
        "Flat",
        TypeKind::Named {
            definition: path("Gate"),
            arguments: vec![].into(),
        },
        vec![Bound::Flat],
        false,
        true,
    );
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
    verdict(s, false);
    verdict(
        declaration(
            "Flat",
            TypeKind::Primitive(Primitive::String),
            vec![Bound::Flat],
            false,
            true,
        ),
        true,
    );
}

#[test]
fn flat_storage_inspects_handles_inside_plain_nominal_fields() {
    let mut wrapper = declaration(
        "Holder",
        TypeKind::Primitive(Primitive::U32),
        vec![Bound::Plain],
        false,
        false,
    );
    let u32_type = wrapper.types.id(0).unwrap();
    let id = wrapper.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::SlotId,
        arguments: vec![GenericArgument::Type(u32_type)].into(),
    }));
    let root = wrapper.types.push(ty(TypeKind::Record(
        vec![Field {
            name: "id".into(),
            ty: id,
            growable: false,
        }]
        .into(),
    )));
    let SignatureKind::Type(d) = &mut wrapper.kind else {
        unreachable!()
    };
    d.representation = Some(root);
    let flat = declaration(
        "Flat",
        TypeKind::Named {
            definition: path("Holder"),
            arguments: vec![].into(),
        },
        vec![Bound::Flat],
        false,
        true,
    );
    let mut f = Fixture::unit();
    f.signatures.insert(path("Holder"), wrapper.clone());
    f.signatures.insert(path("Flat"), flat);
    // MEM-12 rejects a handle even when a Plain nominal wraps its scalar bits.
    // With declaration::valid disabled, this unused owner passes validation.
    f.broken(vec![ViolationKind::InvalidSignature(path("Flat"))]);
    let root = wrapper.types.push(ty(TypeKind::Record(
        vec![Field {
            name: "id".into(),
            ty: u32_type,
            growable: false,
        }]
        .into(),
    )));
    let SignatureKind::Type(d) = &mut wrapper.kind else {
        unreachable!()
    };
    d.representation = Some(root);
    f.signatures.insert(path("Holder"), wrapper);
    f.valid();
}

fn seal(kind: TypeKind, bounds: Vec<Bound>) -> Signature {
    let mut s = declaration("Seal", kind, bounds, true, false);
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.capability = Some(CapabilityKind::Global);
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Seal"))].into());
    d.implementation_authority = Some(AuthoritySummary::Known(
        vec![AuthorityTerm::Capability(path("Gate"))].into(),
    ));
    s
}

#[test]
fn seal_ceiling_has_independent_ir_controls() {
    let good = seal(
        TypeKind::Named {
            definition: path("Gate"),
            arguments: vec![].into(),
        },
        vec![Bound::Plain],
    );
    verdict(good.clone(), true);
    for ceiling in [
        None,
        Some(AuthoritySummary::inert()),
        Some(AuthoritySummary::Unknown),
    ] {
        let mut bad = good.clone();
        let SignatureKind::Type(d) = &mut bad.kind else {
            unreachable!()
        };
        d.implementation_authority = ceiling;
        verdict(bad, false);
    }
}

#[test]
fn seal_nominal_summary_has_independent_ir_controls() {
    let good = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
    verdict(good.clone(), true);
    for summary in [
        AuthoritySummary::inert(),
        AuthoritySummary::Known(
            vec![
                AuthorityTerm::Capability(path("Seal")),
                AuthorityTerm::Capability(path("Gate")),
            ]
            .into(),
        ),
    ] {
        let mut bad = good.clone();
        let SignatureKind::Type(d) = &mut bad.kind else {
            unreachable!()
        };
        d.authority = summary;
        verdict(bad, false);
    }
}

#[test]
fn seal_structural_bounds_have_independent_ir_controls() {
    for bound in [Bound::Plain, Bound::Sendable, Bound::NoallocDrop] {
        // A closure has no Plain or noalloc destruction proof, and its crossing
        // evidence is NotSendable. This owner is unused by executable IR.
        let mut good = seal(TypeKind::Primitive(Primitive::U32), vec![bound]);
        verdict(good.clone(), true);
        let signature = function_signature("callback", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(mut callback) = signature.kind else {
            unreachable!()
        };
        callback.result = import_type(&signature.types, callback.result, &mut good.types).unwrap();
        callback.authority = AuthoritySummary::inert();
        let root = good.types.push(ty(TypeKind::Function(callback)));
        let SignatureKind::Type(d) = &mut good.kind else {
            unreachable!()
        };
        d.representation = Some(root);
        verdict(good, false);
    }
}

#[test]
fn seal_excluded_bounds_have_independent_ir_controls() {
    verdict(
        seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]),
        true,
    );
    for bound in [
        Bound::Inert,
        Bound::Data,
        Bound::Key,
        Bound::Flat,
        Bound::Serializable,
    ] {
        verdict(
            seal(TypeKind::Primitive(Primitive::U32), vec![bound]),
            false,
        );
    }
}

#[test]
fn seal_resource_copy_promises_have_independent_ir_controls() {
    let mut good = seal(
        TypeKind::Primitive(Primitive::U32),
        vec![Bound::NoallocDrop],
    );
    let SignatureKind::Type(d) = &mut good.kind else {
        unreachable!()
    };
    d.resource = true;
    verdict(good.clone(), true);
    for bound in [Bound::Plain, Bound::Copyable] {
        let mut bad = good.clone();
        let SignatureKind::Type(d) = &mut bad.kind else {
            unreachable!()
        };
        d.bounds.push(bound);
        verdict(bad, false);
    }
}

#[test]
fn seal_ceiling_binders_have_independent_ir_controls() {
    let good = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
    verdict(good.clone(), true);
    let mut bad = good;
    let SignatureKind::Type(d) = &mut bad.kind else {
        unreachable!()
    };
    d.implementation_authority = Some(AuthoritySummary::Known(
        vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))]
        .into(),
    ));
    verdict(bad, false);
}

#[test]
fn seal_scope_retention_has_independent_ir_controls() {
    let good = seal(TypeKind::Primitive(Primitive::U32), vec![]);
    verdict(good.clone(), true);
    let mut bad = good;
    let mut row = ty(TypeKind::Primitive(Primitive::U32));
    row.modes.relation = Relation::environment();
    let root = bad.types.push(row);
    let SignatureKind::Type(d) = &mut bad.kind else {
        unreachable!()
    };
    d.representation = Some(root);
    verdict(bad, false);
}

#[test]
fn seal_scoped_authority_has_independent_ir_controls() {
    for scoped in [false, true] {
        let mut f = Fixture::unit();
        let mut gate = capability();
        let SignatureKind::Type(d) = &mut gate.kind else {
            unreachable!()
        };
        d.capability = Some(if scoped {
            CapabilityKind::Scoped
        } else {
            CapabilityKind::Global
        });
        d.bounds.clear();
        f.signatures.insert(path("Gate"), gate);
        f.signatures.insert(
            path("Seal"),
            seal(
                TypeKind::Named {
                    definition: path("Gate"),
                    arguments: vec![].into(),
                },
                vec![],
            ),
        );
        if scoped {
            f.broken(vec![ViolationKind::InvalidSignature(path("Seal"))]);
        } else {
            f.valid();
        }
    }
}

#[test]
fn scoped_seal_returns_have_independent_ir_controls() {
    for scoped in [false, true] {
        let mut f = Fixture::unit();
        let mut signature = capability();
        let SignatureKind::Type(d) = &mut signature.kind else {
            unreachable!()
        };
        d.capability = Some(if scoped {
            CapabilityKind::Scoped
        } else {
            CapabilityKind::Global
        });
        if scoped {
            d.bounds.clear();
        }
        f.signatures.insert(path("Gate"), signature);
        let kind = TypeKind::Named {
            definition: path("Gate"),
            arguments: vec![].into(),
        };
        let source = f.slot(kind.clone(), Storage::Parameter);
        let source_type = f.body().places[source as usize].ty;
        f.body().parameters.push(LocalId(source));
        f.body().result = source_type;
        f.body().blocks[0].terminator = ret(source, 0);
        let mut owner = function_signature("main", vec![(PassingMode::Take, kind.clone())], kind);
        // Plain parameters/results normalize to global rows; the local seal's
        // rows deliberately look global too, to isolate MODE-6 from MODE-1.
        let SignatureKind::Function(fun) = &mut owner.kind else {
            unreachable!()
        };
        let mut row = owner.types.get(fun.parameters[0].ty).unwrap().clone();
        row.modes.relation = Relation::global();
        fun.parameters[0].ty = owner.types.push(row);
        f.signatures.insert(path("main"), owner);
        if scoped {
            f.broken(vec![ViolationKind::TypeMismatch {
                entity: "scoped capability escape",
            }]);
        } else {
            f.valid();
        }
    }
}

#[test]
fn seal_projection_privacy_has_independent_ir_controls() {
    for (version, module) in [("1", "sibling"), ("2", "sibling")] {
        let mut f = Fixture::unit();
        f.definition.path.0.package.version = version.into();
        f.definition.path.0.modules = vec![module.into()];
        let mut signature = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
        let scalar = signature
            .types
            .push(ty(TypeKind::Primitive(Primitive::U32)));
        let root = signature.types.push(ty(TypeKind::Record(
            vec![Field {
                name: "private".into(),
                ty: scalar,
                growable: false,
            }]
            .into(),
        )));
        let SignatureKind::Type(d) = &mut signature.kind else {
            unreachable!()
        };
        d.representation = Some(root);
        f.signatures.insert(path("Seal"), signature);
        f.signatures.insert(path("Gate"), capability());
        let kind = TypeKind::Named {
            definition: path("Seal"),
            arguments: vec![].into(),
        };
        let parameter = f.slot(kind.clone(), Storage::Parameter);
        f.body().parameters.push(LocalId(parameter));
        let output = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
        let scalar = f.body().places[output as usize].ty;
        let projected = f.body().places.len() as u32;
        f.body().places.push(Place {
            root: LocalId(parameter),
            projection: vec![Projection::Field(0)].into(),
            ty: scalar,
            loan: None,
            binding: None,
        });
        f.assign(output, Rvalue::Use(copy(projected)), 0);
        f.body().result = scalar;
        f.body().blocks[0].terminator = ret(output, 0);
        let mut owner = function_signature(
            "main",
            vec![(PassingMode::Borrow, kind)],
            TypeKind::Primitive(Primitive::U32),
        );
        owner.definition = f.definition.path.clone();
        f.signatures.insert(owner.definition.clone(), owner);
        if version == "1" {
            f.valid();
        } else {
            assert!(
                f.kinds()
                    .contains(&ViolationKind::InvalidProjection(PlaceId(projected))),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn seal_generic_ceiling_has_independent_ir_controls() {
    let parameter = TypeParameterRef::bound(TypeParamId(0));
    let mut good = seal(TypeKind::Parameter(parameter), vec![Bound::Plain]);
    let SignatureKind::Type(d) = &mut good.kind else {
        unreachable!()
    };
    d.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Plain].into(),
    });
    d.implementation_authority = Some(AuthoritySummary::Known(
        vec![AuthorityTerm::TypeParameter(parameter)].into(),
    ));
    verdict(good.clone(), true);
    let SignatureKind::Type(d) = &mut good.kind else {
        unreachable!()
    };
    d.implementation_authority = Some(AuthoritySummary::Known(
        vec![AuthorityTerm::Capability(path("Gate"))].into(),
    ));
    verdict(good, false);
}

#[test]
fn seal_constructor_privacy_has_independent_ir_controls() {
    for package in [("validate", "1"), ("validate", "2"), ("client", "1")] {
        let mut f = Fixture::unit();
        f.definition.path.0.package.name = package.0.into();
        f.definition.path.0.package.version = package.1.into();
        f.definition.path.0.modules = vec!["sibling".into()];
        let mut signature = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
        let scalar = signature
            .types
            .push(ty(TypeKind::Primitive(Primitive::U32)));
        let root = signature.types.push(ty(TypeKind::Record(
            vec![Field {
                name: "private".into(),
                ty: scalar,
                growable: false,
            }]
            .into(),
        )));
        let SignatureKind::Type(d) = &mut signature.kind else {
            unreachable!()
        };
        d.representation = Some(root);
        f.signatures.insert(path("Seal"), signature);
        f.signatures.insert(path("Gate"), capability());
        let scalar = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
        f.assign(scalar, Rvalue::Constant(Constant::Integer(0)), 0);
        let result = f.slot(
            TypeKind::Named {
                definition: path("Seal"),
                arguments: vec![].into(),
            },
            Storage::Temporary,
        );
        f.assign(
            result,
            Rvalue::Construct {
                constructor: Constructor::Record(path("Seal")),
                fields: vec![ConstructField {
                    field: 0,
                    value: take(scalar),
                }],
                destination: Destination::Inferred,
            },
            0,
        );
        let mut owner = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
        owner.definition = f.definition.path.clone();
        f.signatures.insert(owner.definition.clone(), owner);
        if package == ("validate", "1") {
            f.valid();
        } else {
            f.broken(vec![ViolationKind::InvalidConstructor]);
        }
    }
}

#[test]
fn seal_serializable_opt_in_has_independent_ir_controls() {
    let good = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
    verdict(good.clone(), true);
    let mut bad = good;
    let SignatureKind::Type(d) = &mut bad.kind else {
        unreachable!()
    };
    d.serializable = true;
    verdict(bad, false);
}

#[test]
fn represented_empty_seals_still_require_a_written_ceiling() {
    let mut good = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
    let SignatureKind::Type(d) = &mut good.kind else {
        unreachable!()
    };
    d.implementation_authority = Some(AuthoritySummary::inert());
    verdict(good.clone(), true);
    let SignatureKind::Type(d) = &mut good.kind else {
        unreachable!()
    };
    d.implementation_authority = None;
    verdict(good, false);
}

#[test]
fn seal_finalizer_effects_have_independent_ir_controls() {
    for (allowed, allocation) in [
        (true, Allocation::MayAllocate),
        (false, Allocation::MayAllocate),
        (true, Allocation::NoAlloc),
    ] {
        let mut f = Fixture::unit();
        f.definition.path = path("Seal");
        let self_place = f.slot(
            TypeKind::Named {
                definition: path("Seal"),
                arguments: vec![].into(),
            },
            Storage::Parameter,
        );
        f.body().parameters.push(LocalId(self_place));
        let mut owner = seal(TypeKind::Primitive(Primitive::U32), vec![]);
        let SignatureKind::Type(d) = &mut owner.kind else {
            unreachable!()
        };
        d.resource = true;
        d.drop = Some(path("Seal"));
        d.destructor_allocation = allocation;
        if !allowed {
            d.implementation_authority = Some(AuthoritySummary::inert());
        }
        f.signatures.insert(path("Seal"), owner);
        f.signatures.insert(path("Gate"), capability());
        let mut operation = function_signature("operation", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(fun) = &mut operation.kind else {
            unreachable!()
        };
        fun.authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
        fun.uses = Uses::Only(fun.authority.clone());
        fun.allocation = Allocation::MayAllocate;
        f.signatures.insert(path("operation"), operation);
        let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("operation"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(destination),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(vec![], ret(destination, 0)));
        if !allowed {
            f.broken(vec![ViolationKind::TypeMismatch {
                entity: "body uses",
            }]);
        } else if allocation == Allocation::NoAlloc {
            f.broken(vec![ViolationKind::TypeMismatch {
                entity: "body allocation",
            }]);
        } else {
            f.valid();
        }
    }
}

#[test]
fn represented_seal_opacity_has_independent_ir_controls() {
    let good = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
    verdict(good.clone(), true);
    for visibility in [
        hirsel_syntax::Visibility::Private,
        hirsel_syntax::Visibility::Public,
    ] {
        let mut bad = good.clone();
        bad.metadata.visibility = visibility;
        verdict(bad, false);
    }
}

fn serializable_declaration(kind: TypeKind) -> Signature {
    let mut s = declaration("Data", kind, vec![Bound::Serializable], false, false);
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.serializable = true;
    s
}

#[test]
fn serializable_opt_in_is_independently_proved_even_on_unused_ir_declarations() {
    // Every bad snapshot below is accepted if serialization::declaration is
    // removed from signature validation. No Source judgment or use site exists.
    let good = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    verdict(good.clone(), true);
    let mut unmarked = good.clone();
    let SignatureKind::Type(d) = &mut unmarked.kind else {
        unreachable!()
    };
    d.serializable = false;
    verdict(unmarked, false);
    for opaque in [false, true] {
        let mut s = good.clone();
        if opaque {
            s.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
        }
        verdict(s, !opaque);
    }
    let mut s = good.clone();
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.resource = true;
    // Resource type-level Serializable alone is already forbidden by CAP-5 for
    // seals. An ordinary resource needs the independent serialization check.
    d.bounds.clear();
    verdict(s, false);
    let mut s = good;
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.bounds.clear();
    d.representation = None;
    verdict(s, false);
}

#[test]
fn serializable_authority_and_branded_ir_payloads_have_data_twins() {
    let mut data = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    verdict(data.clone(), true);
    let SignatureKind::Type(d) = &mut data.kind else {
        unreachable!()
    };
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
    verdict(data, false);
    let mut s = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    let root = match &s.kind {
        SignatureKind::Type(d) => d.representation.unwrap(),
        _ => unreachable!(),
    };
    let mut branded = s.types.get(root).unwrap().clone();
    branded.brand.alternatives = vec![Origin {
        root: OriginRoot::Sibling(FieldId(0)),
        fields: Default::default(),
    }]
    .into();
    let branded = s.types.push(branded);
    let box_type = s.types.push(ty(TypeKind::Box(branded)));
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.representation = Some(box_type);
    verdict(s, false);
    let good = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    verdict(good, true);
}

#[test]
fn serializable_ir_rejects_hidden_authority_and_unsupported_runtime_containers() {
    for kind in [
        BuiltinType::Ptr,
        BuiltinType::FnPtr,
        BuiltinType::Shared,
        BuiltinType::Slots,
        BuiltinType::SlotId,
        BuiltinType::Mutex,
        BuiltinType::Iter,
        BuiltinType::SortedMap,
        BuiltinType::Shape,
        BuiltinType::Region,
        BuiltinType::Atomic,
        BuiltinType::Task,
        BuiltinType::Sender,
        BuiltinType::Receiver,
    ] {
        let mut s = serializable_declaration(TypeKind::Primitive(Primitive::U64));
        let n = match &s.kind {
            SignatureKind::Type(d) => d.representation.unwrap(),
            _ => unreachable!(),
        };
        let r = s.types.push(ty(TypeKind::Builtin {
            kind,
            arguments: vec![GenericArgument::Type(n)].into(),
        }));
        let SignatureKind::Type(d) = &mut s.kind else {
            unreachable!()
        };
        d.representation = Some(r);
        verdict(s, false);
    }
    verdict(
        serializable_declaration(TypeKind::Named {
            definition: path("Gate"),
            arguments: vec![].into(),
        }),
        false,
    );
    verdict(
        serializable_declaration(TypeKind::Primitive(Primitive::U64)),
        true,
    );
}

#[test]
fn serializable_ir_templates_need_their_own_generic_schema_bound() {
    let mut s =
        serializable_declaration(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))));
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Serializable].into(),
    }];
    verdict(s.clone(), true);
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Inert].into(),
    }];
    verdict(s, false);
}

#[test]
fn refined_distinct_ir_cannot_opt_in_but_unconstrained_data_can() {
    let mut s = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.distinct = true;
    verdict(s.clone(), true);
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.fact = Some(Contract {
        kind: hirsel_syntax::ContractKind::Requires,
        ordinal: 0,
        code_hash: ContentHash::default(),
    });
    verdict(s, false);
}

#[test]
fn serializable_inert_callable_ir_still_has_no_codec() {
    let mut s = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    let result = match &s.kind {
        SignatureKind::Type(d) => d.representation.unwrap(),
        _ => unreachable!(),
    };
    let callable = s.types.push(ty(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })));
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.representation = Some(callable);
    verdict(s, false);
    verdict(
        serializable_declaration(TypeKind::Primitive(Primitive::U64)),
        true,
    );
}

#[test]
fn growing_schema_limits_remain_operational_and_owner_replacement_clears_them() {
    let mut s =
        serializable_declaration(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))));
    let parameter = match &s.kind {
        SignatureKind::Type(d) => d.representation.unwrap(),
        _ => unreachable!(),
    };
    let list = s.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(parameter)].into(),
    }));
    let recur = s.types.push(ty(TypeKind::Named {
        definition: path("Data"),
        arguments: vec![GenericArgument::Type(list)].into(),
    }));
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Serializable].into(),
    }];
    d.representation = Some(recur);
    let mut f = Fixture::unit();
    f.signatures.insert(s.definition.clone(), s);
    let checked = ValidatedSignatures::new(f.signatures.clone());
    let expected = ViolationKind::TypeProofToolingLimit {
        limit: MAX_POLYMORPHIC_EXPANSIONS,
    };
    assert_eq!(
        checked
            .validate(&f.definition)
            .violations
            .iter()
            .map(|v| v.kind.clone())
            .collect::<Vec<_>>(),
        vec![expected.clone()]
    );
    assert_eq!(f.kinds(), vec![expected]);
    let replacement = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    assert!(
        checked
            .validate_with_signature(&f.definition, &replacement)
            .violations
            .is_empty()
    );
}
