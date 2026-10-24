//! Phase-one encoding probes: administrative relocation never owns a view.
use crate::support::*;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::{ViolationKind, validate};

fn transport() -> (Fixture, u32, u32, u32) {
    let mut f = Fixture::string();
    let cursor = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let rhs = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let retired = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let saved = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    for _ in 0..3 {
        f.body().scopes.push(hirsel_tir::Scope {
            parent: Some(ScopeId(0)),
            span: Default::default(),
        });
    }
    for p in [cursor, saved] {
        f.body().locals[p as usize].scope = ScopeId(1);
    }
    f.body().locals[rhs as usize].scope = ScopeId(2);
    f.body().locals[retired as usize].scope = ScopeId(3);
    f.emit(InstructionKind::BeginLoan {
        id: LoanId(0),
        place: PlaceId(1),
        mutable: false,
        if_inactive: false,
    });
    let source = f.through_loan(1, 0);
    f.assign(cursor, Rvalue::Use(operand(source, UseKind::Borrow)), 0);
    f.assign(saved, Rvalue::Use(operand(cursor, UseKind::Borrow)), 0);
    f.assign(rhs, Rvalue::Use(operand(cursor, UseKind::Borrow)), 0);
    f.assign(retired, Rvalue::Use(take(cursor)), 0);
    f.assign(cursor, Rvalue::Use(take(rhs)), 0);
    f.emit(InstructionKind::EndScope(ScopeId(3)));
    // The saved snapshot and newly installed cursor still retain the real loan.
    f.assign(rhs, Rvalue::Use(operand(saved, UseKind::Borrow)), 0);
    f.emit(InstructionKind::EndScope(ScopeId(2)));
    f.emit(InstructionKind::EndScope(ScopeId(1)));
    f.emit(InstructionKind::EndLoan {
        loan: LoanId(0),
        if_active: false,
    });
    (f, cursor, saved, retired)
}

#[test]
fn borrow_cursor_8_existing_transport_preserves_snapshot() {
    let (f, _, _, _) = transport();
    f.valid();
}

#[test]
fn borrow_cursor_8_existing_transport_joins_a_backedge() {
    let (mut f, _, _, _) = transport();
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    let root = f.body().places[condition as usize].root;
    f.body().parameters.push(root);
    owner_signature(&mut f, hirsel_syntax::PassingMode::Borrow);
    let exit = f.body().blocks[0].terminator.clone();
    let mut instructions = std::mem::take(&mut f.body().blocks[0].instructions);
    let cleanup = instructions.split_off(9);
    let advance = instructions.split_off(5);
    f.body().blocks[0] = block(instructions, Terminator::Goto(BlockId(1)));
    f.body().blocks.push(block(
        vec![],
        Terminator::Branch {
            condition: copy(condition),
            yes: BlockId(2),
            no: BlockId(3),
        },
    ));
    f.body()
        .blocks
        .push(block(advance, Terminator::Goto(BlockId(1))));
    f.body().blocks.push(block(cleanup, exit));
    f.valid();
}

#[test]
fn borrow_cursor_8_real_loan_expiry_is_not_carrier_retirement() {
    let (mut f, _, saved, _) = transport();
    let end = f.body().blocks[0].instructions.pop().unwrap();
    f.body().blocks[0].instructions.insert(8, end);
    let mut reads = vec![];
    let verdict = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut reads,
    );
    assert!(
        verdict.violations.iter().any(|v| v.kind
            == ViolationKind::ExpiredLoan {
                loan: LoanId(0),
                place: PlaceId(saved)
            }),
        "{verdict:?}"
    );
}

#[test]
fn borrow_cursor_8_borrowed_retired_view_cannot_be_dropped() {
    let (mut f, _, _, retired) = transport();
    f.body().blocks[0].instructions[8].kind = InstructionKind::Drop {
        place: PlaceId(retired),
        if_live: false,
    };
    assert!(
        f.kinds()
            .contains(&ViolationKind::BorrowedMove(PlaceId(retired)))
    );
}

fn receipt() -> (Fixture, u32) {
    let (mut f, cursor, _, _) = transport();
    let mut named = f.body().places[1].clone();
    named.binding = Some(Origin {
        root: OriginRoot::Local(lexical(), hirsel_types::LocalId(1000)),
        fields: vec![].into(),
    });
    let name = PlaceId(f.body().places.len() as u32);
    f.body().places.push(named);
    f.body().blocks[0].instructions.insert(
        2,
        inst(
            InstructionKind::BindOrigin {
                place: name,
                source: PlaceId(1),
                kind: OriginBindingKind::Fresh,
            },
            0,
        ),
    );
    f.body().borrow_cursors.push(BorrowCursor {
        carrier: LocalId(cursor),
        initializer: RvalueId(2),
        cover: None,
    });
    (f, cursor)
}

#[test]
fn borrow_cursor_8_receipt_rederives_borrowed_declaration() {
    let (f, _) = receipt();
    f.valid();
}

#[test]
fn borrow_cursor_8_receipt_rejects_wrong_initializer() {
    let (mut f, _) = receipt();
    f.body().borrow_cursors[0].initializer = RvalueId(1);
    assert!(f.kinds().iter().any(|kind| matches!(
        kind,
        ViolationKind::WrongKind {
            entity: "borrow cursor declaration",
            ..
        }
    )));
}

#[test]
fn borrow_cursor_8_receipt_rejects_owning_carrier() {
    let (mut f, _) = receipt();
    f.body().borrow_cursors[0].carrier = LocalId(1);
    f.body().borrow_cursors[0].initializer = RvalueId(1);
    assert!(
        f.kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::InvalidUse(PlaceId(1))))
    );
}

#[test]
fn borrow_cursor_8_real_owner_drop_conflicts_with_saved_view() {
    let (mut f, _, _, _) = transport();
    f.body().blocks[0].instructions.insert(
        8,
        inst(
            InstructionKind::Drop {
                place: PlaceId(1),
                if_live: false,
            },
            0,
        ),
    );
    let verdict = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        verdict.violations.iter().any(|v| v.instruction == Some(8)
            && v.kind
                == ViolationKind::LoanOutlivesSource {
                    loan: LoanId(0),
                    place: PlaceId(1)
                }),
        "{verdict:?}"
    );
}
