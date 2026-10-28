//! The IR checker against hand-built TIR. The child modules hold hand-built controls
//! for individual rules.
use crate::support::*;
use hirsel_diag::RuleId;
use hirsel_span::Span;
use hirsel_syntax::{BinaryOp, ContentHash, DefId, DefPath, ItemPath, PackageId, PassingMode};
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::*;
use std::collections::BTreeMap;

#[path = "box_read_controls.rs"]
mod box_read_controls;
#[path = "cleanup_nesting_controls.rs"]
mod cleanup_nesting_controls;
#[path = "composed_relation_controls.rs"]
mod composed_relation_controls;
#[path = "declaration_controls.rs"]
mod declaration_controls;
#[path = "defuse_controls.rs"]
mod defuse_controls;
#[path = "field_loan_controls.rs"]
mod field_loan_controls;
#[path = "field_replace_controls.rs"]
mod field_replace_controls;
#[path = "gap_controls.rs"]
mod gap_controls;
#[path = "last_use_controls.rs"]
mod last_use_controls;
#[path = "protocol_controls.rs"]
mod protocol_controls;
#[path = "state_controls.rs"]
mod state_controls;
#[path = "typing_gap_controls.rs"]
mod typing_gap_controls;
#[path = "unique_handback_controls.rs"]
mod unique_handback_controls;
#[path = "value_controls.rs"]
mod value_controls;
#[path = "wave1_controls.rs"]
mod wave1_controls;

#[test]
fn branch_and_check_require_booleans() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(p, Rvalue::Constant(Constant::Bool(true)), 0);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(p),
        yes: BlockId(1),
        no: BlockId(1),
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.valid();
    if let Terminator::Branch { condition, .. } = &mut f.body().blocks[0].terminator {
        *condition = copy(0);
    }
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "branch condition",
    }]);
    let mut f = Fixture::unit();
    f.emit(InstructionKind::Check {
        condition: copy(0),
        rule: const { RuleId::of("FACT-1") },
    });
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "check condition",
    }]);
}
#[test]
fn binary_types_and_lowered_operators() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    f.assign(p, Rvalue::Constant(Constant::Integer(1)), 0);
    let result = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    f.assign(
        result,
        Rvalue::Binary {
            op: BinaryOp::Add,
            left: copy(p),
            right: copy(p),
        },
        0,
    );
    f.valid();
    f.body().rvalues[2] = Rvalue::Binary {
        op: BinaryOp::Add,
        left: copy(p),
        right: copy(0),
    };
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "binary operands",
    }]);
    f.body().rvalues[2] = Rvalue::Binary {
        op: BinaryOp::And,
        left: copy(p),
        right: copy(p),
    };
    f.broken(vec![ViolationKind::InvalidOperator]);
}
#[test]
fn unary_types_snapshot_and_shape() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(p, Rvalue::Constant(Constant::Bool(true)), 0);
    let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(
        result,
        Rvalue::Unary {
            op: hirsel_syntax::UnaryOp::Not,
            value: copy(p),
        },
        0,
    );
    f.valid();
    f.body().rvalues[2] = Rvalue::Unary {
        op: hirsel_syntax::UnaryOp::Take,
        value: copy(p),
    };
    f.broken(vec![ViolationKind::InvalidOperator]);
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.emit(InstructionKind::Snapshot {
        destination: PlaceId(p),
        value: copy(0),
    });
    f.valid();
    let bool_ty = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bool)));
    f.body().places[p as usize].ty = bool_ty;
    f.body().locals[p as usize].ty = bool_ty;
    f.broken(vec![ViolationKind::TypeMismatch { entity: "snapshot" }]);
    let mut f = Fixture::unit();
    let unit_ty = f.body().result;
    let shape = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::TypeShape,
            arguments: vec![GenericArgument::Type(unit_ty)].into(),
        },
        Storage::Temporary,
    );
    f.assign(shape, Rvalue::Shape(unit_ty), 0);
    f.valid();
    f.body().rvalues[1] = Rvalue::Shape(f.body().places[shape as usize].ty);
    f.broken(vec![ViolationKind::TypeMismatch { entity: "shape" }]);
}
#[test]
fn tuple_constructor_arity_and_field_types() {
    let mut f = Fixture::unit();
    let t = f.body().result;
    let tuple = f.slot(TypeKind::Tuple(vec![t].into()), Storage::Local);
    f.assign(
        tuple,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: copy(0),
            }],
            destination: Destination::Inferred,
        },
        0,
    );
    f.valid();
    if let Rvalue::Construct { fields, .. } = &mut f.body().rvalues[1] {
        fields.clear();
    }
    f.broken(vec![ViolationKind::Arity {
        entity: "constructor fields",
        expected: 1,
        actual: 0,
    }]);
}
fn string_field_constructor(actual: Type, expected: Type) -> Fixture {
    let mut f = Fixture::string();
    let actual = f.definition.types.push(actual);
    f.body().locals[1].ty = actual;
    f.body().places[1].ty = actual;
    let expected = f.definition.types.push(expected);
    let tuple = f.slot(TypeKind::Tuple(vec![expected].into()), Storage::Temporary);
    f.assign(
        tuple,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(1),
            }],
            destination: Destination::Inferred,
        },
        1,
    );
    f.body().result = f.body().places[tuple as usize].ty;
    f.body().blocks[0].terminator = ret(tuple, 0);
    f
}
#[test]
fn constructor_fields_preserve_required_evidence_and_exact_cleanup() {
    for mutation in 0..7 {
        let mut actual = ty(TypeKind::Primitive(Primitive::String));
        let mut expected = actual.clone();
        match mutation {
            0 => expected.modes.uniqueness = Uniqueness::Unique,
            1 => {
                expected.sendability_requirement = hirsel_types::SendabilityRequirement::Sendable;
                expected.modes.sendability = Sendability::Sendable;
            }
            2 => actual.modes.relation = Relation::environment(),
            3 => actual.modes.multiplicity = Multiplicity::Once,
            4 => actual.drop_allocation = DropAllocation::MayAllocate,
            5 => expected.drop_allocation = DropAllocation::MayAllocate,
            6 => {
                actual.modes.relation = actual
                    .modes
                    .relation
                    .union(&Relation::new([Reach::Region(hirsel_types::RegionId(1))]))
            }
            _ => unreachable!(),
        }
        let mut fixture = string_field_constructor(actual, expected);
        if mutation <= 1 {
            // A fresh constant is a unique, sendable receipt (MODE-2, #80)...
            fixture.valid();
            // ...and an actual counted share loses that proof on both holders.
            let alias = fixture.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
            fixture.assign(alias, Rvalue::Use(copy(1)), 0);
            let share = fixture.body().blocks[0].instructions.pop().unwrap();
            fixture.body().blocks[0].instructions.insert(2, share);
            fixture.emit(InstructionKind::Drop {
                place: PlaceId(alias),
                if_live: false,
            });
        }
        assert!(
            fixture.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "constructor field",
            }),
            "{mutation}: {:?}",
            fixture.kinds()
        );
    }
}
#[test]
fn constructor_fields_weaken_taken_parts_but_keep_copied_contracts_invariant() {
    let mut f = Fixture::string();
    let mut unique = ty(TypeKind::Primitive(Primitive::String));
    unique.modes.uniqueness = Uniqueness::Unique;
    let inner = f.definition.types.push(unique);
    f.body().locals[1].ty = inner;
    f.body().places[1].ty = inner;
    let source = f.slot(TypeKind::Tuple(vec![inner].into()), Storage::Temporary);
    f.assign(
        source,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(1),
            }],
            destination: Destination::Inferred,
        },
        0,
    );
    let input_cleanup = f.cleanup(vec![CleanupAction::Drop {
        place: PlaceId(1),
        if_live: true,
    }]);
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(input_cleanup);
    let aliased = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let expected = f
        .definition
        .types
        .push(ty(TypeKind::Tuple(vec![aliased].into())));
    let result = f.slot(TypeKind::Tuple(vec![expected].into()), Storage::Temporary);
    f.assign(
        result,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(source),
            }],
            destination: Destination::Inferred,
        },
        0,
    );
    let source_cleanup = f.cleanup(vec![CleanupAction::Drop {
        place: PlaceId(source),
        if_live: true,
    }]);
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(source_cleanup);
    f.body().result = f.body().places[result as usize].ty;
    let cleanup = f.cleanup(vec![CleanupAction::Drop {
        place: PlaceId(source),
        if_live: true,
    }]);
    f.body().blocks[0].terminator = ret(result, cleanup);
    // Owner198789: a whole taken part may weaken its nested evidence. A copy
    // leaves the stronger source holder live and keeps the nested contract.
    f.valid();
    let Rvalue::Construct { fields, .. } = f.body().rvalues.last_mut().unwrap() else {
        panic!()
    };
    fields[0].value = copy(source);
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "constructor field",
    }]);
}
#[test]
fn projected_field_types_and_partial_move_state() {
    let mut f = Fixture::unit();
    let t = f.body().result;
    let tuple = f.slot(TypeKind::Tuple(vec![t, t].into()), Storage::Local);
    f.assign(
        tuple,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![
                ConstructField {
                    field: 0,
                    value: copy(0),
                },
                ConstructField {
                    field: 1,
                    value: copy(0),
                },
            ],
            destination: Destination::Inferred,
        },
        0,
    );
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(tuple),
        projection: vec![Projection::Field(0)].into(),
        ty: t,
    });
    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.assign(destination, Rvalue::Use(take(field)), 0);
    f.body().blocks[0].terminator = ret(destination, 0);
    f.valid();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(field),
        cleanup: CleanupId(0),
    };
    f.broken(vec![ViolationKind::UseAfterMove(PlaceId(field))]);
    f.body().places[field as usize].projection = vec![Projection::Field(3)].into();
    f.broken(vec![ViolationKind::InvalidProjection(PlaceId(field))]);
}
#[test]
fn exhaustive_variant_match_accepts_an_impossible_default() {
    let mut f = Fixture::unit();
    let p = f.slot(
        TypeKind::Variant(
            vec![
                Variant {
                    name: "A".into(),
                    fields: vec![].into(),
                },
                Variant {
                    name: "B".into(),
                    fields: vec![].into(),
                },
            ]
            .into(),
        ),
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(p));
    owner_signature(&mut f, PassingMode::Borrow);
    f.body().blocks[0].terminator = Terminator::Switch {
        value: copy(p),
        arms: vec![(0, BlockId(1)), (1, BlockId(1))],
        otherwise: BlockId(2),
    };
    f.body().blocks.extend([
        block(vec![], ret(0, 0)),
        block(vec![], Terminator::Unreachable),
    ]);
    f.valid();
    if let Terminator::Switch { arms, .. } = &mut f.body().blocks[0].terminator {
        arms.pop();
    }
    f.broken(vec![ViolationKind::MatchCoverage]);
}
#[test]
fn explicit_default_covers_remaining_variants() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters.push(LocalId(p));
    owner_signature(&mut f, PassingMode::Borrow);
    f.body().blocks[0].terminator = Terminator::Switch {
        value: copy(p),
        arms: vec![(0, BlockId(1))],
        otherwise: BlockId(2),
    };
    f.body()
        .blocks
        .extend([block(vec![], ret(0, 0)), block(vec![], ret(0, 0))]);
    f.valid();
    if let Terminator::Switch { arms, .. } = &mut f.body().blocks[0].terminator {
        arms[0].0 = 2;
    }
    f.broken(vec![ViolationKind::MatchCoverage]);
}
#[test]
fn conditional_cleanup_matches_maybe_initialized_slots() {
    let mut f = Fixture::unit();
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
    let s = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    let r = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Constant(Constant::String("branch".into())));
    let cleanup = f.cleanup(vec![drop(s)]);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(c),
        yes: BlockId(1),
        no: BlockId(2),
    };
    f.body().blocks.extend([
        block(
            vec![inst(
                InstructionKind::Assign {
                    destination: PlaceId(s),
                    value: r,
                },
                0,
            )],
            Terminator::Goto(BlockId(3)),
        ),
        block(vec![], Terminator::Goto(BlockId(3))),
        block(vec![], ret(0, cleanup)),
    ]);
    f.valid();
    edit_cleanup(f.body(), cleanup, |actions| {
        if let CleanupAction::Drop { if_live, .. } = &mut actions[0] {
            *if_live = false;
        }
    });
    f.broken(vec![ViolationKind::ReadBeforeInit(PlaceId(s))]);
}
#[test]
fn moves_at_a_cfg_join_are_not_definitely_live() {
    let mut f = Fixture::unit();
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
    let r = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(take(0)));
    let dest = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(c),
        yes: BlockId(1),
        no: BlockId(2),
    };
    f.body().blocks.extend([
        block(vec![], Terminator::Goto(BlockId(3))),
        block(vec![], Terminator::Goto(BlockId(3))),
        block(vec![], ret(0, 0)),
    ]);
    // Retain the unused rvalue through a valid move in the success-only branch.
    f.body().blocks[1].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(dest),
            value: r,
        },
        0,
    ));
    f.body().blocks[3].terminator = Terminator::Return {
        value: copy(c),
        cleanup: CleanupId(0),
    };
    f.body().result = f.body().places[c as usize].ty;
    f.valid();
    f.body().blocks[3].instructions.push(inst(
        InstructionKind::Snapshot {
            destination: PlaceId(dest),
            value: copy(0),
        },
        0,
    ));
    f.broken(vec![
        ViolationKind::UseAfterMove(PlaceId(0)),
        ViolationKind::WriteToLive(PlaceId(dest)),
    ]);
}
#[test]
fn run_cleanup_releases_slots_before_continuing() {
    let mut f = Fixture::string();
    f.emit(InstructionKind::RunCleanup(CleanupId(1)));
    f.body().blocks[0].terminator = ret(0, 0);
    f.valid();
    let dest = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(dest, Rvalue::Use(copy(1)), 0);
    let cleanup = f.cleanup(vec![drop(dest)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.broken(vec![ViolationKind::ReadBeforeInit(PlaceId(1))]);
}
#[test]
fn borrowed_temporaries_retain_their_source_loan() {
    let mut f = Fixture::loan();
    let alias = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let second = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let source = f.through_loan(1, 0);
    f.assign(alias, Rvalue::Use(operand(source, UseKind::Borrow)), 0);
    f.body().blocks[0].instructions.swap(3, 4);
    f.assign(second, Rvalue::Use(operand(alias, UseKind::Borrow)), 0);
    f.body().blocks[0].instructions.swap(4, 5);
    f.valid();
    // A derived borrow is invalid once its originating loan has ended.
    f.body().blocks[0].instructions.swap(4, 5);
    f.broken(vec![
        ViolationKind::ExpiredLoan {
            loan: LoanId(0),
            place: PlaceId(alias),
        },
        ViolationKind::MissingLoan(PlaceId(alias)),
    ]);
}

#[test]
fn constructor_failure_covers_pre_extraction_owned_inputs() {
    let mut f = Fixture::string();
    f.body().locals[1].storage = Storage::Temporary;
    let string_ty = f.body().places[1].ty;
    let boxed = f.slot(TypeKind::Box(string_ty), Storage::Temporary);
    f.assign(
        boxed,
        Rvalue::Construct {
            constructor: Constructor::Box,
            fields: vec![ConstructField {
                field: 0,
                value: take(1),
            }],
            destination: Destination::Inferred,
        },
        1,
    );
    let cleanup = f.cleanup(vec![drop(boxed)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.valid();
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(0);
    f.broken(vec![ViolationKind::CleanupMismatch {
        cleanup: CleanupId(0),
        expected: vec![LocalId(1)],
        actual: vec![],
    }]);
}

#[test]
fn noncopyable_operands_cannot_be_copied() {
    let mut f = Fixture::unit();
    let unit = f.body().result;
    let task = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::Task,
            arguments: vec![GenericArgument::Type(unit)].into(),
        },
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(task));
    let dest = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::Task,
            arguments: vec![GenericArgument::Type(unit)].into(),
        },
        Storage::Temporary,
    );
    f.assign(dest, Rvalue::Use(take(task)), 0);
    let c = f.cleanup(vec![drop(dest)]);
    f.body().blocks[0].terminator = ret(0, c);
    owner_signature(&mut f, PassingMode::Take);
    f.valid();
    f.body().rvalues[1] = Rvalue::Use(copy(task));
    edit_cleanup(&mut f.body(), c, |actions| {
        actions.push(drop(task));
    });
    f.broken(vec![ViolationKind::InvalidUse(PlaceId(task))]);
}

#[test]
fn sequence_indices_alias_for_loans_and_moves() {
    let mut f = Fixture::unit();
    let t = f.body().result;
    let sequence = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(t),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        },
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(sequence));
    let a = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
    let b = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
    f.assign(a, Rvalue::Constant(Constant::Integer(0)), 0);
    f.assign(b, Rvalue::Constant(Constant::Integer(1)), 0);
    let first = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(sequence),
        projection: vec![Projection::Index(LocalId(a))].into(),
        ty: t,
    });
    let second = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(sequence),
        projection: vec![Projection::Index(LocalId(b))].into(),
        ty: t,
    });
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(first),
        mutable: false,
        if_inactive: false,
    });
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(second),
        mutable: true,
        if_inactive: false,
    });
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(1),
        if_active: false,
    });
    let c = f.cleanup(vec![drop(sequence)]);
    f.body().blocks[0].terminator = ret(0, c);
    owner_signature(&mut f, PassingMode::Take);
    f.valid();
    f.body().blocks[0].instructions.swap(4, 5);
    f.broken(vec![ViolationKind::LoanConflict {
        loan: LoanId(0),
        place: PlaceId(second),
    }]);
}

/// All elements of a sequence are one place, but distinct fields below it are
/// not (OWN-3): a write loan on `xs[j].1` may begin while a read loan
/// on `xs[i].0` is live, and one on `xs[j].0` may not.
#[test]
fn distinct_fields_below_sequence_elements_do_not_overlap() {
    let mut f = Fixture::unit();
    let t = f.body().result;
    let element = f
        .definition
        .types
        .push(ty(TypeKind::Tuple(vec![t, t].into())));
    let sequence = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(element),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        },
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(sequence));
    let a = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
    let b = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
    f.assign(a, Rvalue::Constant(Constant::Integer(0)), 0);
    f.assign(b, Rvalue::Constant(Constant::Integer(1)), 0);
    let field = |f: &mut Fixture, index: u32, field: u32| {
        let place = f.body().places.len() as u32;
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(sequence),
            projection: vec![Projection::Index(LocalId(index)), Projection::Field(field)].into(),
            ty: t,
        });
        place
    };
    let read = field(&mut f, a, 0);
    let sibling = field(&mut f, b, 1);
    let same = field(&mut f, b, 0);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(read),
        mutable: false,
        if_inactive: false,
    });
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(sibling),
        mutable: true,
        if_inactive: false,
    });
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(1),
        if_active: false,
    });
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    let c = f.cleanup(vec![drop(sequence)]);
    f.body().blocks[0].terminator = ret(0, c);
    owner_signature(&mut f, PassingMode::Take);
    f.valid();
    for instruction in &mut f.body().blocks[0].instructions {
        if let InstructionKind::BeginLoan {
            id: LoanId(1),
            place,
            ..
        } = &mut instruction.kind
        {
            *place = PlaceId(same);
        }
    }
    f.broken(vec![ViolationKind::LoanConflict {
        loan: LoanId(0),
        place: PlaceId(same),
    }]);
}

#[test]
fn cleanup_and_loan_states_are_checked_across_branches() {
    let mut f = Fixture::string();
    let c = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(c, Rvalue::Constant(Constant::Bool(true)), 0);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(c),
        yes: BlockId(1),
        no: BlockId(2),
    };
    f.body().blocks.extend([
        block(
            vec![
                inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(0),
                        place: PlaceId(1),
                        mutable: false,
                        if_inactive: false,
                    },
                    0,
                ),
                inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    },
                    0,
                ),
            ],
            Terminator::Goto(BlockId(3)),
        ),
        block(vec![], Terminator::Goto(BlockId(3))),
        block(vec![], ret(0, 1)),
    ]);
    f.valid();
    f.body().blocks[1].instructions.pop();
    f.broken(vec![
        ViolationKind::LoanOutlivesSource {
            loan: LoanId(0),
            place: PlaceId(1),
        },
        ViolationKind::LiveLoanAtExit(LoanId(0)),
    ]);
}

fn closure_fixture() -> Fixture {
    let mut f = Fixture::string();
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable: false,
        if_inactive: false,
    });
    let capture_ty = f.body().places[1].ty;
    let nested = Body {
        arena_scope_grant: None,
        scoped_parameters: Vec::new(),
        parameter_scope: scoped(hirsel_types::NameSite::Item),

        locals: vec![Local {
            ty: capture_ty,
            storage: Storage::Capture,
            scope: ScopeId(0),
            span: Span::default(),
        }],
        places: vec![Place {
            loan: None,
            binding: None,
            root: LocalId(0),
            projection: vec![].into(),
            ty: capture_ty,
        }],
        scopes: vec![Scope {
            parent: None,
            span: Span::default(),
        }],
        blocks: vec![block(
            vec![],
            Terminator::Return {
                value: copy(0),
                cleanup: CleanupId(0),
            },
        )],
        rvalues: vec![],
        cleanup_nodes: vec![],
        cleanups: vec![Cleanup::default()],
        entry: BlockId(0),
        parameters: vec![],
        result: capture_ty,
        borrow_cursors: vec![],
        tail_sites: vec![],
        matches: vec![],
    };
    f.definition.bodies.push(nested);
    let mut fun = match function_signature("ignored", vec![], TypeKind::Tuple(vec![].into())).kind {
        SignatureKind::Function(f) => f,
        _ => unreachable!(),
    };
    fun.result = capture_ty;
    let closure = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let mut scoped = ty(TypeKind::Function(fun));
    scoped.modes.relation = Relation::environment();
    let scoped = f.definition.types.push(scoped);
    f.body().places[closure as usize].ty = scoped;
    f.body().locals[closure as usize].ty = scoped;
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions.insert(
            0,
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        );
    });
    let source = f.through_loan(1, 0);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(source, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        1,
    );
    let cleanup = f.cleanup(vec![
        drop(closure),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(1),
    ]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn closure_capture_types_arity_and_borrowed_ownership() {
    let mut f = closure_fixture();
    f.valid();
    if let Rvalue::Closure { captures, .. } = &mut f.body().rvalues[2] {
        captures.clear();
    }
    f.broken(vec![ViolationKind::Arity {
        entity: "closure captures",
        expected: 1,
        actual: 0,
    }]);
}

fn copied_capture_fixture(source: Uniqueness, capture: Uniqueness) -> Fixture {
    let mut f = Fixture::string();
    let source_ty = f.body().places[1].ty;
    let mut source_ty = interner().get(source_ty).unwrap().clone();
    source_ty.modes.uniqueness = source;
    let source_ty = f.definition.types.push(source_ty);
    f.body().locals[1].ty = source_ty;
    f.body().places[1].ty = source_ty;

    // Manufacture the capture in the shared definition store; only its
    // root uniqueness differs from the checked source operand's type.
    let mut nested = Fixture::unit();
    nested.definition.types = f.definition.types.clone();
    nested.body().locals[0].ty = f.body().result;
    nested.body().places[0].ty = f.body().result;
    nested.body().result = f.body().result;
    let slot = nested.slot(TypeKind::Primitive(Primitive::String), Storage::Capture);
    let capture_ty = nested.body().places[slot as usize].ty;
    let mut capture_ty = interner().get(capture_ty).unwrap().clone();
    capture_ty.modes.uniqueness = capture;
    let capture_ty = nested.definition.types.push(capture_ty);
    nested.body().locals[slot as usize].ty = capture_ty;
    nested.body().places[slot as usize].ty = capture_ty;
    // The environment owns the copied field; each many invocation only borrows it.
    nested.body().blocks[0].terminator = ret(0, 0);
    f.definition.types = nested.definition.types;
    f.definition.bodies.push(nested.definition.bodies.remove(0));

    let mut fun = match function_signature("ignored", vec![], TypeKind::Tuple(vec![].into())).kind {
        SignatureKind::Function(fun) => fun,
        _ => unreachable!(),
    };
    fun.result = f.body().result;
    let closure = f.slot(TypeKind::Function(fun), Storage::Temporary);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![copy(1)],
            destination: Destination::Inferred,
        },
        1,
    );
    let cleanup = f.cleanup(vec![drop(closure), drop(1)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn copy_captures_may_weaken_root_uniqueness_but_never_strengthen_it() {
    let f = copied_capture_fixture(Uniqueness::Unique, Uniqueness::Aliased);
    f.valid();
    let f = copied_capture_fixture(Uniqueness::Aliased, Uniqueness::Unique);
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "closure capture",
    }]);
}

#[test]
fn copy_capture_weakening_preserves_other_modes_and_type_structure() {
    let mut f = copied_capture_fixture(Uniqueness::Unique, Uniqueness::Aliased);
    f.valid();
    let nested = &mut f.definition.bodies[1];
    let mut changed = interner().get(nested.locals[1].ty).unwrap().clone();
    changed.modes.relation = Relation::environment();
    let changed = f.definition.types.push(changed);
    nested.locals[1].ty = changed;
    nested.places[1].ty = changed;
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "closure capture",
    }]);

    let mut f = copied_capture_fixture(Uniqueness::Unique, Uniqueness::Aliased);
    let nested = &mut f.definition.bodies[1];
    let mut changed = interner().get(nested.locals[1].ty).unwrap().clone();
    changed.kind = TypeKind::Primitive(Primitive::Bytes);
    let changed = f.definition.types.push(changed);
    nested.locals[1].ty = changed;
    nested.places[1].ty = changed;
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "closure capture",
    }]);
}

#[test]
fn take_capture_types_remain_exact() {
    let taken_capture = || {
        let mut f = copied_capture_fixture(Uniqueness::Unique, Uniqueness::Aliased);
        if let Rvalue::Closure { captures, .. } = &mut f.body().rvalues[2] {
            captures[0].usage = UseKind::Take;
        } else {
            panic!("expected closure producer");
        }
        let closure = f.body().places.len() - 1;
        let ty = f.body().places[closure].ty;
        let mut row = interner().get(ty).unwrap().clone();
        row.modes.multiplicity = Multiplicity::Once;
        let ty = f.definition.types.push(row);
        let root = f.body().places[closure].root;
        f.body().places[closure].ty = ty;
        f.body().locals[root.0 as usize].ty = ty;
        let nested = &mut f.definition.bodies[1];
        nested.add_cleanup(vec![drop(1)]);
        nested.blocks[0].terminator = ret(0, 1);
        f
    };
    // dd474e53 compares neutral shapes for a whole take: a fresh unique
    // String may enter an aliased capture. Shape and relation stay exact.
    taken_capture().valid();
    for change_kind in [false, true] {
        let mut f = taken_capture();
        let nested = &mut f.definition.bodies[1];
        let mut row = interner().get(nested.locals[1].ty).unwrap().clone();
        if change_kind {
            row.kind = TypeKind::Primitive(Primitive::Bytes);
        } else {
            row.modes.relation = Relation::environment();
        }
        let ty = f.definition.types.push(row);
        nested.locals[1].ty = ty;
        nested.places[1].ty = ty;
        f.broken(vec![ViolationKind::TypeMismatch {
            entity: "closure capture",
        }]);
    }
    // Positive capture modes must still be proved from the current content
    // by flow, even though static typing no longer compares those row modes.
    taken_slot_capture_fixture(Uniqueness::Unique, Sendability::NotSendable, true).broken(vec![
        ViolationKind::TypeMismatch {
            entity: "value modes",
        },
    ]);
    taken_slot_capture_fixture(Uniqueness::Aliased, Sendability::Sendable, true).broken(vec![
        ViolationKind::TypeMismatch {
            entity: "closure capture",
        },
    ]);
}

/// A whole `let mut` slot whose storage row is aliased and unsendable, holding
/// a fresh string (shared first when `shared`), taken into a once closure whose
/// capture row claims `uniqueness` and `sendability` (CLO-4).
fn taken_slot_capture_fixture(
    uniqueness: Uniqueness,
    sendability: Sendability,
    shared: bool,
) -> Fixture {
    let mut f = Fixture::string();
    let mut actions = vec![drop(1)];
    let mut failure = 1;
    if shared {
        let alias = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
        f.assign(alias, Rvalue::Use(copy(1)), 1);
        actions.insert(0, drop(alias));
        failure = f.cleanup(actions.clone());
    }
    let mut nested = Fixture::unit();
    nested.definition.types = f.definition.types.clone();
    nested.body().locals[0].ty = f.body().result;
    nested.body().places[0].ty = f.body().result;
    nested.body().result = f.body().result;
    let slot = nested.slot(TypeKind::Primitive(Primitive::String), Storage::Capture);
    let capture_ty = nested.body().places[slot as usize].ty;
    let mut capture_ty = interner().get(capture_ty).unwrap().clone();
    capture_ty.modes.uniqueness = uniqueness;
    capture_ty.modes.sendability = sendability;
    let capture_ty = nested.definition.types.push(capture_ty);
    nested.body().locals[slot as usize].ty = capture_ty;
    nested.body().places[slot as usize].ty = capture_ty;
    let cleanup = nested.cleanup(vec![drop(slot)]);
    nested.body().blocks[0].terminator = ret(0, cleanup);
    f.definition.types = nested.definition.types;
    f.definition.bodies.push(nested.definition.bodies.remove(0));

    let mut fun = match function_signature("ignored", vec![], TypeKind::Tuple(vec![].into())).kind {
        SignatureKind::Function(fun) => fun,
        _ => unreachable!(),
    };
    fun.result = f.body().result;
    let closure = f.slot(TypeKind::Function(fun), Storage::Temporary);
    let ty = f.body().places[closure as usize].ty;
    let mut row = interner().get(ty).unwrap().clone();
    row.modes.multiplicity = Multiplicity::Once;
    let ty = f.definition.types.push(row);
    f.body().places[closure as usize].ty = ty;
    f.body().locals[closure as usize].ty = ty;
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![take(1)],
            destination: Destination::Inferred,
        },
        failure,
    );
    actions.insert(0, drop(closure));
    let cleanup = f.cleanup(actions);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn take_capture_of_a_mutable_slot_claims_its_current_content() {
    taken_slot_capture_fixture(Uniqueness::Unique, Sendability::Sendable, false).valid();
    taken_slot_capture_fixture(Uniqueness::Aliased, Sendability::NotSendable, false).valid();
    taken_slot_capture_fixture(Uniqueness::Aliased, Sendability::NotSendable, true).valid();
}

#[test]
fn take_capture_claims_need_current_unique_and_sendable_content() {
    // Shared content is neither unique nor sendable: flow refutes each claim.
    taken_slot_capture_fixture(Uniqueness::Unique, Sendability::NotSendable, true).broken(vec![
        ViolationKind::TypeMismatch {
            entity: "value modes",
        },
    ]);
    taken_slot_capture_fixture(Uniqueness::Aliased, Sendability::Sendable, true).broken(vec![
        ViolationKind::TypeMismatch {
            entity: "closure capture",
        },
    ]);
}

#[test]
fn returning_a_closure_cannot_outlive_its_borrowed_source() {
    let mut f = closure_fixture();
    f.valid();
    let closure = 2;
    let cleanup = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(1),
    ]);
    f.body().result = f.body().places[closure].ty;
    f.body().blocks[0].terminator = ret(closure as u32, cleanup);
    f.broken(vec![
        ViolationKind::InvalidResultSource(PlaceId(closure as u32)),
        ViolationKind::LoanOutlivesSource {
            loan: LoanId(0),
            place: PlaceId(1),
        },
    ]);
}

#[test]
fn explicit_field_cleanup_is_in_reverse_declaration_order() {
    let mut f = Fixture::unit();
    let string = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::String)));
    let tuple = f.slot(
        TypeKind::Tuple(vec![string, string].into()),
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(tuple));
    let a = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(tuple),
        projection: vec![Projection::Field(0)].into(),
        ty: string,
    });
    let b = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(tuple),
        projection: vec![Projection::Field(1)].into(),
        ty: string,
    });
    let cleanup = f.cleanup(vec![drop(b), drop(a), drop(tuple)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    f.valid();
    edit_cleanup(&mut f.body(), cleanup, |actions| {
        actions.swap(0, 1);
    });
    f.broken(vec![ViolationKind::CleanupOrder(CleanupId(cleanup))]);
}

#[test]
fn call_parameters_weaken_actual_modes_without_granting_stronger_ones() {
    let mut f = Fixture::call();
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    if let SignatureKind::Function(fun) = &mut signature.kind {
        let mut parameter = interner().get(fun.parameters[0].ty).unwrap().clone();
        parameter.modes.relation = Relation::environment();
        fun.parameters[0].ty = interner().intern(parameter);
    }
    f.valid();
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    if let SignatureKind::Function(fun) = &mut signature.kind {
        let mut parameter = interner().get(fun.parameters[0].ty).unwrap().clone();
        parameter.modes.uniqueness = Uniqueness::Unique;
        fun.parameters[0].ty = interner().intern(parameter);
    }
    // Fresh content remains unique even in aliased receiving storage.
    f.valid();
    let alias = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(alias, Rvalue::Use(copy(1)), 0);
    f.emit(InstructionKind::Drop {
        place: PlaceId(alias),
        if_live: false,
    });
    let cleanup = f.cleanup(vec![drop(1)]);
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(cleanup);
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "call argument",
    }]);
}

#[test]
fn an_owned_plain_temporary_can_supply_a_borrowed_parameter() {
    let mut f = Fixture::unit();
    let argument = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(argument, Rvalue::Constant(Constant::Bool(true)), 0);
    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: take(argument),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(destination, 0)));
    let s = function_signature(
        "callee",
        vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::Bool))],
        TypeKind::Tuple(vec![].into()),
    );
    f.signatures.insert(id("callee"), s);
    f.valid();
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    if let SignatureKind::Function(fun) = &mut signature.kind {
        let mut parameter = interner().get(fun.parameters[0].ty).unwrap().clone();
        parameter.modes.uniqueness = Uniqueness::Unique;
        fun.parameters[0].ty = interner().intern(parameter);
    }
    // MODE-2 is vacuous for an operand that holds no counted blocks.
    f.valid();
    let mut f = Fixture::call();
    if let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("callee")).unwrap().kind {
        fun.parameters[0].passing = PassingMode::Borrow;
    }
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "argument use",
    }]);
}

#[test]
fn builtin_type_arity_and_argument_kind_are_validated() {
    let mut f = Fixture::unit();
    let t = f.body().result;
    f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![
            GenericArgument::Type(t),
            GenericArgument::Size(Size::Constant(3)),
        ]
        .into(),
    }));
    f.valid();
    f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![
            GenericArgument::Size(Size::Constant(3)),
            GenericArgument::Type(t),
        ]
        .into(),
    }));
    f.broken(vec![ViolationKind::InvalidTypeGraph]);
    let mut f = Fixture::unit();
    f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![].into(),
    }));
    f.broken(vec![ViolationKind::InvalidTypeGraph]);
}

#[test]
fn snapshot_requires_plain_values() {
    let mut f = Fixture::string();
    let dest = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.emit(InstructionKind::Snapshot {
        destination: PlaceId(dest),
        value: copy(1),
    });
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "nonplain snapshot",
    }]);
}

#[test]
fn atomic_replacement_preserves_caller_storage_on_drop_failure() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    f.body().parameters.push(LocalId(p));
    let new = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let old = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(
        new,
        Rvalue::Constant(Constant::String("replacement".into())),
        0,
    );
    f.emit(InstructionKind::Replace {
        destination: PlaceId(p),
        value: take(new),
        displaced: PlaceId(old),
        if_live: false,
    });
    f.emit(InstructionKind::Drop {
        place: PlaceId(old),
        if_live: false,
    });
    owner_signature(&mut f, PassingMode::Mut);
    f.valid();
    owner_signature(&mut f, PassingMode::Borrow);
    assert!(f.kinds().contains(&ViolationKind::InvalidUse(PlaceId(p))));
    owner_signature(&mut f, PassingMode::Mut);
    let old_type = f.body().places[old as usize].ty;
    let mut localized = interner().get(old_type).unwrap().clone();
    localized.modes.relation = Relation::environment();
    let ty = f.definition.types.push(localized);
    f.body().locals[old as usize].ty = ty;
    f.body().places[old as usize].ty = ty;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "displaced storage content",
    }));
}

#[test]
fn projected_writes_preserve_uniqueness_for_plain_bits_and_shared_boundaries() {
    for kind in [Primitive::U64, Primitive::String, Primitive::Bytes] {
        let mut f = Fixture::unit();
        let bytes = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::Bytes)));
        let element_kind = if kind == Primitive::Bytes {
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                arguments: vec![GenericArgument::Type(bytes)].into(),
            }
        } else {
            TypeKind::Primitive(kind)
        };
        let element = f.definition.types.push(ty(element_kind.clone()));
        let container_kind = TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(element)].into(),
        };
        let input = f.slot(container_kind.clone(), Storage::Parameter);
        let input_root = f.body().places[input as usize].root;
        let input_ty = f.body().places[input as usize].ty;
        let mut unique = interner().get(input_ty).unwrap().clone();
        unique.modes.uniqueness = Uniqueness::Unique;
        let unique = f.definition.types.push(unique);
        f.body().locals[input_root.0 as usize].ty = unique;
        f.body().places[input as usize].ty = unique;
        f.body().parameters.push(input_root);
        let storage = f.slot(container_kind.clone(), Storage::Local);
        f.assign(storage, Rvalue::Use(take(input)), 0);
        let replacement = f.slot(
            element_kind.clone(),
            if kind == Primitive::U64 {
                Storage::Temporary
            } else {
                Storage::Parameter
            },
        );
        if kind == Primitive::U64 {
            f.assign(replacement, Rvalue::Constant(Constant::Integer(3)), 0);
        } else {
            let replacement_root = f.body().places[replacement as usize].root;
            f.body().parameters.push(replacement_root);
        }
        let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
        let target = f.body().places.len() as u32;
        let root = f.body().places[storage as usize].root;
        let index_root = f.body().places[index as usize].root;
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root,
            projection: vec![Projection::Index(index_root)].into(),
            ty: element,
        });
        let displaced = f.slot(element_kind, Storage::Temporary);
        let cleanup = f.cleanup(vec![drop(replacement), drop(storage)]);
        f.emit(InstructionKind::Replace {
            destination: PlaceId(target),
            value: take(replacement),
            displaced: PlaceId(displaced),
            if_live: false,
        });
        f.body().blocks[0]
            .instructions
            .last_mut()
            .unwrap()
            .on_failure = CleanupId(cleanup);
        f.emit(InstructionKind::Drop {
            place: PlaceId(displaced),
            if_live: false,
        });
        let cleanup = f.cleanup(vec![drop(storage)]);
        f.body().blocks[0]
            .instructions
            .last_mut()
            .unwrap()
            .on_failure = CleanupId(cleanup);
        let output = f.slot(container_kind, Storage::Temporary);
        let output_root = f.body().places[output as usize].root;
        f.body().locals[output_root.0 as usize].ty = unique;
        f.body().places[output as usize].ty = unique;
        f.assign(output, Rvalue::Use(take(storage)), 0);
        f.emit(InstructionKind::Drop {
            place: PlaceId(output),
            if_live: false,
        });
        owner_signature(&mut f, PassingMode::Take);
        if kind == Primitive::String {
            f.broken(vec![ViolationKind::TypeMismatch {
                entity: "value modes",
            }]);
        } else {
            f.valid();
        }
    }
}

#[test]
fn borrowed_index_results_retain_the_container_parameter() {
    let mut f = Fixture::unit();
    let element = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bytes)));
    let xs = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(element)].into(),
        },
        Storage::Parameter,
    );
    let fallback = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    for place in [xs, fallback] {
        let root = f.body().places[place as usize].root;
        let declared = f.body().locals[root.0 as usize].ty;
        let mut row = interner().get(declared).unwrap().clone();
        row.modes.relation = Relation::environment();
        row.modes.sendability = Sendability::NotSendable;
        let local = f.definition.types.push(row);
        f.body().locals[root.0 as usize].ty = local;
        f.body().places[place as usize].ty = local;
        f.body().parameters.push(root);
    }
    let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
    let projected = f.body().places.len() as u32;
    let root = f.body().places[xs as usize].root;
    let index_root = f.body().places[index as usize].root;
    let element = f.body().places[fallback as usize].ty;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root,
        projection: vec![Projection::Index(index_root)].into(),
        ty: element,
    });
    let scope = f.body().parameter_scope;
    let mut result = interner().get(element).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    f.body().result = f.definition.types.push(result.clone());
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(projected),
        cleanup: CleanupId(0),
    };
    owner_signature(&mut f, PassingMode::Borrow);
    let result_id = f.body().result;
    f.body().result = bind_parameter_scopes(interner(), result_id, &[scope]).unwrap();
    f.valid();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(1))]);
    f.body().result = f.definition.types.push(result);
    owner_signature(&mut f, PassingMode::Borrow);
    let result_id = f.body().result;
    f.body().result = bind_parameter_scopes(interner(), result_id, &[scope]).unwrap();
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(projected)))
    );
}
#[test]
fn bad_branch_target() {
    let mut f = Fixture::unit();
    f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.valid();
    f.body().blocks[0].terminator = Terminator::Goto(BlockId(9));
    f.broken(vec![ViolationKind::InvalidId {
        arena: "block",
        index: 9,
    }]);
}
#[test]
fn every_arena_reference_is_checked() {
    let mut f = Fixture::unit();
    f.valid();
    f.body().blocks[0].instructions[0].kind = InstructionKind::Assign {
        destination: PlaceId(99),
        value: RvalueId(0),
    };
    f.broken(vec![ViolationKind::InvalidId {
        arena: "place",
        index: 99,
    }]);
    let mut f = Fixture::unit();
    f.definition.entry = BodyId(2);
    f.broken(vec![ViolationKind::InvalidId {
        arena: "body",
        index: 2,
    }]);
    let mut f = Fixture::unit();
    f.body().blocks[0].instructions[0].on_failure = CleanupId(7);
    f.broken(vec![ViolationKind::InvalidId {
        arena: "cleanup",
        index: 7,
    }]);
    let mut f = Fixture::unit();
    f.body().places[0].root = LocalId(3);
    f.broken(vec![ViolationKind::InvalidId {
        arena: "local",
        index: 3,
    }]);
    let mut f = Fixture::unit();
    f.body().locals[0].scope = ScopeId(3);
    f.broken(vec![ViolationKind::InvalidId {
        arena: "scope",
        index: 3,
    }]);
}
#[test]
fn foreign_in_range_type_does_not_panic() {
    let mut f = Fixture::unit();
    f.valid();
    // A session row outside the definition's table.
    let foreign = interner().intern(ty(TypeKind::Primitive(Primitive::Bool)));
    f.body().places[0].ty = foreign;
    f.broken_as_built(vec![ViolationKind::InvalidType(foreign)]);
}
#[test]
fn invalid_type_edge_does_not_reach_asserting_accessor() {
    let mut f = Fixture::unit();
    // The table lists a box but not the row its edge reaches.
    let foreign = interner().intern(ty(TypeKind::Primitive(Primitive::Bool)));
    let boxed = interner().intern(ty(TypeKind::Box(foreign)));
    f.definition.types = f.definition.types.iter().copied().chain([boxed]).collect();
    f.broken_as_built(vec![ViolationKind::InvalidType(foreign)]);
}
#[test]
fn type_cycles_and_recovery_are_rejected() {
    let mut f = Fixture::unit();
    f.valid();
    f.definition.types = interner()
        .intern_graph(1, |ids| vec![ty(TypeKind::Box(ids[0]))])
        .into();
    let id = f.definition.types.id(0).unwrap();
    f.body().result = id;
    f.body().locals[0].ty = id;
    f.body().places[0].ty = id;
    f.broken(vec![ViolationKind::InvalidTypeGraph]);
    let mut f = Fixture::unit();
    f.definition.types.push(ty(TypeKind::Error));
    f.broken(vec![ViolationKind::InvalidTypeGraph]);
}
#[test]
fn scope_cycles_are_detected() {
    let mut f = Fixture::unit();
    f.valid();
    f.body().scopes[0].parent = Some(ScopeId(0));
    f.broken(vec![ViolationKind::ScopeCycle(ScopeId(0))]);
}
#[test]
fn unused_rvalue_and_unreachable_block_are_detected() {
    let mut f = Fixture::unit();
    f.valid();
    f.body().rvalues.push(Rvalue::Constant(Constant::Unit));
    f.broken(vec![ViolationKind::UnreferencedRvalue(RvalueId(1))]);
    let mut f = Fixture::unit();
    f.body().blocks.push(block(vec![], ret(0, 0)));
    f.broken(vec![ViolationKind::UnreachableBlock(BlockId(1))]);
}
#[test]
fn result_type_mismatch() {
    let mut f = Fixture::unit();
    f.valid();
    f.body().result = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bool)));
    f.broken(vec![ViolationKind::TypeMismatch { entity: "return" }]);
}
#[test]
fn instruction_constant_type_mismatch() {
    let mut f = Fixture::unit();
    f.valid();
    f.body().rvalues[0] = Rvalue::Constant(Constant::Bool(false));
    f.broken(vec![ViolationKind::TypeMismatch { entity: "constant" }]);
}
#[test]
fn read_before_init() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.assign(p, Rvalue::Use(copy(0)), 0);
    f.body().blocks[0].terminator = ret(p, 0);
    f.valid();
    f.body().blocks[0].instructions.remove(0);
    f.broken(vec![ViolationKind::UnreferencedRvalue(RvalueId(0))]);
    // Keep the arena referenced while reading before its later initializer.
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.assign(p, Rvalue::Use(copy(0)), 0);
    f.body().blocks[0].instructions.swap(0, 1);
    f.broken(vec![ViolationKind::ReadBeforeInit(PlaceId(0))]);
}
#[test]
fn use_after_move_and_double_move_have_distinct_errors() {
    for (usage, expected) in [
        (UseKind::Copy, ViolationKind::UseAfterMove(PlaceId(0))),
        (UseKind::Take, ViolationKind::DoubleMove(PlaceId(0))),
    ] {
        let mut f = Fixture::unit();
        let p = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        f.assign(p, Rvalue::Use(copy(0)), 0);
        f.body().blocks[0].terminator = Terminator::Return {
            value: operand(0, usage),
            cleanup: CleanupId(0),
        };
        f.valid();
        f.body().rvalues[1] = Rvalue::Use(take(0));
        f.broken(vec![expected]);
    }
}
#[test]
fn live_storage_requires_replacement_drop() {
    let mut f = Fixture::unit();
    f.emit(InstructionKind::Drop {
        place: PlaceId(0),
        if_live: false,
    });
    f.assign(0, Rvalue::Constant(Constant::Unit), 0);
    f.valid();
    f.body().blocks[0].instructions.remove(1);
    f.broken(vec![ViolationKind::WriteToLive(PlaceId(0))]);
}
#[test]
fn owned_string_leak_and_missing_normal_cleanup() {
    let mut f = Fixture::string();
    f.valid();
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions.clear();
    });
    f.broken(vec![
        ViolationKind::CleanupMismatch {
            cleanup: CleanupId(1),
            expected: vec![LocalId(1)],
            actual: vec![],
        },
        ViolationKind::LeakedSlot(LocalId(1)),
    ]);
}
#[test]
fn missing_failure_cleanup_includes_earlier_argument_temporaries() {
    let mut f = Fixture::string();
    f.body().locals[1].storage = Storage::Temporary;
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Check {
            condition: copy(condition),
            rule: const { RuleId::of("FACT-1") },
        },
        1,
    ));
    f.valid();
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(0);
    f.broken(vec![ViolationKind::CleanupMismatch {
        cleanup: CleanupId(0),
        expected: vec![LocalId(1)],
        actual: vec![],
    }]);
}
#[test]
fn temporary_cleanup_uses_evaluation_order_not_slot_order() {
    let mut f = Fixture::string();
    f.body().locals[1].storage = Storage::Temporary;
    let p = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(p, Rvalue::Constant(Constant::String("second".into())), 0);
    edit_cleanup(&mut f.body(), 1, |actions| {
        *actions = vec![drop(p), drop(1)];
    });
    f.valid();
    f.body().blocks[0].instructions.swap(1, 2);
    edit_cleanup(&mut f.body(), 1, |actions| {
        *actions = vec![drop(1), drop(p)];
    });
    f.valid();
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions.swap(0, 1);
    });
    f.broken(vec![ViolationKind::CleanupOrder(CleanupId(1))]);
}
#[test]
fn locals_drop_in_reverse_declaration_order_after_temporaries() {
    let mut f = Fixture::string();
    let local = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    let temp = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(local, Rvalue::Constant(Constant::String("local".into())), 0);
    f.assign(temp, Rvalue::Constant(Constant::String("temp".into())), 0);
    edit_cleanup(&mut f.body(), 1, |actions| {
        *actions = vec![drop(temp), drop(local), drop(1)];
    });
    f.valid();
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions.swap(1, 2);
    });
    f.broken(vec![ViolationKind::CleanupOrder(CleanupId(1))]);
    edit_cleanup(&mut f.body(), 1, |actions| {
        *actions = vec![drop(local), drop(temp), drop(1)];
    });
    f.broken(vec![ViolationKind::CleanupOrder(CleanupId(1))]);
}
#[test]
fn return_extracts_value_before_cleanup() {
    let mut f = Fixture::string();
    f.body().result = f.body().places[1].ty;
    f.body().blocks[0].terminator = ret(1, 0);
    f.valid();
    f.body().blocks[0].terminator = ret(1, 1);
    f.valid(); // A skipped conditional drop is legal after extracting the result.
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions[0] = CleanupAction::Drop {
            place: PlaceId(1),
            if_live: false,
        };
    });
    f.broken(vec![
        ViolationKind::UseAfterMove(PlaceId(1)),
        ViolationKind::CleanupMismatch {
            cleanup: CleanupId(1),
            expected: vec![],
            actual: vec![LocalId(1)],
        },
    ]);
}
#[test]
fn loan_outliving_source_and_write_under_loan() {
    let mut f = Fixture::loan();
    f.valid();
    f.body().blocks[0].instructions.pop();
    // Cleanup must end the loan before destroying its source.
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions.push(CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        });
    });
    f.broken(vec![ViolationKind::LoanOutlivesSource {
        loan: LoanId(0),
        place: PlaceId(1),
    }]);
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions.swap(0, 1);
    });
    f.valid();
    // Dead storage is illegal to borrow; nevertheless an Assign must also
    // report its active-loan conflict without a validator panic.
    let mut f = Fixture::string();
    f.emit(InstructionKind::Drop {
        place: PlaceId(1),
        if_live: false,
    });
    f.assign(1, Rvalue::Constant(Constant::String("new".into())), 0);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable: false,
        if_inactive: false,
    });
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    f.valid();
    f.body().blocks[0].instructions.swap(3, 4);
    f.broken(vec![
        ViolationKind::ReadBeforeInit(PlaceId(1)),
        ViolationKind::LoanConflict {
            loan: LoanId(0),
            place: PlaceId(1),
        },
    ]);
}
#[test]
fn moving_a_loan_source_is_rejected() {
    let mut f = Fixture::loan();
    let p = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(p, Rvalue::Use(take(1)), 0);
    edit_cleanup(&mut f.body(), 1, |actions| {
        *actions = vec![drop(p)];
    });
    f.valid();
    f.body().blocks[0].instructions.swap(3, 4);
    f.broken(vec![ViolationKind::LoanConflict {
        loan: LoanId(0),
        place: PlaceId(1),
    }]);
}
#[test]
fn mutable_loans_are_exclusive() {
    let mut f = Fixture::loan();
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(1),
        mutable: true,
        if_inactive: false,
    });
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(1),
        if_active: false,
    });
    f.valid();
    f.body().blocks[0].instructions.swap(3, 4);
    f.broken(vec![ViolationKind::LoanConflict {
        loan: LoanId(0),
        place: PlaceId(1),
    }]);
}
#[test]
fn loan_ends_must_be_definite() {
    let mut f = Fixture::loan();
    f.valid();
    f.body().blocks[0].instructions.swap(2, 3);
    edit_cleanup(&mut f.body(), 1, |actions| {
        actions.insert(
            0,
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
        );
    });
    f.broken(vec![ViolationKind::InactiveLoan(LoanId(0))]);
}
#[test]
fn missing_loan_on_nonplain_borrow() {
    let mut f = Fixture::loan();
    let p = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    let source = f.through_loan(1, 0);
    f.assign(p, Rvalue::Use(operand(source, UseKind::Borrow)), 0);
    f.body().blocks[0].instructions.swap(3, 4);
    f.valid();
    f.body().places[source as usize].loan = None;
    f.broken(vec![ViolationKind::MissingLoan(PlaceId(source))]);
}
#[test]
fn exactly_the_calls_to_never_callees_diverge() {
    let never = || TypeKind::Primitive(Primitive::Never);
    let unit = || TypeKind::Tuple(vec![].into());
    Fixture::result_call(never(), false).valid();
    Fixture::result_call(unit(), true).valid();
    // The former lowering: a Never result copied into a slot with a successor.
    Fixture::result_call(never(), true).broken(vec![ViolationKind::TypeMismatch {
        entity: "returning call to Never callee",
    }]);
    // A normal callee cannot end its caller's control flow.
    Fixture::result_call(unit(), false).broken(vec![ViolationKind::TypeMismatch {
        entity: "diverging call to returning callee",
    }]);
}
#[test]
fn call_wrong_arity() {
    let mut f = Fixture::call();
    f.valid();
    if let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator {
        arguments.clear();
    }
    f.broken(vec![ViolationKind::Arity {
        entity: "call arguments",
        expected: 1,
        actual: 0,
    }]);
}
#[test]
fn call_missing_signature_and_type_mismatch() {
    let mut f = Fixture::call();
    f.valid();
    f.signatures.clear();
    f.broken(vec![ViolationKind::MissingSignature(path("callee"))]);
    let mut f = Fixture::call();
    let s = function_signature(
        "callee",
        vec![(PassingMode::Take, TypeKind::Primitive(Primitive::Bool))],
        TypeKind::Tuple(vec![].into()),
    );
    f.signatures.insert(id("callee"), s);
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "call argument",
    }]);
}
#[test]
fn transferred_arguments_are_absent_from_call_failure_cleanup() {
    let mut f = Fixture::call();
    f.valid();
    let c = f.cleanup(vec![drop(1)]);
    if let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator {
        *on_failure = CleanupId(c);
    }
    f.valid(); // Call failure may skip a listed conditional drop of transferred storage.
    edit_cleanup(&mut f.body(), c, |actions| {
        actions[0] = CleanupAction::Drop {
            place: PlaceId(1),
            if_live: false,
        };
    });
    f.broken(vec![
        ViolationKind::UseAfterMove(PlaceId(1)),
        ViolationKind::CleanupMismatch {
            cleanup: CleanupId(c),
            expected: vec![],
            actual: vec![LocalId(1)],
        },
    ]);
}
#[test]
fn body_parameter_arity_kind_and_borrowed_ownership() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    f.body().parameters.push(LocalId(p));
    let s = function_signature(
        "main",
        vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::String))],
        TypeKind::Tuple(vec![].into()),
    );
    f.signatures.insert(id("main"), s);
    f.valid();
    let s = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    f.signatures.insert(id("main"), s);
    f.broken(vec![ViolationKind::Arity {
        entity: "body parameters",
        expected: 0,
        actual: 1,
    }]);
    let s = function_signature(
        "main",
        vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::String))],
        TypeKind::Tuple(vec![].into()),
    );
    f.signatures.insert(id("main"), s);
    f.body().result = f.body().places[p as usize].ty;
    f.body().blocks[0].terminator = ret(p, 0);
    if let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind {
        fun.result = fun.parameters[0].ty;
    }
    f.broken(vec![
        ViolationKind::InvalidResultSource(PlaceId(p)),
        ViolationKind::BorrowedMove(PlaceId(p)),
    ]);
}
#[test]
fn definite_initialization_joins_all_predecessors() {
    let mut f = Fixture::unit();
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
    let p = f.slot(TypeKind::Tuple(vec![].into()), Storage::Local);
    let r = f.body().rvalues.len() as u32;
    f.body().rvalues.push(Rvalue::Constant(Constant::Unit));
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(condition),
        yes: BlockId(1),
        no: BlockId(2),
    };
    let assign = inst(
        InstructionKind::Assign {
            destination: PlaceId(p),
            value: RvalueId(r),
        },
        0,
    );
    f.body().blocks.extend([
        block(vec![assign.clone()], Terminator::Goto(BlockId(3))),
        block(vec![assign], Terminator::Goto(BlockId(3))),
        block(vec![], ret(p, 0)),
    ]);
    f.valid();
    f.body().blocks[2].instructions.clear();
    f.broken(vec![ViolationKind::ReadBeforeInit(PlaceId(p))]);
}
#[test]
fn irreducible_cfg_terminates_at_a_fixed_point() {
    let mut f = Fixture::unit();
    let p = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(p, Rvalue::Constant(Constant::Bool(true)), 0);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(p),
        yes: BlockId(1),
        no: BlockId(2),
    };
    f.body().blocks.extend([
        block(vec![], Terminator::Goto(BlockId(2))),
        block(
            vec![],
            Terminator::Branch {
                condition: copy(p),
                yes: BlockId(1),
                no: BlockId(3),
            },
        ),
        block(vec![], ret(0, 0)),
    ]);
    f.valid();
}
#[test]
fn random_mutations_never_panic_and_arbitrary_cfgs_terminate() {
    mutation_verdicts();
}

/// The same fault corpus also feeds the three-salt byte differential.
pub(super) fn mutation_verdicts() -> String {
    use std::fmt::Write;
    let mut verdicts = String::new();
    // Reproducible dependency-free property campaign, including foreign type
    // IDs, broken arenas, malformed cleanup and cyclic/irreducible CFGs.
    let mut seed = 0x72134a20cafe1234u64;
    for _ in 0..10_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut f = Fixture::call();
        let n = (seed >> 32) as u32;
        match seed % 12 {
            0 => f.body().entry = BlockId(n % 5),
            1 => f.body().places[0].root = LocalId(n),
            2 => f.body().blocks[0].terminator = Terminator::Goto(BlockId(n % 3)),
            3 => edit_cleanup(&mut f.body(), 0, |actions| {
                actions.push(CleanupAction::Drop {
                    place: PlaceId(n % 5),
                    if_live: seed & 64 != 0,
                });
            }),
            4 => f.body().blocks[0].instructions[0].on_failure = CleanupId(n),
            5 => f.body().places[0].ty = interner().push(ty(TypeKind::Primitive(Primitive::Bool))),
            6 => f.body().scopes[0].parent = Some(ScopeId(n % 3)),
            7 => f.body().blocks[0].instructions.push(inst(
                InstructionKind::EndLoan {
                    loan: LoanId(n % 4),
                    if_active: false,
                },
                0,
            )),
            8 => f.body().rvalues[0] = Rvalue::Use(operand(n % 5, UseKind::Take)),
            9 => {
                f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
                f.body().blocks[1].terminator = Terminator::Goto(BlockId(n % 2));
            }
            10 => {
                interner().intern(ty(TypeKind::Error));
            }
            _ => {
                f.body().blocks[0].instructions.push(inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(0),
                        place: PlaceId(n % 4),
                        mutable: true,
                        if_inactive: false,
                    },
                    0,
                ));
            }
        };
        let mut first_reads = vec![];
        let first = validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut first_reads,
        );
        let mut second_reads = vec![];
        let second = validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut second_reads,
        );
        assert_eq!(first, second);
        assert_eq!(first_reads, second_reads);
        writeln!(verdicts, "{first:?}; reads={first_reads:?}").unwrap();
    }
    verdicts
}

#[test]
fn dead_conditional_cleanup_entries_are_skipped() {
    let mut f = Fixture::unit();
    let dead = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    let cleanup = f.cleanup(vec![drop(dead)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.valid();
    edit_cleanup(&mut f.body(), cleanup, |actions| {
        actions[0] = CleanupAction::Drop {
            place: PlaceId(dead),
            if_live: false,
        };
    });
    f.broken(vec![
        ViolationKind::ReadBeforeInit(PlaceId(dead)),
        ViolationKind::CleanupMismatch {
            cleanup: CleanupId(cleanup),
            expected: vec![],
            actual: vec![LocalId(dead)],
        },
    ]);
}

#[test]
fn type_owned_resource_finalizer_borrows_self() {
    let mut f = Fixture::unit();
    let self_place = f.slot(
        TypeKind::Named {
            definition: id("main"),
            arguments: vec![].into(),
        },
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(self_place));
    let mut signature = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    let unit = if let SignatureKind::Function(fun) = &signature.kind {
        fun.result
    } else {
        unreachable!()
    };
    signature.kind = SignatureKind::Type(TypeDefinition {
        loop_source: None,
        format: None,
        destructor_allocation: Allocation::NoAlloc,
        generics: vec![],
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
    });
    f.signatures.insert(id("main"), signature);
    f.valid();
    f.emit(InstructionKind::Drop {
        place: PlaceId(self_place),
        if_live: false,
    });
    f.broken(vec![ViolationKind::BorrowedMove(PlaceId(self_place))]);
}

#[test]
fn box_payload_projects_at_field_zero() {
    let mut f = Fixture::unit();
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let boxed = f.slot(TypeKind::Box(scalar), Storage::Parameter);
    f.body().parameters.push(LocalId(boxed));
    let payload = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(boxed),
        projection: vec![Projection::Field(0)].into(),
        ty: scalar,
    });
    let cleanup = f.cleanup(vec![drop(boxed)]);
    f.body().result = scalar;
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(payload),
        cleanup: CleanupId(cleanup),
    };
    owner_signature(&mut f, PassingMode::Take);
    f.valid();
    f.body().places[payload as usize].projection[0] = Projection::Field(1);
    f.broken(vec![ViolationKind::InvalidProjection(PlaceId(payload))]);
}

#[test]
fn unresolved_call_origins_fail_closed() {
    let mut f = Fixture::call();
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let mut branded = ty(TypeKind::Primitive(Primitive::String));
    branded.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(hirsel_types::ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    let branded = interner().intern(branded);
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.parameters[0].ty = branded;
    }
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
            .any(|v| matches!(v.kind, ViolationKind::InvalidOrigin(_))),
        "{report:?}"
    );
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let mut wrong = interner().get(branded).unwrap().clone();
    wrong.kind = TypeKind::Primitive(Primitive::Bool);
    let wrong = interner().intern(wrong);
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.parameters[0].ty = wrong;
    }
    assert!(
        !validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut vec![]
        )
        .violations
        .is_empty()
    );
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let mut fixed = interner().get(branded).unwrap().clone();
    fixed.brand.alternatives[0].root = OriginRoot::Definition(path("known"));
    let fixed = interner().intern(fixed);
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.parameters[0].ty = fixed;
    }
    assert!(
        !validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut vec![]
        )
        .violations
        .is_empty()
    );
}

#[test]
fn inferred_placement_does_not_constrain_an_explicit_constructor_destination() {
    let mut f = Fixture::unit();
    let empty = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.assign(
        empty,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![],
            destination: Destination::Heap,
        },
        0,
    );
    f.valid();
    let current = f.body().places[empty as usize].ty;
    let mut row = interner().get(current).unwrap().clone();
    row.modes.relation = row
        .modes
        .relation
        .union(&Relation::new([Reach::Region(hirsel_types::RegionId(0))]));
    let region = f.definition.types.push(row);
    f.body().places[empty as usize].ty = region;
    f.body().locals[empty as usize].ty = region;
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "constructor placement",
    }]);
}

#[test]
fn dependent_result_origins_require_a_live_binding() {
    let mut f = Fixture::call();
    let mut result = ty(TypeKind::Primitive(Primitive::String));
    result.brand.alternatives.push(Origin {
        root: OriginRoot::Local(lexical(), hirsel_types::LocalId(7)),
        fields: vec![].into(),
    });
    result.modes.relation = result
        .modes
        .relation
        .union(&Relation::place(result.brand.alternatives[0].clone()));
    let result = f.definition.types.push(result);
    f.body().locals[2].ty = result;
    f.body().places[2].ty = result;
    f.body().result = result;
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let mut expected = ty(TypeKind::Primitive(Primitive::String));
    expected.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(hirsel_types::ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    expected.modes.relation = expected
        .modes
        .relation
        .union(&Relation::place(expected.brand.alternatives[0].clone()));
    expected.modes.relation = Relation::parameters([hirsel_types::ParameterRef::bound(ParamId(0))]);
    let expected = interner().intern(expected);
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.result = expected;
    }
    let report = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        report.violations.iter().any(|v| v.kind
            == ViolationKind::TypeMismatch {
                entity: "call result"
            }),
        "{report:?}"
    );
    // A malformed parameter ordinal is a real declaration invariant violation.
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let mut invalid = interner().get(expected).unwrap().clone();
    invalid.brand.alternatives[0].root =
        OriginRoot::Parameter(hirsel_types::ParameterRef::bound(ParamId(1)));
    let invalid = interner().intern(invalid);
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.result = invalid;
    }
    f.broken(vec![ViolationKind::InvalidSignature(path("callee"))]);
}

#[test]
fn nested_call_brands_do_not_hide_payload_type_mismatches() {
    let mut f = Fixture::call();
    // Replace the prepared string argument with an already owned box parameter.
    f.body().blocks[0].instructions.pop();
    f.body().rvalues.pop();
    f.body().locals[1].storage = Storage::Parameter;
    f.body().parameters.push(LocalId(1));
    let string = f.body().locals[1].ty;
    let boxed = f.definition.types.push(ty(TypeKind::Box(string)));
    f.body().locals[1].ty = boxed;
    f.body().places[1].ty = boxed;
    owner_signature(&mut f, PassingMode::Take);
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let mut branded = ty(TypeKind::Primitive(Primitive::String));
    branded.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(hirsel_types::ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    let branded = interner().intern(branded);
    let boxed = interner().intern(ty(TypeKind::Box(branded)));
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.parameters[0].ty = boxed;
    }
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "call argument",
    }]);
    let signature = f.signatures.get_mut(&path("callee")).unwrap();
    let mut wrong = interner().get(branded).unwrap().clone();
    wrong.kind = TypeKind::Primitive(Primitive::Bool);
    let wrong = interner().intern(wrong);
    let boxed = interner().intern(ty(TypeKind::Box(wrong)));
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.parameters[0].ty = boxed;
    }
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "call argument",
    }]);
}

// A retained scope is independent of parameter and TIR storage ordinals.
// The nested callable references the enclosing binder, not its own parameter 0.
fn owner_origin_fixture(nested: bool) -> Fixture {
    let mut f = Fixture::unit();
    f.body().parameter_scope = scoped(NameSite::Item);
    let scope = f.body().parameter_scope;
    let mut body_string = ty(TypeKind::Primitive(Primitive::String));
    body_string.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    });
    let body_string = f.definition.types.push(body_string);
    let mut owner = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    let mut declared_string = ty(TypeKind::Primitive(Primitive::String));
    declared_string.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::Bound {
            depth: u32::from(nested),
            parameter: ParamId(0),
        }),
        fields: vec![].into(),
    });
    let declared_string = interner().intern(declared_string);
    let (body_kind, declared) = if nested {
        let SignatureKind::Function(mut inner) = owner.kind.clone() else {
            unreachable!()
        };
        inner.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: declared_string,
        }]
        .into();
        inner.result = declared_string;
        let declared = interner().intern(ty(TypeKind::Function(inner.clone())));
        inner.parameters[0].ty = body_string;
        inner.result = body_string;
        (TypeKind::Function(inner), declared)
    } else {
        (TypeKind::Primitive(Primitive::String), declared_string)
    };
    let parameter = f.slot(body_kind, Storage::Parameter);
    if !nested {
        f.body().locals[parameter as usize].ty = body_string;
        f.body().places[parameter as usize].ty = body_string;
    }
    f.body().parameters.push(LocalId(parameter));
    f.body().result = f.body().places[parameter as usize].ty;
    f.body().blocks[0].terminator = ret(parameter, 0);
    if let SignatureKind::Function(fun) = &mut owner.kind {
        fun.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Take,
            ty: declared,
        }]
        .into();
        fun.result = declared;
    }
    f.signatures.insert(id("main"), owner);
    f
}

#[test]
fn owner_parameter_and_result_origins_are_checked_without_weakening_modes() {
    let mut f = owner_origin_fixture(false);
    assert_valid(&tabled(&f.definition), interner(), &f.signatures);
    let root = f.body().result;
    let mut row = interner().get(root).unwrap().clone();
    row.modes.relation = Relation::environment();
    let local = f.definition.types.push(row);
    f.body().locals[1].ty = local;
    f.body().places[1].ty = local;
    f.body().result = local;
    f.broken(vec![
        ViolationKind::TypeMismatch {
            entity: "body parameter",
        },
        ViolationKind::TypeMismatch {
            entity: "body result",
        },
    ]);
}

#[test]
fn scoped_nested_owner_origins_preserve_type_mode_placement_and_arity_checks() {
    for mutation in 0..5 {
        let mut f = owner_origin_fixture(true);
        assert_valid(&tabled(&f.definition), interner(), &f.signatures);
        let root = f.body().result;
        let mut row = interner().get(root).unwrap().clone();
        let TypeKind::Function(inner) = &mut row.kind else {
            unreachable!()
        };
        let mut payload = interner().get(inner.result).unwrap().clone();
        match mutation {
            0 => payload.kind = TypeKind::Primitive(Primitive::Bool),
            1 => payload.modes.relation = Relation::environment(),
            2 => {
                payload.modes.relation = Relation::place(Origin {
                    root: OriginRoot::Local(lexical(), hirsel_types::LocalId(0)),
                    fields: vec![].into(),
                })
            }
            3 => inner.parameters[0].passing = PassingMode::Mut,
            _ => inner.parameters.push(inner.parameters[0].clone()),
        }
        let payload = f.definition.types.push(payload);
        for parameter in &mut inner.parameters {
            parameter.ty = payload;
        }
        inner.result = payload;
        let wrong = f.definition.types.push(row);
        f.body().locals[1].ty = wrong;
        f.body().places[1].ty = wrong;
        f.body().result = wrong;
        f.broken(vec![
            ViolationKind::TypeMismatch {
                entity: "body parameter",
            },
            ViolationKind::TypeMismatch {
                entity: "body result",
            },
        ]);
    }
}

#[test]
fn known_owner_origins_still_require_equality() {
    let mut f = owner_origin_fixture(false);
    let root = f.body().result;
    let mut row = interner().get(root).unwrap().clone();
    row.brand.alternatives[0].root = OriginRoot::Definition(path("actual"));
    row.modes.relation = Relation::global();
    let actual = f.definition.types.push(row);
    f.body().locals[1].ty = actual;
    f.body().places[1].ty = actual;
    f.body().result = actual;
    let owner = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(fun) = &mut owner.kind else {
        unreachable!()
    };
    let mut row = interner().get(fun.result).unwrap().clone();
    row.brand.alternatives[0].root = OriginRoot::Definition(path("expected"));
    row.modes.relation = Relation::global();
    let expected = interner().intern(row);
    fun.parameters[0].ty = expected;
    fun.result = expected;
    f.broken(vec![
        ViolationKind::TypeMismatch {
            entity: "body parameter",
        },
        ViolationKind::TypeMismatch {
            entity: "body result",
        },
    ]);
}

#[test]
fn constant_initializer_requires_no_parameters_and_its_declared_result() {
    let mut f = Fixture::unit();
    let types = interner();
    let unit = types.push(ty(TypeKind::Tuple(vec![].into())));
    let mut signature = Signature {
        definition: f.definition.path.clone(),
        kind: SignatureKind::Const {
            ty: unit,
            generics: Default::default(),
        },
        metadata: SignatureMetadata::default(),
        fingerprint: ContentHash::default(),
    };
    f.signatures
        .insert(signature.definition.clone(), signature.clone());
    f.valid();
    let wrong = interner().intern(ty(TypeKind::Primitive(Primitive::Bool)));
    signature.kind = SignatureKind::Const {
        ty: wrong,
        generics: Default::default(),
    };
    f.signatures.insert(signature.definition.clone(), signature);
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "constant result",
    }]);
    let signature = f.signatures.get_mut(f.definition.path).unwrap();
    signature.kind = SignatureKind::Const {
        ty: unit,
        generics: Default::default(),
    };
    let p = f.slot(TypeKind::Tuple(vec![].into()), Storage::Parameter);
    f.body().parameters.push(LocalId(p));
    // Return still uses the ordinary initialized unit slot; the parameter is unused.
    f.broken(vec![ViolationKind::Arity {
        entity: "constant parameters",
        expected: 0,
        actual: 1,
    }]);
}

#[test]
fn returns_cannot_strengthen_modes_or_cleanup_promises() {
    for mutation in 0..5 {
        let mut f = if mutation <= 2 {
            Fixture::string()
        } else {
            Fixture::unit()
        };
        let source = if mutation <= 2 { 1 } else { 0 };
        let root = f.body().locals[source].ty;
        let mut expected = interner().get(root).unwrap().clone();
        let mut actual = expected.clone();
        match mutation {
            0 => expected.modes.uniqueness = Uniqueness::Unique,
            1 => expected.sendability_requirement = hirsel_types::SendabilityRequirement::Sendable,
            2 => actual.modes.relation = Relation::environment(),
            3 => actual.modes.multiplicity = Multiplicity::Once,
            _ => actual.drop_allocation = DropAllocation::MayAllocate,
        }
        f.body().result = f.definition.types.push(expected);
        let actual = f.definition.types.push(actual);
        f.body().locals[source].ty = actual;
        f.body().places[source].ty = actual;
        f.body().blocks[0].terminator = ret(source as u32, 0);
        if mutation <= 1 {
            f.valid();
            let alias = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
            f.assign(alias, Rvalue::Use(copy(source as u32)), 0);
            f.emit(InstructionKind::Drop {
                place: PlaceId(alias),
                if_live: false,
            });
            let cleanup = f.cleanup(vec![drop(source as u32)]);
            f.body().blocks[0]
                .instructions
                .last_mut()
                .unwrap()
                .on_failure = CleanupId(cleanup);
        }
        f.broken(vec![ViolationKind::TypeMismatch {
            entity: if mutation <= 1 {
                "value modes"
            } else {
                "return"
            },
        }]);
    }
}

fn generic_variant_equality_fixture(data_payload: bool) -> Fixture {
    let mut f = Fixture::unit();
    let payload = if data_payload {
        f.definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U8)))
    } else {
        let unit = f.body().result;
        f.definition.types.push(ty(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result: unit,
            error: None,
            allocation: Allocation::NoAlloc,
            authority: AuthoritySummary::inert(),
            uses: Uses::Only(AuthoritySummary::inert()),
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        })))
    };
    let value = f.slot(
        TypeKind::Named {
            definition: id("Choice"),
            arguments: vec![GenericArgument::Type(payload)].into(),
        },
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(value));
    let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(
        result,
        Rvalue::Binary {
            op: BinaryOp::Eq,
            left: operand(value, UseKind::Borrow),
            right: operand(value, UseKind::Borrow),
        },
        0,
    );
    let mut owner = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    let param_type = f.body().places[value as usize].ty;
    let unit = f.body().result;
    if let SignatureKind::Function(fun) = &mut owner.kind {
        fun.result = unit;
        fun.parameters.push(Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: param_type,
        });
    }
    f.signatures.insert(id("main"), owner);
    let types = interner();
    let parameter = types.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let representation = types.push(ty(TypeKind::Variant(
        vec![Variant {
            name: "Value".into(),
            fields: vec![(None, parameter)].into(),
        }]
        .into(),
    )));
    let declaration = Signature {
        definition: id("Choice"),
        metadata: Default::default(),
        kind: SignatureKind::Type(TypeDefinition {
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
            layout: Default::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
            destructor_allocation: Allocation::NoAlloc,
        }),
        fingerprint: ContentHash::default(),
    };
    f.signatures.insert(id("Choice"), declaration);
    f
}

#[test]
fn generic_nominal_equality_proves_instantiated_fields() {
    generic_variant_equality_fixture(true).valid();
    generic_variant_equality_fixture(false).broken(vec![ViolationKind::InvalidOperator]);
    let mut opaque = generic_variant_equality_fixture(true);
    opaque
        .signatures
        .get_mut(&path("Choice"))
        .unwrap()
        .metadata
        .visibility = hirsel_syntax::Visibility::PublicOpaque;
    // A client sees no owner representation in its public snapshot.
    let SignatureKind::Type(d) = &mut opaque.signatures.get_mut(&path("Choice")).unwrap().kind
    else {
        panic!()
    };
    d.representation = None;
    opaque.broken(vec![ViolationKind::InvalidOperator]);
}

fn owner_local_projection(
    plain: bool,
    indexed: bool,
    declared: Relation,
    actual: Relation,
) -> Fixture {
    let mut f = if plain {
        Fixture::unit()
    } else {
        Fixture::string()
    };
    let source = if plain { 0 } else { 1 };
    let source_ty = f.body().places[source].ty;
    let mut field = interner().get(source_ty).unwrap().clone();
    field.modes.relation = declared;
    let expected = f.definition.types.push(field.clone());
    f.body().places[source].ty = expected;
    f.body().locals[source].ty = expected;
    let kind = if indexed {
        TypeKind::Builtin {
            kind: BuiltinType::Array,
            arguments: vec![GenericArgument::Type(expected)].into(),
        }
    } else {
        TypeKind::Tuple(vec![expected].into())
    };
    let owner = f.slot(kind, Storage::Local);
    let owner_ty = f.body().places[owner as usize].ty;
    let mut owner_row = interner().get(owner_ty).unwrap().clone();
    owner_row.modes.relation = Relation::environment();
    let owner_ty = f.definition.types.push(owner_row);
    f.body().places[owner as usize].ty = owner_ty;
    f.body().locals[owner as usize].ty = owner_ty;
    f.assign(
        owner,
        Rvalue::Construct {
            constructor: if indexed {
                Constructor::Sequence
            } else {
                Constructor::Tuple
            },
            fields: vec![ConstructField {
                field: 0,
                value: if plain { copy(0) } else { take(1) },
            }],
            destination: Destination::Inferred,
        },
        if plain { 0 } else { 1 },
    );
    let projection = if indexed {
        let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
        Projection::Index(LocalId(index))
    } else {
        Projection::Field(0)
    };
    field.modes.relation = actual;
    let actual = f.definition.types.push(field);
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(owner),
        projection: vec![projection].into(),
        ty: actual,
    });
    let cleanup = if plain {
        0
    } else {
        f.cleanup(vec![drop(owner)])
    };
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}
#[test]
fn counted_projections_retain_their_owner_locality() {
    for indexed in [false, true] {
        owner_local_projection(false, indexed, Relation::global(), Relation::environment()).valid();
    }
}
#[test]
fn projection_locality_does_not_grant_other_type_evidence() {
    for indexed in [false, true] {
        for mutation in 0..7 {
            let mut f =
                owner_local_projection(false, indexed, Relation::global(), Relation::environment());
            let id = f.body().places.len() - 1;
            let actual_ty = f.body().places[id].ty;
            let mut row = interner().get(actual_ty).unwrap().clone();
            match mutation {
                0 => row.modes.relation = Relation::global(),
                1 => row.modes.uniqueness = Uniqueness::Unique,
                2 => row.sendability_requirement = hirsel_types::SendabilityRequirement::Sendable,
                3 => row.modes.multiplicity = Multiplicity::Once,
                4 => row.drop_allocation = DropAllocation::MayAllocate,
                5 => row.modes.relation = Relation::global(),
                6 => row.kind = TypeKind::Primitive(Primitive::Bytes),
                _ => unreachable!(),
            }
            f.body().places[id].ty = f.definition.types.push(row);
            f.broken(vec![ViolationKind::InvalidProjection(PlaceId(id as u32))]);
        }
        for (plain, declared, actual) in [
            (false, Relation::environment(), Relation::global()),
            (true, Relation::global(), Relation::environment()),
        ] {
            let mut f = owner_local_projection(plain, indexed, declared, actual);
            let id = f.body().places.len() - 1;
            f.broken(vec![ViolationKind::InvalidProjection(PlaceId(id as u32))]);
        }
    }
}

#[test]
fn owned_plain_operator_results_do_not_retain_operand_loans() {
    let mut f = Fixture::unit();
    let source = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Local);
    f.assign(source, Rvalue::Constant(Constant::Bool(true)), 0);
    let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(source),
        mutable: true,
        if_inactive: false,
    });
    let failure = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    let access = f.through_loan(source, 0);
    f.assign(
        result,
        Rvalue::Unary {
            op: hirsel_syntax::UnaryOp::Not,
            value: operand(access, UseKind::Mut),
        },
        failure,
    );
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    f.body().result = f.body().places[result as usize].ty;
    f.body().blocks[0].terminator = ret(result, 0);
    f.valid();
    // An explicit view still depends on the loan, including plain mut storage.
    f.body().rvalues[2] = Rvalue::Use(operand(access, UseKind::Mut));
    assert!(f.kinds().contains(&ViolationKind::ExpiredLoan {
        loan: LoanId(0),
        place: PlaceId(result)
    }));
}

fn nominal_copy_fixture(payload: TypeKind, resource: bool) -> Fixture {
    let mut f = Fixture::unit();
    let mut payload = payload;
    if let TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments,
    } = &mut payload
    {
        *arguments = vec![GenericArgument::Type(f.body().result)].into();
    }
    let child = f.definition.types.push(ty(payload));
    let kind = TypeKind::Named {
        definition: id("Holder"),
        arguments: vec![GenericArgument::Type(child)].into(),
    };
    let source = f.slot(kind.clone(), Storage::Parameter);
    let destination = f.slot(kind, Storage::Temporary);
    f.body().parameters.push(LocalId(source));
    owner_signature(&mut f, PassingMode::Borrow);
    let types = interner();
    let parameter = types.push(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let representation = types.push(ty(TypeKind::Record(
        vec![Field {
            name: "value".into(),
            ty: parameter,
            growable: false,
        }]
        .into(),
    )));
    f.signatures.insert(
        id("Holder"),
        Signature {
            definition: id("Holder"),
            metadata: Default::default(),
            kind: SignatureKind::Type(TypeDefinition {
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
                layout: Default::default(),
                distinct: false,
                resource,
                drop: None,
                fact: None,
                captures: None,
                destructor_allocation: Allocation::NoAlloc,
            }),
            fingerprint: ContentHash::default(),
        },
    );
    f.assign(destination, Rvalue::Use(copy(source)), 0);
    f
}

#[test]
fn structural_nominal_plain_copies_need_no_cleanup() {
    nominal_copy_fixture(TypeKind::Primitive(Primitive::U32), false).valid();
}

#[test]
fn structural_nominal_counted_copies_still_require_cleanup() {
    let mut f = nominal_copy_fixture(TypeKind::Primitive(Primitive::String), false);
    let cleanup = f.cleanup(vec![drop(2)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.valid();
    f.body().blocks[0].terminator = ret(0, 0);
    assert!(f.kinds().iter().any(|v| matches!(
        v,
        ViolationKind::CleanupMismatch { .. } | ViolationKind::LeakedSlot(_)
    )));
}

#[test]
fn structural_nominal_noncopyable_payloads_and_resources_reject_copy() {
    for (payload, resource) in [
        (
            TypeKind::Builtin {
                kind: BuiltinType::Iter,
                arguments: vec![].into(),
            },
            false,
        ),
        (TypeKind::Primitive(Primitive::U32), true),
    ] {
        let mut f = nominal_copy_fixture(payload, resource);
        let cleanup = f.cleanup(vec![drop(2)]);
        f.body().blocks[0].terminator = ret(0, cleanup);
        assert!(
            f.kinds().contains(&ViolationKind::InvalidUse(PlaceId(1))),
            "{:?}",
            f.kinds()
        );
    }
}

#[test]
fn opaque_nominal_properties_use_only_exported_bounds() {
    let mut f = nominal_copy_fixture(TypeKind::Primitive(Primitive::U32), false);
    f.signatures
        .get_mut(&path("Holder"))
        .unwrap()
        .metadata
        .visibility = hirsel_syntax::Visibility::PublicOpaque;
    // This fixture models the public client view, not the owner view.
    let SignatureKind::Type(d) = &mut f.signatures.get_mut(&path("Holder")).unwrap().kind else {
        panic!()
    };
    d.representation = None;
    let cleanup = f.cleanup(vec![drop(2)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    assert!(f.kinds().contains(&ViolationKind::InvalidUse(PlaceId(1))));
    let SignatureKind::Type(declaration) = &mut f.signatures.get_mut(&path("Holder")).unwrap().kind
    else {
        panic!()
    };
    declaration.bounds.push(Bound::Plain);
    f.body().blocks[0].terminator = ret(0, 0);
    f.valid();
}

#[test]
fn recursive_nominal_properties_check_changed_generic_arguments() {
    let mut f = nominal_copy_fixture(TypeKind::Primitive(Primitive::U32), false);
    let parameter = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let iterator = interner().intern(ty(TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(parameter)].into(),
    }));
    let recursive = interner().intern(ty(TypeKind::Named {
        definition: id("Holder"),
        arguments: vec![GenericArgument::Type(iterator)].into(),
    }));
    let boxed = interner().intern(ty(TypeKind::Box(recursive)));
    let representation = interner().intern(ty(TypeKind::Record(
        vec![
            Field {
                name: "value".into(),
                ty: parameter,
                growable: false,
            },
            Field {
                name: "next".into(),
                ty: boxed,
                growable: false,
            },
        ]
        .into(),
    )));
    let SignatureKind::Type(declaration) = &mut f.signatures.get_mut(id("Holder")).unwrap().kind
    else {
        panic!()
    };
    declaration.representation = Some(representation);
    let cleanup = f.cleanup(vec![drop(2)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    assert!(
        f.kinds().contains(&ViolationKind::InvalidUse(PlaceId(1))),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn finite_data_proofs_accept_many_distinct_nominal_instantiations() {
    let mut f = generic_variant_equality_fixture(true);
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U8)));
    let mut fields = Vec::new();
    for width in 0..130 {
        let payload = f
            .definition
            .types
            .push(ty(TypeKind::Tuple(vec![scalar; width].into())));
        fields.push(f.definition.types.push(ty(TypeKind::Named {
            definition: id("Choice"),
            arguments: vec![GenericArgument::Type(payload)].into(),
        })));
    }
    let root = f.definition.types.push(ty(TypeKind::Tuple(fields.into())));
    f.body().locals[1].ty = root;
    f.body().places[1].ty = root;
    let types = f.definition.types.clone();
    let owner = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(fun) = &mut owner.kind else {
        panic!()
    };
    fun.parameters[0].ty = root;
    f.valid();
}

fn packed_variant_fixture() -> (Fixture, PlaceId) {
    let mut f = generic_variant_equality_fixture(true);
    let nominal = f.body().locals[1].ty;
    let TypeKind::Named { arguments, .. } = interner().get(nominal).unwrap().kind.clone() else {
        panic!()
    };
    let GenericArgument::Type(payload) = arguments[0] else {
        panic!()
    };
    let packed_kind = TypeKind::Builtin {
        kind: BuiltinType::Packed,
        arguments: vec![GenericArgument::Type(nominal)].into(),
    };
    let packed = f.definition.types.push(ty(packed_kind.clone()));
    f.body().locals[1].ty = packed;
    f.body().places[1].ty = packed;
    let projected = PlaceId(f.body().places.len() as u32);
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(1),
        projection: vec![Projection::VariantField {
            variant: 0,
            field: 0,
        }]
        .into(),
        ty: payload,
    });
    let destination = f.slot(packed_kind, Storage::Temporary);
    f.assign(
        destination,
        Rvalue::Construct {
            constructor: Constructor::Variant {
                definition: path("Choice"),
                variant: 0,
            },
            fields: vec![ConstructField {
                field: 0,
                value: operand(projected.0, UseKind::Copy),
            }],
            destination: Destination::Inferred,
        },
        0,
    );
    let types = f.definition.types.clone();
    let owner = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(fun) = &mut owner.kind else {
        panic!()
    };
    fun.parameters[0].ty = packed;
    (f, projected)
}
#[test]
fn packed_variant_layout_retains_nominal_owner_and_payload_ordinals() {
    let (f, _) = packed_variant_fixture();
    f.valid();
    let (mut wrong_owner, _) = packed_variant_fixture();
    let Rvalue::Construct {
        constructor: Constructor::Variant { definition, .. },
        ..
    } = wrong_owner.body().rvalues.last_mut().unwrap()
    else {
        panic!()
    };
    *definition = path("Other");
    wrong_owner.broken(vec![ViolationKind::InvalidConstructor]);
    let (mut wrong_projection, projected) = packed_variant_fixture();
    wrong_projection.body().places[projected.0 as usize].projection[0] = Projection::VariantField {
        variant: 1,
        field: 0,
    };
    wrong_projection.broken(vec![ViolationKind::InvalidProjection(projected)]);
}

#[test]
fn packed_storage_does_not_admit_nonvariant_payload_layouts() {
    let (mut f, _) = packed_variant_fixture();
    let declaration = f.signatures.get_mut(&path("Choice")).unwrap();
    let parameter = interner().intern(ty(TypeKind::Parameter(TypeParameterRef::bound(
        TypeParamId(0),
    ))));
    let record = interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "value".into(),
            ty: parameter,
            growable: false,
        }]
        .into(),
    )));
    let SignatureKind::Type(td) = &mut declaration.kind else {
        panic!()
    };
    td.representation = Some(record);
    assert!(f.kinds().contains(&ViolationKind::InvalidConstructor));
}

#[test]
fn deep_unique_opcode_establishes_an_independent_root_without_ordinary_copy_strengthening() {
    for ordinary in [false, true] {
        let mut f = Fixture::string();
        let destination = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        let type_id = f.body().places[destination as usize].ty;
        let mut unique = interner().get(type_id).unwrap().clone();
        unique.modes.uniqueness = Uniqueness::Unique;
        let unique = f.definition.types.push(unique);
        f.body().locals[destination as usize].ty = unique;
        f.body().places[destination as usize].ty = unique;
        let cleanup = f.cleanup(vec![drop(destination), drop(1)]);
        f.assign(
            destination,
            if ordinary {
                Rvalue::Use(copy(1))
            } else {
                Rvalue::Unary {
                    op: hirsel_syntax::UnaryOp::Unique,
                    value: copy(1),
                }
            },
            cleanup,
        );
        f.body().blocks[0].terminator = ret(0, cleanup);
        if ordinary {
            f.broken(vec![ViolationKind::TypeMismatch {
                entity: "value modes",
            }]);
        } else {
            f.valid();
        }
    }
}

#[test]
fn deep_unique_opcode_cannot_move_an_aliased_source_or_change_its_value_type() {
    for wrong_type in [false, true] {
        let mut f = Fixture::string();
        let destination = f.slot(
            if wrong_type {
                TypeKind::Primitive(Primitive::Bytes)
            } else {
                TypeKind::Primitive(Primitive::String)
            },
            Storage::Temporary,
        );
        let cleanup = f.cleanup(vec![drop(destination), drop(1)]);
        f.assign(
            destination,
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value: if wrong_type { copy(1) } else { take(1) },
            },
            cleanup,
        );
        f.body().blocks[0].terminator = ret(0, cleanup);
        assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: if wrong_type {
                "unique result"
            } else {
                "unique extraction"
            }
        }));
    }
}

fn revise_comparison_place(f: &mut Fixture, place: u32, change: impl FnOnce(&mut Type)) {
    let body = &mut f.definition.bodies[0];
    let p = &body.places[place as usize];
    let root = p.root;
    let mut row = interner().get(p.ty).unwrap().clone();
    change(&mut row);
    let ty = f.definition.types.push(row);
    body.places[place as usize].ty = ty;
    body.locals[root.0 as usize].ty = ty;
}

fn comparison_fixture(op: BinaryOp, reversed: bool) -> Fixture {
    let mut f = Fixture::string();
    let right = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
    revise_comparison_place(&mut f, right, |t| t.modes.uniqueness = Uniqueness::Unique);
    let first = f.cleanup(vec![drop(1)]);
    f.assign(
        right,
        Rvalue::Constant(Constant::String("other".into())),
        first,
    );
    let cleanup = f.cleanup(vec![drop(right), drop(1)]);
    let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    let (left, right) = if reversed { (right, 1) } else { (1, right) };
    f.assign(
        result,
        Rvalue::Binary {
            op,
            left: copy(left),
            right: copy(right),
        },
        cleanup,
    );
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

/// Registers the registry Arena and binds an Arena-typed local to a source
/// origin, so a relation naming that origin names an arena (PROV-4).
pub(super) fn arena_binding(f: &mut Fixture) -> Origin {
    let local = arena_slot(f, Storage::Local);
    let origin = Origin {
        root: OriginRoot::Local(lexical(), hirsel_types::LocalId(90)),
        fields: vec![].into(),
    };
    f.body().places[local as usize].binding = Some(origin.clone());
    origin
}

#[test]
fn read_only_comparisons_set_aside_root_holder_and_identity_evidence() {
    for op in [
        BinaryOp::Eq,
        BinaryOp::Ne,
        BinaryOp::Lt,
        BinaryOp::Le,
        BinaryOp::Gt,
        BinaryOp::Ge,
    ] {
        for reversed in [false, true] {
            comparison_fixture(op, reversed).valid();
            let mut heap = comparison_fixture(op, reversed);
            revise_comparison_place(&mut heap, 2, |t| t.modes.relation = Relation::global());
            heap.valid();
            let mut f = comparison_fixture(op, reversed);
            revise_comparison_place(&mut f, 2, |t| t.modes.sendability = Sendability::Sendable);
            f.valid();
        }
    }
    // Identity and holder evidence are not operand type: a branded or local
    // operand compares with an unbranded global one (PROV-2, MODE-1).
    let origin = || Origin {
        root: OriginRoot::Local(lexical(), hirsel_types::LocalId(0)),
        fields: vec![].into(),
    };
    for place in [1, 2] {
        let mut f = comparison_fixture(BinaryOp::Eq, false);
        revise_comparison_place(&mut f, place, |t| {
            t.brand = Brand {
                alternatives: vec![origin()].into(),
            }
        });
        // The forged brand has no origin evidence; only the operand relation is under test.
        assert!(!f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "binary operands"
        }));
        let mut f = comparison_fixture(BinaryOp::Eq, false);
        revise_comparison_place(&mut f, place, |t| {
            t.modes.relation = Relation::environment()
        });
        assert!(!f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "binary operands"
        }));
    }
    // A read is the same wherever the blocks live: an arena-placed operand
    // compares with a heap one, as the source checker accepts.
    for place in [1, 2] {
        let mut f = comparison_fixture(BinaryOp::Eq, false);
        let arena = arena_binding(&mut f);
        revise_comparison_place(&mut f, place, |t| {
            t.modes.relation = t.modes.relation.union(&Relation::place(arena.clone()))
        });
        assert!(!f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "binary operands"
        }));
    }
    for difference in [0, 2] {
        let mut f = comparison_fixture(BinaryOp::Eq, false);
        // Sendability is derived for the unique String. Forge that proof
        // on the aliased operand to retain the unsupported-evidence negative.
        let place = if difference == 1 { 1 } else { 2 };
        revise_comparison_place(&mut f, place, |t| match difference {
            0 => t.modes.multiplicity = Multiplicity::Once,
            1 => t.modes.sendability = Sendability::Sendable,
            _ => t.drop_allocation = DropAllocation::MayAllocate,
        });
        assert!(
            f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "binary operands"
            }),
            "{difference}: {:?}",
            f.kinds()
        );
    }
    // Fresh content can carry proof under Aliased permission. A counted copy
    // cannot forge the same proof, even when its only consumer is comparison.
    let mut copied = comparison_fixture(BinaryOp::Eq, false);
    let alias = copied.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    copied.assign(alias, Rvalue::Use(copy(1)), 0);
    let share = copied.body().blocks[0].instructions.pop().unwrap();
    copied.body().blocks[0].instructions.insert(2, share);
    copied.emit(InstructionKind::Drop {
        place: PlaceId(alias),
        if_live: false,
    });
    assert!(!copied.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "sendable receipt"
    }));
    revise_comparison_place(&mut copied, alias, |t| {
        t.modes.sendability = Sendability::Sendable
    });
    assert!(copied.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "sendable receipt"
    }));
    let mut f = comparison_fixture(BinaryOp::Eq, false);
    revise_comparison_place(&mut f, 2, |t| {
        t.kind = TypeKind::Primitive(Primitive::Bytes)
    });
    f.body().rvalues[2] = Rvalue::Constant(Constant::Bytes(b"other".to_vec()));
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "binary operands",
    }]);
}

/// OWN-8: parts live in their owner's locality. Only the constructor field
/// relation is under test; the forged locality carries no source proof.
#[test]
fn constructed_parts_are_at_most_as_local_as_their_aggregate() {
    for (owner_local, part_local) in [(false, false), (true, false), (true, true), (false, true)] {
        let mut f = Fixture::string();
        if part_local {
            revise_comparison_place(&mut f, 1, |t| {
                t.modes.relation = Relation::environment();
                t.modes.sendability = Sendability::NotSendable;
            });
        }
        let declared = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::String)));
        let aggregate = f.slot(TypeKind::Tuple(vec![declared].into()), Storage::Temporary);
        if owner_local {
            revise_comparison_place(&mut f, aggregate, |t| {
                t.modes.relation = Relation::environment();
                t.modes.sendability = Sendability::NotSendable;
            });
        }
        f.assign(
            aggregate,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![ConstructField {
                    field: 0,
                    value: copy(1),
                }],
                destination: Destination::Inferred,
            },
            0,
        );
        let mismatch = f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "constructor field",
        });
        assert_eq!(
            mismatch,
            part_local && !owner_local,
            "owner local {owner_local}, part local {part_local}: {:?}",
            f.kinds()
        );
    }
}

/// PROV-1: a nested brand is identity evidence, not operand type, so tuples
/// whose elements carry different brands compare, as the source checker
/// accepts `make(r) == make_n(r)`.
#[test]
fn comparison_sets_aside_nested_brands() {
    for branded in [[true, false], [true, true]] {
        let mut f = Fixture::unit();
        let scalar = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
        f.assign(scalar, Rvalue::Constant(Constant::Integer(1)), 0);
        let plain = f.body().places[scalar as usize].ty;
        let children = [0u32, 1].map(|field| {
            if !branded[field as usize] {
                return plain;
            }
            let mut row = interner().get(plain).unwrap().clone();
            row.brand = Brand {
                alternatives: vec![Origin {
                    root: OriginRoot::Local(lexical(), hirsel_types::LocalId(scalar)),
                    fields: vec![hirsel_types::FieldId(field)].into(),
                }]
                .into(),
            };
            f.definition.types.push(row)
        });
        let [left, right] =
            children.map(|child| f.slot(TypeKind::Tuple(vec![child].into()), Storage::Temporary));
        for destination in [left, right] {
            f.assign(
                destination,
                Rvalue::Construct {
                    constructor: Constructor::Tuple,
                    fields: vec![ConstructField {
                        field: 0,
                        value: copy(scalar),
                    }],
                    destination: Destination::Inferred,
                },
                0,
            );
        }
        let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(
            result,
            Rvalue::Binary {
                op: BinaryOp::Eq,
                left: copy(left),
                right: copy(right),
            },
            0,
        );
        // The forged brands have no origin evidence; only operand typing is under test.
        assert!(
            !f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "binary operands"
            }),
            "{branded:?}: {:?}",
            f.kinds()
        );
    }
}

#[test]
fn comparison_nested_qualifiers_but_uniqueness_and_arithmetic_operands_remain_exact() {
    // A comparison only borrows, so a nested `unique` part compares with an
    // aliased one (MODE-2); another nested qualifier stays exact.
    let (mut f, scalar, result) = nested_comparison(|t| t.modes.uniqueness = Uniqueness::Unique);
    f.valid();
    let (allocating, _, _) = nested_comparison(|t| t.drop_allocation = DropAllocation::MayAllocate);
    // The forged cleanup also fails the tuple's construction; only the
    // comparison is under test.
    assert_eq!(
        allocating.kinds(),
        vec![
            ViolationKind::TypeMismatch {
                entity: "constructor field"
            },
            ViolationKind::TypeMismatch {
                entity: "binary operands"
            }
        ]
    );
    revise_comparison_place(&mut f, result, |t| {
        t.kind = TypeKind::Primitive(Primitive::U8)
    });
    let unique_scalar = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    revise_comparison_place(&mut f, unique_scalar, |t| {
        t.modes.uniqueness = Uniqueness::Unique
    });
    // Initialize before the arithmetic instruction, preserving operand availability.
    let comparison = f.body().blocks[0].instructions.pop().unwrap();
    f.assign(unique_scalar, Rvalue::Constant(Constant::Integer(2)), 0);
    f.body().blocks[0].instructions.push(comparison);
    f.body().rvalues[4] = Rvalue::Binary {
        op: BinaryOp::Add,
        left: copy(scalar),
        right: copy(unique_scalar),
    };
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "binary operands",
    }]);
}

/// Compares `(u8,)` with a tuple whose part is the `u8` row as `change` revises
/// it, returning the fixture, the scalar slot and the result slot.
fn nested_comparison(change: impl FnOnce(&mut Type)) -> (Fixture, u32, u32) {
    let mut f = Fixture::unit();
    let scalar = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    f.assign(scalar, Rvalue::Constant(Constant::Integer(1)), 0);
    let left_child = f.body().places[scalar as usize].ty;
    let mut changed = interner().get(left_child).unwrap().clone();
    change(&mut changed);
    let right_child = f.definition.types.push(changed);
    let left = f.slot(TypeKind::Tuple(vec![left_child].into()), Storage::Temporary);
    let right = f.slot(
        TypeKind::Tuple(vec![right_child].into()),
        Storage::Temporary,
    );
    for destination in [left, right] {
        f.assign(
            destination,
            Rvalue::Construct {
                constructor: Constructor::Tuple,
                fields: vec![ConstructField {
                    field: 0,
                    value: copy(scalar),
                }],
                destination: Destination::Inferred,
            },
            0,
        );
    }
    let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(
        result,
        Rvalue::Binary {
            op: BinaryOp::Eq,
            left: copy(left),
            right: copy(right),
        },
        0,
    );
    (f, scalar, result)
}

fn function_authority_flow_fixture(
    expected_authority: AuthoritySummary,
    actual_authority: AuthoritySummary,
    expected_uses: Uses,
    actual_uses: Uses,
    flow: u8,
) -> Fixture {
    let mut f = Fixture::unit();
    let unit = f.body().result;
    let callback = |authority, uses| FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: unit,
        error: None,
        allocation: Allocation::NoAlloc,
        authority,
        uses,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let expected_kind = TypeKind::Function(callback(expected_authority, expected_uses));
    let actual_kind = TypeKind::Function(callback(actual_authority, actual_uses));
    value_flow_fixture(f, expected_kind, actual_kind, flow)
}

const VALUE_FLOWS: [&str; 4] = ["return", "assignment", "call argument", "constructor field"];

/// A taken parameter of `actual_kind` flows into a place of `expected_kind`
/// through `VALUE_FLOWS[flow]`.
fn value_flow_fixture(
    mut f: Fixture,
    expected_kind: TypeKind,
    actual_kind: TypeKind,
    flow: u8,
) -> Fixture {
    let unit = f.body().result;
    let source = f.slot(actual_kind, Storage::Parameter);
    f.body().parameters.push(LocalId(source));
    let expected = f.definition.types.push(ty(expected_kind.clone()));
    match flow {
        0 => {
            f.body().result = expected;
            f.body().blocks[0].terminator = ret(source, 0);
        }
        1 => {
            let destination = f.slot(expected_kind, Storage::Temporary);
            f.assign(destination, Rvalue::Use(take(source)), 0);
            f.body().result = expected;
            f.body().blocks[0].terminator = ret(destination, 0);
        }
        2 => {
            let mut signature =
                function_signature("callee", vec![], TypeKind::Tuple(vec![].into()));
            let SignatureKind::Function(callee) = &mut signature.kind else {
                panic!()
            };
            callee.parameters.push(Parameter {
                name: None,
                label: None,
                passing: PassingMode::Take,
                ty: expected,
            });
            callee.result = unit;
            f.signatures.insert(signature.definition.clone(), signature);
            f.body().blocks[0].terminator = Terminator::Call {
                callee: Callee::Definition {
                    path: path("callee"),
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: take(source),
                }],
                returns: Some(CallReturn {
                    destination: PlaceId(0),
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            };
            f.body().blocks[0].instructions.clear();
            f.body().rvalues.clear();
            f.body().blocks.push(block(vec![], ret(0, 0)));
        }
        _ => {
            let destination = f.slot(TypeKind::Tuple(vec![expected].into()), Storage::Temporary);
            let cleanup = f.cleanup(vec![drop(source)]);
            f.assign(
                destination,
                Rvalue::Construct {
                    constructor: Constructor::Tuple,
                    destination: Destination::Inferred,
                    fields: vec![ConstructField {
                        field: 0,
                        value: take(source),
                    }],
                },
                cleanup,
            );
            f.body().result = f.body().places[destination as usize].ty;
            f.body().blocks[0].terminator = ret(destination, 0);
        }
    }
    owner_signature(&mut f, PassingMode::Take);
    f
}

#[test]
fn function_values_satisfy_broader_authority_promises_at_each_value_flow() {
    let dir = AuthorityTerm::Capability(path("Dir"));
    let stdout = AuthorityTerm::Capability(path("Stdout"));
    let smaller = AuthoritySummary::Known(vec![dir.clone()].into());
    let larger = AuthoritySummary::Known(vec![dir, stdout].into());
    for flow in 0..4 {
        function_authority_flow_fixture(
            AuthoritySummary::Unknown,
            AuthoritySummary::inert(),
            Uses::Unrestricted,
            Uses::Unrestricted,
            flow,
        )
        .valid();
        function_authority_flow_fixture(
            larger.clone(),
            smaller.clone(),
            Uses::Only(larger.clone()),
            Uses::Only(smaller.clone()),
            flow,
        )
        .valid();
    }
}

#[test]
fn function_values_cannot_widen_authority_or_uses_restrictions() {
    let dir = AuthorityTerm::Capability(path("Dir"));
    let stdout = AuthorityTerm::Capability(path("Stdout"));
    let smaller = AuthoritySummary::Known(vec![dir.clone()].into());
    let larger = AuthoritySummary::Known(vec![dir, stdout].into());
    for flow in 0..4 {
        for (expected, actual, expected_uses, actual_uses) in [
            (
                smaller.clone(),
                larger.clone(),
                Uses::Only(smaller.clone()),
                Uses::Only(larger.clone()),
            ),
            (
                smaller.clone(),
                AuthoritySummary::Unknown,
                Uses::Only(smaller.clone()),
                Uses::Unrestricted,
            ),
            (
                AuthoritySummary::inert(),
                smaller.clone(),
                Uses::Unrestricted,
                Uses::Unrestricted,
            ),
            (
                AuthoritySummary::Unknown,
                AuthoritySummary::inert(),
                Uses::Only(AuthoritySummary::inert()),
                Uses::Unrestricted,
            ),
            (
                AuthoritySummary::Unknown,
                AuthoritySummary::inert(),
                Uses::Only(smaller.clone()),
                Uses::Only(larger.clone()),
            ),
        ] {
            let f =
                function_authority_flow_fixture(expected, actual, expected_uses, actual_uses, flow);
            assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: VALUE_FLOWS[flow as usize]
            }));
        }
    }
}

/// CLO-2 (#58) at depth: one subsumption judges a callable at the root of a
/// received value and in every reading position below it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Nested {
    Stronger,
    Weaker,
    Neither,
}

/// Each shape holds `callback` once; the tag says which callback promise the
/// expected type may carry there relative to the value's.
fn nested_callback_shapes(f: &mut Fixture, narrow: bool) -> Vec<(&'static str, Nested, TypeKind)> {
    let unit = f.body().result;
    let function = |parameters: Vec<(PassingMode, TypeId)>, result, uses: Uses| FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters
            .into_iter()
            .map(|(passing, ty)| Parameter {
                name: None,
                label: None,
                passing,
                ty,
            })
            .collect(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: match &uses {
            Uses::Only(authority) => authority.clone(),
            Uses::Unrestricted => AuthoritySummary::Unknown,
        },
        uses,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let callback = f.definition.types.push(ty(TypeKind::Function(function(
        vec![],
        unit,
        if narrow {
            Uses::Only(AuthoritySummary::inert())
        } else {
            Uses::Unrestricted
        },
    ))));
    let holder =
        |parameters, result| TypeKind::Function(function(parameters, result, Uses::Unrestricted));
    let returns = holder(vec![], callback);
    let borrows = holder(vec![(PassingMode::Borrow, callback)], unit);
    let takes = holder(vec![(PassingMode::Take, callback)], unit);
    let writes = holder(vec![(PassingMode::Mut, callback)], unit);
    let returned = f.definition.types.push(ty(returns.clone()));
    let borrowed = f.definition.types.push(ty(borrows.clone()));
    let returns_consumer = holder(vec![], borrowed);
    let consumes_consumer = holder(vec![(PassingMode::Borrow, borrowed)], unit);
    let consumes_producer = holder(vec![(PassingMode::Borrow, returned)], unit);
    let sequence = |kind| TypeKind::Builtin {
        kind,
        arguments: vec![GenericArgument::Type(callback)].into(),
    };
    vec![
        (
            "tuple part",
            Nested::Weaker,
            TypeKind::Tuple(vec![callback].into()),
        ),
        ("box payload", Nested::Weaker, TypeKind::Box(callback)),
        ("list element", Nested::Weaker, sequence(BuiltinType::List)),
        (
            "tuple of tuple",
            Nested::Weaker,
            TypeKind::Tuple(vec![returned].into()),
        ),
        ("result", Nested::Weaker, returns),
        ("borrowed parameter", Nested::Stronger, borrows),
        ("taken parameter", Nested::Stronger, takes),
        ("mut parameter", Nested::Neither, writes),
        ("result's parameter", Nested::Stronger, returns_consumer),
        ("parameter's parameter", Nested::Weaker, consumes_consumer),
        ("parameter's result", Nested::Stronger, consumes_producer),
        ("set element", Nested::Neither, sequence(BuiltinType::Set)),
    ]
}

#[test]
fn nested_callables_subsume_by_position_at_each_value_flow() {
    for flow in 0..4 {
        let entity = VALUE_FLOWS[flow as usize];
        let mismatch = ViolationKind::TypeMismatch { entity };
        let shapes = nested_callback_shapes(&mut Fixture::unit(), true).len();
        for shape in 0..shapes {
            // The value promises `uses {}` where the place leaves uses open,
            // then the reverse; a twin of equal promises is always received.
            for (value_narrow, place_narrow) in
                [(true, false), (false, true), (true, true), (false, false)]
            {
                let mut f = Fixture::unit();
                let (name, nested, actual) =
                    nested_callback_shapes(&mut f, value_narrow).swap_remove(shape);
                let (_, _, expected) =
                    nested_callback_shapes(&mut f, place_narrow).swap_remove(shape);
                let f = value_flow_fixture(f, expected, actual, flow);
                let received = value_narrow == place_narrow
                    || nested
                        == if value_narrow {
                            Nested::Weaker
                        } else {
                            Nested::Stronger
                        };
                assert_eq!(
                    !f.kinds().contains(&mismatch),
                    received,
                    "{entity}: {name}, value narrow {value_narrow}, place narrow {place_narrow}: {:?}",
                    f.kinds()
                );
            }
        }
    }
}

#[test]
fn a_mut_argument_keeps_nested_callable_promises_exact() {
    for (value_narrow, place_narrow) in [(true, true), (true, false), (false, true)] {
        let mut f = Fixture::unit();
        let (_, _, actual) = nested_callback_shapes(&mut f, value_narrow).swap_remove(0);
        let (_, _, expected) = nested_callback_shapes(&mut f, place_narrow).swap_remove(0);
        let mut f = value_flow_fixture(f, expected, actual, 2);
        let SignatureKind::Function(callee) =
            &mut f.signatures.get_mut(&path("callee")).unwrap().kind
        else {
            panic!()
        };
        callee.parameters[0].passing = PassingMode::Mut;
        let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
            panic!()
        };
        arguments[0].value.usage = UseKind::Mut;
        assert_eq!(
            f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "call argument"
            }),
            value_narrow != place_narrow,
            "{:?}",
            f.kinds()
        );
    }
}

/// CLO-2: a `mut` argument is written back through its parameter, so its
/// own callable promise is the parameter's exactly. Storage typed `uses {}`
/// handed to an open-`uses` parameter would come back holding authority and
/// still be read as `uses {}`; a borrowed argument only weakens.
#[test]
fn a_mut_argument_keeps_its_root_callable_promise_exact() {
    for passing in [PassingMode::Mut, PassingMode::Borrow] {
        for (value_narrow, place_narrow) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let mut f = Fixture::unit();
            let root = |f: &mut Fixture, narrow| {
                let (_, _, holder) = nested_callback_shapes(f, narrow).swap_remove(0);
                let TypeKind::Tuple(parts) = holder else {
                    panic!()
                };
                interner().get(parts[0]).unwrap().kind.clone()
            };
            let actual = root(&mut f, value_narrow);
            let expected = root(&mut f, place_narrow);
            let mut f = value_flow_fixture(f, expected, actual, 2);
            if passing == PassingMode::Mut {
                let SignatureKind::Function(callee) =
                    &mut f.signatures.get_mut(&path("callee")).unwrap().kind
                else {
                    panic!()
                };
                callee.parameters[0].passing = PassingMode::Mut;
                let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
                    panic!()
                };
                arguments[0].value.usage = UseKind::Mut;
            }
            let rejected = match passing {
                PassingMode::Mut => value_narrow != place_narrow,
                _ => !value_narrow && place_narrow,
            };
            assert_eq!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
                rejected,
                "{passing:?} {value_narrow} {place_narrow}: {:?}",
                f.kinds()
            );
        }
    }
}

/// CLO-2/MEM-5: a `mut` argument's own erased state and cleanup summary
/// are the parameter's exactly, as its callable promise is: storage whose
/// state held no authority, or whose destruction could not allocate, would
/// come back holding a state the parameter admits. A borrowed argument only
/// weakens. A parameter declared at the argument's own captures (an
/// advance-only core operation, spec #91) receives any state exactly.
#[test]
fn a_mut_argument_keeps_its_root_erased_state_and_cleanup_summary_exact() {
    #[derive(Clone, Copy, Debug)]
    enum Fact {
        Captures,
        Cleanup,
        OwnCaptures,
    }
    let strengthen = |row: &mut Type, fact: Fact, strong: bool, place: bool| match fact {
        Fact::Captures => {
            row.erased_captures = strong.then(|| ErasedCaptures::Known(AuthoritySummary::inert()))
        }
        Fact::OwnCaptures => {
            row.erased_captures = if place {
                Some(ErasedCaptures::FromArguments(
                    vec![ParameterRef::bound(ParamId(0)).into()].into(),
                ))
            } else {
                strong.then(|| ErasedCaptures::Known(AuthoritySummary::inert()))
            }
        }
        Fact::Cleanup => {
            row.drop_allocation = if strong {
                DropAllocation::NoAlloc
            } else {
                DropAllocation::MayAllocate
            }
        }
    };
    for fact in [Fact::Captures, Fact::Cleanup, Fact::OwnCaptures] {
        for passing in [PassingMode::Mut, PassingMode::Borrow] {
            for (value_strong, place_strong) in
                [(true, true), (true, false), (false, true), (false, false)]
            {
                let f = Fixture::unit();
                let state = TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    arguments: vec![GenericArgument::Type(f.definition.types.id(0).unwrap())]
                        .into(),
                };
                let mut f = value_flow_fixture(f, state.clone(), state, 2);
                let source = f.body().parameters[0];
                let held = f.body().locals[source.0 as usize].ty;
                let mut row = interner().get(held).unwrap().clone();
                strengthen(&mut row, fact, value_strong, false);
                let actual = f.definition.types.push(row);
                f.body().locals[source.0 as usize].ty = actual;
                for place in &mut f.body().places {
                    if place.root == source {
                        place.ty = actual;
                    }
                }
                owner_signature(&mut f, PassingMode::Take);
                let signature = f.signatures.get_mut(&path("callee")).unwrap();
                let SignatureKind::Function(callee) = &mut signature.kind else {
                    panic!()
                };
                let mut row = interner().get(callee.parameters[0].ty).unwrap().clone();
                strengthen(&mut row, fact, place_strong, true);
                callee.parameters[0].ty = interner().intern(row);
                callee.parameters[0].passing = passing;
                let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
                    panic!()
                };
                arguments[0].value.usage = match passing {
                    PassingMode::Mut => UseKind::Mut,
                    _ => UseKind::Borrow,
                };
                let rejected = match (fact, passing) {
                    (Fact::OwnCaptures, _) => false,
                    (_, PassingMode::Mut) => value_strong != place_strong,
                    _ => !value_strong && place_strong,
                };
                assert_eq!(
                    f.kinds().contains(&ViolationKind::TypeMismatch {
                        entity: "call argument"
                    }),
                    rejected,
                    "{fact:?} {passing:?} {value_strong} {place_strong}: {:?}",
                    f.kinds()
                );
            }
        }
    }
}

/// MODE-4: a `mut` argument's own crossing mode is the parameter's exactly.
/// A callable's sendability is evidence of what it holds: sendable storage
/// handed to a parameter that is not would come back holding what crosses no
/// task, still typed as sendable. A borrowed argument only weakens.
#[test]
fn a_mut_argument_keeps_its_root_crossing_mode_exact() {
    for passing in [PassingMode::Mut, PassingMode::Borrow] {
        for (value_sendable, place_sendable) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let mut f = Fixture::unit();
            let (_, _, holder) = nested_callback_shapes(&mut f, false).swap_remove(0);
            let TypeKind::Tuple(parts) = holder else {
                panic!()
            };
            let callable = interner().get(parts[0]).unwrap().kind.clone();
            let mut f = value_flow_fixture(f, callable.clone(), callable, 2);
            let crossing = |sendable| {
                if sendable {
                    Sendability::Sendable
                } else {
                    Sendability::NotSendable
                }
            };
            let source = f.body().parameters[0];
            let held = f.body().locals[source.0 as usize].ty;
            let mut row = interner().get(held).unwrap().clone();
            row.modes.sendability = crossing(value_sendable);
            let actual = f.definition.types.push(row);
            f.body().locals[source.0 as usize].ty = actual;
            for place in &mut f.body().places {
                if place.root == source {
                    place.ty = actual;
                }
            }
            owner_signature(&mut f, PassingMode::Take);
            let signature = f.signatures.get_mut(&path("callee")).unwrap();
            let SignatureKind::Function(callee) = &mut signature.kind else {
                panic!()
            };
            let mut row = interner().get(callee.parameters[0].ty).unwrap().clone();
            row.modes.sendability = crossing(place_sendable);
            callee.parameters[0].ty = interner().intern(row);
            callee.parameters[0].passing = passing;
            let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
                panic!()
            };
            arguments[0].value.usage = match passing {
                PassingMode::Mut => UseKind::Mut,
                _ => UseKind::Borrow,
            };
            let rejected = match passing {
                PassingMode::Mut => value_sendable != place_sendable,
                _ => !value_sendable && place_sendable,
            };
            assert_eq!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
                rejected,
                "{passing:?} {value_sendable} {place_sendable}: {:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn a_mut_iterator_argument_keeps_its_capture_sendability_exact() {
    for passing in [PassingMode::Mut, PassingMode::Borrow] {
        for (value_sendable, place_sendable) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let mut f = Fixture::unit();
            let element = f
                .definition
                .types
                .push(ty(TypeKind::Primitive(Primitive::U64)));
            let callable = TypeKind::Builtin {
                kind: BuiltinType::Iter,
                arguments: vec![GenericArgument::Type(element)].into(),
            };
            let mut f = value_flow_fixture(f, callable.clone(), callable, 2);
            let crossing = |sendable| {
                if sendable {
                    Sendability::Sendable
                } else {
                    Sendability::NotSendable
                }
            };
            let source = f.body().parameters[0];
            let held = f.body().locals[source.0 as usize].ty;
            let mut row = interner().get(held).unwrap().clone();
            row.modes.sendability = crossing(value_sendable);
            let actual = f.definition.types.push(row);
            f.body().locals[source.0 as usize].ty = actual;
            for place in &mut f.body().places {
                if place.root == source {
                    place.ty = actual;
                }
            }
            owner_signature(&mut f, PassingMode::Take);
            let signature = f.signatures.get_mut(&path("callee")).unwrap();
            let SignatureKind::Function(callee) = &mut signature.kind else {
                panic!()
            };
            let mut row = interner().get(callee.parameters[0].ty).unwrap().clone();
            row.modes.sendability = crossing(place_sendable);
            callee.parameters[0].ty = interner().intern(row);
            callee.parameters[0].passing = passing;
            let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
                panic!()
            };
            arguments[0].value.usage = match passing {
                PassingMode::Mut => UseKind::Mut,
                _ => UseKind::Borrow,
            };
            // The owned iterator is lent under a real loan, and both call
            // exits end that loan before destroying the caller's iterator.
            let root_place = f
                .body()
                .places
                .iter()
                .position(|p| p.root == source)
                .unwrap();
            let mut loaned = f.body().places[root_place].clone();
            loaned.loan = Some(LoanId(0));
            let loaned_place = PlaceId(f.body().places.len() as u32);
            f.body().places.push(loaned);
            let drop_source = f.cleanup(vec![drop(root_place as u32)]);
            let cleanup = f.cleanup(vec![
                CleanupAction::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                drop(root_place as u32),
            ]);
            f.body().blocks[0].instructions.push(inst(
                InstructionKind::BeginLoan {
                    id: LoanId(0),
                    place: PlaceId(root_place as u32),
                    mutable: passing == PassingMode::Mut,
                    if_inactive: false,
                },
                drop_source,
            ));
            let Terminator::Call {
                arguments,
                on_failure,
                ..
            } = &mut f.body().blocks[0].terminator
            else {
                panic!()
            };
            arguments[0].value.place = loaned_place;
            *on_failure = CleanupId(cleanup);
            f.body().blocks[1].terminator = ret(0, cleanup);
            let rejected = match passing {
                PassingMode::Mut => value_sendable != place_sendable,
                _ => !value_sendable && place_sendable,
            };
            if rejected {
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
}

/// MODE-1: an owned value whose content flow proves Global is received by
/// Global storage although a nested row is written local to an environment
/// (a capture-free closure made inside a called closure), at a return, an
/// assignment and a constructor field as at a call argument.
#[test]
fn proven_global_content_is_acquired_at_depth_at_each_storage_flow() {
    for flow in [0, 1, 3] {
        for proven in [true, false] {
            let mut f = Fixture::unit();
            let unit = f.body().result;
            let callback = FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: vec![].into(),
                result: unit,
                error: None,
                allocation: Allocation::NoAlloc,
                authority: AuthoritySummary::inert(),
                uses: Uses::Only(AuthoritySummary::inert()),
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            };
            let global = f
                .definition
                .types
                .push(ty(TypeKind::Function(callback.clone())));
            let mut scoped = ty(TypeKind::Function(callback));
            scoped.modes.relation = Relation::environment();
            let scoped = f.definition.types.push(scoped);
            let mut f = value_flow_fixture(
                f,
                TypeKind::Tuple(vec![global].into()),
                TypeKind::Tuple(vec![scoped].into()),
                flow,
            );
            if !proven {
                // A borrowed parameter is the caller's: this frame does not
                // own its content, so nothing proves what it holds Global.
                owner_signature(&mut f, PassingMode::Borrow);
                let source = PlaceId(f.body().parameters[0].0);
                for rvalue in &mut f.body().rvalues {
                    match rvalue {
                        Rvalue::Use(value) if value.place == source => value.usage = UseKind::Copy,
                        Rvalue::Construct { fields, .. } => fields[0].value.usage = UseKind::Copy,
                        _ => {}
                    }
                }
                if let Terminator::Return { value, .. } = &mut f.body().blocks[0].terminator {
                    if value.place == source {
                        value.usage = UseKind::Copy;
                    }
                }
            }
            let mismatch = ViolationKind::TypeMismatch {
                entity: VALUE_FLOWS[flow as usize],
            };
            assert_eq!(
                !f.kinds().contains(&mismatch),
                proven,
                "{}: proven {proven}: {:?}",
                VALUE_FLOWS[flow as usize],
                f.kinds()
            );
        }
    }
}

#[test]
fn callable_allocation_promises_weaken_only_from_noalloc_to_may_allocate() {
    for reverse in [false, true] {
        let mut f = Fixture::unit();
        let unit = f.body().result;
        let callback = |allocation| FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result: unit,
            error: None,
            allocation,
            authority: AuthoritySummary::inert(),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        };
        let source = f.slot(
            TypeKind::Function(callback(if reverse {
                Allocation::MayAllocate
            } else {
                Allocation::NoAlloc
            })),
            Storage::Parameter,
        );
        f.body().parameters.push(LocalId(source));
        f.body().result = f
            .definition
            .types
            .push(ty(TypeKind::Function(callback(if reverse {
                Allocation::NoAlloc
            } else {
                Allocation::MayAllocate
            }))));
        f.body().blocks[0].terminator = ret(source, 0);
        owner_signature(&mut f, PassingMode::Take);
        if reverse {
            f.broken(vec![ViolationKind::TypeMismatch { entity: "return" }]);
        } else {
            f.valid();
        }
    }
}

fn size_scope(f: &mut Fixture) {
    owner_signature(f, PassingMode::Borrow);
    let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    owner.generics = vec![
        GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Plain].into(),
        },
        GenericParameter::Size { name: "n".into() },
    ]
    .into();
}

#[test]
fn size_reads_require_a_declared_size_parameter_and_u64_result() {
    for difference in 0..5 {
        let mut f = Fixture::unit();
        let result = f.slot(
            TypeKind::Primitive(if difference == 3 {
                Primitive::U32
            } else {
                Primitive::U64
            }),
            Storage::Temporary,
        );
        let parameter = match difference {
            1 => 0,
            2 => 2,
            _ => 1,
        };
        f.assign(
            result,
            Rvalue::Size(SizeParameterRef::body(
                scoped(NameSite::Item),
                SizeParamId(parameter),
            )),
            0,
        );
        size_scope(&mut f);
        if difference == 4 {
            f.signatures.clear();
        }
        if difference == 0 {
            f.valid();
        } else {
            let expected = if difference == 3 {
                ViolationKind::TypeMismatch {
                    entity: "size read",
                }
            } else {
                ViolationKind::InvalidId {
                    arena: "size parameter",
                    index: parameter,
                }
            };
            assert!(
                f.kinds().contains(&expected),
                "{difference}: {:?}",
                f.kinds()
            );
        }
    }
}

#[test]
fn size_reads_inherit_lexical_scope_but_nested_generics_bind_fresh_ordinals() {
    let mut f = closure_fixture();
    size_scope(&mut f);
    f.definition.bodies[1].parameter_scope = scoped(NameSite::Statement(hirsel_syntax::StmtId(1)));
    let nested = &mut f.definition.bodies[1];
    let ty = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    nested.locals.push(Local {
        ty,
        storage: Storage::Temporary,
        scope: ScopeId(0),
        span: Span::default(),
    });
    nested.places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(1),
        projection: vec![].into(),
        ty,
    });
    nested.rvalues.push(Rvalue::Size(SizeParameterRef::body(
        scoped(NameSite::Item),
        SizeParamId(1),
    )));
    nested.blocks[0].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(1),
            value: RvalueId(0),
        },
        0,
    ));
    f.valid();
    let body = &mut f.definition.bodies[0];
    let closure = body
        .places
        .iter()
        .position(|p| matches!(interner().get(p.ty).unwrap().kind, TypeKind::Function(_)))
        .unwrap();
    let mut row = interner().get(body.places[closure].ty).unwrap().clone();
    let TypeKind::Function(function) = &mut row.kind else {
        unreachable!()
    };
    function.generics = vec![GenericParameter::Type {
        name: "Inner".into(),
        bounds: vec![].into(),
    }]
    .into();
    let ty = f.definition.types.push(row);
    body.locals[body.places[closure].root.0 as usize].ty = ty;
    body.places[closure].ty = ty;
    f.valid(); // Adding an inner binder cannot hide an explicitly captured outer size.
    f.definition.bodies[1].rvalues[0] = Rvalue::Size(SizeParameterRef::body(
        scoped(NameSite::Statement(hirsel_syntax::StmtId(1))),
        SizeParamId(1),
    ));
    assert!(f.kinds().contains(&ViolationKind::InvalidId {
        arena: "size parameter",
        index: 1
    }));
}

#[test]
fn forwarded_size_arguments_require_the_callers_size_scope() {
    let mut f = Fixture::unit();
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let mut callee = function_signature("sized", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(function) = &mut callee.kind else {
        unreachable!()
    };
    function.generics = vec![GenericParameter::Size { name: "n".into() }].into();
    f.signatures.insert(id("sized"), callee);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("sized"),
            arguments: vec![GenericArgument::Size(Size::Parameter(
                SizeParameterRef::body(scoped(NameSite::Item), SizeParamId(1)),
            ))],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(result, 0)));
    size_scope(&mut f);
    f.valid();
    f.signatures.remove(&path("main"));
    assert!(f.kinds().contains(&ViolationKind::InvalidId {
        arena: "size parameter",
        index: 1
    }));
}

#[test]
fn closure_parameter_origins_must_name_the_nested_bodys_scope() {
    let mut f = closure_fixture();
    let scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(7)));
    let mut parameter = ty(TypeKind::Primitive(Primitive::U32));
    parameter.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    });
    let nested = &mut f.definition.bodies[1];
    nested.parameter_scope = scope;
    let nested_ty = f.definition.types.push(parameter.clone());
    let local = LocalId(nested.locals.len() as u32);
    nested.locals.push(Local {
        ty: nested_ty,
        storage: Storage::Parameter,
        scope: ScopeId(0),
        span: Span::default(),
    });
    nested.parameters.push(local);
    nested.places.push(Place {
        loan: None,
        binding: None,
        root: local,
        projection: vec![].into(),
        ty: nested_ty,
    });
    parameter.brand.alternatives[0].root = OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
    let parameter = f.definition.types.push(parameter);
    let (old, mut callable) = f
        .definition
        .types
        .iter()
        .map(|&id| (id, interner().row(id).clone()))
        .find(|(_, t)| matches!(t.kind, TypeKind::Function(_)))
        .unwrap();
    let TypeKind::Function(fun) = &mut callable.kind else {
        panic!()
    };
    fun.parameters.push(Parameter {
        name: None,
        label: None,
        passing: PassingMode::Borrow,
        ty: parameter,
    });
    let new = f.definition.types.push(callable);
    for local in &mut f.body().locals {
        if local.ty == old {
            local.ty = new;
        }
    }
    for place in &mut f.body().places {
        if place.ty == old {
            place.ty = new;
        }
    }
    f.valid();
    f.definition.bodies[1].parameter_scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(8)));
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "closure parameter",
    }]);
}

#[test]
fn owner_origin_references_cannot_forge_a_different_declaring_scope() {
    for nested in [false, true] {
        let mut f = owner_origin_fixture(nested);
        f.valid();
        // Another definition's item binder.
        f.body().parameter_scope =
            hirsel_syntax::LexicalOwner::first(&path("other")).site(NameSite::Item);
        f.broken(vec![
            ViolationKind::TypeMismatch {
                entity: "body parameter",
            },
            ViolationKind::TypeMismatch {
                entity: "body result",
            },
        ]);
    }
}

#[test]
fn local_result_source_is_checked_independently_of_matching_types() {
    let mut f = Fixture::unit();
    let a = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let b = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let mut local = ty(TypeKind::Primitive(Primitive::Bytes));
    local.modes.relation = Relation::environment();
    let local = f.definition.types.push(local);
    for p in [a, b] {
        f.body().locals[p as usize].ty = local;
        f.body().places[p as usize].ty = local;
        f.body().parameters.push(LocalId(p));
    }
    let mut result = interner().get(local).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    let result = f.definition.types.push(result);
    f.body().result = result;
    owner_signature(&mut f, PassingMode::Borrow);
    let scope = f.body().parameter_scope;
    f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(a),
        cleanup: CleanupId(0),
    };
    f.valid();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(b),
        cleanup: CleanupId(0),
    };
    f.broken(vec![ViolationKind::InvalidResultSource(PlaceId(b))]);

    let destination = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
    f.body().locals[destination as usize].ty = local;
    f.body().places[destination as usize].ty = local;
    let mut callee = f.signatures.get(id("main")).unwrap().clone();
    callee.definition = id("callee");
    f.signatures.insert(id("callee"), callee);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![],
        },
        arguments: vec![
            CallArgument {
                parameter: ParamId(1),
                value: operand(b, UseKind::Borrow),
            },
            CallArgument {
                parameter: ParamId(0),
                value: operand(a, UseKind::Borrow),
            },
        ],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(destination, 0)));
    let report = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(report.violations.is_empty(), "{report:?}");
    let callee = f.signatures.get_mut(&path("callee")).unwrap();
    let SignatureKind::Function(fun) = &mut callee.kind else {
        unreachable!()
    };
    let mut result = interner().get(fun.result).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(1))]);
    fun.result = interner().intern(result);
    f.broken(vec![ViolationKind::InvalidResultSource(PlaceId(
        destination,
    ))]);
}

#[test]
fn alternative_loan_expiry_tracks_the_selected_value_across_a_join() {
    let mut f = Fixture::unit();
    let a = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let b = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    let result = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
    let mut local = ty(TypeKind::Primitive(Primitive::Bytes));
    local.modes.relation = Relation::environment();
    let local = f.definition.types.push(local);
    for p in [a, b, result] {
        f.body().locals[p as usize].ty = local;
        f.body().places[p as usize].ty = local;
    }
    f.body().parameters = vec![LocalId(a), LocalId(b), LocalId(condition)];
    f.body().result = local;
    owner_signature(&mut f, PassingMode::Borrow);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(condition),
        yes: BlockId(1),
        no: BlockId(2),
    };
    for (loan, source) in [(0, a), (1, b)] {
        let access = f.through_loan(source, loan);
        let value = RvalueId(f.body().rvalues.len() as u32);
        f.body()
            .rvalues
            .push(Rvalue::Use(operand(access, UseKind::Borrow)));
        f.body().blocks.push(block(
            vec![
                inst(
                    InstructionKind::BeginLoan {
                        id: LoanId(loan),
                        place: PlaceId(source),
                        mutable: false,
                        if_inactive: false,
                    },
                    0,
                ),
                inst(
                    InstructionKind::Assign {
                        destination: PlaceId(result),
                        value,
                    },
                    0,
                ),
            ],
            Terminator::Goto(BlockId(3)),
        ));
    }
    let cleanup = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: true,
        },
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: true,
        },
    ]);
    f.body().blocks.push(block(
        vec![],
        Terminator::Return {
            value: copy(result),
            cleanup: CleanupId(cleanup),
        },
    ));
    f.valid();
    f.body().blocks[3].instructions.push(inst(
        InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: true,
        },
        0,
    ));
    assert!(f.kinds().contains(&ViolationKind::ExpiredLoan {
        loan: LoanId(0),
        place: PlaceId(result)
    }));
    f.body().blocks[3].instructions.clear();
    edit_cleanup(&mut f.body(), cleanup, |actions| {
        actions[0] = CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        };
    });
    f.broken(vec![ViolationKind::InactiveLoan(LoanId(0))]);
}

#[test]
fn relocating_a_borrowed_temporary_does_not_grant_a_taken_call_argument() {
    let mut f = Fixture::unit();
    let source = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let alias = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
    let relocated = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
    let destination = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    let mut local = ty(TypeKind::Primitive(Primitive::Bytes));
    local.modes.relation = Relation::environment();
    let local = f.definition.types.push(local);
    for p in [source, alias, relocated] {
        f.body().locals[p as usize].ty = local;
        f.body().places[p as usize].ty = local;
    }
    f.body().parameters = vec![LocalId(source)];
    owner_signature(&mut f, PassingMode::Borrow);
    f.assign(alias, Rvalue::Use(operand(source, UseKind::Borrow)), 0);
    f.assign(relocated, Rvalue::Use(take(alias)), 0);
    let mut callee = function_signature(
        "callee",
        vec![(PassingMode::Take, TypeKind::Primitive(Primitive::Bytes))],
        TypeKind::Tuple(vec![].into()),
    );
    let SignatureKind::Function(fun) = &mut callee.kind else {
        unreachable!()
    };
    let mut parameter = interner().get(fun.parameters[0].ty).unwrap().clone();
    parameter.modes.relation = Relation::environment();
    fun.parameters[0].ty = interner().intern(parameter);
    f.signatures.insert(id("callee"), callee);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("callee"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: take(relocated),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    f.body().blocks.push(block(vec![], ret(destination, 0)));
    f.broken(vec![ViolationKind::BorrowedMove(PlaceId(relocated))]);
}

#[test]
fn builtin_handle_authority_is_checked_inside_generic_callable_results() {
    let mut f = Fixture::call();
    let roles = NominalRoles {
        tasks_capability: Some(path("Scheduler")),
        scope_capability: Some(path("ChildScope")),
        heap: None,
        ..Default::default()
    };
    let mut owner = function_signature("main", vec![], TypeKind::Tuple(vec![].into()));
    owner.metadata.roles = roles.clone().into();
    f.signatures.insert(id("main"), owner);
    let callback = |result, authority| FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result,
        error: None,
        allocation: Allocation::NoAlloc,
        authority,
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let callee = f.signatures.get_mut(&path("callee")).unwrap();
    let unit = interner().intern(ty(TypeKind::Tuple(vec![].into())));
    let result = interner().intern(ty(TypeKind::Function(callback(
        unit,
        AuthoritySummary::Known(
            vec![AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
                depth: 1,
                parameter: TypeParamId(0),
            })]
            .into(),
        ),
    ))));
    let SignatureKind::Function(fun) = &mut callee.kind else {
        unreachable!()
    };
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    fun.result = result;
    let mut unit_row = ty(TypeKind::Tuple(vec![].into()));
    unit_row.modes.sendability = Sendability::Sendable;
    let unit = f.definition.types.push(unit_row);
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U32)));
    let sender = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Sender,
        arguments: vec![GenericArgument::Type(scalar)].into(),
    }));
    let good = f.definition.types.push(ty(TypeKind::Function(callback(
        unit,
        AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Scheduler"))].into()),
    ))));
    f.body().locals[2].ty = good;
    f.body().places[2].ty = good;
    let Terminator::Call {
        callee: Callee::Definition { arguments, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        unreachable!()
    };
    arguments.push(GenericArgument::Type(sender));
    let cleanup = f.cleanup(vec![drop(2)]);
    f.body().blocks[1].terminator = ret(0, cleanup);
    f.valid();
    let bad = f.definition.types.push(ty(TypeKind::Function(callback(
        unit,
        AuthoritySummary::inert(),
    ))));
    f.body().locals[2].ty = bad;
    f.body().places[2].ty = bad;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "call result"
    }));
}

#[test]
fn erased_capture_results_are_proved_from_actual_parameters() {
    let mut f = Fixture::call();
    let directory = TypeKind::Named {
        definition: id("Dir"),
        arguments: vec![].into(),
    };
    let mut row = ty(directory.clone());
    row.modes.sendability = Sendability::Sendable;
    let dir = f.definition.types.push(row);
    f.body().locals[1].ty = dir;
    f.body().locals[1].storage = Storage::Parameter;
    f.body().places[1].ty = dir;
    f.body().parameters.push(LocalId(1));
    f.body().blocks[0].instructions.truncate(1);
    f.body().rvalues.truncate(1);
    owner_signature(&mut f, PassingMode::Take);
    let capability = Signature {
        definition: id("Dir"),
        metadata: SignatureMetadata::default(),
        fingerprint: ContentHash::default(),
        kind: SignatureKind::Type(TypeDefinition {
            loop_source: None,
            format: None,
            generics: vec![],
            representation: None,
            bounds: vec![Bound::Plain, Bound::NoallocDrop],
            authority: AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Dir"))].into()),
            capability: Some(CapabilityKind::Global),
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
        }),
    };
    f.signatures.insert(id("Dir"), capability);
    let callee = f.signatures.get_mut(&path("callee")).unwrap();
    let param = interner().intern(ty(directory));
    let item = interner().intern(ty(TypeKind::Primitive(Primitive::U32)));
    let result = interner().intern(Type {
        sendability_requirement: hirsel_types::SendabilityRequirement::None,
        allocation_target: None,
        erased_captures: Some(ErasedCaptures::FromArguments(
            vec![ParameterRef::bound(ParamId(0)).into()].into(),
        )),
        ..ty(TypeKind::Builtin {
            kind: BuiltinType::Iter,
            arguments: vec![GenericArgument::Type(item)].into(),
        })
    });
    let SignatureKind::Function(fun) = &mut callee.kind else {
        unreachable!()
    };
    fun.parameters[0].ty = param;
    fun.result = result;
    let item = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U32)));
    let mut result = ty(TypeKind::Builtin {
        kind: BuiltinType::Iter,
        arguments: vec![GenericArgument::Type(item)].into(),
    });
    result.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
        vec![AuthorityTerm::Capability(path("Dir"))].into(),
    )));
    result.modes.sendability = Sendability::Sendable;
    let good = f.definition.types.push(result.clone());
    f.body().locals[2].ty = good;
    f.body().places[2].ty = good;
    let cleanup = f.cleanup(vec![drop(2)]);
    f.body().blocks[1].terminator = ret(0, cleanup);
    f.valid();
    result.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::inert()));
    let bad = f.definition.types.push(result);
    f.body().locals[2].ty = bad;
    f.body().places[2].ty = bad;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "call result"
    }));
}

#[test]
fn heap_allocator_evidence_requires_the_declared_result_and_canonical_arena() {
    let mut f = Fixture::call();
    let roles = NominalRoles {
        heap: Some(HeapRole {
            arena: path("Arena"),
            operation: path("heap"),
        }),
        ..Default::default()
    };
    let mut arena = function_signature("Arena", vec![], TypeKind::Tuple(vec![].into()));
    arena.metadata.roles = roles.clone().into();
    arena.kind = SignatureKind::Type(TypeDefinition {
        loop_source: None,
        format: None,
        generics: vec![],
        representation: None,
        bounds: vec![],
        authority: AuthoritySummary::Known(vec![AuthorityTerm::Capability(path("Arena"))].into()),
        capability: Some(CapabilityKind::Scoped),
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
    });
    f.signatures.insert(id("Arena"), arena);
    let mut row = ty(TypeKind::Named {
        definition: id("Arena"),
        arguments: vec![].into(),
    });
    // Ordinary Arena producer cannot establish the ambient heap refinement.
    let callee = f.signatures.get_mut(&path("callee")).unwrap();
    callee.metadata.roles = roles.into();
    let SignatureKind::Function(fun) = &mut callee.kind else {
        unreachable!()
    };
    fun.result = interner().intern(row.clone());
    let result = f.definition.types.push(row.clone());
    f.body().locals[2].ty = result;
    f.body().places[2].ty = result;
    let cleanup = f.cleanup(vec![drop(2)]);
    f.body().blocks[1].terminator = ret(0, cleanup);
    f.valid();
    row.allocation_target = Some(AllocationTarget::Heap);
    row.modes.sendability = Sendability::Sendable;
    let forged = f.definition.types.push(row.clone());
    f.body().locals[2].ty = forged;
    f.body().places[2].ty = forged;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "call result"
    }));
    let callee = f.signatures.get_mut(&path("callee")).unwrap();
    let SignatureKind::Function(fun) = &mut callee.kind else {
        unreachable!()
    };
    fun.result = interner().intern(row.clone());
    f.valid();
    row.kind = TypeKind::Tuple(vec![].into());
    f.definition.types.push(row);
    assert!(f.kinds().contains(&ViolationKind::InvalidTypeGraph));
}

#[test]
fn loop_backedge_expires_a_handle_of_an_iteration_owner() {
    let mut f = Fixture::unit();
    let scope = f.body().parameter_scope;
    let outer = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0))),
        fields: vec![].into(),
    };
    let iteration = Origin {
        root: OriginRoot::Local(lexical(), hirsel_types::LocalId(71)),
        fields: vec![].into(),
    };
    let owner = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Parameter);
    let handle = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Parameter);
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters = vec![LocalId(owner), LocalId(handle), LocalId(condition)];
    let handle_ty = f.body().places[handle as usize].ty;
    let mut branded = interner().get(handle_ty).unwrap().clone();
    branded.brand = Brand {
        alternatives: vec![outer.clone()].into(),
    };
    let branded = f.definition.types.push(branded);
    f.body().locals[handle as usize].ty = branded;
    f.body().places[handle as usize].ty = branded;
    let local = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Local);
    f.assign(local, Rvalue::Constant(Constant::Integer(0)), 0);
    let carry = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Local);
    let mut union = interner().get(branded).unwrap().clone();
    union.brand.alternatives.push(iteration.clone());
    union.brand.alternatives.sort();
    let union = f.definition.types.push(union);
    f.body().locals[carry as usize].ty = union;
    f.body().places[carry as usize].ty = union;
    f.assign(carry, Rvalue::Use(copy(handle)), 0);
    let scratch = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    let marked = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    let mut marked_ty = interner().get(branded).unwrap().clone();
    marked_ty.brand = Brand {
        alternatives: vec![iteration.clone()].into(),
    };
    let marked_ty = f.definition.types.push(marked_ty);
    f.body().locals[marked as usize].ty = marked_ty;
    f.body().places[marked as usize].ty = marked_ty;
    let bound = f.body().places.len() as u32;
    let mut access = f.body().places[local as usize].clone();
    access.binding = Some(iteration);
    f.body().places.push(access);
    let read = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Use(operand(carry, UseKind::Borrow)));
    let replace = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(take(marked)));
    f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(scratch),
                    value: read,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(scratch),
                    if_live: false,
                },
                0,
            ),
            inst(
                InstructionKind::BindOrigin {
                    source: PlaceId(local),
                    place: PlaceId(bound),
                    kind: OriginBindingKind::Fresh,
                },
                0,
            ),
        ],
        Terminator::Call {
            callee: Callee::Definition {
                path: path("mark"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: copy(bound),
            }],
            returns: Some(CallReturn {
                destination: PlaceId(marked),
                next: BlockId(2),
            }),
            on_failure: CleanupId(0),
        },
    ));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Drop {
                    place: PlaceId(carry),
                    if_live: false,
                },
                0,
            ),
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(carry),
                    value: replace,
                },
                0,
            ),
        ],
        Terminator::Goto(BlockId(3)),
    ));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    let mut mark = function_signature(
        "mark",
        vec![(PassingMode::Borrow, TypeKind::Primitive(Primitive::U8))],
        TypeKind::Primitive(Primitive::U8),
    );
    let mut result = ty(TypeKind::Primitive(Primitive::U8));
    result.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    let result = interner().intern(result);
    if let SignatureKind::Function(fun) = &mut mark.kind {
        fun.result = result;
    }
    f.signatures.insert(id("mark"), mark);
    let mut signature = function_signature(
        "main",
        vec![
            (PassingMode::Borrow, TypeKind::Primitive(Primitive::U8)),
            (PassingMode::Borrow, TypeKind::Primitive(Primitive::U8)),
            (PassingMode::Borrow, TypeKind::Primitive(Primitive::Bool)),
        ],
        TypeKind::Tuple(vec![].into()),
    );
    let mut handle_row = ty(TypeKind::Primitive(Primitive::U8));
    handle_row.brand.alternatives.push(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    });
    let handle_row = interner().intern(handle_row);
    if let SignatureKind::Function(fun) = &mut signature.kind {
        fun.parameters[1].ty = handle_row;
    }
    f.signatures.insert(id("main"), signature);
    f.valid();
    let baseline = f.definition.clone();
    let replacement = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Constant(Constant::Integer(1)));
    let replacements = vec![
        inst(InstructionKind::EndOrigin(PlaceId(bound)), 0),
        inst(
            InstructionKind::Drop {
                place: PlaceId(bound),
                if_live: false,
            },
            0,
        ),
        inst(
            InstructionKind::Assign {
                destination: PlaceId(bound),
                value: replacement,
            },
            0,
        ),
        inst(
            InstructionKind::BindOrigin {
                source: PlaceId(bound),
                place: PlaceId(bound),
                kind: OriginBindingKind::Fresh,
            },
            0,
        ),
    ];
    f.body().blocks[2].instructions.splice(0..0, replacements);
    // Moving the handle into storage after its owner ended is not a use: it
    // may sit there and be dropped. The storage holds an ended-brand value.
    let report = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(report.violations.is_empty(), "{report:?}");
    f.body().blocks[2].instructions.extend([
        inst(
            InstructionKind::Assign {
                destination: PlaceId(scratch),
                value: read,
            },
            0,
        ),
        inst(
            InstructionKind::Drop {
                place: PlaceId(scratch),
                if_live: false,
            },
            0,
        ),
    ]);
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
            .any(|v| v.kind == ViolationKind::InvalidOrigin(PlaceId(carry))),
        "{report:?}"
    );
    assert!(
        report
            .violations
            .iter()
            .all(|v| matches!(v.kind, ViolationKind::InvalidOrigin(_))),
        "{report:?}"
    );
    f.definition = baseline;
    // The sole mutation creates a backedge carrying the previously valid handle.
    f.body().blocks[2].terminator = Terminator::Branch {
        condition: copy(condition),
        yes: BlockId(1),
        no: BlockId(3),
    };
    let report = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        report.violations.iter().any(|v| v.block == Some(BlockId(1))
            && v.instruction == Some(0)
            && v.kind == ViolationKind::InvalidOrigin(PlaceId(carry))),
        "{report:?}"
    );
    assert!(
        report
            .violations
            .iter()
            .all(|v| matches!(v.kind, ViolationKind::InvalidOrigin(_))),
        "{report:?}"
    );
}

#[test]
fn a_rebound_name_cannot_read_its_previous_storage_location() {
    let mut f = Fixture::unit();
    let a = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Local);
    let b = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Local);
    f.assign(a, Rvalue::Constant(Constant::Integer(1)), 0);
    f.assign(b, Rvalue::Constant(Constant::Integer(2)), 0);
    let mut aliases = vec![];
    for (source, name) in [(a, 7), (b, 8)] {
        let mut place = f.body().places[source as usize].clone();
        place.binding = Some(Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(name)),
            fields: vec![].into(),
        });
        let target = PlaceId(f.body().places.len() as u32);
        f.body().places.push(place);
        f.emit(InstructionKind::BindOrigin {
            source: PlaceId(source),
            place: target,
            kind: OriginBindingKind::Fresh,
        });
        aliases.push(target);
    }
    f.body().result = f.body().places[a as usize].ty;
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(aliases[0].0),
        cleanup: CleanupId(0),
    };
    f.valid();
    // Both physical addresses have carried this name, but only the second is
    // current after its semantic introduction.
    f.body().places[aliases[1].0 as usize]
        .binding
        .as_mut()
        .unwrap()
        .root = OriginRoot::Local(lexical(), hirsel_types::LocalId(7));
    f.broken(vec![ViolationKind::InvalidOrigin(aliases[0])]);
}

fn reborrow_fixture() -> (Fixture, u32, u32) {
    let mut f = Fixture::unit();
    let source = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Local);
    let result = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(source, Rvalue::Constant(Constant::Bool(true)), 0);
    let parent = f.through_loan(source, 0);
    let child = f.through_loan(source, 1);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(source),
        mutable: true,
        if_inactive: false,
    });
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(parent),
        mutable: true,
        if_inactive: false,
    });
    f.assign(result, Rvalue::Use(copy(child)), 0);
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(1),
        if_active: false,
    });
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    f.body().result = f.body().places[result as usize].ty;
    f.body().blocks[0].terminator = ret(result, 0);
    (f, parent, child)
}

#[test]
fn explicit_reborrows_grant_only_the_selected_active_access_chain() {
    let (f, _, _) = reborrow_fixture();
    f.valid();
    // The parent cannot be read until its exclusive child ends.
    let (mut f, parent, _) = reborrow_fixture();
    f.body().rvalues[2] = Rvalue::Use(copy(parent));
    assert!(f.kinds().iter().any(|v| matches!(
        v,
        ViolationKind::LoanConflict {
            loan: LoanId(1),
            ..
        }
    )));
    // A live loan on some other place does not authorize this access.
    let (mut f, _, child) = reborrow_fixture();
    f.body().places[child as usize].root = LocalId(2);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidUse(PlaceId(child)))
    );
    // Ended capabilities cannot be used even when their storage remains live.
    let (mut f, _, child) = reborrow_fixture();
    f.body().blocks[0].instructions.swap(4, 5);
    assert!(f.kinds().contains(&ViolationKind::ExpiredLoan {
        loan: LoanId(1),
        place: PlaceId(child)
    }));
    // An ancestor cannot end ahead of a live descendant.
    let (mut f, _, _) = reborrow_fixture();
    f.body().blocks[0].instructions.swap(5, 6);
    assert!(f.kinds().iter().any(|v| matches!(
        v,
        ViolationKind::LoanConflict {
            loan: LoanId(1),
            ..
        }
    )));
    // A shared parent cannot authorize a mutable reborrow.
    let (mut f, _, _) = reborrow_fixture();
    if let InstructionKind::BeginLoan { mutable, .. } = &mut f.body().blocks[0].instructions[2].kind
    {
        *mutable = false;
    } else {
        panic!()
    }
    assert!(
        f.kinds()
            .iter()
            .any(|v| matches!(v, ViolationKind::InvalidUse(_)))
    );
    // Borrow permission never authorizes moving the source out.
    let (mut f, _, child) = reborrow_fixture();
    f.body().rvalues[2] = Rvalue::Use(take(child));
    assert!(
        f.kinds()
            .iter()
            .any(|v| matches!(v, ViolationKind::LoanConflict { .. }))
    );
}

#[test]
fn a_copied_scalar_field_of_a_mut_parameter_has_independent_storage() {
    let mut f = Fixture::unit();
    let snapshot = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Local);
    let observed = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
    let scalar = f.body().places[snapshot as usize].ty;
    let p = param(&mut f, TypeKind::Tuple(vec![scalar].into()));
    owner_signature(&mut f, PassingMode::Mut);
    let field = f.body().places.len() as u32;
    let root = f.body().places[p as usize].root;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root,
        projection: vec![Projection::Field(0)].into(),
        ty: scalar,
    });
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(field),
        mutable: false,
        if_inactive: false,
    });
    let view = f.through_loan(field, 0);
    f.assign(snapshot, Rvalue::Use(copy(view)), 0);
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    // The snapshot survives the source access loan, and either storage may
    // be replaced without requiring access to the other.
    for place in [field, snapshot] {
        f.emit(InstructionKind::StorageDead {
            place: PlaceId(place),
            if_live: false,
        });
        f.assign(place, Rvalue::Constant(Constant::Integer(1)), 0);
        if place == field {
            f.assign(observed, Rvalue::Use(copy(snapshot)), 0);
        }
    }
    f.valid();

    // Forging an alias access instead of the owned local must still reject.
    let replacement = f.body().blocks[0].instructions.last_mut().unwrap();
    let InstructionKind::Assign { destination, .. } = &mut replacement.kind else {
        panic!()
    };
    *destination = PlaceId(view);
    let kinds = f.kinds();
    assert!(
        kinds.contains(&ViolationKind::InvalidUse(PlaceId(view))),
        "{kinds:?}"
    );
}

#[test]
fn mutable_alias_replacement_preserves_its_loan_until_scope_exit() {
    let (mut f, _, child) = reborrow_fixture();
    let failure = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    f.body().blocks[0].instructions.insert(
        4,
        inst(
            InstructionKind::Drop {
                place: PlaceId(child),
                if_live: false,
            },
            failure,
        ),
    );
    f.body()
        .rvalues
        .push(Rvalue::Constant(Constant::Bool(false)));
    f.body().blocks[0].instructions.insert(
        5,
        inst(
            InstructionKind::Assign {
                destination: PlaceId(child),
                value: RvalueId(3),
            },
            0,
        ),
    );
    f.valid();
    // Replacement through a shared alias is rejected independently of type.
    if let InstructionKind::BeginLoan { mutable, .. } = &mut f.body().blocks[0].instructions[3].kind
    {
        *mutable = false;
    } else {
        panic!()
    }
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidUse(PlaceId(child)))
    );
}

#[test]
fn conditional_parent_loans_cannot_end_before_their_conditional_children() {
    let (mut f, _, _) = reborrow_fixture();
    let mut instructions = f.body().blocks[0].instructions.split_off(2);
    instructions.truncate(3);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(1),
        yes: BlockId(1),
        no: BlockId(2),
    };
    f.body().blocks.extend([
        block(instructions, Terminator::Goto(BlockId(3))),
        block(vec![], Terminator::Goto(BlockId(3))),
        block(
            vec![
                inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(1),
                        if_active: true,
                    },
                    0,
                ),
                inst(
                    InstructionKind::EndLoan {
                        loan: LoanId(0),
                        if_active: true,
                    },
                    0,
                ),
            ],
            ret(0, 0),
        ),
    ]);
    f.body().result = f.body().places[0].ty;
    f.valid();
    f.body().blocks[3].instructions.swap(0, 1);
    assert!(f.kinds().iter().any(|v| matches!(
        v,
        ViolationKind::LoanConflict {
            loan: LoanId(1),
            ..
        }
    )));
}

#[test]
fn dependent_generic_return_derives_local_sendability_without_changing_actuals() {
    let mut f = Fixture::unit();
    let a = f.slot(
        TypeKind::Parameter(TypeParameterRef::body(
            scoped(NameSite::Item),
            TypeParamId(0),
        )),
        Storage::Parameter,
    );
    let b = f.slot(
        TypeKind::Parameter(TypeParameterRef::body(
            scoped(NameSite::Item),
            TypeParamId(0),
        )),
        Storage::Parameter,
    );
    let mut local = ty(TypeKind::Parameter(TypeParameterRef::body(
        scoped(NameSite::Item),
        TypeParamId(0),
    )));
    local.modes.relation = Relation::environment();
    local.drop_allocation = DropAllocation::DependsOn(
        vec![TypeParameterRef::body(
            scoped(NameSite::Item),
            TypeParamId(0),
        )]
        .into(),
    );
    let local = f.definition.types.push(local);
    for p in [a, b] {
        f.body().locals[p as usize].ty = local;
        f.body().places[p as usize].ty = local;
        f.body().parameters.push(LocalId(p));
    }
    let mut result = interner().get(local).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    result.modes.sendability = Sendability::DependsOn(
        vec![TypeParameterRef::body(
            scoped(NameSite::Item),
            TypeParamId(0),
        )]
        .into(),
    );
    let result = f.definition.types.push(result);
    f.body().result = result;
    owner_signature(&mut f, PassingMode::Borrow);
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    fun.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Copyable].into(),
    });
    let scope = f.body().parameter_scope;
    f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(a),
        cleanup: CleanupId(0),
    };
    f.valid();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(b),
        cleanup: CleanupId(0),
    };
    f.broken(vec![ViolationKind::InvalidResultSource(PlaceId(b))]);
}

#[path = "review_tests.rs"]
mod review;

fn copied_source_scope(local: bool, leave: bool) -> Fixture {
    let mut f = Fixture::string();
    f.body().scopes.push(Scope {
        parent: Some(ScopeId(0)),
        span: Span::default(),
    });
    f.body().locals[1].scope = ScopeId(1);
    let view = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.body().scopes.push(Scope {
        parent: Some(ScopeId(1)),
        span: Span::default(),
    });
    f.body().locals[view as usize].scope = ScopeId(2);
    let copied = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    if local {
        for place in [view, copied] {
            let old = f.body().places[place as usize].ty;
            let mut t = interner().get(old).unwrap().clone();
            t.modes.relation = Relation::environment();
            let t = f.definition.types.push(t);
            f.body().places[place as usize].ty = t;
            f.body().locals[place as usize].ty = t;
        }
    }
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable: false,
        if_inactive: false,
    });
    let access = f.through_loan(1, 0);
    f.assign(view, Rvalue::Use(operand(access, UseKind::Borrow)), 0);
    f.assign(copied, Rvalue::Use(copy(view)), 0);
    if !leave {
        // A copy destroyed before its source's scope ends is always sound.
        f.emit(InstructionKind::Drop {
            place: PlaceId(copied),
            if_live: false,
        });
        let failure = f.cleanup(vec![
            CleanupAction::EndLoan {
                loan: LoanId(0),
                if_active: true,
            },
            drop(1),
        ]);
        f.body().blocks[0]
            .instructions
            .last_mut()
            .unwrap()
            .on_failure = CleanupId(failure);
    }
    f.emit(InstructionKind::EndScope(ScopeId(2)));
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    let cleanup = f.cleanup(vec![drop(copied)]);
    f.emit(InstructionKind::Drop {
        place: PlaceId(1),
        if_live: false,
    });
    f.body().blocks[0]
        .instructions
        .last_mut()
        .unwrap()
        .on_failure = CleanupId(cleanup);
    f.emit(InstructionKind::EndScope(ScopeId(1)));
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn nonflat_local_copy_keeps_its_named_sources_scope_bound() {
    copied_source_scope(false, true).valid();
    copied_source_scope(true, false).valid();
    let f = copied_source_scope(true, true);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(3))),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn copying_an_external_source_view_ends_access_but_retains_its_enduring_bound() {
    for copied in [false, true] {
        let mut f = copied_source_scope(true, true);
        // The access dies with its inner scope, while parameter storage belongs
        // to the caller. Keep an unrelated local owner to exercise scope cleanup.
        f.body().locals[1].storage = Storage::Parameter;
        f.body().locals[1].scope = ScopeId(0);
        f.body().parameters.push(LocalId(1));
        let scratch = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
        let root = f.body().places[scratch as usize].root;
        f.body().locals[root.0 as usize].scope = ScopeId(1);
        let InstructionKind::Assign { destination, .. } =
            &mut f.body().blocks[0].instructions[1].kind
        else {
            unreachable!()
        };
        *destination = PlaceId(scratch);
        for instruction in &mut f.body().blocks[0].instructions {
            if let InstructionKind::Drop { place, .. } = &mut instruction.kind {
                if *place == PlaceId(1) {
                    *place = PlaceId(scratch);
                }
            }
        }
        if !copied {
            let Rvalue::Use(value) = &mut f.body().rvalues[3] else {
                unreachable!()
            };
            value.usage = UseKind::Borrow;
            let used = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
            let t = f.body().places[3].ty;
            let root = f.body().places[used as usize].root;
            f.body().places[used as usize].ty = t;
            f.body().locals[root.0 as usize].ty = t;
            let failure = f.cleanup(vec![]);
            for instruction in &mut f.body().blocks[0].instructions {
                if matches!(instruction.kind, InstructionKind::Drop { .. }) {
                    instruction.on_failure = CleanupId(failure);
                }
            }
            f.assign(used, Rvalue::Use(copy(3)), failure);
            let cleanup = f.cleanup(vec![drop(used)]);
            f.body().blocks[0].terminator = ret(0, cleanup);
        }
        owner_signature(&mut f, PassingMode::Borrow);
        if copied {
            f.valid();
        } else {
            assert!(
                f.kinds()
                    .iter()
                    .any(|v| matches!(v, ViolationKind::ExpiredLoan { .. })),
                "{:?}",
                f.kinds()
            );
        }
    }
}
// Review p12a: a borrowed view cannot replace caller-owned storage.
#[test]
fn mutable_parameter_replacement_requires_owned_storage() {
    for usage in [UseKind::Borrow, UseKind::Copy, UseKind::Take] {
        let mut f = Fixture::unit();
        let h = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
        let x = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
        f.body().parameters = vec![LocalId(h), LocalId(x)];
        if usage == UseKind::Borrow {
            f.emit(InstructionKind::Drop {
                place: PlaceId(h),
                if_live: false,
            });
            f.assign(h, Rvalue::Use(operand(x, usage)), 0);
        } else {
            let old = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
            f.emit(InstructionKind::Replace {
                destination: PlaceId(h),
                value: operand(x, usage),
                displaced: PlaceId(old),
                if_live: false,
            });
            f.emit(InstructionKind::Drop {
                place: PlaceId(old),
                if_live: false,
            });
        }
        owner_signature(&mut f, PassingMode::Borrow);
        let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            unreachable!()
        };
        fun.parameters[0].passing = PassingMode::Mut;
        if usage == UseKind::Take {
            fun.parameters[1].passing = PassingMode::Take;
        }
        let kinds = f.kinds();
        if usage == UseKind::Borrow {
            assert!(
                kinds.contains(&ViolationKind::BorrowedMove(PlaceId(h))),
                "{kinds:?}"
            );
        } else {
            assert!(kinds.is_empty(), "{kinds:?}");
        }
    }
}
// A Plain value moved out of borrowed storage is its own bits, so it may be
// installed by a replacement like any other Take operand; borrowed storage
// with release obligations still cannot move into owned storage.
#[test]
fn replacement_installs_plain_bits_read_from_borrowed_storage() {
    for (kind, old, plain) in [
        (Primitive::U64, Constant::Integer(1), true),
        (Primitive::String, Constant::String("old".into()), false),
    ] {
        for usage in [UseKind::Take, UseKind::Borrow] {
            let mut f = Fixture::unit();
            let x = f.slot(TypeKind::Primitive(kind.clone()), Storage::Parameter);
            f.body().parameters = vec![LocalId(x)];
            let y = f.slot(TypeKind::Primitive(kind.clone()), Storage::Local);
            f.assign(y, Rvalue::Constant(old.clone()), 0);
            let moved = f.slot(TypeKind::Primitive(kind.clone()), Storage::Temporary);
            f.assign(moved, Rvalue::Use(operand(x, usage)), 0);
            let displaced = f.slot(TypeKind::Primitive(kind.clone()), Storage::Temporary);
            f.emit(InstructionKind::Replace {
                destination: PlaceId(y),
                value: take(moved),
                displaced: PlaceId(displaced),
                if_live: false,
            });
            f.emit(InstructionKind::Drop {
                place: PlaceId(displaced),
                if_live: false,
            });
            let c = f.cleanup(vec![drop(y)]);
            f.body().blocks[0].terminator = ret(0, c);
            owner_signature(&mut f, PassingMode::Borrow);
            let kinds = f.kinds();
            if plain {
                assert!(kinds.is_empty(), "{usage:?}: {kinds:?}");
            } else {
                assert!(
                    kinds
                        .iter()
                        .any(|k| matches!(k, ViolationKind::BorrowedMove(_))),
                    "{usage:?}: {kinds:?}"
                );
            }
        }
    }
}
// Review p12b: caller retains mutable storage on normal and failure exits.
#[test]
fn mutable_parameter_must_be_live_on_every_exit() {
    for fail in [false, true] {
        let mut f = Fixture::unit();
        let h = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
        f.body().parameters = vec![LocalId(h)];
        f.emit(InstructionKind::Drop {
            place: PlaceId(h),
            if_live: false,
        });
        if fail {
            f.body().blocks[0].terminator = Terminator::Fail {
                rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                cleanup: CleanupId(0),
            };
        }
        owner_signature(&mut f, PassingMode::Mut);
        assert!(
            f.kinds()
                .contains(&ViolationKind::ReadBeforeInit(PlaceId(h)))
        );
    }
}

#[test]
fn ending_a_field_expires_enclosing_brands_but_not_sibling_brands() {
    for fields in [vec![], vec![FieldId(0)], vec![FieldId(1)]] {
        let mut f = Fixture::unit();
        let scalar = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U64)));
        let record = TypeKind::Record(
            vec![
                Field {
                    growable: false,
                    name: "n".into(),
                    ty: scalar,
                },
                Field {
                    growable: false,
                    name: "other".into(),
                    ty: scalar,
                },
            ]
            .into(),
        );
        let owner = f.slot(record, Storage::Parameter);
        let value = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
        let origin = Origin {
            root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
            fields: fields.into(),
        };
        let mut branded = ty(TypeKind::Primitive(Primitive::U64));
        branded.brand.alternatives.push(origin);
        let branded = f.definition.types.push(branded);
        f.body().locals[value as usize].ty = branded;
        f.body().places[value as usize].ty = branded;
        f.body().parameters = vec![LocalId(owner), LocalId(value)];
        let result = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.body().locals[result as usize].ty = branded;
        f.body().places[result as usize].ty = branded;
        let mut field = f.body().places[owner as usize].clone();
        field.projection.push(Projection::Field(0));
        field.ty = scalar;
        field.binding = Some(Origin {
            root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
            fields: vec![FieldId(0)].into(),
        });
        let field_id = PlaceId(f.body().places.len() as u32);
        f.body().places.push(field);
        owner_signature(&mut f, PassingMode::Borrow);
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let mut marked = interner().get(branded).unwrap().clone();
        marked.brand.alternatives[0].root = OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
        let marked = interner().intern(marked);
        if let SignatureKind::Function(fun) = &mut signature.kind {
            fun.parameters[1].ty = marked;
        }
        f.emit(InstructionKind::EndOrigin(field_id));
        f.assign(result, Rvalue::Use(operand(value, UseKind::Borrow)), 0);
        f.emit(InstructionKind::Drop {
            place: PlaceId(result),
            if_live: false,
        });
        if interner().get(branded).unwrap().brand.alternatives[0].fields == vec![FieldId(1)] {
            f.valid();
        } else {
            let kinds = f.kinds();
            assert!(
                kinds.contains(&ViolationKind::InvalidOrigin(PlaceId(value))),
                "{kinds:?}"
            );
        }
    }
}

#[test]
fn branch_joins_preserve_owner_expiry_including_nested_brands() {
    for fields in [vec![], vec![FieldId(0)], vec![FieldId(1)]] {
        let mut f = Fixture::unit();
        let scalar = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U64)));
        let record = TypeKind::Record(
            vec![
                Field {
                    growable: false,
                    name: "n".into(),
                    ty: scalar,
                },
                Field {
                    growable: false,
                    name: "other".into(),
                    ty: scalar,
                },
            ]
            .into(),
        );
        let owner = f.slot(record, Storage::Parameter);
        let value = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
        let origin = Origin {
            root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
            fields: fields.into(),
        };
        let mut branded = ty(TypeKind::Primitive(Primitive::U64));
        branded.brand.alternatives.push(origin);
        let branded = f.definition.types.push(branded);
        f.body().locals[value as usize].ty = branded;
        f.body().places[value as usize].ty = branded;
        f.body().parameters = vec![LocalId(owner), LocalId(value)];
        let result = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.body().locals[result as usize].ty = branded;
        f.body().places[result as usize].ty = branded;
        let mut field = f.body().places[owner as usize].clone();
        field.projection.push(Projection::Field(0));
        field.ty = scalar;
        field.binding = Some(Origin {
            root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
            fields: vec![FieldId(0)].into(),
        });
        let field_id = PlaceId(f.body().places.len() as u32);
        f.body().places.push(field);
        owner_signature(&mut f, PassingMode::Borrow);
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let mut marked = interner().get(branded).unwrap().clone();
        marked.brand.alternatives[0].root = OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
        let marked = interner().intern(marked);
        if let SignatureKind::Function(fun) = &mut signature.kind {
            fun.parameters[1].ty = marked;
        }
        f.emit(InstructionKind::EndOrigin(field_id));
        f.assign(result, Rvalue::Use(operand(value, UseKind::Borrow)), 0);
        f.emit(InstructionKind::Drop {
            place: PlaceId(result),
            if_live: false,
        });
        let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
        let mut reads = f.body().blocks[0].instructions.split_off(1);
        let condition_init = reads.pop().unwrap();
        let expiry = reads.remove(0);
        f.body().blocks[0].instructions.push(condition_init);
        let exit = f.body().blocks[0].terminator.clone();
        f.body().blocks[0].terminator = Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(1),
            no: BlockId(2),
        };
        f.body()
            .blocks
            .push(block(vec![expiry], Terminator::Goto(BlockId(3))));
        f.body()
            .blocks
            .push(block(vec![], Terminator::Goto(BlockId(3))));
        f.body().blocks.push(block(reads, exit));
        if interner().get(branded).unwrap().brand.alternatives[0].fields == vec![FieldId(1)] {
            f.valid();
        } else {
            let kinds = f.kinds();
            assert!(
                kinds.contains(&ViolationKind::InvalidOrigin(PlaceId(value))),
                "{kinds:?}"
            );
        }
    }
}

#[test]
fn arithmetic_reads_branded_scalars_but_cannot_brand_its_result() {
    for op in [
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Div,
        BinaryOp::Rem,
        BinaryOp::BitAnd,
        BinaryOp::BitOr,
        BinaryOp::BitXor,
        BinaryOp::Shl,
        BinaryOp::Shr,
    ] {
        let mut f = Fixture::unit();
        let owner = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Parameter);
        let branded = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Parameter);
        f.body().parameters = vec![hirsel_tir::LocalId(owner), hirsel_tir::LocalId(branded)];
        let brand = Brand {
            alternatives: vec![Origin {
                root: OriginRoot::Parameter(ParameterRef::body(
                    f.body().parameter_scope.clone(),
                    ParamId(0),
                )),
                fields: vec![].into(),
            }]
            .into(),
        };
        revise_comparison_place(&mut f, branded, |t| t.brand = brand.clone());
        let result = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
        f.assign(
            result,
            Rvalue::Binary {
                op,
                left: copy(branded),
                right: copy(owner),
            },
            0,
        );
        owner_signature(&mut f, PassingMode::Borrow);
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        if let SignatureKind::Function(function) = &mut signature.kind {
            let mut declared = interner().get(function.parameters[1].ty).unwrap().clone();
            declared.brand.alternatives[0].root =
                OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
            function.parameters[1].ty = interner().intern(declared);
        }
        f.valid();
        revise_comparison_place(&mut f, result, |t| t.brand = brand.clone());
        assert!(
            f.kinds().contains(&ViolationKind::TypeMismatch {
                entity: "binary result"
            }),
            "{:?}",
            f.kinds()
        );
    }
}

#[test]
fn dependent_arena_return_derives_placement_and_still_checks_its_source() {
    let mut f = Fixture::unit();
    let arena = arena_slot(&mut f, Storage::Parameter);
    let other_arena = arena_slot(&mut f, Storage::Parameter);
    let a = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let b = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let unselected = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    let mut local = ty(TypeKind::Primitive(Primitive::Bytes));
    let environment = Relation::environment();
    local.modes.relation = environment.union(&Relation::place(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    }));
    let first = f.definition.types.push(local.clone());
    local.modes.relation = environment.union(&Relation::place(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(1))),
        fields: vec![].into(),
    }));
    let second = f.definition.types.push(local);
    for (p, t) in [(a, first), (b, second), (unselected, first)] {
        f.body().locals[p as usize].ty = t;
        f.body().places[p as usize].ty = t;
    }
    f.body().parameters = vec![
        LocalId(arena),
        LocalId(other_arena),
        LocalId(a),
        LocalId(b),
        LocalId(unselected),
    ];
    let mut result = ty(TypeKind::Primitive(Primitive::Bytes));
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(2))]);
    let single = f.definition.types.push(result.clone());
    result.modes.relation = Relation::parameters([
        ParameterRef::bound(ParamId(2)),
        ParameterRef::bound(ParamId(3)),
    ]);
    let mixed = f.definition.types.push(result);
    f.body().result = single;
    owner_signature(&mut f, PassingMode::Borrow);
    let scope = f.body().parameter_scope;
    for (p, t) in [(a, first), (b, second), (unselected, first)] {
        let t = bind_parameter_scopes(interner(), t, &[scope]).unwrap();
        f.body().locals[p as usize].ty = t;
        f.body().places[p as usize].ty = t;
    }
    f.body().result = bind_parameter_scopes(interner(), single, &[scope]).unwrap();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(a),
        cleanup: CleanupId(0),
    };
    f.valid();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(unselected),
        cleanup: CleanupId(0),
    };
    f.broken(vec![ViolationKind::InvalidResultSource(PlaceId(
        unselected,
    ))]);
    if let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind {
        fun.result = mixed;
    }
    f.body().result = bind_parameter_scopes(interner(), mixed, &[scope]).unwrap();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(a),
        cleanup: CleanupId(0),
    };
    f.valid();
    f.body().blocks[0].terminator = Terminator::Return {
        value: copy(unselected),
        cleanup: CleanupId(0),
    };
    f.broken(vec![ViolationKind::InvalidResultSource(PlaceId(
        unselected,
    ))]);
}

#[test]
fn computed_unary_results_cannot_forge_an_operands_brand() {
    use hirsel_syntax::UnaryOp;
    for (op, primitive) in [
        (UnaryOp::Neg, Primitive::I64),
        (UnaryOp::BitNot, Primitive::U64),
        (UnaryOp::Not, Primitive::Bool),
    ] {
        let mut f = Fixture::unit();
        let owner = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Parameter);
        let source = f.slot(TypeKind::Primitive(primitive), Storage::Parameter);
        let result = f.slot(TypeKind::Primitive(primitive), Storage::Temporary);
        let mut marked = ty(TypeKind::Primitive(primitive));
        marked.brand.alternatives.push(Origin {
            root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
            fields: vec![].into(),
        });
        let marked = f.definition.types.push(marked);
        f.body().locals[source as usize].ty = marked;
        f.body().places[source as usize].ty = marked;
        f.body().parameters = vec![LocalId(owner), LocalId(source)];
        owner_signature(&mut f, PassingMode::Borrow);
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let mut row = interner().get(marked).unwrap().clone();
        row.brand.alternatives[0].root = OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
        let row = interner().intern(row);
        if let SignatureKind::Function(fun) = &mut signature.kind {
            fun.parameters[1].ty = row;
        }
        f.assign(
            result,
            Rvalue::Unary {
                op,
                value: copy(source),
            },
            0,
        );
        f.emit(InstructionKind::Drop {
            place: PlaceId(result),
            if_live: false,
        });
        f.valid();
        f.body().locals[result as usize].ty = marked;
        f.body().places[result as usize].ty = marked;
        let kinds = f.kinds();
        assert!(
            kinds.contains(&ViolationKind::InvalidOrigin(PlaceId(result))),
            "{kinds:?}"
        );
    }
}

#[test]
fn loop_backedge_expires_a_retained_parameter_handle() {
    let mut f = Fixture::unit();
    let owner = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Parameter);
    let handle = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Parameter);
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters = vec![LocalId(owner), LocalId(handle), LocalId(condition)];
    let brand = Brand {
        alternatives: vec![Origin {
            root: OriginRoot::Parameter(ParameterRef::body(
                f.body().parameter_scope.clone(),
                ParamId(0),
            )),
            fields: vec![].into(),
        }]
        .into(),
    };
    f.body().places[owner as usize].binding = Some(brand.alternatives[0].clone());
    revise_comparison_place(&mut f, handle, |t| t.brand = brand.clone());
    let scratch = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
    revise_comparison_place(&mut f, scratch, |t| t.brand = brand.clone());
    let read = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Use(operand(handle, UseKind::Borrow)));
    f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(scratch),
                    value: read,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(scratch),
                    if_live: false,
                },
                0,
            ),
        ],
        Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(1),
            no: BlockId(2),
        },
    ));
    f.body().blocks.push(block(vec![], ret(0, 0)));
    owner_signature(&mut f, PassingMode::Borrow);
    let signature = f.signatures.get_mut(&path("main")).unwrap();
    if let SignatureKind::Function(function) = &mut signature.kind {
        let mut declared = interner().get(function.parameters[1].ty).unwrap().clone();
        declared.brand.alternatives[0].root =
            OriginRoot::Parameter(ParameterRef::bound(ParamId(0)));
        function.parameters[1].ty = interner().intern(declared);
    }
    f.valid();
    f.body().blocks[1]
        .instructions
        .push(inst(InstructionKind::EndOrigin(PlaceId(owner)), 0));
    let report = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        report.violations.iter().any(|v| v.block == Some(BlockId(1))
            && v.instruction == Some(0)
            && v.kind == ViolationKind::InvalidOrigin(PlaceId(handle))),
        "{report:?}"
    );
    // A break exits without carrying the expired identity back to the read.
    f.body().blocks[1].terminator = Terminator::Goto(BlockId(2));
    f.valid();
}

#[test]
fn a_mutable_noncopyable_element_can_be_borrowed_but_cannot_be_taken() {
    let mut f = Fixture::unit();
    let unit = f.body().result;
    let task = f.definition.types.push(ty(TypeKind::Builtin {
        kind: BuiltinType::Task,
        arguments: vec![GenericArgument::Type(unit)].into(),
    }));
    let sequence = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(task)].into(),
        },
        Storage::Parameter,
    );
    f.body().parameters.push(LocalId(sequence));
    let index = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Temporary);
    f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
    let element = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(sequence),
        projection: vec![Projection::Index(LocalId(index))].into(),
        ty: task,
    });
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(element),
        mutable: true,
        if_inactive: false,
    });
    let failure = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    let access = f.through_loan(element, 0);
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("observe"),
            arguments: vec![],
        },
        arguments: vec![CallArgument {
            parameter: ParamId(0),
            value: operand(access, UseKind::Borrow),
        }],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure),
    };
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(0),
                if_active: false,
            },
            0,
        )],
        ret(0, 0),
    ));
    owner_signature(&mut f, PassingMode::Mut);
    let mut signature = function_signature("observe", vec![], TypeKind::Tuple(vec![].into()));
    if let SignatureKind::Function(function) = &mut signature.kind {
        function.parameters = vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Borrow,
            ty: task,
        }]
        .into();
        function.result = unit;
    }
    f.signatures.insert(id("observe"), signature);
    f.valid();
    if let SignatureKind::Function(function) =
        &mut f.signatures.get_mut(&path("observe")).unwrap().kind
    {
        function.parameters[0].passing = PassingMode::Take;
    }
    if let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator {
        arguments[0].value = take(access);
    }
    assert!(
        f.kinds()
            .contains(&ViolationKind::BorrowedMove(PlaceId(access))),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn a_live_borrowing_closure_protects_its_source_from_destruction_and_mutation() {
    for destroy in [true, false] {
        let mut f = closure_fixture();
        f.valid();
        let cleanup = match f.body().blocks[0].terminator {
            Terminator::Return { cleanup, .. } => cleanup,
            _ => unreachable!(),
        };
        if destroy {
            f.body().blocks[0].instructions.push(inst(
                InstructionKind::Drop {
                    place: PlaceId(1),
                    if_live: false,
                },
                cleanup.0,
            ));
        } else {
            f.body().blocks[0].instructions.push(inst(
                InstructionKind::BeginLoan {
                    id: LoanId(1),
                    place: PlaceId(1),
                    mutable: true,
                    if_inactive: false,
                },
                cleanup.0,
            ));
            f.body().blocks[0].instructions.push(inst(
                InstructionKind::EndLoan {
                    loan: LoanId(1),
                    if_active: false,
                },
                cleanup.0,
            ));
        }
        let expected = if destroy {
            ViolationKind::LoanOutlivesSource {
                loan: LoanId(0),
                place: PlaceId(1),
            }
        } else {
            ViolationKind::LoanConflict {
                loan: LoanId(0),
                place: PlaceId(1),
            }
        };
        assert!(f.kinds().contains(&expected), "{:?}", f.kinds());
    }
}

#[test]
fn unique_call_evidence_follows_current_content_of_aliased_storage() {
    for shared in [false, true] {
        let mut f = Fixture::call();
        let signature = f.signatures.get_mut(&path("callee")).unwrap();
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        let mut row = interner().get(function.parameters[0].ty).unwrap().clone();
        row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
        row.modes.sendability = hirsel_types::Sendability::Sendable;
        function.parameters[0].ty = interner().intern(row);
        if shared {
            let alias = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
            f.assign(alias, Rvalue::Use(copy(1)), 0);
            let cleanup = f.cleanup(vec![drop(alias)]);
            if let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator {
                *on_failure = CleanupId(cleanup);
            }
            f.body().blocks[1].terminator = ret(2, cleanup);
        }
        let kinds = f.kinds();
        if shared {
            assert!(
                kinds.contains(&ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
                "{kinds:?}"
            );
        } else {
            assert!(kinds.is_empty(), "{kinds:?}");
        }
    }
}

#[test]
fn joined_content_is_unique_only_when_every_predecessor_proves_it() {
    for unknown in [false, true] {
        let mut f = Fixture::call();
        let signature = f.signatures.get_mut(&path("callee")).unwrap();
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        let mut row = interner().get(function.parameters[0].ty).unwrap().clone();
        row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
        function.parameters[0].ty = interner().intern(row);
        let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
        f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
        let initialize = f.body().blocks[0].instructions.remove(1);
        let call = f.body().blocks[0].terminator.clone();
        let exit = f.body().blocks[1].clone();
        f.body().blocks.truncate(1);
        f.body().blocks[0].terminator = Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(1),
            no: BlockId(2),
        };
        f.body().blocks.push(block(
            vec![initialize.clone()],
            Terminator::Goto(BlockId(3)),
        ));
        let other = if unknown {
            let signature =
                function_signature("unknown", vec![], TypeKind::Primitive(Primitive::String));
            f.signatures.insert(id("unknown"), signature);
            block(
                vec![],
                Terminator::Call {
                    callee: Callee::Definition {
                        path: path("unknown"),
                        arguments: vec![],
                    },
                    arguments: vec![],
                    returns: Some(CallReturn {
                        destination: PlaceId(1),
                        next: BlockId(3),
                    }),
                    on_failure: CleanupId(0),
                },
            )
        } else {
            block(vec![initialize], Terminator::Goto(BlockId(3)))
        };
        f.body().blocks.push(other);
        let mut call = call;
        if let Terminator::Call {
            returns: Some(CallReturn { next, .. }),
            ..
        } = &mut call
        {
            *next = BlockId(4);
        }
        f.body().blocks.push(block(vec![], call));
        f.body().blocks.push(exit);
        let kinds = f.kinds();
        if unknown {
            assert!(
                kinds.contains(&ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
                "{kinds:?}"
            );
        } else {
            assert!(kinds.is_empty(), "{kinds:?}");
        }
    }
}

#[test]
fn indirect_generic_calls_check_actuals_arity_kinds_and_bounds() {
    let mut f = Fixture::call();
    let argument_type = f.body().places[1].ty;
    let parameter = f
        .definition
        .types
        .push(ty(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
    let result = f.body().result;
    let callable = FunctionType {
        state_promises: Default::default(),
        generics: vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Copyable].into(),
        }]
        .into(),
        parameters: vec![Parameter {
            name: None,
            label: None,
            passing: PassingMode::Take,
            ty: parameter,
        }]
        .into(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    };
    let callee = f.slot(TypeKind::Function(callable), Storage::Parameter);
    let local = f.body().places[callee as usize].root;
    f.body().parameters.push(local);
    owner_signature(&mut f, PassingMode::Borrow);
    if let Terminator::Call { callee: target, .. } = &mut f.body().blocks[0].terminator {
        *target = Callee::Indirect {
            forward_caller: false,
            value: operand(callee, UseKind::Borrow),
            arguments: vec![GenericArgument::Type(argument_type)],
        };
    }
    f.valid();
    let valid = (f.definition.clone(), f.signatures.clone());
    if let Terminator::Call {
        callee: Callee::Indirect { arguments, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    {
        arguments.clear();
    }
    assert!(f.kinds().contains(&ViolationKind::Arity {
        entity: "generic arguments",
        expected: 1,
        actual: 0
    }));
    f.definition = valid.0.clone();
    f.signatures = valid.1.clone();
    if let Terminator::Call {
        callee: Callee::Indirect { arguments, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    {
        arguments[0] = GenericArgument::Size(Size::Constant(3));
    }
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "generic argument kind"
    }));
    f.definition = valid.0;
    f.signatures = valid.1;
    let root = f.body().places[callee as usize].ty;
    let mut row = interner().get(root).unwrap().clone();
    if let TypeKind::Function(fun) = &mut row.kind {
        fun.generics[0] = GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Plain].into(),
        };
    }
    let root = f.definition.types.push(row);
    f.body().places[callee as usize].ty = root;
    f.body().locals[local.0 as usize].ty = root;
    owner_signature(&mut f, PassingMode::Borrow);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "generic bound"
    }));
}

#[test]
fn global_closure_captures_cannot_retain_scoped_storage() {
    let mut f = copied_capture_fixture(Uniqueness::Unique, Uniqueness::Aliased);
    f.valid();
    for body in &mut f.definition.bodies {
        for index in 0..f.definition.types.len() {
            let id = f.definition.types.id(index).unwrap();
            if matches!(
                interner().get(id).unwrap().kind,
                TypeKind::Primitive(Primitive::String)
            ) {
                let mut row = interner().get(id).unwrap().clone();
                row.modes.relation = row
                    .modes
                    .relation
                    .union(&Relation::new([Reach::Region(hirsel_types::RegionId(0))]));
                let placed = f.definition.types.push(row);
                for local in &mut body.locals {
                    if local.ty == id {
                        local.ty = placed;
                    }
                }
                for place in &mut body.places {
                    if place.ty == id {
                        place.ty = placed;
                    }
                }
            }
        }
    }
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "closure capture placement"
        }),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn acyclic_structural_type_graphs_at_the_source_boundary_validate() {
    let mut f = Fixture::unit();
    let mut previous = f.definition.types.id(0).unwrap();
    for _ in 1..1024 {
        previous = f.definition.types.push(ty(TypeKind::Box(previous)));
    }
    f.valid();
}

#[test]
fn counted_copy_capture_rederives_sendability_after_weakening_uniqueness() {
    for claimed_sendable in [false, true] {
        let mut f = copied_capture_fixture(Uniqueness::Unique, Uniqueness::Aliased);
        let source_ty = f.body().locals[1].ty;
        let mut source = interner().get(source_ty).unwrap().clone();
        source.modes.sendability = Sendability::Sendable;
        let source = f.definition.types.push(source);
        f.body().locals[1].ty = source;
        f.body().places[1].ty = source;
        if claimed_sendable {
            let nested = &mut f.definition.bodies[1];
            let mut capture = interner().get(nested.locals[1].ty).unwrap().clone();
            capture.modes.sendability = Sendability::Sendable;
            let capture = f.definition.types.push(capture);
            nested.locals[1].ty = capture;
            nested.places[1].ty = capture;
            f.broken(vec![ViolationKind::TypeMismatch {
                entity: "closure capture",
            }]);
        } else {
            f.valid();
        }
    }
}

#[test]
fn counted_copy_capture_cannot_preserve_its_sources_unique_promise() {
    let f = copied_capture_fixture(Uniqueness::Unique, Uniqueness::Unique);
    f.broken(vec![ViolationKind::TypeMismatch {
        entity: "closure capture",
    }]);
}

#[test]
fn cleanup_chain_parents_are_checked_before_traversal() {
    let mut f = Fixture::unit();
    let id = f.cleanup(vec![drop(0)]);
    f.valid();
    let head = f.body().cleanups[id as usize].head.unwrap();
    f.body().cleanup_nodes[head.0 as usize].parent = Some(head);
    assert!(f.kinds().iter().any(|k| matches!(
        k,
        ViolationKind::InvalidId {
            arena: "cleanup parent",
            ..
        }
    )));
    f.body().cleanup_nodes[head.0 as usize].parent = Some(CleanupNodeId(u32::MAX));
    assert!(f.kinds().iter().any(|k| matches!(
        k,
        ViolationKind::InvalidId {
            arena: "cleanup parent",
            ..
        }
    )));
    f.body().cleanup_nodes[head.0 as usize].parent = None;
    f.body().cleanups[id as usize].head = Some(CleanupNodeId(u32::MAX));
    assert!(f.kinds().iter().any(|k| matches!(
        k,
        ViolationKind::InvalidId {
            arena: "cleanup node",
            ..
        }
    )));
}

#[test]
fn storage_retirement_reproves_plain_and_scope_exit_rejects_live_owners() {
    let mut plain = Fixture::unit();
    let p = plain.slot(TypeKind::Primitive(Primitive::U8), Storage::Local);
    plain.body().scopes.push(Scope {
        parent: Some(ScopeId(0)),
        span: Span::default(),
    });
    plain.body().locals[p as usize].scope = ScopeId(1);
    plain.assign(p, Rvalue::Constant(Constant::Integer(7)), 0);
    plain.emit(InstructionKind::EndScope(ScopeId(1)));
    plain.emit(InstructionKind::StorageLive(LocalId(p)));
    plain.assign(p, Rvalue::Constant(Constant::Integer(8)), 0);
    plain.emit(InstructionKind::StorageDead {
        place: PlaceId(p),
        if_live: false,
    });
    plain.valid();
    let mut counted = Fixture::string();
    counted.emit(InstructionKind::StorageDead {
        place: PlaceId(1),
        if_live: true,
    });
    assert!(
        counted
            .kinds()
            .contains(&ViolationKind::InvalidUse(PlaceId(1)))
    );
    let mut leaking = Fixture::string();
    leaking.body().scopes.push(Scope {
        parent: Some(ScopeId(0)),
        span: Span::default(),
    });
    leaking.body().locals[1].scope = ScopeId(1);
    leaking.emit(InstructionKind::EndScope(ScopeId(1)));
    assert!(
        leaking
            .kinds()
            .contains(&ViolationKind::LeakedSlot(LocalId(1)))
    );
}

#[test]
fn map_entry_values_are_writable_but_keys_and_whole_entries_are_readonly() {
    for kind in [BuiltinType::Map, BuiltinType::SortedMap] {
        let mut f = Fixture::unit();
        let mut scalar = ty(TypeKind::Primitive(Primitive::U64));
        scalar.modes.sendability = Sendability::Sendable;
        let scalar = f.definition.types.push(scalar);
        let map = f.slot(
            TypeKind::Builtin {
                kind,
                arguments: vec![GenericArgument::Type(scalar), GenericArgument::Type(scalar)]
                    .into(),
            },
            Storage::Parameter,
        );
        f.body().parameters.push(LocalId(map));
        let index = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.assign(index, Rvalue::Constant(Constant::Integer(0)), 0);
        let target = f.body().places.len() as u32;
        f.body().places.push(Place {
            loan: None,
            binding: None,
            root: LocalId(map),
            projection: vec![Projection::Index(LocalId(index)), Projection::Field(1)].into(),
            ty: scalar,
        });
        let replacement = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.assign(replacement, Rvalue::Constant(Constant::Integer(3)), 0);
        revise_comparison_place(&mut f, replacement, |row| {
            row.modes.sendability = Sendability::Sendable
        });
        let displaced = f.slot(TypeKind::Primitive(Primitive::U64), Storage::Temporary);
        f.emit(InstructionKind::Replace {
            destination: PlaceId(target),
            value: take(replacement),
            displaced: PlaceId(displaced),
            if_live: false,
        });
        f.emit(InstructionKind::Drop {
            place: PlaceId(displaced),
            if_live: false,
        });
        owner_signature(&mut f, PassingMode::Mut);
        f.valid();
        f.body().places[target as usize].projection[1] = Projection::Field(0);
        assert!(
            f.kinds()
                .contains(&ViolationKind::InvalidUse(PlaceId(target)))
        );
        let mut entry = ty(TypeKind::Tuple(vec![scalar, scalar].into()));
        entry.modes.sendability = Sendability::Sendable;
        let entry = f.definition.types.push(entry);
        f.body().places[target as usize].projection.pop();
        f.body().places[target as usize].ty = entry;
        f.body().blocks[0].instructions[3].kind = InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(target),
            mutable: false,
            if_inactive: false,
        };
        f.body().blocks[0].instructions[4].kind = InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        };
        f.valid();
        f.body().blocks[0].instructions[3].kind = InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(target),
            mutable: true,
            if_inactive: false,
        };
        assert!(
            f.kinds()
                .contains(&ViolationKind::InvalidUse(PlaceId(target)))
        );
    }
}

#[test]
fn whole_root_failure_cleanup_reuses_incremental_obligation_proofs() {
    for count in [16, 64] {
        let mut f = Fixture::unit();
        let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Local);
        f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
        let mut cleanup = 0;
        let mut parent = None;
        for _ in 0..count {
            let root = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
            f.assign(
                root,
                Rvalue::Constant(Constant::String("owner".into())),
                cleanup,
            );
            let body = f.body();
            let node = CleanupNodeId(body.cleanup_nodes.len() as u32);
            body.cleanup_nodes.push(CleanupNode {
                action: drop(root),
                parent,
            });
            parent = Some(node);
            cleanup = body.cleanups.len() as u32;
            body.cleanups.push(Cleanup { head: parent });
            body.blocks[0].instructions.push(inst(
                InstructionKind::Check {
                    condition: copy(condition),
                    rule: const { hirsel_diag::RuleId::of("FACT-1") },
                },
                cleanup,
            ));
        }
        f.body().blocks[0].terminator = ret(0, cleanup);
        let before =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCleanupReplaySteps);
        f.valid();
        let steps =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCleanupReplaySteps)
                - before;
        assert!(
            steps <= 8 * count,
            "{count} owners replayed {steps} cleanup actions"
        );
        // A stale failure head cannot reuse the proof for the current live set.
        f.body().blocks[0]
            .instructions
            .last_mut()
            .unwrap()
            .on_failure = CleanupId(0);
        assert!(
            f.kinds()
                .iter()
                .any(|kind| matches!(kind, ViolationKind::CleanupMismatch { .. }))
        );
    }
}

#[test]
fn independent_loan_ends_share_the_incremental_failure_proof() {
    for count in [16, 64] {
        let mut f = Fixture::unit();
        let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Local);
        f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
        let owner = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
        f.assign(
            owner,
            Rvalue::Constant(Constant::String("borrowed".into())),
            0,
        );
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(owner),
            mutable: false,
            if_inactive: false,
        });
        let mut parent = None;
        let mut cleanup = 0;
        for root_index in 0..count {
            let root = if root_index == 0 {
                owner
            } else {
                let root = f.slot(TypeKind::Primitive(Primitive::String), Storage::Local);
                f.assign(
                    root,
                    Rvalue::Constant(Constant::String("owner".into())),
                    cleanup,
                );
                root
            };
            let body = f.body();
            let drop_node = CleanupNodeId(body.cleanup_nodes.len() as u32);
            body.cleanup_nodes.push(CleanupNode {
                action: drop(root),
                parent,
            });
            parent = Some(drop_node);
            let head = CleanupNodeId(body.cleanup_nodes.len() as u32);
            body.cleanup_nodes.push(CleanupNode {
                action: CleanupAction::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                parent,
            });
            cleanup = body.cleanups.len() as u32;
            body.cleanups.push(Cleanup { head: Some(head) });
            body.blocks[0].instructions.push(inst(
                InstructionKind::Check {
                    condition: copy(condition),
                    rule: const { hirsel_diag::RuleId::of("FACT-1") },
                },
                cleanup,
            ));
        }
        f.body().blocks[0].terminator = ret(0, cleanup);
        let before =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCleanupReplaySteps);
        f.valid();
        let steps =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCleanupReplaySteps)
                - before;
        assert!(
            steps <= 8 * (count + 1),
            "{count} owners replayed {steps} actions"
        );
        f.body().cleanups[cleanup as usize].head = parent;
        assert!(
            f.kinds()
                .iter()
                .any(|kind| matches!(kind, ViolationKind::LiveLoanAtExit(LoanId(0))))
        );
    }
}

#[test]
fn view_verdicts_preserve_universe_errors_and_their_order() {
    let mut f = Fixture::unit();
    for name in ["a", "z"] {
        let mut signature = function_signature(name, vec![], TypeKind::Tuple(vec![].into()));
        interner().intern(ty(TypeKind::Error));
        f.signatures.insert(id(name), signature);
    }
    let mut verdicts = ValidatedSignatures::default();
    let universe = |verdicts: &mut ValidatedSignatures, signatures: &SignatureMap| {
        verdicts.universe(
            interner(),
            signatures.keys().collect::<Vec<_>>(),
            signatures,
        )
    };
    let checked = universe(&mut verdicts, &f.signatures);
    assert_eq!(
        (checked.digest, checked.errors.as_slice()),
        f.signatures.universe_errors()
    );
    let mut reads = vec![];
    assert_eq!(
        validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut reads
        )
        .violations,
        checked.errors
    );
    assert!(reads.contains(&VRead::UniverseErrors(checked.digest)));
    for name in ["a", "m", "z"] {
        for malformed in [false, true] {
            let mut replacement = function_signature(name, vec![], TypeKind::Tuple(vec![].into()));
            if malformed {
                interner().intern(ty(TypeKind::Error));
            }
            let mut replaced = f.signatures.clone();
            replaced.insert(id(name), replacement);
            let checked = universe(&mut verdicts, &replaced);
            assert_eq!(
                (checked.digest, checked.errors.as_slice()),
                replaced.universe_errors()
            );
            assert_eq!(
                validate(&tabled(&f.definition), interner(), &replaced, &mut vec![]).violations,
                checked.errors
            );
        }
    }
    // Structural errors come first and alone.
    f.definition.entry = BodyId(u32::MAX);
    assert!(matches!(
        validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut vec![]
        )
        .violations[..],
        [Violation {
            kind: ViolationKind::InvalidId { .. },
            ..
        }]
    ));
}

fn literal_collection(constructor: Constructor, keys: &[u128]) -> (Fixture, u32) {
    literal_collection_of(constructor, keys, Primitive::U64)
}

fn literal_collection_of(
    constructor: Constructor,
    keys: &[u128],
    primitive: Primitive,
) -> (Fixture, u32) {
    let mut f = Fixture::unit();
    let constant = |n: u128| {
        if primitive == Primitive::F64 {
            Constant::F64((n as f64).to_bits())
        } else {
            Constant::Integer(n)
        }
    };
    let scalar = f.definition.types.push(ty(TypeKind::Primitive(primitive)));
    let mut fields = vec![];
    for (ordinal, key) in keys.iter().enumerate() {
        let k = f.slot(TypeKind::Primitive(primitive), Storage::Temporary);
        f.assign(k, Rvalue::Constant(constant(*key)), 0);
        let element = if constructor == Constructor::Map {
            let v = f.slot(TypeKind::Primitive(primitive), Storage::Temporary);
            f.assign(v, Rvalue::Constant(constant(ordinal as u128)), 0);
            let entry = f.slot(
                TypeKind::Tuple(vec![scalar, scalar].into()),
                Storage::Temporary,
            );
            let entry_type = f.body().places[entry as usize].ty;
            let canonical = normalize_type_properties(
                interner(),
                entry_type,
                &GenericDeclarations::default(),
                &mut |_| Ok::<_, hirsel_types::QueryError>(None),
            )
            .unwrap();
            let root = f.body().places[entry as usize].root;
            f.body().places[entry as usize].ty = canonical;
            f.body().locals[root.0 as usize].ty = canonical;
            f.assign(
                entry,
                Rvalue::Construct {
                    destination: Destination::Inferred,
                    constructor: Constructor::Tuple,
                    fields: vec![
                        ConstructField {
                            field: 0,
                            value: copy(k),
                        },
                        ConstructField {
                            field: 1,
                            value: copy(v),
                        },
                    ],
                },
                0,
            );
            entry
        } else {
            k
        };
        fields.push(ConstructField {
            field: ordinal as u32,
            value: take(element),
        });
    }
    let arguments = if constructor == Constructor::Map {
        vec![GenericArgument::Type(scalar), GenericArgument::Type(scalar)]
    } else {
        vec![GenericArgument::Type(scalar)]
    };
    let collection = f.slot(
        TypeKind::Builtin {
            kind: if constructor == Constructor::Map {
                BuiltinType::Map
            } else {
                BuiltinType::Set
            },
            arguments: arguments.into(),
        },
        Storage::Local,
    );
    f.assign(
        collection,
        Rvalue::Construct {
            destination: Destination::Inferred,
            constructor,
            fields,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(collection)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    (f, collection)
}

#[test]
fn materialized_collections_require_distinct_literal_keys() {
    for constructor in [Constructor::Map, Constructor::Set] {
        for keys in [vec![], vec![2, 1], vec![2, 1, 3]] {
            literal_collection(constructor.clone(), &keys).0.valid();
        }
        let (f, _) = literal_collection(constructor.clone(), &[2, 1, 2]);
        assert_eq!(f.kinds(), vec![ViolationKind::InvalidConstructor]);
    }
}

#[test]
fn materialized_collection_keys_cannot_be_unknown_or_non_key_types() {
    for constructor in [Constructor::Map, Constructor::Set] {
        let (mut f, _) = literal_collection(constructor.clone(), &[1, 2]);
        // Arithmetic is outside the literal graph, even when its inputs are literal.
        let second_key = if constructor == Constructor::Map {
            4
        } else {
            2
        };
        f.body().rvalues[second_key] = Rvalue::Binary {
            op: BinaryOp::Add,
            left: copy(1),
            right: copy(1),
        };
        assert_eq!(f.kinds(), vec![ViolationKind::InvalidConstructor]);
        let (f, _) = literal_collection_of(constructor, &[1], Primitive::F64);
        assert!(f.kinds().contains(&ViolationKind::InvalidConstructor));
    }
}

#[test]
fn empty_materialized_maps_still_require_data_values() {
    let (mut f, collection) = literal_collection(Constructor::Map, &[]);
    let unit = f.body().result;
    let function = f.definition.types.push(ty(TypeKind::Function(FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: unit,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::inert(),
        uses: Uses::Only(AuthoritySummary::inert()),
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    })));
    let old = f.body().places[collection as usize].ty;
    let mut row = interner().get(old).unwrap().clone();
    let TypeKind::Builtin { arguments, .. } = &mut row.kind else {
        unreachable!()
    };
    arguments[1] = GenericArgument::Type(function);
    let new = f.definition.types.push(row);
    let root = f.body().places[collection as usize].root;
    f.body().places[collection as usize].ty = new;
    f.body().locals[root.0 as usize].ty = new;
    assert_eq!(f.kinds(), vec![ViolationKind::InvalidConstructor]);
}

fn mixed_replacement_fixture() -> (Fixture, u32, u32) {
    let mut f = Fixture::string();
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
    let moved = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let fresh = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let displaced = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let use_old = RvalueId(f.body().rvalues.len() as u32);
    f.body().rvalues.push(Rvalue::Use(take(1)));
    let new_value = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Constant(Constant::String("new".into())));
    let failure = f.cleanup(vec![drop(displaced), drop(1)]);
    f.body().blocks[0].terminator = Terminator::Branch {
        condition: copy(condition),
        yes: BlockId(1),
        no: BlockId(2),
    };
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(moved),
                    value: use_old,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(moved),
                    if_live: false,
                },
                0,
            ),
        ],
        Terminator::Goto(BlockId(3)),
    ));
    f.body()
        .blocks
        .push(block(vec![], Terminator::Goto(BlockId(3))));
    let end = f.cleanup(vec![drop(1)]);
    f.body().blocks.push(block(
        vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(fresh),
                    value: new_value,
                },
                0,
            ),
            inst(
                InstructionKind::Replace {
                    destination: PlaceId(1),
                    value: take(fresh),
                    displaced: PlaceId(displaced),
                    if_live: true,
                },
                0,
            ),
            inst(
                InstructionKind::Drop {
                    place: PlaceId(displaced),
                    if_live: true,
                },
                failure,
            ),
        ],
        ret(0, end),
    ));
    (f, moved, displaced)
}

#[test]
fn mixed_replacements_require_a_tied_conditional_release_and_the_canonical_flag() {
    let (f, _, displaced) = mixed_replacement_fixture();
    f.valid();
    let (mut f, _, _) = mixed_replacement_fixture();
    let InstructionKind::Replace { if_live, .. } = &mut f.body().blocks[3].instructions[1].kind
    else {
        unreachable!()
    };
    *if_live = false;
    assert!(f.kinds().contains(&ViolationKind::UseAfterMove(PlaceId(1))));
    let (mut f, _, _) = mixed_replacement_fixture();
    let InstructionKind::Drop { if_live, .. } = &mut f.body().blocks[3].instructions[2].kind else {
        unreachable!()
    };
    *if_live = false;
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidUse(PlaceId(displaced)))
    );
    let (mut f, _, _) = mixed_replacement_fixture();
    // Both alternatives keep the old value: a conditional exchange is noncanonical.
    f.body().blocks[1].instructions.clear();
    f.body().rvalues.remove(3); // remove the now-unreferenced Take producer
    let InstructionKind::Assign { value, .. } = &mut f.body().blocks[3].instructions[0].kind else {
        unreachable!()
    };
    value.0 -= 1;
    assert!(f.kinds().contains(&ViolationKind::InvalidUse(PlaceId(1))));
    let (mut f, _, _) = mixed_replacement_fixture();
    // Both alternatives move the old value: this must be a plain initialization.
    f.body().blocks[2].instructions = f.body().blocks[1].instructions.clone();
    assert!(f.kinds().contains(&ViolationKind::InvalidUse(PlaceId(1))));
}

#[test]
fn loop_feedback_expires_exact_written_prefixes_and_owned_aliases() {
    // Each acyclic fixture is valid. The only forged control-flow change is a
    // backedge carrying the same branded parameter into the next incarnation.
    for mode in ["whole", "prefix", "alias", "mut", "sibling"] {
        let mut f = Fixture::unit();
        let scalar = f
            .definition
            .types
            .push(ty(TypeKind::Primitive(Primitive::U8)));
        let inner = f
            .definition
            .types
            .push(ty(TypeKind::Tuple(vec![scalar, scalar].into())));
        let owner = f.slot(
            TypeKind::Tuple(vec![inner, scalar].into()),
            Storage::Parameter,
        );
        let handle = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Parameter);
        let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
        f.body().parameters = vec![LocalId(owner), LocalId(handle), LocalId(condition)];
        let scope = f.body().parameter_scope;
        let root = OriginRoot::Parameter(ParameterRef::body(scope, ParamId(0)));
        let carried = Origin {
            root: root.clone(),
            fields: if mode == "sibling" {
                vec![FieldId(1)]
            } else {
                vec![FieldId(0), FieldId(1)]
            }
            .into(),
        };
        let handle_ty = f.body().places[handle as usize].ty;
        let mut row = interner().get(handle_ty).unwrap().clone();
        row.brand.alternatives.push(carried);
        let branded = f.definition.types.push(row);
        f.body().locals[handle as usize].ty = branded;
        f.body().places[handle as usize].ty = branded;
        let scratch = f.slot(TypeKind::Primitive(Primitive::U8), Storage::Temporary);
        let displaced = f.slot(
            TypeKind::Tuple(vec![inner, scalar].into()),
            Storage::Temporary,
        );
        let replacement = f.slot(
            TypeKind::Tuple(vec![inner, scalar].into()),
            Storage::Parameter,
        );
        f.body().parameters.push(LocalId(replacement));
        let mut base = f.body().places[owner as usize].clone();
        base.binding = Some(Origin {
            root: root.clone(),
            fields: vec![].into(),
        });
        let base_id = PlaceId(f.body().places.len() as u32);
        f.body().places.push(base.clone());
        let mut mutation = vec![];
        if mode == "alias" {
            base.binding.as_mut().unwrap().root =
                OriginRoot::Local(lexical(), hirsel_types::LocalId(72));
            let alias = PlaceId(f.body().places.len() as u32);
            f.body().places.push(base.clone());
            f.emit(InstructionKind::BindOrigin {
                source: base_id,
                place: alias,
                kind: OriginBindingKind::Alias,
            });
        }
        if mode == "mut" {
            mutation.push(inst(
                InstructionKind::BeginLoan {
                    id: LoanId(0),
                    place: base_id,
                    mutable: true,
                    if_inactive: false,
                },
                0,
            ));
            base.loan = Some(LoanId(0));
        }
        if mode != "whole" {
            base.projection.push(Projection::Field(0));
            base.ty = inner;
            base.binding.as_mut().unwrap().fields.push(FieldId(0));
        }
        let written = PlaceId(f.body().places.len() as u32);
        f.body().places.push(base.clone());
        if matches!(mode, "alias" | "mut" | "whole") {
            mutation.push(inst(InstructionKind::EndOrigin(written), 0));
        }
        if mode == "whole" {
            mutation.push(inst(
                InstructionKind::Replace {
                    destination: written,
                    value: copy(replacement),
                    displaced: PlaceId(displaced),
                    if_live: false,
                },
                0,
            ));
            mutation.push(inst(
                InstructionKind::StorageDead {
                    place: PlaceId(displaced),
                    if_live: false,
                },
                0,
            ));
        }
        mutation.push(inst(
            InstructionKind::BindOrigin {
                source: written,
                place: written,
                kind: OriginBindingKind::Fresh,
            },
            0,
        ));
        if mode == "mut" {
            mutation.push(inst(
                InstructionKind::EndLoan {
                    loan: LoanId(0),
                    if_active: false,
                },
                0,
            ));
        }
        let read = RvalueId(f.body().rvalues.len() as u32);
        f.body()
            .rvalues
            .push(Rvalue::Use(operand(handle, UseKind::Borrow)));
        let mut instructions = vec![
            inst(
                InstructionKind::Assign {
                    destination: PlaceId(scratch),
                    value: read,
                },
                0,
            ),
            inst(
                InstructionKind::StorageDead {
                    place: PlaceId(scratch),
                    if_live: false,
                },
                0,
            ),
        ];
        instructions.extend(mutation);
        f.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
        f.body()
            .blocks
            .push(block(instructions, Terminator::Goto(BlockId(2))));
        f.body().blocks.push(block(vec![], ret(0, 0)));
        owner_signature(&mut f, PassingMode::Take);
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let SignatureKind::Function(fun) = &mut signature.kind else {
            unreachable!()
        };
        if mode == "whole" {
            fun.parameters[0].passing = PassingMode::Mut;
        }
        for parameter in &mut fun.parameters {
            parameter.ty = map_parameter_references(interner(), parameter.ty, |p, depth| {
                Ok(match p {
                    ParameterRef::Body {
                        scope: owner,
                        parameter,
                    } if owner == scope => ParameterRef::Bound { depth, parameter },
                    p => p,
                })
            })
            .unwrap();
        }
        let report = validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut vec![],
        );
        assert!(report.violations.is_empty(), "acyclic {mode}: {report:?}");
        f.body().blocks[1].terminator = Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(1),
            no: BlockId(2),
        };
        let report = validate(
            &tabled(&f.definition),
            interner(),
            &f.signatures,
            &mut vec![],
        );
        if mode == "sibling" {
            assert!(report.violations.is_empty(), "{mode}: {report:?}");
        } else {
            assert!(
                report.violations.iter().any(|v| v.block == Some(BlockId(1))
                    && v.instruction == Some(0)
                    && v.kind == ViolationKind::InvalidOrigin(PlaceId(handle))),
                "{mode}: {report:?}"
            );
            assert!(
                report
                    .violations
                    .iter()
                    .all(|v| matches!(v.kind, ViolationKind::InvalidOrigin(_))),
                "{mode}: {report:?}"
            );
        }
    }
}

#[test]
fn literal_result_promises_weaken_but_cannot_invent_evidence() {
    let mut f = Fixture::unit();
    let mut child = Fixture::string();
    let mut row = interner()
        .get(child.definition.bodies[0].places[1].ty)
        .unwrap()
        .clone();
    row.modes.uniqueness = Uniqueness::Unique;
    row.modes.sendability = Sendability::Sendable;
    let unique = child.definition.types.push(row);
    child.body().locals[1].ty = unique;
    child.body().places[1].ty = unique;
    child.body().result = unique;
    child.body().parameter_scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(1)));
    child.body().blocks[0].terminator = ret(1, 0);
    let mut child_body = child.definition.bodies.remove(0);
    child_body.result = child_body.places[1].ty;
    let body_result = child_body.result;
    f.definition.bodies.push(child_body);
    let mut promise = interner().get(body_result).unwrap().clone();
    promise.modes.uniqueness = Uniqueness::Aliased;
    promise.modes.sendability = Sendability::NotSendable;
    let promise = f.definition.types.push(promise);
    let SignatureKind::Function(mut header) =
        function_signature("literal", vec![], TypeKind::Tuple(vec![].into())).kind
    else {
        unreachable!()
    };
    header.result = promise;
    header.allocation = Allocation::MayAllocate;
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
    let cleanup = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f.valid();
    // Forge a unique promise on an executable aliased result. The initializer
    // remains valid; only the independent promise proof must fail.
    f.definition.bodies[1].result = promise;
    let mut row = interner()
        .get(f.definition.bodies[0].places[closure as usize].ty)
        .unwrap()
        .clone();
    if let TypeKind::Function(header) = &mut row.kind {
        header.result = body_result;
    }
    let forged = f.definition.types.push(row);
    f.body().places[closure as usize].ty = forged;
    f.body().locals[closure as usize].ty = forged;
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "closure result"
    }));
}

// A Local receiving row can hold deep Global content. The field declaration
// stays Local: extraction proves its current content independently.
fn acquired_global_field_fixture(
    nested: bool,
    passing: PassingMode,
) -> (Fixture, u32, u32, u32, u32, u32) {
    let mut f = Fixture::unit();
    let bytes = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bytes)));
    let mut local = interner().get(bytes).unwrap().clone();
    local.modes.relation = Relation::environment();
    let local = f.definition.types.push(local);
    let inner = if nested {
        let mut row = ty(TypeKind::Tuple(vec![local].into()));
        row.modes.relation = Relation::environment();
        f.definition.types.push(row)
    } else {
        local
    };
    let kind = TypeKind::Tuple(vec![inner].into());
    let input = f.slot(kind.clone(), Storage::Parameter);
    let seed = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Parameter);
    f.body().locals[seed as usize].ty = local;
    f.body().places[seed as usize].ty = local;
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    f.body().parameters = vec![LocalId(input), LocalId(seed), LocalId(condition)];
    let root = if passing == PassingMode::Take {
        let root = f.slot(kind, Storage::Local);
        let mut row = interner()
            .get(f.definition.bodies[0].places[root as usize].ty)
            .unwrap()
            .clone();
        row.modes.relation = Relation::environment();
        let row = f.definition.types.push(row);
        f.body().locals[root as usize].ty = row;
        f.body().places[root as usize].ty = row;
        f.assign(root, Rvalue::Use(take(input)), 0);
        root
    } else {
        let mut row = interner()
            .get(f.definition.bodies[0].places[input as usize].ty)
            .unwrap()
            .clone();
        row.modes.relation = Relation::environment();
        let row = f.definition.types.push(row);
        f.body().locals[input as usize].ty = row;
        f.body().places[input as usize].ty = row;
        input
    };
    let result = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
    f.body().locals[result as usize].ty = local;
    f.body().places[result as usize].ty = local;
    let mut result_row = interner().get(bytes).unwrap().clone();
    result_row.modes.relation =
        Relation::parameters([ParameterRef::body(f.body().parameter_scope, ParamId(0))]);
    f.body().result = f.definition.types.push(result_row);
    let old = f.slot(TypeKind::Primitive(Primitive::Bytes), Storage::Temporary);
    f.body().locals[old as usize].ty = local;
    f.body().places[old as usize].ty = local;
    let field = f.body().places.len() as u32;
    f.body().places.push(Place {
        loan: None,
        binding: None,
        root: LocalId(root),
        projection: if nested {
            vec![Projection::Field(0), Projection::Field(0)]
        } else {
            vec![Projection::Field(0)]
        }
        .into(),
        ty: local,
    });
    let mut actions = vec![drop(result), drop(old)];
    if passing == PassingMode::Take {
        actions.push(drop(root));
    }
    let cleanup = f.cleanup(actions);
    f.assign(result, Rvalue::Use(copy(field)), 0);
    f.body().blocks[0].terminator = ret(result, cleanup);
    owner_signature(&mut f, passing);
    if let SignatureKind::Function(_) = &mut f.signatures.get_mut(&path("main")).unwrap().kind {
        let scope = f.definition.bodies[0].parameter_scope;
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let SignatureKind::Function(fun) = &mut signature.kind else {
            unreachable!()
        };
        fun.result = map_parameter_references(interner(), fun.result, |p, depth| {
            Ok(match p {
                ParameterRef::Body {
                    scope: owner,
                    parameter,
                } if owner == scope => ParameterRef::Bound { depth, parameter },
                p => p,
            })
        })
        .unwrap();
        fun.parameters[1].passing = PassingMode::Borrow;
        fun.parameters[2].passing = PassingMode::Take;
    }
    (f, field, seed, old, condition, result)
}

#[test]
fn projected_initialization_cannot_install_local_content_in_global_storage() {
    let (mut f, field, seed, old, _, result) =
        acquired_global_field_fixture(false, PassingMode::Take);
    let root = f.body().places[field as usize].root.0 as usize;
    let input = f.body().places[1].ty;
    let local = f.body().locals[root].ty;
    f.body().locals[root].ty = input;
    f.body().places[root].ty = input;
    f.body().blocks[0].instructions.pop();
    f.body().rvalues.pop();
    let failure = f.cleanup(vec![drop(root as u32)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::Drop {
            place: PlaceId(field),
            if_live: false,
        },
        failure,
    ));
    f.assign(field, Rvalue::Use(copy(seed)), failure);
    let moved = f.slot(interner().get(local).unwrap().kind.clone(), Storage::Local);
    let moved_root = f.body().places[moved as usize].root.0 as usize;
    f.body().locals[moved_root].ty = local;
    f.body().places[moved as usize].ty = local;
    f.assign(moved, Rvalue::Use(take(root as u32)), 0);
    let cleanup = f.cleanup(vec![drop(moved), drop(old), drop(result)]);
    f.body().result = f.body().places[0].ty;
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Take);
    let SignatureKind::Function(fun) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
    else {
        unreachable!()
    };
    fun.parameters[1].passing = PassingMode::Borrow;
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(field)))
    );
}

#[test]
fn acquired_fields_require_deep_current_global_content() {
    for nested in [false, true] {
        let (f, field, seed, old, condition, result) =
            acquired_global_field_fixture(nested, PassingMode::Take);
        f.valid();
        let mut written = f;
        let read = written.body().blocks[0].instructions.pop().unwrap();
        let replace = inst(
            InstructionKind::Replace {
                destination: PlaceId(field),
                value: copy(seed),
                displaced: PlaceId(old),
                if_live: false,
            },
            0,
        );
        written.body().blocks[0]
            .instructions
            .extend([replace.clone(), read.clone()]);
        let check = |f: &Fixture| {
            let errors = f.kinds();
            assert!(
                errors.contains(&ViolationKind::InvalidResultSource(PlaceId(result))),
                "{errors:?}"
            );
        };
        check(&written);
        // The receiving root remains Global only if every live predecessor does.
        let mut joined = written;
        joined.body().blocks[0].instructions.truncate(2);
        let exit = joined.body().blocks[0].terminator.clone();
        joined.body().blocks[0].terminator = Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(1),
            no: BlockId(2),
        };
        joined
            .body()
            .blocks
            .push(block(vec![replace.clone()], Terminator::Goto(BlockId(3))));
        joined
            .body()
            .blocks
            .push(block(vec![], Terminator::Goto(BlockId(3))));
        joined
            .body()
            .blocks
            .push(block(vec![read.clone()], exit.clone()));
        check(&joined);
        // A read before this iteration's write must include the last iteration.
        let mut looped = joined;
        looped.body().blocks[0].terminator = Terminator::Goto(BlockId(1));
        looped.body().blocks[1] = block(
            vec![
                inst(
                    InstructionKind::Drop {
                        place: PlaceId(result),
                        if_live: true,
                    },
                    0,
                ),
                read,
                replace,
                inst(
                    InstructionKind::Drop {
                        place: PlaceId(old),
                        if_live: false,
                    },
                    0,
                ),
            ],
            Terminator::Branch {
                condition: copy(condition),
                yes: BlockId(1),
                no: BlockId(2),
            },
        );
        looped.body().blocks.truncate(3);
        looped.body().blocks[2] = block(vec![], exit);
        check(&looped);
    }
    for passing in [PassingMode::Borrow, PassingMode::Mut] {
        let (mut f, _, _, _, _, result) = acquired_global_field_fixture(true, passing);
        // The result may name another borrowed input, never this receiver.
        // A fabricated Global acquisition would incorrectly erase that bound.
        let mut receiver = interner()
            .get(f.definition.bodies[0].locals[1].ty)
            .unwrap()
            .clone();
        receiver.modes.relation = Relation::global();
        let receiver = f.definition.types.push(receiver);
        f.body().locals[1].ty = receiver;
        f.body().places[1].ty = receiver;
        let mut result_row = ty(TypeKind::Primitive(Primitive::Bytes));
        result_row.modes.relation =
            Relation::parameters([ParameterRef::body(f.body().parameter_scope, ParamId(1))]);
        f.body().result = f.definition.types.push(result_row);
        owner_signature(&mut f, passing);
        let scope = f.body().parameter_scope;
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let SignatureKind::Function(fun) = &mut signature.kind else {
            unreachable!()
        };
        fun.result = map_parameter_references(interner(), fun.result, |p, depth| {
            Ok(match p {
                ParameterRef::Body {
                    scope: owner,
                    parameter,
                } if owner == scope => ParameterRef::Bound { depth, parameter },
                p => p,
            })
        })
        .unwrap();
        fun.parameters[1].passing = PassingMode::Borrow;
        fun.parameters[2].passing = PassingMode::Take;
        let errors = f.kinds();
        assert!(
            errors.contains(&ViolationKind::InvalidResultSource(PlaceId(result))),
            "{passing:?}: {errors:?}"
        );
    }
}

#[test]
fn local_receiving_writes_invalidate_global_content_and_keep_the_installed_source() {
    for nested in [false, true] {
        let (mut f, field, seed, old, _, result) =
            acquired_global_field_fixture(nested, PassingMode::Take);
        let read = f.body().blocks[0].instructions.pop().unwrap();
        f.body().blocks[0].instructions.extend([
            inst(
                InstructionKind::Replace {
                    destination: PlaceId(field),
                    value: copy(seed),
                    displaced: PlaceId(old),
                    if_live: false,
                },
                0,
            ),
            read,
        ]);
        assert!(
            f.kinds()
                .contains(&ViolationKind::InvalidResultSource(PlaceId(result)))
        );
        let scope = f.body().parameter_scope;
        let body_result = f.body().result;
        let mut row = interner().get(body_result).unwrap().clone();
        row.modes.relation = Relation::parameters([ParameterRef::body(scope, ParamId(1))]);
        let result_type = f.definition.types.push(row);
        f.body().result = result_type;
        let signature = f.signatures.get_mut(&path("main")).unwrap();
        let SignatureKind::Function(fun) = &mut signature.kind else {
            unreachable!()
        };
        let mut row = interner().get(fun.result).unwrap().clone();
        row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(1))]);
        fun.result = interner().intern(row);
        f.valid();
        // The same installed source is forbidden by a Global receiving root.
        let root = f.body().places[field as usize].root;
        let root_type = f.body().locals[root.0 as usize].ty;
        let mut row = interner().get(root_type).unwrap().clone();
        row.modes.relation = Relation::global();
        let global = f.definition.types.push(row);
        f.body().locals[root.0 as usize].ty = global;
        for place in &mut f.body().places {
            if place.root == root && place.projection.is_empty() {
                place.ty = global;
            }
        }
        assert!(
            f.kinds()
                .contains(&ViolationKind::BorrowedMove(PlaceId(seed))),
            "{:?}",
            f.kinds()
        );
    }
}

#[test]
fn local_physical_projection_survives_global_current_content() {
    let (mut f, field, seed, old, _, result) =
        acquired_global_field_fixture(false, PassingMode::Take);
    let input = f.body().parameters[0];
    let input_type = f.body().locals[input.0 as usize].ty;
    let TypeKind::Tuple(parts) = &interner().get(input_type).unwrap().kind else {
        unreachable!()
    };
    let mut payload = interner().get(parts[0]).unwrap().clone();
    payload.modes.relation = Relation::global();
    let payload = f.definition.types.push(payload);
    let mut input_row = interner().get(input_type).unwrap().clone();
    input_row.kind = TypeKind::Tuple(vec![payload].into());
    let input_type = f.definition.types.push(input_row);
    let root = f.body().places[field as usize].root;
    let root_type = f.body().locals[root.0 as usize].ty;
    let mut root_row = interner().get(root_type).unwrap().clone();
    root_row.kind = TypeKind::Tuple(vec![payload].into());
    let root_type = f.definition.types.push(root_row);
    f.body().locals[input.0 as usize].ty = input_type;
    f.body().locals[root.0 as usize].ty = root_type;
    for place in &mut f.body().places {
        if place.projection.is_empty() {
            if place.root == input {
                place.ty = input_type;
            } else if place.root == root {
                place.ty = root_type;
            }
        }
    }
    // A default field follows current deep-Global content; an explicitly
    // Local field under this same Local receiving owner may not strengthen.
    let body_result = f.body().result;
    let mut row = interner().get(body_result).unwrap().clone();
    row.modes.relation = Relation::global();
    let global_result = f.definition.types.push(row);
    f.body().result = global_result;
    f.body().locals[result as usize].ty = global_result;
    f.body().places[result as usize].ty = global_result;
    let signature = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    let mut row = interner().get(fun.parameters[0].ty).unwrap().clone();
    let TypeKind::Tuple(parts) = &row.kind else {
        unreachable!()
    };
    let mut payload = interner().get(parts[0]).unwrap().clone();
    payload.modes.relation = Relation::global();
    let payload = interner().intern(payload);
    row.kind = TypeKind::Tuple(vec![payload].into());
    fun.parameters[0].ty = interner().intern(row);
    let mut row = interner().get(fun.result).unwrap().clone();
    row.modes.relation = Relation::global();
    fun.result = interner().intern(row);
    f.valid();
    let read = f.body().blocks[0].instructions.pop().unwrap();
    f.body().blocks[0].instructions.extend([
        inst(
            InstructionKind::Replace {
                destination: PlaceId(field),
                value: copy(seed),
                displaced: PlaceId(old),
                if_live: false,
            },
            0,
        ),
        read,
    ]);
    // The same static acquisition shape cannot export retained Local content.
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidResultSource(PlaceId(result))),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn explicitly_local_parts_of_local_receivers_cannot_claim_global_acquisition() {
    let (mut f, _, _, _, _, result) = acquired_global_field_fixture(false, PassingMode::Take);
    f.valid();
    let previous = f.body().result;
    let mut row = interner().get(previous).unwrap().clone();
    row.modes.relation = Relation::global();
    let global = f.definition.types.push(row);
    f.body().result = global;
    f.body().locals[result as usize].ty = global;
    f.body().places[result as usize].ty = global;
    let signature = f.signatures.get_mut(&path("main")).unwrap();
    let SignatureKind::Function(fun) = &mut signature.kind else {
        unreachable!()
    };
    let mut row = interner().get(fun.result).unwrap().clone();
    row.modes.relation = Relation::global();
    fun.result = interner().intern(row);
    assert!(
        f.kinds().contains(&ViolationKind::TypeMismatch {
            entity: "assignment"
        }),
        "{:?}",
        f.kinds()
    );
}

#[test]
fn local_closure_results_retain_capture_content_but_not_callable_access() {
    // Hand-built IR: two distinct loans, one on the captured content and
    // one used only to invoke the closure. The latter ends before the result
    // is returned; the former must still bound that result.
    let mut f = Fixture::unit();
    let seed = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    let mut local = ty(TypeKind::Primitive(Primitive::String));
    local.modes.relation = Relation::environment();
    let local = f.definition.types.push(local);
    f.body().locals[seed as usize].ty = local;
    f.body().places[seed as usize].ty = local;
    f.body().parameters = vec![LocalId(seed)];
    let mut result = interner().get(local).unwrap().clone();
    result.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(0))]);
    let result = f.definition.types.push(result);
    f.body().result = result;
    owner_signature(&mut f, PassingMode::Borrow);
    let scope = f.body().parameter_scope;
    f.body().result = bind_parameter_scopes(interner(), result, &[scope]).unwrap();

    let mut child = f.body().clone();
    child.parameter_scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(1)));
    child.locals = vec![Local {
        ty: local,
        storage: Storage::Capture,
        scope: ScopeId(0),
        span: Span::default(),
    }];
    child.places = vec![Place {
        root: LocalId(0),
        projection: vec![].into(),
        ty: local,
        loan: None,
        binding: None,
    }];
    child.parameters.clear();
    child.result = local;
    child.rvalues.clear();
    child.blocks = vec![block(
        vec![],
        Terminator::Return {
            value: copy(0),
            cleanup: CleanupId(0),
        },
    )];
    f.definition.bodies.push(child);
    let SignatureKind::Function(mut callable) =
        function_signature("unused", vec![], TypeKind::Tuple(vec![].into())).kind
    else {
        unreachable!()
    };
    callable.result = local;
    let closure = f.slot(TypeKind::Function(callable), Storage::Temporary);
    let closure_ty = f.body().places[closure as usize].ty;
    let mut scoped = interner().get(closure_ty).unwrap().clone();
    scoped.modes.relation = Relation::environment();
    let scoped = f.definition.types.push(scoped);
    f.body().locals[closure as usize].ty = scoped;
    f.body().places[closure as usize].ty = scoped;
    let destination = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.body().locals[destination as usize].ty = local;
    f.body().places[destination as usize].ty = local;
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(seed),
        mutable: false,
        if_inactive: false,
    });
    let capture = f.through_loan(seed, 0);
    let capture_failure = f.cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: false,
    }]);
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![operand(capture, UseKind::Borrow)],
            destination: Destination::Inferred,
        },
        capture_failure,
    );
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(1),
        place: PlaceId(closure),
        mutable: false,
        if_inactive: false,
    });
    let callee = f.through_loan(closure, 1);
    let failure = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(1),
            if_active: false,
        },
        drop(closure),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    let cleanup = f.cleanup(vec![
        drop(closure),
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
    ]);
    f.body().blocks[0].terminator = Terminator::Call {
        callee: Callee::Indirect {
            forward_caller: false,
            value: operand(callee, UseKind::Borrow),
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(destination),
            next: BlockId(1),
        }),
        on_failure: CleanupId(failure),
    };
    f.body().blocks.push(block(
        vec![inst(
            InstructionKind::EndLoan {
                loan: LoanId(1),
                if_active: false,
            },
            0,
        )],
        ret(destination, cleanup),
    ));
    f.valid();
    // Releasing the capture permission expires the returned content, even
    // though invoking the closure no longer retains its own access loan.
    f.body().blocks[1].instructions.push(inst(
        InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        0,
    ));
    assert!(f.kinds().iter().any(|kind| matches!(
        kind,
        ViolationKind::ExpiredLoan {
            loan: LoanId(0),
            ..
        }
    )));
}

#[test]
fn a_dependent_callable_receives_proven_or_matching_crossing_evidence() {
    for supplied in [
        Sendability::Sendable,
        Sendability::NotSendable,
        Sendability::DependsOn(
            vec![TypeParameterRef::body(
                scoped(NameSite::Item),
                TypeParamId(0),
            )]
            .into(),
        ),
    ] {
        let mut f = Fixture::unit();
        let unit = f.definition.types.push(Type::unit());
        f.body().result = unit;
        f.body().locals[0].ty = unit;
        f.body().places[0].ty = unit;
        let (_, _, holder) = nested_callback_shapes(&mut f, false).swap_remove(0);
        let TypeKind::Tuple(parts) = holder else {
            panic!()
        };
        let callable = interner().get(parts[0]).unwrap().kind.clone();
        let mut f = value_flow_fixture(f, callable.clone(), callable, 2);
        let source = f.body().parameters[0];
        let held = f.body().locals[source.0 as usize].ty;
        let mut row = interner().get(held).unwrap().clone();
        row.modes.sendability = supplied.clone();
        let actual = f.definition.types.push(row);
        f.body().locals[source.0 as usize].ty = actual;
        for place in &mut f.body().places {
            if place.root == source {
                place.ty = actual;
            }
        }
        let generic = GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into(),
        };
        let scope = f.body().parameter_scope;
        let mut parameter_row = ty(TypeKind::Parameter(TypeParameterRef::body(
            scope,
            TypeParamId(0),
        )));
        parameter_row.modes.sendability =
            Sendability::DependsOn(vec![TypeParameterRef::body(scope, TypeParamId(0))].into());
        let parameter = f.definition.types.push(parameter_row);
        let Terminator::Call {
            callee: Callee::Definition { arguments, .. },
            ..
        } = &mut f.body().blocks[0].terminator
        else {
            panic!()
        };
        *arguments = vec![GenericArgument::Type(parameter)];
        owner_signature(&mut f, PassingMode::Take);
        let SignatureKind::Function(owner) = &mut f.signatures.get_mut(&path("main")).unwrap().kind
        else {
            panic!()
        };
        owner.generics = vec![generic.clone()].into();
        let signature = f.signatures.get_mut(&path("callee")).unwrap();
        let SignatureKind::Function(callee) = &mut signature.kind else {
            panic!()
        };
        callee.generics = vec![generic].into();
        let mut row = interner().get(callee.parameters[0].ty).unwrap().clone();
        row.modes.sendability =
            Sendability::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into());
        callee.parameters[0].ty = interner().intern(row);
        if supplied == Sendability::NotSendable {
            assert!(
                f.kinds().contains(&ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
                "{:?}",
                f.kinds()
            );
        } else {
            assert!(f.kinds().is_empty(), "{supplied:?}: {:?}", f.kinds());
        }
    }
}

/// Mut storage is both received and handed back: its callable multiplicity
/// is invariant. Owned reception still weakens many to once.
#[test]
fn a_mut_argument_keeps_its_root_callable_multiplicity_exact() {
    for passing in [PassingMode::Mut, PassingMode::Take] {
        for actual_once in [false, true] {
            for expected_once in [false, true] {
                let mut f = function_authority_flow_fixture(
                    AuthoritySummary::inert(),
                    AuthoritySummary::inert(),
                    Uses::Unrestricted,
                    Uses::Unrestricted,
                    2,
                );
                let source = f.body().parameters[0];
                let actual = f.body().locals[source.0 as usize].ty;
                let mut row = interner().get(actual).unwrap().clone();
                row.modes.multiplicity = if actual_once {
                    Multiplicity::Once
                } else {
                    Multiplicity::Many
                };
                let actual = f.definition.types.push(row);
                f.body().locals[source.0 as usize].ty = actual;
                f.body().places[source.0 as usize].ty = actual;
                let signature = f.signatures.get_mut(&path("callee")).unwrap();
                let SignatureKind::Function(callee) = &mut signature.kind else {
                    panic!()
                };
                let mut row = interner().get(callee.parameters[0].ty).unwrap().clone();
                row.modes.multiplicity = if expected_once {
                    Multiplicity::Once
                } else {
                    Multiplicity::Many
                };
                callee.parameters[0].ty = interner().intern(row);
                callee.parameters[0].passing = passing;
                let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator else {
                    panic!()
                };
                arguments[0].value.usage = if passing == PassingMode::Mut {
                    UseKind::Mut
                } else {
                    UseKind::Take
                };
                if passing == PassingMode::Mut {
                    let loan = hirsel_tir::LoanId(0);
                    f.emit(InstructionKind::BeginLoan {
                        id: loan,
                        place: PlaceId(source.0),
                        mutable: true,
                        if_inactive: false,
                    });
                    let mut place = f.body().places[source.0 as usize].clone();
                    place.loan = Some(loan);
                    let alias = PlaceId(f.body().places.len() as u32);
                    f.body().places.push(place);
                    let Terminator::Call { arguments, .. } = &mut f.body().blocks[0].terminator
                    else {
                        panic!()
                    };
                    arguments[0].value.place = alias;
                    let cleanup = f.cleanup(vec![
                        CleanupAction::EndLoan {
                            loan,
                            if_active: false,
                        },
                        drop(source.0),
                    ]);
                    let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator
                    else {
                        panic!()
                    };
                    *on_failure = CleanupId(cleanup);
                    f.body().blocks[1].terminator = ret(0, cleanup);
                }
                owner_signature(&mut f, PassingMode::Take);
                let rejected = if passing == PassingMode::Mut {
                    actual_once != expected_once
                } else {
                    actual_once && !expected_once
                };
                if rejected {
                    assert!(
                        f.kinds().contains(&ViolationKind::TypeMismatch {
                            entity: "call argument"
                        }),
                        "{passing:?} {actual_once} {expected_once}: {:?}",
                        f.kinds()
                    );
                } else {
                    f.valid();
                }
            }
        }
    }
}

#[test]
fn closure_parameter_brands_use_the_installed_capture_identity() {
    let mut f = Fixture::unit();
    let owner = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Parameter);
    let other = f.slot(TypeKind::Primitive(Primitive::U32), Storage::Parameter);
    f.body().parameters = vec![LocalId(owner), LocalId(other)];
    let origin = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
        fields: vec![].into(),
    };
    f.body().places[owner as usize].binding = Some(origin.clone());
    let mut other_origin = origin.clone();
    other_origin.root =
        OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(1)));
    f.body().places[other as usize].binding = Some(other_origin);
    let scalar = f.body().places[owner as usize].ty;
    let mut branded = ty(TypeKind::Primitive(Primitive::U32));
    branded.brand.alternatives.push(origin.clone());
    let branded = f.definition.types.push(branded);
    let mut child = empty_body(&mut f);
    child.parameter_scope = scoped(NameSite::Expression(hirsel_syntax::ExprId(7)));
    child.locals[0].ty = branded;
    child.locals[0].storage = Storage::Parameter;
    child.places[0].ty = branded;
    child.parameters = vec![LocalId(0)];
    child.result = branded;
    child.blocks[0] = block(
        vec![],
        Terminator::Return {
            value: copy(0),
            cleanup: CleanupId(0),
        },
    );
    child.rvalues.clear();
    let capture = add_capture(&mut child, scalar);
    child.places[capture as usize].binding = Some(origin);
    f.definition.bodies.push(child);
    let SignatureKind::Function(mut fun) =
        function_signature("callback", vec![], TypeKind::Tuple(vec![].into())).kind
    else {
        unreachable!()
    };
    fun.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Borrow,
        ty: branded,
    }]
    .into();
    fun.result = branded;
    let closure = f.slot(TypeKind::Function(fun), Storage::Temporary);
    let old = f.body().places[closure as usize].ty;
    let mut scoped = interner().get(old).unwrap().clone();
    scoped.modes.relation = Relation::environment();
    let scoped = f.definition.types.push(scoped);
    f.body().places[closure as usize].ty = scoped;
    f.body().locals[closure as usize].ty = scoped;
    f.assign(
        closure,
        Rvalue::Closure {
            context: hirsel_types::CaptureContext::OwningScoped,
            body: BodyId(1),
            captures: vec![copy(owner)],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(closure)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Borrow);
    f.valid();
    let Rvalue::Closure { captures, .. } = f.body().rvalues.last_mut().unwrap() else {
        unreachable!()
    };
    captures[0] = copy(other);
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidOrigin(PlaceId(other))),
        "a same-typed capture cannot claim another owner's identity"
    );
}

/// `fn main(take s: String) -> (unique String,)` building its tuple from `s`,
/// which is owned but not unique (MODE-2).
fn aliased_unique_part() -> Fixture {
    let mut f = Fixture::unit();
    let s = param(&mut f, TypeKind::Primitive(Primitive::String));
    let mut part = ty(TypeKind::Primitive(Primitive::String));
    part.modes.uniqueness = Uniqueness::Unique;
    let part = f.definition.types.push(part);
    let tuple = f.slot(TypeKind::Tuple(vec![part].into()), Storage::Temporary);
    let failure = f.cleanup(vec![drop(s)]);
    f.assign(
        tuple,
        Rvalue::Construct {
            constructor: Constructor::Tuple,
            fields: vec![ConstructField {
                field: 0,
                value: take(s),
            }],
            destination: Destination::Inferred,
        },
        failure,
    );
    f.body().result = f.body().places[tuple as usize].ty;
    f.body().blocks[0].terminator = ret(tuple, 0);
    owner_signature(&mut f, PassingMode::Take);
    f
}

/// This IR cannot be written in source: it claims one dictionary's brand while
/// constructing from another dictionary of the same Order[K] type.
#[test]
fn sorted_map_constructor_replays_dictionary_identity() {
    let mut f = Fixture::unit();
    let mut order = capability_signature("Order");
    order.definition = DefId::intern(&DefPath::core("Order"));
    let SignatureKind::Type(declared) = &mut order.kind else {
        unreachable!()
    };
    declared.generics = vec![GenericParameter::Type {
        name: "K".into(),
        bounds: vec![].into(),
    }];
    declared.capability = None;
    declared.authority = AuthoritySummary::inert();
    declared.bounds = vec![Bound::Copyable];
    f.signatures.insert(order.definition.clone(), order);
    let scalar = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::U64)));
    let order_kind = TypeKind::Named {
        definition: DefId::intern(&DefPath::core("Order")),
        arguments: vec![GenericArgument::Type(scalar)].into(),
    };
    let by = param(&mut f, order_kind.clone());
    let other = param(&mut f, order_kind);
    let scalar_by = param(&mut f, TypeKind::Primitive(Primitive::U64));
    // Order's real callable payload may allocate during cleanup; the witness
    // borrows that contract rather than manufacturing NoallocDrop evidence.
    for place in [by, other] {
        revise_comparison_place(&mut f, place, |row| {
            row.drop_allocation = DropAllocation::MayAllocate
        });
    }
    let map = f.slot(
        TypeKind::Builtin {
            kind: BuiltinType::SortedMap,
            arguments: vec![GenericArgument::Type(scalar), GenericArgument::Type(scalar)].into(),
        },
        Storage::Local,
    );
    let map_ty = f.body().places[map as usize].ty;
    let mut row = interner().get(map_ty).unwrap().clone();
    row.brand.alternatives = vec![Origin {
        root: OriginRoot::Parameter(ParameterRef::body(f.body().parameter_scope, ParamId(0))),
        fields: Default::default(),
    }]
    .into();
    let row = f.definition.types.push(row);
    f.body().locals[map as usize].ty = row;
    f.body().places[map as usize].ty = row;
    f.assign(
        map,
        Rvalue::Construct {
            constructor: Constructor::SortedMap {
                by: operand(by, UseKind::Borrow),
            },
            fields: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(map)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    owner_signature(&mut f, PassingMode::Borrow);
    f.valid();
    let accepted = f.definition.clone();
    let Rvalue::Construct {
        constructor: Constructor::SortedMap { by },
        ..
    } = f.body().rvalues.last_mut().unwrap()
    else {
        unreachable!()
    };
    *by = operand(other, UseKind::Borrow);
    f.broken(vec![ViolationKind::InvalidConstructor]);
    f.definition = accepted.clone();
    let Rvalue::Construct {
        constructor: Constructor::SortedMap { by },
        ..
    } = f.body().rvalues.last_mut().unwrap()
    else {
        unreachable!()
    };
    by.usage = UseKind::Copy;
    assert!(f.kinds().contains(&ViolationKind::InvalidConstructor));
    f.definition = accepted.clone();
    let Rvalue::Construct { fields, .. } = f.body().rvalues.last_mut().unwrap() else {
        unreachable!()
    };
    fields.push(ConstructField {
        field: 0,
        value: copy(0),
    });
    assert!(f.kinds().contains(&ViolationKind::InvalidConstructor));
    f.definition = accepted;
    let scope = f.body().parameter_scope;
    revise_comparison_place(&mut f, map, |row| {
        row.brand.alternatives[0].root =
            OriginRoot::Parameter(ParameterRef::body(scope, ParamId(2)))
    });
    let Rvalue::Construct {
        constructor: Constructor::SortedMap { by },
        ..
    } = f.body().rvalues.last_mut().unwrap()
    else {
        unreachable!()
    };
    *by = operand(scalar_by, UseKind::Borrow);
    assert!(f.kinds().contains(&ViolationKind::TypeMismatch {
        entity: "constructor field"
    }));
}

#[path = "table_controls.rs"]
mod table_controls;
