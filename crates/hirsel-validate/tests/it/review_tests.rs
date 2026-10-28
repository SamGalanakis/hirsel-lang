use super::*;

#[test]
fn nested_constructor_siblings_keep_independent_loan_cells() {
    fn localize(f: &mut Fixture, place: u32) -> TypeId {
        let old = f.body().places[place as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
        row.modes.relation = Relation::environment();
        let ty = f.definition.types.push(row);
        let root = f.body().places[place as usize].root;
        f.body().locals[root.0 as usize].ty = ty;
        f.body().places[place as usize].ty = ty;
        ty
    }
    for read_borrowed_sibling in [false, true] {
        let mut f = Fixture::unit();
        let source = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
        f.body().parameters.push(LocalId(source));
        let text = f.body().places[source as usize].ty;
        let number = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Local);
        let number_ty = f.body().places[number as usize].ty;
        f.assign(number, Rvalue::Constant(Constant::Integer(1)), 0);
        let inner = f.slot(
            TypeKind::Tuple(vec![number_ty, text].into()),
            Storage::Local,
        );
        let inner_ty = localize(&mut f, inner);
        let sequence = f.slot(
            TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(inner_ty)].into(),
            },
            Storage::Local,
        );
        let sequence_ty = localize(&mut f, sequence);
        let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
        let outer = f.slot(TypeKind::Tuple(vec![sequence_ty].into()), Storage::Local);
        localize(&mut f, outer);
        // The intermediate observed place exposes the sibling schema without
        // reading the borrowed field. No source checker supplies these paths.
        f.body().places.push(Place {
            root: LocalId(outer),
            projection: vec![Projection::Field(0), Projection::Index(LocalId(index))].into(),
            ty: inner_ty,
            loan: None,
            binding: None,
        });
        let selected = f.body().places.len() as u32;
        let selected_ty = if read_borrowed_sibling {
            let mut row = interner().get(text).unwrap().clone();
            row.modes.relation = Relation::environment();
            f.definition.types.push(row)
        } else {
            number_ty
        };
        f.body().places.push(Place {
            root: LocalId(outer),
            projection: vec![
                Projection::Field(0),
                Projection::Index(LocalId(index)),
                Projection::Field(u32::from(read_borrowed_sibling)),
            ]
            .into(),
            ty: selected_ty,
            loan: None,
            binding: None,
        });
        let borrowed = f.through_loan(source, 0);
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(source),
            mutable: false,
            if_inactive: false,
        });
        let before = f.cleanup(vec![CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        }]);
        f.assign(
            inner,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: take(number),
                    },
                    ConstructField {
                        field: 1,
                        value: operand(borrowed, UseKind::Borrow),
                    },
                ],
                destination: Destination::Inferred,
            },
            before,
        );
        let after_inner = f.cleanup(vec![
            drop(inner),
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        ]);
        f.assign(
            sequence,
            Rvalue::Construct {
                constructor: Constructor::Sequence,
                fields: vec![ConstructField {
                    field: 0,
                    value: take(inner),
                }],
                destination: Destination::Inferred,
            },
            after_inner,
        );
        let after_sequence = f.cleanup(vec![
            drop(sequence),
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        ]);
        f.assign(
            outer,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![ConstructField {
                    field: 0,
                    value: take(sequence),
                }],
                destination: Destination::Inferred,
            },
            after_sequence,
        );
        f.emit(InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        });
        let observed = f.slot(
            if read_borrowed_sibling {
                TypeKind::Primitive(Primitive::String)
            } else {
                TypeKind::Primitive(Primitive::U64)
            },
            Storage::Temporary,
        );
        if read_borrowed_sibling {
            localize(&mut f, observed);
        }
        let after = f.cleanup(vec![drop(outer)]);
        f.assign(observed, Rvalue::Use(copy(selected)), after);
        let mut cleanup = vec![];
        if read_borrowed_sibling {
            cleanup.push(drop(observed));
        }
        cleanup.push(drop(outer));
        let cleanup = f.cleanup(cleanup);
        f.body().blocks[0].terminator = ret(0, cleanup);
        owner_sig(&mut f, &[PassingMode::Borrow]);
        if read_borrowed_sibling {
            assert!(
                f.kinds().iter().any(|kind| matches!(
                    kind,
                    ViolationKind::ExpiredLoan {
                        loan: LoanId(0),
                        ..
                    }
                )),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

fn string_kind() -> TypeKind {
    TypeKind::Primitive(Primitive::String)
}

fn consuming_closure(capture_usage: UseKind, multiplicity: Multiplicity, calls: usize) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let y = f.slot(string_kind(), Storage::Local);
    f.assign(y, Rvalue::Constant(Constant::String("y".into())), 0);
    let sty = f.body().places[y as usize].ty;
    let mut nested = empty_body(&mut f);
    let cap = add_capture(&mut nested, sty);
    if multiplicity == Multiplicity::Once {
        nested.add_cleanup(vec![drop(cap)]);
        nested.blocks[0].terminator = ret(0, 1);
    }
    f.definition.bodies.push(nested);
    let c = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
    let ct = f.body().places[c as usize].ty;
    let mut row = interner().get(ct).unwrap().clone();
    row.modes.multiplicity = multiplicity;
    let ct2 = f.definition.types.push(row);
    let cl = f.body().places[c as usize].root;
    f.body().locals[cl.0 as usize].ty = ct2;
    f.body().places[c as usize].ty = ct2;
    let fail = f.cleanup(vec![drop(y)]);
    f.assign(
        c,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(y, capture_usage)],
            destination: Destination::Inferred,
        },
        fail,
    );
    // loan on closure for the calls
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(c),
        mutable: false,
        if_inactive: false,
    });
    let via = f.through_loan(c, 0);
    let keep_y = capture_usage != UseKind::Take;
    let mut actions = vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(c),
    ];
    if keep_y {
        actions.push(drop(y));
    }
    let exit = f.cleanup(actions);
    let mut from = 0;
    for _ in 0..calls {
        let r = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let next = f.body().blocks.len();
        f.body().blocks[from].terminator = Terminator::Call {
            callee: Callee::Indirect {
                forward_caller: false,
                value: operand(via, UseKind::Borrow),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(r),
                next: BlockId(next as u32),
            }),
            on_failure: CleanupId(exit),
        };
        f.body().blocks.push(block(vec![], ret(0, exit)));
        from = next;
    }
    f.body().blocks[from].terminator = ret(0, exit);
    f
}

#[test]
fn p5a_closure_type_forges_authority_of_its_captures() {
    for wrapper in 0..3 {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        f.signatures.insert(id("Dir"), capability_signature("Dir"));
        let dir = f.definition.types.push(ty(TypeKind::Named {
            definition: id("Dir"),
            arguments: vec![].into(),
        }));
        let kind = match wrapper {
            0 => interner().get(dir).unwrap().kind.clone(),
            1 => TypeKind::Box(dir),
            _ => TypeKind::Variant(
                vec![Variant {
                    name: "D".into(),
                    fields: vec![(None, dir)].into(),
                }]
                .into(),
            ),
        };
        let d = param(&mut f, kind);
        let dty = f.body().places[d as usize].ty;
        let mut nested = empty_body(&mut f);
        add_capture(&mut nested, dty);
        f.definition.bodies.push(nested);
        let c = f.slot(
            TypeKind::Function(fn_type(vec![], unit)),
            Storage::Temporary,
        ); // authority inert, uses {}
        f.assign(
            c,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![copy(d)],
                destination: Destination::Inferred,
            },
            0,
        );
        let cty = f.body().places[c as usize].ty;
        f.body().result = cty;
        f.body().blocks[0].terminator = ret(c, 0);
        owner_sig(&mut f, &[PassingMode::Borrow]);
        assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "closure authority"
        }));
        replace_closure_row(&mut f, |row| {
            if let TypeKind::Function(fun) = &mut row.kind {
                fun.authority =
                    AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into());
            }
        });
        assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "closure uses"
        }));
        replace_closure_row(&mut f, |row| {
            if let TypeKind::Function(fun) = &mut row.kind {
                fun.uses = Uses::Only(AuthoritySummary::Known(
                    vec![AuthorityTerm::Capability(path("Dir"))].into(),
                ));
            }
        });
        f.valid();
    }
}

#[test]
fn p5b_closure_type_forges_sendability() {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let x = param(&mut f, string_kind());
    let sty = f.body().places[x as usize].ty;
    let mut nested = empty_body(&mut f);
    add_capture(&mut nested, sty);
    f.definition.bodies.push(nested);
    let c = f.slot(
        TypeKind::Function(fn_type(vec![], unit)),
        Storage::Temporary,
    );
    let ct = f.body().places[c as usize].ty;
    let mut row = interner().get(ct).unwrap().clone();
    row.modes.sendability = Sendability::Sendable;
    let ct2 = f.definition.types.push(row);
    let cl = f.body().places[c as usize].root;
    f.body().locals[cl.0 as usize].ty = ct2;
    f.body().places[c as usize].ty = ct2;
    f.assign(
        c,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![copy(x)],
            destination: Destination::Inferred,
        },
        0,
    );
    f.body().result = ct2;
    f.body().blocks[0].terminator = ret(c, 0);
    owner_sig(&mut f, &[PassingMode::Borrow]);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "closure sendability"
    }));
    replace_closure_row(&mut f, |row| {
        row.modes.sendability = Sendability::NotSendable;
    });
    f.valid();
}

#[test]
fn taken_capture_requires_once_type() {
    assert!(
        consuming_closure(UseKind::Take, Multiplicity::Many, 2)
            .kinds()
            .contains(&ViolationKind::TypeMismatch {
                entity: "closure multiplicity"
            })
    );
    consuming_closure(UseKind::Copy, Multiplicity::Many, 2).valid();
}

#[test]
fn borrowed_resources_allow_many_but_owned_resources_require_once() {
    for owned in [false, true] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let mut declaration = capability_signature("Trace");
        let representation = interner().intern(ty(TypeKind::Tuple(vec![].into())));
        let SignatureKind::Type(d) = &mut declaration.kind else {
            unreachable!()
        };
        d.capability = None;
        d.resource = true;
        d.authority = AuthoritySummary::inert();
        d.representation = Some(representation);
        f.signatures.insert(id("Trace"), declaration);
        let source = param(
            &mut f,
            TypeKind::Named {
                definition: id("Trace"),
                arguments: vec![].into(),
            },
        );
        let source_ty = f.body().places[source as usize].ty;
        let mut nested = empty_body(&mut f);
        let capture = add_capture(&mut nested, source_ty);
        if owned {
            nested.add_cleanup(vec![drop(capture)]);
            nested.blocks[0].terminator = ret(0, 1);
        }
        f.definition.bodies.push(nested);
        let closure = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
        let original = f.body().places[closure as usize].ty;
        let mut row = interner().get(original).unwrap().clone();
        row.modes.relation = Relation::environment();
        row.modes.multiplicity = if owned {
            Multiplicity::Once
        } else {
            Multiplicity::Many
        };
        let closure_ty = f.definition.types.push(row.clone());
        f.body().locals[closure as usize].ty = closure_ty;
        f.body().places[closure as usize].ty = closure_ty;
        if !owned {
            f.emit(InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(source),
                mutable: false,
                if_inactive: false,
            });
        }
        let captured = if owned {
            source
        } else {
            f.through_loan(source, 0)
        };
        let end_loan = CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        };
        let fail = f.cleanup(if owned {
            vec![drop(source)]
        } else {
            vec![end_loan.clone(), drop(source)]
        });
        f.assign(
            closure,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![operand(
                    captured,
                    if owned {
                        UseKind::Take
                    } else {
                        UseKind::Borrow
                    },
                )],
                destination: Destination::Inferred,
            },
            fail,
        );
        let exit = f.cleanup(if owned {
            vec![drop(closure)]
        } else {
            vec![drop(closure), end_loan, drop(source)]
        });
        f.body().blocks[0].terminator = ret(0, exit);
        owner_sig(&mut f, &[PassingMode::Take]);
        f.valid();
        if owned {
            // Change only the claimed multiplicity of a valid owned closure.
            // The independent checker must still reject this forged producer.
            row.modes.multiplicity = Multiplicity::Many;
            let forged = f.definition.types.push(row);
            f.body().locals[closure as usize].ty = forged;
            f.body().places[closure as usize].ty = forged;
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "closure multiplicity"
            }));
            // Forging Copy is independently illegal for this resource, and
            // must still require Once without relying on the Take check.
            for value in &mut f.body().rvalues {
                if let Rvalue::Closure { captures, .. } = value {
                    captures[0].usage = UseKind::Copy;
                }
            }
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "closure multiplicity"
            }));
        }
    }
}

/// Repair the published row while preserving the producer and executable body.
fn replace_closure_row(f: &mut Fixture, edit: impl FnOnce(&mut Type)) {
    let place = f.body().places.len() - 1;
    let ty = f.body().places[place].ty;
    let mut row = interner().get(ty).unwrap().clone();
    edit(&mut row);
    let ty = f.definition.types.push(row);
    let root = f.body().places[place].root;
    f.body().locals[root.0 as usize].ty = ty;
    f.body().places[place].ty = ty;
    f.body().result = ty;
    owner_signature(f, PassingMode::Borrow);
}

fn indirect_call(f: &mut Fixture, from: usize, callee: Operand, dest: u32) -> usize {
    let next = f.body().blocks.len();
    f.body().blocks[from].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: callee,
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(dest),
            next: BlockId(next as u32),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    next
}

#[test]
fn p3_once_callable_invoked_twice_by_borrow() {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let g = f.slot(
        TypeKind::Function(fn_type(vec![], unit)),
        Storage::Parameter,
    );
    let gl = f.body().places[g as usize].root;
    let gt = f.body().places[g as usize].ty;
    // mark once
    let mut row = interner().get(gt).unwrap().clone();
    row.modes.multiplicity = Multiplicity::Once;
    let once = f.definition.types.push(row);
    f.body().locals[gl.0 as usize].ty = once;
    f.body().places[g as usize].ty = once;
    f.body().parameters = vec![gl];
    let r1 = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let r2 = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let b1 = indirect_call(&mut f, 0, operand(g, UseKind::Borrow), r1);
    let b2 = indirect_call(&mut f, b1, operand(g, UseKind::Borrow), r2);
    f.body().blocks[b2].terminator = ret(0, 0);
    owner_signature(&mut f, PassingMode::Borrow);
    assert!(f.kinds().contains(&ViolationKind::InvalidUse(PlaceId(g))));
}

#[test]
fn once_calls_consume_the_callable_exactly_once() {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let g = param(&mut f, TypeKind::Function(fn_type(vec![], unit)));
    let old = f.body().places[g as usize].ty;
    let mut row = interner().get(old).unwrap().clone();
    row.modes.multiplicity = Multiplicity::Once;
    let ty = f.definition.types.push(row);
    let root = f.body().places[g as usize].root;
    f.body().places[g as usize].ty = ty;
    f.body().locals[root.0 as usize].ty = ty;
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let next = indirect_call(&mut f, 0, take(g), result);
    owner_sig(&mut f, &[PassingMode::Take]);
    f.valid();
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    indirect_call(&mut f, next, take(g), result);
    assert!(f.kinds().contains(&ViolationKind::DoubleMove(PlaceId(g))));
}

#[test]
fn p4_one_body_two_producers_with_different_capture_usage() {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let x = f.slot(string_kind(), Storage::Local);
    f.assign(x, Rvalue::Constant(Constant::String("x".into())), 0);
    let y = f.slot(string_kind(), Storage::Local);
    let cx = f.cleanup(vec![drop(x)]);
    f.assign(y, Rvalue::Constant(Constant::String("y".into())), cx);
    let sty = f.body().places[x as usize].ty;
    let mut nested = empty_body(&mut f);
    let cap = add_capture(&mut nested, sty);
    nested.add_cleanup(vec![drop(cap)]);
    nested.blocks[0].terminator = ret(0, 1); // body releases its capture
    f.definition.bodies.push(nested);
    let a = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
    let b = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
    // producer A borrows x through a loan; producer B takes y.
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(x),
        mutable: false,
        if_inactive: false,
    });
    let via = f.through_loan(x, 0);
    let end = CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    };
    let c1 = f.cleanup(vec![end.clone(), drop(y), drop(x)]);
    f.assign(
        a,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(via, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        c1,
    );
    let c2 = f.cleanup(vec![drop(a), end.clone(), drop(y), drop(x)]);
    f.assign(
        b,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![take(y)],
            destination: Destination::Inferred,
        },
        c2,
    );
    let exit = f.cleanup(vec![drop(b), drop(a), end, drop(x)]);
    f.body().blocks[0].terminator = ret(0, exit);
    // Finding 7 now rejects the report's Many rows. Correct that
    // independent error so differing producer conventions still reach flow.
    for place in [a, b] {
        let old = f.body().places[place as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
        row.modes.multiplicity = Multiplicity::Once;
        row.modes.relation = Relation::environment();
        let ty = f.definition.types.push(row);
        let root = f.body().places[place as usize].root;
        f.body().locals[root.0 as usize].ty = ty;
        f.body().places[place as usize].ty = ty;
    }
    assert!(f.kinds().contains(&ViolationKind::Duplicate {
        entity: "closure body",
        index: 1
    }));
}

#[test]
fn p13b_expired_view_inside_aggregate_then_whole_aggregate_taken() {
    // x: owned local String; t = (own,) ; loan L on x ; t.0 = Use(Borrow x via L) ; EndLoan L ; drop x ; return t
    let mut f = Fixture::unit();
    let x = f.slot(string_kind(), Storage::Local);
    f.assign(x, Rvalue::Constant(Constant::String("x".into())), 0);
    let sty = f.body().places[x as usize].ty;
    let t = f.slot(TypeKind::Tuple(vec![sty].into()), Storage::Temporary);
    let s0 = f.slot(string_kind(), Storage::Temporary);
    let cx = f.cleanup(vec![drop(x)]);
    f.assign(s0, Rvalue::Constant(Constant::String("own".into())), cx);
    let c2 = f.cleanup(vec![drop(s0), drop(x)]);
    f.assign(
        t,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(s0),
            }],
            destination: Destination::Inferred,
        },
        c2,
    );
    let mut owned = Fixture {
        definition: f.definition.clone(),
        signatures: f.signatures.clone(),
    };
    let ty = owned.body().places[t as usize].ty;
    owned.body().result = ty;
    owned.body().blocks[0].terminator = ret(t, cx);
    owned.valid();
    let troot = f.body().places[t as usize].root;
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: troot,
        projection: vec![Projection::Field(0)].into(),
        ty: sty,
    });
    let c3 = f.cleanup(vec![drop(t), drop(x)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(field),
            if_live: false,
        },
        c3,
    ));
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(x),
        mutable: false,
        if_inactive: false,
    });
    let via = f.through_loan(x, 0);
    let c4 = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(t),
        drop(x),
    ]);
    f.assign(field, Rvalue::Use(operand(via, UseKind::Borrow)), c4);
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    let tty = f.body().places[t as usize].ty;
    f.body().result = tty;
    f.body().blocks[0].terminator = ret(t, cx);
    assert!(f.kinds().contains(&ViolationKind::ExpiredLoan {
        loan: LoanId(0),
        place: PlaceId(t)
    }));
    let mut read_copy = Fixture {
        definition: f.definition.clone(),
        signatures: f.signatures.clone(),
    };
    let out = read_copy.slot(TypeKind::Tuple(vec![sty].into()), Storage::Temporary);
    read_copy.assign(out, Rvalue::Use(copy(t)), c3);
    let exit = read_copy.cleanup(vec![drop(out), drop(t), drop(x)]);
    read_copy.body().result = read_copy.body().places[0].ty;
    read_copy.body().blocks[0].terminator = ret(0, exit);
    assert!(read_copy.kinds().contains(&ViolationKind::ExpiredLoan {
        loan: LoanId(0),
        place: PlaceId(t)
    }));
}

#[test]
fn p1_borrowed_parameter_laundered_into_owned_global_aggregate() {
    let mut f = Fixture::unit();
    let x = f.slot(string_kind(), Storage::Parameter);
    let xl = f.body().places[x as usize].root;
    f.body().parameters = vec![xl];
    let sty = f.body().places[x as usize].ty;
    let t = f.slot(TypeKind::Tuple(vec![sty].into()), Storage::Temporary);
    f.assign(
        t,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: operand(x, UseKind::Borrow),
            }],
            destination: Destination::Inferred,
        },
        0,
    );
    let tty = f.body().places[t as usize].ty;
    f.body().result = tty;
    f.body().blocks[0].terminator = ret(t, 0);
    owner_signature(&mut f, PassingMode::Borrow);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(t)))
    );
    let InstructionKind::Assign { value, .. } =
        f.body().blocks[0].instructions.last().unwrap().kind
    else {
        unreachable!()
    };
    let Rvalue::Construct { fields, .. } = &mut f.body().rvalues[value.0 as usize] else {
        unreachable!()
    };
    fields[0].value.usage = UseKind::Copy;
    f.valid();
}

#[test]
fn p13_borrowed_view_written_into_field_of_owned_aggregate_then_returned() {
    let mut f = Fixture::unit();
    let x = param(&mut f, string_kind());
    let sty = f.body().places[x as usize].ty;
    let t = f.slot(TypeKind::Tuple(vec![sty].into()), Storage::Temporary);
    let s0 = f.slot(string_kind(), Storage::Temporary);
    f.assign(s0, Rvalue::Constant(Constant::String("own".into())), 0);
    let fail = f.cleanup(vec![drop(s0)]);
    f.assign(
        t,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(s0),
            }],
            destination: Destination::Inferred,
        },
        fail,
    );
    let troot = f.body().places[t as usize].root;
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: troot,
        projection: vec![Projection::Field(0)].into(),
        ty: sty,
    });
    let failt = f.cleanup(vec![drop(t)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(field),
            if_live: false,
        },
        failt,
    ));
    f.assign(field, Rvalue::Use(operand(x, UseKind::Borrow)), failt);
    let tty = f.body().places[t as usize].ty;
    f.body().result = tty;
    f.body().blocks[0].terminator = ret(t, 0);
    owner_sig(&mut f, &[PassingMode::Borrow]);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(field)))
    );
}

#[test]
fn p5c_global_closure_borrowing_a_borrowed_parameter_is_returned() {
    for local in [false, true] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let x = param(&mut f, string_kind());
        if local {
            let xt = f.body().places[x as usize].ty;
            let mut row = interner().get(xt).unwrap().clone();
            row.modes.relation = Relation::environment();
            let t2 = f.definition.types.push(row);
            let xl = f.body().places[x as usize].root;
            f.body().locals[xl.0 as usize].ty = t2;
            f.body().places[x as usize].ty = t2;
        }
        let sty = f.body().places[x as usize].ty;
        let mut nested = empty_body(&mut f);
        add_capture(&mut nested, sty);
        f.definition.bodies.push(nested);
        let c = f.slot(
            TypeKind::Function(fn_type(vec![], unit)),
            Storage::Temporary,
        );
        f.assign(
            c,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![operand(x, UseKind::Borrow)],
                destination: Destination::Inferred,
            },
            0,
        );
        let cty = f.body().places[c as usize].ty;
        f.body().result = cty;
        f.body().blocks[0].terminator = ret(c, 0);
        owner_sig(&mut f, &[PassingMode::Borrow]);
        assert!(f.kinds().iter().any(
            |v| matches!(v, ViolationKind::InvalidResultSource(p) if *p == PlaceId(c))
                || matches!(
                    v,
                    ViolationKind::TypeMismatch {
                        entity: "closure locality"
                    }
                )
        ));
    }
}

#[test]
fn p14_owner_slot_refilled_without_origin_events_keeps_name_identity() {
    for (refill, atomic, control) in [
        (false, false, 0),
        (true, false, 0),
        (false, false, 1),
        (true, false, 2),
        (true, true, 0),
        (true, true, 2),
        (true, true, 3),
    ] {
        let mut f = Fixture::unit();
        let handle_owner = if control == 3 { 1 } else { 0 };
        f.signatures.insert(id("Dir"), capability_signature("Dir"));
        let scope = scoped(hirsel_types::NameSite::Item);
        let d = param(
            &mut f,
            TypeKind::Named {
                definition: id("Dir"),
                arguments: vec![].into(),
            },
        );
        let e = param(
            &mut f,
            TypeKind::Named {
                definition: id("Dir"),
                arguments: vec![].into(),
            },
        );
        let h = param(&mut f, TypeKind::Primitive(Primitive::U8));
        // signature first (Bound references), then the body row (Body references)
        owner_sig(
            &mut f,
            &[PassingMode::Mut, PassingMode::Borrow, PassingMode::Borrow],
        );
        {
            let s = f.signatures.get_mut(&path("main")).unwrap();
            let SignatureKind::Function(fun) = &mut s.kind else {
                unreachable!()
            };
            let mut row = interner().get(fun.parameters[2].ty).unwrap().clone();
            row.brand.alternatives.push(Origin {
                root: OriginRoot::Parameter(ParameterRef::bound(ParamId(handle_owner))),
                fields: vec![].into(),
            });
            fun.parameters[2].ty = interner().intern(row);
        }
        let ht = f.body().places[h as usize].ty;
        let mut row = interner().get(ht).unwrap().clone();
        row.brand.alternatives.push(Origin {
            root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(handle_owner))),
            fields: vec![].into(),
        });
        let bt = f.definition.types.push(row);
        let hl = f.body().places[h as usize].root;
        f.body().locals[hl.0 as usize].ty = bt;
        f.body().places[h as usize].ty = bt;
        let name = Origin {
            root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
            fields: vec![].into(),
        };
        f.body().places[d as usize].binding = Some(name.clone());
        if refill {
            let t = f.slot(
                TypeKind::Named {
                    definition: id("Dir"),
                    arguments: vec![].into(),
                },
                Storage::Temporary,
            );
            if atomic {
                f.emit(InstructionKind::Replace {
                    destination: PlaceId(d),
                    value: copy(e),
                    displaced: PlaceId(t),
                    if_live: false,
                });
            } else {
                f.assign(t, Rvalue::Use(take(d)), 0); // original review probe
                f.assign(d, Rvalue::Use(copy(e)), 0);
            }
        }
        // inspect(d, h) : fn inspect(x: Dir, p: u8 of x)
        let mut s = function_signature(
            "inspect",
            vec![
                (
                    PassingMode::Borrow,
                    TypeKind::Named {
                        definition: id("Dir"),
                        arguments: vec![].into(),
                    },
                ),
                (PassingMode::Borrow, TypeKind::Primitive(Primitive::U8)),
            ],
            TypeKind::Tuple(vec![].into()),
        );
        if let SignatureKind::Function(fun) = &mut s.kind {
            let mut row = interner().get(fun.parameters[1].ty).unwrap().clone();
            row.brand.alternatives.push(Origin {
                root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
                fields: vec![].into(),
            });
            fun.parameters[1].ty = interner().intern(row);
        }
        f.signatures.insert(id("inspect"), s);
        let r = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("inspect"),
                arguments: vec![],
            },
            arguments: vec![
                CallArgument {
                    parameter: ParamId(0),
                    value: operand(d, UseKind::Borrow),
                },
                CallArgument {
                    parameter: ParamId(1),
                    value: copy(h),
                },
            ],
            returns: Some(CallReturn {
                destination: PlaceId(r),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(vec![], ret(0, 0)));
        if control == 1 {
            // control: pass the other directory `e` as the owner
            if let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator {
                arguments[0].value = operand(e, UseKind::Borrow);
            }
        }
        if control == 2 || control == 3 {
            // control: same refill, but the access does not claim the source name
            f.body().places[d as usize].binding = None;
        }
        if (!refill && control == 0) || control == 3 {
            f.valid();
        } else {
            assert!(f.kinds().iter().any(|v| matches!(
                v,
                ViolationKind::TypeMismatch {
                    entity: "call argument"
                }
            ) | matches!(v, ViolationKind::InvalidOrigin(_))
                || (!atomic && matches!(v, ViolationKind::WriteToLive(_)))));
        }
    }
}

#[test]
fn fresh_field_replacement_preserves_the_enclosing_owner_for_new_brands() {
    let mut f = Fixture::unit();
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let r = param(&mut f, TypeKind::Tuple(vec![scalar].into()));
    let scope = f.body().parameter_scope;
    let owner = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    };
    f.body().places[r as usize].binding = Some(owner.clone());
    owner_sig(&mut f, &[PassingMode::Take]);
    let field = f.body().places.len() as u32;
    let root = f.body().places[r as usize].root;
    let mut field_name = owner.clone();
    field_name.fields.push(FieldId(0));
    f.body().places.push(Place {
        loan: None,
        binding: Some(field_name),
        root,
        projection: vec![Projection::Field(0)].into(),
        ty: scalar,
    });
    let new = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    f.assign(new, Rvalue::Constant(Constant::Integer(7)), 0);
    let old = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    f.emit(InstructionKind::EndOrigin(PlaceId(field)));
    f.emit(InstructionKind::Replace {
        destination: PlaceId(field),
        value: take(new),
        displaced: PlaceId(old),
        if_live: false,
    });
    f.emit(InstructionKind::BindOrigin {
        source: PlaceId(field),
        place: PlaceId(field),
        kind: OriginBindingKind::Fresh,
    });
    let h = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    let mut handle = ty(TypeKind::Primitive(Primitive::U8));
    handle.brand.alternatives.push(owner);
    let ht = f.definition.types.push(handle);
    f.body().places[h as usize].ty = ht;
    let hroot = f.body().places[h as usize].root;
    f.body().locals[hroot.0 as usize].ty = ht;
    let out = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let rty = f.body().places[r as usize].ty;
    for name in ["make", "inspect"] {
        let mut signature = function_signature(name, vec![], TypeKind::Tuple(vec![].into()));
        let record = rty;
        let mut branded = ty(TypeKind::Primitive(Primitive::U8));
        branded.brand.alternatives.push(Origin {
            root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
            fields: vec![].into(),
        });
        let branded = interner().intern(branded);
        let SignatureKind::Function(fun) = &mut signature.kind else {
            unreachable!()
        };
        fun.parameters.push(Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: record,
        });
        if name == "make" {
            fun.result = branded;
        } else {
            fun.parameters.push(Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: branded,
            });
        }
        f.signatures.insert(id(name), signature);
    }
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("make"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: copy(r),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(h),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(
        vec![],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("inspect"),
                arguments: vec![],
            },
            arguments: vec![
                CallArgument {
                    parameter: ParamId(0),
                    value: copy(r),
                },
                CallArgument {
                    parameter: ParamId(1),
                    value: copy(h),
                },
            ],
            returns: Some(CallReturn {
                destination: PlaceId(out),
                next: BlockId(2),
            }),
            on_failure: CleanupId(0),
        },
    ));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.valid();
}

#[test]
fn p7_otherwise_block_of_exhaustive_switch_is_never_flow_checked() {
    let mut f = Fixture::unit();
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
    let s = f.slot(string_kind(), Storage::Local);
    let t = f.slot(string_kind(), Storage::Local);
    f.body().blocks[0].terminator = Terminator::Switch {
        value: copy(c),
        arms: vec![(0, BlockId(1)), (1, BlockId(1))],
        otherwise: BlockId(2),
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    // block 2: reads uninitialised s, moves it twice, leaks, never returns properly
    let body = &mut f.definition.bodies[0];
    let r0 = RvalueId(body.rvalues.len() as u32);
    body.rvalues.push(Rvalue::Use(take(s)));
    let r1 = RvalueId(body.rvalues.len() as u32);
    body.rvalues.push(Rvalue::Use(take(s)));
    body.blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(t),
                    value: r0,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(t),
                    if_live: false,
                },
                0,
            ),
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(t),
                    value: r1,
                },
                0,
            ),
        ],
        ret(0, 0),
    ));
    assert!(
        f.kinds()
            .contains(&ViolationKind::ReadBeforeInit(PlaceId(s)))
    );
    f.body().blocks[2].instructions.clear();
    f.body().rvalues.truncate(2);
    f.body().blocks[2].terminator = Terminator::Unreachable;
    f.valid();
}

#[test]
fn exhaustive_switch_defaults_also_validate_their_closure_bodies() {
    let mut f = Fixture::unit();
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
    let mut nested = Fixture::string();
    nested.body().blocks[0].terminator = ret(0, 0);
    let body = nested.definition.bodies.remove(0);
    f.definition.bodies.push(body);
    let unit = unit_ty(&mut f);
    let closure = f.slot(
        TypeKind::Function(fn_type(vec![], unit)),
        Storage::Temporary,
    );
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let construction = f.body().blocks[0].instructions.pop().unwrap();
    let exit = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = Terminator::Switch {
        value: copy(c),
        arms: vec![(0, BlockId(1)), (1, BlockId(1))],
        otherwise: BlockId(2),
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.body()
        .blocks
        .push(block(vec![construction], ret(0, exit)));
    let report = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.body == Some(BodyId(1)) && v.kind == ViolationKind::LeakedSlot(LocalId(1)))
    );
}

#[test]
fn p6_arena_destination_with_unplaced_type_is_returned() {
    let mut f = Fixture::unit();
    f.signatures.insert(id("Arena"), {
        let mut s = capability_signature("Arena");
        if let SignatureKind::Type(t) = &mut s.kind {
            t.capability = Some(CapabilityKind::Scoped);
            t.bounds.clear();
        }
        s
    });
    let scope = scoped(hirsel_types::NameSite::Item);
    let a = param(
        &mut f,
        TypeKind::Named {
            definition: id("Arena"),
            arguments: vec![].into(),
        },
    );
    let _ = a;
    let st = f.definition.types.push(ty(string_kind()));
    let s0 = f.slot(string_kind(), Storage::Temporary);
    f.assign(s0, Rvalue::Constant(Constant::String("x".into())), 0);
    let fail = f.cleanup(vec![drop(s0)]);
    let _ = st;
    let sty = f.body().places[s0 as usize].ty;
    let t = f.slot(TypeKind::Box(sty), Storage::Temporary);
    let arena = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    };
    f.assign(
        t,
        Rvalue::Construct {
            constructor: Constructor::Box,
            fields: vec![ConstructField {
                field: 0,
                value: take(s0),
            }],
            destination: Destination::Arena(arena),
        },
        fail,
    );
    let tty = f.body().places[t as usize].ty;
    f.body().result = tty;
    f.body().blocks[0].terminator = ret(t, 0);
    owner_sig(&mut f, &[PassingMode::Borrow]);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "constructor placement"
    }));
}

#[test]
fn scoped_construction_placements_are_exact_for_aggregates_and_closures() {
    for closure in [false, true] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let placed = f.slot(
            if closure {
                TypeKind::Function(fn_type(vec![], unit))
            } else {
                TypeKind::Tuple(vec![].into())
            },
            Storage::Temporary,
        );
        let destination = Destination::Region(RegionId(0));
        let value = if closure {
            let nested = empty_body(&mut f);
            f.definition.bodies.push(nested);
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![],
                destination: destination.clone(),
            }
        } else {
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![],
                destination: destination.clone(),
            }
        };
        f.assign(placed, value, 0);
        let exit = f.cleanup(vec![drop(placed)]);
        f.body().blocks[0].terminator = ret(0, exit);
        let entity = if closure {
            "closure placement"
        } else {
            "constructor placement"
        };
        assert!(f.kinds().contains(&ViolationKind::TypeMismatch { entity }));
        let old_ty = f.body().places[placed as usize].ty;
        let mut row = interner().get(old_ty).unwrap().clone();
        row.modes.relation = Relation::new([Reach::Region(RegionId(0))]);
        let scoped = f.definition.types.push(row);
        let root = f.body().places[placed as usize].root;
        f.body().places[placed as usize].ty = scoped;
        f.body().locals[root.0 as usize].ty = scoped;
        f.valid();
    }
}

fn nested_generic_parameter(outer: Vec<Bound>, inner: Option<Vec<Bound>>) -> Fixture {
    let mut f = Fixture::unit();
    owner_signature(&mut f, PassingMode::Take);
    let sig = f.signatures.get_mut(f.definition.path).unwrap();
    let SignatureKind::Function(owner) = &mut sig.kind else {
        panic!()
    };
    owner.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: outer.into(),
    }]
    .into();
    let unit = unit_ty(&mut f);
    let reference = if inner.is_some() {
        TypeParameterRef::bound(TypeParamId(0))
    } else {
        TypeParameterRef::body(f.body().parameter_scope, TypeParamId(0))
    };
    let p = f.definition.types.push(ty(TypeKind::Parameter(reference)));
    let mut function = fn_type(
        vec![Parameter {
            name: Some("y".into()),
            label: None,
            passing: PassingMode::Take,
            ty: p,
        }],
        unit,
    );
    if let Some(bounds) = inner {
        function.generics = vec![GenericParameter::Type {
            name: "U".into(),
            bounds: bounds.into(),
        }]
        .into();
    }
    let c = f.slot(TypeKind::Function(function), Storage::Local);
    let mut nested = empty_body(&mut f);
    let p = bind_parameter_scopes(interner(), p, &[nested.parameter_scope]).unwrap();
    let y = add_capture(&mut nested, p);
    nested.locals[y as usize].storage = Storage::Parameter;
    nested.parameters.push(LocalId(y));
    f.definition.bodies.push(nested);
    f.assign(
        c,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(c)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn nested_generic_bodies_prove_release_with_their_own_bounds() {
    let f = nested_generic_parameter(vec![Bound::Plain], Some(vec![]));
    assert!(
        f.kinds().contains(&ViolationKind::LeakedSlot(LocalId(1))),
        "{:?}",
        f.kinds()
    );
    nested_generic_parameter(vec![], Some(vec![Bound::Plain])).valid();
    nested_generic_parameter(vec![Bound::Plain], None).valid();
}

fn generic_call_fixture(bound: Bound, kind: TypeKind) -> Fixture {
    let mut f = Fixture::unit();
    let x = param(&mut f, kind.clone());
    if matches!(kind, TypeKind::Primitive(p) if !matches!(p, Primitive::String | Primitive::Bytes))
    {
        let original = f.body().places[x as usize].ty;
        let mut row = interner().get(original).unwrap().clone();
        row.modes.sendability = Sendability::Sendable;
        let t = f.definition.types.push(row);
        f.body().places[x as usize].ty = t;
        f.body().locals[x as usize].ty = t;
    }
    owner_sig(&mut f, &[PassingMode::Take]);
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let mut s = function_signature(
        "callee",
        vec![(PassingMode::Take, kind)],
        TypeKind::Tuple(vec![].into()),
    );
    let t = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let SignatureKind::Function(fun) = &mut s.kind else {
        panic!()
    };
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![bound].into(),
    }]
    .into();
    fun.parameters[0].ty = t;
    f.signatures.insert(id("callee"), s);
    let actual = f.body().places[x as usize].ty;
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![GenericArgument::Type(actual)],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: take(x),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    f
}

#[test]
fn generic_calls_prove_every_declared_bound() {
    for bound in [
        Bound::Plain,
        Bound::Copyable,
        Bound::Data,
        Bound::Key,
        Bound::Flat,
        Bound::Inert,
        Bound::Sendable,
        Bound::NoallocDrop,
    ] {
        let f = generic_call_fixture(bound, TypeKind::Primitive(Primitive::U64));
        assert!(f.kinds().is_empty(), "{bound:?}: {:?}", f.kinds());
    }
    for (bound, kind) in [
        (Bound::Plain, string_kind()),
        (Bound::Flat, string_kind()),
        (Bound::Sendable, string_kind()),
        (Bound::Key, TypeKind::Primitive(Primitive::F64)),
    ] {
        assert!(
            generic_call_fixture(bound, kind)
                .kinds()
                .contains(&ViolationKind::TypeMismatch {
                    entity: "generic bound"
                })
        );
    }
    generic_call_fixture(Bound::Data, string_kind()).valid();
    generic_call_fixture(Bound::NoallocDrop, string_kind()).valid();
}

#[test]
fn nested_generic_calls_cannot_inherit_inert_or_sendable_evidence() {
    for bound in [Bound::Inert, Bound::Sendable] {
        for proven in [false, true] {
            let mut f = nested_generic_parameter(
                vec![bound],
                Some(if proven { vec![bound] } else { vec![] }),
            );
            let original = f.definition.bodies[1].places[1].ty;
            let mut row = interner().get(original).unwrap().clone();
            row.modes.sendability = if bound == Bound::Sendable && proven {
                Sendability::Sendable
            } else {
                Sendability::DependsOn(
                    vec![TypeParameterRef::body(
                        f.definition.bodies[1].parameter_scope,
                        TypeParamId(0),
                    )]
                    .into(),
                )
            };
            let p = f.definition.types.push(row.clone());
            f.definition.bodies[1].locals[1].ty = p;
            f.definition.bodies[1].places[1].ty = p;
            // The closure signature must retain exactly the nested parameter contract.
            let c = f.body().places[1].ty;
            row.kind = TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0)));
            if let Sendability::DependsOn(ps) = &mut row.modes.sendability {
                *ps = vec![TypeParameterRef::bound(TypeParamId(0))].into();
            }
            let outer_p = f.definition.types.push(row);
            let mut c_row = interner().get(c).unwrap().clone();
            let TypeKind::Function(fun) = &mut c_row.kind else {
                panic!()
            };
            fun.parameters[0].ty = outer_p;
            let c_row = f.definition.types.push(c_row);
            f.body().places[1].ty = c_row;
            f.body().locals[1].ty = c_row;
            let mut s = function_signature(
                "accept",
                vec![(PassingMode::Take, TypeKind::Primitive(Primitive::U64))],
                TypeKind::Tuple(vec![].into()),
            );
            let t = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let SignatureKind::Function(fun) = &mut s.kind else {
                panic!()
            };
            fun.generics = vec![GenericParameter::Type {
                name: "X".into(),
                bounds: vec![bound].into(),
            }]
            .into();
            fun.parameters[0].ty = t;
            f.signatures.insert(id("accept"), s);
            let nested = &mut f.definition.bodies[1];
            nested.blocks[0].terminator = Terminator::Call {
                callee: Callee::Definition {
                    path: path("accept"),
                    arguments: vec![GenericArgument::Type(p)],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: take(1),
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(0),
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            };
            nested.blocks[0].instructions.clear();
            nested.rvalues.clear();
            nested.blocks.push(block(vec![], ret(0, 0)));
            if proven {
                f.valid();
            } else {
                assert!(
                    f.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "generic bound"
                    }),
                    "{:?}",
                    f.kinds()
                );
            }
        }
    }
}

#[test]
fn nominal_arguments_discharge_bounds_at_their_declaring_binders() {
    let mut f = Fixture::unit();
    let mut s = capability_signature("Wrapper");
    let rep = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let SignatureKind::Type(d) = &mut s.kind else {
        panic!()
    };
    d.capability = None;
    d.authority = AuthoritySummary::inert();
    d.representation = Some(rep);
    d.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Plain].into(),
    }];
    f.signatures.insert(id("Wrapper"), s);
    let actual = f.definition.types.push(ty(string_kind()));
    let w = param(
        &mut f,
        TypeKind::Named {
            definition: id("Wrapper"),
            arguments: vec![GenericArgument::Type(actual)].into(),
        },
    );
    owner_sig(&mut f, &[PassingMode::Take]);
    let c = f.cleanup(vec![drop(w)]);
    f.body().blocks[0].terminator = ret(0, c);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "nominal generic bound"
    }));
    let actual = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let root = f.body().places[w as usize].root;
    let t = f.definition.types.push(ty(TypeKind::Named {
        definition: id("Wrapper"),
        arguments: vec![GenericArgument::Type(actual)].into(),
    }));
    f.body().places[w as usize].ty = t;
    f.body().locals[root.0 as usize].ty = t;
    owner_sig(&mut f, &[PassingMode::Take]);
    f.valid();
}

#[test]
fn sendable_bounds_use_each_complete_actual_before_generic_defaults() {
    let mut f = generic_call_fixture(Bound::Sendable, string_kind());
    let original = f.body().places[1].ty;
    let mut row = interner().get(original).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    row.modes.sendability = Sendability::Sendable;
    let unique = f.definition.types.push(row);
    f.body().locals[1].ty = unique;
    f.body().places[1].ty = unique;
    owner_sig(&mut f, &[PassingMode::Take]);
    // Substitution retains the aliased generic default; the complete value proves Sendable.
    f.valid();
    let second = param(&mut f, string_kind());
    owner_sig(&mut f, &[PassingMode::Take, PassingMode::Take]);
    let s = f.signatures.get_mut(&path("callee")).unwrap();
    let SignatureKind::Function(fun) = &mut s.kind else {
        panic!()
    };
    fun.parameters.push(fun.parameters[0].clone());
    let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
        panic!()
    };
    arguments.push(CallArgument {
        parameter: ParamId(1),
        value: take(second),
    });
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "generic bound"
    }));
}

#[test]
fn noalloc_drop_bounds_derive_nominal_cleanup_instead_of_trusting_the_value_row() {
    for allocation in [Allocation::MayAllocate, Allocation::NoAlloc] {
        let mut f = generic_call_fixture(
            Bound::NoallocDrop,
            TypeKind::Named {
                definition: id("Resource"),
                arguments: vec![].into(),
            },
        );
        let mut s = capability_signature("Resource");
        let rep = interner().intern(ty(TypeKind::Tuple(vec![].into())));
        let SignatureKind::Type(d) = &mut s.kind else {
            panic!()
        };
        d.capability = None;
        d.authority = AuthoritySummary::inert();
        d.resource = true;
        d.representation = Some(rep);
        d.destructor_allocation = allocation;
        f.signatures.insert(id("Resource"), s);
        if allocation == Allocation::NoAlloc {
            let original = f.body().places[1].ty;
            let mut row = interner().get(original).unwrap().clone();
            row.modes.sendability = Sendability::Sendable;
            let t = f.definition.types.push(row);
            f.body().places[1].ty = t;
            f.body().locals[1].ty = t;
            owner_sig(&mut f, &[PassingMode::Take]);
            f.valid();
        } else {
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "generic bound"
            }));
        }
    }
}

fn unique_copy_fixture(plain: bool) -> Fixture {
    let kind = if plain {
        TypeKind::Primitive(Primitive::U64)
    } else {
        string_kind()
    };
    let mut f = Fixture::unit();
    let x = f.slot(kind.clone(), Storage::Local);
    let xt = f.body().places[x as usize].ty;
    let mut row = interner().get(xt).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let ut = f.definition.types.push(row);
    let xl = f.body().places[x as usize].root;
    f.body().locals[xl.0 as usize].ty = ut;
    f.body().places[x as usize].ty = ut;
    f.assign(
        x,
        Rvalue::Constant(if plain {
            Constant::Integer(1)
        } else {
            Constant::String("u".into())
        }),
        0,
    );
    let y = f.slot(kind.clone(), Storage::Local);
    let cx = f.cleanup(vec![drop(x)]);
    f.assign(y, Rvalue::Use(copy(x)), cx);
    let exit = f.cleanup(vec![drop(y), drop(x)]);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(x),
        mutable: true,
        if_inactive: false,
    });
    let via = f.through_loan(x, 0);
    let r = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let fail = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(y),
        drop(x),
    ]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("require_unique"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(via, UseKind::Mut),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(r),
            next: BlockId(1),
        }),
        on_failure: CleanupId(fail),
    };
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            exit,
        )],
        ret(0, exit),
    ));
    let mut s = function_signature(
        "require_unique",
        vec![(PassingMode::Mut, kind)],
        TypeKind::Tuple(vec![].into()),
    );
    if let SignatureKind::Function(fun) = &mut s.kind {
        let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        fun.parameters[0].ty = interner().intern(row);
    }
    f.signatures.insert(id("require_unique"), s);
    f
}

#[test]
fn pf_unique_binding_shared_by_copy_then_used_as_unique() {
    assert!(
        unique_copy_fixture(false)
            .kinds()
            .contains(&ViolationKind::TypeMismatch {
                entity: "call argument"
            })
    );
    unique_copy_fixture(true).valid();
    let mut f = unique_copy_fixture(false);
    f.body().rvalues[2] = Rvalue::Unary {
        op: hirsel_syntax::UnaryOp::Unique,
        value: copy(1),
    };
    f.valid();
}

#[test]
fn counted_copies_cannot_claim_unique_destinations_or_returns() {
    let mut f = unique_copy_fixture(false);
    let unique = f.body().places[1].ty;
    f.body().places[2].ty = unique;
    f.body().locals[2].ty = unique;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "value modes"
    }));
    let mut f = unique_copy_fixture(false);
    // Return x after sharing it. Keep the unique declared result and clean the alias.
    let unique = f.body().places[1].ty;
    f.body().result = unique;
    f.body().blocks.truncate(1);
    f.body().blocks[0]
        .instructions
        .retain(|i| !matches!(i.kind, InstructionKind::BeginLoan { .. }));
    let c = f.cleanup(vec![drop(2)]);
    f.body().blocks[0].terminator = ret(1, c);
    f.signatures.clear();
    for place in &mut f.body().places {
        place.loan = None;
    }
    for id in 0..f.body().cleanups.len() {
        edit_cleanup(f.body(), id as u32, |actions| {
            actions.retain(|a| !matches!(a, CleanupAction::EndLoan { .. }))
        });
    }
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "value modes"
    }));
}

#[test]
fn callback_bounds_inspect_complete_return_values_and_their_flowing_modes() {
    for shared in [false, true] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let promise = f.definition.types.push(ty(string_kind()));
        // This callback constructs its returned String on the counted heap.
        // Its independent sendability proof needs the truthful invocation effect.
        let mut callable = fn_type(vec![], promise);
        callable.allocation = Allocation::MayAllocate;
        let c = f.slot(TypeKind::Function(callable), Storage::Local);
        let mut nested = empty_body(&mut f);
        nested.result = promise;
        let mut row = ty(string_kind());
        row.modes.uniqueness = Uniqueness::Unique;
        row.modes.sendability = Sendability::Sendable;
        let unique = f.definition.types.push(row);
        let value = add_capture(&mut nested, unique);
        nested.locals[value as usize].storage = Storage::Temporary;
        nested
            .rvalues
            .push(Rvalue::Constant(Constant::String("result".into())));
        nested.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(value),
                value: RvalueId(1),
            },
            0,
        ));
        let mut actions = vec![];
        if shared {
            let alias = add_capture(&mut nested, promise);
            nested.locals[alias as usize].storage = Storage::Temporary;
            nested.rvalues.push(Rvalue::Use(copy(value)));
            nested.blocks[0].instructions.push(inst(
                InstructionKind::Assign {
                    destination: PlaceId(alias),
                    value: RvalueId(2),
                },
                0,
            ));
            actions.push(drop(alias));
        }
        nested.add_cleanup(actions);
        nested.blocks[0].terminator = ret(value, 1);
        f.definition.bodies.push(nested);
        f.assign(
            c,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![],
                destination: Destination::Inferred,
            },
            0,
        );
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(c),
            mutable: false,
            if_inactive: false,
        });
        let view = f.through_loan(c, 0);
        let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let cleanup = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(c),
        ]);
        let mut sig = function_signature("callee", vec![], TypeKind::Tuple(vec![].into()));
        let parameter = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        })));
        let mut callable = fn_type(vec![], parameter);
        callable.allocation = Allocation::MayAllocate;
        let callback = interner().intern(ty(TypeKind::Function(callable)));
        let SignatureKind::Function(fun) = &mut sig.kind else {
            panic!()
        };
        fun.generics = vec![GenericParameter::Type {
            name: "R".into(),
            bounds: vec![Bound::Sendable].into(),
        }]
        .into();
        fun.parameters.push(Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: callback,
        });
        f.signatures.insert(id("callee"), sig);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("callee"),
                arguments: vec![GenericArgument::Type(promise)],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(view, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: CleanupId(cleanup),
        };
        f.body().blocks.push(block(vec![], ret(result, cleanup)));
        if shared {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "generic bound"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
        let _ = unit;
    }
}

#[test]
fn ordinary_mut_calls_end_uniqueness_before_a_later_unique_call() {
    let mut f = unique_copy_fixture(false);
    f.body().rvalues[2] = Rvalue::Unary {
        op: hirsel_syntax::UnaryOp::Unique,
        value: copy(1),
    };
    let original = f.body().blocks[0].terminator.clone();
    let tail = f.body().blocks[1].clone();
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let mut second = original.clone();
    let Terminator::Call {
        returns: Some(CallReturn { destination, next }),
        ..
    } = &mut second
    else {
        panic!()
    };
    *destination = PlaceId(result);
    *next = BlockId(2);
    f.body().blocks[1] = block(vec![], second);
    f.body().blocks.push(tail);
    let Terminator::Call { callee, .. } = &mut f.body().blocks[0].terminator else {
        panic!()
    };
    *callee = Callee::Definition {
        path: path("ordinary_mut"),
        arguments: vec![],
    };
    f.signatures.insert(
        id("ordinary_mut"),
        function_signature(
            "ordinary_mut",
            vec![(PassingMode::Mut, string_kind())],
            TypeKind::Tuple(vec![].into()),
        ),
    );
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "call argument"
        }),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn sharing_a_counted_field_ends_the_whole_owners_uniqueness() {
    for deep_copy in [false, true] {
        let mut f = Fixture::unit();
        let x = f.slot(string_kind(), Storage::Local);
        let original = f.body().places[x as usize].ty;
        let mut row = interner().get(original).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let unique = f.definition.types.push(row);
        f.body().locals[x as usize].ty = unique;
        f.body().places[x as usize].ty = unique;
        f.assign(x, Rvalue::Constant(Constant::String("field".into())), 0);
        let owner = f.slot(TypeKind::Tuple(vec![unique].into()), Storage::Temporary);
        let original = f.body().places[owner as usize].ty;
        let mut row = interner().get(original).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let tuple = f.definition.types.push(row);
        f.body().locals[owner as usize].ty = tuple;
        f.body().places[owner as usize].ty = tuple;
        let unwind = f.cleanup(vec![CleanupAction::Drop {
            place: PlaceId(x),
            if_live: true,
        }]);
        f.assign(
            owner,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![ConstructField {
                    field: 0,
                    value: take(x),
                }],
                destination: Destination::Inferred,
            },
            unwind,
        );
        let field = f.body().places.len() as u32;
        f.body().places.push(Place {
            root: LocalId(owner),
            projection: vec![Projection::Field(0)].into(),
            ty: unique,
            loan: None,
            binding: None,
        });
        let alias = f.slot(string_kind(), Storage::Temporary);
        let rvalue = if deep_copy {
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value: copy(field),
            }
        } else {
            Rvalue::Use(copy(field))
        };
        let unwind = f.cleanup(vec![drop(owner)]);
        f.assign(alias, rvalue, unwind);
        f.body().result = tuple;
        let cleanup = f.cleanup(vec![drop(alias)]);
        f.body().blocks[0].terminator = ret(owner, cleanup);
        if deep_copy {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "value modes"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn scoped_closures_cannot_claim_unique_borrowed_counted_captures() {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let text = f.slot(string_kind(), Storage::Local);
    f.assign(
        text,
        Rvalue::Constant(Constant::String("borrowed".into())),
        0,
    );
    let text_ty = f.body().places[text as usize].ty;
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(text),
        mutable: false,
        if_inactive: false,
    });
    let view = f.through_loan(text, 0);
    let c = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
    let original = f.body().places[c as usize].ty;
    let mut row = interner().get(original).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    row.modes.relation = Relation::environment();
    let ty = f.definition.types.push(row.clone());
    let root = f.body().places[c as usize].root;
    f.body().places[c as usize].ty = ty;
    f.body().locals[root.0 as usize].ty = ty;
    let mut nested = empty_body(&mut f);
    add_capture(&mut nested, text_ty);
    f.definition.bodies.push(nested);
    let failure = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(text),
    ]);
    f.assign(
        c,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(view, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        failure,
    );
    let cleanup = f.cleanup(vec![
        drop(c),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(text),
    ]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "unique construction"
    }));
    row.modes.uniqueness = Uniqueness::Aliased;
    let ty = f.definition.types.push(row);
    f.body().places[c as usize].ty = ty;
    f.body().locals[root.0 as usize].ty = ty;
    f.valid();
}

#[test]
fn captured_generic_values_and_aliases_keep_the_outer_namespace() {
    for alias in [false, true] {
        captured_generic_copy(true, false, alias).valid();
        let f = captured_generic_copy(false, true, alias);
        assert!(
            f.kinds()
                .iter()
                .any(|k| matches!(k, ViolationKind::InvalidUse(_))),
            "{:?}",
            f.kinds()
        );
    }
}

#[test]
fn captured_generic_call_bounds_use_the_capture_and_alias_namespace() {
    for proven in [false, true] {
        for alias in [false, true] {
            let mut f = captured_generic_copy(proven, !proven, alias);
            let SignatureKind::Function(owner) =
                &mut f.signatures.get_mut(f.definition.path).unwrap().kind
            else {
                panic!()
            };
            owner.generics[0] = GenericParameter::Type {
                name: "T".into(),
                bounds: if proven {
                    vec![Bound::Inert].into()
                } else {
                    vec![].into()
                },
            };
            let c = f.body().places[2].ty;
            let mut row = interner().get(c).unwrap().clone();
            let TypeKind::Function(fun) = &mut row.kind else {
                panic!()
            };
            fun.generics[0] = GenericParameter::Type {
                name: "U".into(),
                bounds: if proven {
                    vec![].into()
                } else {
                    vec![Bound::Inert].into()
                },
            };
            fun.authority = if proven {
                AuthoritySummary::inert()
            } else {
                AuthoritySummary::Known(
                    vec![AuthorityTerm::TypeParameter(TypeParameterRef::body(
                        f.body().parameter_scope,
                        TypeParamId(0),
                    ))]
                    .into(),
                )
            };
            let c = f.definition.types.push(row);
            f.body().places[2].ty = c;
            f.body().locals[2].ty = c;
            let mut s = function_signature(
                "accept",
                vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::U64))],
                TypeKind::Tuple(vec![].into()),
            );
            let p = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let SignatureKind::Function(fun) = &mut s.kind else {
                panic!()
            };
            fun.generics = vec![GenericParameter::Type {
                name: "X".into(),
                bounds: vec![Bound::Inert].into(),
            }]
            .into();
            fun.parameters[0].ty = p;
            f.signatures.insert(id("accept"), s);
            let nested = &mut f.definition.bodies[1];
            nested.blocks[0].instructions.pop();
            edit_cleanup(nested, 0, |actions| {
                actions.clear();
            });
            let read = if alias { 2 } else { 1 };
            let p = nested.places[read].ty;
            let destination = add_capture(nested, nested.result);
            nested.locals[destination as usize].storage = Storage::Temporary;
            nested.blocks[0].terminator = Terminator::Call {
                callee: Callee::Definition {
                    path: path("accept"),
                    arguments: vec![GenericArgument::Type(p)],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: operand(read as u32, UseKind::Borrow),
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(destination),
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            };
            nested.rvalues.pop();
            if !alias {
                nested.rvalues.truncate(1);
            }
            nested.blocks[0].instructions.truncate(1);
            if alias {
                nested.blocks[0].instructions.push(inst(
                    InstructionKind::Assign {
                        destination: PlaceId(2),
                        value: RvalueId(1),
                    },
                    0,
                ));
            }
            if alias {
                nested.blocks[0].instructions.push(inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(0),
                        place: PlaceId(2),
                        mutable: false,
                        if_inactive: false,
                    },
                    0,
                ));
                let access = PlaceId(nested.places.len() as u32);
                nested.places.push({
                    let mut p = nested.places[2].clone();
                    p.loan = Some(LoanId(0));
                    p
                });
                let Terminator::Call {
                    arguments,
                    on_failure,
                    ..
                } = &mut nested.blocks[0].terminator
                else {
                    panic!()
                };
                arguments[0].value.place = access;
                *on_failure = CleanupId(1);
                nested.add_cleanup(vec![CleanupAction::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                }]);
            }
            nested
                .blocks
                .push(block(vec![], ret(0, if alias { 1 } else { 0 })));
            // These hand-built checked rows carry the conditional sendability
            // of the outer T, which now has an Inert rather than Plain bound.
            for body in &mut f.definition.bodies {
                let old_rows: Vec<_> = f
                    .definition
                    .types
                    .iter()
                    .map(|&id| (id, interner().row(id).clone()))
                    .collect();
                for (old, mut row) in old_rows {
                    let TypeKind::Parameter(p) = row.kind else {
                        continue;
                    };
                    if !matches!(p, TypeParameterRef::Body { .. }) {
                        continue;
                    }
                    row.modes.sendability = Sendability::DependsOn(vec![p].into());
                    let new = f.definition.types.push(row);
                    for local in &mut body.locals {
                        if local.ty == old {
                            local.ty = new;
                        }
                    }
                    for place in &mut body.places {
                        if place.ty == old {
                            place.ty = new;
                        }
                    }
                }
            }
            let signature = f.signatures.get_mut(f.definition.path).unwrap();
            let SignatureKind::Function(owner) = &mut signature.kind else {
                unreachable!()
            };
            let mut row = interner().get(owner.parameters[0].ty).unwrap().clone();
            row.modes.sendability =
                Sendability::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into());
            owner.parameters[0].ty = interner().intern(row);
            if proven {
                f.valid();
            } else {
                assert!(
                    f.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "generic bound"
                    }),
                    "{:?}",
                    f.kinds()
                );
            }
        }
    }
}

#[test]
fn capture_alias_names_are_proved_at_their_binding_instruction() {
    for before in [false, true] {
        let mut f = captured_generic_copy(true, false, false);
        let nested = &mut f.definition.bodies[1];
        let mut place = nested.places[1].clone();
        place.binding = Some(Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(99)),
            fields: vec![].into(),
        });
        let alias = PlaceId(nested.places.len() as u32);
        nested.places.push(place);
        let Rvalue::Use(value) = &mut nested.rvalues[1] else {
            panic!()
        };
        value.place = alias;
        nested.blocks[0].instructions.insert(
            if before { 1 } else { 2 },
            inst(
                InstructionKind::BindOrigin {
                    source: PlaceId(1),
                    place: alias,
                    kind: OriginBindingKind::Alias,
                },
                0,
            ),
        );
        if before {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::InvalidOrigin(alias)),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn captured_type_bounds_survive_adjacent_size_parameters() {
    let mut f = captured_generic_copy(true, false, true);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(f.definition.path).unwrap().kind
    else {
        panic!()
    };
    owner
        .generics
        .push(GenericParameter::Size { name: "n".into() });
    f.valid();
}

fn variant_payload_fixture() -> Fixture {
    let mut f = Fixture::unit();
    let u8t = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let u64t = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let v = param(
        &mut f,
        TypeKind::Variant(
            vec![
                Variant {
                    name: "A".into(),
                    fields: vec![(None, u8t)].into(),
                },
                Variant {
                    name: "B".into(),
                    fields: vec![(None, u64t)].into(),
                },
            ]
            .into(),
        ),
    );
    let vroot = f.body().places[v as usize].root;
    let payload = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: vroot,
        projection: vec![Projection::VariantField {
            variant: 1,
            field: 0,
        }]
        .into(),
        ty: u64t,
    });
    let out = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    f.assign(out, Rvalue::Use(copy(payload)), 0);
    owner_sig(&mut f, &[PassingMode::Borrow]);
    f
}

#[test]
fn p8c_variant_payload_read_without_tag_test() {
    let f = variant_payload_fixture();
    let payload = 2;
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidProjection(PlaceId(payload))),
        "{:?}",
        f.kinds()
    );
}

fn switched_payload_fixture(shared_target: bool, otherwise: bool) -> Fixture {
    let mut f = variant_payload_fixture();
    let read = f.body().blocks[0].instructions.pop().unwrap();
    f.body().blocks.push(block(vec![read], ret(0, 0)));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.body().blocks.push(block(vec![], Terminator::Unreachable));
    f.body().blocks[0].terminator = Terminator::Switch {
        value: operand(1, UseKind::Borrow),
        arms: if otherwise {
            vec![(0, BlockId(2))]
        } else {
            vec![
                (0, BlockId(if shared_target { 1 } else { 2 })),
                (1, BlockId(1)),
            ]
        },
        otherwise: BlockId(if otherwise { 1 } else { 3 }),
    };
    if otherwise || shared_target {
        f.body().blocks.remove(3);
        if shared_target {
            f.body().blocks[0].terminator = Terminator::Switch {
                value: operand(1, UseKind::Borrow),
                arms: vec![(0, BlockId(1)), (1, BlockId(1))],
                otherwise: BlockId(2),
            };
        }
    }
    f
}

#[test]
fn payload_access_requires_one_case_on_every_incoming_edge() {
    switched_payload_fixture(false, false).valid();
    switched_payload_fixture(false, true).valid();
    let f = switched_payload_fixture(true, false);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidProjection(PlaceId(2))),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn mutation_invalidates_the_switched_case() {
    let mut f = switched_payload_fixture(false, false);
    owner_sig(&mut f, &[PassingMode::Mut]);
    let mut callee = f.signatures.get(f.definition.path).unwrap().clone();
    callee.definition = id("change");
    f.signatures.insert(id("change"), callee);
    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let read = f.body().blocks[1].instructions.pop().unwrap();
    f.body().blocks.push(block(vec![read], ret(0, 0)));
    f.body().blocks[1].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("change"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(1, UseKind::Mut),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(4),
        }),
        on_failure: CleanupId(0),
    };
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidProjection(PlaceId(2))),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn different_cases_share_the_same_payload_storage_for_loans() {
    let mut f = switched_payload_fixture(false, false);
    owner_sig(&mut f, &[PassingMode::Mut]);
    let root = f.body().places[1].root;
    let variant_ty = f.body().places[1].ty;
    let a_type = match interner().get(variant_ty).unwrap().kind.clone() {
        TypeKind::Variant(v) => v[0].fields[0].1,
        _ => panic!(),
    };
    let a = f.body().places.len() as u32;
    f.body().places.push(Place {
        root,
        projection: vec![Projection::VariantField {
            variant: 0,
            field: 0,
        }]
        .into(),
        ty: a_type,
        binding: None,
        loan: None,
    });
    let read = f.body().blocks[1].instructions.pop().unwrap();
    f.body().blocks[1].instructions = vec![
        inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(2),
                mutable: true,
                if_inactive: false,
            },
            0,
        ),
        inst(
            InstructionKind::BeginLoan {
                id: LoanId(1),
                place: PlaceId(a),
                mutable: false,
                if_inactive: false,
            },
            1,
        ),
        read,
    ];
    f.body().add_cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    f.body().add_cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    f.body().blocks[1].terminator = ret(0, 2);
    assert!(
        f.kinds().iter().any(|k| matches!(
            k,
            ViolationKind::LoanConflict {
                loan: LoanId(0),
                ..
            }
        )),
        "{:?}",
        f.kinds()
    );
}

fn constructed_payload_fixture(case: u32, transfer: bool) -> Fixture {
    let mut f = variant_payload_fixture();
    let read = f.body().blocks[0].instructions.pop().unwrap();
    let representation = f.body().places[1].ty;
    let choice = path("Choice");
    let named = f.definition.types.push(ty(TypeKind::Named {
        definition: DefId::intern(&choice),
        arguments: vec![].into(),
    }));
    f.body().locals[1].ty = named;
    f.body().locals[1].storage = Storage::Local;
    f.body().parameters.clear();
    f.body().places[1].ty = named;
    f.signatures.insert(
        &choice,
        Signature {
            definition: DefId::intern(&choice),
            metadata: Default::default(),
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
                layout: Default::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
                captures: None,
                destructor_allocation: Allocation::NoAlloc,
            })),
            fingerprint: ContentHash::default(),
        },
    );
    let field = f.slot(
        TypeKind::Primitive(if case == 0 {
            Primitive::U8
        } else {
            Primitive::U64
        }),
        Storage::Temporary,
    );
    f.assign(field, Rvalue::Constant(Constant::Integer(7)), 0);
    f.assign(
        1,
        Rvalue::Construct {
            constructor: Constructor::Variant {
                definition: choice,
                variant: case,
            },
            fields: vec![ConstructField {
                field: 0,
                value: take(field),
            }],
            destination: Destination::Inferred,
        },
        0,
    );
    if transfer {
        let target = f.slot(
            TypeKind::Named {
                definition: id("Choice"),
                arguments: vec![].into(),
            },
            Storage::Local,
        );
        f.assign(target, Rvalue::Use(copy(1)), 0);
        let target_root = f.body().places[target as usize].root;
        f.body().places[2].root = target_root;
    }
    f.body().blocks[0].instructions.push(read);
    owner_sig(&mut f, &[]);
    f
}

#[test]
fn construction_and_copy_prove_the_selected_case() {
    for transfer in [false, true] {
        constructed_payload_fixture(1, transfer).valid();
        let f = constructed_payload_fixture(0, transfer);
        assert!(
            f.kinds()
                .contains(&ViolationKind::InvalidProjection(PlaceId(2))),
            "{:?}",
            f.kinds()
        );
    }
}

#[test]
fn mutating_an_original_does_not_invalidate_an_owned_variant_copy() {
    let mut f = constructed_payload_fixture(1, true);
    let read = f.body().blocks[0].instructions.pop().unwrap();
    let callee = function_signature(
        "change",
        vec![(
            PassingMode::Mut,
            TypeKind::Named {
                definition: id("Choice"),
                arguments: vec![].into(),
            },
        )],
        TypeKind::Tuple(vec![].into()),
    );
    f.signatures.insert(id("change"), callee);
    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable: true,
        if_inactive: false,
    });
    let mut access = f.body().places[1].clone();
    access.loan = Some(LoanId(0));
    let access_place = PlaceId(f.body().places.len() as u32);
    f.body().places.push(access);
    let cleanup = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("change"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(access_place.0, UseKind::Mut),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(cleanup),
    };
    f.body().blocks.push(block(vec![read], ret(0, cleanup)));
    f.valid();
}

fn indexed_variant_fixture(other_index: bool, change_index: bool) -> Fixture {
    let mut f = variant_payload_fixture();
    let read = f.body().blocks[0].instructions.pop().unwrap();
    let variant = f.body().places[1].ty;
    let array = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Array,
        arguments: vec![GenericArgument::Type(variant)].into(),
    }));
    f.body().locals[1].ty = array;
    f.body().places[1].ty = array;
    let i = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Local);
    let j = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Local);
    f.assign(i, Rvalue::Constant(Constant::Integer(0)), 0);
    f.assign(j, Rvalue::Constant(Constant::Integer(1)), 0);
    let i_root = f.body().places[i as usize].root;
    let j_root = f.body().places[j as usize].root;
    f.body().places[2].projection.insert(
        0,
        Projection::Index(if other_index { j_root } else { i_root }),
    );
    let scrutinee = PlaceId(f.body().places.len() as u32);
    f.body().places.push(Place {
        root: LocalId(1),
        projection: vec![Projection::Index(i_root)].into(),
        ty: variant,
        binding: None,
        loan: None,
    });
    f.body().blocks.push(block(vec![read], ret(0, 0)));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.body().blocks.push(block(vec![], Terminator::Unreachable));
    f.body().blocks[0].terminator = Terminator::Switch {
        value: operand(scrutinee.0, UseKind::Borrow),
        arms: vec![(0, BlockId(2)), (1, BlockId(1))],
        otherwise: BlockId(3),
    };
    if change_index {
        let fresh = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        let displaced = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        let value = RvalueId(f.body().rvalues.len() as u32);
        f.body()
            .rvalues
            .push(Rvalue::Constant(Constant::Integer(1)));
        f.body().blocks[1].instructions.insert(
            0,
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(fresh),
                    value,
                },
                0,
            ),
        );
        f.body().blocks[1].instructions.insert(
            1,
            inst(
                InstructionKind::Replace {
                    destination: PlaceId(i),
                    value: take(fresh),
                    displaced: PlaceId(displaced),
                    if_live: false,
                },
                0,
            ),
        );
    }
    owner_sig(&mut f, &[PassingMode::Borrow]);
    f
}

#[test]
fn switched_element_cases_require_the_same_stable_index_value() {
    indexed_variant_fixture(false, false).valid();
    for (other, changed) in [(true, false), (false, true)] {
        let f = indexed_variant_fixture(other, changed);
        assert!(
            f.kinds()
                .contains(&ViolationKind::InvalidProjection(PlaceId(2))),
            "{:?}",
            f.kinds()
        );
    }
}

#[test]
fn comparison_of_captured_generics_requires_the_outer_data_proof() {
    for proven in [true, false] {
        let mut f = captured_generic_copy(true, true, false);
        let SignatureKind::Function(owner) =
            &mut f.signatures.get_mut(f.definition.path).unwrap().kind
        else {
            panic!()
        };
        let GenericParameter::Type { bounds, .. } = &mut owner.generics[0] else {
            panic!()
        };
        *bounds = vec![Bound::Copyable, Bound::Inert].into();
        if proven {
            bounds.push(Bound::Data);
        }
        let ct = f.body().places[2].ty;
        let mut row = interner().get(ct).unwrap().clone();
        if let TypeKind::Function(inner) = &mut row.kind {
            // This control tests equality evidence after replacing Plain with
            // Data. It makes no independent cleanup-allocation promise.
            inner.allocation = Allocation::MayAllocate;
            let GenericParameter::Type { bounds, .. } = &mut inner.generics[0] else {
                panic!()
            };
            *bounds = vec![Bound::Data].into();
        }
        let ct = f.definition.types.push(row);
        f.body().places[2].ty = ct;
        f.body().locals[2].ty = ct;
        let b = &mut f.definition.bodies[1];
        let bool_ty = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::Bool)));
        let result = add_capture(b, bool_ty);
        b.locals[result as usize].storage = Storage::Temporary;
        let rv = RvalueId(b.rvalues.len() as u32);
        b.rvalues.push(Rvalue::Binary {
            op: hirsel_syntax::BinaryOp::Eq,
            left: copy(1),
            right: copy(1),
        });
        b.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(result),
                value: rv,
            },
            0,
        ));
        edit_cleanup(b, 0, |actions| {
            actions.push(drop(result));
        });
        if proven {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::InvalidOperator),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn discriminator_proofs_exclude_contradictions_and_preserve_default_audits() {
    for audit_default in [false, true] {
        let mut f = switched_payload_fixture(false, false);
        let read = f.body().blocks[1].instructions.pop().unwrap();
        f.body().blocks.push(block(vec![read], ret(0, 0)));
        let root = f.body().places[1].root;
        f.body().blocks.push(block(
            if audit_default {
                vec![inst(InstructionKind::StorageLive(root), 0)]
            } else {
                vec![]
            },
            Terminator::Goto(BlockId(6)),
        ));
        f.body().blocks.push(block(vec![], Terminator::Unreachable));
        f.body().blocks[1].terminator = Terminator::Switch {
            value: operand(1, UseKind::Borrow),
            arms: if audit_default {
                vec![(0, BlockId(4)), (1, BlockId(4))]
            } else {
                vec![(1, BlockId(4))]
            },
            otherwise: BlockId(5),
        };
        if audit_default {
            assert!(
                f.kinds().contains(&ViolationKind::WriteToLive(PlaceId(1))),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn copied_flat_capture_aliases_own_independent_bytes() {
    for alias in [false, true] {
        let mut f = captured_generic_copy(true, false, alias);
        let SignatureKind::Function(owner) =
            &mut f.signatures.get_mut(f.definition.path).unwrap().kind
        else {
            panic!()
        };
        owner.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Flat].into(),
        }]
        .into();
        // Flat tests independent byte ownership, without a NoAllocDrop bound.
        let original = f.body().places[2].ty;
        let mut row = interner().get(original).unwrap().clone();
        if let TypeKind::Function(function) = &mut row.kind {
            function.allocation = Allocation::MayAllocate;
        }
        let row = f.definition.types.push(row);
        f.body().places[2].ty = row;
        f.body().locals[2].ty = row;
        f.valid();
    }
}

#[test]
fn copied_local_flat_capture_aliases_cannot_claim_global_storage() {
    let mut f = captured_generic_copy(true, false, true);
    for (body, places) in [(0, vec![1]), (1, vec![1, 2])] {
        let body = &mut f.definition.bodies[body];
        let mut local = interner().get(body.places[1].ty).unwrap().clone();
        local.modes.relation = Relation::environment();
        let local = f.definition.types.push(local);
        for place in places {
            body.places[place].ty = local;
            body.locals[body.places[place].root.0 as usize].ty = local;
        }
    }
    owner_sig(&mut f, &[PassingMode::Borrow]);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(f.definition.path).unwrap().kind
    else {
        panic!()
    };
    owner.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Flat].into(),
    }]
    .into();
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "assignment"
        }]
    );
}

fn flat_unique_copy_fixture() -> Fixture {
    let mut f = unique_copy_fixture(false);
    let body = &mut f.definition.bodies[0];
    let originals: Vec<_> = f
        .definition
        .types
        .iter()
        .map(|&id| (id, interner().row(id).clone()))
        .collect();
    for (old, mut row) in originals {
        if row.kind != string_kind() {
            continue;
        }
        row.kind =
            TypeKind::Parameter(TypeParameterRef::body(body.parameter_scope, TypeParamId(0)));
        let new = f.definition.types.push(row);
        for place in &mut body.places {
            if place.ty == old {
                place.ty = new;
            }
        }
        for local in &mut body.locals {
            if local.ty == old {
                local.ty = new;
            }
        }
    }
    body.blocks[0].instructions.remove(1);
    body.rvalues.remove(1);
    for instruction in &mut body.blocks[0].instructions {
        if let InstructionKind::Assign { value, .. } = &mut instruction.kind {
            if value.0 > 1 {
                value.0 -= 1;
            }
        }
    }
    let root = body.places[1].root;
    body.locals[root.0 as usize].storage = Storage::Parameter;
    body.parameters.push(root);
    owner_sig(&mut f, &[PassingMode::Take]);
    for name in ["main", "require_unique"] {
        let signature = f.signatures.get_mut(&path(name)).unwrap();
        let SignatureKind::Function(fun) = &mut signature.kind else {
            panic!()
        };
        fun.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Flat].into(),
        }]
        .into();
        if name == "require_unique" {
            let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
            row.kind = TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0)));
            fun.parameters[0].ty = interner().intern(row);
        }
    }
    let actual = f.body().places[1].ty;
    let Terminator::Call {
        callee: Callee::Definition { arguments, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    *arguments = vec![GenericArgument::Type(actual)];
    f
}

#[test]
fn flat_copies_preserve_source_uniqueness_and_own_unique_results() {
    for unique_result in [false, true] {
        let mut f = flat_unique_copy_fixture();
        if unique_result {
            let original = f.body().places[2].ty;
            let mut row = interner().get(original).unwrap().clone();
            row.modes.uniqueness = Uniqueness::Unique;
            let ty = f.definition.types.push(row);
            let root = f.body().places[2].root;
            f.body().places[2].ty = ty;
            f.body().locals[root.0 as usize].ty = ty;
        }
        f.valid();
    }
}

#[test]
fn flat_call_bounds_require_flat_roots_instead_of_compatible_fields() {
    for (explicit_flat, alias) in [(false, false), (true, false), (false, true)] {
        let mut flat = capability_signature("Flat");
        let text = interner().intern(ty(string_kind()));
        let representation = interner().intern(ty(TypeKind::Tuple(vec![text].into())));
        let SignatureKind::Type(declaration) = &mut flat.kind else {
            panic!()
        };
        declaration.capability = None;
        declaration.bounds.clear();
        declaration.layout.flat = true;
        declaration.authority = AuthoritySummary::inert();
        declaration.representation = Some(representation);
        let mut wrapper = capability_signature("Wrapper");
        wrapper.metadata.alias = alias;
        let flat_ty = interner().intern(ty(TypeKind::Named {
            definition: id("Flat"),
            arguments: vec![].into(),
        }));
        let representation = if alias {
            flat_ty
        } else {
            interner().intern(ty(TypeKind::Tuple(vec![flat_ty].into())))
        };
        let SignatureKind::Type(declaration) = &mut wrapper.kind else {
            panic!()
        };
        declaration.capability = None;
        declaration.bounds.clear();
        declaration.layout.flat = explicit_flat;
        declaration.authority = AuthoritySummary::inert();
        declaration.representation = Some(representation);
        let mut f = generic_call_fixture(
            Bound::Flat,
            TypeKind::Named {
                definition: id("Wrapper"),
                arguments: vec![].into(),
            },
        );
        f.signatures.insert(id("Flat"), flat);
        f.signatures.insert(id("Wrapper"), wrapper);
        if explicit_flat || alias {
            let original = f.body().places[1].ty;
            let mut row = interner().get(original).unwrap().clone();
            row.modes.sendability = Sendability::Sendable;
            let ty = f.definition.types.push(row);
            f.body().places[1].ty = ty;
            f.body().locals[1].ty = ty;
            if let Terminator::Call {
                callee: Callee::Definition { arguments, .. },
                ..
            } = &mut f.body().blocks[0].terminator
            {
                arguments[0] = GenericArgument::Type(ty);
            }
            owner_sig(&mut f, &[PassingMode::Take]);
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "generic bound"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
    for explicit_flat in [false, true] {
        let mut f = generic_call_fixture(
            Bound::Flat,
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        );
        let source = f.body().places[1].ty;
        let mut row = interner().get(source).unwrap().clone();
        row.modes.sendability = Sendability::Sendable;
        let source = f.definition.types.push(row);
        f.body().places[1].ty = source;
        f.body().locals[1].ty = source;
        if let Terminator::Call {
            callee: Callee::Definition { arguments, .. },
            ..
        } = &mut f.body().blocks[0].terminator
        {
            arguments[0] = GenericArgument::Type(source);
        }
        owner_sig(&mut f, &[PassingMode::Take]);
        let SignatureKind::Function(owner) =
            &mut f.signatures.get_mut(f.definition.path).unwrap().kind
        else {
            panic!()
        };
        owner.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![if explicit_flat {
                Bound::Flat
            } else {
                Bound::Plain
            }]
            .into(),
        }]
        .into();
        if explicit_flat {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "generic bound"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn flat_copies_are_owned_fields_of_unique_aggregates() {
    let mut f = flat_unique_copy_fixture();
    let field = f.body().places[1].ty;
    let aggregate = f.slot(TypeKind::Tuple(vec![field].into()), Storage::Local);
    let root = f.body().places[aggregate as usize].root;
    let original = f.body().places[aggregate as usize].ty;
    let mut row = interner().get(original).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let ty = f.definition.types.push(row);
    f.body().places[aggregate as usize].ty = ty;
    f.body().locals[root.0 as usize].ty = ty;
    let rv = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Construct {
        constructor: Constructor::Tuple,
        fields: vec![ConstructField {
            field: 0,
            value: copy(1),
        }],
        destination: Destination::Inferred,
    });
    f.body().blocks[0].instructions.insert(
        2,
        inst(
            InstructionKind::Assign {
                destination: PlaceId(aggregate),
                value: rv,
            },
            2,
        ),
    );
    for id in 2..f.body().cleanups.len() {
        edit_cleanup(f.body(), id as u32, |actions| {
            let first_drop = actions
                .iter()
                .position(|action| matches!(action, CleanupAction::Drop { .. }))
                .unwrap();
            actions.insert(first_drop, drop(aggregate));
        });
    }
    f.valid();
}

#[test]
fn flat_copied_aliases_prove_unique_assignments_fields_results_and_arguments() {
    for relation in ["assignment", "field", "return", "argument"] {
        let mut f = Fixture::unit();
        let x = param(
            &mut f,
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        );
        let source = f.body().places[x as usize].ty;
        let mut row = interner().get(source).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        row.modes.sendability = Sendability::Sendable;
        let unique = f.definition.types.push(row);
        let exit = f.cleanup(vec![drop(x)]);
        match relation {
            "assignment" | "field" => {
                let y = f.slot(
                    if relation == "assignment" {
                        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0)))
                    } else {
                        TypeKind::Tuple(vec![unique].into())
                    },
                    Storage::Local,
                );
                let ty = if relation == "assignment" {
                    unique
                } else {
                    let original = f.body().places[y as usize].ty;
                    let mut row = interner().get(original).unwrap().clone();
                    row.modes.uniqueness = Uniqueness::Unique;
                    row.modes.sendability = Sendability::Sendable;
                    f.definition.types.push(row)
                };
                f.body().places[y as usize].ty = ty;
                f.body().locals[y as usize].ty = ty;
                f.assign(
                    y,
                    if relation == "assignment" {
                        Rvalue::Use(copy(x))
                    } else {
                        Rvalue::Construct {
                            constructor: Constructor::Tuple,
                            fields: vec![ConstructField {
                                field: 0,
                                value: copy(x),
                            }],
                            destination: Destination::Inferred,
                        }
                    },
                    exit,
                );
                let cleanup = f.cleanup(vec![drop(y), drop(x)]);
                f.body().blocks[0].terminator = ret(0, cleanup);
            }
            "return" => {
                f.body().result = unique;
                f.body().blocks[0].terminator = Terminator::Return {
                    value: copy(x),
                    cleanup: CleanupId(exit),
                };
            }
            "argument" => {
                let mut signature = function_signature(
                    "callee",
                    vec![(
                        PassingMode::Borrow,
                        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
                    )],
                    TypeKind::Tuple(vec![].into()),
                );
                let SignatureKind::Function(fun) = &mut signature.kind else {
                    panic!()
                };
                fun.generics = vec![GenericParameter::Type {
                    name: "U".into(),
                    bounds: vec![Bound::Flat].into(),
                }]
                .into();
                let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
                row.modes.uniqueness = Uniqueness::Unique;
                row.modes.sendability = Sendability::Sendable;
                fun.parameters[0].ty = interner().intern(row);
                f.signatures.insert(id("callee"), signature);
                let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
                f.body().blocks[0].terminator = Terminator::Call {
                    callee: Callee::Definition {
                        path: path("callee"),
                        arguments: vec![GenericArgument::Type(source)],
                    },
                    arguments: vec![CallArgument {
                        parameter: ParamId(0),
                        value: copy(x),
                    }],
                    returns: Some(CallReturn {
                        destination: PlaceId(result),
                        next: BlockId(1),
                    }),
                    on_failure: CleanupId(exit),
                };
                f.body().blocks.push(block(vec![], ret(result, exit)));
            }
            _ => unreachable!(),
        }
        owner_sig(&mut f, &[PassingMode::Take]);
        let SignatureKind::Function(owner) =
            &mut f.signatures.get_mut(f.definition.path).unwrap().kind
        else {
            panic!()
        };
        owner.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Flat].into(),
        }]
        .into();
        f.valid();
    }
}

fn push_rvalue(f: &mut Fixture, r: Rvalue) -> RvalueId {
    let body = &mut f.definition.bodies[0];
    let id = RvalueId(body.rvalues.len() as u32);
    body.rvalues.push(r);
    id
}
/// N-H2: ownership is a may-fact at joins: one path stores a borrowed view, the other an owned value,
/// and the join drops the slot unconditionally.
#[test]
fn n2_owned_may_join_drops_a_borrowed_view() {
    let mut f = Fixture::unit();
    let x = param(&mut f, string_kind());
    let c = param(&mut f, TypeKind::Primitive(Primitive::Bool));
    let t = f.slot(string_kind(), Storage::Temporary);
    let r1 = push_rvalue(&mut f, Rvalue::Use(operand(x, UseKind::Borrow)));
    let r2 = push_rvalue(&mut f, Rvalue::Constant(Constant::String("own".into())));
    let exit = f.cleanup(vec![drop(t)]);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(c),
        yes: BlockId(1),
        no: BlockId(2),
    };
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::Assign {
                destination: PlaceId(t),
                value: r1,
            },
            0,
        )],
        Terminator::Goto(BlockId(3)),
    ));
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::Assign {
                destination: PlaceId(t),
                value: r2,
            },
            0,
        )],
        Terminator::Goto(BlockId(3)),
    ));
    f.body().blocks.push(block(vec![], ret(0, exit)));
    owner_sig(&mut f, &[PassingMode::Borrow, PassingMode::Borrow]);
    f.broken(vec![
        ViolationKind::BorrowedMove(PlaceId(t)),
        ViolationKind::CleanupMismatch {
            cleanup: CleanupId(exit),
            expected: vec![],
            actual: vec![LocalId(t)],
        },
    ]);
    // Two owned alternatives are releasable at the same join.
    f.body().rvalues[r1.0 as usize] = Rvalue::Constant(Constant::String("other".into()));
    f.valid();
    // A dead alternative needs a conditional drop, not conflicting ownership.
    f.body().blocks[1].instructions.clear();
    f.body().rvalues.remove(r1.0 as usize);
    if let InstructionKind::Assign { value, .. } = &mut f.body().blocks[2].instructions[0].kind {
        *value = RvalueId(r1.0);
    }
    f.valid();
}
/// N2b: straight-line: a temporary holding a borrowed view of a borrowed parameter is dropped.
#[test]
fn n2b_drop_of_a_borrowed_view_slot() {
    for (resource, instruction) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut f = Fixture::unit();
        if resource {
            let mut s = capability_signature("Trace");
            if let SignatureKind::Type(t) = &mut s.kind {
                t.capability = None;
                t.resource = true;
                t.authority = AuthoritySummary::inert();
                t.representation = Some(interner().intern(ty(TypeKind::Tuple(vec![].into()))));
            }
            f.signatures.insert(id("Trace"), s);
        }
        let kind = if resource {
            TypeKind::Named {
                definition: id("Trace"),
                arguments: vec![].into(),
            }
        } else {
            string_kind()
        };
        let x = param(&mut f, kind.clone());
        let t = f.slot(kind, Storage::Temporary);
        f.assign(t, Rvalue::Use(operand(x, UseKind::Borrow)), 0);
        if instruction {
            f.emit(InstructionKind::Drop {
                place: PlaceId(t),
                if_live: false,
            });
        } else {
            let exit = f.cleanup(vec![drop(t)]);
            f.body().blocks[0].terminator = ret(0, exit);
        }
        owner_sig(&mut f, &[PassingMode::Borrow]);
        if instruction {
            f.broken(vec![ViolationKind::BorrowedMove(PlaceId(t))]);
        } else {
            f.broken(vec![
                ViolationKind::BorrowedMove(PlaceId(t)),
                ViolationKind::CleanupMismatch {
                    cleanup: CleanupId(1),
                    expected: vec![],
                    actual: vec![LocalId(t)],
                },
            ]);
        }
        // RunCleanup uses the same ownership rule as explicit Drop.
        if instruction {
            let cleanup = f.cleanup(vec![drop(t)]);
            f.body().blocks[0].instructions.pop();
            f.emit(InstructionKind::RunCleanup(CleanupId(cleanup)));
            f.broken(vec![ViolationKind::BorrowedMove(PlaceId(t))]);
        }
    }
}

#[test]
fn inert_bounds_use_authority_evidence_instead_of_plain_layout() {
    for bound in [
        Bound::Plain,
        Bound::Inert,
        Bound::Data,
        Bound::Key,
        Bound::Flat,
    ] {
        let mut f = generic_call_fixture(
            Bound::Inert,
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        );
        let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            panic!()
        };
        owner.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![bound].into(),
        }]
        .into();
        assert_eq!(
            f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "generic bound"
            }),
            bound == Bound::Plain,
            "{bound:?}: {:?}",
            f.kinds()
        );
    }
}

fn two_case(f: &mut Fixture) -> TypeKind {
    let u8t = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let u64t = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    TypeKind::Variant(
        vec![
            Variant {
                name: "A".into(),
                fields: vec![(None, u8t)].into(),
            },
            Variant {
                name: "B".into(),
                fields: vec![(None, u64t)].into(),
            },
        ]
        .into(),
    )
}
/// N-H1: a Many closure captures a variant by Mut after a tag test; its body reads the tested payload and
/// then replaces the variant. The second call re-enters with the creation-time tag fact.
#[test]
fn n1_mut_captured_variant_keeps_creation_time_tag_across_calls() {
    for mutable_capture in [true, false] {
        let mutate = mutable_capture;
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let kind = two_case(&mut f);
        let c = param(&mut f, kind.clone());
        let d = param(&mut f, kind.clone());
        let vty = f.body().places[c as usize].ty;
        let u64t = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U64)));
        // nested body
        let mut nested = empty_body(&mut f);
        let cap = add_capture(&mut nested, vty);
        let other = add_capture(&mut nested, vty);
        let cap_root = nested.places[cap as usize].root;
        nested.places.push(Place {
            loan: None,
            binding: None,
            root: cap_root,
            projection: vec![Projection::VariantField {
                variant: 1,
                field: 0,
            }]
            .into(),
            ty: u64t,
        });
        let payload = nested.places.len() as u32 - 1;
        let l = nested.locals.len() as u32;
        nested.locals.push(Local {
            ty: u64t,
            storage: Storage::Temporary,
            scope: ScopeId(0),
            span: Span::default(),
        });
        nested.places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(l),
            projection: vec![].into(),
            ty: u64t,
        });
        let out = nested.places.len() as u32 - 1;
        let l2 = nested.locals.len() as u32;
        nested.locals.push(Local {
            ty: vty,
            storage: Storage::Temporary,
            scope: ScopeId(0),
            span: Span::default(),
        });
        nested.places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(l2),
            projection: vec![].into(),
            ty: vty,
        });
        let old = nested.places.len() as u32 - 1;
        nested.rvalues.push(Rvalue::Use(copy(payload)));
        nested.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(out),
                value: RvalueId(1),
            },
            0,
        ));
        if mutate {
            nested.blocks[0].instructions.push(inst(
                InstructionKind::Replace {
                    destination: PlaceId(cap),
                    value: copy(other),
                    displaced: PlaceId(old),
                    if_live: false,
                },
                0,
            ));
            nested.blocks[0].instructions.push(inst(
                InstructionKind::Drop {
                    place: PlaceId(old),
                    if_live: false,
                },
                0,
            ));
        }
        f.definition.bodies.push(nested);
        // outer
        let clo = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
        {
            let ct = f.body().places[clo as usize].ty;
            let mut row = interner().get(ct).unwrap().clone();
            row.modes.relation = Relation::environment();
            let ct2 = f.definition.types.push(row);
            let cl = f.body().places[clo as usize].root;
            f.body().locals[cl.0 as usize].ty = ct2;
            f.body().places[clo as usize].ty = ct2;
        }
        let via_c = f.through_loan(c, 0);
        let via_clo = f.through_loan(clo, 1);
        let rc = push_rvalue(
            &mut f,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![
                    operand(
                        via_c,
                        if mutable_capture {
                            UseKind::Mut
                        } else {
                            UseKind::Borrow
                        },
                    ),
                    copy(d),
                ],
                destination: Destination::Inferred,
            },
        );
        let r1 = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let r2 = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let end0 = CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        };
        let end1 = CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        };
        let fail0 = f.cleanup(vec![end0.clone()]);
        let exit = f.cleanup(vec![end1.clone(), drop(clo), end0.clone()]);
        f.body().blocks[0].terminator = Terminator::Switch {
            value: operand(c, UseKind::Borrow),
            arms: vec![(0, BlockId(1)), (1, BlockId(2))],
            otherwise: BlockId(3),
        };
        f.body().blocks.push(block(vec![], ret(0, 0)));
        f.body().blocks.push(block(
            vec![
                inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(0),
                        place: PlaceId(c),
                        mutable: mutable_capture,
                        if_inactive: false,
                    },
                    0,
                ),
                inst(
                    InstructionKind::Assign {
                        destination: PlaceId(clo),
                        value: rc,
                    },
                    fail0,
                ),
                inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(1),
                        place: PlaceId(clo),
                        mutable: false,
                        if_inactive: false,
                    },
                    exit,
                ),
            ],
            Terminator::Call {
                callee: Callee::Indirect {
                    forward_caller: false,
                    value: operand(via_clo, UseKind::Borrow),
                    arguments: vec![],
                },
                arguments: vec![],
                returns: Some(CallReturn {
                    destination: PlaceId(r1),
                    next: BlockId(4),
                }),
                on_failure: CleanupId(exit),
            },
        ));
        f.body().blocks.push(block(vec![], Terminator::Unreachable));
        f.body().blocks.push(block(
            vec![],
            Terminator::Call {
                callee: Callee::Indirect {
                    forward_caller: false,
                    value: operand(via_clo, UseKind::Borrow),
                    arguments: vec![],
                },
                arguments: vec![],
                returns: Some(CallReturn {
                    destination: PlaceId(r2),
                    next: BlockId(5),
                }),
                on_failure: CleanupId(exit),
            },
        ));
        f.body().blocks.push(block(vec![], ret(0, exit)));
        owner_sig(
            &mut f,
            &[
                if mutable_capture {
                    PassingMode::Mut
                } else {
                    PassingMode::Borrow
                },
                PassingMode::Borrow,
            ],
        );
        let kinds = f.kinds();
        if mutable_capture {
            assert!(
                kinds.contains(&ViolationKind::InvalidProjection(PlaceId(payload))),
                "{kinds:?}"
            );
        } else {
            assert!(kinds.is_empty(), "{kinds:?}");
        }
    }
}

/// N-H5: a switch arm contradicted by a proven tag is never flow-checked.
#[test]
fn n5_arm_contradicting_a_proven_tag_is_never_flow_checked() {
    let mut f = Fixture::unit();
    let kind = two_case(&mut f);
    let v = param(&mut f, kind);
    let s = f.slot(string_kind(), Storage::Local);
    let t = f.slot(string_kind(), Storage::Local);
    let r0 = push_rvalue(&mut f, Rvalue::Use(take(s)));
    let r1 = push_rvalue(&mut f, Rvalue::Use(take(s)));
    // b0: switch v {0->b1, 1->b2} default b3(sentinel)
    f.body().blocks[0].terminator = Terminator::Switch {
        value: operand(v, UseKind::Borrow),
        arms: vec![(0, BlockId(1)), (1, BlockId(2))],
        otherwise: BlockId(3),
    };
    // b1 (tag = A): switch again {0->b2, 1->b4 garbage}
    f.body().blocks.push(block(
        vec![],
        Terminator::Switch {
            value: operand(v, UseKind::Borrow),
            arms: vec![(0, BlockId(2)), (1, BlockId(4))],
            otherwise: BlockId(3),
        },
    ));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.body().blocks.push(block(vec![], Terminator::Unreachable));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(t),
                    value: r0,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(t),
                    if_live: false,
                },
                0,
            ),
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(t),
                    value: r1,
                },
                0,
            ),
        ],
        ret(0, 0),
    ));
    owner_sig(&mut f, &[PassingMode::Borrow]);
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::ReadBeforeInit(PlaceId(s))),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&ViolationKind::DoubleMove(PlaceId(s))),
        "{kinds:?}"
    );
    // control: the garbage block is the arm of the first switch
    if let Terminator::Switch { arms, .. } = &mut f.body().blocks[0].terminator {
        arms[0].1 = BlockId(4);
    }
    f.body().blocks[1] = block(vec![], Terminator::Unreachable);
    let reachable = f.kinds();
    assert_eq!(kinds, reachable);
    // The empty unreachable sentinel is the sole permitted unaudited target.
    if let Terminator::Switch { arms, .. } = &mut f.body().blocks[0].terminator {
        arms[0].1 = BlockId(1);
    }
    f.body().blocks[1].terminator = Terminator::Switch {
        value: operand(v, UseKind::Borrow),
        arms: vec![(0, BlockId(2)), (1, BlockId(4))],
        otherwise: BlockId(3),
    };
    f.body().blocks[4] = block(vec![], Terminator::Unreachable);
    f.body().rvalues.truncate(1);
    f.valid();
}

// Local result annotations identify sources; every returned counted value is owned.
#[test]
fn local_results_cannot_return_borrowed_counted_operands() {
    for usage in [UseKind::Borrow, UseKind::Mut, UseKind::Copy] {
        for temporary in [false, true] {
            let mut f = Fixture::unit();
            let x = param(&mut f, string_kind());
            let original = f.body().places[x as usize].ty;
            let mut local = interner().get(original).unwrap().clone();
            local.modes.relation = Relation::environment();
            let local = f.definition.types.push(local);
            f.body().locals[x as usize].ty = local;
            f.body().places[x as usize].ty = local;
            let mut result = interner().get(local).unwrap().clone();
            result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
            let result = f.definition.types.push(result);
            f.body().result = result;
            owner_sig(&mut f, &[PassingMode::Borrow]);
            let scope = f.body().parameter_scope;
            f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
            let returned = if temporary {
                let t = f.slot(string_kind(), Storage::Temporary);
                f.body().locals[t as usize].ty = local;
                f.body().places[t as usize].ty = local;
                f.assign(t, Rvalue::Use(operand(x, UseKind::Borrow)), 0);
                t
            } else {
                x
            };
            f.body().blocks[0].terminator = Terminator::Return {
                value: operand(returned, usage),
                cleanup: CleanupId(0),
            };
            let kinds = f.kinds();
            if usage == UseKind::Copy {
                assert!(kinds.is_empty(), "{kinds:?}");
            } else {
                assert!(
                    kinds.contains(&ViolationKind::BorrowedMove(PlaceId(returned))),
                    "{kinds:?}"
                );
            }
        }
    }
}

#[test]
fn generic_local_result_calls_prove_implicit_copyability() {
    for resource in [false, true] {
        let kind = if resource {
            TypeKind::Named {
                definition: id("Trace"),
                arguments: vec![].into(),
            }
        } else {
            string_kind()
        };
        let mut f = generic_call_fixture(Bound::Copyable, kind);
        if resource {
            let mut s = capability_signature("Trace");
            let representation = interner().intern(ty(TypeKind::Tuple(vec![].into())));
            let SignatureKind::Type(d) = &mut s.kind else {
                unreachable!()
            };
            d.capability = None;
            d.resource = true;
            d.authority = AuthoritySummary::inert();
            d.representation = Some(representation);
            f.signatures.insert(id("Trace"), s);
        }
        let s = f.signatures.get_mut(&path("callee")).unwrap();
        let SignatureKind::Function(fun) = &mut s.kind else {
            unreachable!()
        };
        fun.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        }]
        .into();
        fun.parameters[0].passing = PassingMode::Borrow;
        let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
        row.modes.relation = Relation::environment();
        fun.parameters[0].ty = interner().intern(row.clone());
        row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        fun.result = interner().intern(row);
        let actual = f.body().places[1].ty;
        let mut local = interner().get(actual).unwrap().clone();
        local.modes.relation = Relation::environment();
        let local = f.definition.types.push(local);
        f.body().places[1].ty = local;
        f.body().locals[1].ty = local;
        f.body().places[2].ty = local;
        f.body().locals[2].ty = local;
        owner_sig(&mut f, &[PassingMode::Borrow]);
        if let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator {
            arguments[0].value.usage = UseKind::Borrow;
        }
        let exit = f.cleanup(vec![drop(2)]);
        f.body().blocks[1].terminator = ret(0, exit);
        let kinds = f.kinds();
        if resource {
            assert!(
                kinds.contains(&ViolationKind::TypeMismatch {
                    entity: "generic bound"
                }),
                "{kinds:?}"
            );
        } else {
            assert!(kinds.is_empty(), "{kinds:?}");
        }
    }
}

#[test]
fn implicit_generic_sharing_is_confined_to_local_result_reads() {
    for (result_flow, written_bound, temporary) in [
        (true, false, false),
        (true, false, true),
        (false, false, true),
        (false, true, true),
    ] {
        let mut f = Fixture::unit();
        let x = param(
            &mut f,
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        );
        let x_ty = f.body().places[x as usize].ty;
        let mut row = interner().get(x_ty).unwrap().clone();
        row.modes.relation = Relation::environment();
        let local = f.definition.types.push(row.clone());
        f.body().locals[x as usize].ty = local;
        f.body().places[x as usize].ty = local;
        row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        let result = f.definition.types.push(row);
        let t = if temporary {
            let t = f.slot(
                TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
                Storage::Temporary,
            );
            f.body().locals[t as usize].ty = local;
            f.body().places[t as usize].ty = local;
            f.assign(t, Rvalue::Use(copy(x)), 0);
            t
        } else {
            x
        };
        if result_flow {
            f.body().result = result;
            f.body().blocks[0].terminator = Terminator::Return {
                value: operand(
                    t,
                    if temporary {
                        UseKind::Take
                    } else {
                        UseKind::Copy
                    },
                ),
                cleanup: CleanupId(0),
            };
        } else {
            f.emit(InstructionKind::Drop {
                place: PlaceId(t),
                if_live: false,
            });
        }
        owner_sig(&mut f, &[PassingMode::Borrow]);
        if result_flow {
            let scope = f.body().parameter_scope;
            f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
        }
        let SignatureKind::Function(fun) =
            &mut f.signatures.get_mut(f.definition.path).unwrap().kind
        else {
            unreachable!()
        };
        fun.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: if written_bound {
                vec![Bound::Copyable].into()
            } else {
                vec![].into()
            },
        }]
        .into();
        let kinds = f.kinds();
        if result_flow || written_bound {
            assert!(kinds.is_empty(), "{kinds:?}");
        } else {
            assert!(
                kinds.contains(&ViolationKind::InvalidUse(PlaceId(x))),
                "{kinds:?}"
            );
        }
    }
}

#[test]
fn sorted_map_hidden_cleanup_cannot_prove_noalloc_drop() {
    for kind in [BuiltinType::Map, BuiltinType::SortedMap] {
        let mut f = generic_call_fixture(Bound::NoallocDrop, TypeKind::Tuple(vec![].into()));
        let mut scalar = ty(TypeKind::Primitive(Primitive::U8));
        scalar.modes.sendability = Sendability::Sendable;
        let scalar = f.definition.types.push(scalar);
        // Even a producer-written NoAlloc row cannot hide the comparator.
        let actual = f.definition.types.push(ty(TypeKind::Builtin {
            kind,
            arguments: vec![GenericArgument::Type(scalar), GenericArgument::Type(scalar)].into(),
        }));
        let Terminator::Call { arguments, .. } = &f.body().blocks[0].terminator else {
            panic!()
        };
        let place = arguments[0].value.place;
        let root = f.body().places[place.0 as usize].root;
        f.body().places[place.0 as usize].ty = actual;
        f.body().locals[root.0 as usize].ty = actual;
        owner_sig(&mut f, &[PassingMode::Take]);
        if let Terminator::Call {
            callee: Callee::Definition { arguments, .. },
            ..
        } = &mut f.body().blocks[0].terminator
        {
            arguments[0] = GenericArgument::Type(actual);
        }
        if kind == BuiltinType::Map {
            f.valid()
        } else {
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "generic bound"
            }));
        }
    }
}

#[test]
fn local_result_calls_end_counted_source_uniqueness() {
    for plain in [false, true] {
        let mut f = unique_copy_fixture(plain);
        let kind = if plain {
            TypeKind::Primitive(Primitive::U64)
        } else {
            string_kind()
        };
        let mut signature = function_signature(
            "view",
            vec![(PassingMode::Borrow, kind)],
            if plain {
                TypeKind::Primitive(Primitive::U64)
            } else {
                string_kind()
            },
        );
        let SignatureKind::Function(fun) = &mut signature.kind else {
            unreachable!()
        };
        let mut row = interner().get(fun.result).unwrap().clone();
        row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        fun.result = interner().intern(row);
        f.signatures.insert(id("view"), signature);
        let original_call = f.body().blocks[0].terminator.clone();
        let begin_mut = f.body().blocks[0].instructions.pop().unwrap();
        f.body().blocks[0].instructions.pop(); // Replace the share assignment with its call form.
        f.body().rvalues.pop();
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(1),
            place: PlaceId(1),
            mutable: false,
            if_inactive: false,
        });
        let via = f.through_loan(1, 1);
        let fail = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(1),
                if_active: false,
            },
            drop(1),
        ]);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("view"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(via, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(2),
                next: BlockId(2),
            }),
            on_failure: CleanupId(fail),
        };
        let live = f.cleanup(vec![drop(2), drop(1)]);
        f.body().blocks.push(block(
            vec![
                inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(1),
                        if_active: false,
                    },
                    live,
                ),
                begin_mut,
            ],
            original_call,
        ));
        let kinds = f.kinds();
        if plain {
            assert!(kinds.is_empty(), "{kinds:?}");
        } else {
            assert!(
                kinds.contains(&ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
                "{kinds:?}"
            );
        }
    }
}

#[test]
fn copying_a_borrowed_temporary_ends_its_original_sources_uniqueness() {
    let mut f = unique_copy_fixture(false);
    let temporary = f.slot(string_kind(), Storage::Temporary);
    let via = f.through_loan(1, 1);
    let rv = push_rvalue(&mut f, Rvalue::Use(operand(via, UseKind::Borrow)));
    let fail = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        drop(1),
    ]);
    f.body().rvalues[2] = Rvalue::Use(copy(temporary));
    f.body().blocks[0].instructions[2].on_failure = CleanupId(fail);
    f.body().blocks[0].instructions.insert(
        2,
        inst(
            InstructionKind::Assign {
                destination: PlaceId(temporary),
                value: rv,
            },
            fail,
        ),
    );
    f.body().blocks[0].instructions.insert(
        2,
        inst(
            InstructionKind::BeginLoan {
                id: LoanId(1),
                place: PlaceId(1),
                mutable: false,
                if_inactive: false,
            },
            0,
        ),
    );
    f.body().blocks[0].instructions.insert(
        5,
        inst(
            InstructionKind::EndLoan {
                loan: LoanId(1),
                if_active: false,
            },
            0,
        ),
    );
    let kinds = f.kinds();
    assert_eq!(
        kinds,
        vec![ViolationKind::TypeMismatch {
            entity: "call argument"
        }]
    );
    let signature = f.signatures.get_mut(&path("require_unique")).unwrap();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Aliased;
    fun.parameters[0].ty = interner().intern(row);
    f.valid();
}

#[test]
fn plain_pointer_generic_actuals_are_also_sendable() {
    for kind in [BuiltinType::Ptr, BuiltinType::FnPtr] {
        for bound in [
            Bound::Plain,
            Bound::Copyable,
            Bound::Sendable,
            Bound::Flat,
            Bound::Data,
            Bound::Key,
        ] {
            let mut f = generic_call_fixture(bound, TypeKind::Tuple(vec![].into()));
            let mut scalar = ty(TypeKind::Primitive(Primitive::U8));
            scalar.modes.sendability = Sendability::Sendable;
            let scalar = f.definition.types.push(scalar);
            let mut pointer = ty(TypeKind::Builtin {
                kind,
                arguments: vec![GenericArgument::Type(scalar)].into(),
            });
            pointer.modes.sendability = Sendability::Sendable;
            let actual = f.definition.types.push(pointer);
            let Terminator::Call { arguments, .. } = &f.body().blocks[0].terminator else {
                panic!()
            };
            let place = arguments[0].value.place;
            let root = f.body().places[place.0 as usize].root;
            f.body().places[place.0 as usize].ty = actual;
            f.body().locals[root.0 as usize].ty = actual;
            owner_sig(&mut f, &[PassingMode::Take]);
            if let Terminator::Call {
                callee: Callee::Definition { arguments, .. },
                ..
            } = &mut f.body().blocks[0].terminator
            {
                arguments[0] = GenericArgument::Type(actual);
            }
            if matches!(bound, Bound::Plain | Bound::Copyable | Bound::Sendable) {
                f.valid();
            } else {
                assert!(
                    f.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "generic bound"
                    }),
                    "{kind:?}/{bound:?}: {:?}",
                    f.kinds()
                );
            }
        }
    }
}

fn scoped_capture_result_fixture() -> Fixture {
    let mut f = unique_copy_fixture(false);
    let old_call = f.body().blocks[0].terminator.clone();
    let begin_mut = f.body().blocks[0].instructions.pop().unwrap();
    f.body().blocks[0].instructions.pop();
    f.body().rvalues.pop();
    let mut local_row = ty(string_kind());
    local_row.modes.relation = Relation::environment();
    let local = f.definition.types.push(local_row);
    let captured = f.body().places[1].ty;
    let mut nested = empty_body(&mut f);
    nested.result = local;
    let cap = add_capture(&mut nested, captured);
    nested.blocks[0].terminator = Terminator::Return {
        value: copy(cap),
        cleanup: CleanupId(0),
    };
    f.definition.bodies.push(nested);
    let closure = f.slot(
        TypeKind::Function(fn_type(vec![], local)),
        Storage::Temporary,
    );
    let ty = f.body().places[closure as usize].ty;
    let mut row = interner().get(ty).unwrap().clone();
    row.modes.relation = Relation::environment();
    let ty = f.definition.types.push(row);
    f.body().places[closure as usize].ty = ty;
    let closure_root = f.body().places[closure as usize].root;
    f.body().locals[closure_root.0 as usize].ty = ty;
    f.body().places[2].ty = local;
    f.body().locals[2].ty = local;
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(1),
        mutable: false,
        if_inactive: false,
    });
    let via = f.through_loan(1, 1);
    let capture_fail = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        drop(1),
    ]);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(via, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        capture_fail,
    );
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(2),
        place: PlaceId(closure),
        mutable: false,
        if_inactive: false,
    });
    let via_closure = f.through_loan(closure, 2);
    let fail = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(2),
            if_active: false,
        },
        drop(closure),
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        drop(1),
    ]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: operand(via_closure, UseKind::Borrow),
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(2),
            next: BlockId(2),
        }),
        on_failure: CleanupId(fail),
    };
    // Drop the returned share and closure before requiring the original root unique.
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Drop {
                    place: PlaceId(2),
                    if_live: false,
                },
                fail,
            ),
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(2),
                    if_active: false,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(closure),
                    if_live: false,
                },
                capture_fail,
            ),
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(1),
                    if_active: false,
                },
                0,
            ),
            begin_mut,
        ],
        old_call,
    ));
    for node in &mut f.body().cleanup_nodes {
        for action in [&mut node.action] {
            if let CleanupAction::Drop {
                place: PlaceId(2),
                if_live,
            } = action
            {
                *if_live = true;
            }
        }
    }
    f
}

#[test]
fn a_scoped_capture_result_call_ends_its_borrowed_sources_uniqueness() {
    let mut f = scoped_capture_result_fixture();
    let kinds = f.kinds();
    assert_eq!(
        kinds,
        vec![ViolationKind::TypeMismatch {
            entity: "call argument"
        }]
    );
    let signature = f.signatures.get_mut(&path("require_unique")).unwrap();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Aliased;
    fun.parameters[0].ty = interner().intern(row);
    f.valid();
}

fn copied_scoped_capture_result_fixture() -> Fixture {
    let mut f = scoped_capture_result_fixture();
    let original = f.body().blocks[0].terminator.clone();
    let Terminator::Call {
        callee: Callee::Indirect { value, .. },
        on_failure,
        ..
    } = &original
    else {
        panic!("expected scoped closure call")
    };
    let callable = f.body().places[value.place.0 as usize].ty;
    let result = f.slot(
        interner().get(callable).unwrap().kind.clone(),
        Storage::Temporary,
    );
    let result_root = f.body().places[result as usize].root;
    f.body().locals[result_root.0 as usize].ty = callable;
    f.body().places[result as usize].ty = callable;
    // The copied environment is released before its input view and original
    // environment, on normal and failure cleanup.
    let old_cleanups = f.body().cleanups.len();
    let mut cleanup_map = Vec::new();
    for old in 0..old_cleanups {
        let mut actions: Vec<_> = f
            .body()
            .cleanup_actions(CleanupId(old as u32))
            .cloned()
            .collect();
        actions.insert(
            0,
            CleanupAction::Drop {
                place: PlaceId(result),
                if_live: true,
            },
        );
        cleanup_map.push(f.cleanup(actions));
    }
    for block in &mut f.body().blocks {
        for instruction in &mut block.instructions {
            instruction.on_failure = CleanupId(cleanup_map[instruction.on_failure.0 as usize]);
        }
        match &mut block.terminator {
            Terminator::Call { on_failure, .. } => {
                *on_failure = CleanupId(cleanup_map[on_failure.0 as usize])
            }
            Terminator::Return { cleanup, .. } => {
                *cleanup = CleanupId(cleanup_map[cleanup.0 as usize])
            }
            _ => (),
        }
    }
    let mut signature = function_signature("copy_function", vec![], TypeKind::Tuple(vec![].into()));
    let abstract_ty = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let mut returned = interner().get(abstract_ty).unwrap().clone();
    returned.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    let returned = interner().intern(returned);
    let SignatureKind::Function(function) = &mut signature.kind else {
        panic!()
    };
    function.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Copyable].into(),
    }]
    .into();
    function.parameters = vec![Parameter {
        name: Some("x".into()),
        label: None,
        passing: PassingMode::Borrow,
        ty: abstract_ty,
    }]
    .into();
    function.result = returned;
    f.signatures.insert(id("copy_function"), signature);
    let actual = generic_type_argument(interner(), callable, |_| false).unwrap();
    let next = f.body().blocks.len() as u32;
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("copy_function"),
            arguments: vec![GenericArgument::Type(actual)],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: value.clone(),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(next),
        }),
        on_failure: CleanupId(cleanup_map[on_failure.0 as usize]),
    };
    let mut invocation = original;
    let Terminator::Call {
        callee, on_failure, ..
    } = &mut invocation
    else {
        panic!()
    };
    *callee = Callee::Indirect {
        forward_caller: false,
        value: operand(result, UseKind::Borrow),
        arguments: vec![],
    };
    *on_failure = CleanupId(cleanup_map[on_failure.0 as usize]);
    f.body().blocks.push(block(vec![], invocation));
    f
}

#[test]
fn copied_callable_results_keep_their_invoked_sharing_dependencies() {
    let mut f = copied_scoped_capture_result_fixture();
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "call argument"
        }]
    );
    let signature = f.signatures.get_mut(&path("require_unique")).unwrap();
    let SignatureKind::Function(function) = &mut signature.kind else {
        panic!()
    };
    let mut ordinary = interner().get(function.parameters[0].ty).unwrap().clone();
    ordinary.modes.uniqueness = Uniqueness::Aliased;
    function.parameters[0].ty = interner().intern(ordinary);
    f.valid();
}

#[test]
fn element_mutation_expires_container_brands_but_preserves_siblings() {
    for mutation in ["end", "replace", "call"] {
        for fields in [vec![], vec![FieldId(0)], vec![FieldId(1)]] {
            let mut f = Fixture::unit();
            let scalar = f
                .definition
                .types
                .push(ty(TypeKind::Primitive(Primitive::U64)));
            let list = f.definition.types.push(ty(TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(scalar)].into(),
            }));
            let owner = f.slot(
                TypeKind::Record(
                    vec![
                        Field {
                            name: "xs".into(),
                            ty: list,
                            growable: false,
                        },
                        Field {
                            name: "other".into(),
                            ty: scalar,
                            growable: false,
                        },
                    ]
                    .into(),
                ),
                Storage::Parameter,
            );
            let value = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
            let origin = Origin {
                root: OriginRoot::Parameter(ParameterRef::body(
                    f.body().parameter_scope,
                    ParamId(0),
                )),
                fields: fields.clone().into(),
            };
            let mut branded = ty(TypeKind::Primitive(Primitive::U64));
            branded.brand.alternatives.push(origin);
            let branded = f.definition.types.push(branded);
            f.body().locals[value as usize].ty = branded;
            f.body().places[value as usize].ty = branded;
            f.body().parameters = vec![LocalId(owner), LocalId(value)];
            let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
            f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
            let mut element = f.body().places[owner as usize].clone();
            element.projection =
                vec![Projection::Field(0), Projection::Index(LocalId(index))].into();
            element.ty = scalar;
            element.binding = None;
            let element_place = element;
            let element = PlaceId(f.body().places.len() as u32);
            f.body().places.push(element_place);
            owner_signature(&mut f, PassingMode::Borrow);
            let signature = f.signatures.get_mut(&path("main")).unwrap();
            let mut marked = interner().get(branded).unwrap().clone();
            marked.brand.alternatives[0].root =
                OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
            let marked = interner().intern(marked);
            let SignatureKind::Function(fun) = &mut signature.kind else {
                unreachable!()
            };
            fun.parameters[0].passing = PassingMode::Mut;
            fun.parameters[1].ty = marked;
            match mutation {
                "end" => f.emit(InstructionKind::EndOrigin(element)),
                "replace" => {
                    let displaced = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
                    f.emit(InstructionKind::Replace {
                        destination: element,
                        value: copy(index),
                        displaced: PlaceId(displaced),
                        if_live: false,
                    });
                    f.emit(InstructionKind::Drop {
                        place: PlaceId(displaced),
                        if_live: false,
                    });
                }
                "call" => {
                    let types = interner();
                    let scalar = types.push(ty(TypeKind::Primitive(Primitive::U64)));
                    let unit = types.push(ty(TypeKind::Tuple(vec![].into())));
                    let mut fun = fn_type(
                        vec![Parameter {
                            name: None,
                            label: None,
                            ty: scalar,
                            passing: PassingMode::Mut,
                        }],
                        unit,
                    );
                    fun.authority = AuthoritySummary::inert();
                    f.signatures.insert(
                        id("bump"),
                        Signature {
                            definition: id("bump"),
                            metadata: SignatureMetadata::default(),
                            fingerprint: ContentHash::default(),
                            kind: SignatureKind::Function(fun),
                        },
                    );
                    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
                    f.body().blocks[0].terminator = Terminator::Call {
                        callee: Callee::Definition {
                            path: path("bump"),
                            arguments: vec![],
                        },
                        arguments: vec![CallArgument {
                            parameter: ParamId(0),
                            value: Operand {
                                place: element,
                                usage: UseKind::Mut,
                            },
                        }],
                        returns: Some(CallReturn {
                            destination: PlaceId(destination),
                            next: BlockId(1),
                        }),
                        on_failure: CleanupId(0),
                    };
                    f.body().blocks.push(block(vec![], ret(0, 0)));
                }
                _ => unreachable!(),
            }
            let result = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
            let result_root = f.body().places[result as usize].root;
            f.body().locals[result_root.0 as usize].ty = branded;
            f.body().places[result as usize].ty = branded;
            let rv = RvalueId(f.body().rvalues.len() as u32);
            f.body()
                .rvalues
                .push(Rvalue::Use(operand(value, UseKind::Borrow)));
            let last = f.body().blocks.last_mut().unwrap();
            last.instructions.push(inst(
                InstructionKind::Assign {
                    destination: PlaceId(result),
                    value: rv,
                },
                0,
            ));
            last.instructions.push(inst(
                InstructionKind::Drop {
                    place: PlaceId(result),
                    if_live: false,
                },
                0,
            ));
            let kinds = f.kinds();
            if fields == vec![FieldId(1)] {
                assert!(kinds.is_empty(), "{mutation}: {kinds:?}");
            } else {
                assert!(
                    kinds.contains(&ViolationKind::InvalidOrigin(PlaceId(value))),
                    "{mutation}: {kinds:?}"
                );
            }
        }
    }
}

#[test]
fn captured_generic_binder_swaps_are_rejected_even_when_bounds_coincide() {
    for consumer in 0..4 {
        let mut fixture = captured_generic_copy(true, true, false);
        fixture.valid();
        let body = &mut fixture.definition.bodies[1];
        let old = body.places[1].ty;
        let mut row = interner().get(old).unwrap().clone();
        let other = TypeParameterRef::body(body.parameter_scope, TypeParamId(0));
        match consumer {
            0 => row.kind = TypeKind::Parameter(other),
            1 => row.modes.sendability = Sendability::DependsOn(vec![other].into()),
            2 => row.drop_allocation = DropAllocation::DependsOn(vec![other].into()),
            _ => {
                row.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
                    vec![AuthorityTerm::TypeParameter(other)].into(),
                )))
            }
        }
        let forged = fixture.definition.types.push(row);
        body.places[1].ty = forged;
        body.locals[body.places[1].root.0 as usize].ty = forged;
        assert!(
            !fixture.kinds().is_empty(),
            "consumer {consumer} accepted a different binder"
        );
    }
}

#[test]
fn a_nested_generic_declaration_cannot_reuse_its_parents_scope() {
    let mut f = captured_generic_copy(true, true, false);
    f.valid();
    f.definition.bodies[1].parameter_scope = f.definition.bodies[0].parameter_scope;
    assert!(f.kinds().contains(&ViolationKind::InvalidTypeGraph));
}

#[test]
fn schema_and_instantiation_evidence_cannot_name_an_unrelated_binder() {
    let mut f = generic_call_fixture(Bound::Data, string_kind());
    let callee = path("callee");
    f.signatures.get_mut(&callee).unwrap().metadata.schema = vec![SchemaConstraint::NoallocDrop(
        TypeParameterRef::bound(TypeParamId(0)),
    )];
    f.valid();
    f.signatures.get_mut(&callee).unwrap().metadata.schema = vec![SchemaConstraint::NoallocDrop(
        TypeParameterRef::body(scoped(NameSite::Item), TypeParamId(0)),
    )];
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidSignature(callee.clone()))
    );
    f.signatures
        .get_mut(&callee)
        .unwrap()
        .metadata
        .schema
        .clear();
    let forged = f
        .definition
        .types
        .push(ty(TypeKind::Parameter(TypeParameterRef::body(
            scoped(NameSite::Item),
            TypeParamId(0),
        ))));
    let Terminator::Call {
        callee: Callee::Definition { arguments, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    arguments[0] = GenericArgument::Type(forged);
    assert!(f.kinds().contains(&ViolationKind::InvalidTypeGraph));
}

#[test]
fn lexical_declarations_inspect_each_instruction_once() {
    for count in [16, 64, 256] {
        let mut f = Fixture::unit();
        for _ in 0..count {
            let slot = f.slot(TypeKind::Tuple(vec![].into()), Storage::Local);
            f.assign(slot, Rvalue::Constant(Constant::Unit), 0);
        }
        owner_sig(&mut f, &[]);
        let before =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateDeclarationRows);
        let validation = validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut vec![],
        );
        assert!(
            validation.violations.is_empty(),
            "{:?}",
            validation.violations
        );
        let inspected =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateDeclarationRows)
                - before;
        let instructions = f
            .body()
            .blocks
            .iter()
            .map(|b| b.instructions.len())
            .sum::<usize>();
        assert!(
            inspected <= instructions as u64,
            "{count} assignments inspected {inspected} declaration instructions ({instructions} emitted)"
        );
    }
}

#[test]
fn intrinsic_bound_constraints_use_actual_types_and_exact_declarations() {
    for (kind, valid) in [
        (TypeKind::Primitive(Primitive::U64), true),
        (string_kind(), false),
    ] {
        let mut fixture = generic_call_fixture(Bound::Plain, kind);
        let signature = fixture.signatures.get_mut(&path("callee")).unwrap();
        let SignatureKind::Function(function) = &mut signature.kind else {
            unreachable!()
        };
        let GenericParameter::Type { bounds, .. } = &mut function.generics[0] else {
            unreachable!()
        };
        bounds.clear();
        signature.metadata.schema = vec![SchemaConstraint::Bound {
            parameter: TypeParameterRef::bound(TypeParamId(0)),
            bound: Bound::Plain,
            rule: const { hirsel_diag::RuleId::of("GEN-1") },
        }];
        if valid {
            fixture.valid();
        } else {
            assert!(fixture.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "intrinsic bound"
            }));
        }
        for reference in [
            TypeParameterRef::Body {
                scope: scoped(NameSite::Item),
                parameter: TypeParamId(0),
            },
            TypeParameterRef::Bound {
                depth: 1,
                parameter: TypeParamId(0),
            },
        ] {
            fixture
                .signatures
                .get_mut(&path("callee"))
                .unwrap()
                .metadata
                .schema = vec![SchemaConstraint::Bound {
                parameter: reference,
                bound: Bound::Plain,
                rule: const { hirsel_diag::RuleId::of("GEN-1") },
            }];
            assert!(
                fixture
                    .kinds()
                    .contains(&ViolationKind::InvalidSignature(path("callee")))
            );
        }
    }
}

#[test]
fn local_owned_receivers_accept_owned_content_without_changing_caller_storage_rules() {
    for owned_receiver in [true, false] {
        for local_source in [true, false] {
            for usage in [UseKind::Copy, UseKind::Borrow, UseKind::Take] {
                let mut f = Fixture::unit();
                let receiver = f.slot(
                    TypeKind::Primitive(Primitive::String),
                    if owned_receiver {
                        Storage::Local
                    } else {
                        Storage::Parameter
                    },
                );
                let receiver_ty = f.body().places[receiver as usize].ty;
                let mut receiver_ty = interner().get(receiver_ty).unwrap().clone();
                receiver_ty.modes.relation = Relation::environment();
                let receiver_ty = f.definition.types.push(receiver_ty);
                f.body().locals[receiver as usize].ty = receiver_ty;
                f.body().places[receiver as usize].ty = receiver_ty;
                if owned_receiver {
                    f.assign(
                        receiver,
                        Rvalue::Constant(Constant::String("old".into())),
                        0,
                    );
                } else {
                    f.body().parameters.push(LocalId(receiver));
                }
                let source = param(&mut f, TypeKind::Primitive(Primitive::String));
                if local_source {
                    let source_ty = f.body().places[source as usize].ty;
                    let mut row = interner().get(source_ty).unwrap().clone();
                    row.modes.relation = Relation::environment();
                    let ty = f.definition.types.push(row);
                    f.body().locals[source as usize].ty = ty;
                    f.body().places[source as usize].ty = ty;
                }
                let displaced = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
                if owned_receiver {
                    f.body().locals[displaced as usize].ty = receiver_ty;
                    f.body().places[displaced as usize].ty = receiver_ty;
                }
                let failure = if owned_receiver {
                    f.cleanup(vec![drop(receiver)])
                } else {
                    0
                };
                f.body().blocks[0].instructions.push(inst(
                    InstructionKind::Replace {
                        destination: PlaceId(receiver),
                        value: operand(source, usage),
                        displaced: PlaceId(displaced),
                        if_live: false,
                    },
                    failure,
                ));
                let cleanup = f.cleanup(if owned_receiver {
                    vec![drop(displaced), drop(receiver)]
                } else {
                    vec![drop(displaced)]
                });
                f.body().blocks[0].terminator = ret(0, cleanup);
                owner_sig(
                    &mut f,
                    if owned_receiver {
                        &[PassingMode::Borrow]
                    } else {
                        &[PassingMode::Mut, PassingMode::Borrow]
                    },
                );
                if usage == UseKind::Copy && (owned_receiver || !local_source) {
                    f.valid();
                } else {
                    let kinds = f.kinds();
                    assert!(
                        kinds.contains(&ViolationKind::BorrowedMove(PlaceId(source))),
                        "{owned_receiver}/{local_source}/{usage:?}: {kinds:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn borrowed_capture_promises_follow_current_source_modes() {
    for unique_storage in [false, true] {
        for shared in [false, true] {
            for (unique_capture, sendable_capture) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let mut f = Fixture::unit();
                let source = f.slot(string_kind(), Storage::Local);
                if unique_storage {
                    let ty = f.body().places[source as usize].ty;
                    let mut row = interner().get(ty).unwrap().clone();
                    row.modes.uniqueness = Uniqueness::Unique;
                    row.modes.sendability = Sendability::Sendable;
                    let ty = f.definition.types.push(row);
                    f.body().locals[source as usize].ty = ty;
                    f.body().places[source as usize].ty = ty;
                }
                f.assign(
                    source,
                    Rvalue::Constant(Constant::String("source".into())),
                    0,
                );
                let mut owners = vec![drop(source)];
                if shared {
                    let alias = f.slot(string_kind(), Storage::Local);
                    let fail = f.cleanup(owners.clone());
                    f.assign(alias, Rvalue::Use(copy(source)), fail);
                    owners.insert(0, drop(alias));
                }
                f.emit(InstructionKind::BeginLoan {
                    id: LoanId(0),
                    place: PlaceId(source),
                    mutable: false,
                    if_inactive: false,
                });
                let via = f.through_loan(source, 0);
                let source_ty = f.body().places[source as usize].ty;
                let mut capture = interner().get(source_ty).unwrap().clone();
                capture.modes.uniqueness = if unique_capture {
                    Uniqueness::Unique
                } else {
                    Uniqueness::Aliased
                };
                capture.modes.sendability = if sendable_capture {
                    Sendability::Sendable
                } else {
                    Sendability::NotSendable
                };
                let capture_ty = f.definition.types.push(capture);
                let mut child = empty_body(&mut f);
                add_capture(&mut child, capture_ty);
                f.definition.bodies.push(child);
                let unit = unit_ty(&mut f);
                let closure = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
                let ty = f.body().places[closure as usize].ty;
                let mut row = interner().get(ty).unwrap().clone();
                row.modes.relation = Relation::environment();
                let ty = f.definition.types.push(row);
                let root = f.body().places[closure as usize].root;
                f.body().locals[root.0 as usize].ty = ty;
                f.body().places[closure as usize].ty = ty;
                owners.insert(
                    0,
                    CleanupAction::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    },
                );
                let fail = f.cleanup(owners.clone());
                f.assign(
                    closure,
                    Rvalue::Closure {
                        context: hirsel_types::CaptureContext::OwningScoped,
                        body: BodyId(1),
                        captures: vec![operand(via, UseKind::Borrow)],
                        destination: Destination::Inferred,
                    },
                    fail,
                );
                owners.insert(0, drop(closure));
                let exit = f.cleanup(owners);
                f.body().blocks[0].terminator = ret(0, exit);
                owner_sig(&mut f, &[]);
                let kinds = f.kinds();
                if shared && unique_capture || sendable_capture && shared {
                    assert!(
                        kinds.contains(&ViolationKind::TypeMismatch {
                            entity: "value modes"
                        }),
                        "{unique_storage}/{shared}/{unique_capture}: {kinds:?}"
                    );
                } else {
                    assert!(
                        kinds.is_empty(),
                        "{unique_storage}/{shared}/{unique_capture}: {kinds:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn validation_interns_working_rows_bounded_by_the_owner() {
    for count in [16, 64, 256] {
        let mut f = Fixture::unit();
        for _ in 0..count {
            let slot = f.slot(TypeKind::Tuple(vec![].into()), Storage::Local);
            f.assign(slot, Rvalue::Use(copy(0)), 0);
        }
        owner_sig(&mut f, &[]);
        let rows = f.definition.types.len();
        let definition = tabled(&f.definition);
        let before =
            hirsel_intern::work::read(hirsel_intern::work::Counter::InternedTypeRows) as usize;
        let result = validate(&definition, interner(), &f.signatures, &mut vec![]);
        assert!(result.violations.is_empty(), "{:?}", result.violations);
        let interned = hirsel_intern::work::read(hirsel_intern::work::Counter::InternedTypeRows)
            as usize
            - before;
        assert!(
            interned <= 4 * rows,
            "{count} assignments interned {interned} type rows for {rows} owner rows"
        );
    }
}

#[test]
fn closure_bodies_intern_working_rows_bounded_by_the_owner() {
    for count in [16, 64] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let mut releases = vec![];
        let mut cleanup = 0;
        for index in 0..count {
            let destination = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
            let child = empty_body(&mut f);
            f.definition.bodies.push(child);
            f.assign(
                destination,
                Rvalue::Closure {
                    context: hirsel_types::CaptureContext::OwningScoped,
                    body: BodyId(index + 1),
                    captures: vec![],
                    destination: Destination::Inferred,
                },
                cleanup,
            );
            releases.insert(0, drop(destination));
            cleanup = f.cleanup(releases.clone());
        }
        f.body().blocks[0].terminator = ret(0, cleanup);
        let rows = f.definition.types.len();
        let before =
            hirsel_intern::work::read(hirsel_intern::work::Counter::InternedTypeRows) as usize;
        f.valid();
        let interned = hirsel_intern::work::read(hirsel_intern::work::Counter::InternedTypeRows)
            as usize
            - before;
        assert!(
            interned <= 4 * rows,
            "{count} bodies interned {interned} rows for {rows}"
        );
    }
}

#[test]
fn named_function_value_uses_come_from_the_named_signature() {
    for capability in [false, true] {
        for restricted in [false, true] {
            let mut f = Fixture::unit();
            if capability {
                f.signatures.insert(id("Dir"), capability_signature("Dir"));
            }
            let argument = if capability {
                TypeKind::Named {
                    definition: id("Dir"),
                    arguments: vec![].into(),
                }
            } else {
                TypeKind::Primitive(Primitive::U8)
            };
            let mut signature = function_signature(
                "worker",
                vec![(PassingMode::Borrow, argument)],
                TypeKind::Tuple(vec![].into()),
            );
            let SignatureKind::Function(function) = &mut signature.kind else {
                unreachable!()
            };
            let authority = if capability {
                AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into())
            } else {
                AuthoritySummary::inert()
            };
            function.authority = authority.clone();
            function.uses = Uses::Unrestricted;
            let imported = f
                .definition
                .types
                .push(ty(TypeKind::Function(function.clone())));
            let TypeKind::Function(mut receiving) = interner().get(imported).unwrap().kind.clone()
            else {
                unreachable!()
            };
            receiving.uses = Uses::Only(if restricted {
                AuthoritySummary::inert()
            } else {
                authority
            });
            let closure = f.slot(TypeKind::Function(receiving), Storage::Temporary);
            f.assign(
                closure,
                Rvalue::FunctionReference {
                    definition: path("worker"),
                    arguments: vec![],
                },
                0,
            );
            let cleanup = f.cleanup(vec![drop(closure)]);
            f.body().blocks[0].terminator = ret(0, cleanup);
            f.signatures.insert(id("worker"), signature);
            owner_sig(&mut f, &[]);
            if capability && restricted {
                assert!(
                    f.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "function reference"
                    }),
                    "{:?}",
                    f.kinds()
                );
            } else {
                f.valid();
            }
        }
    }
}

#[test]
fn captured_generic_fields_keep_their_receiving_lifetime() {
    for forged_local in [false, true] {
        let mut f = captured_generic_copy(true, false, false);
        let outer = f.definition.bodies[0].places[1].ty;
        let aggregate = f
            .definition
            .types
            .push(ty(TypeKind::Tuple(vec![outer].into())));
        f.definition.bodies[0].places[1].ty = aggregate;
        f.definition.bodies[0].locals[1].ty = aggregate;
        let owner = f.signatures.get_mut(f.definition.path).unwrap();
        let root = aggregate;
        let scope = f.definition.bodies[0].parameter_scope;
        let root = map_generic_binders(interner(), root, |binder, nested| {
            Ok(if binder == GenericBinder::Body(scope) {
                GenericBinder::Bound(nested)
            } else {
                binder
            })
        })
        .unwrap();
        let SignatureKind::Function(function) = &mut owner.kind else {
            panic!()
        };
        function.parameters[0].ty = root;
        let child = &mut f.definition.bodies[1];
        child.places[1].ty = aggregate;
        child.locals[1].ty = aggregate;
        let mut field = child.places[1].clone();
        field.projection = vec![Projection::Field(0)].into();
        if forged_local {
            let mut row = interner().get(outer).unwrap().clone();
            row.modes.relation = Relation::environment();
            field.ty = f.definition.types.push(row);
        } else {
            field.ty = outer;
        }
        let field_id = PlaceId(child.places.len() as u32);
        child.places.push(field);
        let Rvalue::Use(value) = child.rvalues.last_mut().unwrap() else {
            panic!()
        };
        value.place = field_id;
        if forged_local {
            assert!(
                f.kinds()
                    .contains(&ViolationKind::InvalidProjection(field_id)),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn read_only_data_comparison_accepts_independent_root_lifetimes() {
    for left_local in [false, true] {
        for right_local in [false, true] {
            let mut f = Fixture::unit();
            let left = param(
                &mut f,
                TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
            );
            let right = param(
                &mut f,
                TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
            );
            for (place, local) in [(left, left_local), (right, right_local)] {
                if local {
                    let source = f.body().places[place as usize].ty;
                    let mut row = interner().get(source).unwrap().clone();
                    row.modes.relation = Relation::environment();
                    row.modes.sendability = Sendability::NotSendable;
                    let ty = f.definition.types.push(row);
                    f.body().places[place as usize].ty = ty;
                    f.body().locals[place as usize].ty = ty;
                }
            }
            let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
            f.assign(
                result,
                Rvalue::Binary {
                    op: hirsel_syntax::BinaryOp::Eq,
                    left: operand(left, UseKind::Borrow),
                    right: operand(right, UseKind::Borrow),
                },
                0,
            );
            owner_sig(&mut f, &[PassingMode::Borrow, PassingMode::Borrow]);
            let SignatureKind::Function(owner) =
                &mut f.signatures.get_mut(f.definition.path).unwrap().kind
            else {
                panic!()
            };
            owner.generics = vec![GenericParameter::Type {
                name: "T".into(),
                bounds: vec![Bound::Data].into(),
            }]
            .into();
            f.valid();
            let SignatureKind::Function(owner) =
                &mut f.signatures.get_mut(f.definition.path).unwrap().kind
            else {
                panic!()
            };
            owner.generics = vec![GenericParameter::Type {
                name: "T".into(),
                bounds: vec![].into(),
            }]
            .into();
            assert!(
                f.kinds().contains(&ViolationKind::InvalidOperator),
                "{:?}",
                f.kinds()
            );
        }
    }
}

fn contract_clause_fixture(passing: PassingMode) -> Fixture {
    let mut f = Fixture::unit();
    let string = f.definition.types.push(ty(string_kind()));
    let boolean = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bool)));
    let mut clause = empty_body(&mut f);
    clause.locals[0].ty = boolean;
    clause.places[0].ty = boolean;
    clause.result = boolean;
    clause.rvalues[0] = Rvalue::Constant(Constant::Bool(true));
    let parameter = add_capture(&mut clause, string);
    clause.locals[parameter as usize].storage = Storage::Parameter;
    clause.parameters.push(LocalId(parameter));
    f.definition.bodies.push(clause);
    let function = f.slot(
        TypeKind::Function(fn_type(
            vec![Parameter {
                name: None,
                label: None,
                passing,
                ty: string,
            }],
            boolean,
        )),
        Storage::Local,
    );
    f.assign(
        function,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(function)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.definition.contracts.push((0, BodyId(1)));
    f
}

#[test]
fn contract_clauses_independently_require_borrowed_formals_and_readonly_access() {
    contract_clause_fixture(PassingMode::Borrow).valid();
    for passing in [PassingMode::Take, PassingMode::Mut] {
        let f = contract_clause_fixture(passing);
        assert!(
            f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "contract parameters"
            }),
            "{:?}",
            f.kinds()
        );
    }
    for alias in [None, Some(UseKind::Borrow), Some(UseKind::Copy)] {
        let mut f = contract_clause_fixture(PassingMode::Borrow);
        let source = if let Some(usage) = alias {
            let string = f.definition.bodies[1].locals[1].ty;
            let child = &mut f.definition.bodies[1];
            let place = add_capture(child, string);
            child.locals[place as usize].storage = Storage::Temporary;
            let value = RvalueId(child.rvalues.len() as u32);
            child.rvalues.push(Rvalue::Use(operand(1, usage)));
            child.blocks[0].instructions.push(inst(
                InstructionKind::Assign {
                    destination: PlaceId(place),
                    value,
                },
                0,
            ));
            place
        } else {
            1
        };
        let child = &mut f.definition.bodies[1];
        child.blocks[0].instructions.push(inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(source),
                mutable: true,
                if_inactive: false,
            },
            0,
        ));
        let mut access = child.places[source as usize].clone();
        access.loan = Some(LoanId(0));
        let access = {
            child.places.push(access);
            PlaceId(child.places.len() as u32 - 1)
        };
        let unit = unit_ty(&mut f);
        let string = f.definition.bodies[1].locals[1].ty;
        let mut signature = function_signature("poke", vec![], TypeKind::Tuple(vec![].into()));
        signature.kind = SignatureKind::Function(fn_type(
            vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Mut,
                ty: string,
            }],
            unit,
        ));
        f.signatures.insert(id("poke"), signature);
        let child = &mut f.definition.bodies[1];
        let result = add_capture(child, unit);
        child.locals[child.places[result as usize].root.0 as usize].storage = Storage::Temporary;
        let mut actions = vec![CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        }];
        if alias == Some(UseKind::Copy) {
            actions.push(drop(source));
        }
        let cleanup = child.add_cleanup(actions);
        child.blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("poke"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: Operand {
                    place: access,
                    usage: UseKind::Mut,
                },
            }],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: cleanup,
        };
        child.blocks.push(block(
            vec![inst(
                InstructionKind::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                0,
            )],
            ret(0, 0),
        ));
        if alias == Some(UseKind::Copy) {
            f.definition.bodies[1].blocks[1].instructions.push(inst(
                InstructionKind::Drop {
                    place: PlaceId(source),
                    if_live: false,
                },
                0,
            ));
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::InvalidUse(access)),
                "alias={alias:?}: {:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn contract_roots_cannot_hide_captures_or_omit_their_producer() {
    let mut captured = contract_clause_fixture(PassingMode::Borrow);
    captured.definition.bodies[1].locals[1].storage = Storage::Capture;
    assert!(captured.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "contract captures"
    }));
    let mut missing = contract_clause_fixture(PassingMode::Borrow);
    missing.definition.bodies[0].rvalues[1] = Rvalue::Constant(Constant::Unit);
    assert!(missing.kinds().contains(&ViolationKind::WrongKind {
        entity: "contract producer",
        index: 1
    }));
}

#[test]
fn contract_borrowed_formals_cannot_be_consumed_directly_or_through_an_alias() {
    for alias in [false, true] {
        let mut f = contract_clause_fixture(PassingMode::Borrow);
        let string = f.definition.bodies[1].locals[1].ty;
        let child = &mut f.definition.bodies[1];
        let source = if alias {
            let alias = add_capture(child, string);
            child.locals[alias as usize].storage = Storage::Temporary;
            let value = RvalueId(child.rvalues.len() as u32);
            child.rvalues.push(Rvalue::Use(operand(1, UseKind::Borrow)));
            child.blocks[0].instructions.push(inst(
                InstructionKind::Assign {
                    destination: PlaceId(alias),
                    value,
                },
                0,
            ));
            alias
        } else {
            1
        };
        let boolean = child.result;
        child.blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("eat"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(source, UseKind::Take),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(0),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        child.blocks.push(block(vec![], ret(0, 0)));
        let mut signature = function_signature("eat", vec![], TypeKind::Primitive(Primitive::Bool));
        signature.kind = SignatureKind::Function(fn_type(
            vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Take,
                ty: string,
            }],
            boolean,
        ));
        f.signatures.insert(id("eat"), signature);
        assert!(
            f.kinds()
                .contains(&ViolationKind::InvalidUse(PlaceId(source))),
            "alias={alias}: {:?}",
            f.kinds()
        );
    }
}

#[test]
fn contract_calls_rederive_required_authority_instead_of_trusting_the_clause_row() {
    for authority in [
        AuthoritySummary::inert(),
        AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into()),
        AuthoritySummary::Unknown,
    ] {
        let mut f = contract_clause_fixture(PassingMode::Borrow);
        let boolean = f.definition.bodies[1].result;
        let mut signature =
            function_signature("test", vec![], TypeKind::Primitive(Primitive::Bool));
        let mut function = fn_type(vec![], boolean);
        function.authority = authority.clone();
        signature.kind = SignatureKind::Function(function);
        f.signatures.insert(id("test"), signature);
        let child = &mut f.definition.bodies[1];
        child.blocks[0].instructions.clear();
        child.rvalues.clear();
        child.blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("test"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(0),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        child.blocks.push(block(vec![], ret(0, 0)));
        if authority == AuthoritySummary::inert() {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "contract call authority",
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

fn replaced_local_lifetime_fixture(mixed: bool, usage: UseKind, field: bool) -> Fixture {
    let mut f = Fixture::unit();
    let string = f.definition.types.push(ty(string_kind()));
    let kind = if field {
        TypeKind::Tuple(vec![string].into())
    } else {
        string_kind()
    };
    let x = param(&mut f, kind.clone());
    let source_ty = f.body().places[x as usize].ty;
    let mut local = interner().get(source_ty).unwrap().clone();
    local.modes.relation = Relation::environment();
    let local = f.definition.types.push(local);
    f.body().locals[x as usize].ty = local;
    f.body().places[x as usize].ty = local;
    let fresh = param(&mut f, kind.clone());
    let condition = param(&mut f, TypeKind::Primitive(Primitive::Bool));
    let receiver = f.slot(kind.clone(), Storage::Local);
    let displaced = f.slot(kind, Storage::Temporary);
    for slot in [receiver, displaced] {
        f.body().locals[slot as usize].ty = local;
        f.body().places[slot as usize].ty = local;
    }
    let output = f.slot(string_kind(), Storage::Temporary);
    f.assign(receiver, Rvalue::Use(copy(x)), 0);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(condition),
        yes: BlockId(1),
        no: BlockId(2),
    };
    for source in [take(fresh), if mixed { copy(x) } else { take(fresh) }] {
        f.body().blocks.push(block(
            vec![inst(
                InstructionKind::Replace {
                    destination: PlaceId(receiver),
                    value: source,
                    displaced: PlaceId(displaced),
                    if_live: false,
                },
                0,
            )],
            Terminator::Goto(BlockId(3)),
        ));
    }
    let source = if field {
        let body = f.body();
        let mut access = body.places[receiver as usize].clone();
        access.projection.push(Projection::Field(0));
        access.ty = string;
        body.places.push(access);
        body.places.len() as u32 - 1
    } else {
        receiver
    };
    let value = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(operand(source, usage)));
    let mut actions = vec![
        drop(output),
        drop(displaced),
        CleanupAction::Drop {
            place: PlaceId(fresh),
            if_live: true,
        },
    ];
    if usage == UseKind::Copy || field {
        actions.push(drop(receiver));
    }
    let cleanup = f.cleanup(actions);
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::Assign {
                destination: PlaceId(output),
                value,
            },
            0,
        )],
        ret(0, cleanup),
    ));
    owner_sig(
        &mut f,
        &[PassingMode::Borrow, PassingMode::Take, PassingMode::Borrow],
    );
    f
}

#[test]
fn replaced_local_content_proves_global_lifetime_on_every_live_path() {
    for usage in [UseKind::Copy, UseKind::Take] {
        replaced_local_lifetime_fixture(false, usage, false).valid();
        let mixed = replaced_local_lifetime_fixture(true, usage, false);
        assert!(
            mixed.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "assignment"
            }),
            "{:?}: {:?}",
            usage,
            mixed.kinds()
        );
    }
}

#[test]
fn replacement_lifetime_reaches_declared_global_fields_without_promoting_local_fields() {
    replaced_local_lifetime_fixture(false, UseKind::Copy, true).valid();
    let mixed = replaced_local_lifetime_fixture(true, UseKind::Copy, true);
    assert!(
        mixed
            .kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::InvalidProjection(_))),
        "{:?}",
        mixed.kinds()
    );
    let mut local = replaced_local_lifetime_fixture(false, UseKind::Copy, true);
    let tuple = local.definition.bodies[0].locals[1].ty;
    let TypeKind::Tuple(fields) = &interner().get(tuple).unwrap().kind else {
        panic!()
    };
    let mut field = interner().get(fields[0]).unwrap().clone();
    field.modes.relation = Relation::environment();
    let field = local.definition.types.push(field);
    let originals: Vec<_> = local
        .definition
        .types
        .iter()
        .map(|&id| (id, interner().row(id).clone()))
        .collect();
    let mut remaps = std::collections::HashMap::new();
    for (id, mut row) in originals {
        if matches!(row.kind, TypeKind::Tuple(ref fields) if fields.len() == 1) {
            row.kind = TypeKind::Tuple(vec![field].into());
            remaps.insert(id, local.definition.types.push(row));
        }
    }
    for body in &mut local.definition.bodies {
        for slot in &mut body.locals {
            if let Some(ty) = remaps.get(&slot.ty) {
                slot.ty = *ty;
            }
        }
        for place in &mut body.places {
            if let Some(ty) = remaps.get(&place.ty) {
                place.ty = *ty;
            }
        }
    }
    owner_sig(
        &mut local,
        &[PassingMode::Borrow, PassingMode::Take, PassingMode::Borrow],
    );
    assert!(
        local
            .kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::InvalidProjection(_))),
        "{:?}",
        local.kinds()
    );
}

fn closure_call_effect_fixture(allocation: Allocation, callback: bool) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let mut child = empty_body(&mut f);
    child.blocks[0].instructions.clear();
    child.rvalues.clear();
    let mut function = fn_type(vec![], unit);
    function.allocation = allocation;
    let mut signature = function_signature("invoke", vec![], TypeKind::Tuple(vec![].into()));
    if callback {
        let mut callback = fn_type(vec![], unit);
        callback.allocation = Allocation::MayAllocate;
        let callback = f.definition.types.push(ty(TypeKind::Function(callback)));
        let argument = add_capture(&mut child, callback);
        child.locals[argument as usize].storage = Storage::Parameter;
        child.parameters.push(LocalId(argument));
        child.scoped_parameters = vec![ScopedParameter {
            parameter: 0,
            fields: vec![],
        }];
        function.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: callback,
        }]
        .into();
        signature.kind = SignatureKind::Function(fn_type(function.parameters.to_vec(), unit));
        child.blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("invoke"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(argument, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(0),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
    } else {
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        function.allocation = Allocation::MayAllocate;
        child.blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("invoke"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: Some(CallReturn {
                destination: PlaceId(0),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
    }
    child.blocks.push(block(vec![], ret(0, 0)));
    f.signatures.insert(id("invoke"), signature);
    f.definition.bodies.push(child);
    let closure = f.slot(TypeKind::Function(function), Storage::Local);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
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
fn closure_invocation_allocation_is_rederived_from_direct_calls_and_borrowed_callbacks() {
    for callback in [false, true] {
        closure_call_effect_fixture(Allocation::MayAllocate, callback).valid();
        let forged = closure_call_effect_fixture(Allocation::NoAlloc, callback);
        assert!(
            forged.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "body allocation"
            }),
            "callback={callback}: {:?}",
            forged.kinds()
        );
    }
}

#[test]
fn closure_body_calls_cannot_exceed_the_written_uses_restriction() {
    for allowed in [false, true] {
        let mut f = closure_call_effect_fixture(Allocation::MayAllocate, false);
        let authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into());
        let SignatureKind::Function(function) =
            &mut f.signatures.get_mut(&path("invoke")).unwrap().kind
        else {
            panic!()
        };
        function.authority = authority.clone();
        let parent = &f.definition.bodies[0];
        let root = parent.locals.len() - 1;
        let mut row = interner().get(parent.locals[root].ty).unwrap().clone();
        let TypeKind::Function(function) = &mut row.kind else {
            panic!()
        };
        if allowed {
            function.uses = Uses::Only(authority);
        }
        let row = f.definition.types.push(row);
        f.definition.bodies[0].locals[root].ty = row;
        f.definition.bodies[0].places[root].ty = row;
        if allowed {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "body uses"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn a_provided_unknown_callback_does_not_supply_unrelated_ambient_authority() {
    for allowed in [false, true] {
        let mut f = closure_call_effect_fixture(Allocation::MayAllocate, true);
        let authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into());
        let parent = &f.definition.bodies[0];
        let root = parent.locals.len() - 1;
        let mut row = interner().get(parent.locals[root].ty).unwrap().clone();
        let TypeKind::Function(header) = &mut row.kind else {
            panic!()
        };
        let mut callback = interner().get(header.parameters[0].ty).unwrap().clone();
        let TypeKind::Function(function) = &mut callback.kind else {
            panic!()
        };
        function.authority = AuthoritySummary::Unknown;
        function.uses = Uses::Unrestricted;
        let supplied = f.definition.types.push(callback.clone());
        header.parameters[0].ty = supplied;
        if allowed {
            header.uses = Uses::Only(authority.clone());
        }
        let receiving = f.definition.types.push(row);
        f.definition.bodies[0].locals[root].ty = receiving;
        f.definition.bodies[0].places[root].ty = receiving;
        let input = f.definition.bodies[1].parameters[0].0 as usize;
        f.definition.bodies[1].locals[input].ty = supplied;
        f.definition.bodies[1].places[input].ty = supplied;
        let signature = f.signatures.get_mut(&path("invoke")).unwrap();
        let supplied = interner().intern(callback);
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        function.parameters[0].ty = supplied;
        function.authority = authority;
        if allowed {
            f.valid();
        } else {
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "body uses"
            }));
        }
    }
}

#[test]
fn a_contract_root_requires_a_callable_producer_instead_of_the_definition_entry() {
    contract_clause_fixture(PassingMode::Borrow).valid();
    for boolean in [false, true] {
        let mut forged = Fixture::unit();
        if boolean {
            let row = forged
                .definition
                .types
                .push(ty(TypeKind::Primitive(Primitive::Bool)));
            let entry = forged.body();
            entry.locals[0].ty = row;
            entry.places[0].ty = row;
            entry.result = row;
            entry.rvalues[0] = Rvalue::Constant(Constant::Bool(true));
        }
        forged.valid();
        forged
            .definition
            .contracts
            .push((0, forged.definition.entry));
        assert!(
            forged.kinds().contains(&ViolationKind::WrongKind {
                entity: "contract producer",
                index: forged.definition.entry.0,
            }),
            "{:?}",
            forged.kinds()
        );
    }
}

#[test]
fn conditional_receivers_retain_live_loan_dependencies_until_their_last_read() {
    for end_early in [false, true] {
        let mut f = Fixture::unit();
        let x = param(&mut f, string_kind());
        let condition = param(&mut f, TypeKind::Primitive(Primitive::Bool));
        let receiver = f.slot(string_kind(), Storage::Local);
        let displaced = f.slot(string_kind(), Storage::Temporary);
        let output = f.slot(string_kind(), Storage::Temporary);
        let incoming = f.slot(string_kind(), Storage::Temporary);
        let root_type = f.body().places[x as usize].ty;
        let mut local = interner().get(root_type).unwrap().clone();
        local.modes.relation = Relation::environment();
        let local = f.definition.types.push(local);
        for slot in [x, receiver, displaced, output, incoming] {
            f.body().locals[slot as usize].ty = local;
            f.body().places[slot as usize].ty = local;
        }
        f.assign(receiver, Rvalue::Use(copy(x)), 0);
        f.body().blocks[0].terminator = Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(1),
            no: BlockId(2),
        };
        f.body()
            .blocks
            .push(block(vec![], Terminator::Goto(BlockId(3))));
        let mut access = f.body().places[x as usize].clone();
        access.loan = Some(LoanId(0));
        let lent = PlaceId(f.body().places.len() as u32);
        f.body().places.push(access);
        let mut signature = function_signature(
            "view",
            vec![(PassingMode::Borrow, string_kind())],
            string_kind(),
        );
        let SignatureKind::Function(function) = &mut signature.kind else {
            unreachable!()
        };
        let mut parameter = interner().get(function.parameters[0].ty).unwrap().clone();
        parameter.modes.relation = Relation::environment();
        function.parameters[0].ty = interner().intern(parameter);
        let mut result = interner().get(function.result).unwrap().clone();
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        function.result = interner().intern(result);
        f.signatures.insert(id("view"), signature);
        let failure = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: true,
            },
            drop(receiver),
        ]);
        f.body().blocks.push(block(
            vec![inst(
                InstructionKind::BeginLoan {
                    id: LoanId(0),
                    place: PlaceId(x),
                    mutable: false,
                    if_inactive: false,
                },
                0,
            )],
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("view"),
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: Operand {
                        place: lent,
                        usage: UseKind::Borrow,
                    },
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(incoming),
                    next: BlockId(4),
                }),
                on_failure: CleanupId(failure),
            },
        ));
        let mut instructions = vec![
            inst(
                InstructionKind::Replace {
                    destination: PlaceId(receiver),
                    value: take(incoming),
                    displaced: PlaceId(displaced),
                    if_live: false,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(displaced),
                    if_live: false,
                },
                failure,
            ),
        ];
        if end_early {
            instructions.push(inst(
                InstructionKind::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                0,
            ));
        }

        let value = RvalueId(f.body().rvalues.len() as u32);
        f.body().rvalues.push(Rvalue::Use(copy(receiver)));
        let cleanup = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: true,
            },
            drop(output),
            drop(receiver),
        ]);
        f.body().blocks.push(block(
            vec![inst(
                InstructionKind::Assign {
                    destination: PlaceId(output),
                    value,
                },
                0,
            )],
            ret(0, cleanup),
        ));
        f.body()
            .blocks
            .push(block(instructions, Terminator::Goto(BlockId(3))));
        owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Borrow]);
        if end_early {
            assert!(
                f.kinds().iter().any(|kind| matches!(
                    kind,
                    ViolationKind::ExpiredLoan {
                        loan: LoanId(0),
                        ..
                    }
                )),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

fn derived_capability_closure_fixture(derive: bool, permitted: bool, parameter: bool) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    for name in ["Io", "Stdout"] {
        f.signatures.insert(id(name), capability_signature(name));
    }
    let named = |name: &str| TypeKind::Named {
        definition: id(name),
        arguments: vec![].into(),
    };
    let source = param(&mut f, named(if derive { "Io" } else { "Stdout" }));
    let source_ty = f.body().locals[source as usize].ty;
    let stdout = f.definition.types.push(ty(named("Stdout")));
    let mut child = empty_body(&mut f);
    child.blocks[0].instructions.clear();
    child.rvalues.clear();
    let input = add_capture(&mut child, source_ty);
    let output = if derive {
        add_capture(&mut child, stdout)
    } else {
        input
    };
    if derive {
        child.locals[output as usize].storage = Storage::Temporary;
    }
    let function_authority = AuthoritySummary::Known(
        vec![AuthorityTerm::Capability(path(if derive {
            "Io"
        } else {
            "Stdout"
        }))]
        .into(),
    );
    let mut function = fn_type(vec![], unit);
    function.authority = if parameter {
        AuthoritySummary::inert()
    } else {
        function_authority.clone()
    };
    function.uses = Uses::Only(if permitted {
        function.authority.clone()
    } else {
        AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Io"))].into())
    });
    if parameter {
        child.locals[input as usize].storage = Storage::Parameter;
        child.parameters.push(LocalId(input));
        function.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: source_ty,
        }]
        .into();
    }
    let mut acquire = function_signature(
        "acquire",
        vec![(PassingMode::Borrow, named("Io"))],
        named("Stdout"),
    );
    let mut invoke = function_signature(
        "invoke",
        vec![(PassingMode::Borrow, named("Stdout"))],
        TypeKind::Tuple(vec![].into()),
    );
    for signature in [&mut acquire, &mut invoke] {
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        function.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
        );
    }
    f.signatures.insert(id("acquire"), acquire);
    f.signatures.insert(id("invoke"), invoke);
    let call = |name: &str, source: u32, destination: u32, next: u32| Terminator::Call {
        callee: Callee::Definition {
            path: path(name),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(source, UseKind::Borrow),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(next),
        }),
        on_failure: CleanupId(0),
    };
    child.blocks[0].terminator = if derive {
        call("acquire", input, output, 1)
    } else {
        call("invoke", input, 0, 1)
    };
    if derive {
        child
            .blocks
            .push(block(vec![], call("invoke", output, 0, 2)));
    }
    // Plain capabilities have no release obligation; these explicit drops also
    // exercise the fixture with a conservatively opaque declaration.
    let cleanup = child.add_cleanup(if parameter {
        if derive { vec![drop(output)] } else { vec![] }
    } else if derive {
        vec![drop(output), drop(input)]
    } else {
        vec![drop(input)]
    });
    child.blocks.push(block(vec![], ret(0, cleanup.0)));
    f.definition.bodies.push(child);
    let closure = f.slot(TypeKind::Function(function), Storage::Temporary);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: if parameter {
                vec![]
            } else {
                vec![copy(source)]
            },
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_sig(&mut f, &[PassingMode::Borrow]);
    f
}

#[test]
fn closure_uses_bounds_external_sources_and_preserves_in_frame_attenuation() {
    for parameter in [false, true] {
        derived_capability_closure_fixture(true, true, parameter).valid();
        derived_capability_closure_fixture(false, true, parameter).valid();
    }
    let forged = derived_capability_closure_fixture(false, false, false);
    assert!(
        forged.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "closure uses"
        }),
        "{:?}",
        forged.kinds()
    );
}

#[test]
fn constant_reads_require_the_declared_constant_type_without_initializer_operands() {
    for variant in 0..3 {
        let mut f = Fixture::unit();
        let mut signature = function_signature(
            "DATA",
            vec![],
            if variant == 1 {
                string_kind()
            } else {
                TypeKind::Tuple(vec![].into())
            },
        );
        if variant != 2 {
            let SignatureKind::Function(function) = &signature.kind else {
                unreachable!()
            };
            signature.kind = SignatureKind::Const {
                ty: function.result,
                generics: Default::default(),
            };
        }
        f.signatures.insert(id("DATA"), signature);
        f.body().rvalues[0] = Rvalue::ReadConstant {
            arguments: vec![],
            definition: path("DATA"),
        };
        match variant {
            0 => f.valid(),
            1 => assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "constant declaration"
                }),
                "{:?}",
                f.kinds()
            ),
            _ => assert!(
                f.kinds()
                    .iter()
                    .any(|v| matches!(v, ViolationKind::InvalidOrigin(_))),
                "{:?}",
                f.kinds()
            ),
        }
    }
}

#[test]
fn named_function_references_independently_prove_allocation_and_uses() {
    for variant in 0..3 {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let function = f
            .definition
            .types
            .push(ty(TypeKind::Function(fn_type(vec![], unit))));
        let target = add_capture(f.body(), function);
        f.body().locals[target as usize].storage = Storage::Temporary;
        let mut signature = function_signature("target", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(header) = &mut signature.kind else {
            unreachable!()
        };
        header.allocation = if variant == 1 {
            Allocation::MayAllocate
        } else {
            Allocation::NoAlloc
        };
        if variant == 2 {
            header.uses = Uses::Only(AuthoritySummary::Known(
                vec![AuthorityTerm::Capability(path("Dir"))].into(),
            ));
            header.authority =
                AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into());
            f.signatures.insert(id("Dir"), capability_signature("Dir"));
        }
        f.signatures.insert(id("target"), signature);
        f.body().rvalues.push(Rvalue::FunctionReference {
            definition: path("target"),
            arguments: vec![],
        });
        f.body().blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(target),
                value: RvalueId(1),
            },
            0,
        ));
        let cleanup = f.body().add_cleanup(vec![drop(target)]);
        f.body().blocks[0].terminator = ret(0, cleanup.0);
        if variant == 0 {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "function reference"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn counted_constant_reads_independently_require_shared_root_modes() {
    for forged in [false, true] {
        let mut f = Fixture::unit();
        let target = f.slot(string_kind(), Storage::Temporary);
        let root = f.body().places[target as usize].ty;
        if forged {
            let mut row = interner().get(root).unwrap().clone();
            row.modes.uniqueness = Uniqueness::Unique;
            row.modes.sendability = Sendability::Sendable;
            let root = f.definition.types.push(row);
            f.body().places[target as usize].ty = root;
            f.body().locals[target as usize].ty = root;
        }
        let mut signature = function_signature("DATA", vec![], string_kind());
        let SignatureKind::Function(function) = &signature.kind else {
            unreachable!()
        };
        let root = function.result;
        let mut row = interner().get(root).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        row.modes.sendability = Sendability::Sendable;
        let root = interner().intern(row);
        signature.kind = SignatureKind::Const {
            ty: root,
            generics: Default::default(),
        };
        f.signatures.insert(id("DATA"), signature);
        f.assign(
            target,
            Rvalue::ReadConstant {
                arguments: vec![],
                definition: path("DATA"),
            },
            0,
        );
        let cleanup = f.cleanup(vec![drop(target)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        if forged {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "constant declaration"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn named_reference_specializations_check_declared_bounds_independently() {
    for valid in [true, false] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let target = f.slot(
            TypeKind::Function(fn_type(vec![], unit)),
            Storage::Temporary,
        );
        let mut signature = function_signature("target", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(header) = &mut signature.kind else {
            unreachable!()
        };
        header.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Plain].into(),
        }]
        .into();
        f.signatures.insert(id("target"), signature);
        let actual = f.definition.types.push(ty(if valid {
            TypeKind::Primitive(Primitive::U64)
        } else {
            string_kind()
        }));
        f.assign(
            target,
            Rvalue::FunctionReference {
                definition: path("target"),
                arguments: vec![GenericArgument::Type(actual)],
            },
            0,
        );
        let cleanup = f.cleanup(vec![drop(target)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        if valid {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "function reference bound"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

fn closure_string_construction_fixture(allocation: Allocation, escapes: bool) -> Fixture {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let string = f.definition.types.push(ty(string_kind()));
    let result = if escapes { string } else { unit };
    let mut child = empty_body(&mut f);
    let text = add_capture(&mut child, string);
    child.locals[text as usize].storage = Storage::Temporary;
    child.blocks[0].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(text),
            value: RvalueId(1),
        },
        0,
    ));
    child
        .rvalues
        .push(Rvalue::Constant(Constant::String("new".into())));
    let cleanup = if escapes {
        0
    } else {
        let id = CleanupId(child.cleanups.len() as u32);
        let node = CleanupNodeId(child.cleanup_nodes.len() as u32);
        child.cleanup_nodes.push(CleanupNode {
            action: drop(text),
            parent: None,
        });
        child.cleanups.push(Cleanup { head: Some(node) });
        id.0
    };
    child.result = result;
    child.blocks[0].terminator = ret(if escapes { text } else { 0 }, cleanup);
    f.definition.bodies.push(child);
    let mut function = fn_type(vec![], result);
    function.allocation = allocation;
    let value = f.slot(TypeKind::Function(function), Storage::Local);
    f.assign(
        value,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(value)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn closure_allocation_promises_distinguish_local_construction_from_counted_destinations() {
    // `in heap` is the empty relation (PROV-4): a construction is counted
    // exactly when its value escapes the noalloc frame.
    for escapes in [false, true] {
        closure_string_construction_fixture(Allocation::MayAllocate, escapes).valid();
        let f = closure_string_construction_fixture(Allocation::NoAlloc, escapes);
        if escapes {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "body construction allocation"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

/// A noalloc closure body that builds a pair `("new", 7)` and returns a
/// copy of its part `field` through a temporary.
fn closure_pair_part_fixture(field: u32) -> Fixture {
    let mut f = Fixture::unit();
    let string = f.definition.types.push(ty(string_kind()));
    let number = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let pair = f
        .definition
        .types
        .push(ty(TypeKind::Tuple(vec![string, number].into())));
    let pair = normalize_type_properties(
        interner(),
        pair,
        &hirsel_types::GenericDeclarations::default(),
        &mut |_| Ok::<_, hirsel_types::QueryError>(None),
    )
    .unwrap();
    let TypeKind::Tuple(fields) = &interner().get(pair).unwrap().kind else {
        panic!()
    };
    let [string, number] = [fields[0], fields[1]];
    let part = [string, number][field as usize];
    let mut child = empty_body(&mut f);
    let [text, count, both, out] = [string, number, pair, part].map(|ty| {
        let place = add_capture(&mut child, ty);
        child.locals[place as usize].storage = Storage::Temporary;
        place
    });
    let projected = child.places.len() as u32;
    child.places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(both),
        projection: vec![Projection::Field(field)].into(),
        ty: part,
    });
    let text_node = CleanupNodeId(child.cleanup_nodes.len() as u32);
    child.cleanup_nodes.push(CleanupNode {
        action: drop(text),
        parent: None,
    });
    let holding_text = child.cleanups.len() as u32;
    child.cleanups.push(Cleanup {
        head: Some(text_node),
    });
    for (destination, value, on_failure) in [
        (text, Rvalue::Constant(Constant::String("new".into())), 0),
        (count, Rvalue::Constant(Constant::Integer(7)), holding_text),
        (
            both,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: take(text),
                    },
                    ConstructField {
                        field: 1,
                        value: take(count),
                    },
                ],
                destination: Destination::Inferred,
            },
            holding_text,
        ),
        (out, Rvalue::Use(copy(projected)), 0),
    ] {
        let id = RvalueId(child.rvalues.len() as u32);
        child.rvalues.push(value);
        child.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(destination),
                value: id,
            },
            on_failure,
        ));
    }
    let node = CleanupNodeId(child.cleanup_nodes.len() as u32);
    child.cleanup_nodes.push(CleanupNode {
        action: drop(both),
        parent: None,
    });
    let cleanup = child.cleanups.len() as u32;
    child.cleanups.push(Cleanup { head: Some(node) });
    child.result = part;
    child.blocks[0].terminator = ret(out, cleanup);
    f.definition.bodies.push(child);
    let value = f.slot(TypeKind::Function(fn_type(vec![], part)), Storage::Local);
    f.assign(
        value,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(value)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn a_plain_part_read_out_of_a_local_construction_leaves_it_in_the_frame() {
    // MEM-5: a region-local frame construction charges no heap allocation.
    // A `u64` copied out of a pair holding a fresh String owns none of its
    // blocks, so returning it does not make the String escape the noalloc
    // frame; returning a copy of the String part does.
    closure_pair_part_fixture(1).valid();
    assert!(
        closure_pair_part_fixture(0)
            .kinds()
            .contains(&ViolationKind::TypeMismatch {
                entity: "body construction allocation"
            }),
        "{:?}",
        closure_pair_part_fixture(0).kinds()
    );
}

#[test]
fn noalloc_callee_signatures_do_not_hide_counted_argument_construction() {
    for taken in [false, true] {
        let mut f = closure_string_construction_fixture(Allocation::NoAlloc, false);
        let mut signature = function_signature(
            "use_text",
            vec![(
                if taken {
                    PassingMode::Take
                } else {
                    PassingMode::Borrow
                },
                string_kind(),
            )],
            TypeKind::Tuple(vec![].into()),
        );
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        if !taken {
            let mut row = interner().get(function.parameters[0].ty).unwrap().clone();
            row.modes.relation = Relation::environment();
            function.parameters[0].ty = interner().intern(row);
        }
        f.signatures.insert(id("use_text"), signature);
        let child = &mut f.definition.bodies[1];
        let unit = child.result;
        let result = add_capture(child, unit);
        child.locals[result as usize].storage = Storage::Temporary;
        let loan_place = if !taken {
            let mut access = child.places[1].clone();
            access.loan = Some(LoanId(0));
            let id = PlaceId(child.places.len() as u32);
            child.places.push(access);
            child.blocks[0].instructions.push(inst(
                InstructionKind::BeginLoan {
                    id: LoanId(0),
                    place: PlaceId(1),
                    mutable: false,
                    if_inactive: false,
                },
                0,
            ));
            id.0
        } else {
            1
        };
        let failure = if !taken {
            child.add_cleanup(vec![
                CleanupAction::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                drop(1),
            ])
        } else {
            CleanupId(0)
        };
        child.blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("use_text"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(
                    loan_place,
                    if taken {
                        UseKind::Take
                    } else {
                        UseKind::Borrow
                    },
                ),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: failure,
        };
        child.blocks.push(block(
            if taken {
                vec![]
            } else {
                vec![inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    },
                    0,
                )]
            },
            ret(result, if taken { 0 } else { 1 }),
        ));
        if taken {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "body construction allocation"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn retained_call_results_carry_the_construction_destination_of_selected_arguments() {
    for allocation in [Allocation::NoAlloc, Allocation::MayAllocate] {
        let mut f = closure_string_construction_fixture(allocation, true);
        let mut signature = function_signature(
            "view",
            vec![(PassingMode::Borrow, string_kind())],
            string_kind(),
        );
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        let mut parameter = interner().get(function.parameters[0].ty).unwrap().clone();
        parameter.modes.relation = Relation::environment();
        function.parameters[0].ty = interner().intern(parameter);
        let mut result = interner().get(function.result).unwrap().clone();
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        function.result = interner().intern(result);
        f.signatures.insert(id("view"), signature);
        let child = &mut f.definition.bodies[1];
        let result_ty = child.result;
        let result = add_capture(child, result_ty);
        child.locals[result as usize].storage = Storage::Temporary;
        let cleanup = child.add_cleanup(vec![drop(1)]);
        let mut access = child.places[1].clone();
        access.loan = Some(LoanId(0));
        let loan_place = PlaceId(child.places.len() as u32);
        child.places.push(access);
        child.blocks[0].instructions.push(inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(1),
                mutable: false,
                if_inactive: false,
            },
            0,
        ));
        let failure = child.add_cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(1),
        ]);
        child.blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("view"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(loan_place.0, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: failure,
        };
        child.blocks.push(block(
            vec![inst(
                InstructionKind::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                0,
            )],
            ret(result, cleanup.0),
        ));
        if allocation == Allocation::MayAllocate {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "body construction allocation"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn closure_allocation_checks_counted_sequence_map_set_and_box_construction() {
    for shape in 0..4 {
        for allocation in [Allocation::NoAlloc, Allocation::MayAllocate] {
            let mut f = closure_string_construction_fixture(allocation, true);
            let number = f
                .definition
                .types
                .push(ty(TypeKind::Primitive(Primitive::U64)));
            let old = f.definition.bodies[1].locals[1].ty;
            let mut row = interner().get(old).unwrap().clone();
            row.kind = match shape {
                0 => TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: vec![GenericArgument::Type(number)].into(),
                },
                1 => TypeKind::Builtin {
                    kind: BuiltinType::Map,
                    arguments: vec![GenericArgument::Type(number), GenericArgument::Type(number)]
                        .into(),
                },
                2 => TypeKind::Builtin {
                    kind: BuiltinType::Set,
                    arguments: vec![GenericArgument::Type(number)].into(),
                },
                _ => TypeKind::Box(number),
            };
            let aggregate = f.definition.types.push(row);
            let closure_root = f.body().places[1].ty;
            let mut row = interner().get(closure_root).unwrap().clone();
            let TypeKind::Function(function) = &mut row.kind else {
                panic!()
            };
            function.result = aggregate;
            let callable = f.definition.types.push(row);
            f.body().locals[1].ty = callable;
            f.body().places[1].ty = callable;
            let entry_type = if shape == 1 {
                let tuple = f
                    .definition
                    .types
                    .push(ty(TypeKind::Tuple(vec![number, number].into())));
                normalize_type_properties(
                    interner(),
                    tuple,
                    &hirsel_types::GenericDeclarations::default(),
                    &mut |_| Ok::<_, hirsel_types::QueryError>(None),
                )
                .unwrap()
            } else {
                number
            };
            let child = &mut f.definition.bodies[1];
            child.result = aggregate;
            child.locals[1].ty = aggregate;
            child.places[1].ty = aggregate;
            let value = add_capture(child, number);
            child.locals[value as usize].storage = Storage::Temporary;
            let scalar = RvalueId(child.rvalues.len() as u32);
            child.rvalues.push(Rvalue::Constant(Constant::Integer(7)));
            child.blocks[0].instructions.insert(
                1,
                inst(
                    InstructionKind::Assign {
                        destination: PlaceId(value),
                        value: scalar,
                    },
                    0,
                ),
            );
            let entry = if shape == 1 {
                let entry = add_capture(child, entry_type);
                child.locals[entry as usize].storage = Storage::Temporary;
                let tuple = RvalueId(child.rvalues.len() as u32);
                child.rvalues.push(Rvalue::Construct {
                    constructor: Constructor::Tuple,
                    fields: vec![
                        ConstructField {
                            field: 0,
                            value: copy(value),
                        },
                        ConstructField {
                            field: 1,
                            value: copy(value),
                        },
                    ],
                    destination: Destination::Inferred,
                });
                child.blocks[0].instructions.insert(
                    2,
                    inst(
                        InstructionKind::Assign {
                            destination: PlaceId(entry),
                            value: tuple,
                        },
                        0,
                    ),
                );
                entry
            } else {
                value
            };
            child.rvalues[1] = Rvalue::Construct {
                constructor: match shape {
                    0 => Constructor::Sequence,
                    1 => Constructor::Map,
                    2 => Constructor::Set,
                    _ => Constructor::Box,
                },
                fields: vec![ConstructField {
                    field: 0,
                    value: copy(entry),
                }],
                destination: Destination::Heap,
            };
            let construction = child.blocks[0].instructions.remove(0);
            child.blocks[0].instructions.push(construction);
            if allocation == Allocation::MayAllocate {
                f.valid();
            } else {
                assert!(
                    f.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "body construction allocation"
                    }),
                    "shape={shape}: {:?}",
                    f.kinds()
                );
            }
        }
    }
}

#[test]
fn replacement_loan_death_is_proved_from_installed_content() {
    for keeps_view in [false, true] {
        let mut f = Fixture::unit();
        let source = param(&mut f, string_kind());
        let receiver = f.slot(string_kind(), Storage::Local);
        let prepared = f.slot(string_kind(), Storage::Temporary);
        let displaced = f.slot(string_kind(), Storage::Temporary);
        let observed = f.slot(string_kind(), Storage::Temporary);
        for place in [source, receiver, displaced, observed] {
            let previous = f.body().places[place as usize].ty;
            let mut row = interner().get(previous).unwrap().clone();
            row.modes.relation = Relation::environment();
            let local = f.definition.types.push(row);
            f.body().locals[place as usize].ty = local;
            f.body().places[place as usize].ty = local;
        }
        let mut view = function_signature(
            "view",
            vec![(PassingMode::Borrow, string_kind())],
            string_kind(),
        );
        let SignatureKind::Function(function) = &mut view.kind else {
            unreachable!()
        };
        let mut parameter = interner().get(function.parameters[0].ty).unwrap().clone();
        parameter.modes.relation = Relation::environment();
        function.parameters[0].ty = interner().intern(parameter);
        let mut result = interner().get(function.result).unwrap().clone();
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        function.result = interner().intern(result);
        f.signatures.insert(id("view"), view);
        let via = f.through_loan(source, 0);
        f.body().blocks[0].instructions.push(inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(source),
                mutable: false,
                if_inactive: false,
            },
            0,
        ));
        let call_failure = f.cleanup(vec![CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        }]);
        let call = |destination, next, on_failure| Terminator::Call {
            callee: Callee::Definition {
                path: path("view"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(via, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(destination),
                next: BlockId(next),
            }),
            on_failure: CleanupId(on_failure),
        };
        f.body().blocks[0].terminator = call(receiver, 1, call_failure);
        let failure = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(receiver),
            drop(prepared),
        ]);
        f.body()
            .blocks
            .push(block(vec![], Terminator::Goto(BlockId(2))));
        if keeps_view {
            let local = f.body().places[receiver as usize].ty;
            f.body().locals[prepared as usize].ty = local;
            f.body().places[prepared as usize].ty = local;
            f.body().blocks[1].terminator = call(prepared, 2, failure);
        } else {
            f.body().blocks[1].instructions.push(inst(
                InstructionKind::Assign {
                    destination: PlaceId(prepared),
                    value: RvalueId(1),
                },
                failure,
            ));
            f.body()
                .rvalues
                .push(Rvalue::Constant(Constant::String("fresh".into())));
        }
        let read = RvalueId(f.body().rvalues.len() as u32);
        f.body().rvalues.push(Rvalue::Use(copy(receiver)));
        let cleanup = f.cleanup(vec![drop(observed), drop(receiver)]);
        f.body().blocks.push(block(
            vec![
                inst(
                    InstructionKind::Replace {
                        destination: PlaceId(receiver),
                        value: take(prepared),
                        displaced: PlaceId(displaced),
                        if_live: false,
                    },
                    failure,
                ),
                inst(
                    InstructionKind::Drop {
                        place: PlaceId(displaced),
                        if_live: false,
                    },
                    failure,
                ),
                inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    },
                    0,
                ),
                inst(
                    InstructionKind::Assign {
                        destination: PlaceId(observed),
                        value: read,
                    },
                    cleanup,
                ),
            ],
            ret(0, cleanup),
        ));
        owner_sig(&mut f, &[PassingMode::Mut]);
        if keeps_view {
            assert!(
                f.kinds().iter().any(|kind| matches!(
                    kind,
                    ViolationKind::ExpiredLoan {
                        loan: LoanId(0),
                        ..
                    }
                )),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

fn repeated_read_loan_fixture() -> Fixture {
    let mut f = Fixture::unit();
    let source = param(&mut f, string_kind());
    let condition = param(&mut f, TypeKind::Primitive(Primitive::Bool));
    let receiver = f.slot(string_kind(), Storage::Local);
    let prepared = f.slot(string_kind(), Storage::Temporary);
    let displaced = f.slot(string_kind(), Storage::Temporary);
    for place in [source, receiver, prepared, displaced] {
        let previous = f.body().places[place as usize].ty;
        let mut row = interner().get(previous).unwrap().clone();
        row.modes.relation = Relation::environment();
        let local = f.definition.types.push(row);
        f.body().locals[place as usize].ty = local;
        f.body().places[place as usize].ty = local;
    }
    let mut view = function_signature(
        "view",
        vec![(PassingMode::Borrow, string_kind())],
        string_kind(),
    );
    let SignatureKind::Function(function) = &mut view.kind else {
        unreachable!()
    };
    let mut parameter = interner().get(function.parameters[0].ty).unwrap().clone();
    parameter.modes.relation = Relation::environment();
    function.parameters[0].ty = interner().intern(parameter);
    let mut result = interner().get(function.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    function.result = interner().intern(result);
    f.signatures.insert(id("view"), view);
    f.assign(receiver, Rvalue::Use(copy(source)), 0);
    f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
    f.body().blocks.push(block(
        vec![],
        Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(2),
            no: BlockId(3),
        },
    ));
    let via = f.through_loan(source, 0);
    let begin = inst(
        InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(source),
            mutable: false,
            if_inactive: true,
        },
        0,
    );
    let failure = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(receiver),
        drop(prepared),
    ]);
    f.body().blocks.push(block(
        vec![begin],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("view"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(via, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(prepared),
                next: BlockId(4),
            }),
            on_failure: CleanupId(failure),
        },
    ));
    let cleanup = f.cleanup(vec![drop(receiver)]);
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: true,
            },
            0,
        )],
        ret(0, cleanup),
    ));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Replace {
                    destination: PlaceId(receiver),
                    value: take(prepared),
                    displaced: PlaceId(displaced),
                    if_live: false,
                },
                failure,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(displaced),
                    if_live: false,
                },
                failure,
            ),
        ],
        Terminator::Goto(BlockId(1)),
    ));
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Borrow]);
    f
}

#[test]
fn conditional_read_begins_require_mixed_exact_source_permissions() {
    let valid = repeated_read_loan_fixture();
    valid.valid();
    for mutation in ["strict", "must-inactive", "mutable", "different-source"] {
        let mut f = repeated_read_loan_fixture();
        match mutation {
            "strict" => {
                let InstructionKind::BeginLoan { if_inactive, .. } =
                    &mut f.body().blocks[2].instructions[0].kind
                else {
                    unreachable!()
                };
                *if_inactive = false;
            }
            "mutable" => {
                let InstructionKind::BeginLoan { mutable, .. } =
                    &mut f.body().blocks[2].instructions[0].kind
                else {
                    unreachable!()
                };
                *mutable = true;
            }
            "must-inactive" => f.body().blocks[4].instructions.push(inst(
                InstructionKind::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                0,
            )),
            "different-source" => f.body().blocks[0].instructions.push(inst(
                InstructionKind::BeginLoan {
                    id: LoanId(0),
                    place: PlaceId(2),
                    mutable: false,
                    if_inactive: false,
                },
                0,
            )),
            _ => unreachable!(),
        }
        let kinds = f.kinds();
        let expected = match mutation {
            "strict" => kinds.iter().any(|kind| {
                matches!(
                    kind,
                    ViolationKind::LoanConflict {
                        loan: LoanId(0),
                        ..
                    }
                )
            }),
            "different-source" => kinds.contains(&ViolationKind::Duplicate {
                entity: "loan",
                index: 0,
            }),
            _ => kinds.contains(&ViolationKind::WrongKind {
                entity: "conditional loan begin",
                index: 0,
            }),
        };
        assert!(expected, "{mutation}: {kinds:?}");
    }
}

#[test]
fn projected_replacement_releases_only_its_installed_content_dependencies() {
    for end_sibling_early in [false, true] {
        let mut f = Fixture::unit();
        let left = param(&mut f, string_kind());
        let right = param(&mut f, string_kind());
        let prepared = f.slot(string_kind(), Storage::Temporary);
        let displaced = f.slot(string_kind(), Storage::Temporary);
        for place in [left, right, displaced] {
            let previous = f.body().places[place as usize].ty;
            let mut row = interner().get(previous).unwrap().clone();
            row.modes.relation = Relation::environment();
            let local = f.definition.types.push(row);
            f.body().locals[place as usize].ty = local;
            f.body().places[place as usize].ty = local;
        }
        let field_ty = f.body().places[left as usize].ty;
        let tuple = f.slot(
            TypeKind::Tuple(vec![field_ty, field_ty].into()),
            Storage::Local,
        );
        let observed = f.slot(
            TypeKind::Tuple(vec![field_ty, field_ty].into()),
            Storage::Temporary,
        );
        for place in [tuple, observed] {
            let previous = f.body().places[place as usize].ty;
            let mut row = interner().get(previous).unwrap().clone();
            row.modes.relation = Relation::environment();
            let local = f.definition.types.push(row);
            f.body().locals[place as usize].ty = local;
            f.body().places[place as usize].ty = local;
        }
        let field = f.body().places.len() as u32;
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(tuple),
            projection: vec![Projection::Field(0)].into(),
            ty: field_ty,
        });
        let a = f.through_loan(left, 0);
        let b = f.through_loan(right, 1);
        for (id, source) in [(0, left), (1, right)] {
            f.emit(InstructionKind::BeginLoan {
                id: LoanId(id),
                place: PlaceId(source),
                mutable: false,
                if_inactive: false,
            });
        }
        let failure_before = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            CleanupAction::EndLoan {
                loan: LoanId(1),
                if_active: false,
            },
        ]);
        let failure_after = f.cleanup(vec![
            drop(tuple),
            CleanupAction::EndLoan {
                loan: LoanId(1),
                if_active: false,
            },
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        ]);
        f.assign(
            tuple,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![
                    ConstructField {
                        field: 0,
                        value: operand(a, UseKind::Borrow),
                    },
                    ConstructField {
                        field: 1,
                        value: operand(b, UseKind::Borrow),
                    },
                ],
                destination: Destination::Inferred,
            },
            failure_before,
        );
        f.assign(
            prepared,
            Rvalue::Constant(Constant::String("fresh".into())),
            0,
        );
        f.emit(InstructionKind::Replace {
            destination: PlaceId(field),
            value: take(prepared),
            displaced: PlaceId(displaced),
            if_live: false,
        });
        f.body().blocks[0].instructions.push(inst(
            InstructionKind::Drop {
                place: PlaceId(displaced),
                if_live: false,
            },
            failure_after,
        ));
        f.emit(InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        });
        if end_sibling_early {
            f.emit(InstructionKind::EndLoan {
                loan: LoanId(1),
                if_active: false,
            });
        }
        f.assign(observed, Rvalue::Use(take(tuple)), 0);
        let mut cleanup = vec![];
        if !end_sibling_early {
            cleanup.push(CleanupAction::EndLoan {
                loan: LoanId(1),
                if_active: false,
            });
        }
        cleanup.push(drop(observed));
        let cleanup = f.cleanup(cleanup);
        f.body().blocks[0].terminator = ret(0, cleanup);
        owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Mut]);
        if end_sibling_early {
            assert!(
                f.kinds().iter().any(|kind| matches!(
                    kind,
                    ViolationKind::ExpiredLoan {
                        loan: LoanId(1),
                        ..
                    }
                )),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

fn owned_local_field_receiver_fixture() -> Fixture {
    let mut f = Fixture::unit();
    let source = param(&mut f, string_kind());
    let seed = f.slot(string_kind(), Storage::Temporary);
    let prepared = f.slot(string_kind(), Storage::Temporary);
    let displaced = f.slot(string_kind(), Storage::Temporary);
    let observed = f.slot(string_kind(), Storage::Temporary);
    let global = f.body().places[seed as usize].ty;
    for place in [source, prepared, displaced, observed] {
        let mut row = interner().get(global).unwrap().clone();
        row.modes.relation = Relation::environment();
        let local = f.definition.types.push(row);
        f.body().locals[place as usize].ty = local;
        f.body().places[place as usize].ty = local;
    }
    let tuple = f.slot(TypeKind::Tuple(vec![global].into()), Storage::Local);
    let previous = f.body().places[tuple as usize].ty;
    let mut row = interner().get(previous).unwrap().clone();
    row.modes.relation = Relation::environment();
    let local = f.definition.types.push(row);
    f.body().locals[tuple as usize].ty = local;
    f.body().places[tuple as usize].ty = local;
    let field = f.body().places.len() as u32;
    let field_ty = f.body().places[prepared as usize].ty;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(tuple),
        projection: vec![Projection::Field(0)].into(),
        ty: field_ty,
    });
    f.assign(seed, Rvalue::Constant(Constant::String("seed".into())), 0);
    let construct_failure = f.cleanup(vec![CleanupAction::Drop {
        place: PlaceId(seed),
        if_live: true,
    }]);
    f.assign(
        tuple,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(seed),
            }],
            destination: Destination::Inferred,
        },
        construct_failure,
    );
    let via = f.through_loan(source, 0);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(source),
        mutable: false,
        if_inactive: false,
    });
    let failure = f.cleanup(vec![
        drop(tuple),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    let mut view = function_signature(
        "view",
        vec![(PassingMode::Borrow, string_kind())],
        string_kind(),
    );
    let SignatureKind::Function(function) = &mut view.kind else {
        unreachable!()
    };
    let mut parameter = interner().get(function.parameters[0].ty).unwrap().clone();
    parameter.modes.relation = Relation::environment();
    function.parameters[0].ty = interner().intern(parameter);
    let mut result = interner().get(function.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    function.result = interner().intern(result);
    f.signatures.insert(id("view"), view);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("view"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(via, UseKind::Borrow),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(prepared),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure),
    };
    let value = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(copy(field)));
    let cleanup = f.cleanup(vec![
        drop(observed),
        drop(tuple),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Replace {
                    destination: PlaceId(field),
                    value: take(prepared),
                    displaced: PlaceId(displaced),
                    if_live: false,
                },
                failure,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(displaced),
                    if_live: false,
                },
                failure,
            ),
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(observed),
                    value,
                },
                0,
            ),
        ],
        ret(0, cleanup),
    ));
    owner_sig(&mut f, &[PassingMode::Mut]);
    f
}

#[test]
fn owned_local_field_receivers_use_content_lifetimes_without_relaxing_caller_storage() {
    owned_local_field_receiver_fixture().valid();
    let mut expired = owned_local_field_receiver_fixture();
    expired.body().blocks[1].instructions.insert(
        2,
        inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        ),
    );
    assert!(
        expired.kinds().iter().any(|kind| matches!(
            kind,
            ViolationKind::ExpiredLoan {
                loan: LoanId(0),
                ..
            }
        )),
        "{:?}",
        expired.kinds()
    );
    let mut promoted = owned_local_field_receiver_fixture();
    let InstructionKind::Replace { displaced, .. } = promoted.body().blocks[1].instructions[0].kind
    else {
        unreachable!()
    };
    let previous = promoted.body().places[displaced.0 as usize].ty;
    let mut row = interner().get(previous).unwrap().clone();
    row.modes.relation = Relation::global();
    let global = promoted.definition.types.push(row);
    promoted.body().locals[displaced.0 as usize].ty = global;
    promoted.body().places[displaced.0 as usize].ty = global;
    assert!(promoted.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "displaced storage content"
    }));
    let mut caller = owned_local_field_receiver_fixture();
    let InstructionKind::Replace {
        destination, value, ..
    } = caller.body().blocks[1].instructions[0].kind.clone()
    else {
        unreachable!()
    };
    let root = caller.body().places[destination.0 as usize].root;
    caller.body().locals[root.0 as usize].storage = Storage::Parameter;
    caller.body().parameters.push(root);
    // A borrowed caller aggregate is live at entry; it is never constructed here.
    caller.body().blocks[0].instructions.drain(1..3);
    caller.body().rvalues.drain(1..3);
    if let InstructionKind::Assign { value, .. } = &mut caller.body().blocks[1].instructions[2].kind
    {
        *value = RvalueId(1);
    }
    owner_sig(&mut caller, &[PassingMode::Mut, PassingMode::Borrow]);
    assert!(
        caller
            .kinds()
            .contains(&ViolationKind::BorrowedMove(value.place)),
        "{:?}",
        caller.kinds()
    );
}

#[test]
fn postcondition_formals_cannot_claim_uniqueness_lost_by_the_parent() {
    for forged in [false, true] {
        let mut f = contract_clause_fixture(PassingMode::Borrow);
        let mut row = ty(string_kind());
        row.modes.uniqueness = Uniqueness::Unique;
        let unique = f.definition.types.push(row);
        // MODE-2: a weakened `mut x: unique` parameter fails its hand-back at
        // the next failure edge, so the parent's `x` is a plain `mut` one,
        // aliased once `weaken` may have shared it.
        let plain = f.definition.types.push(ty(string_kind()));
        let source = add_capture(f.body(), plain);
        f.body().locals[source as usize].storage = Storage::Parameter;
        f.body().parameters.push(LocalId(source));
        let mut signature = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
        let formal = interner().intern(ty(string_kind()));
        let SignatureKind::Function(header) = &mut signature.kind else {
            unreachable!()
        };
        header.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Mut,
            ty: formal,
        }]
        .into();
        f.signatures.insert(id("main"), signature);
        f.signatures.insert(
            id("weaken"),
            function_signature(
                "weaken",
                vec![(PassingMode::Mut, string_kind())],
                TypeKind::Tuple(vec![].into()),
            ),
        );
        let unit = unit_ty(&mut f);
        let destination = add_capture(f.body(), unit);
        f.body().locals[destination as usize].storage = Storage::Temporary;
        let mut access = f.body().places[source as usize].clone();
        access.loan = Some(LoanId(0));
        let exclusive = PlaceId(f.body().places.len() as u32);
        f.body().places.push(access.clone());
        access.loan = Some(LoanId(1));
        let borrowed = PlaceId(f.body().places.len() as u32);
        f.body().places.push(access);
        f.body().blocks[0].instructions.push(inst(
            InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(source),
                mutable: true,
                if_inactive: false,
            },
            0,
        ));
        let failure_mut = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(1),
        ]);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("weaken"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: Operand {
                    place: exclusive,
                    usage: UseKind::Mut,
                },
            }],
            returns: Some(CallReturn {
                destination: PlaceId(destination),
                next: BlockId(1),
            }),
            on_failure: CleanupId(failure_mut),
        };
        let function = PlaceId(1);
        let function_type = f.body().locals[1].ty;
        let TypeKind::Function(header) = &interner().get(function_type).unwrap().kind else {
            unreachable!()
        };
        let result = header.result;
        let result_slot = add_capture(f.body(), result);
        let result_root = f.body().places[result_slot as usize].root.0 as usize;
        f.body().locals[result_root].storage = Storage::Temporary;
        let failure_read = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(1),
                if_active: false,
            },
            drop(1),
        ]);
        f.body().blocks.push(block(
            vec![
                inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    },
                    0,
                ),
                inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(1),
                        place: PlaceId(source),
                        mutable: false,
                        if_inactive: false,
                    },
                    0,
                ),
            ],
            Terminator::Call {
                callee: Callee::Indirect {
                    forward_caller: true,
                    value: Operand {
                        place: function,
                        usage: UseKind::Copy,
                    },
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: Operand {
                        place: borrowed,
                        usage: UseKind::Borrow,
                    },
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(result_slot),
                    next: BlockId(2),
                }),
                on_failure: CleanupId(failure_read),
            },
        ));
        let cleanup = f.cleanup(vec![drop(function.0)]);
        f.body().blocks.push(block(
            vec![inst(
                InstructionKind::EndLoan {
                    loan: LoanId(1),
                    if_active: false,
                },
                0,
            )],
            ret(0, cleanup),
        ));
        if forged {
            let mut row = interner().get(function_type).unwrap().clone();
            let TypeKind::Function(header) = &mut row.kind else {
                unreachable!()
            };
            let mut parameters = header.parameters.to_vec();
            parameters[0].ty = unique;
            header.parameters = parameters.into();
            let function_type = f.definition.types.push(row);
            f.body().locals[1].ty = function_type;
            f.body().places[1].ty = function_type;
            f.definition.bodies[1].locals[1].ty = unique;
            f.definition.bodies[1].places[1].ty = unique;
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn a_temporary_callee_source_must_die_after_its_result_permission() {
    let mut f = scoped_capture_result_fixture();
    let signature = f.signatures.get_mut(&path("require_unique")).unwrap();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Aliased;
    fun.parameters[0].ty = interner().intern(row);
    f.valid();
    // The result is already dead, but the permission on its callee source must
    // end before that source is destroyed. Destruction ends captured loans later.
    f.body().blocks[2].instructions.swap(1, 2);
    let kinds = f.kinds();
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            ViolationKind::LoanOutlivesSource {
                loan: LoanId(2),
                ..
            }
        )),
        "dropping a still-borrowed temporary source was accepted: {kinds:?}"
    );
}

fn lexical_temporary_view_fixture() -> Fixture {
    let mut f = Fixture::unit();
    let source = f.slot(string_kind(), Storage::Temporary);
    let view = f.slot(string_kind(), Storage::Local);
    let previous = f.body().places[source as usize].ty;
    let mut row = interner().get(previous).unwrap().clone();
    row.modes.relation = Relation::environment();
    let local = f.definition.types.push(row);
    for place in [source, view] {
        let root = f.body().places[place as usize].root;
        f.body().locals[root.0 as usize].ty = local;
        f.body().places[place as usize].ty = local;
    }
    f.body().scopes.push(Scope {
        parent: Some(ScopeId(0)),
        span: Span::default(),
    });
    f.body().scopes.push(Scope {
        parent: Some(ScopeId(1)),
        span: Span::default(),
    });
    let view_root = f.body().places[view as usize].root;
    f.body().locals[view_root.0 as usize].scope = ScopeId(2);
    let root = f.body().places[source as usize].root;
    f.body().locals[root.0 as usize].scope = ScopeId(1);
    f.assign(
        source,
        Rvalue::Constant(Constant::String("local source".into())),
        0,
    );
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(source),
        mutable: false,
        if_inactive: false,
    });
    let through = f.through_loan(source, 0);
    let cleanup = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(source),
    ]);
    f.assign(
        view,
        Rvalue::Use(operand(through, UseKind::Borrow)),
        cleanup,
    );
    f.emit(InstructionKind::EndScope(ScopeId(2)));
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    f.emit(InstructionKind::Drop {
        place: PlaceId(source),
        if_live: false,
    });
    f.emit(InstructionKind::EndScope(ScopeId(1)));
    f
}

#[test]
fn views_of_lexical_temporaries_cannot_survive_the_source_scope() {
    lexical_temporary_view_fixture().valid();
    let mut forged = lexical_temporary_view_fixture();
    let instructions = &mut forged.body().blocks[0].instructions;
    let end = instructions
        .iter()
        .position(|instruction| matches!(instruction.kind, InstructionKind::EndLoan { .. }))
        .unwrap();
    instructions.swap(end, end + 1);
    let kinds = forged.kinds();
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            ViolationKind::LoanOutlivesSource {
                loan: LoanId(0),
                ..
            }
        )),
        "{kinds:?}"
    );
}

#[test]
fn constant_field_places_keep_declared_modes_independently_of_copy_receipts() {
    for forged in [false, true] {
        let mut f = Fixture::unit();
        let mut field = ty(string_kind());
        field.modes.uniqueness = Uniqueness::Unique;
        field.modes.sendability = Sendability::Sendable;
        let field = f.definition.types.push(field);
        let root = f.slot(TypeKind::Tuple(vec![field].into()), Storage::Temporary);
        let root_ty = f.body().places[root as usize].ty;
        let mut row = interner().get(root_ty).unwrap().clone();
        row.modes.sendability = Sendability::Sendable;
        let root_ty = f.definition.types.push(row);
        f.body().locals[root as usize].ty = root_ty;
        f.body().places[root as usize].ty = root_ty;
        let mut signature = function_signature("DATA", vec![], TypeKind::Tuple(vec![].into()));
        let constant = root_ty;
        signature.kind = SignatureKind::Const {
            ty: constant,
            generics: Default::default(),
        };
        f.signatures.insert(id("DATA"), signature);
        // Reading the constant copies its counted owner; physical field
        // contracts survive, while current whole-value evidence does not.
        let mut copied = interner().get(root_ty).unwrap().clone();
        copied.modes.sendability = Sendability::NotSendable;
        let copied = f.definition.types.push(copied);
        f.body().locals[root as usize].ty = copied;
        f.body().places[root as usize].ty = copied;
        f.assign(
            root,
            Rvalue::ReadConstant {
                arguments: vec![],
                definition: path("DATA"),
            },
            0,
        );
        let field_ty = if forged {
            f.definition.types.push(ty(string_kind()))
        } else {
            field
        };
        let projection = PlaceId(f.body().places.len() as u32);
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(root),
            projection: vec![Projection::Field(0)].into(),
            ty: field_ty,
        });
        let cleanup = f.cleanup(vec![drop(root)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        if forged {
            assert!(
                f.kinds()
                    .contains(&ViolationKind::InvalidProjection(projection)),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn aggregate_fields_cannot_keep_a_removed_call_locality_binder() {
    for forged in [false, true] {
        let mut f = captured_generic_copy(true, false, false);
        let child = &f.definition.bodies[1];
        let source = child.places[1].ty;
        let child_ty = if forged {
            let mut row = interner().get(source).unwrap().clone();
            row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
            f.definition.types.push(row)
        } else {
            source
        };
        let aggregate = f
            .definition
            .types
            .push(ty(TypeKind::Tuple(vec![child_ty].into())));
        let child = &mut f.definition.bodies[1];
        child.locals[2].ty = aggregate;
        child.places[2].ty = aggregate;
        child.rvalues[1] = Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: copy(1),
            }],
            destination: Destination::Inferred,
        };
        if forged {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "constructor field"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid()
        }
    }
}

fn closure_unique_extraction_fixture(
    allocation: Allocation,
    shape: u8,
    source_unique: bool,
) -> Fixture {
    let mut f = closure_string_construction_fixture(allocation, true);
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let string = f.definition.types.push(ty(string_kind()));
    let mut shared = ty(TypeKind::Builtin {
        kind: BuiltinType::Shared,
        arguments: vec![GenericArgument::Type(string)].into(),
    });
    shared.modes.sendability = Sendability::Sendable;
    let shared = f.definition.types.push(shared);
    let kind = match shape {
        0 => string_kind(),
        1 => TypeKind::Primitive(Primitive::U64),
        2 => TypeKind::Tuple(vec![scalar, string].into()),
        3 => TypeKind::Tuple(vec![scalar, scalar].into()),
        4 => interner().get(shared).unwrap().kind.clone(),
        5 => TypeKind::Tuple(vec![shared].into()),
        _ => unreachable!(),
    };
    let mut row = ty(kind);
    if shape >= 4 {
        row.modes.sendability = Sendability::Sendable;
    }
    if source_unique {
        row.modes.uniqueness = Uniqueness::Unique;
    }
    let source = f.definition.types.push(row.clone());
    if shape != 4 {
        row.modes.uniqueness = Uniqueness::Unique;
    }
    let result = f.definition.types.push(row);
    let child = &mut f.definition.bodies[1];
    child.locals[1].ty = result;
    child.places[1].ty = result;
    child.result = result;
    let parameter = add_capture(child, source);
    let root = child.places[parameter as usize].root;
    child.locals[root.0 as usize].storage = Storage::Parameter;
    child.parameters.push(root);
    child.rvalues[1] = Rvalue::Unary {
        op: hirsel_syntax::UnaryOp::Unique,
        value: copy(parameter),
    };
    let function_ty = f.definition.bodies[0].locals[1].ty;
    let mut row = interner().get(function_ty).unwrap().clone();
    let TypeKind::Function(header) = &mut row.kind else {
        unreachable!()
    };
    header.result = result;
    header.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Borrow,
        ty: source,
    }]
    .into();
    let function_ty = f.definition.types.push(row);
    f.body().locals[1].ty = function_ty;
    f.body().places[1].ty = function_ty;
    f
}

#[test]
fn closure_allocation_promises_cover_counted_deep_unique_extraction() {
    for shape in 0..6 {
        closure_unique_extraction_fixture(Allocation::MayAllocate, shape, false).valid();
        let noalloc = closure_unique_extraction_fixture(Allocation::NoAlloc, shape, false);
        if matches!(shape, 1 | 3 | 4 | 5) {
            noalloc.valid()
        } else {
            assert!(
                noalloc.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "body unique extraction allocation"
                }),
                "{:?}",
                noalloc.kinds()
            );
        }
    }
}

#[test]
fn noalloc_cannot_trust_unique_source_modes_or_discarded_results() {
    for discarded in [false, true] {
        for allocation in [Allocation::MayAllocate, Allocation::NoAlloc] {
            let mut f = closure_unique_extraction_fixture(allocation, 0, true);
            if discarded {
                let unit = f.definition.bodies[1].locals[0].ty;
                let child = &mut f.definition.bodies[1];
                child.result = unit;
                let node = CleanupNodeId(child.cleanup_nodes.len() as u32);
                child.cleanup_nodes.push(CleanupNode {
                    action: drop(1),
                    parent: None,
                });
                let cleanup = CleanupId(child.cleanups.len() as u32);
                child.cleanups.push(Cleanup { head: Some(node) });
                child.blocks[0].terminator = ret(0, cleanup.0);
                let function_ty = f.definition.bodies[0].locals[1].ty;
                let mut row = interner().get(function_ty).unwrap().clone();
                let TypeKind::Function(header) = &mut row.kind else {
                    unreachable!()
                };
                header.result = unit;
                let function_ty = f.definition.types.push(row);
                f.body().locals[1].ty = function_ty;
                f.body().places[1].ty = function_ty;
            }
            if allocation == Allocation::MayAllocate {
                f.valid()
            } else {
                assert!(
                    f.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "body unique extraction allocation"
                    }),
                    "{:?}",
                    f.kinds()
                );
            }
        }
    }
}

#[test]
fn acquiring_a_local_item_copy_ends_only_its_storage_access_dependency() {
    for copied in [false, true] {
        let mut f = lexical_temporary_view_fixture();
        let local = f.body().places[2].ty;
        let held = f.slot(string_kind(), Storage::Local);
        let held_root = f.body().places[held as usize].root;
        f.body().locals[held_root.0 as usize].ty = local;
        f.body().places[held as usize].ty = local;
        let value = RvalueId(f.body().rvalues.len() as u32);
        f.body().rvalues.push(Rvalue::Use(operand(
            2,
            if copied {
                UseKind::Copy
            } else {
                UseKind::Borrow
            },
        )));
        let before_end = f.body().blocks[0]
            .instructions
            .iter()
            .position(|instruction| {
                matches!(instruction.kind, InstructionKind::EndScope(ScopeId(2)))
            })
            .unwrap();
        f.body().blocks[0].instructions.insert(
            before_end,
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(held),
                    value,
                },
                0,
            ),
        );
        let used = f.slot(string_kind(), Storage::Temporary);
        let used_root = f.body().places[used as usize].root;
        f.body().locals[used_root.0 as usize].ty = local;
        f.body().places[used as usize].ty = local;
        let failure = f.cleanup(if copied { vec![drop(held)] } else { vec![] });
        for instruction in &mut f.body().blocks[0].instructions {
            if matches!(instruction.kind, InstructionKind::Drop { .. }) {
                instruction.on_failure = CleanupId(failure);
            }
        }
        f.assign(used, Rvalue::Use(copy(held)), failure);
        let cleanup = f.cleanup(vec![drop(used), drop(held)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        if copied {
            // The fixture owns a region-local literal, not an enduring cursor
            // payload: copying its view cannot outlive that region (OWN-1/MEM-10).
            assert!(
                f.kinds()
                    .contains(&ViolationKind::InvalidResultSource(PlaceId(held))),
                "{:?}",
                f.kinds()
            );
        } else {
            assert!(
                f.kinds()
                    .iter()
                    .any(|kind| matches!(kind, ViolationKind::ExpiredLoan { .. })),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn inline_replacement_storage_is_derived_from_the_root_representation() {
    let mut f = Fixture::unit();
    let string = f.definition.types.push(ty(string_kind()));
    let record = param(
        &mut f,
        TypeKind::Record(
            vec![
                Field {
                    name: "left".into(),
                    ty: string,
                    growable: false,
                },
                Field {
                    name: "right".into(),
                    ty: string,
                    growable: false,
                },
            ]
            .into(),
        ),
    );
    let incoming = param(&mut f, string_kind());
    let displaced = f.slot(string_kind(), Storage::Temporary);
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        root: LocalId(record),
        projection: vec![Projection::Field(0)].into(),
        ty: string,
        loan: None,
        binding: None,
    });
    f.emit(InstructionKind::Replace {
        destination: PlaceId(field),
        value: take(incoming),
        displaced: PlaceId(displaced),
        if_live: false,
    });
    f.emit(InstructionKind::Drop {
        place: PlaceId(displaced),
        if_live: false,
    });
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Take]);
    f.valid();
    // A counted box adds a storage edge. It cannot keep the inline field path
    // and label the payload as the old string field.
    let previous = f.body().locals[record as usize].ty;
    let boxed = f.definition.types.push(ty(TypeKind::Box(previous)));
    f.body().locals[record as usize].ty = boxed;
    f.body().places[record as usize].ty = boxed;
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Take]);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidProjection(PlaceId(field)))
    );
}

fn literal_body_fixture(
    mut child: Body,
    f: &mut Fixture,
    parameters: Vec<Parameter>,
    allocation: Allocation,
) {
    child.parameter_scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(901)));
    let mut header = fn_type(parameters, child.result);
    header.allocation = allocation;
    f.definition.bodies.push(child);
    let closure = f.slot(TypeKind::Function(header), Storage::Temporary);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let exit = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = ret(0, exit);
}
fn replacement_effect_fixture(
    container: Option<BuiltinType>,
    unique: bool,
    allocation: Allocation,
) -> Fixture {
    let mut f = Fixture::unit();
    let byte = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let mut arguments = vec![GenericArgument::Type(byte)];
    if container == Some(BuiltinType::Inline) {
        arguments.push(GenericArgument::Size(Size::Constant(1)))
    }
    let mut row = ty(match container {
        Some(kind) => TypeKind::Builtin {
            kind,
            arguments: arguments.into(),
        },
        None => TypeKind::Box(byte),
    });
    if unique {
        row.modes.uniqueness = Uniqueness::Unique;
        row.modes.sendability = Sendability::Sendable;
    }
    let array = f.definition.types.push(row);
    let index = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let mut child = empty_body(&mut f);
    let xs = add_capture(&mut child, array);
    child.locals[xs as usize].storage = Storage::Parameter;
    child.parameters.push(LocalId(xs));
    let idx = add_capture(&mut child, index);
    child.locals[idx as usize].storage = Storage::Temporary;
    let incoming = add_capture(&mut child, byte);
    child.locals[incoming as usize].storage = Storage::Temporary;
    let old = add_capture(&mut child, byte);
    child.locals[old as usize].storage = Storage::Temporary;
    let element = child.places.len() as u32;
    child.places.push(Place {
        root: LocalId(xs),
        projection: vec![if container.is_some() {
            Projection::Index(LocalId(idx))
        } else {
            Projection::Field(0)
        }]
        .into(),
        ty: byte,
        loan: None,
        binding: None,
    });
    for (place, n) in [(idx, 0), (incoming, 1)] {
        let value = child.rvalues.len() as u32;
        child.rvalues.push(Rvalue::Constant(Constant::Integer(n)));
        child.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(place),
                value: RvalueId(value),
            },
            0,
        ));
    }
    child.blocks[0].instructions.push(inst(
        InstructionKind::Replace {
            destination: PlaceId(element),
            value: take(incoming),
            displaced: PlaceId(old),
            if_live: false,
        },
        0,
    ));
    child.blocks[0].instructions.push(inst(
        InstructionKind::StorageDead {
            place: PlaceId(old),
            if_live: false,
        },
        0,
    ));
    literal_body_fixture(
        child,
        &mut f,
        vec![Parameter {
            name: Some("xs".into()),
            label: None,
            passing: PassingMode::Mut,
            ty: array,
        }],
        allocation,
    );
    f
}
#[test]
fn noalloc_literal_replacement_checks_counted_storage_independently() {
    for container in [Some(BuiltinType::Array), Some(BuiltinType::List), None] {
        replacement_effect_fixture(container, false, Allocation::MayAllocate).valid();
        replacement_effect_fixture(container, true, Allocation::NoAlloc).valid();
        let rejected = replacement_effect_fixture(container, false, Allocation::NoAlloc);
        assert!(rejected.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "body replacement allocation"
        }));
    }
    replacement_effect_fixture(Some(BuiltinType::Inline), false, Allocation::NoAlloc).valid();
}
fn destructor_effect_fixture(allocation: Allocation, transfer: bool, failure: bool) -> Fixture {
    let mut f = Fixture::unit();
    let mut signature = capability_signature("R");
    let rep = interner().intern(ty(TypeKind::Tuple(vec![].into())));
    let SignatureKind::Type(resource) = &mut signature.kind else {
        unreachable!()
    };
    resource.capability = None;
    resource.authority = AuthoritySummary::inert();
    resource.resource = true;
    resource.representation = Some(rep);
    resource.destructor_allocation = allocation;
    f.signatures.insert(id("R"), signature);
    // The forged cached row claims NoAlloc; declaration evidence must win.
    let resource = f.definition.types.push(ty(TypeKind::Named {
        definition: id("R"),
        arguments: vec![].into(),
    }));
    let mut child = empty_body(&mut f);
    let x = add_capture(&mut child, resource);
    child.locals[x as usize].storage = Storage::Parameter;
    child.parameters.push(LocalId(x));
    let exit = child.add_cleanup(vec![drop(x)]);
    if failure {
        let boolean = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::Bool)));
        let condition = add_capture(&mut child, boolean);
        child.locals[condition as usize].storage = Storage::Temporary;
        let rv = child.rvalues.len() as u32;
        child.rvalues.push(Rvalue::Constant(Constant::Bool(true)));
        child.blocks[0].instructions.push(inst(
            InstructionKind::Assign {
                destination: PlaceId(condition),
                value: RvalueId(rv),
            },
            0,
        ));
        child.blocks[0].instructions.push(inst(
            InstructionKind::Check {
                condition: operand(condition, UseKind::Borrow),
                rule: const { hirsel_diag::RuleId::of("FAIL-1") },
            },
            exit.0,
        ));
    }
    if transfer {
        child.result = resource;
        child.blocks[0].terminator = ret(x, exit.0)
    } else {
        child.blocks[0].terminator = ret(0, exit.0)
    }
    literal_body_fixture(
        child,
        &mut f,
        vec![Parameter {
            name: Some("x".into()),
            label: None,
            passing: PassingMode::Take,
            ty: resource,
        }],
        Allocation::NoAlloc,
    );
    f
}
#[test]
fn noalloc_literal_cleanup_uses_live_storage_and_declared_destructor_effects() {
    for failure in [false, true] {
        destructor_effect_fixture(Allocation::NoAlloc, false, failure).valid();
        let rejected = destructor_effect_fixture(Allocation::MayAllocate, false, failure);
        assert!(rejected.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "body destructor allocation"
        }));
    }
    destructor_effect_fixture(Allocation::MayAllocate, true, false).valid();
}

#[test]
fn noalloc_aggregate_cleanup_skips_the_transferred_allocating_field() {
    let mut f = destructor_effect_fixture(Allocation::MayAllocate, true, false);
    let resource = f.definition.bodies[1].locals[1].ty;
    let string = f.definition.types.push(ty(string_kind()));
    let tuple = f
        .definition
        .types
        .push(ty(TypeKind::Tuple(vec![resource, string].into())));
    let child = &mut f.definition.bodies[1];
    child.locals[1].ty = tuple;
    child.places[1].ty = tuple;
    let field = child.places.len() as u32;
    child.places.push(Place {
        root: LocalId(1),
        projection: vec![Projection::Field(0)].into(),
        ty: resource,
        loan: None,
        binding: None,
    });
    child.blocks[0].terminator = ret(field, 1);
    let original = f.body().locals[1].ty;
    let mut header = interner().get(original).unwrap().clone();
    let TypeKind::Function(function) = &mut header.kind else {
        unreachable!()
    };
    function.parameters[0].ty = tuple;
    let header = f.definition.types.push(header);
    f.body().locals[1].ty = header;
    f.body().places[1].ty = header;
    f.valid();
    // Returning only the plain sibling leaves the allocating resource live.
    let child = &mut f.definition.bodies[1];
    child.result = string;
    child.places[field as usize].ty = string;
    child.places[field as usize].projection = vec![Projection::Field(1)].into();
    let mut header = interner().get(header).unwrap().clone();
    if let TypeKind::Function(function) = &mut header.kind {
        function.result = string
    }
    let header = f.definition.types.push(header);
    f.body().locals[1].ty = header;
    f.body().places[1].ty = header;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "body destructor allocation"
    }));
}

#[test]
fn written_generic_receipt_requires_this_actuals_proof() {
    let mut f = generic_call_fixture(Bound::Copyable, string_kind());
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let SignatureKind::Function(function) = &mut signature.kind else {
        panic!()
    };
    let mut required = interner().get(function.parameters[0].ty).unwrap().clone();
    required.sendability_requirement = SendabilityRequirement::Sendable;
    required.modes.sendability = Sendability::Sendable;
    function.parameters[0].ty = interner().intern(required);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "call argument"
    }));
    // A checked entry assumption supplies occurrence evidence without unique
    // mutation permission. Every caller of this owner must establish it.
    let original = f.body().places[1].ty;
    let mut actual = interner().get(original).unwrap().clone();
    actual.sendability_requirement = SendabilityRequirement::Sendable;
    actual.modes.sendability = Sendability::Sendable;
    let actual = f.definition.types.push(actual);
    f.body().locals[1].ty = actual;
    f.body().places[1].ty = actual;
    owner_sig(&mut f, &[PassingMode::Take]);
    f.valid();
    // A legal counted copy invalidates the original proof too. The fixed
    // receiving requirement remains, but cannot establish this later receipt.
    let alias = f.slot(string_kind(), Storage::Temporary);
    f.assign(alias, Rvalue::Use(copy(1)), 0);
    f.emit(InstructionKind::Drop {
        place: PlaceId(alias),
        if_live: false,
    });
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "call argument"
    }));
}

#[test]
fn copying_generic_calls_cannot_hide_owned_crossing_contracts() {
    for nested in [false, true] {
        let mut f = generic_call_fixture(Bound::Copyable, string_kind());
        let original = f.body().places[1].ty;
        let mut actual = interner().get(original).unwrap().clone();
        actual.sendability_requirement = SendabilityRequirement::Sendable;
        actual.modes.sendability = Sendability::Sendable;
        let qualified = f.definition.types.push(actual.clone());
        if nested {
            actual.kind = TypeKind::Box(qualified);
        }
        let actual = f.definition.types.push(actual);
        f.body().locals[1].ty = actual;
        f.body().places[1].ty = actual;
        let argument = if nested {
            // The forbidden qualifier is solely inside the copied payload.
            let mut row = interner().get(actual).unwrap().clone();
            row.sendability_requirement = SendabilityRequirement::None;
            f.definition.types.push(row)
        } else {
            actual
        };
        let Terminator::Call {
            callee: Callee::Definition { arguments, .. },
            ..
        } = &mut f.body().blocks[0].terminator
        else {
            panic!()
        };
        arguments[0] = GenericArgument::Type(argument);
        owner_sig(&mut f, &[PassingMode::Take]);
        assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "generic crossing transport"
        }));
        let SignatureKind::Function(function) =
            &mut f.signatures.get_mut(&path("callee")).unwrap().kind
        else {
            panic!()
        };
        function.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Inert].into(),
        }]
        .into();
        f.valid();
    }
}

#[test]
fn enclosing_copy_invalidates_nested_generic_bound_evidence() {
    let mut f = generic_call_fixture(Bound::Sendable, string_kind());
    let original = f.body().places[1].ty;
    let mut part = interner().get(original).unwrap().clone();
    part.modes.sendability = Sendability::Sendable;
    let part = f.definition.types.push(part);
    let mut row = interner().get(original).unwrap().clone();
    row.kind = TypeKind::Tuple(vec![part].into());
    row.sendability_requirement = SendabilityRequirement::Sendable;
    row.modes.sendability = Sendability::Sendable;
    let tuple = f.definition.types.push(row);
    f.body().locals[1].ty = tuple;
    f.body().places[1].ty = tuple;
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let SignatureKind::Function(function) = &mut signature.kind else {
        panic!()
    };
    let template = interner().intern(ty(TypeKind::Tuple(vec![function.parameters[0].ty].into())));
    function.parameters[0].ty = template;
    let Terminator::Call {
        callee: Callee::Definition { arguments, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    arguments[0] = GenericArgument::Type(part);
    owner_sig(&mut f, &[PassingMode::Take]);
    f.valid();
    let alias = f.slot(TypeKind::Tuple(vec![part].into()), Storage::Temporary);
    f.assign(alias, Rvalue::Use(copy(1)), 0);
    f.emit(InstructionKind::Drop {
        place: PlaceId(alias),
        if_live: false,
    });
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "owned crossing receipt"
    }));
}

#[test]
fn mutable_qualified_receipts_require_current_root_and_part_proofs_on_exit() {
    for nested in [false, true] {
        let mut f = Fixture::unit();
        let x = param(&mut f, string_kind());
        let original = f.body().places[x as usize].ty;
        let mut qualified = interner().get(original).unwrap().clone();
        qualified.sendability_requirement = SendabilityRequirement::Sendable;
        qualified.modes.sendability = Sendability::Sendable;
        let part = f.definition.types.push(qualified.clone());
        if nested {
            qualified.kind = TypeKind::Tuple(vec![part].into());
            qualified.sendability_requirement = SendabilityRequirement::None;
            qualified.modes.sendability = Sendability::NotSendable;
        }
        let receiving = f.definition.types.push(qualified);
        f.body().places[x as usize].ty = receiving;
        f.body().locals[x as usize].ty = receiving;
        owner_sig(&mut f, &[PassingMode::Mut]);
        f.valid();
        let copied_kind = if nested {
            TypeKind::Tuple(vec![original].into())
        } else {
            string_kind()
        };
        let alias = f.slot(copied_kind, Storage::Temporary);
        f.assign(alias, Rvalue::Use(copy(x)), 0);
        f.emit(InstructionKind::Drop {
            place: PlaceId(alias),
            if_live: false,
        });
        let entity = if nested {
            "owned crossing receipt"
        } else {
            "mutable crossing hand-back"
        };
        assert!(f.kinds().contains(&ViolationKind::TypeMismatch { entity }));
    }
}

#[test]
fn substituted_copying_results_cannot_publish_unpromised_crossing_evidence() {
    let mut f = generic_call_fixture(Bound::Copyable, string_kind());
    let original = f.body().places[1].ty;
    let mut incoming = interner().get(original).unwrap().clone();
    incoming.sendability_requirement = SendabilityRequirement::Sendable;
    incoming.modes.sendability = Sendability::Sendable;
    let incoming = f.definition.types.push(incoming);
    f.body().locals[1].ty = incoming;
    f.body().places[1].ty = incoming;
    owner_sig(&mut f, &[PassingMode::Take]);
    let SignatureKind::Function(function) =
        &mut f.signatures.get_mut(&path("callee")).unwrap().kind
    else {
        panic!()
    };
    function.result = function.parameters[0].ty;
    let Terminator::Call {
        returns: Some(CallReturn { destination, .. }),
        ..
    } = f.body().blocks[0].terminator
    else {
        panic!()
    };
    let root = f.body().places[destination.0 as usize].root;
    f.body().locals[root.0 as usize].ty = original;
    f.body().places[destination.0 as usize].ty = original;
    f.body().blocks[1].instructions.push(inst(
        InstructionKind::Drop {
            place: destination,
            if_live: false,
        },
        0,
    ));
    f.body().blocks[1].terminator = ret(0, 0);
    f.valid();
    let mut claimed = interner().get(original).unwrap().clone();
    claimed.modes.sendability = Sendability::Sendable;
    let claimed = f.definition.types.push(claimed);
    f.body().locals[root.0 as usize].ty = claimed;
    f.body().places[destination.0 as usize].ty = claimed;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "call result"
    }));
}

#[test]
fn selected_borrow_result_read_permissions_cannot_hide_qualified_actuals() {
    for qualified_actual in [false, true] {
        let mut f = generic_call_fixture(Bound::Copyable, string_kind());
        let original = f.body().places[1].ty;
        let mut received = interner().get(original).unwrap().clone();
        received.sendability_requirement = SendabilityRequirement::Sendable;
        received.modes.sendability = Sendability::Sendable;
        let received = f.definition.types.push(received);
        f.body().locals[1].ty = received;
        f.body().places[1].ty = received;
        owner_sig(&mut f, &[PassingMode::Take]);
        let signature = f.signatures.get_mut(&path("callee")).unwrap();
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        function.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        }]
        .into();
        function.parameters[0].passing = PassingMode::Borrow;
        let mut selected = interner().get(function.parameters[0].ty).unwrap().clone();
        selected.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        function.result = interner().intern(selected);
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(1),
            mutable: false,
            if_inactive: false,
        });
        let view = f.through_loan(1, 0);
        let failure = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(1),
        ]);
        let argument = if qualified_actual { received } else { original };
        let Terminator::Call {
            callee: Callee::Definition { arguments, .. },
            arguments: values,
            returns: Some(CallReturn { destination, .. }),
            on_failure,
            ..
        } = &mut f.body().blocks[0].terminator
        else {
            panic!()
        };
        arguments[0] = GenericArgument::Type(argument);
        values[0].value = operand(view, UseKind::Borrow);
        *on_failure = CleanupId(failure);
        let destination = *destination;
        let local = f.body().places[destination.0 as usize].root;
        f.body().places[destination.0 as usize].ty = argument;
        f.body().locals[local.0 as usize].ty = argument;
        let cleanup = f.cleanup(vec![drop(1)]);
        f.body().blocks[1].instructions.push(inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        ));
        f.body().blocks[1].instructions.push(inst(
            InstructionKind::Drop {
                place: destination,
                if_live: false,
            },
            cleanup,
        ));
        f.body().blocks[1].terminator = ret(0, cleanup);
        if qualified_actual {
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "generic crossing transport"
            }));
        } else {
            f.valid();
        }
    }
}

#[test]
fn borrowed_qualified_capture_inputs_are_rechecked_when_invoked() {
    for route in 0..3 {
        for shared in [false, true] {
            let mut f = Fixture::unit();
            let unit = unit_ty(&mut f);
            let x = param(&mut f, string_kind());
            let original = f.body().places[x as usize].ty;
            let mut qualified = interner().get(original).unwrap().clone();
            qualified.sendability_requirement = SendabilityRequirement::Sendable;
            qualified.modes.sendability = Sendability::Sendable;
            let qualified = f.definition.types.push(qualified);
            f.body().places[x as usize].ty = qualified;
            f.body().locals[x as usize].ty = qualified;
            owner_sig(&mut f, &[PassingMode::Take]);
            f.emit(InstructionKind::BeginLoan {
                id: LoanId(0),
                place: PlaceId(x),
                mutable: false,
                if_inactive: false,
            });
            let view = f.through_loan(x, 0);
            let mut nested = empty_body(&mut f);
            let captured = add_capture(&mut nested, qualified);
            let mut sink = f.signatures.get(f.definition.path).unwrap().clone();
            sink.definition = id("sink");
            let SignatureKind::Function(function) = &mut sink.kind else {
                panic!()
            };
            function.parameters[0].passing = PassingMode::Borrow;
            f.signatures.insert(id("sink"), sink);
            nested.locals.push(Local {
                ty: unit,
                storage: Storage::Temporary,
                scope: ScopeId(0),
                span: Span::default(),
            });
            nested.places.push(Place {
                root: LocalId(2),
                projection: vec![].into(),
                ty: unit,
                binding: None,
                loan: None,
            });
            nested.blocks[0].terminator = Terminator::Call {
                callee: Callee::Definition {
                    path: path("sink"),
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: operand(captured, UseKind::Borrow),
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(2),
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            };
            nested.blocks.push(block(vec![], ret(0, 0)));
            f.definition.bodies.push(nested);
            let closure = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
            let old = f.body().places[closure as usize].ty;
            let mut row = interner().get(old).unwrap().clone();
            row.modes.relation = Relation::environment();
            let local = f.definition.types.push(row);
            f.body().places[closure as usize].ty = local;
            let closure_root = f.body().places[closure as usize].root;
            f.body().locals[closure_root.0 as usize].ty = local;
            let before = f.cleanup(vec![
                CleanupAction::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                drop(x),
            ]);
            f.assign(
                closure,
                Rvalue::Closure {
                    context: hirsel_types::CaptureContext::OwningScoped,
                    body: BodyId(1),
                    captures: vec![operand(view, UseKind::Borrow)],
                    destination: Destination::Inferred,
                },
                before,
            );
            let mut owners = vec![
                drop(closure),
                CleanupAction::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                drop(x),
            ];
            if shared {
                let alias = f.slot(string_kind(), Storage::Local);
                let fail = f.cleanup(owners.clone());
                f.assign(alias, Rvalue::Use(copy(x)), fail);
                owners.insert(0, drop(alias));
            }
            if route != 0 {
                f.emit(InstructionKind::BeginLoan {
                    id: LoanId(1),
                    place: PlaceId(closure),
                    mutable: false,
                    if_inactive: false,
                });
                owners.insert(
                    0,
                    CleanupAction::EndLoan {
                        loan: LoanId(1),
                        if_active: false,
                    },
                );
            }
            let exit = f.cleanup(owners.clone());
            let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
            let next = indirect_call(&mut f, 0, operand(closure, UseKind::Borrow), result);
            let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator else {
                panic!()
            };
            *on_failure = CleanupId(exit);
            f.body().blocks[next].terminator = ret(0, exit);
            if route != 0 {
                let callable = f.body().places[closure as usize].ty;
                let view = f.through_loan(closure, 1);
                let mut signature =
                    function_signature("relay", vec![], TypeKind::Tuple(vec![].into()));
                let parameter = callable;
                let mut row = interner().get(parameter).unwrap().clone();
                row.modes.relation = Relation::global();
                let parameter = interner().intern(row);
                let SignatureKind::Function(function) = &mut signature.kind else {
                    panic!()
                };
                function.parameters = vec![Parameter {
                    name: None,
                    label: None,
                    passing: PassingMode::Borrow,
                    ty: parameter,
                }]
                .into();
                let arguments = if route == 2 {
                    let t = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
                        TypeParamId(0),
                    ))));
                    function.parameters[0].ty = t;
                    function.generics = vec![GenericParameter::Type {
                        name: "T".into(),
                        bounds: vec![Bound::Copyable].into(),
                    }]
                    .into();
                    let mut result = interner().get(t).unwrap().clone();
                    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
                    function.result = interner().intern(result);
                    vec![GenericArgument::Type(callable)]
                } else {
                    vec![]
                };
                f.signatures.insert(id("relay"), signature);
                let destination = if route == 2 {
                    let copied = f.slot(
                        interner().get(callable).unwrap().kind.clone(),
                        Storage::Local,
                    );
                    f.body().places[copied as usize].ty = callable;
                    let root = f.body().places[copied as usize].root;
                    f.body().locals[root.0 as usize].ty = callable;
                    copied
                } else {
                    result
                };
                f.body().blocks[0].terminator = Terminator::Call {
                    callee: Callee::Definition {
                        path: path("relay"),
                        arguments,
                    },
                    arguments: vec![CallArgument {
                        parameter: ParamId(0),
                        value: operand(view, UseKind::Borrow),
                    }],
                    returns: Some(CallReturn {
                        destination: PlaceId(destination),
                        next: BlockId(next as u32),
                    }),
                    on_failure: CleanupId(exit),
                };
                if route == 2 {
                    owners.insert(1, drop(destination));
                    let final_cleanup = f.cleanup(owners);
                    let after =
                        indirect_call(&mut f, next, operand(destination, UseKind::Borrow), result);
                    let Terminator::Call { on_failure, .. } = &mut f.body().blocks[next].terminator
                    else {
                        panic!()
                    };
                    *on_failure = CleanupId(final_cleanup);
                    f.body().blocks[after].terminator = ret(0, final_cleanup);
                }
            }
            if shared {
                assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "borrowed capture crossing receipt"
                }));
            } else {
                f.valid();
            }
        }
    }
}

#[test]
fn retained_mutable_state_uses_the_callbacks_fixed_contract() {
    for unique_callback in [false, true] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let state = param(&mut f, string_kind());
        let mut row = interner()
            .get(f.definition.bodies[0].places[state as usize].ty)
            .unwrap()
            .clone();
        row.modes.uniqueness = Uniqueness::Unique;
        row.modes.sendability = Sendability::Sendable;
        let unique_state = f.definition.types.push(row);
        f.body().places[state as usize].ty = unique_state;
        f.body().locals[state as usize].ty = unique_state;
        let mut state_contract = ty(string_kind());
        if unique_callback {
            state_contract.modes.uniqueness = Uniqueness::Unique;
        }
        let state_contract = f.definition.types.push(state_contract);
        let callback = param(
            &mut f,
            TypeKind::Function(fn_type(
                vec![Parameter {
                    name: Some("state".into()),
                    label: None,
                    passing: PassingMode::Mut,
                    ty: state_contract,
                }],
                unit,
            )),
        );
        let mut callback_row = interner()
            .get(f.definition.bodies[0].places[callback as usize].ty)
            .unwrap()
            .clone();
        callback_row.modes.sendability = Sendability::Sendable;
        let callback_type = f.definition.types.push(callback_row);
        f.body().places[callback as usize].ty = callback_type;
        f.body().locals[callback as usize].ty = callback_type;
        owner_sig(&mut f, &[PassingMode::Take, PassingMode::Take]);

        let mut signature = function_signature("factory", vec![], TypeKind::Tuple(vec![].into()));
        let header_unit = interner().intern(ty(TypeKind::Tuple(vec![].into())));
        let state_parameter = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let nested_state = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        })));
        let next = interner().intern(ty(TypeKind::Function(fn_type(
            vec![Parameter {
                name: Some("state".into()),
                label: None,
                passing: PassingMode::Mut,
                ty: nested_state,
            }],
            header_unit,
        ))));
        let mut iterator = ty(TypeKind::Builtin {
            kind: BuiltinType::Iter,
            arguments: vec![GenericArgument::Type(header_unit)].into(),
        });
        iterator.erased_captures = Some(ErasedCaptures::FromArguments(
            vec![
                hirsel_types::RelationPlace {
                    parameter: ParameterRef::bound(ParamId(0)),
                    fields: vec![].into(),
                },
                hirsel_types::RelationPlace {
                    parameter: ParameterRef::bound(ParamId(1)),
                    fields: vec![].into(),
                },
            ]
            .into(),
        ));
        let iterator = interner().intern(iterator);
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        function.generics = vec![GenericParameter::Type {
            name: "S".into(),
            bounds: vec![].into(),
        }]
        .into();
        function.parameters = vec![
            Parameter {
                name: Some("state".into()),
                label: None,
                passing: PassingMode::Take,
                ty: state_parameter,
            },
            Parameter {
                name: Some("next".into()),
                label: None,
                passing: PassingMode::Take,
                ty: next,
            },
        ]
        .into();
        function.result = iterator;
        f.signatures.insert(id("factory"), signature);
        let result = f.slot(
            TypeKind::Builtin {
                kind: BuiltinType::Iter,
                arguments: vec![GenericArgument::Type(unit)].into(),
            },
            Storage::Temporary,
        );
        let mut result_row = interner()
            .get(f.definition.bodies[0].places[result as usize].ty)
            .unwrap()
            .clone();
        result_row.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::inert()));
        result_row.modes.sendability = if unique_callback {
            Sendability::Sendable
        } else {
            Sendability::NotSendable
        };
        let result_type = f.definition.types.push(result_row.clone());
        f.body().places[result as usize].ty = result_type;
        f.body().locals[result as usize].ty = result_type;
        let cleanup = f.cleanup(vec![drop(result)]);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("factory"),
                arguments: vec![GenericArgument::Type(state_contract)],
            },
            arguments: vec![
                CallArgument {
                    parameter: ParamId(0),
                    value: take(state),
                },
                CallArgument {
                    parameter: ParamId(1),
                    value: take(callback),
                },
            ],
            returns: Some(CallReturn {
                destination: PlaceId(result),
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        };
        f.body().blocks.push(block(vec![], ret(0, cleanup)));
        if !unique_callback {
            result_row.modes.sendability = Sendability::Sendable;
            let forged = f.definition.types.push(result_row);
            f.body().places[result as usize].ty = forged;
            f.body().locals[result as usize].ty = forged;
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "call result"
            }));
            f.body().places[result as usize].ty = result_type;
            f.body().locals[result as usize].ty = result_type;
        }
        f.valid();
    }
}

#[test]
fn copied_aggregate_captures_do_not_reuse_descendant_proofs() {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let mut bytes = ty(string_kind());
    bytes.modes.sendability = Sendability::Sendable;
    let bytes = f.definition.types.push(bytes);
    let x = param(&mut f, TypeKind::Tuple(vec![bytes, unit].into()));
    let original = f.body().places[x as usize].ty;
    let mut source = interner().get(original).unwrap().clone();
    source.sendability_requirement = SendabilityRequirement::Sendable;
    source.modes.sendability = Sendability::Sendable;
    let qualified = f.definition.types.push(source.clone());
    f.body().locals[x as usize].ty = qualified;
    f.body().places[x as usize].ty = qualified;
    source.sendability_requirement = SendabilityRequirement::None;
    source.modes.sendability = Sendability::NotSendable;
    let copied = f.definition.types.push(source);
    let mut nested = empty_body(&mut f);
    add_capture(&mut nested, copied);
    f.definition.bodies.push(nested);
    let closure = f.slot(
        TypeKind::Function(fn_type(vec![], unit)),
        Storage::Temporary,
    );
    let old = f.body().places[closure as usize].ty;
    let mut row = interner().get(old).unwrap().clone();
    row.modes.sendability = Sendability::Sendable;
    let forged = f.definition.types.push(row);
    f.body().locals[closure as usize].ty = forged;
    f.body().places[closure as usize].ty = forged;
    let cleanup = f.cleanup(vec![drop(x)]);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![copy(x)],
            destination: Destination::Inferred,
        },
        cleanup,
    );
    f.body().result = forged;
    f.body().blocks[0].terminator = ret(closure, cleanup);
    owner_sig(&mut f, &[PassingMode::Take]);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "closure sendability"
    }));
    replace_closure_row(&mut f, |row| {
        row.modes.sendability = Sendability::NotSendable
    });
    owner_sig(&mut f, &[PassingMode::Take]);
    f.valid();
}

#[test]
fn copied_callable_payload_paths_survive_successive_destructuring() {
    for depth in [1, 7, 19] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let mut callable = ty(TypeKind::Function(fn_type(vec![], unit)));
        callable.sendability_requirement = SendabilityRequirement::Sendable;
        callable.modes.sendability = Sendability::NotSendable;
        let callable = f.definition.types.push(callable);
        let mut rows = vec![callable];
        for ordinal in 0..depth {
            let inner = *rows.last().unwrap();
            if ordinal % 2 == 1 {
                rows.push(f.definition.types.push(ty(TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: vec![GenericArgument::Type(inner)].into(),
                })));
                continue;
            }
            rows.push(
                f.definition
                    .types
                    .push(ty(TypeKind::Tuple(vec![inner, unit].into()))),
            );
        }
        let outer = *rows.last().unwrap();
        let outer_kind = interner().get(outer).unwrap().kind.clone();
        let input = param(&mut f, outer_kind.clone());
        let mut qualified = interner().get(outer).unwrap().clone();
        qualified.sendability_requirement = SendabilityRequirement::Sendable;
        qualified.modes.sendability = Sendability::Sendable;
        let qualified = f.definition.types.push(qualified);
        f.body().places[input as usize].ty = qualified;
        let input_root = f.body().places[input as usize].root;
        f.body().locals[input_root.0 as usize].ty = qualified;
        owner_sig(&mut f, &[PassingMode::Take]);
        let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Local);
        f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
        let index_root = f.body().places[index as usize].root;
        let mut owner = f.slot(outer_kind, Storage::Temporary);
        let initial = f.cleanup(vec![drop(input)]);
        f.assign(owner, Rvalue::Use(copy(input)), initial);
        let mut held = vec![owner, input];
        for inner in rows[..depth].iter().rev() {
            let root = f.body().places[owner as usize].root;
            let owner_type = f.body().places[owner as usize].ty;
            let projection = if matches!(
                interner().get(owner_type).unwrap().kind,
                TypeKind::Builtin {
                    kind: BuiltinType::List,
                    ..
                }
            ) {
                Projection::Index(index_root)
            } else {
                Projection::Field(0)
            };
            let field = f.body().places.len() as u32;
            f.body().places.push(Place {
                loan: None,
                binding: None,
                root,
                projection: vec![projection].into(),
                ty: *inner,
            });
            let next = f.slot(
                interner().get(*inner).unwrap().kind.clone(),
                Storage::Temporary,
            );
            if *inner == callable {
                let mut row = interner().get(callable).unwrap().clone();
                row.modes.sendability = Sendability::Sendable;
                let proved = f.definition.types.push(row);
                f.body().places[next as usize].ty = proved;
                let root = f.body().places[next as usize].root;
                f.body().locals[root.0 as usize].ty = proved;
            }
            let fail = f.cleanup(held.iter().copied().map(drop).collect());
            f.assign(next, Rvalue::Use(copy(field)), fail);
            held.insert(0, next);
            owner = next;
        }
        let cleanup = f.cleanup(held.into_iter().map(drop).collect());
        f.body().blocks[0].terminator = ret(0, cleanup);
        f.valid();
    }
}

#[test]
fn dependent_unique_call_results_accept_stronger_current_content_summaries() {
    let mut f = Fixture::unit();
    let mut scalar = ty(TypeKind::Primitive(Primitive::U64));
    scalar.modes.sendability = Sendability::Sendable;
    let scalar = f.definition.types.push(scalar);
    let mut result = ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(scalar)].into(),
    });
    result.modes.uniqueness = Uniqueness::Unique;
    result.modes.relation = Relation::environment();
    let header_result = f.definition.types.push(result.clone());
    let callback = param(&mut f, TypeKind::Function(fn_type(vec![], header_result)));
    let original = f.body().places[callback as usize].ty;
    let mut header = interner().get(original).unwrap().clone();
    header.modes.sendability = Sendability::Sendable;
    let header = f.definition.types.push(header);
    f.body().places[callback as usize].ty = header;
    let root = f.body().places[callback as usize].root;
    f.body().locals[root.0 as usize].ty = header;
    owner_sig(&mut f, &[PassingMode::Borrow]);
    let destination = f.slot(result.kind.clone(), Storage::Temporary);
    result.modes.sendability = Sendability::Sendable;
    let current = f.definition.types.push(result);
    f.body().places[destination as usize].ty = current;
    let root = f.body().places[destination as usize].root;
    f.body().locals[root.0 as usize].ty = current;
    let cleanup = f.cleanup(vec![drop(destination)]);
    let next = indirect_call(&mut f, 0, operand(callback, UseKind::Borrow), destination);
    f.body().blocks[next].terminator = ret(0, cleanup);
    f.valid();
}

#[test]
fn copying_a_borrowed_qualified_owner_keeps_its_callable_field_proof() {
    let mut f = Fixture::unit();
    let unit = unit_ty(&mut f);
    let callable = f
        .definition
        .types
        .push(ty(TypeKind::Function(fn_type(vec![], unit))));
    let owner = param(&mut f, TypeKind::Tuple(vec![callable].into()));
    let root = f.body().places[owner as usize].root;
    let old = f.body().places[owner as usize].ty;
    let mut row = interner().get(old).unwrap().clone();
    row.sendability_requirement = SendabilityRequirement::Sendable;
    row.modes.sendability = Sendability::Sendable;
    let qualified = f.definition.types.push(row);
    f.body().places[owner as usize].ty = qualified;
    f.body().locals[root.0 as usize].ty = qualified;
    owner_sig(&mut f, &[PassingMode::Borrow]);
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root,
        projection: vec![Projection::Field(0)].into(),
        ty: callable,
    });
    let copied = f.slot(
        TypeKind::Function(fn_type(vec![], unit)),
        Storage::Temporary,
    );
    let mut row = interner().get(callable).unwrap().clone();
    row.modes.sendability = Sendability::Sendable;
    let proved = f.definition.types.push(row);
    f.body().places[copied as usize].ty = proved;
    let root = f.body().places[copied as usize].root;
    f.body().locals[root.0 as usize].ty = proved;
    f.assign(copied, Rvalue::Use(copy(field)), 0);
    let cleanup = f.cleanup(vec![drop(copied)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.valid();
}

#[test]
fn dependent_result_sources_normalize_after_the_actual_lifetime_is_installed() {
    let mut f = Fixture::unit();
    let scope = f.body().parameter_scope;
    let reference = TypeParameterRef::body(scope, TypeParamId(0));
    let mut parameter = ty(TypeKind::Parameter(reference));
    parameter.modes.sendability = Sendability::DependsOn(vec![reference].into());
    parameter.drop_allocation = DropAllocation::DependsOn(vec![reference].into());
    let actual = f.definition.types.push(parameter);
    let source = param(&mut f, TypeKind::Parameter(reference));
    f.body().locals[source as usize].ty = actual;
    f.body().places[source as usize].ty = actual;
    owner_sig(&mut f, &[PassingMode::Take]);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        panic!()
    };
    owner.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    });
    let mut result = ty(TypeKind::Tuple(vec![actual].into()));
    result.modes.sendability = Sendability::DependsOn(vec![reference].into());
    result.drop_allocation = DropAllocation::DependsOn(vec![reference].into());
    let result_ty = f.definition.types.push(result);
    let destination = f.slot(TypeKind::Tuple(vec![actual].into()), Storage::Temporary);
    f.body().locals[destination as usize].ty = result_ty;
    f.body().places[destination as usize].ty = result_ty;
    let mut signature = function_signature("callee", vec![], TypeKind::Tuple(vec![].into()));
    let mut formal = ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))));
    formal.modes.sendability =
        Sendability::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into());
    formal.drop_allocation =
        DropAllocation::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into());
    let formal = interner().intern(formal);
    let mut result = ty(TypeKind::Tuple(vec![formal].into()));
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    result.drop_allocation =
        DropAllocation::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into());
    let result = interner().intern(result);
    let SignatureKind::Function(callee) = &mut signature.kind else {
        panic!()
    };
    callee.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    });
    callee.parameters.push(Parameter {
        name: None,
        label: None,
        passing: PassingMode::Take,
        ty: formal,
    });
    callee.result = result;
    f.signatures.insert(id("callee"), signature);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![GenericArgument::Type(actual)],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: take(source),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    let cleanup = f.cleanup(vec![drop(destination)]);
    f.body().blocks.push(block(vec![], ret(0, cleanup)));
    f.valid();
    let mut forged = interner().get(result_ty).unwrap().clone();
    forged.modes.sendability = Sendability::Sendable;
    let forged = f.definition.types.push(forged);
    f.body().locals[destination as usize].ty = forged;
    f.body().places[destination as usize].ty = forged;
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "call result"
        }),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn qualified_content_keeps_a_related_taken_sources_frame_bound() {
    for fresh_copy in [false, true] {
        let mut f = Fixture::unit();
        let source = param(&mut f, TypeKind::Primitive(Primitive::String));
        let old = f.body().places[source as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
        row.sendability_requirement = SendabilityRequirement::Sendable;
        row.modes.sendability = Sendability::Sendable;
        row.modes.relation = Relation::environment();
        let source_ty = f.definition.types.push(row);
        f.body().locals[source as usize].ty = source_ty;
        f.body().places[source as usize].ty = source_ty;
        owner_sig(&mut f, &[PassingMode::Take]);
        let borrowed_source = if fresh_copy {
            let fresh = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
            let old = f.body().places[fresh as usize].ty;
            let mut row = interner().get(old).unwrap().clone();
            row.modes.uniqueness = Uniqueness::Unique;
            row.modes.sendability = Sendability::Sendable;
            let unique = f.definition.types.push(row);
            f.body().locals[fresh as usize].ty = unique;
            f.body().places[fresh as usize].ty = unique;
            let failure = f.cleanup(vec![drop(source)]);
            f.assign(
                fresh,
                Rvalue::Unary {
                    op: hirsel_syntax::UnaryOp::Unique,
                    value: copy(source),
                },
                failure,
            );
            fresh
        } else {
            source
        };
        let destination = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
        let global = f.body().places[destination as usize].ty;
        let mut row = interner().get(global).unwrap().clone();
        row.modes.relation = Relation::environment();
        let local = f.definition.types.push(row);
        f.body().locals[destination as usize].ty = local;
        f.body().places[destination as usize].ty = local;
        let mut signature = function_signature(
            "callee",
            vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::String))],
            TypeKind::Primitive(Primitive::Bytes),
        );
        let SignatureKind::Function(callee) = &mut signature.kind else {
            panic!()
        };
        let mut row = interner().get(callee.parameters[0].ty).unwrap().clone();
        row.modes.relation = Relation::environment();
        callee.parameters[0].ty = interner().intern(row);
        let mut row = interner().get(callee.result).unwrap().clone();
        row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        callee.result = interner().intern(row);
        f.signatures.insert(id("callee"), signature);
        let borrowed = f.through_loan(borrowed_source, 0);
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(borrowed_source),
            mutable: false,
            if_inactive: false,
        });
        let end = CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        };
        let mut failure_actions = vec![end.clone()];
        let mut success_actions = vec![drop(destination), end];
        if fresh_copy {
            failure_actions.push(drop(borrowed_source));
            success_actions.push(drop(borrowed_source));
        }
        failure_actions.push(drop(source));
        success_actions.push(drop(source));
        let failure = f.cleanup(failure_actions);
        let success = f.cleanup(success_actions);
        f.body().blocks[0].terminator = Terminator::Call {
            callee: Callee::Definition {
                path: path("callee"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(borrowed, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(destination),
                next: BlockId(1),
            }),
            on_failure: CleanupId(failure),
        };
        f.body().blocks.push(block(vec![], ret(0, success)));
        if !fresh_copy {
            // Global content is not a certificate that its related access survives
            // the source frame. The same call cannot publish a Global share.
            f.body().locals[destination as usize].ty = global;
            f.body().places[destination as usize].ty = global;
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "call result"
                }),
                "{:?}",
                f.kinds()
            );
            f.body().locals[destination as usize].ty = local;
            f.body().places[destination as usize].ty = local;
        } else {
            f.body().locals[destination as usize].ty = global;
            f.body().places[destination as usize].ty = global;
        }
        f.valid();
    }
}

#[test]
fn an_unqualified_generic_move_cannot_invent_conditional_crossing_evidence() {
    for qualified in [false, true] {
        let mut f = Fixture::unit();
        let reference = TypeParameterRef::body(f.body().parameter_scope, TypeParamId(0));
        let source = param(&mut f, TypeKind::Parameter(reference));
        owner_sig(&mut f, &[PassingMode::Take]);
        let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            panic!()
        };
        owner.generics.push(GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::NoallocDrop].into(),
        });
        let temporary = f.slot(TypeKind::Parameter(reference), Storage::Temporary);
        let temporary_ty = f.body().places[temporary as usize].ty;
        let mut row = interner().get(temporary_ty).unwrap().clone();
        row.modes.sendability = Sendability::DependsOn(vec![reference].into());
        let receiving = f.definition.types.push(row);
        f.body().locals[temporary as usize].ty = receiving;
        f.body().places[temporary as usize].ty = receiving;
        f.assign(temporary, Rvalue::Use(take(source)), 0);
        let output = f.slot(TypeKind::Parameter(reference), Storage::Temporary);
        if qualified {
            let mut row = interner().get(receiving).unwrap().clone();
            row.sendability_requirement = SendabilityRequirement::Sendable;
            row.modes.sendability = Sendability::Sendable;
            let receiving = f.definition.types.push(row);
            f.body().locals[output as usize].ty = receiving;
            f.body().places[output as usize].ty = receiving;
        }
        f.assign(output, Rvalue::Use(take(temporary)), 0);
        let cleanup = f.cleanup(vec![drop(output)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        if qualified {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "sendable receipt"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            f.valid();
        }
    }
}

#[test]
fn ordinary_generic_moves_do_not_supply_borrowed_capture_obligations() {
    for proven in [false, true] {
        let mut f = Fixture::unit();
        let reference = TypeParameterRef::body(f.body().parameter_scope, TypeParamId(0));
        let source = param(&mut f, TypeKind::Parameter(reference));
        if proven {
            let mut row = ty(TypeKind::Parameter(reference));
            row.modes.sendability = Sendability::DependsOn(vec![reference].into());
            let received = f.definition.types.push(row);
            f.body().locals[source as usize].ty = received;
            f.body().places[source as usize].ty = received;
        }
        owner_sig(&mut f, &[PassingMode::Take]);
        let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            panic!()
        };
        owner.generics.push(GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::NoallocDrop, Bound::Inert].into(),
        });
        let temporary = f.slot(TypeKind::Parameter(reference), Storage::Local);
        let mut row = ty(TypeKind::Parameter(reference));
        row.modes.sendability = Sendability::DependsOn(vec![reference].into());
        let receiving = f.definition.types.push(row.clone());
        f.body().locals[temporary as usize].ty = receiving;
        f.body().places[temporary as usize].ty = receiving;
        f.assign(temporary, Rvalue::Use(take(source)), 0);
        row.modes.relation = Relation::environment();
        let capture = f.definition.types.push(row);
        let mut nested = empty_body(&mut f);
        add_capture(&mut nested, capture);
        f.definition.bodies.push(nested);
        let unit = unit_ty(&mut f);
        let closure = f.slot(TypeKind::Function(fn_type(vec![], unit)), Storage::Local);
        let mut row = ty(TypeKind::Function(fn_type(vec![], unit)));
        row.modes.relation = Relation::environment();
        let local = f.definition.types.push(row);
        f.body().locals[closure as usize].ty = local;
        f.body().places[closure as usize].ty = local;
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(temporary),
            mutable: false,
            if_inactive: false,
        });
        let borrowed = f.through_loan(temporary, 0);
        let failure = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(temporary),
        ]);
        f.assign(
            closure,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![operand(borrowed, UseKind::Borrow)],
                destination: Destination::Inferred,
            },
            failure,
        );
        let cleanup = f.cleanup(vec![
            drop(closure),
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            drop(temporary),
        ]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        if proven {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "value modes"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn scoped_seal_global_capture_has_an_independent_ir_control() {
    for (scoped, erased_sibling) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut f = Fixture::unit();
        let unit = unit_ty(&mut f);
        let mut gate = capability_signature("Gate");
        let SignatureKind::Type(d) = &mut gate.kind else {
            unreachable!()
        };
        if scoped {
            d.capability = Some(CapabilityKind::Scoped);
            d.bounds = vec![Bound::NoallocDrop];
        }
        f.signatures.insert(id("Gate"), gate);
        let gate = f.definition.types.push(ty(TypeKind::Named {
            definition: id("Gate"),
            arguments: vec![].into(),
        }));
        let kind = if erased_sibling {
            let mut callback = fn_type(vec![], unit);
            callback.authority = AuthoritySummary::Unknown;
            callback.uses = Uses::Unrestricted;
            let callback = f.definition.types.push(ty(TypeKind::Function(callback)));
            TypeKind::Tuple(vec![gate, callback].into())
        } else {
            interner().get(gate).unwrap().kind.clone()
        };
        let source = param(&mut f, kind);
        let source_ty = f.body().places[source as usize].ty;
        let mut nested = empty_body(&mut f);
        let capture = add_capture(&mut nested, source_ty);
        nested.add_cleanup(vec![drop(capture)]);
        nested.blocks[0].terminator = ret(0, 1);
        f.definition.bodies.push(nested);
        let mut fun = fn_type(vec![], unit);
        fun.authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Gate"))].into());
        if erased_sibling {
            fun.authority = AuthoritySummary::Unknown;
            fun.uses = Uses::Unrestricted;
        } else {
            fun.uses = Uses::Only(fun.authority.clone());
        }
        let mut row = ty(TypeKind::Function(fun));
        row.modes.multiplicity = Multiplicity::Once;
        let closure = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let closure_ty = f.definition.types.push(row);
        f.body().locals[closure as usize].ty = closure_ty;
        f.body().places[closure as usize].ty = closure_ty;
        let failure = if erased_sibling {
            f.cleanup(vec![drop(source)])
        } else {
            0
        };
        f.assign(
            closure,
            Rvalue::Closure {
                context: hirsel_types::CaptureContext::OwningScoped,
                body: BodyId(1),
                captures: vec![take(source)],
                destination: Destination::Inferred,
            },
            failure,
        );
        let cleanup = f.cleanup(vec![drop(closure)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        owner_sig(&mut f, &[PassingMode::Take]);
        if scoped {
            f.broken(vec![ViolationKind::TypeMismatch {
                entity: "scoped capability capture",
            }]);
        } else {
            f.valid();
        }
    }
}

#[test]
fn serializable_calls_reprove_nominal_opt_in_from_hand_built_ir() {
    // Deleting only the Serializable arm of Checker::bound accepts the bad
    // call. Both nominal declarations are independently valid unmarked data.
    for marked in [false, true] {
        let mut f = generic_call_fixture(
            Bound::Serializable,
            TypeKind::Named {
                definition: id("Data"),
                arguments: vec![].into(),
            },
        );
        let types = interner();
        let repr = types.push(ty(TypeKind::Primitive(Primitive::U64)));
        f.signatures.insert(
            id("Data"),
            Signature {
                definition: id("Data"),
                metadata: SignatureMetadata::default(),
                fingerprint: ContentHash::default(),
                kind: SignatureKind::Type(Box::new(TypeDefinition {
                    loop_source: None,
                    format: None,
                    generics: vec![],
                    representation: Some(repr),
                    bounds: vec![],
                    authority: AuthoritySummary::inert(),
                    capability: None,
                    implementation_authority: None,
                    serializable: marked,
                    serializable_check: None,
                    layout: Layout::default(),
                    distinct: false,
                    resource: false,
                    drop: None,
                    fact: None,
                    captures: None,
                    destructor_allocation: Allocation::MayAllocate,
                })),
            },
        );
        if marked {
            f.valid()
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "generic bound"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
    generic_call_fixture(Bound::Serializable, string_kind()).valid();
}

#[test]
fn integer_to_float_scalar_domains_are_replayed_on_hand_built_calls() {
    use hirsel_types::ScalarDomain;
    for (domain, valid_kind, invalid_kind) in [
        (ScalarDomain::Integer, Primitive::U64, Primitive::F64),
        (ScalarDomain::Float, Primitive::F32, Primitive::U32),
        (ScalarDomain::Scalar, Primitive::Bool, Primitive::String),
    ] {
        for (kind, valid) in [(valid_kind, true), (invalid_kind, false)] {
            let mut fixture = generic_call_fixture(Bound::Plain, TypeKind::Primitive(kind));
            let signature = fixture.signatures.get_mut(&path("callee")).unwrap();
            let SignatureKind::Function(function) = &mut signature.kind else {
                unreachable!()
            };
            let GenericParameter::Type { bounds, .. } = &mut function.generics[0] else {
                unreachable!()
            };
            bounds.clear();
            signature.metadata.schema = vec![SchemaConstraint::Domain {
                parameter: TypeParameterRef::bound(TypeParamId(0)),
                domain,
                rule: const { hirsel_diag::RuleId::of("SEM-1") },
            }];
            if valid {
                fixture.valid();
            } else {
                assert!(
                    fixture.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "intrinsic bound"
                    }),
                    "{domain:?} {kind:?}: {:?}",
                    fixture.kinds()
                );
                // The same hand-built call passes when its domain rule is absent.
                fixture
                    .signatures
                    .get_mut(&path("callee"))
                    .unwrap()
                    .metadata
                    .schema
                    .clear();
                fixture.valid();
            }
        }
    }
}

#[test]
fn declared_conversion_ranges_and_positive_sizes_have_ir_controls() {
    let source = TypeParameterRef::bound(TypeParamId(0));
    let destination = TypeParameterRef::bound(TypeParamId(1));
    for (constraint, valid_kind, invalid_kind) in [
        (
            SchemaConstraint::CompleteRangeFits {
                source,
                destination,
            },
            Primitive::U128,
            Primitive::I64,
        ),
        (
            SchemaConstraint::DestinationNoWider {
                source,
                destination,
            },
            Primitive::U32,
            Primitive::U128,
        ),
    ] {
        for (kind, valid) in [(valid_kind, true), (invalid_kind, false)] {
            let mut fixture =
                generic_call_fixture(Bound::Plain, TypeKind::Primitive(Primitive::U64));
            let actual = fixture.definition.types.push(ty(TypeKind::Primitive(kind)));
            let signature = fixture.signatures.get_mut(&path("callee")).unwrap();
            let SignatureKind::Function(function) = &mut signature.kind else {
                unreachable!()
            };
            function.generics.push(GenericParameter::Type {
                name: "U".into(),
                bounds: vec![Bound::Plain].into(),
            });
            signature.metadata.schema = vec![constraint.clone()];
            let Terminator::Call {
                callee: Callee::Definition { arguments, .. },
                ..
            } = &mut fixture.body().blocks[0].terminator
            else {
                unreachable!()
            };
            arguments.push(GenericArgument::Type(actual));
            if valid {
                fixture.valid();
            } else {
                assert!(
                    fixture.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "intrinsic bound"
                    }),
                    "{constraint:?}: {:?}",
                    fixture.kinds()
                );
                fixture
                    .signatures
                    .get_mut(&path("callee"))
                    .unwrap()
                    .metadata
                    .schema
                    .clear();
                fixture.valid();
            }
        }
    }
    for (size, valid) in [(1, true), (0, false)] {
        let mut fixture = generic_call_fixture(Bound::Plain, TypeKind::Primitive(Primitive::U64));
        let signature = fixture.signatures.get_mut(&path("callee")).unwrap();
        let SignatureKind::Function(function) = &mut signature.kind else {
            unreachable!()
        };
        function
            .generics
            .push(GenericParameter::Size { name: "n".into() });
        signature.metadata.schema = vec![SchemaConstraint::PositiveSize(SizeParameterRef::bound(
            SizeParamId(1),
        ))];
        let Terminator::Call {
            callee: Callee::Definition { arguments, .. },
            ..
        } = &mut fixture.body().blocks[0].terminator
        else {
            unreachable!()
        };
        arguments.push(GenericArgument::Size(Size::Constant(size)));
        if valid {
            fixture.valid();
        } else {
            assert!(fixture.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "intrinsic bound"
            }));
            fixture
                .signatures
                .get_mut(&path("callee"))
                .unwrap()
                .metadata
                .schema
                .clear();
            fixture.valid();
        }
    }
}

#[test]
fn constant_size_actuals_are_complete_and_lexically_declared() {
    for variant in 0..3 {
        let mut f = Fixture::unit();
        let mut signature = function_signature("DATA", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(function) = &signature.kind else {
            unreachable!()
        };
        signature.kind = SignatureKind::Const {
            ty: function.result,
            generics: vec![GenericParameter::Size { name: "n".into() }].into(),
        };
        f.signatures.insert(id("DATA"), signature);
        f.body().rvalues[0] = Rvalue::ReadConstant {
            definition: path("DATA"),
            arguments: match variant {
                0 => vec![Size::Constant(2)],
                1 => vec![],
                _ => vec![Size::Parameter(SizeParameterRef::body(
                    scoped(NameSite::Item),
                    SizeParamId(0),
                ))],
            },
        };
        match variant {
            0 => f.valid(),
            1 => assert_eq!(
                f.kinds(),
                vec![ViolationKind::Arity {
                    entity: "constant size arguments",
                    expected: 1,
                    actual: 0
                }]
            ),
            _ => assert_eq!(
                f.kinds(),
                vec![ViolationKind::InvalidId {
                    arena: "size parameter",
                    index: 0
                }]
            ),
        }
    }
}

#[test]
fn constant_size_actuals_instantiate_the_declared_result() {
    for actual in [2, 3] {
        let mut f = Fixture::unit();
        let element = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U8)));
        let result = f.definition.types.push(ty(TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(element),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        }));
        f.body().locals[0].ty = result;
        f.body().places[0].ty = result;
        f.body().result = result;
        let mut signature = function_signature("DATA", vec![], TypeKind::Tuple(vec![].into()));
        let element = interner().intern(ty(TypeKind::Primitive(Primitive::U8)));
        let root = interner().intern(ty(TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(element),
                GenericArgument::Size(Size::Parameter(SizeParameterRef::bound(SizeParamId(0)))),
            ]
            .into(),
        }));
        signature.kind = SignatureKind::Const {
            ty: root,
            generics: vec![GenericParameter::Size { name: "n".into() }].into(),
        };
        f.signatures.insert(id("DATA"), signature);
        f.body().rvalues[0] = Rvalue::ReadConstant {
            definition: path("DATA"),
            arguments: vec![Size::Constant(actual)],
        };
        if actual == 2 {
            f.valid();
        } else {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "constant declaration"
                }),
                "{:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn constant_declarations_exclude_type_parameters() {
    for size in [true, false] {
        let mut f = Fixture::unit();
        let mut signature = function_signature("DATA", vec![], TypeKind::Tuple(vec![].into()));
        let SignatureKind::Function(function) = &signature.kind else {
            unreachable!()
        };
        signature.kind = SignatureKind::Const {
            ty: function.result,
            generics: vec![if size {
                GenericParameter::Size { name: "n".into() }
            } else {
                GenericParameter::Type {
                    name: "T".into(),
                    bounds: vec![].into(),
                }
            }]
            .into(),
        };
        f.signatures.insert(id("DATA"), signature);
        f.body().rvalues[0] = Rvalue::ReadConstant {
            definition: path("DATA"),
            arguments: vec![Size::Constant(2)],
        };
        if size {
            f.valid();
        } else {
            assert!(
                f.kinds()
                    .contains(&ViolationKind::InvalidSignature(path("DATA"))),
                "{:?}",
                f.kinds()
            );
        }
    }
}
