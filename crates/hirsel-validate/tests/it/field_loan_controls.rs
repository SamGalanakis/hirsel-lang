//! Hand-built controls for MODE-1 field-path containment at a call (#77): a
//! result related only to field path F of a parameter depends on a read loan
//! on the argument place extended by F, not on the whole argument.
use super::*;

/// `p: (String, String)`, borrowed by `get(p) -> String in p.0` through the
/// whole-argument loan 0 while loan 1 reads `p.0`. Loan `early` ends right
/// after the call; the other ends after the result is read. When `projected`,
/// `p` is `((String, String),)` and the argument is the place `p.0`, so the
/// field loan is on `p.0.0`.
fn field_call(early: u32, projected: bool) -> (Fixture, u32) {
    let mut f = Fixture::unit();
    let string = TypeKind::Primitive(Primitive::String);
    let a = f.slot(string.clone(), Storage::Temporary);
    let b = f.slot(string.clone(), Storage::Temporary);
    f.assign(a, Rvalue::Constant(Constant::String("a".into())), 0);
    let c = f.cleanup(vec![drop(a)]);
    f.assign(b, Rvalue::Constant(Constant::String("b".into())), c);
    let s = f.body().places[a as usize].ty;
    let pair = f.slot(TypeKind::Tuple(vec![s, s].into()), Storage::Local);
    let p = if projected {
        let pair_ty = f.body().places[pair as usize].ty;
        f.slot(TypeKind::Tuple(vec![pair_ty].into()), Storage::Local)
    } else {
        pair
    };
    let c = f.cleanup(vec![drop(b), drop(a)]);
    f.assign(
        pair,
        Rvalue::Construct {
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
        },
        c,
    );
    if projected {
        let c = f.cleanup(vec![drop(pair)]);
        f.assign(
            p,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![ConstructField {
                    field: 0,
                    value: take(pair),
                }],
                destination: Destination::Inferred,
            },
            c,
        );
    }
    // p holds borrowed content, as a pair of a parameter's views would: it
    // is local storage, and so is the result reading within it (MODE-1).
    let localize = |f: &mut Fixture, place: u32| {
        let mut row = f
            .definition
            .types
            .get(f.definition.bodies[0].places[place as usize].ty)
            .unwrap()
            .clone();
        row.modes.relation = Relation::environment();
        let t = f.definition.types.push(row);
        let root = f.definition.bodies[0].places[place as usize].root;
        f.body().places[place as usize].ty = t;
        f.body().locals[root.0 as usize].ty = t;
        t
    };
    localize(&mut f, p);
    // The argument place: p itself, or its field p.0.
    let prefix: Vec<Projection> = if projected {
        vec![Projection::Field(0)]
    } else {
        vec![]
    };
    let argument = if projected {
        let root = f.body().places[p as usize].root;
        let mut row = f
            .definition
            .types
            .get(f.definition.bodies[0].places[pair as usize].ty)
            .unwrap()
            .clone();
        row.modes.relation = Relation::environment();
        let ty = f.definition.types.push(row);
        let argument = f.body().places.len() as u32;
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root,
            projection: prefix.clone().into(),
            ty,
        });
        argument
    } else {
        p
    };
    let local = {
        let mut row = f.definition.types.get(s).unwrap().clone();
        row.modes.relation = Relation::environment();
        f.definition.types.push(row)
    };
    let root = f.body().places[p as usize].root;
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root,
        projection: prefix
            .iter()
            .cloned()
            .chain([Projection::Field(0)])
            .collect::<Vec<_>>()
            .into(),
        ty: local,
    });
    let held = |f: &mut Fixture, loans: &[u32], rest: Vec<CleanupAction>| {
        let mut actions: Vec<_> = loans
            .iter()
            .map(|l| CleanupAction::EndLoan {
                loan: LoanId(*l),
                if_active: false,
            })
            .collect();
        actions.extend(rest);
        f.cleanup(actions)
    };
    let c = f.cleanup(vec![drop(p)]);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(argument),
        mutable: false,
        if_inactive: false,
    });
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(c);
    let c = held(&mut f, &[0], vec![drop(p)]);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(field),
        mutable: false,
        if_inactive: false,
    });
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(c);
    let via = f.through_loan(argument, 0);
    let r = f.slot(string.clone(), Storage::Temporary);
    localize(&mut f, r);
    let failure = held(&mut f, &[1, 0], vec![drop(p)]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("get"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(via, UseKind::Borrow),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(r),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure),
    };
    let late = 1 - early;
    let n = f.slot(string.clone(), Storage::Temporary);
    localize(&mut f, n);
    let after_early = held(&mut f, &[late], vec![drop(r), drop(p)]);
    let after_read = held(&mut f, &[late], vec![drop(n), drop(r), drop(p)]);
    let after_late = f.cleanup(vec![drop(n), drop(r), drop(p)]);
    let rvalue = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(copy(r)));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(early),
                    if_active: false,
                },
                after_early,
            ),
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(n),
                    value: rvalue,
                },
                after_early,
            ),
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(late),
                    if_active: false,
                },
                after_read,
            ),
        ],
        ret(0, after_late),
    ));
    let mut signature = function_signature(
        "get",
        vec![(PassingMode::Borrow, TypeKind::Tuple(vec![].into()))],
        TypeKind::Primitive(Primitive::String),
    );
    let field_ty = signature.types.push(ty(string));
    let pair = signature
        .types
        .push(ty(TypeKind::Tuple(vec![field_ty, field_ty].into())));
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    fun.parameters[0].ty = pair;
    let mut result = signature.types.get(fun.result).unwrap().clone();
    result.modes.relation = Relation::place(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![FieldId(0)].into(),
    });
    fun.result = signature.types.push(result);
    f.signatures.insert(path("get"), signature);
    (f, r)
}

#[test]
fn a_field_related_result_holds_the_field_loan_and_not_the_whole_argument() {
    // The whole-argument loan may end while the result lives (#77)...
    let (f, _) = field_call(0, false);
    f.valid();
    // ...but the field loan may not: the result reads within p.0.
    let (f, r) = field_call(1, false);
    f.broken(vec![ViolationKind::ExpiredLoan {
        loan: LoanId(1),
        place: PlaceId(r),
    }]);
}

#[test]
fn a_projected_argument_place_extends_to_the_field_loan_the_result_holds() {
    // Passing p.0, the loan on the argument place p.0 may end while the
    // result lives: it holds the loan on p.0.0 (#77, projected argument)...
    let (f, _) = field_call(0, true);
    f.valid();
    // ...and that field loan may not end.
    let (f, r) = field_call(1, true);
    f.broken(vec![ViolationKind::ExpiredLoan {
        loan: LoanId(1),
        place: PlaceId(r),
    }]);
}
