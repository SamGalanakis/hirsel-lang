//! Fixed cover controls start from accepted transfers of two live real loans.
use crate::support::*;
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::{ViolationKind, validate};

fn contract(cover: Option<Vec<PlaceId>>) -> Fixture {
    let mut f = Fixture::unit();
    let a = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    let b = f.slot(TypeKind::Primitive(Primitive::String), Storage::Parameter);
    let c = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let rhs = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    let old = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.body().parameters = vec![LocalId(a), LocalId(b)];
    for _ in 0..2 {
        f.body().scopes.push(hirsel_tir::Scope {
            parent: Some(ScopeId(0)),
            span: Default::default(),
        });
    }
    f.body().locals[c as usize].scope = ScopeId(1);
    f.body().locals[rhs as usize].scope = ScopeId(1);
    f.body().locals[old as usize].scope = ScopeId(2);
    owner_signature(&mut f, hirsel_syntax::PassingMode::Borrow);
    for (loan, root) in [(0, a), (1, b)] {
        f.emit(InstructionKind::BeginLoan {
            id: LoanId(loan),
            place: PlaceId(root),
            mutable: false,
            if_inactive: false,
        });
    }
    let a = f.through_loan(a, 0);
    let b = f.through_loan(b, 1);
    let initializer = RvalueId(f.body().rvalues.len() as u32);
    f.assign(c, Rvalue::Use(operand(a, UseKind::Borrow)), 0);
    f.body().borrow_cursors.push(BorrowCursor {
        carrier: LocalId(c),
        initializer,
        cover,
    });
    f.assign(rhs, Rvalue::Use(operand(b, UseKind::Borrow)), 0);
    f.assign(old, Rvalue::Use(take(c)), 0);
    f.assign(c, Rvalue::Use(take(rhs)), 0);
    f.emit(InstructionKind::EndScope(ScopeId(2)));
    f.emit(InstructionKind::EndScope(ScopeId(1)));
    for loan in [0, 1] {
        f.emit(InstructionKind::EndLoan {
            loan: LoanId(loan),
            if_active: false,
        });
    }
    f
}
fn rejects_at(f: &Fixture, instruction: u32, expected: ViolationKind) {
    let verdict = validate(
        &tabled(&f.definition),
        interner(),
        &f.signatures,
        &mut vec![],
    );
    assert!(
        verdict
            .violations
            .iter()
            .any(|v| v.block == Some(BlockId(0))
                && v.instruction == Some(instruction)
                && v.kind == expected),
        "{verdict:?}"
    );
}
#[test]
fn borrow_cursor_8_declared_two_roots_accepts_real_transfer() {
    contract(Some(vec![PlaceId(1), PlaceId(2)])).valid();
}
#[test]
fn borrow_cursor_8_absent_receipt_preserves_existing_transfer() {
    let mut f = contract(None);
    f.body().borrow_cursors.clear();
    f.valid();
}
#[test]
fn borrow_cursor_8_inferred_cover_cannot_grow_at_rebind() {
    rejects_at(&contract(None), 6, ViolationKind::InvalidOrigin(PlaceId(3)));
}
#[test]
fn borrow_cursor_8_forged_sibling_cover_does_not_grant_initial_borrow() {
    rejects_at(
        &contract(Some(vec![PlaceId(2)])),
        3,
        ViolationKind::InvalidOrigin(PlaceId(3)),
    );
}
#[test]
fn borrow_cursor_8_stale_claim_cannot_hide_inserted_storage_loan() {
    let mut f = contract(Some(vec![PlaceId(1), PlaceId(2)]));
    f.body().blocks[0].instructions.insert(
        4,
        inst(
            InstructionKind::BeginLoan {
                id: LoanId(2),
                place: PlaceId(3),
                mutable: false,
                if_inactive: false,
            },
            0,
        ),
    );
    // The new edge is a real borrow of the carrier storage, not its referent.
    rejects_at(
        &f,
        6,
        ViolationKind::LoanConflict {
            loan: LoanId(2),
            place: PlaceId(3),
        },
    );
}
#[test]
fn borrow_cursor_8_expired_incoming_real_loan_is_rejected() {
    let mut f = contract(Some(vec![PlaceId(1), PlaceId(2)]));
    let end = f.body().blocks[0].instructions.pop().unwrap();
    f.body().blocks[0].instructions.insert(4, end);
    rejects_at(
        &f,
        5,
        ViolationKind::ExpiredLoan {
            loan: LoanId(1),
            place: PlaceId(7),
        },
    );
}

#[test]
fn borrow_cursor_8_owned_refill_is_independently_rejected() {
    let mut f = contract(Some(vec![PlaceId(1), PlaceId(2)]));
    let rv = match f.body().blocks[0].instructions[6].kind {
        InstructionKind::Assign { value, .. } => value,
        _ => unreachable!(),
    };
    let Rvalue::Use(mut operand) = f.body().rvalues[rv.0 as usize].clone() else {
        unreachable!()
    };
    operand.usage = UseKind::Copy;
    f.body().rvalues[rv.0 as usize] = Rvalue::Use(operand);
    rejects_at(&f, 6, ViolationKind::InvalidOrigin(PlaceId(3)));
}
#[test]
fn borrow_cursor_8_relocation_requires_temporary_source_storage() {
    let mut f = contract(Some(vec![PlaceId(1), PlaceId(2)]));
    f.body().locals[3].storage = Storage::Local;
    rejects_at(&f, 5, ViolationKind::BorrowedMove(PlaceId(3)));
}
#[test]
fn borrow_cursor_8_wrong_scope_ends_the_new_carrier() {
    let mut f = contract(Some(vec![PlaceId(1), PlaceId(2)]));
    f.body().blocks[0].instructions[7].kind = InstructionKind::EndScope(ScopeId(1));
    let value = RvalueId(f.body().rvalues.len() as u32);
    f.body()
        .rvalues
        .push(Rvalue::Use(operand(3, UseKind::Borrow)));
    f.body().blocks[0].instructions.insert(
        8,
        inst(
            InstructionKind::Assign {
                destination: PlaceId(4),
                value,
            },
            0,
        ),
    );
    rejects_at(&f, 8, ViolationKind::ReadBeforeInit(PlaceId(3)));
}
#[test]
fn borrow_cursor_8_verdict_and_reads_survive_session_salts() {
    let run = || {
        let mut out = String::new();
        for cover in [
            None,
            Some(vec![PlaceId(1)]),
            Some(vec![PlaceId(1), PlaceId(2)]),
            Some(vec![PlaceId(2)]),
        ] {
            let f = contract(cover);
            let mut reads = vec![];
            let verdict = validate(
                &tabled(&f.definition),
                interner(),
                &f.signatures,
                &mut reads,
            );
            out += &format!("{verdict:?}; reads={reads:?}\n");
        }
        out
    };
    let Some(runs) = hirsel_intern::under_salts(
        "borrow_cursor_contracts_8::borrow_cursor_8_verdict_and_reads_survive_session_salts",
        &[1, 2, 0x5eed],
        run,
    ) else {
        return;
    };
    let ordinary = run();
    assert_eq!(runs.len(), 3);
    assert!(
        runs.iter().all(|run| run == &ordinary),
        "{runs:#?}\n{ordinary}"
    );
}
