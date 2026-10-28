//! Hand-built MODE-2 hand-back controls: a `mut x: unique T` parameter and a
//! `@preserves_unique` selection must leave unique; a preserving call keeps it.
use super::*;

/// `fn main(mut x: String)`, with `x` declared unique when `unique`.
fn handback(unique: bool) -> (Fixture, u32) {
    let mut f = Fixture::unit();
    let x = param(&mut f, TypeKind::Primitive(Primitive::String));
    if unique {
        let old = f.body().places[x as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
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
/// Every exit that observes the lost proof reports it, the call's failure edge
/// included: `exits` reports, each naming the hand-back.
fn handback_error(f: &Fixture, entity: &'static str, exits: usize) {
    assert_eq!(
        f.kinds(),
        vec![ViolationKind::TypeMismatch { entity }; exits]
    );
}

#[test]
fn a_unique_mut_parameter_must_be_handed_back_unique() {
    sharing(false).valid(); // The same body, for a plain `mut` parameter.
    handback_error(&sharing(true), "mutable unique hand-back", 1);
    let (mut f, _) = handback(true);
    finish(&mut f);
    f.valid();
}

#[test]
fn a_preserves_unique_body_is_proved_with_its_selection_entering_unique() {
    let mut f = sharing(false);
    f.valid(); // Without the promise.
    promise(&mut f);
    handback_error(&f, "preserved unique hand-back", 1);
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
    handback_error(
        &preserving_call(false, false),
        "preserved unique hand-back",
        2,
    );
    handback_error(
        &preserving_call(true, true),
        "preserved unique hand-back",
        2,
    );
}

/// Before/after-call and unwind separation follows rustc_mir_dataflow's
/// initialized.rs:255-303 and framework/tests.rs:1-68 at
/// 19833af7224424e8b53e288097d56b6af43d5bcf (MIT, Rust Project Contributors).
/// Only the test shape is adapted: Hirsel's current proof is the oracle.
#[test]
fn a_late_share_is_not_repaired_by_a_preserving_call_or_its_return() {
    let mut f = preserving_call(true, false);
    f.valid();
    // The last argument is evaluated after reserving x, and shares x.
    // The callee's promise cannot supply the missing selected-input proof.
    f.body().rvalues[1] = Rvalue::Use(copy(1));
    handback_error(&f, "preserved unique hand-back", 2);

    let mut dropped = preserving_call(true, false);
    let saved = dropped.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    dropped.assign(saved, Rvalue::Use(copy(1)), 0);
    let cleanup = dropped.cleanup(vec![drop(3)]);
    dropped.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(saved),
            if_live: false,
        },
        cleanup,
    ));
    handback_error(&dropped, "preserved unique hand-back", 2);
}

#[test]
fn a_unique_call_requirement_does_not_supply_its_own_entry_evidence() {
    let mut f = preserving_call(true, false);
    // The ordinary body must meet this fixed requirement as well as the
    // conditional re-proof: declare x unique on the caller itself.
    let mut incoming = interner().get(f.body().places[1].ty).unwrap().clone();
    incoming.modes.uniqueness = Uniqueness::Unique;
    let incoming = f.definition.types.push(incoming);
    f.body().locals[1].ty = incoming;
    f.body().places[1].ty = incoming;
    owner_sig(&mut f, &[PassingMode::Mut, PassingMode::Borrow]);
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("callee")).unwrap().kind
    else {
        panic!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let mut parameters = fun.parameters.to_vec();
    parameters[0].ty = interner().intern(row);
    fun.parameters = parameters.into();
    f.valid();
    f.body().rvalues[1] = Rvalue::Use(copy(1));
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::TypeMismatch {
            entity: "call argument"
        }),
        "{kinds:?}"
    );
}

#[test]
fn a_handback_promise_does_not_cover_another_parameter() {
    let mut f = sibling_call(false, false);
    f.valid();
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("callee")).unwrap().kind
    else {
        panic!()
    };
    let mut promises = fun.state_promises.to_vec();
    promises[0].selector.parameter = ParameterRef::bound(ParamId(1));
    fun.state_promises = promises.into();
    handback_error(&f, "mutable unique hand-back", 2);
}

#[test]
fn a_call_without_a_normal_successor_still_checks_its_failure_handback() {
    for (preserves, shared) in [(true, false), (false, false), (true, true)] {
        let mut f = preserving_call(preserves, shared);
        let Terminator::Call { returns, .. } = &mut f.body().blocks[0].terminator else {
            panic!()
        };
        *returns = None;
        f.body().blocks.truncate(1);
        let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("callee")).unwrap().kind
        else {
            panic!()
        };
        fun.result = interner().intern(ty(TypeKind::Primitive(Primitive::Never)));
        if preserves && !shared {
            f.valid();
        } else {
            handback_error(&f, "preserved unique hand-back", 1);
        }
    }
}

/// `fn main() -> unique String { callee(take "fresh") }`, through a callee
/// whose taken parameter may be selected by `@preserves_unique`.
fn preserving_result(preserves: bool) -> Fixture {
    let mut f = Fixture::result_call(TypeKind::Primitive(Primitive::String), true);
    let result = f.body().result;
    let mut row = interner().get(result).unwrap().clone();
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
    assert_eq!(
        preserving_result(false).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "value modes"
        }]
    );
}

#[test]
fn an_indirect_call_does_not_use_the_promise() {
    let mut f = preserving_call(true, false);
    f.valid(); // The direct call keeps `x` unique.
    let mut signature = f.signatures.get(id("callee")).unwrap().clone();
    let SignatureKind::Function(fun) = signature.kind.clone() else {
        panic!()
    };
    let row = interner().intern(ty(TypeKind::Function(fun)));
    let imported = row;
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
        forward_caller: false,
        value: operand(place, UseKind::Borrow),
        arguments: vec![],
    };
    handback_error(&f, "preserved unique hand-back", 2);
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
    callee.definition = DefId::intern(&callee_path);
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
    f.signatures.insert(&callee_path, callee);
    f
}

#[test]
fn a_bodyless_core_promise_needs_plain_borrowed_arguments() {
    borrowing_call(false, &[]).valid(); // A proved body only shares out of `y`.
    handback_error(&borrowing_call(true, &[]), "preserved unique hand-back", 2);
}

#[test]
fn an_unretained_borrowed_argument_is_exempt_from_the_core_clause() {
    borrowing_call(true, &[1]).valid(); // The ledger says `y` is only read.
    // Marking another parameter leaves `y` under the clause.
    handback_error(&borrowing_call(true, &[0]), "preserved unique hand-back", 2);
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
    let mut result = interner().get(fun.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    fun.result = interner().intern(result);
    f.signatures.insert(id("view"), callee);
    f
}

#[test]
fn a_related_result_share_ends_with_the_result() {
    // The result's share of `x` dies before the return: `x` leaves unique.
    viewing_call(false).valid();
    // The returned result keeps it, so `x` cannot be handed back unique.
    assert_eq!(
        viewing_call(true).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "mutable unique hand-back"
        }]
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
    handback_error(&f, "preserved unique hand-back", 2);
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
    view_and_need(&mut f);
    f
}

/// `view(s: String) -> String in s` and `need(mut s: unique String)`.
fn view_and_need(f: &mut Fixture) {
    let string = TypeKind::Primitive(Primitive::String);
    let mut callee = function_signature(
        "view",
        vec![(PassingMode::Borrow, string.clone())],
        string.clone(),
    );
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    let mut result = interner().get(fun.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    fun.result = interner().intern(result);
    f.signatures.insert(id("view"), callee);
    let mut need = function_signature(
        "need",
        vec![(PassingMode::Mut, string)],
        TypeKind::Tuple(vec![].into()),
    );
    let SignatureKind::Function(fun) = &mut need.kind else {
        panic!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    fun.parameters[0].ty = interner().intern(row);
    f.signatures.insert(id("need"), need);
}

#[test]
fn a_dead_related_share_leaves_its_storage_unique_mid_body() {
    // The share died before the body needs `x` unique again.
    viewed_then_needed(Share::Dropped).valid();
    // Its temporary, or the local it moved to, still holds it.
    assert_eq!(
        viewed_then_needed(Share::Held).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "call argument"
        }]
    );
    assert_eq!(
        viewed_then_needed(Share::Moved).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "call argument"
        }]
    );
}

#[test]
fn a_returned_value_keeps_only_the_shares_it_derives_from() {
    // The returned `y` is counted but unrelated to `x`'s dead share.
    viewed_then_needed(Share::Returned).valid();
}

/// `fn main(mut x: unique String)` passing the related share `view(x)` by
/// borrow to `digest(s: String) -> String`, keeping the digest in a local
/// and dropping the share, then calling `need(mut x)`. With `related`, the
/// digest is `in s` and so may still hold the share.
fn digested_then_needed(related: bool) -> Fixture {
    let (mut f, x) = handback(true);
    let string = TypeKind::Primitive(Primitive::String);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(x),
        mutable: false,
        if_inactive: false,
    });
    let via = f.through_loan(x, 0);
    let shared = f.slot(string.clone(), Storage::Temporary);
    let lent = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
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
            destination: PlaceId(shared),
            next: BlockId(1),
        }),
        on_failure: CleanupId(lent),
    };
    let holding = f.cleanup(vec![drop(shared)]);
    let through = f.through_loan(shared, 1);
    let digest = f.slot(string.clone(), Storage::Local);
    let reading = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        drop(shared),
    ]);
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                holding,
            ),
            inst(
                InstructionKind::BeginLoan {
                    id: LoanId(1),
                    place: PlaceId(shared),
                    mutable: false,
                    if_inactive: false,
                },
                reading,
            ),
        ],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("digest"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: operand(through, UseKind::Borrow),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(digest),
                next: BlockId(2),
            }),
            on_failure: CleanupId(reading),
        },
    ));
    let both = f.cleanup(vec![drop(digest), drop(shared)]);
    let kept = f.cleanup(vec![drop(digest)]);
    let unit = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::EndLoan {
                    loan: LoanId(1),
                    if_active: false,
                },
                both,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(shared),
                    if_live: false,
                },
                kept,
            ),
        ],
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
                next: BlockId(3),
            }),
            on_failure: CleanupId(kept),
        },
    ));
    let exit = f.cleanup(vec![drop(unit), drop(digest)]);
    f.body().blocks.push(block(vec![], ret(0, exit)));
    owner_sig(&mut f, &[PassingMode::Mut]);
    view_and_need(&mut f);
    let mut digest = function_signature(
        "digest",
        vec![(PassingMode::Borrow, string.clone())],
        string,
    );
    if related {
        let SignatureKind::Function(fun) = &mut digest.kind else {
            panic!()
        };
        let mut result = interner().get(fun.result).unwrap().clone();
        result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
        fun.result = interner().intern(result);
    }
    f.signatures.insert(id("digest"), digest);
    f
}

#[test]
fn a_global_result_holds_no_share_of_a_borrowed_argument() {
    // MODE-1: `digest`'s global result cannot reach its borrowed argument,
    // so `x` is unique again once the share itself is dropped.
    digested_then_needed(false).valid();
    // A result related to the argument may still hold the share.
    assert!(!digested_then_needed(true).kinds().is_empty());
}

/// Hand-built IR has no source annotation: the receiver's association alone
/// selects the builder for proof. The identical unassociated body is valid.
fn format_body(aliases: bool, associated: bool) -> Fixture {
    let mut f = Fixture::unit();
    param(
        &mut f,
        TypeKind::Named {
            definition: id("Point"),
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
    let representation = interner().intern(Type::unit());
    point.kind = SignatureKind::Type(Box::new(TypeDefinition {
        loop_source: None,
        format: associated.then(|| path("main")),
        generics: vec![].into(),
        representation: Some(representation),
        bounds: vec![Bound::Plain],
        authority: AuthoritySummary::inert(),
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
        destructor_allocation: Allocation::NoAlloc,
    }));
    f.signatures.insert(point.definition.clone(), point);
    f
}

#[test]
fn a_format_association_implies_the_builder_body_proof() {
    format_body(true, false).valid();
    format_body(false, true).valid();
    handback_error(&format_body(true, true), "preserved unique hand-back", 1);
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
    callee.definition = DefId::intern(&callee_path);
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
    f.signatures.insert(&callee_path, callee);
    f
}

#[test]
fn a_plain_mut_sibling_does_not_block_a_proved_promise() {
    // The callee's proof took `z` aliased, so `x` comes back unique.
    sibling_call(false, false).valid();
    // A bodyless core promise has no proof covering `z`.
    handback_error(&sibling_call(true, false), "mutable unique hand-back", 2);
}

#[test]
fn a_preserves_unique_proof_enters_a_plain_mut_sibling_aliased() {
    // `z` enters as declared, so the core callee cannot keep `x` unique.
    handback_error(&sibling_call(true, true), "preserved unique hand-back", 2);
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
    let mut row = interner().get(string).unwrap().clone();
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
    let list = interner().intern(ty(TypeKind::Builtin {
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
    let mut row = interner().get(result).unwrap().clone();
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
        let mut row = interner().get(old).unwrap().clone();
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
    let list = interner().intern(ty(TypeKind::Builtin {
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
    assert_eq!(
        parametric_call(false).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "value modes"
        }]
    );
}

/// A resource whose finalizer may fail (DROP-2).
fn token_signature() -> Signature {
    let mut signature = function_signature("Token", vec![], TypeKind::Tuple(vec![].into()));
    let scalar = interner().intern(ty(TypeKind::Primitive(Primitive::U64)));
    let representation = interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "value".into(),
            ty: scalar,
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
        drop: Some(path("Token_drop")),
        fact: None,
        captures: None,
    }));
    signature
}

/// `fn main(mut x: unique String, take fresh: unique String, take b: Token)`
/// copies `x`, then replaces it with `fresh`. With `weakened`, `b` is released
/// between the copy and the replacement; otherwise before the copy.
fn released_resource(weakened: bool) -> Fixture {
    let (mut f, x) = handback(true);
    let string = TypeKind::Primitive(Primitive::String);
    let fresh = param(&mut f, string.clone());
    let old = f.body().places[fresh as usize].ty;
    let mut row = interner().get(old).unwrap().clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let row = f.definition.types.push(row);
    f.body().locals[fresh as usize].ty = row;
    f.body().places[fresh as usize].ty = row;
    let b = param(
        &mut f,
        TypeKind::Named {
            definition: id("Token"),
            arguments: vec![].into(),
        },
    );
    f.signatures.insert(id("Token"), token_signature());
    let kept = f.slot(string.clone(), Storage::Local);
    let displaced = f.slot(string, Storage::Temporary);
    let release = |on_failure| {
        inst(
            InstructionKind::Drop {
                place: PlaceId(b),
                if_live: false,
            },
            on_failure,
        )
    };
    let copy_id = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(copy(x)));
    let copied = |on_failure| {
        inst(
            InstructionKind::Assign {
                destination: PlaceId(kept),
                value: copy_id,
            },
            on_failure,
        )
    };
    let fresh_only = f.cleanup(vec![drop(fresh)]);
    let with_b = f.cleanup(vec![drop(fresh), drop(b)]);
    let kept_fresh = f.cleanup(vec![drop(kept), drop(fresh)]);
    let kept_only = f.cleanup(vec![drop(kept)]);
    let kept_displaced = f.cleanup(vec![drop(kept), drop(displaced)]);
    let mut instructions = if weakened {
        vec![copied(with_b), release(kept_fresh)]
    } else {
        vec![release(fresh_only), copied(fresh_only)]
    };
    instructions.push(inst(
        InstructionKind::Replace {
            destination: PlaceId(x),
            value: take(fresh),
            displaced: PlaceId(displaced),
            if_live: false,
        },
        kept_only,
    ));
    instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(displaced),
            if_live: false,
        },
        kept_displaced,
    ));
    f.body().blocks[0].instructions.extend(instructions);
    f.body().blocks[0].terminator = ret(0, kept_only);
    owner_sig(
        &mut f,
        &[PassingMode::Mut, PassingMode::Take, PassingMode::Take],
    );
    f
}

#[test]
fn a_failing_finalizer_is_a_hand_back_failure_site() {
    released_resource(false).valid();
    // The finalizer may fail while `kept` shares `x`.
    handback_error(&released_resource(true), "mutable unique hand-back", 1);
}

/// What a `parametric_call` variant passes `first` beside `xs`.
enum Extra {
    /// A borrowed `seen: List[String]` for `seen: List[T]`; `first` is a
    /// bodyless core declaration when `core`.
    Borrowed { core: bool },
    /// A unique `tag: Tagged[String]` taken for `tag: Tagged[T]`, where
    /// `Tagged[T] { item: T, label: L }` and `L` is `Shared[String]`, or
    /// `fn() -> T` when `thunk`.
    Named { thunk: bool },
    /// A callback `f: fn(String) -> u64` for `f: fn(view: T) -> u64`, whose
    /// function type is `uses {}` when `pure`.
    Lent { pure: bool },
}

fn tagged(thunk: bool) -> Signature {
    let mut tagged = function_signature("Tagged", vec![], TypeKind::Tuple(vec![].into()));
    let item = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let label = if thunk {
        let inner = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        })));
        interner().intern(ty(TypeKind::Function(fn_type(vec![], inner))))
    } else {
        let string = interner().intern(ty(TypeKind::Primitive(Primitive::String)));
        interner().intern(ty(TypeKind::Builtin {
            kind: BuiltinType::Shared,
            arguments: vec![GenericArgument::Type(string)].into(),
        }))
    };
    let representation = interner().intern(ty(TypeKind::Record(
        vec![
            Field {
                growable: false,
                name: "item".into(),
                ty: item,
            },
            Field {
                growable: false,
                name: "label".into(),
                ty: label,
            },
        ]
        .into(),
    )));
    tagged.kind = SignatureKind::Type(Box::new(TypeDefinition {
        loop_source: None,
        format: None,
        generics: vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        }],
        representation: Some(representation),
        bounds: vec![],
        authority: AuthoritySummary::inert(),
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
        destructor_allocation: Allocation::NoAlloc,
    }));
    tagged
}

/// `parametric_call(true)` with a second argument, `extra`.
fn parametric_variant(extra: Extra) -> Fixture {
    let mut f = parametric_call(true);
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let (kind, usage, passing) = match &extra {
        Extra::Borrowed { .. } => (
            TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(string)].into(),
            },
            UseKind::Borrow,
            PassingMode::Borrow,
        ),
        Extra::Named { .. } => (
            TypeKind::Named {
                definition: id("Tagged"),
                arguments: vec![GenericArgument::Type(string)].into(),
            },
            UseKind::Take,
            PassingMode::Take,
        ),
        Extra::Lent { .. } => {
            let count = f
                .definition
                .types
                .push(ty(TypeKind::Primitive(Primitive::U64)));
            let parameter = Parameter {
                name: None,
                label: None,
                passing: PassingMode::Borrow,
                ty: string,
            };
            (
                TypeKind::Function(fn_type(vec![parameter], count)),
                UseKind::Borrow,
                PassingMode::Borrow,
            )
        }
    };
    let second = param(&mut f, kind);
    if matches!(extra, Extra::Named { .. }) {
        let old = f.body().places[second as usize].ty;
        let mut row = interner().get(old).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        let row = f.definition.types.push(row);
        f.body().locals[second as usize].ty = row;
        f.body().places[second as usize].ty = row;
    }
    let Terminator::Call {
        callee: Callee::Definition { path: called, .. },
        arguments,
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        panic!()
    };
    arguments.push(CallArgument {
        parameter: ParamId(1),
        value: operand(second, usage),
    });
    if matches!(extra, Extra::Borrowed { core: true }) {
        *called = DefPath::core("test.first");
    }
    let called = called.clone();
    owner_sig(&mut f, &[PassingMode::Take, passing]);
    let mut callee = f.signatures.remove(&path("callee")).unwrap();
    callee.definition = DefId::intern(&called);
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    let t = |depth| {
        ty(TypeKind::Parameter(TypeParameterRef::Bound {
            depth,
            parameter: TypeParamId(0),
        }))
    };
    let ty_kind = match extra {
        Extra::Borrowed { .. } => {
            let element = interner().intern(t(0));
            TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(element)].into(),
            }
        }
        Extra::Named { thunk } => {
            let element = interner().intern(t(0));
            f.signatures.insert(id("Tagged"), tagged(thunk));
            TypeKind::Named {
                definition: id("Tagged"),
                arguments: vec![GenericArgument::Type(element)].into(),
            }
        }
        Extra::Lent { pure } => {
            let element = interner().intern(t(1));
            let count = interner().intern(ty(TypeKind::Primitive(Primitive::U64)));
            let mut callback = fn_type(
                vec![Parameter {
                    name: None,
                    label: None,
                    passing: PassingMode::Borrow,
                    ty: element,
                }],
                count,
            );
            if !pure {
                callback.uses = Uses::Unrestricted;
            }
            TypeKind::Function(callback)
        }
    };
    let mut parameters = fun.parameters.to_vec();
    parameters.push(Parameter {
        name: None,
        label: None,
        passing,
        ty: interner().intern(ty(ty_kind)),
    });
    fun.parameters = parameters.into();
    f.signatures.insert(&called, callee);
    f
}

#[test]
fn a_borrowed_input_supplies_nothing_to_a_checked_parametric_body() {
    // A checked `first` cannot move a `T` out of `seen` or return a share of it.
    parametric_variant(Extra::Borrowed { core: false }).valid();
    // A bodyless core `first` might hand back a share of `seen`.
    assert_eq!(
        parametric_variant(Extra::Borrowed { core: true }).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "value modes"
        }]
    );
}

#[test]
fn a_nominal_part_naming_no_held_parameter_is_ignored() {
    // `label: Shared[String]` cannot hold or hand over a `T`.
    parametric_variant(Extra::Named { thunk: false }).valid();
    // `label: fn() -> T` can hand over a `T` the caller still holds.
    assert_eq!(
        parametric_variant(Extra::Named { thunk: true }).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "value modes"
        }]
    );
}

#[test]
fn a_lent_callback_without_authority_or_counted_outputs_supplies_nothing() {
    // `fn(view: T) -> u64 uses {}` has nowhere to keep a copy of the `T`.
    parametric_variant(Extra::Lent { pure: true }).valid();
    // With authority it might keep one beyond the call.
    assert_eq!(
        parametric_variant(Extra::Lent { pure: false }).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "value modes"
        }]
    );
}

/// `fn main(take xs: unique String, f: fn(take String) -> (unique String, S))
/// -> unique (String, String)` returning `apply[String, (unique String, S)](take xs, f)`,
/// where `fn apply[T, U](take value: T, f: fn(take T) -> U) -> U` and `S` is
/// `unique String` when `unique`.
fn tuple_result(unique: bool) -> Fixture {
    let mut f = Fixture::result_call(TypeKind::Primitive(Primitive::String), true);
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let mut unique_row = interner().get(string).unwrap().clone();
    unique_row.modes.uniqueness = Uniqueness::Unique;
    let unique_string = f.definition.types.push(unique_row);
    let second = if unique { unique_string } else { string };
    let parts = f
        .definition
        .types
        .push(ty(TypeKind::Tuple(vec![unique_string, second].into())));
    let mut result = ty(TypeKind::Tuple(vec![string, string].into()));
    result.modes.uniqueness = Uniqueness::Unique;
    let result = f.definition.types.push(result);
    f.body().result = result;
    f.body().locals[0].ty = result;
    f.body().places[0].ty = result;
    let Terminator::Call {
        returns: Some(CallReturn { destination, .. }),
        ..
    } = &f.body().blocks[0].terminator
    else {
        panic!()
    };
    let destination = destination.0 as usize;
    let local = f.body().places[destination].root.0 as usize;
    f.body().places[destination].ty = parts;
    f.body().locals[local].ty = parts;
    let xs = param(&mut f, TypeKind::Primitive(Primitive::String));
    f.body().locals[xs as usize].ty = unique_string;
    f.body().places[xs as usize].ty = unique_string;
    let callback = param(
        &mut f,
        TypeKind::Function(fn_type(
            vec![Parameter {
                name: None,
                label: None,
                passing: PassingMode::Take,
                ty: string,
            }],
            parts,
        )),
    );
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
    *generics = vec![GenericArgument::Type(string), GenericArgument::Type(parts)];
    arguments.push(CallArgument {
        parameter: ParamId(0),
        value: take(xs),
    });
    arguments.push(CallArgument {
        parameter: ParamId(1),
        value: operand(callback, UseKind::Borrow),
    });
    owner_sig(&mut f, &[PassingMode::Take, PassingMode::Borrow]);
    let mut callee = function_signature(
        "callee",
        vec![],
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(1))),
    );
    let SignatureKind::Function(fun) = &mut callee.kind else {
        panic!()
    };
    let parameter = |depth, ordinal| {
        ty(TypeKind::Parameter(TypeParameterRef::Bound {
            depth,
            parameter: TypeParamId(ordinal),
        }))
    };
    let value = interner().intern(parameter(0, 0));
    let received = interner().intern(parameter(1, 0));
    let returned = interner().intern(parameter(1, 1));
    let f_ty = interner().intern(ty(TypeKind::Function(fn_type(
        vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Take,
            ty: received,
        }],
        returned,
    ))));
    fun.parameters = vec![
        Parameter {
            name: None,
            label: None,
            passing: PassingMode::Take,
            ty: value,
        },
        Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: f_ty,
        },
    ]
    .into();
    fun.generics = ["T", "U"]
        .map(|name| GenericParameter::Type {
            name: name.into(),
            bounds: vec![].into(),
        })
        .to_vec()
        .into();
    f.signatures.insert(callee.definition.clone(), callee);
    f
}

#[test]
fn a_callback_tuple_of_unique_parts_is_a_unique_result() {
    // A tuple owns no block of its own, so unique parts make it unique.
    tuple_result(true).valid();
    // An aliased part leaves the result aliased: the unique return is forged.
    assert_eq!(
        tuple_result(false).kinds(),
        vec![ViolationKind::TypeMismatch {
            entity: "value modes"
        }]
    );
}

fn inferred_promise(f: &mut Fixture) {
    let summary = hirsel_types::UniqueSummary {
        nonretaining: Vec::new(),
        preserves_unique: vec![ParamId(0)],
        results: vec![],
    };
    f.signatures
        .get_mut(id("main"))
        .unwrap()
        .metadata
        .inferred_unique = summary.clone();
    f.definition.inferred_unique = summary;
}

#[test]
fn inferred_preservation_bits_get_the_declared_body_reproof() {
    let (mut good, _) = handback(false);
    finish(&mut good);
    inferred_promise(&mut good);
    good.valid();
    let mut bad = sharing(false);
    inferred_promise(&mut bad);
    handback_error(&bad, "preserved unique hand-back", 1);
}

#[test]
fn inferred_result_root_cannot_forge_an_ordinary_call_result() {
    let mut f = Fixture::result_call(TypeKind::Primitive(Primitive::String), true);
    owner_signature(&mut f, PassingMode::Take);
    let summary = hirsel_types::UniqueSummary {
        nonretaining: Vec::new(),
        preserves_unique: vec![],
        results: vec![hirsel_types::UniqueResultRow::Root],
    };
    f.signatures
        .get_mut(id("main"))
        .unwrap()
        .metadata
        .inferred_unique = summary.clone();
    f.definition.inferred_unique = summary;
    assert!(f.kinds().iter().any(|v| matches!(
        v,
        ViolationKind::TypeMismatch {
            entity: "private unique result proof"
        }
    )));
}
