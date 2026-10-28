//! Forged controls for V rules that no conformance file can reach: J rejects
//! every source form first, so each control here is its rule's only guard.
//! Each rejected program has an accepted twin one edit away.
use super::*;

fn u32_kind() -> TypeKind {
    TypeKind::Primitive(Primitive::U32)
}

fn integer(f: &mut Fixture, value: u128) -> u32 {
    let p = f.slot(u32_kind(), Storage::Temporary);
    f.assign(p, Rvalue::Constant(Constant::Integer(value)), 0);
    p
}

fn projected(f: &mut Fixture, of: u32, projection: Vec<Projection>, ty: TypeId) -> u32 {
    let root = f.body().places[of as usize].root;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root,
        projection: projection.into(),
        ty,
    });
    f.body().places.len() as u32 - 1
}

fn inline(f: &mut Fixture, element: TypeKind, length: u64) -> TypeKind {
    let element = f.definition.types.push(ty(element));
    TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![
            GenericArgument::Type(element),
            GenericArgument::Size(Size::Constant(length)),
        ]
        .into(),
    }
}

/// `callee(first, second)` for `fn callee(mut u32, <second's mode> u32)`.
/// A mutable loan is begun on each of `loaned` before the call and ended on
/// both of its exits; the taken parameters `owned` are dropped at each exit.
fn pair_call(f: &mut Fixture, loaned: &[u32], first: u32, second: (u32, UseKind), owned: &[u32]) {
    for (i, place) in loaned.iter().enumerate() {
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(i as u32),
            place: PlaceId(*place),
            mutable: true,
            if_inactive: false,
        });
    }
    let ends = (0..loaned.len() as u32)
        .rev()
        .map(|loan| CleanupAction::EndLoan {
            loan: LoanId(loan),
            if_active: false,
        });
    let failure = f.cleanup(ends.chain(owned.iter().map(|p| drop(*p))).collect());
    let exit = f.cleanup(owned.iter().map(|p| drop(*p)).collect());
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(0),
                value: operand(first, UseKind::Mut),
            },
            CallArgument {
                parameter: ParamId(1),
                value: operand(second.0, second.1),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure),
    };
    let ends = (0..loaned.len() as u32)
        .rev()
        .map(|loan| {
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(loan),
                    if_active: false,
                },
                0,
            )
        })
        .collect();
    f.body().blocks.push(block(ends, ret(0, exit)));
    let second_mode = match second.1 {
        UseKind::Mut => PassingMode::Mut,
        UseKind::Borrow => PassingMode::Borrow,
        _ => PassingMode::Take,
    };
    let signature = function_signature(
        "callee",
        vec![(PassingMode::Mut, u32_kind()), (second_mode, u32_kind())],
        TypeKind::Tuple(vec![].into()),
    );
    f.signatures.insert(signature.definition.clone(), signature);
    owner_signature(f, PassingMode::Take);
}

fn aliased() -> ViolationKind {
    ViolationKind::AliasedArguments {
        first: 0,
        second: 1,
    }
}

/// OWN-3: one storage passed as `mut` and again as `mut` or borrowed gives
/// the callee two views of one block. Both arguments go through the same
/// loan, so no second BeginLoan raises a LoanConflict: the call is the only check.
#[test]
fn one_storage_cannot_be_passed_twice_when_either_use_is_mut() {
    for second in [UseKind::Mut, UseKind::Borrow] {
        let mut f = Fixture::unit();
        let x = param(&mut f, u32_kind());
        let via = f.through_loan(x, 0);
        pair_call(&mut f, &[x], via, (via, second), &[x]);
        f.broken(vec![aliased()]);
    }
}

/// Disjoint fields of one tuple are separate storage, so both may be `mut`;
/// the same field twice is aliased.
#[test]
fn disjoint_fields_of_one_storage_may_both_be_mut() {
    let build = |same: bool| {
        let mut f = Fixture::unit();
        let element = f.definition.types.push(ty(u32_kind()));
        let x = param(&mut f, TypeKind::Tuple(vec![element, element].into()));
        let first = projected(&mut f, x, vec![Projection::Field(0)], element);
        let second = projected(&mut f, x, vec![Projection::Field(1)], element);
        let first_via = f.through_loan(first, 0);
        let second_via = f.through_loan(second, 1);
        let passed = if same { first_via } else { second_via };
        pair_call(
            &mut f,
            &[first, second],
            first_via,
            (passed, UseKind::Mut),
            &[x],
        );
        f
    };
    build(false).valid();
    build(true).broken(vec![aliased()]);
}

/// All elements of a sequence are one storage (OWN-3): `xs[i]` and
/// `xs[j]` alias whatever the indices hold, while elements of two sequences do not.
#[test]
fn two_index_projections_of_one_sequence_alias() {
    let build = |same: bool| {
        let mut f = Fixture::unit();
        let kind = inline(&mut f, u32_kind(), 2);
        let element = f.definition.types.push(ty(u32_kind()));
        let xs = param(&mut f, kind.clone());
        let ys = param(&mut f, kind);
        let i = integer(&mut f, 0);
        let j = integer(&mut f, 1);
        let i_root = f.body().places[i as usize].root;
        let j_root = f.body().places[j as usize].root;
        let first = projected(&mut f, xs, vec![Projection::Index(i_root)], element);
        let other = if same { xs } else { ys };
        let second = projected(&mut f, other, vec![Projection::Index(j_root)], element);
        let first = f.through_loan(first, 0);
        let second = f.through_loan(second, if same { 0 } else { 1 });
        let loaned: &[u32] = if same { &[xs] } else { &[xs, ys] };
        pair_call(&mut f, loaned, first, (second, UseKind::Mut), &[ys, xs]);
        f
    };
    build(false).valid();
    build(true).broken(vec![aliased()]);
}

/// Cleanup that drops one element by index, then the whole sequence, would
/// free that element twice; V has no proof for element-wise release.
#[test]
fn cleanup_cannot_drop_a_sequence_element_by_index() {
    let build = |element_first: bool| {
        let mut f = Fixture::unit();
        let string = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::String)));
        let kind = inline(&mut f, TypeKind::Primitive(Primitive::String), 2);
        let xs = param(&mut f, kind);
        let i = integer(&mut f, 0);
        let i_root = f.body().places[i as usize].root;
        let element = projected(&mut f, xs, vec![Projection::Index(i_root)], string);
        let actions = if element_first {
            vec![drop(element), drop(xs)]
        } else {
            vec![drop(xs)]
        };
        let exit = f.cleanup(actions);
        f.body().blocks[0].terminator = ret(0, exit);
        owner_signature(&mut f, PassingMode::Take);
        (f, exit)
    };
    build(false).0.valid();
    let (f, exit) = build(true);
    f.broken(vec![ViolationKind::UnsupportedCleanup(CleanupId(exit))]);
}

/// Region release, arena close and task joins have no ownership proof in
/// frozen IR, so a cleanup naming one fails closed.
#[test]
fn cleanup_without_an_ownership_proof_fails_closed() {
    let actions: Vec<fn(&mut Fixture) -> CleanupAction> = vec![
        |_| CleanupAction::ReleaseRegion(RegionId(0)),
        |f| CleanupAction::CloseArena(PlaceId(arena_slot(f, Storage::Local))),
        |f| CleanupAction::JoinTasks {
            scope: PlaceId(arena_slot(f, Storage::Local)),
            cancel: false,
        },
    ];
    Fixture::string().valid();
    for action in actions {
        let mut f = Fixture::string();
        let action = action(&mut f);
        let exit = f.cleanup(vec![action, drop(1)]);
        f.body().blocks[0].terminator = ret(0, exit);
        f.broken(vec![ViolationKind::UnsupportedCleanup(CleanupId(exit))]);
    }
}

/// `Unreachable` is legal only as the default of a switch whose arms cover
/// every case; a branch edge or an integer default can reach it.
#[test]
fn a_reachable_unreachable_terminator_is_rejected() {
    let branch = |sentinel: bool| {
        let mut f = Fixture::unit();
        let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
        f.body().blocks[0].terminator = Terminator::Branch {
            condition: copy(c),
            yes: BlockId(1),
            no: BlockId(2),
        };
        f.body().blocks.push(block(vec![], ret(0, 0)));
        let no = if sentinel {
            Terminator::Unreachable
        } else {
            ret(0, 0)
        };
        f.body().blocks.push(block(vec![], no));
        f
    };
    branch(false).valid();
    branch(true).broken(vec![ViolationKind::ReachableUnreachable]);
    // Arms 0 and 1 cover a bool, never a u8.
    let switch = |kind: TypeKind, value: Constant| {
        let mut f = Fixture::unit();
        let p = f.slot(kind, Storage::Temporary);
        f.assign(p, Rvalue::Constant(value), 0);
        f.body().blocks[0].terminator = Terminator::Switch {
            value: copy(p),
            arms: vec![(0, BlockId(1)), (1, BlockId(1))],
            otherwise: BlockId(2),
        };
        f.body().blocks.push(block(vec![], ret(0, 0)));
        f.body().blocks.push(block(vec![], Terminator::Unreachable));
        f
    };
    switch(TypeKind::Primitive(Primitive::Bool), Constant::Bool(true)).valid();
    switch(TypeKind::Primitive(Primitive::U8), Constant::Integer(1))
        .broken(vec![ViolationKind::ReachableUnreachable]);
}

/// A switch selects on a bool, an integer or a variant tag, never a String.
#[test]
fn a_switch_scrutinee_is_a_tag_bool_or_integer() {
    let switch = |string: bool| {
        let mut f = Fixture::string();
        let p = if string { 1 } else { integer(&mut f, 0) };
        let exit = f.body().cleanups.len() as u32 - 1;
        f.body().blocks[0].terminator = Terminator::Switch {
            value: operand(
                p,
                if string {
                    UseKind::Borrow
                } else {
                    UseKind::Copy
                },
            ),
            arms: vec![(0, BlockId(1))],
            otherwise: BlockId(1),
        };
        f.body().blocks.push(block(vec![], ret(0, exit)));
        f
    };
    switch(false).valid();
    switch(true).broken(vec![ViolationKind::TypeMismatch {
        entity: "switch scrutinee",
    }]);
}

/// A body reached from no entry, contract or closure producer would skip
/// flow checking entirely.
#[test]
fn every_body_is_reached_from_the_entry_a_contract_or_a_closure() {
    let mut f = Fixture::unit();
    f.valid();
    let orphan = empty_body(&mut f);
    f.definition.bodies.push(orphan);
    f.broken(vec![ViolationKind::UnreferencedBody(BodyId(1))]);
}

/// DROP-3: a resource finalizer is `fn(self) -> ()` where `self` is the
/// owner instantiated at the body's own generic parameters, in order.
fn finalizer(order: &[u32]) -> Fixture {
    let mut f = Fixture::unit();
    let arguments = order
        .iter()
        .map(|i| {
            GenericArgument::Type(f.definition.types.push(ty(TypeKind::Parameter(
                TypeParameterRef::bound(TypeParamId(*i)),
            ))))
        })
        .collect::<Vec<_>>();
    let self_place = f.slot(
        TypeKind::Named {
            definition: id("main"),
            arguments: arguments.into(),
        },
        Storage::Parameter,
    );
    let root = f.body().places[self_place as usize].root;
    f.body().parameters.push(root);
    let mut signature = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(fun) = &signature.kind else {
        unreachable!()
    };
    let unit = fun.result;
    signature.kind = SignatureKind::Type(Box::new(TypeDefinition {
        loop_source: None,
        format: None,
        destructor_allocation: Allocation::NoAlloc,
        generics: (0..order.len())
            .map(|i| GenericParameter::Type {
                name: format!("T{i}"),
                bounds: vec![].into(),
            })
            .collect(),
        representation: Some(unit),
        bounds: vec![],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: true,
        drop: Some(path("main")),
        fact: None,
        captures: None,
    }));
    f.signatures.insert(id("main"), signature);
    f
}

#[test]
fn a_finalizer_takes_exactly_its_owner_at_its_own_generics_and_returns_unit() {
    finalizer(&[]).valid();
    finalizer(&[0, 1]).valid();
    // Swapped arguments read the fields with the other instantiation's layout.
    finalizer(&[1, 0]).broken(vec![ViolationKind::TypeMismatch {
        entity: "finalizer self",
    }]);
    let mut f = finalizer(&[]);
    let self_place = f.body().parameters[0];
    let other = f.definition.types.push(ty(u32_kind()));
    f.body().locals[self_place.0 as usize].ty = other;
    f.body().places[self_place.0 as usize].ty = other;
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "finalizer self",
    }]);
    let mut f = finalizer(&[]);
    param(&mut f, u32_kind());
    f.broken(vec![ViolationKind::Arity {
        entity: "finalizer parameters",
        expected: 1,
        actual: 2,
    }]);
    let mut f = finalizer(&[]);
    let byte = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let body = f.body();
    body.result = byte;
    body.locals[0].ty = byte;
    body.places[0].ty = byte;
    body.rvalues[0] = Rvalue::Constant(Constant::Integer(0));
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "finalizer result",
    }]);
}

/// `match p { (false, _) => .., _ => .. }` over a borrowed `(bool, bool)`.
fn match_fixture() -> Fixture {
    let mut f = Fixture::unit();
    let boolean = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bool)));
    let p = param(&mut f, TypeKind::Tuple(vec![boolean, boolean].into()));
    let left = projected(&mut f, p, vec![Projection::Field(0)], boolean);
    let right = projected(&mut f, p, vec![Projection::Field(1)], boolean);
    let pattern = |place, kind| MatchPattern {
        place: PlaceId(place),
        kind,
    };
    f.body().matches.push(MatchTable {
        scrutinee: PlaceId(p),
        patterns: vec![
            pattern(left, MatchPatternKind::Literal(Constant::Bool(false))),
            pattern(right, MatchPatternKind::Wildcard),
            pattern(
                p,
                MatchPatternKind::Tuple(vec![(0, MatchPatternId(0)), (1, MatchPatternId(1))]),
            ),
            pattern(p, MatchPatternKind::Wildcard),
        ],
        arms: [2, 3]
            .into_iter()
            .map(|pattern| MatchArm {
                pattern: MatchPatternId(pattern),
                guard: None,
                target: BlockId(1),
            })
            .collect(),
    });
    f.body().blocks[0].terminator = Terminator::Match {
        table: MatchTableId(0),
        from: 0,
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

/// Each untested `validate_structure` arm, one mutation of a valid match
/// fixture apiece. The interpreter's only entry check is this one, so an
/// arm lost here admits a cyclic pattern arena or an ambiguous switch.
#[test]
fn structure_rejects_each_malformed_arena() {
    type Mutation = fn(&mut Fixture) -> ViolationKind;
    let cases: Vec<(&str, Mutation)> = vec![
        ("duplicate switch arm", |f| {
            let body = f.body();
            body.matches.clear();
            body.blocks[0].terminator = Terminator::Switch {
                value: copy(2),
                arms: vec![(0, BlockId(1)), (0, BlockId(1))],
                otherwise: BlockId(1),
            };
            ViolationKind::Duplicate {
                entity: "switch arm",
                index: 0,
            }
        }),
        ("duplicate pattern field", |f| {
            f.body().matches[0].patterns[2].kind =
                MatchPatternKind::Tuple(vec![(0, MatchPatternId(0)), (0, MatchPatternId(1))]);
            ViolationKind::Duplicate {
                entity: "pattern field",
                index: 0,
            }
        }),
        ("pattern child at or after its parent", |f| {
            f.body().matches[0].patterns[0].kind =
                MatchPatternKind::Tuple(vec![(0, MatchPatternId(2))]);
            ViolationKind::InvalidId {
                arena: "pattern child",
                index: 2,
            }
        }),
        ("unused match pattern", |f| {
            let wildcard = f.body().matches[0].patterns[3].clone();
            f.body().matches[0].patterns.push(wildcard);
            ViolationKind::WrongKind {
                entity: "unused match pattern",
                index: 4,
            }
        }),
        ("match table without entry", |f| {
            let table = f.body().matches[0].clone();
            f.body().matches.push(table);
            ViolationKind::WrongKind {
                entity: "match table without entry",
                index: 1,
            }
        }),
        ("duplicate contract ordinal", |f| {
            let clause = empty_body(f);
            f.definition.bodies.push(clause);
            f.definition.contracts = vec![(0, BodyId(1)), (0, BodyId(1))];
            ViolationKind::Duplicate {
                entity: "contract",
                index: 0,
            }
        }),
        ("duplicate parameter", |f| {
            let p = f.body().parameters[0];
            f.body().parameters.push(p);
            ViolationKind::Duplicate {
                entity: "parameter",
                index: p.0,
            }
        }),
        ("parameter with local storage", |f| {
            let p = f.body().parameters[0];
            f.body().locals[p.0 as usize].storage = Storage::Local;
            ViolationKind::WrongKind {
                entity: "parameter",
                index: p.0,
            }
        }),
        ("unbound parameter storage", |f| {
            let p = f.slot(u32_kind(), Storage::Parameter);
            ViolationKind::WrongKind {
                entity: "unbound parameter",
                index: f.body().places[p as usize].root.0,
            }
        }),
    ];
    match_fixture().valid();
    for (name, mutate) in cases {
        let mut f = match_fixture();
        let expected = mutate(&mut f);
        assert_eq!(f.kinds(), vec![expected], "{name}");
    }
}

/// LANG-7: `[1, 2]` built into `Inline[u8, size]` for `fn main[n: size]()`.
fn sequence(size: Size, fields: u32) -> Fixture {
    let mut f = Fixture::unit();
    let byte = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let kind = TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![GenericArgument::Type(byte), GenericArgument::Size(size)].into(),
    };
    let fields = (0..fields)
        .map(|i| {
            let p = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
            f.assign(p, Rvalue::Constant(Constant::Integer(i.into())), 0);
            ConstructField {
                field: i,
                value: take(p),
            }
        })
        .collect();
    let sequence = f.slot(kind, Storage::Temporary);
    f.assign(
        sequence,
        Rvalue::Construct {
            constructor: Constructor::Sequence,
            fields,
            destination: Destination::Inferred,
        },
        0,
    );
    owner_signature(&mut f, PassingMode::Take);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    owner.generics = vec![GenericParameter::Size { name: "n".into() }].into();
    f
}

#[test]
fn an_inline_literal_has_exactly_its_constant_length() {
    let n = || {
        Size::Parameter(SizeParameterRef::body(
            scoped(NameSite::Item),
            SizeParamId(0),
        ))
    };
    sequence(Size::Constant(2), 2).valid();
    sequence(Size::Constant(0), 0).valid();
    sequence(Size::Constant(3), 2).broken(vec![
        ViolationKind::Arity {
            entity: "sequence fields",
            expected: 3,
            actual: 2,
        },
        ViolationKind::InvalidConstructor,
    ]);
    // Sizes compare by equality only, so no field count equals an abstract size.
    for fields in [0, 2] {
        sequence(n(), fields).broken(vec![
            ViolationKind::TypeMismatch {
                entity: "sequence size",
            },
            ViolationKind::InvalidConstructor,
        ]);
    }
}

/// Only a closure body has an environment, so only its locals may be
/// captures; an entry body reading one would read storage nobody supplied.
#[test]
fn an_entry_body_has_no_capture_storage() {
    let build = |storage: Storage| {
        let mut f = Fixture::unit();
        let t = f.definition.types.push(ty(u32_kind()));
        let held = add_capture(f.body(), t);
        f.body().locals[held as usize].storage = storage;
        if storage != Storage::Capture {
            f.assign(held, Rvalue::Constant(Constant::Integer(7)), 0);
        }
        let read = f.slot(u32_kind(), Storage::Temporary);
        f.assign(read, Rvalue::Use(copy(held)), 0);
        f
    };
    build(Storage::Local).valid();
    build(Storage::Capture).broken(vec![ViolationKind::WrongKind {
        entity: "entry capture",
        index: 1,
    }]);
}

/// A char constant is a Unicode scalar value: never a surrogate or a value
/// above 0x10FFFF, whose bits no char literal can spell.
#[test]
fn a_char_constant_is_a_unicode_scalar_value() {
    let constant = |value: u128| {
        let mut f = Fixture::unit();
        let p = f.slot(TypeKind::Primitive(Primitive::Char), Storage::Temporary);
        f.assign(p, Rvalue::Constant(Constant::Integer(value)), 0);
        f
    };
    for value in [0, 0x61, 0xd7ff, 0xe000, 0x10_ffff] {
        constant(value).valid();
    }
    for value in [0xd800, 0xdfff, 0x11_0000, 1 << 40] {
        constant(value).broken(vec![ViolationKind::TypeMismatch { entity: "constant" }]);
    }
}
