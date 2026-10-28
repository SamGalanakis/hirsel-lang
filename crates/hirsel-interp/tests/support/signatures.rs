//! Owned declaration snapshots for hand-lowered tests, independent of providers.
//! Function prototypes are authored by the fixture's bodies/call operands, not
//! queried from executable library code. Runtime nominal facts select resource,
//! distinct and finalizer declarations; constructor fields provide their layout.
use super::*;
use hirsel_interp::aggregate::RuntimeTypes;
use hirsel_syntax::PassingMode;
use std::collections::BTreeMap;

fn snapshot(definition: DefPath, kind: SignatureKind) -> Signature {
    Signature {
        definition: DefId::intern(&definition),
        kind,
        metadata: SignatureMetadata::default(),
        fingerprint: ContentHash::default(),
    }
}
fn convention(usage: UseKind) -> PassingMode {
    match usage {
        UseKind::Take => PassingMode::Take,
        UseKind::Mut => PassingMode::Mut,
        _ => PassingMode::Borrow,
    }
}
fn callable(parameters: Vec<Parameter>, result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters.into(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::Known(vec![].into()),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }
}
fn parameter(passing: PassingMode, ty: TypeId) -> Parameter {
    Parameter {
        name: None,
        label: None,
        passing,
        ty,
    }
}
pub fn fixture_signatures(
    definitions: &[Definition],
    runtime: &RuntimeTypes,
) -> BTreeMap<DefPath, Signature> {
    let mut signatures = BTreeMap::new();
    let mut conventions = BTreeMap::new();
    for definition in definitions {
        for body in &definition.bodies {
            for block in &body.blocks {
                if let Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    arguments,
                    ..
                } = &block.terminator
                {
                    let mut arguments = arguments.iter().collect::<Vec<_>>();
                    arguments.sort_by_key(|a| a.parameter.0);
                    conventions.insert(
                        path.clone(),
                        arguments
                            .iter()
                            .map(|a| convention(a.value.usage))
                            .collect::<Vec<_>>(),
                    );
                }
            }
        }
    }
    for definition in definitions {
        let body = &definition.bodies[definition.entry.0 as usize];
        let finalizer = runtime.nominals.iter().any(|n| {
            n.finalizer
                .as_ref()
                .is_some_and(|(p, _)| p == definition.path.path())
        });
        let parameters = body
            .parameters
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let passing = if finalizer {
                    PassingMode::Borrow
                } else {
                    conventions
                        .get(definition.path.path())
                        .and_then(|c| c.get(i))
                        .copied()
                        .unwrap_or(PassingMode::Take)
                };
                parameter(passing, body.locals[l.0 as usize].ty)
            })
            .collect();
        signatures.insert(
            definition.path.path().clone(),
            snapshot(
                definition.path.path().clone(),
                SignatureKind::Function(callable(parameters, body.result)),
            ),
        );
    }
    for definition in definitions {
        for body in &definition.bodies {
            for block in &body.blocks {
                if let Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    arguments,
                    returns: Some(CallReturn { destination, .. }),
                    ..
                } = &block.terminator
                {
                    signatures.entry(path.clone()).or_insert_with(|| {
                        let mut arguments = arguments.iter().collect::<Vec<_>>();
                        arguments.sort_by_key(|a| a.parameter.0);
                        let parameters = arguments
                            .iter()
                            .map(|a| {
                                parameter(
                                    convention(a.value.usage),
                                    body.places[a.value.place.0 as usize].ty,
                                )
                            })
                            .collect();
                        snapshot(
                            path.clone(),
                            SignatureKind::Function(callable(
                                parameters,
                                body.places[destination.0 as usize].ty,
                            )),
                        )
                    });
                }
                for instruction in &block.instructions {
                    let InstructionKind::Assign { destination, value } = instruction.kind else {
                        continue;
                    };
                    let Rvalue::Construct {
                        constructor,
                        fields,
                        ..
                    } = &body.rvalues[value.0 as usize]
                    else {
                        continue;
                    };
                    let path = match constructor {
                        Constructor::Record(p) => p,
                        Constructor::Variant { definition, .. } => definition,
                        _ => continue,
                    };
                    if signatures.contains_key(path) {
                        continue;
                    }
                    let types = session();
                    let destination = body.places[destination.0 as usize].ty;
                    let representation = if !matches!(
                        types.get(destination).unwrap().kind,
                        TypeKind::Named { .. }
                    ) {
                        destination
                    } else {
                        let mut fields = fields.iter().collect::<Vec<_>>();
                        fields.sort_by_key(|f| f.field);
                        let kind = match constructor {
                            Constructor::Variant { variant, .. } => TypeKind::Variant(
                                (0..=*variant)
                                    .map(|i| Variant {
                                        name: format!("Case{i}").as_str().into(),
                                        fields: if i == *variant {
                                            fields
                                                .iter()
                                                .map(|f| {
                                                    (None, body.places[f.value.place.0 as usize].ty)
                                                })
                                                .collect()
                                        } else {
                                            vec![].into()
                                        },
                                    })
                                    .collect(),
                            ),
                            _ => TypeKind::Record(
                                fields
                                    .iter()
                                    .map(|f| Field {
                                        growable: false,
                                        name: format!("field{}", f.field).as_str().into(),
                                        ty: body.places[f.value.place.0 as usize].ty,
                                    })
                                    .collect(),
                            ),
                        };
                        types.intern(ty(kind))
                    };
                    let nominal = runtime.get(path);
                    let resource = nominal.is_some_and(|n| n.resource);
                    let mut bounds = if resource {
                        vec![]
                    } else {
                        vec![Bound::Copyable]
                    };
                    if !resource && !fixture_needs_release(&types, representation) {
                        bounds.push(Bound::Plain);
                    }
                    let declaration = TypeDefinition {
                        loop_source: None,
                        format: None,
                        destructor_allocation: Allocation::MayAllocate,
                        generics: vec![],
                        representation: Some(representation),
                        bounds,
                        authority: AuthoritySummary::Known(vec![].into()),
                        capability: None,
                        implementation_authority: None,
                        serializable: false,
                        serializable_check: None,
                        layout: Layout::default(),
                        distinct: nominal.is_some_and(|n| n.distinct),
                        resource,
                        drop: nominal.and_then(|n| n.finalizer.as_ref().map(|(p, _)| p.clone())),
                        fact: None,
                        captures: None,
                    };
                    signatures.insert(
                        path.clone(),
                        snapshot(path.clone(), SignatureKind::Type(Box::new(declaration))),
                    );
                }
            }
        }
    }
    // Match fixtures supply nominal layouts even when their parameters were
    // authored structurally and no constructor executes in the body.
    for definition in definitions {
        for body in &definition.bodies {
            for pattern in body.matches.iter().flat_map(|table| &table.patterns) {
                let owner = match &pattern.kind {
                    MatchPatternKind::Record { definition, .. }
                    | MatchPatternKind::Variant { definition, .. } => definition,
                    _ => continue,
                };
                if signatures.contains_key(owner) {
                    continue;
                }
                let mut repr = body.places[pattern.place.0 as usize].ty;
                if let TypeKind::Builtin {
                    kind: BuiltinType::Packed,
                    arguments,
                } = &session().row(repr).kind
                {
                    let [GenericArgument::Type(payload)] = arguments.as_slice() else {
                        continue;
                    };
                    repr = *payload;
                }
                if !matches!(
                    session().row(repr).kind,
                    TypeKind::Record(_) | TypeKind::Variant(_)
                ) {
                    continue;
                }
                signatures.insert(
                    owner.clone(),
                    snapshot(
                        owner.clone(),
                        SignatureKind::Type(Box::new(TypeDefinition {
                            loop_source: None,
                            format: None,
                            destructor_allocation: Allocation::MayAllocate,
                            generics: vec![],
                            representation: Some(repr),
                            bounds: vec![Bound::Copyable],
                            authority: AuthoritySummary::Known(vec![].into()),
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
                        })),
                    ),
                );
            }
        }
    }
    // Acquisition fixtures carry the canonical Result/GrantError declarations,
    // independently of the executable Library dispatcher.
    for (name, generic) in [("GrantError", false), ("Result", true)] {
        let types = session();
        let (generics, representation, bounds, authority) = if generic {
            let t = types.intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let e = types.intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(1),
            ))));
            let representation = types.intern(ty(TypeKind::Variant(
                vec![
                    Variant {
                        name: "Ok".into(),
                        fields: vec![(None, t)].into(),
                    },
                    Variant {
                        name: "Error".into(),
                        fields: vec![(None, e)].into(),
                    },
                ]
                .into(),
            )));
            (
                vec![
                    GenericParameter::Type {
                        name: "T".into(),
                        bounds: vec![].into(),
                    },
                    GenericParameter::Type {
                        name: "E".into(),
                        bounds: vec![].into(),
                    },
                ],
                representation,
                vec![],
                AuthoritySummary::Known(
                    vec![
                        AuthorityTerm::TypeParameter(TypeParameterRef::bound(TypeParamId(0))),
                        AuthorityTerm::TypeParameter(TypeParameterRef::bound(TypeParamId(1))),
                    ]
                    .into(),
                ),
            )
        } else {
            let representation = types.intern(ty(TypeKind::Variant(
                vec![Variant {
                    name: "NotGranted".into(),
                    fields: vec![].into(),
                }]
                .into(),
            )));
            (
                vec![],
                representation,
                vec![
                    Bound::Plain,
                    Bound::Copyable,
                    Bound::Inert,
                    Bound::Sendable,
                    Bound::NoallocDrop,
                ],
                AuthoritySummary::inert(),
            )
        };
        let path = hirsel_syntax::DefPath::core(name);
        signatures.insert(
            path.clone(),
            snapshot(
                path,
                SignatureKind::Type(Box::new(TypeDefinition {
                    loop_source: None,
                    format: None,
                    destructor_allocation: Allocation::NoAlloc,
                    generics,
                    representation: Some(representation),
                    bounds,
                    authority,
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
                })),
            ),
        );
    }
    // Explicit global capability declarations used by the simulated-print tests.
    for name in ["Io", "Stdout"] {
        let path = hirsel_syntax::DefPath::core(name);
        let declaration = TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::MayAllocate,
            generics: vec![],
            representation: None,
            bounds: vec![Bound::Plain, Bound::Copyable],
            authority: AuthoritySummary::Known(
                vec![AuthorityTerm::Capability(path.clone())].into(),
            ),
            capability: Some(CapabilityKind::Global),
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
        };
        signatures
            .entry(path.clone())
            .or_insert_with(|| snapshot(path, SignatureKind::Type(Box::new(declaration))));
    }
    signatures
}
