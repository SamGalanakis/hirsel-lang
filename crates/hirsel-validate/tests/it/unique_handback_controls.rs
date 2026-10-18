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
