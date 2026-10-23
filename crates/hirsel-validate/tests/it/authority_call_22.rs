//! Independently built CAP/uses witnesses; source checking supplies no receipts.
//! Adapt finite subset vectors from Rust alloc btree/set/tests.rs:284-304
//! (f6c1107f, MIT). Koka Type/Unify.hs:374-404 (198fc3bb, Apache-2.0)
//! supplies normalization-before-comparison discipline; no handler/row rules.
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn cap(name: &str) -> TypeKind {
    TypeKind::Named {
        definition: id(name),
        arguments: vec![].into(),
    }
}
fn selected(ordinals: &[u32]) -> AuthoritySummary {
    AuthoritySummary::Known(
        ordinals
            .iter()
            .map(|&p| AuthorityTerm::Parameter(ParameterRef::bound(ParamId(p))))
            .collect(),
    )
}
fn capabilities(names: &[&str]) -> AuthoritySummary {
    AuthoritySummary::Known(
        names
            .iter()
            .map(|n| AuthorityTerm::Capability(path(n)))
            .collect(),
    )
}

fn invocation(indirect: bool, reversed: bool, allowed: AuthoritySummary) -> Fixture {
    let mut f = Fixture::unit();
    for name in ["Dir", "Net"] {
        f.signatures.insert(id(name), capability_signature(name));
    }
    let d = f.slot(cap("Dir"), Storage::Parameter);
    let n = f.slot(cap("Net"), Storage::Parameter);
    f.body().parameters = vec![LocalId(d), LocalId(n)];
    owner_signature(&mut f, PassingMode::Borrow);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    owner.uses = Uses::Only(allowed);
    let mut signature = function_signature(
        "effect",
        vec![
            (PassingMode::Borrow, cap("Dir")),
            (PassingMode::Borrow, cap("Net")),
        ],
        TypeKind::Tuple(vec![].into()),
    );
    let SignatureKind::Function(effect) = &mut signature.kind else {
        unreachable!()
    };
    effect.authority = selected(&[0, 1]);
    effect.uses = Uses::Only(selected(&[0, 1]));
    let function = effect.clone();
    f.signatures.insert(id("effect"), signature);
    let mut failure = 0;
    let callee = if indirect {
        let holder = f.slot(TypeKind::Function(function), Storage::Local);
        f.assign(
            holder,
            Rvalue::FunctionReference {
                definition: path("effect"),
                arguments: vec![],
            },
            0,
        );
        failure = f.cleanup(vec![drop(holder)]);
        Callee::Indirect {
            forward_caller: false,
            value: copy(holder),
            arguments: vec![],
        }
    } else {
        Callee::Definition {
            path: path("effect"),
            arguments: vec![],
        }
    };
    let mut arguments = vec![
        CallArgument {
            parameter: ParamId(0),
            value: operand(d, UseKind::Borrow),
        },
        CallArgument {
            parameter: ParamId(1),
            value: operand(n, UseKind::Borrow),
        },
    ];
    if reversed {
        arguments.reverse();
    }
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee,
        arguments,
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure),
    };
    f.body().blocks.push(block(vec![], ret(result, failure)));
    f
}

#[test]
fn authority_call_22_direct_positive() {
    invocation(false, false, capabilities(&["Dir", "Net"])).valid();
}
#[test]
fn authority_call_22_stored_positive() {
    invocation(true, false, capabilities(&["Dir", "Net"])).valid();
}
#[test]
fn authority_call_22_named_order_positive() {
    for indirect in [false, true] {
        invocation(indirect, true, capabilities(&["Dir", "Net"])).valid();
    }
}

fn ceiling_mutations(indirect: bool) {
    let mut missed = vec![];
    for (name, allowed) in [
        ("selector", selected(&[0])),
        ("unrelated-capability", capabilities(&["Dir"])),
        ("empty", AuthoritySummary::inert()),
        ("unknown", AuthoritySummary::Unknown),
    ] {
        let f = invocation(indirect, false, allowed);
        let kinds = f.kinds();
        if kinds
            != vec![ViolationKind::TypeMismatch {
                entity: "body uses",
            }]
        {
            missed.push((name, kinds));
        }
    }
    assert!(
        missed.is_empty(),
        "indirect={indirect}, missing body uses refusal: {missed:?}"
    );
}
#[test]
fn authority_call_22_direct_ceiling_mutations() {
    ceiling_mutations(false);
}
#[test]
fn authority_call_22_stored_ceiling_mutations() {
    ceiling_mutations(true);
}

fn generic(mut f: Fixture) -> Fixture {
    let actuals: Vec<_> = ["Dir", "Net"]
        .iter()
        .map(|name| GenericArgument::Type(interner().intern(ty(cap(name)))))
        .collect();
    let SignatureKind::Function(effect) = &mut f.signatures.get_mut(&path("effect")).unwrap().kind
    else {
        unreachable!()
    };
    effect.generics = ["A", "B"]
        .iter()
        .map(|name| GenericParameter::Type {
            name: (*name).into(),
            bounds: vec![].into(),
        })
        .collect();
    for (ordinal, p) in effect.parameters.iter_mut().enumerate() {
        p.ty = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(ordinal as u32),
        ))));
    }
    match &mut f.body().blocks[0].terminator {
        Terminator::Call {
            callee: Callee::Definition { arguments, .. },
            ..
        } => *arguments = actuals,
        Terminator::Call {
            callee: Callee::Indirect { .. },
            ..
        } => {
            for value in &mut f.body().rvalues {
                if let Rvalue::FunctionReference { arguments, .. } = value {
                    *arguments = actuals.clone();
                }
            }
        }
        _ => unreachable!(),
    }
    f
}

#[test]
fn authority_call_22_generic_actuals_preserve_selected_authority() {
    for indirect in [false, true] {
        generic(invocation(indirect, false, capabilities(&["Dir", "Net"]))).valid();
        let f = generic(invocation(indirect, false, selected(&[0])));
        f.broken(vec![ViolationKind::TypeMismatch {
            entity: "body uses",
        }]);
    }
}

fn record(indirect: bool, allowed: AuthoritySummary) -> Fixture {
    let mut f = invocation(indirect, false, allowed);
    let fields = [
        ("tag", TypeKind::Primitive(Primitive::U64)),
        ("directory", cap("Dir")),
        ("network", cap("Net")),
    ]
    .into_iter()
    .map(|(name, kind)| Field {
        growable: false,
        name: name.into(),
        ty: interner().intern(ty(kind)),
    })
    .collect();
    let record = interner().intern(ty(TypeKind::Record(fields)));
    f.definition.types.adopt(record);
    f.body().locals[1].ty = record;
    f.body().places[1].ty = record;
    // Keep the second parameter as a valid, unrelated inert field/control.
    let scalar = interner().intern(ty(TypeKind::Primitive(Primitive::U64)));
    f.definition.types.adopt(scalar);
    f.body().locals[2].ty = scalar;
    f.body().places[2].ty = scalar;
    let SignatureKind::Function(effect) = &mut f.signatures.get_mut(&path("effect")).unwrap().kind
    else {
        unreachable!()
    };
    effect.parameters[0].ty = record;
    effect.parameters[1].ty = scalar;
    effect.authority = selected(&[0]);
    effect.uses = Uses::Only(selected(&[0]));
    let function = effect.clone();
    if indirect {
        let function = interner().intern(ty(TypeKind::Function(function)));
        f.definition.types.adopt(function);
        f.body().locals[3].ty = function;
        f.body().places[3].ty = function;
    }
    let old = f.signatures.get(&path("main")).unwrap().clone();
    let SignatureKind::Function(old) = old.kind else {
        unreachable!()
    };
    owner_signature(&mut f, PassingMode::Borrow);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    owner.uses = old.uses;
    f
}

#[test]
fn authority_call_22_record_fields_keep_both_capabilities() {
    for indirect in [false, true] {
        record(indirect, capabilities(&["Dir", "Net"])).valid();
        for allowed in [
            capabilities(&["Dir"]),
            capabilities(&["Net"]),
            selected(&[1]),
        ] {
            record(indirect, allowed).broken(vec![ViolationKind::TypeMismatch {
                entity: "body uses",
            }]);
        }
    }
}

#[test]
fn authority_call_22_stored_summaries_cannot_erase_the_declaration() {
    for mutate_uses in [false, true] {
        for summary in [AuthoritySummary::inert(), AuthoritySummary::Unknown] {
            let mut f = invocation(true, false, capabilities(&["Dir", "Net"]));
            let original = f.body().places[3].ty;
            let mut row = interner().get(original).unwrap().clone();
            let TypeKind::Function(function) = &mut row.kind else {
                unreachable!()
            };
            if mutate_uses {
                function.uses = Uses::Only(summary);
            } else {
                function.authority = summary;
            }
            let forged = f.definition.types.push(row);
            f.body().locals[3].ty = forged;
            f.body().places[3].ty = forged;
            let kinds = f.kinds();
            assert!(
                kinds.iter().all(|kind| matches!(
                    kind,
                    ViolationKind::TypeMismatch {
                        entity: "function reference" | "body uses"
                    }
                )),
                "{kinds:?}"
            );
            assert!(!kinds.is_empty(), "mutate_uses={mutate_uses}");
        }
    }
}

#[test]
fn authority_call_22_unselected_inert_generic_call_is_valid() {
    let mut f = generic(invocation(false, false, AuthoritySummary::inert()));
    let SignatureKind::Function(effect) = &mut f.signatures.get_mut(&path("effect")).unwrap().kind
    else {
        unreachable!()
    };
    effect.authority = AuthoritySummary::inert();
    effect.uses = Uses::Only(AuthoritySummary::inert());
    f.valid();
}

#[test]
fn authority_call_22_actual_receipts_follow_parameter_ids() {
    for indirect in [false, true] {
        for reversed in [false, true] {
            for (allowed, rejected) in [
                (capabilities(&["Dir"]), false),
                (capabilities(&["Net"]), true),
            ] {
                let mut f = generic(invocation(indirect, reversed, allowed));
                let SignatureKind::Function(effect) =
                    &mut f.signatures.get_mut(&path("effect")).unwrap().kind
                else {
                    unreachable!()
                };
                effect.authority = AuthoritySummary::inert();
                effect.uses = Uses::Only(selected(&[0]));
                if indirect {
                    let original = f.body().places[3].ty;
                    let mut row = interner().get(original).unwrap().clone();
                    let TypeKind::Function(function) = &mut row.kind else {
                        unreachable!()
                    };
                    function.authority = selected(&[0]);
                    function.uses = Uses::Only(selected(&[0]));
                    let ty = f.definition.types.push(row);
                    f.body().locals[3].ty = ty;
                    f.body().places[3].ty = ty;
                }
                if rejected {
                    f.broken(vec![ViolationKind::TypeMismatch {
                        entity: "body uses",
                    }])
                } else {
                    f.valid()
                }
            }
        }
    }
}
