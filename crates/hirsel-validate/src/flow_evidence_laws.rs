//! Stop-condition oracles for untrusted entry invariants, compiled only in tests.
//!
//! CompCert e276a3f backend/Kildall.v:24-36,82-114 motivates entry/edge
//! inequations. Its lattice theorem does not establish Hirsel's audit simulation.
//! These tests deliberately preserve the counterexample, not a new acceptance
//! path. In particular, do not turn audit into an ordinary may-bit to pass it.
use super::*;
use crate::support::{Fixture, block, copy, inst, interner, owner_sig, param, tabled};

fn with_model(f: &Fixture, run: impl FnOnce(&mut Checker<'_>, &Model, State)) {
    let declarations = typing::declaration_tables(
        interner(),
        &f.definition,
        &Reader::new(interner(), &f.signatures),
    )
    .unwrap();
    let scratch = scratch::Scratch::new(f.definition.bodies.len());
    let memo = TypeMemo::new();
    let reader = Reader::new(interner(), &f.signatures);
    let mut checker = Checker {
        definition: &f.definition,
        t: interner(),
        memo: &memo,
        body: &f.definition.bodies[0],
        signatures: &reader,
        declarations: &declarations,
        scratch: &scratch,
        contract: false,
        site: Site {
            body: BodyId(0),
            block: None,
            instruction: None,
        },
        errors: vec![],
        captured: BTreeMap::new(),
        returned: BTreeMap::new(),
        static_data: None,
        proof_errors: std::cell::RefCell::new(Vec::new()),
        unique_parts_cache: Default::default(),
    };
    checker.types();
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let model = Model::new(&mut checker);
    let mut initial = model.initial(
        checker.body,
        interner(),
        &checker.scratch.origin_types[0],
        &|ty| {
            checker
                .lifetime(&interner().row(ty).modes.relation)
                .is_global()
        },
        &|origin| checker.arena_place(origin),
        &[],
    );
    // Ordinary, capture-free initialization, including the guarantees flow_with
    // independently derives after Model::initial. No producer memo is reused.
    for (root, local) in checker.body.locals.iter().enumerate() {
        if checker.body.parameters.contains(&LocalId(root as u32))
            && checker
                .proof(
                    checker.t,
                    local.ty,
                    crate::proofs::Proof::Content,
                    checker.generic_declarations(),
                )
                .unwrap()
                == Sendability::Sendable
        {
            initial.must.set_fact(root, MustFacts::SENDABLE, true);
            initial.must.set_fact(root, MustFacts::GLOBAL, true);
        }
    }
    checker.initial_authority(&model, &mut initial);
    checker.initial_creation(&model, &mut initial);
    run(&mut checker, &model, initial);
    assert!(
        checker.proof_errors.borrow().is_empty(),
        "{:?}",
        checker.proof_errors.borrow()
    );
}

fn refresh(model: &Model, state: &mut State) {
    state.cleanup_roots = 0;
    state.live_temporaries = 0;
    state.cleanup_conflicts = 0;
    state.cleanup_loans = 0;
    state.cleanup_maybe_loans = 0;
    for root in 0..state.shared.len() {
        model.refresh_cleanup(state, root);
    }
    for loan in 0..state.loans.len() {
        model.refresh_cleanup_loan(state, loan);
    }
}

// Exhaustive destructuring makes a new State field a compile error here. This
// is equality for the fixed fixture domain below, NOT a general normalization
// of arbitrary prefix/default origin, tag or crossing representations.
fn normalized(state: &State, model: &Model) -> (State, [Vec<u32>; 5]) {
    let mut state = state.clone();
    refresh(model, &mut state);
    let State {
        envelopes,
        audit,
        cleanup_roots,
        live_temporaries,
        cleanup_conflicts,
        cleanup_loans,
        cleanup_maybe_loans,
        contract_readonly,
        cursor_views,
        cursor_contracts,
        origins,
        places,
        defused,
        must,
        ownership_conflicts,
        shared,
        nested_shared,
        borrow_only,
        borrowed_shares,
        current_unique,
        crossing_parts,
        unique_parts,
        tags,
        loans,
        content_loans,
        sources,
        before,
    } = state;
    let sets = model.cleanup_sets.borrow();
    let members = [
        cleanup_roots,
        live_temporaries,
        cleanup_conflicts,
        cleanup_loans,
        cleanup_maybe_loans,
    ]
    .map(|root| sets.members(root));
    let mut normalized = State {
        envelopes,
        audit,
        cleanup_roots: 0,
        live_temporaries: 0,
        cleanup_conflicts: 0,
        cleanup_loans: 0,
        cleanup_maybe_loans: 0,
        contract_readonly,
        cursor_views,
        cursor_contracts,
        origins,
        places,
        defused,
        must,
        ownership_conflicts,
        shared,
        nested_shared,
        borrow_only,
        borrowed_shares,
        current_unique,
        crossing_parts,
        unique_parts,
        tags,
        loans,
        content_loans,
        sources,
        before,
    };
    for i in 0..normalized.places.len() {
        if normalized.places[i] & LIVE == 0 {
            normalized.must.set(i, MustFacts(0));
            normalized.defused.set(i, 0);
        }
    }
    (normalized, members)
}

fn covers(candidate: &State, incoming: &State, model: &Model) -> bool {
    if candidate.audit != incoming.audit {
        return false;
    }
    let mut joined = candidate.clone();
    joined.join(incoming, model); // full join, never a fabricated delta base
    normalized(&joined, model) == normalized(candidate, model)
}

#[derive(Clone, Copy, Debug)]
struct Row {
    mask: u8,
    must: u8,
    defused: u8,
    conflict: bool,
}

// Independent small concrete alternatives: each present LIVE alternative
// constrains guaranteed facts; dead/moved alternatives cannot grant facts.
fn row_join(a: Row, b: Row) -> Row {
    let live: Vec<_> = [a, b].into_iter().filter(|r| r.mask & LIVE != 0).collect();
    let must = live.iter().fold(31, |facts, row| facts & row.must);
    Row {
        mask: a.mask | b.mask,
        must: if live.is_empty() { 0 } else { must },
        defused: live.iter().fold(0, |bits, row| bits | row.defused),
        conflict: a.conflict
            || b.conflict
            || live.len() == 2 && (a.must & MustFacts::OWNED != b.must & MustFacts::OWNED),
    }
}

fn install(state: &State, model: &Model, row: Row) -> State {
    let mut out = state.clone();
    out.places.set(1, row.mask);
    out.must.set(1, MustFacts(row.must));
    out.defused.set(1, row.defused);
    out.ownership_conflicts.set(1, row.conflict);
    refresh(model, &mut out);
    out
}

#[test]
fn coupled_rows_match_concrete_alternative_join_and_absorption() {
    let mut f = Fixture::unit();
    param(&mut f, TypeKind::Primitive(Primitive::String));
    owner_sig(&mut f, &[PassingMode::Take]);
    with_model(&f, |_, model, initial| {
        let mut rows = Vec::new();
        for mask in 1..8 {
            for must in [0, MustFacts::OWNED, MustFacts::AUTHORITY, 31] {
                for defused in [1, 2] {
                    for conflict in [false, true] {
                        rows.push(Row {
                            mask,
                            must,
                            defused,
                            conflict,
                        });
                    }
                }
            }
        }
        let mut pairs = 0;
        for &a in &rows {
            for &b in &rows {
                let candidate = install(&initial, model, a);
                let incoming = install(&initial, model, b);
                let expected = install(&initial, model, row_join(a, b));
                let mut joined = candidate.clone();
                joined.join(&incoming, model);
                assert!(
                    normalized(&joined, model) == normalized(&expected, model),
                    "{a:?}, {b:?}"
                );
                assert_eq!(
                    covers(&candidate, &incoming, model),
                    normalized(&candidate, model) == normalized(&expected, model)
                );
                pairs += 1;
            }
        }
        assert_eq!(pairs, 12544);
        // Exercise every guarantee bit, including freshness and sendability.
        for bit in [1, 2, 4, 8, 16] {
            let dead = install(
                &initial,
                model,
                Row {
                    mask: DEAD,
                    must: 31,
                    defused: 1,
                    conflict: false,
                },
            );
            let live = install(
                &initial,
                model,
                Row {
                    mask: LIVE,
                    must: bit,
                    defused: 1,
                    conflict: false,
                },
            );
            let mut joined = dead;
            joined.join(&live, model);
            assert_eq!(joined.must[1].0, bit);
        }
    });
}

#[test]
fn borrow_only_uses_shared_alternatives_instead_of_reversed_boolean_order() {
    let mut f = Fixture::unit();
    param(&mut f, TypeKind::Primitive(Primitive::String));
    owner_sig(&mut f, &[PassingMode::Borrow]);
    with_model(&f, |_, model, initial| {
        // Concrete alias classes: unique, shares confined to a borrow, escaping.
        let classes = [(false, false), (true, true), (true, false)];
        for (a, &(shared_a, only_a)) in classes.iter().enumerate() {
            for (b, &(shared_b, only_b)) in classes.iter().enumerate() {
                let mut left = initial.clone();
                left.shared.set(1, shared_a);
                left.borrow_only.set(1, only_a);
                let mut right = initial.clone();
                right.shared.set(1, shared_b);
                right.borrow_only.set(1, only_b);
                left.join(&right, model);
                assert_eq!(left.shared[1], a != 0 || b != 0);
                assert_eq!(left.borrow_only[1], (a == 1 || b == 1) && a != 2 && b != 2);
            }
        }
    });
}

// Smallest class-routing witness: switch b, then switch b again on the false
// arm. The true target has one instruction so it is NOT the empty sentinel.
fn repeated_switch() -> Fixture {
    let mut f = Fixture::unit();
    let b = param(&mut f, TypeKind::Primitive(Primitive::Bool));
    let switch = |yes, no| Terminator::Switch {
        value: copy(b),
        arms: vec![(1, BlockId(yes))],
        otherwise: BlockId(no),
    };
    let exit = f.body().blocks[0].clone();
    f.body().blocks = vec![
        block(vec![], switch(3, 1)),
        block(vec![], switch(2, 3)),
        block(exit.instructions.clone(), Terminator::Unreachable),
        exit,
    ];
    owner_sig(&mut f, &[PassingMode::Borrow]);
    f
}

#[test]
fn wider_tags_do_not_simulate_the_exact_audit_class() {
    let f = repeated_switch();
    let baseline = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(baseline.violations.is_empty(), "{baseline:?}");
    with_model(&f, |checker, model, initial| {
        let mut narrow = initial.clone();
        // Obtain the tag fact using V itself, not a forged Fact constructor.
        assert!(checker.tag_edge(&mut narrow, BlockId(0), BlockId(1)));
        assert!(!narrow.audit);
        let mut wide = initial;
        assert!(covers(&wide, &narrow, model));
        assert!(!covers(&narrow, &wide, model));
        assert!(checker.tag_edge(&mut narrow, BlockId(1), BlockId(2)));
        assert!(checker.tag_edge(&mut wide, BlockId(1), BlockId(2)));
        assert!(narrow.audit);
        assert!(!wide.audit);
        assert!(
            !covers(&wide, &narrow, model),
            "same-class simulation must fail"
        );
        // An independent Boolean execution cannot take false then true.
        let next = |block, b| match (block, b) {
            (0, false) => 1,
            (1, true) => 2,
            _ => 3,
        };
        let concrete_targets: Vec<_> = [false, true]
            .into_iter()
            .map(|b| next(next(0, b), b))
            .collect();
        assert_eq!(concrete_targets, vec![3, 3]);
        checker.transfer(model, &mut narrow, BlockId(2), true);
        assert!(checker.errors.is_empty());
        checker.transfer(model, &mut wide, BlockId(2), true);
        assert!(
            checker
                .errors
                .iter()
                .any(|v| v.kind == ViolationKind::ReachableUnreachable)
        );
    });
}

#[test]
fn empty_unreachable_sentinel_is_omitted_only_for_the_contradictory_edge() {
    let mut f = repeated_switch();
    f.body().blocks[2].instructions.clear();
    with_model(&f, |checker, model, initial| {
        let mut narrow = initial.clone();
        assert!(checker.tag_edge(&mut narrow, BlockId(0), BlockId(1)));
        assert!(covers(&initial, &narrow, model));
        assert!(!checker.tag_edge(&mut narrow, BlockId(1), BlockId(2)));
        let mut wide = initial;
        assert!(checker.tag_edge(&mut wide, BlockId(1), BlockId(2)));
        assert!(!wide.audit);
    });
}

#[test]
fn reporting_keeps_semantic_state_for_the_minimized_routing_fixture() {
    let f = repeated_switch();
    with_model(&f, |checker, model, initial| {
        for block in 0..f.definition.bodies[0].blocks.len() {
            for audit in [false, true] {
                let mut quiet = initial.clone();
                quiet.audit = audit;
                let mut reporting = quiet.clone();
                checker.transfer(model, &mut quiet, BlockId(block as u32), false);
                checker.transfer(model, &mut reporting, BlockId(block as u32), true);
                assert!(normalized(&quiet, model) == normalized(&reporting, model));
            }
        }
    });
}

fn late_binding(early: bool) -> Fixture {
    let mut f = Fixture::unit();
    let p = param(&mut f, TypeKind::Primitive(Primitive::Bool));
    let mut access = f.body().places[p as usize].clone();
    access.binding = Some(Origin {
        root: OriginRoot::Local(crate::support::lexical(), hirsel_types::LocalId(7)),
        fields: vec![].into(),
    });
    let bound = PlaceId(f.body().places.len() as u32);
    f.body().places.push(access);
    let bind = inst(
        InstructionKind::BindOrigin {
            source: PlaceId(p),
            place: bound,
            kind: OriginBindingKind::Alias,
        },
        0,
    );
    let exit = f.body().blocks[0].clone();
    let mut introduction = block(vec![], Terminator::Goto(BlockId(3)));
    let mut entry = block(vec![], Terminator::Goto(BlockId(1)));
    if early {
        entry.instructions.push(bind.clone());
    } else {
        introduction.instructions.push(bind);
    }
    f.body().blocks = vec![
        entry,
        block(
            vec![],
            Terminator::Branch {
                condition: Operand {
                    place: bound,
                    usage: UseKind::Copy,
                },
                yes: BlockId(2),
                no: BlockId(3),
            },
        ),
        introduction,
        exit,
    ];
    owner_sig(&mut f, &[PassingMode::Borrow]);
    f
}

#[test]
fn union_of_locations_can_invent_the_nonempty_binding_precondition() {
    let good = late_binding(true);
    let good_verdict = validate(
        &tabled(&good.definition),
        interner(),
        &good.signatures,
        &mut vec![],
    );
    assert!(good_verdict.violations.is_empty(), "{good_verdict:?}");
    let bad = late_binding(false);
    let bad_verdict = validate(
        &tabled(&bad.definition),
        interner(),
        &bad.signatures,
        &mut vec![],
    );
    assert_eq!(
        bad_verdict.violations,
        vec![Violation {
            body: Some(BodyId(0)),
            block: Some(BlockId(1)),
            instruction: None,
            kind: ViolationKind::InvalidOrigin(PlaceId(2)),
        }]
    );
    // Independent concrete binding machine: no V state/join is consulted.
    // Record the first invalid access even if execution is continued to b2.
    for value in [false, true] {
        let mut bound = false;
        let mut first_invalid = None;
        let mut block = 0;
        while block != 3 {
            block = match block {
                0 => 1,
                1 => {
                    if !bound {
                        first_invalid = Some(1);
                    }
                    if value { 2 } else { 3 }
                }
                2 => {
                    bound = true;
                    3
                }
                _ => unreachable!(),
            };
        }
        assert_eq!(first_invalid, Some(1));
        assert_eq!(bound, value);
    }
    with_model(&bad, |checker, model, initial| {
        let mut claimed = initial.clone();
        // Obtain the extra location/binding facts from a real V transfer in
        // this SAME immutable TIR. There is no stale-artifact shortcut here.
        // Block 2 creates the binding, but it is used too soon in block 1.
        checker.transfer(model, &mut claimed, BlockId(2), true);
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        assert!(covers(&claimed, &initial, model));
        assert!(!model.binding_valid(&initial, &checker.body.places[2], checker.body));
        assert!(model.binding_valid(&claimed, &checker.body.places[2], checker.body));
        // All four supplied entries use this one wider state. Check the
        // independently enumerated successors; there are no backedges or
        // audit transitions in this witness. This deliberately demonstrates
        // failure of the proposed obligations, not a reusable checker.
        assert!(super::super::flow_origins::loop_expirations(checker.body).is_empty());
        let mut edges = 0;
        for block in 0..checker.body.blocks.len() {
            let block = BlockId(block as u32);
            let mut outgoing = claimed.clone();
            checker.transfer(model, &mut outgoing, block, true);
            let targets =
                checker.flow_successors(&checker.body.blocks[block.0 as usize].terminator);
            for target in targets {
                let mut edge = outgoing.clone();
                assert!(checker.tag_edge(&mut edge, block, target));
                assert!(covers(&claimed, &edge, model));
                edges += 1;
            }
        }
        assert_eq!(edges, 4);
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        assert_eq!(checker.returned.len(), 1);
        let before = bad_verdict.clone();
        let mut first_reads = vec![];
        let mut second_reads = vec![];
        let first = validate(
            &tabled(&bad.definition),
            interner(),
            &bad.signatures,
            &mut first_reads,
        );
        let second = validate(
            &tabled(&bad.definition),
            interner(),
            &bad.signatures,
            &mut second_reads,
        );
        assert_eq!(before, first);
        assert_eq!(first, second);
        assert_eq!(first_reads, second_reads);
    });
}

#[test]
fn counterexamples_survive_session_salts() {
    let run = || {
        coupled_rows_match_concrete_alternative_join_and_absorption();
        borrow_only_uses_shared_alternatives_instead_of_reversed_boolean_order();
        union_of_locations_can_invent_the_nonempty_binding_precondition();
        wider_tags_do_not_simulate_the_exact_audit_class();
        empty_unreachable_sentinel_is_omitted_only_for_the_contradictory_edge();
        reporting_keeps_semantic_state_for_the_minimized_routing_fixture();
        normal_flow_cannot_hide_incomplete_instruction_failure_cleanup();
        derived_cleanup_is_rebuilt_from_rows_instead_of_trusting_cached_roots();
        "12544 row pairs; 9 sharing pairs; 5 guarantee bits; 4 checked unsafe edges; audit/sentinel/reporting/failure-cleanup witnesses\n".to_owned()
    };
    let Some(runs) = hirsel_intern::under_salts(
        "flow::evidence_laws::counterexamples_survive_session_salts",
        &[1, 2, 0x5eed],
        run,
    ) else {
        return;
    };
    assert_eq!(runs.len(), 3, "all requested subprocess salts executed");
    let ordinary = run();
    assert!(runs.iter().all(|row| row == &ordinary));
    eprintln!("evidence laws: salts 1, 2, 0x5eed and unsalted executed; {ordinary}");
}

/// Test-shape port of rustc_mir_dataflow initialized.rs:255-303 and
/// framework/tests.rs:1-68 at 19833af (MIT, Rust Project Contributors):
/// observe before/after effects separately. Hirsel owners, not Rust move bits,
/// determine the expected state. Disconnected-CFG rejection is covered in it.
#[test]
fn owner_histories_keep_shares_until_a_fresh_whole_replacement() {
    use crate::support::{Table, drop, operand, take};
    let mut f = Fixture::unit();
    let root = param(&mut f, TypeKind::Primitive(Primitive::String));
    let mut row = interner()
        .get(f.body().places[root as usize].ty)
        .unwrap()
        .clone();
    row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
    let unique = f.definition.types.push(row);
    f.body().locals[root as usize].ty = unique;
    f.body().places[root as usize].ty = unique;
    let alias = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    let cleanup = f.cleanup(vec![drop(root)]);
    f.assign(alias, Rvalue::Use(copy(root)), cleanup);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(alias),
            if_live: false,
        },
        cleanup,
    ));
    let share = f.body().blocks[0].instructions.split_off(1);
    f.assign(root, Rvalue::Constant(Constant::String("fresh".into())), 0);
    let fresh = f.body().blocks[0].instructions.pop().unwrap();
    let condition = param(&mut f, TypeKind::Primitive(Primitive::U64));
    f.body().blocks[0].terminator = Terminator::Switch {
        value: copy(condition),
        arms: vec![(1, BlockId(1)), (2, BlockId(2))],
        otherwise: BlockId(3),
    };
    f.body().blocks.extend([
        block(share, Terminator::Goto(BlockId(4))),
        block(
            vec![
                inst(
                    InstructionKind::Drop {
                        place: PlaceId(root),
                        if_live: false,
                    },
                    0,
                ),
                fresh,
            ],
            Terminator::Goto(BlockId(4)),
        ),
        block(
            vec![
                inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(0),
                        place: PlaceId(root),
                        mutable: false,
                        if_inactive: false,
                    },
                    cleanup,
                ),
                inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    },
                    cleanup,
                ),
            ],
            Terminator::Goto(BlockId(4)),
        ),
    ]);
    f.body()
        .blocks
        .push(block(vec![], crate::support::ret(0, cleanup)));
    owner_sig(&mut f, &[PassingMode::Take, PassingMode::Borrow]);
    f.valid();
    with_model(&f, |checker, model, initial| {
        // Each graph has possible concrete owners. Fresh installs a new
        // graph; EndLoan leaves its owners alone. No production join supplies
        // an expectation. Enumerate 1+3+9+27 histories and all 1600 pairs.
        let mut histories = vec![vec![]];
        for length in 1..=3 {
            for code in 0..3usize.pow(length) {
                let mut code = code;
                let mut history = vec![];
                for _ in 0..length {
                    history.push(code % 3 + 1);
                    code /= 3;
                }
                histories.push(history);
            }
        }
        let mut outcomes = vec![];
        for history in &histories {
            let mut owners = BTreeSet::from([0usize]);
            let mut state = initial.clone();
            for &event in history {
                assert_eq!(
                    checker.operand_unique(&state, &take(root)),
                    owners.len() == 1,
                    "before {history:?}"
                );
                match event {
                    1 => {
                        owners.insert(1);
                    }
                    2 => {
                        owners = BTreeSet::from([0]);
                    }
                    3 => (),
                    _ => unreachable!(),
                }
                checker.transfer(model, &mut state, BlockId(event as u32), true);
                assert_eq!(
                    checker.operand_unique(&state, &operand(root, UseKind::Mut)),
                    owners.len() == 1,
                    "after {history:?}"
                );
            }
            outcomes.push((state, owners));
        }
        for (a, owners_a) in &outcomes {
            for (b, owners_b) in &outcomes {
                let alternatives = [owners_a, owners_b];
                let expected = alternatives.iter().all(|owners| owners.len() == 1);
                let mut joined = a.clone();
                joined.join(b, model);
                assert_eq!(checker.operand_unique(&joined, &take(root)), expected);
                assert_eq!(joined.shared[root as usize], !expected);
            }
        }
        assert_eq!(histories.len(), 40);
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    });
}

#[test]
fn crossing_guarantees_and_bounds_have_separate_must_and_may_orders() {
    let mut f = Fixture::unit();
    param(&mut f, TypeKind::Primitive(Primitive::String));
    owner_sig(&mut f, &[PassingMode::Take]);
    with_model(&f, |_, model, initial| {
        // Missing positive evidence cannot certify the other path. Explicit
        // negative evidence survives even when the other path has no entry.
        for a in [None, Some(false), Some(true)] {
            for b in [None, Some(false), Some(true)] {
                let mut left = initial.clone();
                let mut right = initial.clone();
                let path = [crossing::Part::Field(0)];
                if let Some(a) = a {
                    left.crossing_parts[1].set(&path, a);
                }
                if let Some(b) = b {
                    right.crossing_parts[1].set(&path, b);
                }
                left.sources[1].insert(2);
                right.sources[1].insert(3);
                left.current_unique.set(1, a == Some(true));
                right.current_unique.set(1, b == Some(true));
                left.join(&right, model);
                let expected = if a == Some(false) || b == Some(false) {
                    Some(false)
                } else if a == Some(true) && b == Some(true) {
                    Some(true)
                } else {
                    None
                };
                assert_eq!(left.crossing_parts[1].get(&path), expected);
                assert_eq!(left.current_unique[1], a == Some(true) && b == Some(true));
                assert_eq!(left.sources[1], BTreeSet::from([2, 3]));
            }
        }
    });
}

fn normal_flow_cannot_hide_incomplete_instruction_failure_cleanup() {
    // OpenJDK a0785e626 VerifyStackForExceptionHandlers.java:25-48,83-111:
    // preserve normal success while breaking only the exceptional obligation.
    // Hirsel uses live owned releases, not JVM operand-stack capacity.
    let mut f = Fixture::string();
    let condition = param(&mut f, TypeKind::Primitive(Primitive::Bool));
    owner_sig(&mut f, &[PassingMode::Borrow]);
    let Terminator::Return { cleanup, .. } = f.body().blocks[0].terminator else {
        unreachable!()
    };
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Check {
            condition: copy(condition),
            rule: hirsel_diag::RuleId::of("FACT-1"),
        },
        cleanup.0,
    ));
    let accepted = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(accepted.violations.is_empty(), "{accepted:?}");
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(0);
    let rejected = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        rejected.violations.iter().any(|v| matches!(
            v.kind,
            ViolationKind::CleanupMismatch { .. } | ViolationKind::LeakedSlot(_)
        )),
        "{rejected:?}"
    );
    assert!(
        rejected
            .violations
            .iter()
            .all(|v| v.block == Some(BlockId(0)) && v.instruction == Some(2)),
        "{rejected:?}"
    );
}

fn derived_cleanup_is_rebuilt_from_rows_instead_of_trusting_cached_roots() {
    let mut f = Fixture::unit();
    param(&mut f, TypeKind::Primitive(Primitive::String));
    owner_sig(&mut f, &[PassingMode::Borrow]);
    with_model(&f, |_, model, initial| {
        let dead = Row {
            mask: DEAD,
            must: 0,
            defused: 1,
            conflict: true,
        };
        let live = Row {
            mask: LIVE,
            must: 0,
            defused: 1,
            conflict: false,
        };
        let mut joined = install(&initial, model, dead);
        joined.join(&install(&initial, model, live), model);
        // V's solver has additional reachability invariants. An arbitrary
        // locally shaped raw row cannot inherit its cleanup-cache shortcut.
        let cached = model
            .cleanup_sets
            .borrow()
            .members(joined.cleanup_conflicts);
        let (_, rebuilt) = normalized(&joined, model);
        assert!(cached.is_empty());
        assert_eq!(rebuilt[2], vec![1]);
    });
}
