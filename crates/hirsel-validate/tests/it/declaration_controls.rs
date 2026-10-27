use super::*;

fn declaration(
    name: &str,
    kind: TypeKind,
    bounds: Vec<Bound>,
    opaque: bool,
    flat: bool,
) -> Signature {
    let root = interner().intern(ty(kind));
    Signature {
        definition: id(name),
        metadata: SignatureMetadata {
            visibility: if opaque {
                hirsel_syntax::Visibility::PublicOpaque
            } else {
                hirsel_syntax::Visibility::Private
            },
            ..Default::default()
        },
        kind: SignatureKind::Type(TypeDefinition {
            loop_source: None,
            format: None,
            generics: vec![],
            representation: Some(root),
            bounds,
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Layout {
                flat,
                ..Default::default()
            },
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
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
    verdict_with(s, vec![], accepted)
}

/// [`verdict`] with `extras` (a check's function) beside the declaration.
fn verdict_with(s: Signature, extras: Vec<Signature>, accepted: bool) {
    let mut f = Fixture::unit();
    f.signatures.insert(id("Gate"), capability());
    for extra in extras {
        f.signatures.insert(extra.definition.clone(), extra);
    }
    f.signatures.insert(s.definition.clone(), s.clone());
    let expected = if accepted {
        vec![]
    } else {
        vec![ViolationKind::InvalidSignature(s.definition.path().clone())]
    };
    f.broken(expected.clone());
    let mut verdicts = ValidatedSignatures::default();
    let universe = verdicts.universe(
        interner(),
        f.signatures.keys().collect::<Vec<_>>(),
        &f.signatures,
    );
    assert_eq!(
        universe
            .errors
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
            definition: id("Gate"),
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
            definition: id("Gate"),
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
    let u32_type = interner().intern(ty(TypeKind::Primitive(Primitive::U32)));
    let slot = interner().intern(ty(TypeKind::Builtin {
        kind: BuiltinType::SlotId,
        arguments: vec![GenericArgument::Type(u32_type)].into(),
    }));
    let root = interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "id".into(),
            ty: slot,
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
            definition: id("Holder"),
            arguments: vec![].into(),
        },
        vec![Bound::Flat],
        false,
        true,
    );
    let mut f = Fixture::unit();
    f.signatures.insert(id("Holder"), wrapper.clone());
    f.signatures.insert(id("Flat"), flat);
    // MEM-12 rejects a handle even when a Plain nominal wraps its scalar bits.
    // With declaration::valid disabled, this unused owner passes validation.
    f.broken(vec![ViolationKind::InvalidSignature(path("Flat"))]);
    let root = interner().intern(ty(TypeKind::Record(
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
    f.signatures.insert(id("Holder"), wrapper);
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
            definition: id("Gate"),
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
        callback.authority = AuthoritySummary::inert();
        let root = interner().intern(ty(TypeKind::Function(callback)));
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
    let root = interner().intern(row);
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
        f.signatures.insert(id("Gate"), gate);
        f.signatures.insert(
            id("Seal"),
            seal(
                TypeKind::Named {
                    definition: id("Gate"),
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
        f.signatures.insert(id("Gate"), signature);
        let kind = TypeKind::Named {
            definition: id("Gate"),
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
        let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
        row.modes.relation = Relation::global();
        fun.parameters[0].ty = interner().intern(row);
        f.signatures.insert(id("main"), owner);
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
        {
            let mut path = f.definition.path.path().clone();
            path.0.package.version = version.into();
            path.0.modules = vec![module.into()];
            f.definition.path = DefId::intern(&path);
        }
        let mut signature = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
        let scalar = interner().intern(ty(TypeKind::Primitive(Primitive::U32)));
        let root = interner().intern(ty(TypeKind::Record(
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
        f.signatures.insert(id("Seal"), signature);
        f.signatures.insert(id("Gate"), capability());
        let kind = TypeKind::Named {
            definition: id("Seal"),
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
        {
            let mut path = f.definition.path.path().clone();
            path.0.package.name = package.0.into();
            path.0.package.version = package.1.into();
            path.0.modules = vec!["sibling".into()];
            f.definition.path = DefId::intern(&path);
        }
        let mut signature = seal(TypeKind::Primitive(Primitive::U32), vec![Bound::Plain]);
        let scalar = interner().intern(ty(TypeKind::Primitive(Primitive::U32)));
        let root = interner().intern(ty(TypeKind::Record(
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
        f.signatures.insert(id("Seal"), signature);
        f.signatures.insert(id("Gate"), capability());
        let scalar = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
        f.assign(scalar, Rvalue::Constant(Constant::Integer(0)), 0);
        let result = f.slot(
            TypeKind::Named {
                definition: id("Seal"),
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
        f.definition.path = id("Seal");
        let self_place = f.slot(
            TypeKind::Named {
                definition: id("Seal"),
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
        f.signatures.insert(id("Seal"), owner);
        f.signatures.insert(id("Gate"), capability());
        let mut operation = function_signature("operation", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(fun) = &mut operation.kind else {
            unreachable!()
        };
        fun.authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
        fun.uses = Uses::Only(fun.authority.clone());
        fun.allocation = Allocation::MayAllocate;
        f.signatures.insert(id("operation"), operation);
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
    let mut branded = interner().get(root).unwrap().clone();
    branded.brand.alternatives = vec![Origin {
        root: OriginRoot::Sibling(FieldId(0)),
        fields: Default::default(),
    }]
    .into();
    let branded = interner().intern(branded);
    let box_type = interner().intern(ty(TypeKind::Box(branded)));
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
        BuiltinType::TypeShape,
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
        let r = interner().intern(ty(TypeKind::Builtin {
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
            definition: id("Gate"),
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
fn unbounded_ir_parameters_are_conditional_never_a_published_bound() {
    // A declaration marked serializable may leave its parameters unbounded (core Option,
    // std ReadError[E]); its uses prove the arguments. A published Serializable bound
    // holds at every instantiation, so it needs the declared bounds alone: without that
    // check in serialization::declaration the bounded twins are accepted.
    let parameter = TypeParameterRef::bound(TypeParamId(0));
    let mut conditional = serializable_declaration(TypeKind::Parameter(parameter));
    let SignatureKind::Type(d) = &mut conditional.kind else {
        unreachable!()
    };
    d.bounds.clear();
    d.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }];
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::TypeParameter(parameter)].into());
    for core in [false, true] {
        let mut s = conditional.clone();
        if core {
            s.definition = DefId::intern(&hirsel_syntax::DefPath::core("Data"));
        }
        verdict(s.clone(), true);
        let SignatureKind::Type(d) = &mut s.kind else {
            unreachable!()
        };
        d.bounds = vec![Bound::Serializable];
        verdict(s, false);
    }
}

#[test]
fn a_checked_opaque_opt_in_names_an_exact_check_of_its_module() {
    // An opaque type opts in through a check (SER-1): `@noalloc fn(value: T) -> bool
    // uses {}` in T's own module. Without serialization::check_function the missing
    // check, the other module's and every bad header below are accepted; without the
    // rest of serialization::checked so are the transparent and bounded twins.
    let named = |name: &str| TypeKind::Named {
        definition: id(name),
        arguments: vec![].into(),
    };
    let mut s = serializable_declaration(TypeKind::Primitive(Primitive::U8));
    s.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.bounds.clear();
    let number = d.representation.take().unwrap();
    let representation = interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "number".into(),
            ty: number,
            growable: false,
        }]
        .into(),
    )));
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.serializable_check = Some(SerializableCheck {
        function: path("valid"),
        representation,
    });
    let good = || {
        function_signature(
            "valid",
            vec![(PassingMode::Borrow, named("Data"))],
            TypeKind::Primitive(Primitive::Bool),
        )
    };
    verdict_with(s.clone(), vec![good()], true);
    // A missing check, or one of another module.
    verdict(s.clone(), false);
    let mut elsewhere = good();
    {
        let mut path = elsewhere.definition.path().clone();
        path.0.modules = vec!["other".into()];
        elsewhere.definition = DefId::intern(&path);
    }
    let mut moved = s.clone();
    let SignatureKind::Type(d) = &mut moved.kind else {
        unreachable!()
    };
    d.serializable_check.as_mut().unwrap().function = elsewhere.definition.path().clone();
    verdict_with(moved, vec![elsewhere], false);
    // Headers that are no check: allocating, with authority, a non-bool result, a
    // mut value, another type's value.
    let mut allocating = good();
    let mut authorized = good();
    if let SignatureKind::Function(f) = &mut allocating.kind {
        f.allocation = Allocation::MayAllocate;
    }
    if let SignatureKind::Function(f) = &mut authorized.kind {
        f.uses = Uses::Only(AuthoritySummary::Known(
            vec![AuthorityTerm::Capability(path("Gate"))].into(),
        ));
    }
    let wide = function_signature(
        "valid",
        vec![(PassingMode::Borrow, named("Data"))],
        TypeKind::Primitive(Primitive::U8),
    );
    let mutating = function_signature(
        "valid",
        vec![(PassingMode::Mut, named("Data"))],
        TypeKind::Primitive(Primitive::Bool),
    );
    let other = function_signature(
        "valid",
        vec![(PassingMode::Borrow, named("Gate"))],
        TypeKind::Primitive(Primitive::Bool),
    );
    for bad in [allocating, authorized, wide, mutating, other] {
        verdict_with(s.clone(), vec![bad], false);
    }
    // A checked declaration must be opaque and publish no structural bound.
    let mut transparent = s.clone();
    transparent.metadata.visibility = hirsel_syntax::Visibility::Private;
    verdict_with(transparent, vec![good()], false);
    let mut bounded = s;
    let SignatureKind::Type(d) = &mut bounded.kind else {
        unreachable!()
    };
    d.bounds = vec![Bound::Serializable];
    verdict_with(bounded, vec![good()], false);
}

#[test]
fn only_the_core_path_is_serializable_as_opaque_text() {
    // A core Path has no representation: its schema is its text. Without the core-identity
    // check in serialization::text_path the same-spelled user twin is accepted, and without
    // its authority check so is the core twin that holds a capability.
    let mut user = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    user.definition = id("Path");
    user.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    let SignatureKind::Type(d) = &mut user.kind else {
        unreachable!()
    };
    d.representation = None;
    let mut core = user.clone();
    core.definition = DefId::intern(&hirsel_syntax::DefPath::core("Path"));
    verdict(core.clone(), true);
    verdict(user, false);
    let SignatureKind::Type(d) = &mut core.kind else {
        unreachable!()
    };
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
    verdict(core, false);
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
    let callable = interner().intern(ty(TypeKind::Function(FunctionType {
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
    let list = interner().intern(ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(parameter)].into(),
    }));
    let recur = interner().intern(ty(TypeKind::Named {
        definition: id("Data"),
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
    let expected = ViolationKind::TypeProofToolingLimit {
        limit: MAX_POLYMORPHIC_EXPANSIONS,
    };
    let mut verdicts = ValidatedSignatures::default();
    let universe = |verdicts: &mut ValidatedSignatures, f: &Fixture| {
        verdicts
            .universe(
                interner(),
                f.signatures.keys().collect::<Vec<_>>(),
                &f.signatures,
            )
            .errors
            .into_iter()
            .map(|v| v.kind)
            .collect::<Vec<_>>()
    };
    assert_eq!(universe(&mut verdicts, &f), vec![expected.clone()]);
    assert_eq!(f.kinds(), vec![expected]);
    let replacement = serializable_declaration(TypeKind::Primitive(Primitive::U64));
    f.signatures
        .insert(replacement.definition.clone(), replacement);
    assert!(universe(&mut verdicts, &f).is_empty());
}

#[test]
fn caller_attribution_is_bound_to_the_independent_declaration() {
    let mut f = Fixture::unit();
    let signature = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    f.signatures.insert(signature.definition.clone(), signature);
    f.valid();
    f.definition.caller_bodies = vec![f.definition.entry];
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "caller attribution",
    }]);
    f.signatures
        .get_mut(&path("main"))
        .unwrap()
        .metadata
        .track_caller = true;
    f.valid();
    f.definition.caller_bodies.clear();
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "caller attribution",
    }]);
}

#[test]
fn caller_tracking_requires_an_ordinary_function() {
    let mut signature = capability();
    verdict(signature.clone(), true);
    signature.metadata.track_caller = true;
    verdict(signature, false);
    let mut f = Fixture::unit();
    let mut foreign = function_signature("external", vec![], TypeKind::Primitive(Primitive::U64));
    let SignatureKind::Function(function) = &mut foreign.kind else {
        unreachable!()
    };
    function.foreign = Some(ForeignContract {
        abi: "c".into(),
        blocking: false,
        aliasing_contract: "Retains nothing.".into(),
    });
    f.signatures.insert(foreign.definition.clone(), foreign);
    f.valid();
    f.signatures
        .get_mut(&path("external"))
        .unwrap()
        .metadata
        .track_caller = true;
    f.broken(vec![ViolationKind::InvalidSignature(path("external"))]);
}

#[test]
fn caller_body_ids_are_checked_at_the_structural_boundary() {
    let mut f = Fixture::unit();
    assert!(validate_structure(&f.definition, interner()).is_empty());
    f.definition.caller_bodies = vec![f.definition.entry];
    assert!(validate_structure(&f.definition, interner()).is_empty());
    f.definition.caller_bodies = vec![BodyId(99)];
    assert_eq!(
        validate_structure(&f.definition, interner()),
        vec![Violation {
            body: None,
            block: None,
            instruction: None,
            kind: ViolationKind::InvalidId {
                arena: "body",
                index: 99
            },
        }]
    );
}

#[test]
fn view_digests_ignore_interner_numbers_but_preserve_literal_type_id_text() {
    let signature = |definition, ty| Signature {
        definition,
        metadata: SignatureMetadata::default(),
        kind: SignatureKind::Const {
            ty,
            generics: vec![].into(),
        },
        fingerprint: ContentHash::default(),
    };
    let first = TypeInterner::new(None);
    let second = TypeInterner::new(None);
    second.intern(ty(TypeKind::Primitive(Primitive::String)));
    let left = first.intern(ty(TypeKind::Primitive(Primitive::U64)));
    let right = second.intern(ty(TypeKind::Primitive(Primitive::U64)));
    assert_ne!(left, right);
    assert_eq!(
        digest(&first, &signature(id("Data"), left)),
        digest(&second, &signature(id("Data"), right)),
    );
    let mut definitions = hirsel_intern::persist::Numbering::default();
    definitions.number(id("Unrelated"));
    let mut symbols = hirsel_intern::persist::Numbering::default();
    symbols.number("unrelated".into());
    let row = signature(id("Data"), left);
    let expected = digest(&first, &row);
    assert_eq!(
        expected,
        hirsel_syntax::with_def_numbering(&mut definitions, || {
            hirsel_intern::persist::with_symbol_numbering(&mut symbols, || digest(&first, &row))
        }),
        "ambient product numbering cannot change V's identity",
    );
    assert_eq!(definitions.ids().len(), 1);
    assert_eq!(symbols.ids().len(), 1);
    let literal = format!("{left:?}");
    assert_ne!(
        digest(&first, &signature(id(&literal), left)),
        digest(&first, &signature(id("TypeId(#0)"), left)),
        "a declaration name is text, never a type-table reference",
    );
}

// V's input: each view is identified by V's own digest, and every verdict is
// reused only while the reads it made still hold.
fn id(name: &str) -> DefId {
    DefId::intern(&path(name))
}
fn reads_view(verdict: &ViewVerdict, name: &str) -> bool {
    verdict
        .reads
        .iter()
        .any(|read| matches!(read, VRead::View(d, _) if *d == id(name)))
}
fn holds(verdict: &ViewVerdict, signatures: &SignatureMap) -> bool {
    verdict
        .reads
        .iter()
        .all(|read| read_holds(read, signatures))
}
fn represent(signatures: &mut SignatureMap, name: &str, kind: TypeKind) {
    let signature = signatures.get_mut(&path(name)).unwrap();
    let root = interner().intern(ty(kind));
    let SignatureKind::Type(d) = &mut signature.kind else {
        unreachable!()
    };
    d.representation = Some(root);
}

#[test]
fn a_private_shape_edit_under_an_unchanged_fingerprint_rechecks_exactly_its_readers() {
    let mut signatures = SignatureMap::new(session());
    let named = |name: &str| TypeKind::Named {
        definition: id(name),
        arguments: vec![].into(),
    };
    for (name, kind) in [
        ("Inner", TypeKind::Primitive(Primitive::U32)),
        ("Hidden", named("Inner")),
        ("Other", TypeKind::Primitive(Primitive::U64)),
    ] {
        signatures.insert(
            id(name),
            declaration(name, kind, vec![Bound::Plain], true, false),
        );
    }
    let mut verdicts = ValidatedSignatures::default();
    let hidden = verdicts.verdict(interner(), id("Hidden"), &signatures);
    let other = verdicts.verdict(interner(), id("Other"), &signatures);
    assert_eq!((&hidden.error, &other.error), (&None, &None));
    assert!(reads_view(&hidden, "Inner") && !reads_view(&other, "Inner"));
    let original = signatures.get(&path("Inner")).unwrap().clone();
    let before = original.fingerprint;
    let digest = signatures.view(id("Inner")).unwrap().digest();
    // A private field edit the producer's fingerprint does not cover.
    represent(
        &mut signatures,
        "Inner",
        TypeKind::Primitive(Primitive::String),
    );
    assert_eq!(signatures.get(&path("Inner")).unwrap().fingerprint, before);
    assert_ne!(signatures.view(id("Inner")).unwrap().digest(), digest);
    assert!(!holds(&hidden, &signatures) && holds(&other, &signatures));
    let again = verdicts.verdict(interner(), id("Hidden"), &signatures);
    assert!(!std::sync::Arc::ptr_eq(&hidden, &again) && reads_view(&again, "Inner"));
    assert!(std::sync::Arc::ptr_eq(
        &other,
        &verdicts.verdict(interner(), id("Other"), &signatures)
    ));
    // Back to the old shape: the first verdict holds again and is reused.
    signatures.insert(id("Inner"), original);
    assert!(std::sync::Arc::ptr_eq(
        &hidden,
        &verdicts.verdict(interner(), id("Hidden"), &signatures)
    ));
}

#[test]
fn malformed_rows_under_an_unchanged_producer_fingerprint_are_checked_again() {
    let mut signatures = SignatureMap::new(session());
    signatures.insert(
        id("Data"),
        declaration(
            "Data",
            TypeKind::Primitive(Primitive::U32),
            vec![],
            false,
            false,
        ),
    );
    let mut verdicts = ValidatedSignatures::default();
    assert_eq!(
        verdicts.verdict(interner(), id("Data"), &signatures).error,
        None
    );
    let fingerprint = signatures.get(&path("Data")).unwrap().fingerprint;
    // The view's rows change under the same producer fingerprint.
    let SignatureKind::Type(d) = &mut signatures.get_mut(&path("Data")).unwrap().kind else {
        unreachable!()
    };
    d.representation = Some(interner().intern(ty(TypeKind::Error)));
    assert_eq!(
        signatures.get(&path("Data")).unwrap().fingerprint,
        fingerprint
    );
    assert_eq!(
        verdicts
            .verdict(interner(), id("Data"), &signatures)
            .error
            .as_ref()
            .map(|v| v.kind.clone()),
        Some(ViolationKind::InvalidSignature(path("Data")))
    );
    // Every body reads the universe's errors, so none of them reuses a pass.
    let f = Fixture::unit();
    let mut universe = f.signatures.clone();
    let mut reads = vec![];
    assert!(
        validate(&tabled(&f.definition), interner(), &universe, &mut reads)
            .violations
            .is_empty()
    );
    universe.insert(id("Data"), signatures.get(&path("Data")).unwrap().clone());
    assert!(!reads.iter().all(|read| read_holds(read, &universe)));
    assert_eq!(
        validate(&tabled(&f.definition), interner(), &universe, &mut vec![])
            .violations
            .iter()
            .map(|v| v.kind.clone())
            .collect::<Vec<_>>(),
        vec![ViolationKind::InvalidSignature(path("Data"))]
    );
}

#[test]
fn a_lookup_going_from_missing_to_present_invalidates() {
    let mut s = declaration(
        "Hidden",
        TypeKind::Named {
            definition: id("Gate"),
            arguments: vec![].into(),
        },
        vec![Bound::Plain],
        true,
        false,
    );
    let SignatureKind::Type(d) = &mut s.kind else {
        unreachable!()
    };
    d.authority = AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
    let mut signatures = SignatureMap::new(session());
    signatures.insert(id("Hidden"), s);
    let mut verdicts = ValidatedSignatures::default();
    let missing = verdicts.verdict(interner(), id("Hidden"), &signatures);
    assert!(missing.reads.contains(&VRead::Missing(id("Gate"))));
    assert!(missing.error.is_some());
    signatures.insert(id("Gate"), capability());
    assert!(!holds(&missing, &signatures));
    let present = verdicts.verdict(interner(), id("Hidden"), &signatures);
    assert!(reads_view(&present, "Gate"));
    assert_eq!(present.error, None);
}

#[test]
fn a_change_in_heap_role_selection_invalidates() {
    let heap = |arena: &str| {
        let mut s = function_signature(arena, vec![], TypeKind::Tuple(vec![].into()));
        s.metadata.roles.heap = Some(HeapRole {
            arena: path(arena),
            operation: path("heap"),
        });
        s
    };
    let mut f = Fixture::unit();
    let mut reads = vec![];
    let _ = arena_slot(&mut f, Storage::Local);
    let _ = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut reads,
    );
    assert!(reads.contains(&VRead::FirstHeap(Some(id("Arena")))));
    assert!(reads_view(
        &ViewVerdict {
            error: None,
            reads: reads.clone()
        },
        "Arena"
    ));
    // A role earlier in path order is selected instead.
    f.signatures.insert(id("Aaa"), heap("Aaa"));
    assert!(!reads.iter().all(|read| read_holds(read, &f.signatures)));
    // Without any role, the selection is a read of absence.
    let mut none = SignatureMap::new(session());
    assert!(read_holds(&VRead::FirstHeap(None), &none));
    none.insert(id("Zzz"), heap("Zzz"));
    assert!(!read_holds(&VRead::FirstHeap(None), &none));
}

#[test]
fn restored_success_rechecks_private_shape_reads_under_unchanged_fingerprints() {
    use hirsel_intern::work::{self, Counter};
    let mut signatures = SignatureMap::new(session());
    signatures.insert(
        id("Inner"),
        declaration(
            "Inner",
            TypeKind::Primitive(Primitive::U32),
            vec![Bound::Plain],
            true,
            false,
        ),
    );
    signatures.insert(
        id("Hidden"),
        declaration(
            "Hidden",
            TypeKind::Named {
                definition: id("Inner"),
                arguments: vec![].into(),
            },
            vec![Bound::Plain],
            true,
            false,
        ),
    );
    let cache = std::sync::Arc::new(hirsel_validate::VerifierCache::default());
    let policy = hirsel_syntax::ContentHash::of_bytes(b"view-receipt-context");
    let mut original = ValidatedSignatures::with_cache(Some(cache.clone()), policy);
    let verdict = original.verdict(interner(), id("Hidden"), &signatures);
    assert!(verdict.error.is_none() && reads_view(&verdict, "Inner"));
    let before = work::read(Counter::ValidateSignatureRows);
    let mut next = ValidatedSignatures::with_cache(Some(cache), policy);
    let reused = next.verdict(interner(), id("Hidden"), &signatures);
    assert_eq!(*reused, *verdict);
    assert_eq!(work::read(Counter::ValidateSignatureRows), before);
    let fingerprint = signatures.get(&path("Inner")).unwrap().fingerprint;
    represent(
        &mut signatures,
        "Inner",
        TypeKind::Primitive(Primitive::String),
    );
    assert_eq!(
        signatures.get(&path("Inner")).unwrap().fingerprint,
        fingerprint
    );
    let recomputed = next.verdict(interner(), id("Hidden"), &signatures);
    assert!(!std::sync::Arc::ptr_eq(&reused, &recomputed));
    assert_eq!(work::read(Counter::ValidateSignatureRows), before + 1);
    // Hidden's own declared nominal bounds can still hold. Its newly recorded
    // view receipt must match a fresh check, and the malformed Inner must make
    // the enclosing universe fail independently.
    assert_eq!(
        *recomputed,
        check_view(
            interner(),
            id("Hidden"),
            signatures.get(id("Hidden")).unwrap(),
            &signatures
        )
    );
    let universe = next.universe(
        interner(),
        signatures.keys().collect::<Vec<_>>(),
        &signatures,
    );
    assert!(
        universe
            .errors
            .iter()
            .any(|v| v.kind == ViolationKind::InvalidSignature(path("Inner")))
    );
}
