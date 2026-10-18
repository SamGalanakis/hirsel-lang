//! Hand-built MODE-2 hand-back controls: a `mut x: unique T` parameter and a
//! `@preserves_unique` selection must leave unique; a preserving call keeps it.
use super::*;

/// `fn main(mut x: String)`, with `x` declared unique when `unique`.
fn handback(unique: bool) -> (Fixture, u32) {
    let mut f = Fixture::unit();
    let x = param(&mut f, TypeKind::Primitive(Primitive::String));
    if unique {
        let old = f.body().places[x as usize].ty;
        let mut row = f.definition.types.get(old).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let row = f.definition.types.push(row);
        f.body().locals[x as usize].ty = row;
        f.body().places[x as usize].ty = row;
    }
    (f, x)
}
fn finish(f: &mut Fixture) {
    owner_signature(f, PassingMode::Mut);
}
/// The body shares `x` into a local that is dropped before the return.
fn sharing(unique: bool) -> Fixture {
    let (mut f, x) = handback(unique);
    let kept = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    let c = f.cleanup(vec![drop(kept)]);
    f.assign(kept, Rvalue::Use(copy(x)), 0);
    f.body().blocks[0].terminator = ret(0, c);
    finish(&mut f);
    f
}
fn promise(f: &mut Fixture) {
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        panic!()
    };
    fun.state_promises = vec![StatePromise {
        selector: ParameterRef::bound(ParamId(0)).into(),
        advance_only: false,
        preserve_captures: false,
        preserve_unique: true,
        unretained: Default::default(),
    }]
    .into();
}
/// Every exit that observes the lost proof reports it, the call's failure edge included.
fn handback_error(f: &Fixture, entity: &'static str) {
    let kinds = f.kinds();
    assert!(!kinds.is_empty());
    assert!(
        kinds
            .iter()
            .all(|k| *k == ViolationKind::TypeMismatch { entity }),
        "{kinds:?}"
    );
}

#[test]
fn a_unique_mut_parameter_must_be_handed_back_unique() {
    sharing(false).valid(); // The same body, for a plain `mut` parameter.
    handback_error(&sharing(true), "mutable unique hand-back");
    let (mut f, _) = handback(true);
    finish(&mut f);
    f.valid();
}

#[test]
fn a_preserves_unique_body_is_proved_with_its_selection_entering_unique() {
    let mut f = sharing(false);
    f.valid(); // Without the promise.
    promise(&mut f);
    handback_error(&f, "preserved unique hand-back");
    let (mut f, _) = handback(false);
    finish(&mut f);
    promise(&mut f);
    f.valid();
}

/// `@preserves_unique(x) fn main(mut x: String)` calling `callee(mut x, take s)`,
/// where `s` is a fresh literal or, when `aliased`, a copy of a parameter `y`.
fn preserving_call(preserves: bool, aliased: bool) -> Fixture {
    let (mut f, x) = handback(false);
    let y = param(&mut f, TypeKind::Primitive(Primitive::String));
    let s = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    if aliased {
        f.assign(s, Rvalue::Use(copy(y)), 0);
    } else {
        f.assign(s, Rvalue::Constant(Constant::String("fresh".into())), 0);
    }
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(0),
                value: operand(x, UseKind::Mut),
            },
            CallArgument {
                parameter: ParamId(1),
                value: take(s),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Borrow]);
    promise(&mut f);
    let mut callee = function_signature(
        "callee",
        vec![
            (PassingMode::Mut, TypeKind::Primitive(Primitive::String)),
            (PassingMode::Take, TypeKind::Primitive(Primitive::String)),
        ],
        TypeKind::Tuple(vec![].into()),
    );
    if preserves {
        let SignatureKind::Function(fun) = &mut callee.kind else {
            panic!()
        };
        fun.state_promises = vec![StatePromise {
            selector: ParameterRef::bound(ParamId(0)).into(),
            advance_only: false,
            preserve_captures: false,
            preserve_unique: true,
            unretained: Default::default(),
        }]
        .into();
    }
    f.signatures.insert(callee.definition.clone(), callee);
    f
}

#[test]
fn a_preserving_call_with_unique_inputs_keeps_the_argument_unique() {
    preserving_call(true, false).valid();
    handback_error(&preserving_call(false, false), "preserved unique hand-back");
    handback_error(&preserving_call(true, true), "preserved unique hand-back");
}

/// `fn main() -> unique String { callee(take "fresh") }`, through a callee
/// whose taken parameter may be selected by `@preserves_unique`.
fn preserving_result(preserves: bool) -> Fixture {
    let mut f = Fixture::result_call(TypeKind::Primitive(Primitive::String), true);
    let result = f.body().result;
    let mut row = f.definition.types.get(result).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let row = f.definition.types.push(row);
    f.body().result = row;
    f.body().locals[0].ty = row;
    f.body().places[0].ty = row;
    let s = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let id = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Constant(Constant::String("fresh".into())));
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(s),
            value: id,
        },
        0,
    ));
    let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
        panic!()
    };
    arguments.push(CallArgument {
        parameter: ParamId(0),
        value: take(s),
    });
    owner_signature(&mut f, PassingMode::Take);
    let mut callee = function_signature(
        "callee",
        vec![(PassingMode::Take, TypeKind::Primitive(Primitive::String))],
        TypeKind::Primitive(Primitive::String),
    );
    if preserves {
        let SignatureKind::Function(fun) = &mut callee.kind else {
            panic!()
        };
        fun.state_promises = vec![StatePromise {
            selector: ParameterRef::bound(ParamId(0)).into(),
            advance_only: false,
            preserve_captures: false,
            preserve_unique: true,
            unretained: Default::default(),
        }]
        .into();
    }
    f.signatures.insert(callee.definition.clone(), callee);
    f
}

#[test]
fn a_preserving_taken_input_makes_the_result_unique() {
    preserving_result(true).valid();
    assert!(!preserving_result(false).kinds().is_empty());
}

#[test]
fn an_indirect_call_does_not_use_the_promise() {
    let mut f = preserving_call(true, false);
    f.valid(); // The direct call keeps `x` unique.
    let mut signature = f.signatures[&path("callee")].clone();
    let SignatureKind::Function(fun) = signature.kind.clone() else {
        panic!()
    };
    let row = signature.types.push(ty(TypeKind::Function(fun)));
    let imported = import_type(&signature.types, row, &mut f.definition.types).unwrap();
    let slot = f.body().locals.len() as u32;
    f.body().locals.push(Local {
        ty: imported,
        storage: Storage::Parameter,
        scope: ScopeId(0),
        span: Span::default(),
    });
    let place = f.body().places.len() as u32;
    f.body().places.push(Place {
        root: LocalId(slot),
        projection: vec![].into(),
        loan: None,
        binding: None,
        ty: imported,
    });
    f.body().parameters.push(LocalId(slot));
    owner_sig(
        &mut f,
        &[PassingMode::Mut, PassingMode::Borrow, PassingMode::Borrow],
    );
    promise(&mut f);
    let Terminator::Call { callee, .. } = &mut f.body().blocks[0].terminator else {
        panic!()
    };
    *callee = Callee::Indirect {
        value: operand(place, UseKind::Borrow),
        arguments: vec![],
    };
    handback_error(&f, "preserved unique hand-back");
}

/// `@preserves_unique(x) fn main(mut x: String, y: String)` calling a
/// preserving `callee(mut x, y)` that borrows `y`, declared in core@1 when `core`,
/// whose promise marks the `unretained` parameters.
fn borrowing_call(core: bool, unretained: &[u32]) -> Fixture {
    let (mut f, x) = handback(false);
    let y = param(&mut f, TypeKind::Primitive(Primitive::String));
    let callee_path = if core {
        DefPath::core("test.keep")
    } else {
        path("callee")
    };
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: callee_path.clone(),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(0),
                value: operand(x, UseKind::Mut),
            },
            CallArgument {
                parameter: ParamId(1),
                value: operand(y, UseKind::Borrow),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Borrow]);
    promise(&mut f);
    let mut callee = function_signature(
        "callee",
        vec![
            (PassingMode::Mut, TypeKind::Primitive(Primitive::String)),
            (PassingMode::Borrow, TypeKind::Primitive(Primitive::String)),
        ],
        TypeKind::Tuple(vec![].into()),
    );
    callee.definition = callee_path.clone();
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    fun.state_promises = vec![StatePromise {
        selector: ParameterRef::bound(ParamId(0)).into(),
        advance_only: false,
        preserve_captures: false,
        preserve_unique: true,
        unretained: unretained
            .iter()
            .copied()
            .map(ParamId)
            .collect::<Vec<_>>()
            .into(),
    }]
    .into();
    f.signatures.insert(callee_path, callee);
    f
}

#[test]
fn a_bodyless_core_promise_needs_plain_borrowed_arguments() {
    borrowing_call(false, &[]).valid(); // A proved body only shares out of `y`.
    handback_error(&borrowing_call(true, &[]), "preserved unique hand-back");
}

#[test]
fn an_unretained_borrowed_argument_is_exempt_from_the_core_clause() {
    borrowing_call(true, &[1]).valid(); // The ledger says `y` is only read.
    // Marking another parameter leaves `y` under the clause.
    handback_error(&borrowing_call(true, &[0]), "preserved unique hand-back");
}

/// `fn main(mut x: unique String)` calling `view(x) -> String in x`, whose
/// counted result holds a share of `x`: dropped before the return, or returned.
fn viewing_call(escape: bool) -> Fixture {
    let (mut f, x) = handback(true);
    let string = TypeKind::Primitive(Primitive::String);
    let entry = f.cleanup(vec![]);
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
        .on_failure = CleanupId(entry);
    let via = f.through_loan(x, 0);
    let held = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    // The result relates to `x`, an owned parameter, so this frame sees it global.
    let destination = if escape {
        0
    } else {
        f.slot(string.clone(), Storage::Temporary)
    };
    if escape {
        // The call writes the return slot, so the unit initialization goes.
        f.body().blocks[0].instructions.remove(0);
        f.body().rvalues.clear();
        let row = f.definition.types.push(ty(string.clone()));
        f.body().result = row;
        f.body().locals[0].ty = row;
        f.body().places[0].ty = row;
    }
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
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(held),
    };
    let exit = if escape {
        held
    } else {
        f.cleanup(vec![
            drop(destination),
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        ])
    };
    f.body().blocks.push(block(vec![], ret(0, exit)));
    finish(&mut f);
    let mut callee =
        function_signature("view", vec![(PassingMode::Borrow, string.clone())], string);
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    let mut result = callee.types.get(fun.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    fun.result = callee.types.push(result);
    f.signatures.insert(path("view"), callee);
    f
}

#[test]
fn a_related_result_share_ends_with_the_result() {
    // The result's share of `x` dies before the return: `x` leaves unique.
    viewing_call(false).valid();
    // The returned result keeps it, so `x` cannot be handed back unique.
    let kinds = viewing_call(true).kinds();
    assert!(
        kinds.contains(&ViolationKind::TypeMismatch {
            entity: "mutable unique hand-back"
        }),
        "{kinds:?}"
    );
}

#[test]
fn a_foreign_promise_is_not_used() {
    let mut f = preserving_call(true, false);
    f.valid(); // A proved callee keeps `x` unique.
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("callee")).unwrap().kind
    else {
        panic!()
    };
    fun.foreign = Some(ForeignContract {
        abi: "c".into(),
        blocking: false,
        aliasing_contract: "Retains nothing.".into(),
    });
    handback_error(&f, "preserved unique hand-back");
}

/// Where the share `view(x)` returns is when the body next needs `x` unique.
#[derive(Clone, Copy, PartialEq)]
enum Share {
    /// Dropped first.
    Dropped,
    /// Still in its temporary.
    Held,
    /// Moved to another local, which still holds it.
    Moved,
    /// Held until the return of the body's taken `y` releases it.
    Returned,
}

/// `fn main(mut x: unique String, take y: String)` borrowing `x` into
/// `view(x) -> String in x`, then calling `need(mut x: unique String)` or,
/// for `Returned`, returning `y`.
fn viewed_then_needed(share: Share) -> Fixture {
    let (mut f, x) = handback(true);
    let string = TypeKind::Primitive(Primitive::String);
    let y = param(&mut f, string.clone());
    if share == Share::Returned {
        f.body().blocks[0].instructions.remove(0);
        f.body().rvalues.clear();
        let row = f.body().places[y as usize].ty;
        f.body().result = row;
        f.body().locals[0].ty = row;
        f.body().places[0].ty = row;
    }
    let entry = f.cleanup(vec![drop(y)]);
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
        .on_failure = CleanupId(entry);
    let via = f.through_loan(x, 0);
    let view = f.slot(string.clone(), Storage::Temporary);
    let lent = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(y),
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
            destination: PlaceId(view),
            next: BlockId(1),
        }),
        on_failure: CleanupId(lent),
    };
    let mut after = vec![inst(
        InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        f.cleanup(vec![drop(view), drop(y)]),
    )];
    let holder = match share {
        Share::Dropped => {
            after.push(inst(
                InstructionKind::Drop {
                    place: PlaceId(view),
                    if_live: false,
                },
                entry,
            ));
            None
        }
        Share::Held | Share::Returned => Some(view),
        Share::Moved => {
            let kept = f.slot(string.clone(), Storage::Local);
            let id = RvalueId(f.body().rvalues.len() as u32);
            f.body().rvalues.push(Rvalue::Use(take(view)));
            after.push(inst(
                InstructionKind::Assign {
                    destination: PlaceId(kept),
                    value: id,
                },
                f.cleanup(vec![drop(view), drop(y)]),
            ));
            Some(kept)
        }
    };
    let held: Vec<_> = holder.into_iter().map(drop).collect();
    if share == Share::Returned {
        let exit = f.cleanup(held);
        f.body().blocks.push(block(after, ret(y, exit)));
    } else {
        let unit = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let failed = f.cleanup(held.iter().cloned().chain([drop(y)]).collect());
        f.body().blocks.push(block(
            after,
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("need"),
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: operand(x, UseKind::Mut),
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(unit),
                    next: BlockId(2),
                }),
                on_failure: CleanupId(failed),
            },
        ));
        let exit = f.cleanup(held.into_iter().chain([drop(unit), drop(y)]).collect());
        f.body().blocks.push(block(vec![], ret(0, exit)));
    }
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Take]);
    let mut callee = function_signature(
        "view",
        vec![(PassingMode::Borrow, string.clone())],
        string.clone(),
    );
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    let mut result = callee.types.get(fun.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    fun.result = callee.types.push(result);
    f.signatures.insert(path("view"), callee);
    let mut need = function_signature(
        "need",
        vec![(PassingMode::Mut, string)],
        TypeKind::Tuple(vec![].into()),
    );
    let SignatureKind::Function(fun) = &mut need.kind else {
        panic!()
    };
    let mut row = need.types.get(fun.parameters[0].ty).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    fun.parameters[0].ty = need.types.push(row);
    f.signatures.insert(path("need"), need);
    f
}

#[test]
fn a_dead_related_share_leaves_its_storage_unique_mid_body() {
    // The share died before the body needs `x` unique again.
    viewed_then_needed(Share::Dropped).valid();
    // Its temporary, or the local it moved to, still holds it.
    assert!(!viewed_then_needed(Share::Held).kinds().is_empty());
    assert!(!viewed_then_needed(Share::Moved).kinds().is_empty());
}

#[test]
fn a_returned_value_keeps_only_the_shares_it_derives_from() {
    // The returned `y` is counted but unrelated to `x`'s dead share.
    viewed_then_needed(Share::Returned).valid();
}

/// Hand-built IR has no source annotation: the receiver's association alone
/// selects the builder for proof. The identical unassociated body is valid.
fn format_body(aliases: bool, associated: bool) -> Fixture {
    let mut f = Fixture::unit();
    param(
        &mut f,
        TypeKind::Named {
            definition: path("Point"),
            arguments: vec![].into(),
        },
    );
    let byte = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let builder = TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(byte)].into(),
    };
    let out = param(&mut f, builder.clone());
    if aliases {
        let kept = f.slot(builder, Storage::Local);
        let cleanup = f.cleanup(vec![drop(kept)]);
        f.assign(kept, Rvalue::Use(copy(out)), 0);
        f.body().blocks[0].terminator = ret(0, cleanup);
    }
    owner_sig(&mut f, &[PassingMode::Borrow, PassingMode::Mut]);
    let mut point = function_signature("Point", vec![], TypeKind::Tuple(vec![].into()));
    let representation = point.types.push(Type::unit());
    point.kind = SignatureKind::Type(TypeDefinition {
        loop_source: None,
        format: associated.then(|| path("main")),
        generics: vec![].into(),
        representation: Some(representation),
        bounds: vec![Bound::Plain],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority: None,
        serializable: false,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        destructor_allocation: Allocation::NoAlloc,
    });
    f.signatures.insert(point.definition.clone(), point);
    f
}

#[test]
fn a_format_association_implies_the_builder_body_proof() {
    format_body(true, false).valid();
    format_body(false, true).valid();
    handback_error(&format_body(true, true), "preserved unique hand-back");
}

/// `fn main(mut x: String, mut z: String)` calling `callee(mut x, mut z)`,
/// which declares `@preserves_unique(x)` and is core@1 when `core`. With
/// `proving`, main declares `@preserves_unique(x)` itself; otherwise its `x`
/// is `unique`.
fn sibling_call(core: bool, proving: bool) -> Fixture {
    let (mut f, x) = handback(!proving);
    let z = param(&mut f, TypeKind::Primitive(Primitive::String));
    let callee_path = if core {
        DefPath::core("test.keep")
    } else {
        path("callee")
    };
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: callee_path.clone(),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(0),
                value: operand(x, UseKind::Mut),
            },
            CallArgument {
                parameter: ParamId(1),
                value: operand(z, UseKind::Mut),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Mut]);
    if proving {
        promise(&mut f);
    }
    let mut callee = function_signature(
        "callee",
        vec![
            (PassingMode::Mut, TypeKind::Primitive(Primitive::String)),
            (PassingMode::Mut, TypeKind::Primitive(Primitive::String)),
        ],
        TypeKind::Tuple(vec![].into()),
    );
    callee.definition = callee_path.clone();
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    fun.state_promises = vec![StatePromise {
        selector: ParameterRef::bound(ParamId(0)).into(),
        advance_only: false,
        preserve_captures: false,
        preserve_unique: true,
        unretained: Default::default(),
    }]
    .into();
    f.signatures.insert(callee_path, callee);
    f
}

#[test]
fn a_plain_mut_sibling_does_not_block_a_proved_promise() {
    // The callee's proof took `z` aliased, so `x` comes back unique.
    sibling_call(false, false).valid();
    // A bodyless core promise has no proof covering `z`.
    handback_error(&sibling_call(true, false), "mutable unique hand-back");
}

#[test]
fn a_preserves_unique_proof_enters_a_plain_mut_sibling_aliased() {
    // `z` enters as declared, so the core callee cannot keep `x` unique.
    handback_error(&sibling_call(true, true), "preserved unique hand-back");
    sibling_call(false, true).valid();
}

/// `fn main(take xs: List[unique String]) -> String` returning
/// `callee[unique String](take xs)`, where `fn callee[T](take value: List[T]) -> T`.
/// With `forged`, the call's destination and main's result claim `unique`.
fn generic_result(forged: bool) -> Fixture {
    let mut f = Fixture::result_call(TypeKind::Primitive(Primitive::String), true);
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let mut row = f.definition.types.get(string).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let unique = f.definition.types.push(row);
    let xs = param(
        &mut f,
        TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(unique)].into(),
        },
    );
    let Terminator::Call {
        callee: Callee::Definition {
            arguments: generics,
            ..
        },
        arguments,
        returns: Some(returns),
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    *generics = vec![GenericArgument::Type(unique)];
    arguments.push(CallArgument {
        parameter: ParamId(0),
        value: take(xs),
    });
    let destination = returns.destination;
    if forged {
        let body = f.body();
        body.result = unique;
        body.locals[0].ty = unique;
        body.places[0].ty = unique;
        let root = body.places[destination.0 as usize].root;
        body.locals[root.0 as usize].ty = unique;
        body.places[destination.0 as usize].ty = unique;
    }
    owner_sig(&mut f, &[PassingMode::Take]);
    let mut callee = function_signature(
        "callee",
        vec![],
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
    );
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    let list = callee.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(fun.result)].into(),
    }));
    fun.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Take,
        ty: list,
    }]
    .into();
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    f.signatures.insert(callee.definition.clone(), callee);
    f
}

#[test]
fn a_bare_generic_result_holds_its_declared_modes() {
    // `T` is instantiated by a contract position, so it carries `unique`, but
    // the result is a value occurrence: aliased, as the checker publishes it.
    generic_result(false).valid();
    let kinds = generic_result(true).kinds();
    assert!(
        kinds.contains(&ViolationKind::TypeMismatch {
            entity: "call result"
        }),
        "{kinds:?}"
    );
}

/// `fn main(take xs: List[String]) -> unique String` returning
/// `first[String](take xs)`, where `fn first[T](take value: List[T]) -> T`
/// cannot copy `T`; `xs` is declared unique when `unique`.
fn parametric_call(unique: bool) -> Fixture {
    let mut f = Fixture::result_call(TypeKind::Primitive(Primitive::String), true);
    let result = f.body().result;
    let mut row = f.definition.types.get(result).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let row = f.definition.types.push(row);
    f.body().result = row;
    f.body().locals[0].ty = row;
    f.body().places[0].ty = row;
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let xs = param(
        &mut f,
        TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(string)].into(),
        },
    );
    if unique {
        let old = f.body().places[xs as usize].ty;
        let mut row = f.definition.types.get(old).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let row = f.definition.types.push(row);
        f.body().locals[xs as usize].ty = row;
        f.body().places[xs as usize].ty = row;
    }
    let Terminator::Call {
        callee: Callee::Definition {
            arguments: generics,
            ..
        },
        arguments,
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    *generics = vec![GenericArgument::Type(string)];
    arguments.push(CallArgument {
        parameter: ParamId(0),
        value: take(xs),
    });
    owner_sig(&mut f, &[PassingMode::Take]);
    let mut callee = function_signature(
        "callee",
        vec![],
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
    );
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    let list = callee.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(fun.result)].into(),
    }));
    fun.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Take,
        ty: list,
    }]
    .into();
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    f.signatures.insert(callee.definition.clone(), callee);
    f
}

#[test]
fn a_parametric_result_is_unique_only_from_unique_inputs() {
    // `first` can only move a `T` out of its unique list.
    parametric_call(true).valid();
    // An aliased list may share its elements: the unique result is forged.
    assert!(!parametric_call(false).kinds().is_empty());
}
