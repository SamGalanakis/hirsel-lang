//! Hand-built controls for MODE-1 relation composition: a `data` result
//! related to a `Local` view, itself a call result related to `x`, holds the
//! view's loan on `x` and no access loan on the view.
use super::*;

/// How the local `v` passed to the second call is made.
#[derive(Clone, Copy, PartialEq)]
enum View {
    /// `let v = get(x)`: related to `x` through loan 0.
    Related,
    /// `let v = copy x`: an owned copy, bounded by `x` but holding no loan.
    Copied,
}

/// `x: String` (local content), `v` made per `view` in storage `storage`, then
/// `r = get(v)` through the access loan 1 on `v`, with
/// `get(p: String) -> String in p`. Loan `early` ends right after the second
/// call, before `r` is read; the other ends after.
fn composed(view: View, storage: Storage, early: u32) -> (Fixture, u32) {
    let mut f = Fixture::unit();
    let string = TypeKind::Primitive(Primitive::String);
    let localize = |f: &mut Fixture, place: u32| {
        let mut row = interner()
            .get(f.definition.bodies[0].places[place as usize].ty)
            .unwrap()
            .clone();
        row.modes.relation = Relation::environment();
        let t = f.definition.types.push(row);
        let root = f.definition.bodies[0].places[place as usize].root;
        f.body().places[place as usize].ty = t;
        f.body().locals[root.0 as usize].ty = t;
    };
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
    let x = f.slot(string.clone(), Storage::Local);
    localize(&mut f, x);
    f.assign(x, Rvalue::Constant(Constant::String("x".into())), 0);
    let v = f.slot(string.clone(), storage);
    localize(&mut f, v);
    let c = f.cleanup(vec![drop(x)]);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(x),
        mutable: false,
        if_inactive: false,
    });
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(c);
    let via_x = f.through_loan(x, 0);
    let get = || Callee::Definition {
        path: path("get"),
        arguments: vec![],
    };
    let failure = held(&mut f, &[0], vec![drop(x)]);
    let after_v = held(&mut f, &[0], vec![drop(v), drop(x)]);
    let mut second = vec![];
    match view {
        View::Related => {
            f.body().blocks[0].terminator = Terminator::Call {
                callee: get(),
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: operand(via_x, UseKind::Borrow),
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(v),
                    next: BlockId(1),
                }),
                on_failure: CleanupId(failure),
            };
            f.body().blocks.push(block(vec![], ret(0, 0)));
        }
        View::Copied => {
            let rvalue = RvalueId(f.body().rvalues.len() as u32);
            f.body().rvalues.push(Rvalue::Use(copy(via_x)));
            f.body().blocks[0].instructions.push(inst(
                InstructionKind::Assign {
                    destination: PlaceId(v),
                    value: rvalue,
                },
                failure,
            ));
            f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
            f.body().blocks.push(block(vec![], ret(0, 0)));
        }
    }
    second.push(inst(
        InstructionKind::BeginLoan {
            id: LoanId(1),
            place: PlaceId(v),
            mutable: false,
            if_inactive: false,
        },
        after_v,
    ));
    let via_v = f.through_loan(v, 1);
    let r = f.slot(string.clone(), Storage::Temporary);
    localize(&mut f, r);
    let failure = held(&mut f, &[1, 0], vec![drop(v), drop(x)]);
    f.body().blocks[1].instructions = second;
    f.body().blocks[1].terminator = Terminator::Call {
        callee: get(),
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(via_v, UseKind::Borrow),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(r),
            next: BlockId(2),
        }),
        on_failure: CleanupId(failure),
    };
    let late = 1 - early;
    let n = f.slot(string.clone(), Storage::Temporary);
    localize(&mut f, n);
    let after_early = held(&mut f, &[late], vec![drop(r), drop(v), drop(x)]);
    let after_read = held(&mut f, &[late], vec![drop(n), drop(r), drop(v), drop(x)]);
    let after_late = f.cleanup(vec![drop(n), drop(r), drop(v), drop(x)]);
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
    let mut signature =
        function_signature("get", vec![(PassingMode::Borrow, string.clone())], string);
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    let mut result = interner().get(fun.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    fun.result = interner().intern(result);
    f.signatures.insert(id("get"), signature);
    (f, r)
}

#[test]
fn a_data_result_related_to_a_local_view_holds_the_views_loan() {
    // The access loan on the view may end while the result lives...
    let (f, _) = composed(View::Related, Storage::Local, 1);
    f.valid();
    // ...but the view's own loan on `x` may not: the result reaches `x`.
    let (f, r) = composed(View::Related, Storage::Local, 0);
    f.broken(vec![ViolationKind::ExpiredLoan {
        loan: LoanId(0),
        place: PlaceId(r),
    }]);
}

#[test]
fn a_temporary_or_copied_root_lends_only_itself() {
    // Only a named Local composes; a temporary keeps the access loan.
    let (f, r) = composed(View::Related, Storage::Temporary, 1);
    assert!(
        f.kinds().contains(&ViolationKind::ExpiredLoan {
            loan: LoanId(1),
            place: PlaceId(r),
        }),
        "{:?}",
        f.kinds()
    );
    // An owned copy holds no loan covering its source bound on `x`.
    let (f, r) = composed(View::Copied, Storage::Local, 1);
    assert!(
        f.kinds().contains(&ViolationKind::ExpiredLoan {
            loan: LoanId(1),
            place: PlaceId(r),
        }),
        "{:?}",
        f.kinds()
    );
}
