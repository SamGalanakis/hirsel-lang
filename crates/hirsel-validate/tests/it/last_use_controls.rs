//! Hand-built controls for loans that end at their holder's last use (LANG-3):
//! lowering ends a loan after the last use of every
//! holder on each path, where destroying a holder with user-visible
//! destruction is a use, and the checker accepts exactly that.
use super::*;

/// A holder whose installed content is a view of `x` through loan 0.
struct Holder {
    f: Fixture,
    x: u32,
    holder: u32,
}

/// `x: String`; `holder: T` with a String field 0 installed through loan 0.
/// `resource` makes T a resource record (user-visible destruction), else a
/// tuple, which is copyable (DROP-1).
fn holder(resource: bool) -> Holder {
    let mut f = Fixture::unit();
    let x = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    f.assign(x, Rvalue::Constant(Constant::String("x".into())), 0);
    let string = f.body().places[x as usize].ty;
    let kind = if resource {
        let mut signature = function_signature("guard", vec![], TypeKind::Tuple(vec![].into()));
        let field = interner().intern(ty(TypeKind::Primitive(Primitive::String)));
        let representation = interner().intern(ty(TypeKind::Record(
            vec![Field {
                name: "data".into(),
                ty: field,
                growable: false,
            }]
            .into(),
        )));
        signature.kind = SignatureKind::Type(Box::new(TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::NoAlloc,
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
            drop: Some(path("guard_drop")),
            fact: None,
            captures: None,
        }));
        f.signatures.insert(id("guard"), signature);
        TypeKind::Named {
            definition: id("guard"),
            arguments: vec![].into(),
        }
    } else {
        TypeKind::Tuple(vec![string].into())
    };
    let holder = f.slot(kind, Storage::Temporary);
    let owned = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let c1 = f.cleanup(vec![drop(x)]);
    f.assign(owned, Rvalue::Constant(Constant::String("own".into())), c1);
    let c2 = f.cleanup(vec![drop(owned), drop(x)]);
    f.assign(
        holder,
        Rvalue::Construct {
            constructor: if resource {
                Constructor::Record(path("guard"))
            } else {
                Constructor::Tuple
            },
            fields: vec![ConstructField {
                field: 0,
                value: take(owned),
            }],
            destination: Destination::Inferred,
        },
        c2,
    );
    // Install a view of x as field 0: the holder now holds loan 0.
    // A holder of a view is local storage (MODE-1), as lowering types it.
    let local = |f: &mut Fixture, t: TypeId| {
        let mut row = interner().get(t).unwrap().clone();
        row.modes.relation = Relation::environment();
        f.definition.types.push(row)
    };
    let root = f.body().places[holder as usize].root;
    let global = f.body().places[holder as usize].ty;
    let holder_type = local(&mut f, global);
    f.body().places[holder as usize].ty = holder_type;
    f.body().locals[root.0 as usize].ty = holder_type;
    let local_string = local(&mut f, string);
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root,
        projection: vec![Projection::Field(0)].into(),
        ty: local_string,
    });
    let c3 = f.cleanup(vec![drop(holder), drop(x)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(field),
            if_live: false,
        },
        c3,
    ));
    at(
        &mut f,
        InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(x),
            mutable: false,
            if_inactive: false,
        },
        c3,
    );
    let via = f.through_loan(x, 0);
    let c4 = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(holder),
        drop(x),
    ]);
    f.assign(field, Rvalue::Use(operand(via, UseKind::Borrow)), c4);
    Holder { f, x, holder }
}
/// Give a slot the local variant of its type.
fn localize(f: &mut Fixture, place: u32) {
    let global = f.body().places[place as usize].ty;
    let mut row = interner().get(global).unwrap().clone();
    row.modes.relation = Relation::environment();
    let local = f.definition.types.push(row);
    let root = f.body().places[place as usize].root;
    f.body().places[place as usize].ty = local;
    f.body().locals[root.0 as usize].ty = local;
}
fn at(f: &mut Fixture, kind: InstructionKind, cleanup: u32) {
    f.body().blocks[0].instructions.push(inst(kind, cleanup));
}
fn end_loan(f: &mut Fixture, cleanup: u32) {
    at(
        f,
        InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        cleanup,
    );
}
fn expired(f: &Fixture, place: u32) -> bool {
    f.kinds().contains(&ViolationKind::ExpiredLoan {
        loan: LoanId(0),
        place: PlaceId(place),
    })
}

#[test]
fn control_1_a_loan_ended_before_a_later_read_of_its_holder_is_expired() {
    for read_first in [true, false] {
        let Holder { mut f, x, holder } = holder(false);
        let string = f.body().places[x as usize].ty;
        let copied = f.slot(TypeKind::Tuple(vec![string].into()), Storage::Local);
        localize(&mut f, copied);
        let holding = f.cleanup(vec![drop(holder), drop(x)]);
        let reading = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(holder),
            drop(x),
        ]);
        let exit = f.cleanup(vec![drop(holder), drop(copied), drop(x)]);
        if read_first {
            f.assign(copied, Rvalue::Use(copy(holder)), reading);
            end_loan(&mut f, exit);
        } else {
            end_loan(&mut f, holding);
            f.assign(copied, Rvalue::Use(copy(holder)), holding);
        }
        f.body().blocks[0].terminator = ret(0, exit);
        if read_first {
            f.valid();
        } else {
            assert!(expired(&f, holder), "{:?}", f.kinds());
        }
    }
}

/// Branch on `c`: the early-return path ends loan 0 before the holder's
/// cleanup drop; the normal path ends it after the drop.
fn early_return(resource: bool, early_end: bool) -> (Fixture, u32) {
    let Holder { mut f, x, holder } = holder(resource);
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Local);
    f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(c),
        yes: BlockId(1),
        no: BlockId(2),
    };
    let after = f.cleanup(vec![
        drop(c),
        drop(holder),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(x),
    ]);
    let exit = f.cleanup(vec![drop(c), drop(holder), drop(x)]);
    let early = if early_end {
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        )]
    } else {
        vec![]
    };
    f.body().blocks.push(block(
        early,
        if early_end {
            ret(0, exit)
        } else {
            ret(0, after)
        },
    ));
    f.body().blocks.push(block(vec![], ret(0, after)));
    (f, holder)
}

#[test]
fn control_2_a_loan_ended_before_a_resource_holders_early_return_drop_is_expired() {
    let (f, _) = early_return(true, false);
    f.valid();
    let (f, holder) = early_return(true, true);
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::ExpiredLoan {
            loan: LoanId(0),
            place: PlaceId(holder),
        }]
    );
}

#[test]
fn control_3_destroying_a_drop_1_holder_after_its_loan_ended_is_valid() {
    let (f, _) = early_return(false, true);
    f.valid();
}

#[test]
fn control_4_a_write_into_the_source_after_the_last_use_is_valid() {
    for ended in [true, false] {
        let Holder { mut f, x, holder } = holder(false);
        let string = f.body().places[x as usize].ty;
        let copied = f.slot(TypeKind::Tuple(vec![string].into()), Storage::Local);
        localize(&mut f, copied);
        let new = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        let old = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        let active = |f: &mut Fixture, temporaries: Vec<u32>| {
            let mut actions = vec![];
            if !ended {
                actions.push(CleanupAction::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                });
            }
            actions.extend(temporaries.into_iter().map(drop));
            actions.extend([drop(holder), drop(copied), drop(x)]);
            f.cleanup(actions)
        };
        let reading = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(holder),
            drop(x),
        ]);
        f.assign(copied, Rvalue::Use(copy(holder)), reading);
        let after = active(&mut f, vec![]);
        if ended {
            end_loan(&mut f, after);
        }
        f.assign(new, Rvalue::Constant(Constant::String("new".into())), after);
        let replacing = active(&mut f, vec![new]);
        at(
            &mut f,
            InstructionKind::Replace {
                destination: PlaceId(x),
                value: take(new),
                displaced: PlaceId(old),
                if_live: false,
            },
            replacing,
        );
        at(
            &mut f,
            InstructionKind::Drop {
                place: PlaceId(old),
                if_live: false,
            },
            after,
        );
        f.body().blocks[0].terminator = ret(0, after);
        if ended {
            f.valid();
        } else {
            assert!(
                f.kinds()
                    .iter()
                    .any(|k| matches!(k, ViolationKind::LoanConflict { .. })),
                "{:?}",
                f.kinds()
            );
        }
    }
}

/// A loop whose holder is assigned a fresh view of `x` each iteration.
fn reassigned_in_loop(ended: bool) -> Fixture {
    let mut f = Fixture::unit();
    let x = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    f.assign(x, Rvalue::Constant(Constant::String("x".into())), 0);
    let string = f.body().places[x as usize].ty;
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Local);
    f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
    let h = f.slot(TypeKind::Tuple(vec![string].into()), Storage::Local);
    localize(&mut f, h);
    f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
    let via = f.through_loan(x, 0);
    let rvalue = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Construct {
        constructor: Constructor::Tuple,
        fields: vec![ConstructField {
            field: 0,
            value: operand(via, UseKind::Borrow),
        }],
        destination: Destination::Inferred,
    });
    let held = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(c),
        drop(x),
    ]);
    let holding = f.cleanup(vec![
        drop(h),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(c),
        drop(x),
    ]);
    let mut body = vec![
        inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(x),
                mutable: false,
                if_inactive: false,
            },
            0,
        ),
        inst(
            InstructionKind::Assign {
                destination: PlaceId(h),
                value: rvalue,
            },
            held,
        ),
        inst(
            InstructionKind::Drop {
                place: PlaceId(h),
                if_live: false,
            },
            holding,
        ),
    ];
    if ended {
        body.push(inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            held,
        ));
    }
    f.body().blocks.push(block(
        body,
        Terminator::Branch {
            condition: copy(c),
            yes: BlockId(1),
            no: BlockId(2),
        },
    ));
    let exit = f.cleanup(vec![drop(c), drop(x)]);
    let exit_holding = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(c),
        drop(x),
    ]);
    f.body().blocks.push(block(
        vec![],
        ret(0, if ended { exit } else { exit_holding }),
    ));
    f
}

#[test]
fn control_5_a_loop_holder_ends_its_loan_before_the_back_edge() {
    reassigned_in_loop(true).valid();
    assert!(!reassigned_in_loop(false).kinds().is_empty());
}

/// A resource holder declared after its kept source: a named source releases
/// after the holder's drop; a temporary source would release first.
fn kept_source(source: Storage) -> Fixture {
    let Holder { mut f, x, holder } = holder(true);
    let root = f.body().places[holder as usize].root;
    f.body().locals[root.0 as usize].storage = Storage::Local;
    f.body().locals[x as usize].storage = source;
    let exit = f.cleanup(vec![
        drop(holder),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(x),
    ]);
    f.body().blocks[0].terminator = ret(0, exit);
    f
}

#[test]
fn control_6_a_kept_temporary_releases_as_a_name_declared_before_its_holder() {
    kept_source(Storage::Local).valid();
    assert!(!kept_source(Storage::Temporary).kinds().is_empty());
}
