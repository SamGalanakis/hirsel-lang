//! Independent LIR controls for projected joined read views.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Rc;

#[derive(Clone, Copy)]
enum Forge {
    None,
    Owned,
    Ended,
    Write,
    BadIncoming,
}

fn projected(case: Forge) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Borrow, l::PAIRBLK), (Passing::Borrow, l::PAIRBLK)],
        None,
    );
    let a = t.slot(l::PAIRBLK, SlotKind::Parameter(0), ROOT);
    let b = t.slot(l::PAIRBLK, SlotKind::Parameter(1), ROOT);
    let av = t.version_of(a);
    t.param(av);
    let bv = t.version_of(b);
    t.param(bv);
    let aa = t.at(ENTRY, a, &[], &[]);
    let ba = t.at(ENTRY, b, &[], &[]);
    let la = t.loan(aa, false);
    let lb = t.loan(ba, false);
    let p = t.borrowed(l::PAIRBLK, la);
    let join = t.builder.block(BlockKind::Body, &[p]);
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    t.begin(yes, la);
    let x = t.read(yes, aa, av, ReadMode::Borrow, Some(la));
    t.builder.jump(yes, join, &[x]);
    t.begin(no, lb);
    // One altered incoming annotation cannot borrow the other root.
    let y = t.read(
        no,
        ba,
        bv,
        ReadMode::Borrow,
        Some(if matches!(case, Forge::BadIncoming) {
            la
        } else {
            lb
        }),
    );
    t.builder.jump(no, join, &[y]);
    if matches!(case, Forge::Ended) {
        t.builder.push(
            join,
            Inst::EndLoan {
                loan: la,
                if_active: true,
            },
        );
    }
    let address_failure = t.builder.block(BlockKind::FailureCleanup, &[]);
    for loan in [la, lb] {
        t.builder.push(
            address_failure,
            Inst::EndLoan {
                loan,
                if_active: true,
            },
        );
    }
    t.builder.term(address_failure, Terminator::Propagate);
    let field = t.address(AddressBase::Value(p), &[ProjectionStep::Field(0)], l::BLK);
    let failure = t.failure(address_failure);
    t.builder.push(
        join,
        Inst::Address {
            address: field,
            failure,
        },
    );
    let selected = t.read(join, field, p, ReadMode::Borrow, Some(la));
    if matches!(case, Forge::Owned) {
        t.builder.unit.values[selected.index()].holding = Holding::Owned;
    }
    if matches!(case, Forge::Write) {
        let version = t.plain(l::PAIRBLK);
        t.builder.push(
            join,
            Inst::Store {
                address: field,
                value: selected,
                version,
            },
        );
    }
    let after = t.builder.block(BlockKind::Body, &[]);
    let fail = t.builder.block(BlockKind::FailureCleanup, &[]);
    for block in [after, fail] {
        for loan in [la, lb] {
            t.builder.push(
                block,
                Inst::EndLoan {
                    loan,
                    if_active: true,
                },
            );
        }
    }
    t.builder.term(fail, Terminator::Propagate);
    t.call(
        join,
        &[(Passing::Borrow, l::BLK)],
        None,
        &[ArgumentValue::Value(selected)],
        (after, &[]),
        fail,
    );
    t.builder.ret(after, None);
    t.check()
}

#[test]
fn borrow_cursor_8_projected_phi_keeps_all_covering_loans() {
    let accepted = projected(Forge::None);
    assert!(accepted.is_empty(), "{accepted:?}");
}

#[test]
fn borrow_cursor_8_projected_view_cannot_claim_ownership() {
    assert!(
        projected(Forge::Owned)
            .iter()
            .any(|k| matches!(k, K::Holding(_)))
    );
}

#[test]
fn borrow_cursor_8_projected_view_expires_with_real_loan() {
    assert!(
        projected(Forge::Ended)
            .iter()
            .any(|k| matches!(k, K::StaleView(_) | K::OutsideLoan(_)))
    );
}

#[test]
fn borrow_cursor_8_readonly_backing_cannot_be_written() {
    assert!(
        projected(Forge::Write)
            .iter()
            .any(|k| matches!(k, K::WriteThroughValue(_)))
    );
}

#[test]
fn borrow_cursor_8_each_incoming_edge_needs_coverage() {
    assert!(
        projected(Forge::BadIncoming)
            .iter()
            .any(|k| matches!(k, K::LoanCoverage(_)))
    );
}

fn value_copy(layout: hirsel_lir::layout_types::LayoutId, expired: bool) -> Vec<K> {
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[(Passing::Borrow, layout)], Some(layout));
    let slot = t.slot(layout, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let at = t.at(ENTRY, slot, &[], &[]);
    let loan = t.loan(at, false);
    t.begin(ENTRY, loan);
    let source = t.read(ENTRY, at, version, ReadMode::Borrow, Some(loan));
    if expired {
        t.end(ENTRY, loan);
    }
    let result = t.owned(layout);
    t.builder.push(
        ENTRY,
        Inst::CopyValue {
            result,
            value: source,
        },
    );
    if !expired {
        t.end(ENTRY, loan);
    }
    t.builder.ret(ENTRY, Some(result));
    t.check()
}
#[test]
fn borrow_cursor_8_explicit_value_copy_obtains_ownership() {
    let v = value_copy(l::BLK, false);
    assert!(v.is_empty(), "{v:?}");
}
#[test]
fn borrow_cursor_8_value_copy_requires_copyable_recipe() {
    let violations = value_copy(l::ERASED_ONCE, false);
    assert!(
        violations
            .iter()
            .any(|k| matches!(k, K::Type("SSA block copy"))),
        "{violations:?}"
    );
}
#[test]
fn borrow_cursor_8_value_copy_rejects_resource() {
    let violations = value_copy(l::RESBLK, false);
    assert!(
        violations
            .iter()
            .any(|k| matches!(k, K::Type("SSA block copy"))),
        "{violations:?}"
    );
}
#[test]
fn borrow_cursor_8_value_copy_rejects_expired_borrow() {
    assert!(
        value_copy(l::BLK, true)
            .iter()
            .any(|k| matches!(k, K::OutsideLoan(_)))
    );
}
