//! Independent TIR actuals: no source diagnostics or producer proof receipts.
use crate::support::*;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn scalar(p: Primitive) -> TypeId {
    interner().intern(ty(TypeKind::Primitive(p)))
}
fn record(part: TypeId) -> TypeId {
    interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "field".into(),
            ty: part,
            growable: false,
        }]
        .into(),
    )))
}
fn variant(part: TypeId) -> TypeId {
    interner().intern(ty(TypeKind::Variant(
        vec![
            Variant {
                name: "Present".into(),
                fields: vec![(None, part)].into(),
            },
            Variant {
                name: "Absent".into(),
                fields: vec![].into(),
            },
        ]
        .into(),
    )))
}
// Explicit expected truth table, independent of the source model's recursion.
fn actuals() -> Vec<(TypeId, bool, bool)> {
    let integer = scalar(Primitive::U32);
    let float = scalar(Primitive::F64);
    let tuple = |parts: Vec<TypeId>| interner().intern(ty(TypeKind::Tuple(parts.into())));
    let callable = interner().intern(ty(TypeKind::Function(fn_type(vec![], tuple(vec![])))));
    vec![
        (integer, true, true),
        (float, true, false),
        (tuple(vec![integer, record(float)]), true, false),
        (record(tuple(vec![integer, integer])), true, true),
        (variant(integer), true, true),
        (record(variant(callable)), false, false),
    ]
}
fn parameters(bound: Bound) -> Vec<GenericParameter> {
    vec![
        GenericParameter::Size { name: "n".into() },
        GenericParameter::Type {
            name: "T".into(),
            bounds: vec![bound].into(),
        },
    ]
}

// The actual is deliberately absent from the value signature. This isolates
// actual-bound checking from argument compatibility and flow/cleanup evidence.
fn fixture(actual: GenericArgument, bound: Bound, reference: bool) -> Fixture {
    let mut f = Fixture::unit();
    let mut signature = function_signature("bounded", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(header) = &mut signature.kind else {
        unreachable!()
    };
    header.generics = parameters(bound).into();
    f.signatures.insert(id("bounded"), signature);
    if let GenericArgument::Type(t) = actual {
        f.definition.types.adopt(t);
    }
    let arguments = vec![GenericArgument::Size(Size::Constant(2)), actual];
    if reference {
        let unit = f.body().result;
        let destination = f.slot(
            TypeKind::Function(fn_type(vec![], unit)),
            Storage::Temporary,
        );
        f.assign(
            destination,
            Rvalue::FunctionReference {
                definition: path("bounded"),
                arguments,
            },
            0,
        );
        let cleanup = f.cleanup(vec![drop(destination)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
    } else {
        let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("bounded"),
                arguments,
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(vec![], ret(result, 0)));
    }
    f
}
fn refusal(f: &Fixture, entity: &'static str) {
    assert_eq!(f.kinds(), vec![ViolationKind::TypeMismatch { entity }]);
}

#[test]
fn generic_bound_22_independent_actual_matrix() {
    let mut accepted = 0;
    let mut rejected = 0;
    for (seed, (actual, data, key_bound)) in actuals().into_iter().enumerate() {
        for key in [false, true] {
            let bound = if key { Bound::Key } else { Bound::Data };
            for reference in [false, true] {
                let f = fixture(GenericArgument::Type(actual), bound, reference);
                let expected = if key { key_bound } else { data };
                if expected {
                    f.valid();
                    accepted += 1;
                } else {
                    let entity = if reference {
                        "function reference bound"
                    } else {
                        "generic bound"
                    };
                    assert_eq!(
                        f.kinds(),
                        vec![ViolationKind::TypeMismatch { entity }],
                        "seed={seed} {bound:?} reference={reference}"
                    );
                    rejected += 1;
                }
            }
        }
    }
    assert_eq!((accepted, rejected), (16, 8));
    eprintln!(
        "generic_bound_22: 24 independent TIR cells: {accepted} accepts, {rejected} precise bound refusals"
    );
}

#[test]
fn generic_bound_22_changed_actual_requirement_and_kind() {
    for reference in [false, true] {
        let integer = scalar(Primitive::U32);
        fixture(GenericArgument::Type(integer), Bound::Key, reference).valid();
        let float = scalar(Primitive::F64);
        fixture(GenericArgument::Type(float), Bound::Data, reference).valid();
        // Change only the actual, or only its required bound.
        refusal(
            &fixture(GenericArgument::Type(float), Bound::Key, reference),
            if reference {
                "function reference bound"
            } else {
                "generic bound"
            },
        );
        refusal(
            &fixture(
                GenericArgument::Size(Size::Constant(3)),
                Bound::Data,
                reference,
            ),
            if reference {
                "function reference generic kind"
            } else {
                "generic argument kind"
            },
        );
    }
}

fn owner_fixture(reference: bool, ordinal: u32, foreign_owner: bool) -> Fixture {
    let scope = if foreign_owner {
        scoped(NameSite::Statement(hirsel_syntax::StmtId(99)))
    } else {
        scoped(NameSite::Item)
    };
    let actual = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::body(
        scope,
        TypeParamId(ordinal),
    ))));
    let mut f = fixture(GenericArgument::Type(actual), Bound::Key, reference);
    owner_sig(&mut f, &[]);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(id("main")).unwrap().kind else {
        unreachable!()
    };
    owner.generics = vec![
        GenericParameter::Size { name: "n".into() },
        GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Key].into(),
        },
        GenericParameter::Type {
            name: "U".into(),
            bounds: vec![Bound::Data].into(),
        },
    ]
    .into();
    f
}
#[test]
fn generic_bound_22_binder_owner_and_mixed_ordinals() {
    for reference in [false, true] {
        owner_fixture(reference, 1, false).valid();
        refusal(
            &owner_fixture(reference, 2, false),
            if reference {
                "function reference bound"
            } else {
                "generic bound"
            },
        );
        let foreign = owner_fixture(reference, 1, true).kinds();
        assert!(
            foreign.contains(&ViolationKind::InvalidTypeGraph),
            "{foreign:?}"
        );
        let size_as_type = owner_fixture(reference, 0, false).kinds();
        assert!(
            size_as_type.contains(&ViolationKind::InvalidTypeGraph),
            "{size_as_type:?}"
        );
    }
}

#[test]
fn generic_bound_22_unbounded_actuals_still_require_exact_declarations() {
    for reference in [false, true] {
        for (ordinal, foreign_owner) in [(1, true), (0, false)] {
            let mut f = owner_fixture(reference, ordinal, foreign_owner);
            let SignatureKind::Function(callee) =
                &mut f.signatures.get_mut(id("bounded")).unwrap().kind
            else {
                unreachable!()
            };
            let GenericParameter::Type { bounds, .. } = &mut callee.generics[1] else {
                unreachable!()
            };
            *bounds = vec![].into();
            let kinds = f.kinds();
            assert!(
                kinds.contains(&ViolationKind::InvalidTypeGraph),
                "reference={reference} ordinal={ordinal} foreign={foreign_owner}: {kinds:?}"
            );
        }
    }
}

// Both declarations exist and are in scope. Changing only the owner selects
// T:data in the inner binder instead of T:key in the outer binder, at the
// same mixed type/size ordinal and with exactly the same written name.
fn lexical_owner_fixture(reference: bool, inner: bool) -> Fixture {
    let child_scope = scoped(NameSite::Statement(hirsel_syntax::StmtId(1)));
    let mut child = owner_fixture(reference, 1, false);
    child.body().parameter_scope = child_scope;
    if inner {
        let actual = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::body(
            child_scope,
            TypeParamId(1),
        ))));
        child.definition.types.adopt(actual);
        let arguments = if reference {
            let Rvalue::FunctionReference { arguments, .. } = &mut child.body().rvalues[1] else {
                unreachable!()
            };
            arguments
        } else {
            let Terminator::Call {
                callee: Callee::Definition { arguments, .. },
                ..
            } = &mut child.body().blocks[0].terminator
            else {
                unreachable!()
            };
            arguments
        };
        arguments[1] = GenericArgument::Type(actual);
    }
    let mut f = Fixture::unit();
    f.signatures = child.signatures.clone();
    for &t in child.definition.types.iter() {
        f.definition.types.adopt(t);
    }
    f.definition.bodies.push(child.definition.bodies.remove(0));
    let unit = f.body().result;
    let mut header = fn_type(vec![], unit);
    header.generics = parameters(Bound::Data).into();
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
    f
}
#[test]
fn generic_bound_22_existing_lexical_owner_selects_its_own_bound() {
    for reference in [false, true] {
        lexical_owner_fixture(reference, false).valid();
        refusal(
            &lexical_owner_fixture(reference, true),
            if reference {
                "function reference bound"
            } else {
                "generic bound"
            },
        );
    }
}
