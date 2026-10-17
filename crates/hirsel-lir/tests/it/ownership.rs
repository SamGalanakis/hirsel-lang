//! Holder credits, initialization, loans, views, anchors and cleanup order:
//! one hand-built mutation per obligation, each paired with its valid twin.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::{Rc, Ready, Reuse, Value};
use support::*;

const TAKE_BLK: (Passing, LayoutId) = (Passing::Take, l::BLK);
const BORROW_BLK: (Passing, LayoutId) = (Passing::Borrow, l::BLK);

#[test]
fn a_failed_statement_cleanup_starts_a_separate_frame_cleanup() {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK, TAKE_BLK], None);
    let a = t.owned(l::BLK);
    t.param(a);
    let b = t.owned(l::BLK);
    t.param(b);
    let older = t.slot(l::BLK, SlotKind::Temporary, ScopeId(0));
    let retained = t.slot(l::BLK, SlotKind::Temporary, ScopeId(0));
    t.builder.push(ENTRY, Inst::StorageLive { slot: older });
    t.builder.push(ENTRY, Inst::StorageLive { slot: retained });
    let first = t.at(ENTRY, older, &[], &[Undo::Release(a), Undo::Release(b)]);
    t.store(ENTRY, first, a, older);
    let second = t.at(ENTRY, retained, &[], &[Undo::Drop(older), Undo::Release(b)]);
    let version = t.store(ENTRY, second, b, retained);
    let frame = t.cleanup(&[Undo::Drop(retained)]);
    let statement = t.builder.block(BlockKind::Cleanup, &[]);
    let failure = t.failure(frame);
    let anchor = t.cleanup_anchor();
    t.builder.push(
        statement,
        Inst::Drop {
            address: first,
            if_live: false,
            anchor,
            failure,
        },
    );
    let continuation = t.builder.block(BlockKind::Body, &[]);
    let loan = t.loan(second, false);
    t.begin(continuation, loan);
    t.read(continuation, second, version, ReadMode::Borrow, Some(loan));
    t.end(continuation, loan);
    t.run(continuation, &[Undo::Drop(retained)], frame);
    t.builder.ret(continuation, None);
    t.builder.jump(statement, continuation, &[]);
    t.builder.jump(ENTRY, statement, &[]);
    let violations = t.check();
    assert!(violations.is_empty(), "{violations:?}");
    // The same two releases in one activation still require descending order.
    assert!(
        cleanup_order(
            &[None],
            (SlotKind::Temporary, 0),
            (SlotKind::Temporary, 0),
            true,
        )
        .contains(&K::CleanupOrder(SlotId(1)))
    );
}

/// Two owned block parameters; one moves into a call and the other must be
/// released. Releasing the moved one instead is the wrong same-typed holder.
fn wrong_holder(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK, TAKE_BLK], None);
    let a = t.owned(l::BLK);
    t.param(a);
    let b = t.owned(l::BLK);
    t.param(b);
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.releasing(&[b]);
    t.call(
        ENTRY,
        &[TAKE_BLK],
        None,
        &[ArgumentValue::Value(a)],
        (n, &[]),
        fail,
    );
    t.release(n, if mutant { a } else { b }, &[]);
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn releasing_the_wrong_same_typed_holder_is_rejected() {
    pair(wrong_holder, |k| *k == K::NoCredit(ValueId(0)));
    assert!(wrong_holder(true).contains(&K::Leak(ValueId(1))));
}

/// A credit moved on one arm must be released on the other.
fn one_arm(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    let join = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    let fail = t.builder.propagate();
    t.call(
        yes,
        &[TAKE_BLK],
        None,
        &[ArgumentValue::Value(x)],
        (join, &[]),
        fail,
    );
    if !mutant {
        t.release(no, x, &[]);
    }
    t.builder.jump(no, join, &[]);
    t.builder.ret(join, None);
    t.check()
}

#[test]
fn a_move_on_only_one_arm_is_rejected_at_the_join() {
    pair(
        one_arm,
        |k| matches!(k, K::JoinCredits { value, .. } if *value == ValueId(0)),
    );
}

/// An owned join parameter receives a fresh block on one arm and a borrowed
/// parameter on the other; the borrow needs a materialized credit.
fn borrow_join(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[BORROW_BLK], None);
    let x = t.plain(l::BLK);
    t.param(x);
    let p = t.owned(l::BLK);
    let join = t.builder.block(BlockKind::Body, &[p]);
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    let y = t.new_block(yes, &[]);
    t.builder.jump(yes, join, &[y]);
    if !mutant {
        t.builder.push(no, Inst::Dup { value: x });
    }
    t.builder.jump(no, join, &[x]);
    t.release(join, p, &[]);
    t.builder.ret(join, None);
    t.check()
}

#[test]
fn an_ownership_borrow_join_needs_a_materialized_credit() {
    pair(borrow_join, |k| *k == K::BorrowIntoOwned(ValueId(0)));
}

/// An owned value passed into a join parameter that only borrows.
fn owned_into_plain_join(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[BORROW_BLK], None);
    let x = t.plain(l::BLK);
    t.param(x);
    let p = t.plain(l::BLK);
    let join = t.builder.block(BlockKind::Body, &[p]);
    let y = t.new_block(ENTRY, &[]);
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.releasing(&[y]);
    t.call(
        ENTRY,
        &[BORROW_BLK],
        None,
        &[ArgumentValue::Value(y)],
        (n, &[]),
        fail,
    );
    if mutant {
        t.builder.jump(n, join, &[y]);
        t.builder.ret(join, None);
    } else {
        t.release(n, y, &[]);
        t.builder.jump(n, join, &[x]);
        t.builder.ret(join, None);
    }
    t.check()
}

#[test]
fn an_owned_credit_cannot_join_a_borrowing_parameter() {
    pair(owned_into_plain_join, |k| {
        matches!(k, K::OwnedIntoBorrowed(_))
    });
}

/// Value LIR: two fields of a taken record parameter move out in turn; taking
/// the first field twice is a move of a field already moved.
fn field_moved(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Take, l::PAIR)], None);
    let s = t.slot(l::PAIR, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(s);
    t.param(pv);
    let drop_s = [Undo::Drop(s)];
    let a0 = t.at(ENTRY, s, &[ProjectionStep::Field(0)], &drop_s);
    let t0 = t.read(ENTRY, a0, pv, ReadMode::Take, None);
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.cleanup(&drop_s);
    t.call(
        ENTRY,
        &[TAKE_BLK],
        None,
        &[ArgumentValue::Value(t0)],
        (n, &[]),
        fail,
    );
    let second = if mutant {
        a0
    } else {
        t.at(n, s, &[ProjectionStep::Field(1)], &drop_s)
    };
    let t1 = t.read(n, second, pv, ReadMode::Take, None);
    let exit = t.cleanup_exit(&drop_s, None);
    let fail = t.cleanup(&drop_s);
    t.call(
        n,
        &[TAKE_BLK],
        None,
        &[ArgumentValue::Value(t1)],
        (exit, &[]),
        fail,
    );
    t.check()
}

#[test]
fn taking_a_field_already_moved_is_rejected() {
    pair(field_moved, |k| *k == K::MovedPart(SlotId(0)));
}

/// A `mut` parameter holding a counted block of two block fields. A view of
/// field 0 is read through a borrowed read of the parameter; then field 1 is
/// replaced, which may copy-on-write the parent block. The view must have
/// been materialized (`Dup`) before the write.
fn stale_after_cow(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, l::PAIRBLK)], None);
    let p = t.slot(l::PAIRBLK, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(p);
    t.param(pv);
    let ap = t.at(ENTRY, p, &[], &[]);
    let whole = t.loan(ap, false);
    t.begin(ENTRY, whole);
    let r = t.read(ENTRY, ap, pv, ReadMode::Borrow, Some(whole));
    let af = t.through(ENTRY, r, &[ProjectionStep::Field(0)], &[Undo::End(whole)]);
    let part = t.loan(af, false);
    t.begin(ENTRY, part);
    let w = t.read(ENTRY, af, r, ReadMode::Borrow, Some(part));
    let mut held = Vec::new();
    if !mutant {
        t.builder.push(ENTRY, Inst::Dup { value: w });
        held.push(Undo::Release(w));
    }
    held.extend([Undo::End(part), Undo::End(whole)]);
    replace_sibling(&mut t, p, &held, w);
    t.check()
}

/// Replace field 1 of slot `p` with a fresh block, then borrow `view` in a
/// call and clean up `held` (ending loans and releasing the view's credit).
fn replace_sibling<S: hirsel_lir::stage::Stage>(
    t: &mut Lir<S>,
    p: SlotId,
    held: &[Undo],
    view: ValueId,
) {
    let y = t.new_block(ENTRY, held);
    let mut with_y = vec![Undo::Release(y)];
    with_y.extend_from_slice(held);
    let ab = t.at(ENTRY, p, &[ProjectionStep::Field(1)], &with_y);
    let d = t.slot(l::BLK, SlotKind::Displaced, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: d });
    let version = t.version_of(p);
    let displaced_version = t.version_of(d);
    let fail = t.cleanup(held);
    let failure = t.failure(fail);
    t.builder.push(
        ENTRY,
        Inst::Replace {
            address: ab,
            value: y,
            displaced: d,
            version,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    let mut steps = held.to_vec();
    steps.push(Undo::Drop(d));
    let exit = t.cleanup_exit(&steps, None);
    let fail = t.cleanup(&steps);
    t.call(
        ENTRY,
        &[BORROW_BLK],
        None,
        &[ArgumentValue::Value(view)],
        (exit, &[]),
        fail,
    );
}

#[test]
fn a_projection_is_stale_after_a_copy_on_write_of_its_parent() {
    pair(stale_after_cow, |k| matches!(k, K::StaleView(_)));
}

/// A view of field 0 read directly from a slot, then a write to its sibling
/// field 1. Through a counted parent block the write may detach the parent;
/// in an inline record it cannot (#77), so that twin needs no
/// materialization.
fn sibling_write(detaching: bool) -> Vec<K> {
    let layout = if detaching { l::PAIRBLK } else { l::PAIR };
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, layout)], None);
    let p = t.slot(layout, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(p);
    t.param(pv);
    let aw = t.at(ENTRY, p, &[ProjectionStep::Field(0)], &[]);
    let loan = t.loan(aw, false);
    t.begin(ENTRY, loan);
    let w = t.read(ENTRY, aw, pv, ReadMode::Borrow, Some(loan));
    replace_sibling(&mut t, p, &[Undo::End(loan)], w);
    t.check()
}

#[test]
fn a_field_view_is_stale_after_a_sibling_write_that_may_detach_its_parent() {
    pair(sibling_write, |k| matches!(k, K::StaleView(_)));
}

#[test]
fn a_materialized_field_view_survives_a_detaching_sibling_write() {
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, l::PAIRBLK)], None);
    let p = t.slot(l::PAIRBLK, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(p);
    t.param(pv);
    let aw = t.at(ENTRY, p, &[ProjectionStep::Field(0)], &[]);
    let loan = t.loan(aw, false);
    t.begin(ENTRY, loan);
    let w = t.read(ENTRY, aw, pv, ReadMode::Borrow, Some(loan));
    t.builder.push(ENTRY, Inst::Dup { value: w });
    replace_sibling(&mut t, p, &[Undo::Release(w), Undo::End(loan)], w);
    assert_eq!(t.check(), vec![]);
}

/// A value used only by failure cleanup stays live until the failing call.
fn failure_only_use(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    if mutant {
        t.release(ENTRY, x, &[]);
    }
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.releasing(&[x]);
    t.call(ENTRY, &[], None, &[], (n, &[]), fail);
    if !mutant {
        t.release(n, x, &[]);
    }
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn a_value_used_only_on_the_failure_path_must_still_be_held() {
    pair(failure_only_use, |k| *k == K::NoCredit(ValueId(0)));
}

/// The result is extracted before the exit cleanup drops a resource; if that
/// drop fails, the extracted result must be released before propagating.
fn discarded_return(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::RES), TAKE_BLK], Some(l::BLK));
    let s = t.slot(l::RES, SlotKind::Parameter(0), ROOT);
    let q = t.version_of(s);
    t.param(q);
    let v = t.owned(l::BLK);
    t.param(v);
    let exit = t.builder.block(BlockKind::Cleanup, &[]);
    t.builder.jump(ENTRY, exit, &[]);
    let failed = t.builder.block(BlockKind::FailureCleanup, &[]);
    if !mutant {
        t.release(failed, v, &[]);
    }
    t.builder.term(failed, Terminator::Propagate);
    let retry = t.cleanup(&[Undo::Drop(s), Undo::Release(v)]);
    let a = t.address(AddressBase::Slot(s), &[], l::RES);
    let failure = t.failure(retry);
    t.builder.push(
        exit,
        Inst::Address {
            address: a,
            failure,
        },
    );
    let anchor = t.cleanup_anchor();
    let failure = t.failure(failed);
    t.builder.push(
        exit,
        Inst::Drop {
            address: a,
            if_live: false,
            anchor,
            failure,
        },
    );
    t.builder.ret(exit, Some(v));
    t.check()
}

#[test]
fn a_return_value_is_released_when_the_exit_cleanup_fails() {
    pair(discarded_return, |k| *k == K::Leak(ValueId(1)));
}

/// A boxed resource is destroyed by its anchored semantic drop until Ready;
/// a release at its last use skips the anchor.
fn resource_release(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::RESBLK)], None);
    let s = t.slot(l::RESBLK, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(s);
    t.param(pv);
    if mutant {
        let a = t.at(ENTRY, s, &[], &[Undo::Drop(s)]);
        let v = t.read(ENTRY, a, pv, ReadMode::Take, None);
        t.release(ENTRY, v, &[]);
        t.builder.ret(ENTRY, None);
    } else {
        let exit = t.cleanup_exit(&[Undo::Drop(s)], None);
        t.builder.jump(ENTRY, exit, &[]);
    }
    t.check()
}

#[test]
fn a_resource_released_away_from_its_anchor_is_rejected() {
    pair(resource_release, |k| {
        matches!(k, K::ResourceOutsideAnchor(_))
    });
}

/// Every cleanup anchor the import carried must be discharged.
fn skipped_anchor(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::RESBLK)], None);
    let s = t.slot(l::RESBLK, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(s);
    t.param(pv);
    if mutant {
        // The anchor exists, its drop does not.
        t.cleanup_anchor();
        t.builder.ret(ENTRY, None);
    } else {
        let exit = t.cleanup_exit(&[Undo::Drop(s)], None);
        t.builder.jump(ENTRY, exit, &[]);
    }
    t.check()
}

#[test]
fn a_skipped_resource_anchor_is_rejected() {
    pair(skipped_anchor, |k| matches!(k, K::SkippedAnchor(_)));
}

/// Two locals stored from parameters and dropped by the exit cleanup. `scopes`
/// is the unit's scope arena as each scope's parent ordinal.
fn cleanup_order(
    scopes: &[Option<u32>],
    first: (SlotKind, u32),
    second: (SlotKind, u32),
    swap: bool,
) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK, TAKE_BLK], None);
    t.builder.unit.scopes = scopes
        .iter()
        .map(|p| ScopeInfo {
            parent: p.map(ScopeId),
        })
        .collect();
    let a = t.owned(l::BLK);
    t.param(a);
    let b = t.owned(l::BLK);
    t.param(b);
    let s0 = t.slot(l::BLK, first.0, ScopeId(first.1));
    let s1 = t.slot(l::BLK, second.0, ScopeId(second.1));
    t.builder.push(ENTRY, Inst::StorageLive { slot: s0 });
    t.builder.push(ENTRY, Inst::StorageLive { slot: s1 });
    let a0 = t.at(ENTRY, s0, &[], &[Undo::Release(a), Undo::Release(b)]);
    t.store(ENTRY, a0, a, s0);
    let a1 = t.at(ENTRY, s1, &[], &[Undo::Drop(s0), Undo::Release(b)]);
    t.store(ENTRY, a1, b, s1);
    // The valid order releases the second slot first.
    let steps = if swap {
        [Undo::Drop(s0), Undo::Drop(s1)]
    } else {
        [Undo::Drop(s1), Undo::Drop(s0)]
    };
    let exit = t.cleanup_exit(&steps, None);
    t.builder.jump(ENTRY, exit, &[]);
    t.check()
}

#[test]
fn cleanup_releases_inner_scopes_before_outer_ones() {
    pair(
        |swap| {
            cleanup_order(
                &[None, Some(0)],
                (SlotKind::Local, 0),
                (SlotKind::Local, 1),
                swap,
            )
        },
        |k| *k == K::CleanupOrder(SlotId(1)),
    );
}

/// Nesting comes from the scope tree, not from ordinals: here the inner scope
/// 1 has the smaller ordinal, as its parent 2 was recorded after it.
#[test]
fn cleanup_order_reads_nesting_from_the_scope_tree() {
    pair(
        |swap| {
            cleanup_order(
                &[None, Some(2), Some(0)],
                (SlotKind::Local, 2),
                (SlotKind::Local, 1),
                swap,
            )
        },
        |k| *k == K::CleanupOrder(SlotId(1)),
    );
}

/// One cleanup sequence never releases slots of two sibling scopes; the twin
/// nests scope 2 in scope 1.
#[test]
fn cleanup_never_releases_sibling_scopes_in_one_sequence() {
    pair(
        |siblings| {
            let parent_of_2 = if siblings { 0 } else { 1 };
            cleanup_order(
                &[None, Some(0), Some(parent_of_2)],
                (SlotKind::Local, 1),
                (SlotKind::Local, 2),
                false,
            )
        },
        |k| *k == K::CleanupOrder(SlotId(0)),
    );
}

#[test]
fn cleanup_releases_temporaries_before_names_of_one_scope() {
    pair(
        |swap| {
            cleanup_order(
                &[None],
                (SlotKind::Local, 0),
                (SlotKind::Temporary, 0),
                swap,
            )
        },
        |k| *k == K::CleanupOrder(SlotId(1)),
    );
    // Names of one scope release in reverse declaration order.
    pair(
        |swap| cleanup_order(&[None], (SlotKind::Local, 0), (SlotKind::Local, 0), swap),
        |k| *k == K::CleanupOrder(SlotId(1)),
    );
}

/// The scope parent relation is a forest.
#[test]
fn a_scope_cycle_is_rejected() {
    pair(
        |cycle| {
            let root = if cycle { Some(1) } else { None };
            cleanup_order(
                &[root, Some(0)],
                (SlotKind::Local, 0),
                (SlotKind::Local, 1),
                false,
            )
        },
        |k| matches!(k, K::ScopeCycle(_)),
    );
}

/// A view of a block's field under a loan; the block must not be released
/// before the loan ends.
fn holder_under_loan(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::PAIRBLK)], None);
    let u = t.owned(l::PAIRBLK);
    t.param(u);
    let au = t.through(ENTRY, u, &[ProjectionStep::Field(0)], &[Undo::Release(u)]);
    let loan = t.loan(au, false);
    t.begin(ENTRY, loan);
    let w = t.read(ENTRY, au, u, ReadMode::Borrow, Some(loan));
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.cleanup(&[Undo::End(loan), Undo::Release(u)]);
    t.call(
        ENTRY,
        &[BORROW_BLK],
        None,
        &[ArgumentValue::Value(w)],
        (n, &[]),
        fail,
    );
    if mutant {
        t.release_then(n, u, &[Undo::End(loan)]);
        t.end(n, loan);
    } else {
        t.end(n, loan);
        t.release(n, u, &[]);
    }
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn a_holder_is_not_released_before_its_loans_end() {
    pair(holder_under_loan, |k| *k == K::HolderUnderLoan(LoanId(0)));
}

/// A borrowed view is used only while its loan is active.
fn view_after_loan(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::PAIRBLK)], None);
    let u = t.owned(l::PAIRBLK);
    t.param(u);
    let au = t.through(ENTRY, u, &[ProjectionStep::Field(0)], &[Undo::Release(u)]);
    let loan = t.loan(au, false);
    t.begin(ENTRY, loan);
    let w = t.read(ENTRY, au, u, ReadMode::Borrow, Some(loan));
    if mutant {
        t.end(ENTRY, loan);
    }
    let n = t.builder.block(BlockKind::Body, &[]);
    let undo = if mutant {
        vec![Undo::Release(u)]
    } else {
        vec![Undo::End(loan), Undo::Release(u)]
    };
    let fail = t.cleanup(&undo);
    t.call(
        ENTRY,
        &[BORROW_BLK],
        None,
        &[ArgumentValue::Value(w)],
        (n, &[]),
        fail,
    );
    if !mutant {
        t.end(n, loan);
    }
    t.release(n, u, &[]);
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn a_borrowed_projection_is_valid_only_while_its_loan_is_active() {
    pair(view_after_loan, |k| matches!(k, K::OutsideLoan(_)));
}

/// A caller `mut` parameter must be whole again at every exit, failure
/// included: content moved out is put back before the call that may fail.
fn mut_exit(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, l::BLK)], None);
    let p = t.slot(l::BLK, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(p);
    t.param(pv);
    // The replacement is built before the move, so its failure leaves p whole.
    let y = (!mutant).then(|| t.new_block(ENTRY, &[]));
    let undo: Vec<_> = y.map(Undo::Release).into_iter().collect();
    let a = t.at(ENTRY, p, &[], &undo);
    let v = t.read(ENTRY, a, pv, ReadMode::Take, None);
    let n = t.builder.block(BlockKind::Body, &[]);
    if let Some(y) = y {
        t.store(ENTRY, a, y, p);
        let fail = t.builder.propagate();
        t.call(
            ENTRY,
            &[TAKE_BLK],
            None,
            &[ArgumentValue::Value(v)],
            (n, &[]),
            fail,
        );
    } else {
        let fail = t.builder.propagate();
        t.call(
            ENTRY,
            &[TAKE_BLK],
            None,
            &[ArgumentValue::Value(v)],
            (n, &[]),
            fail,
        );
    }
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn a_caller_mut_parameter_is_whole_at_every_exit() {
    pair(mut_exit, |k| *k == K::MutExit(SlotId(0)));
}

/// A String literal: a Plain static slice (LIR schema revision 2).
fn static_slice(t: &mut Lir<impl hirsel_lir::stage::Stage>, b: BlockId) -> ValueId {
    let ty = t.ty(ValueRepr::Aggregate, l::SLICE);
    t.builder.unit.tables.constants.push(ConstantData {
        ty,
        value: ConstantValue::String("hirsel".into()),
    });
    let constant = ConstantId(t.builder.unit.tables.constants.len() as u32 - 1);
    let result = t.value_as(ValueRepr::Aggregate, l::SLICE, Holding::Plain);
    t.builder.push(b, Inst::Const { result, constant });
    result
}

/// A literal stored into owned storage and returned from it. The mutant
/// stores a Plain slice that is not static (the literal passed through a
/// block parameter), which has no credit to give.
fn stored_literal(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[], Some(l::SLICE));
    let literal = static_slice(&mut t, ENTRY);
    let (b, value) = if mutant {
        let p = t.value_as(ValueRepr::Aggregate, l::SLICE, Holding::Plain);
        let next = t.builder.block(BlockKind::Body, &[p]);
        t.builder.jump(ENTRY, next, &[literal]);
        (next, p)
    } else {
        (ENTRY, literal)
    };
    let s = t.slot(l::SLICE, SlotKind::Local, ROOT);
    t.builder.push(b, Inst::StorageLive { slot: s });
    let a = t.at(b, s, &[], &[]);
    let version = t.store(b, a, value, s);
    let r = t.read(b, a, version, ReadMode::Take, None);
    t.builder.ret(b, Some(r));
    t.check()
}

#[test]
fn a_static_literal_consumes_without_a_credit_and_nothing_else_does() {
    pair(stored_literal, |k| matches!(k, K::NoCredit(_)));
}

/// A reuse token from a static literal: static storage is never a donor.
fn static_donor(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[(Passing::Take, l::BLK)], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let donor = if mutant {
        let ty = t.ty(ValueRepr::Block, l::BLK);
        t.builder.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Bytes(vec![1]),
        });
        let constant = ConstantId(t.builder.unit.tables.constants.len() as u32 - 1);
        let v = t.value_as(ValueRepr::Block, l::BLK, Holding::Plain);
        t.builder.push(
            ENTRY,
            Inst::Const {
                result: v,
                constant,
            },
        );
        t.release(ENTRY, x, &[]);
        v
    } else {
        x
    };
    let token = t.owned(l::HDR);
    let anchor = t.cleanup_anchor();
    let target = t.cleanup(&[]);
    let failure = t.failure(target);
    t.builder.push(
        ENTRY,
        Inst::Reset {
            result: token,
            value: donor,
            layout: l::PAY,
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
    t.builder.push(ENTRY, Inst::Dispose { token });
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_static_is_never_a_reuse_donor() {
    pair(static_donor, |k| {
        matches!(k, K::NoCredit(_) | K::Holding(_))
    });
}

/// An in-place write through a static literal's payload.
fn write_through_static(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[], Some(l::SLICE));
    let literal = static_slice(&mut t, ENTRY);
    if mutant {
        // Write a fresh literal over the static's own storage.
        let other = static_slice(&mut t, ENTRY);
        let through = t.address(AddressBase::Value(literal), &[], l::SLICE);
        t.prepare(ENTRY, through, &[]);
        let v = t.value_as(ValueRepr::Aggregate, l::SLICE, Holding::Plain);
        t.builder.push(
            ENTRY,
            Inst::Store {
                address: through,
                value: other,
                version: v,
            },
        );
    }
    let s = t.slot(l::SLICE, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let version = t.store(ENTRY, a, literal, s);
    let r = t.read(ENTRY, a, version, ReadMode::Take, None);
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_static_is_never_written_in_place() {
    // A literal's storage is no block a value-based address may enter, and
    // a write through any value is rejected.
    pair(write_through_static, |k| {
        matches!(k, K::WriteThroughValue(_) | K::AddressType(_))
    });
}

/// How `finalized_shell` breaks the explicit finalization of a resource.
#[derive(Clone, Copy, PartialEq)]
enum Shell {
    Valid,
    /// A second `FinalizeBegin`.
    BeginTwice,
    /// `FinalizeEnd` with no `FinalizeBegin`.
    EndOnly,
    /// The counted field taken before finalization.
    TakeLive,
    /// Storage ends while the resource is Finalizing.
    DeadWhileFinalizing,
    /// The counted field stays in the shell when its storage ends.
    KeepField,
}

/// A local counted-field resource finalized explicitly at Ready: begin,
/// end, take and release the counted field, end the storage. The shell is
/// closed by its storage ending, with no take of the whole and no `Forget`.
fn finalized_shell(shape: Shell) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let s = t.slot(l::RESCNT, SlotKind::Local, ROOT);
    // Prepared while the storage is dead, so a failure owns only `x`.
    let whole = t.address(AddressBase::Slot(s), &[], l::RESCNT);
    let field = t.address(AddressBase::Slot(s), &[ProjectionStep::Field(1)], l::BLK);
    t.prepare(ENTRY, whole, &[x]);
    t.prepare(ENTRY, field, &[x]);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let n = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RESCNT,
        &[(0, n), (1, x)],
        placement,
    );
    let r = t.owned(l::RESCNT);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct: site,
            failure,
        },
    );
    let version = t.store(ENTRY, whole, r, s);
    let anchor = t.cleanup_anchor();
    let begin = Inst::FinalizeBegin {
        address: whole,
        anchor,
    };
    let end = Inst::FinalizeEnd {
        address: whole,
        anchor,
    };
    let mut taken = None;
    let mut take = |t: &mut Lir<Ready>| {
        taken = Some(t.read(ENTRY, field, version, ReadMode::Take, None));
    };
    match shape {
        Shell::Valid | Shell::KeepField => {
            t.builder.push(ENTRY, begin);
            t.builder.push(ENTRY, end);
            if shape == Shell::Valid {
                take(&mut t);
            }
        }
        Shell::BeginTwice => {
            t.builder.push(ENTRY, begin);
            t.builder.push(ENTRY, begin);
            t.builder.push(ENTRY, end);
            take(&mut t);
        }
        Shell::EndOnly => {
            t.builder.push(ENTRY, end);
            take(&mut t);
        }
        Shell::TakeLive => {
            take(&mut t);
            t.builder.push(ENTRY, begin);
            t.builder.push(ENTRY, end);
        }
        Shell::DeadWhileFinalizing => t.builder.push(ENTRY, begin),
    }
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    if let Some(y) = taken {
        t.release(ENTRY, y, &[]);
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_resource_finalizes_once_in_order() {
    let state = |k: &K| matches!(k, K::FinalizeState(_));
    pair(
        |m| finalized_shell(if m { Shell::BeginTwice } else { Shell::Valid }),
        state,
    );
    pair(
        |m| finalized_shell(if m { Shell::EndOnly } else { Shell::Valid }),
        state,
    );
    pair(
        |m| finalized_shell(if m { Shell::TakeLive } else { Shell::Valid }),
        state,
    );
    pair(
        |m| {
            finalized_shell(if m {
                Shell::DeadWhileFinalizing
            } else {
                Shell::Valid
            })
        },
        state,
    );
}

#[test]
fn a_finalized_shell_ends_only_once_its_counted_fields_are_taken() {
    pair(
        |m| finalized_shell(if m { Shell::KeepField } else { Shell::Valid }),
        |k| matches!(k, K::LeakedStorage(_)),
    );
}

/// A local record of two counted fields taken out one by one; its storage
/// ends once both are gone, and leaks one still held.
fn emptied_record(keep_one: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[TAKE_BLK, TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let y = t.owned(l::BLK);
    t.param(y);
    let s = t.slot(l::PAIR, SlotKind::Local, ROOT);
    let whole = t.address(AddressBase::Slot(s), &[], l::PAIR);
    let fields: Vec<_> = (0..2)
        .map(|f| t.address(AddressBase::Slot(s), &[ProjectionStep::Field(f)], l::BLK))
        .collect();
    for &a in [whole].iter().chain(&fields) {
        t.prepare(ENTRY, a, &[x, y]);
    }
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::PAIR,
        &[(0, x), (1, y)],
        placement,
    );
    let r = t.owned(l::PAIR);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct: site,
            failure,
        },
    );
    let version = t.store(ENTRY, whole, r, s);
    let count = if keep_one { 1 } else { 2 };
    let taken: Vec<_> = fields
        .iter()
        .rev()
        .take(count)
        .map(|&a| t.read(ENTRY, a, version, ReadMode::Take, None))
        .collect();
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    for (i, &v) in taken.iter().enumerate() {
        t.release(ENTRY, v, &taken[i + 1..]);
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn storage_whose_owning_fields_were_all_taken_holds_nothing() {
    pair(emptied_record, |k| matches!(k, K::LeakedStorage(_)));
}

/// A local `Two(x, y)` whose fields are taken through `VariantField`
/// addresses (each preparation checks the tag), then its storage ends. With
/// both fields taken the slot holds only a tag and nothing owned; with one
/// left it leaks. Before C read the active variant from the taken paths,
/// the valid twin was rejected as `LeakedStorage` too.
fn emptied_variant(keep_one: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK, TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let y = t.owned(l::BLK);
    t.param(y);
    let s = t.slot(l::TWOBOX, SlotKind::Local, ROOT);
    let whole = t.at(ENTRY, s, &[], &[Undo::Release(x), Undo::Release(y)]);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let site = t.construct_site(
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        l::TWOBOX,
        &[(0, x), (1, y)],
        placement,
    );
    let r = t.owned(l::TWOBOX);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct: site,
            failure,
        },
    );
    let version = t.store(ENTRY, whole, r, s);
    let fields: Vec<_> = (0..2)
        .map(|f| {
            t.at(
                ENTRY,
                s,
                &[ProjectionStep::VariantField {
                    variant: 1,
                    field: f,
                }],
                &[Undo::Drop(s)],
            )
        })
        .collect();
    let count = if keep_one { 1 } else { 2 };
    let taken: Vec<_> = fields
        .iter()
        .rev()
        .take(count)
        .map(|&a| t.read(ENTRY, a, version, ReadMode::Take, None))
        .collect();
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    for (i, &v) in taken.iter().enumerate() {
        t.release(ENTRY, v, &taken[i + 1..]);
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_variant_whose_active_fields_were_all_taken_holds_nothing() {
    pair(emptied_variant, |k| matches!(k, K::LeakedStorage(_)));
}

/// One arm stores a record in a live slot and takes its owning fields out;
/// the other never stores. At the join the slot is initialized on one path
/// only, and on that path nothing owned is left, so it holds nothing. Moved
/// parts describe initialized paths only: before C joined them that way,
/// the uninitialized arm turned "moved" into "maybe moved" and the valid
/// twin was rejected as `LeakedStorage` too (the loop-head shape of
/// DROP-2-loop-break and of every partly taken loop variable).
fn emptied_on_one_arm(keep_one: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[TAKE_BLK, TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let y = t.owned(l::BLK);
    t.param(y);
    let s = t.slot(l::PAIR, SlotKind::Local, ROOT);
    let whole = t.address(AddressBase::Slot(s), &[], l::PAIR);
    let fields: Vec<_> = (0..2)
        .map(|f| t.address(AddressBase::Slot(s), &[ProjectionStep::Field(f)], l::BLK))
        .collect();
    for &a in [whole].iter().chain(&fields) {
        t.prepare(ENTRY, a, &[x, y]);
    }
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    let join = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::PAIR,
        &[(0, x), (1, y)],
        placement,
    );
    let r = t.owned(l::PAIR);
    let failure = t.fails();
    t.builder.push(
        yes,
        Inst::Construct {
            result: r,
            construct: site,
            failure,
        },
    );
    let version = t.store(yes, whole, r, s);
    let count = if keep_one { 1 } else { 2 };
    let taken: Vec<_> = fields
        .iter()
        .rev()
        .take(count)
        .map(|&a| t.read(yes, a, version, ReadMode::Take, None))
        .collect();
    for (i, &v) in taken.iter().enumerate() {
        t.release(yes, v, &taken[i + 1..]);
    }
    t.builder.jump(yes, join, &[]);
    t.release(no, x, &[y]);
    t.release(no, y, &[]);
    t.builder.jump(no, join, &[]);
    t.builder.push(
        join,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.builder.ret(join, None);
    t.check()
}

#[test]
fn storage_initialized_on_one_arm_and_emptied_there_holds_nothing() {
    pair(emptied_on_one_arm, |k| matches!(k, K::LeakedStorage(_)));
}

/// Two taken resources finalized by the exit cleanup; names release in
/// descending declaration order, so the later parameter finalizes first.
fn finalize_order(swap: bool) -> Vec<K> {
    let res = (Passing::Take, l::RES);
    let mut t = Lir::<Ready>::new(&[res, res], None);
    let slots = [
        t.slot(l::RES, SlotKind::Parameter(0), ROOT),
        t.slot(l::RES, SlotKind::Parameter(1), ROOT),
    ];
    for &s in &slots {
        let v = t.version_of(s);
        t.param(v);
    }
    let undo = [Undo::Drop(slots[1]), Undo::Drop(slots[0])];
    let addresses: Vec<_> = slots.iter().map(|&s| t.at(ENTRY, s, &[], &undo)).collect();
    let exit = t.builder.block(BlockKind::Cleanup, &[]);
    t.builder.jump(ENTRY, exit, &[]);
    let order: [usize; 2] = if swap { [0, 1] } else { [1, 0] };
    for i in order {
        let anchor = t.cleanup_anchor();
        t.builder.push(
            exit,
            Inst::FinalizeBegin {
                address: addresses[i],
                anchor,
            },
        );
        t.builder.push(
            exit,
            Inst::FinalizeEnd {
                address: addresses[i],
                anchor,
            },
        );
    }
    t.builder.ret(exit, None);
    let succeeds = t.bool_const(ENTRY, true);
    let (mut unit, layouts, manifest) = t.finish();
    // Ready failure cleanup uses the same explicit scalar-resource phases.
    let mut instructions = Vec::new();
    for block in &mut unit.blocks {
        let code = block
            .instructions
            .get(&unit.instructions)
            .unwrap()
            .iter()
            .flat_map(|i| match *i {
                Inst::Drop {
                    address,
                    anchor,
                    failure,
                    ..
                } => vec![
                    Inst::FinalizeBegin { address, anchor },
                    Inst::FinalizeEnd { address, anchor },
                    Inst::Check {
                        condition: succeeds,
                        failure,
                    },
                ],
                _ => vec![i.clone()],
            });
        block.instructions = PoolRange::push(&mut instructions, code);
    }
    unit.instructions = instructions;
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    hirsel_lir::validate::check(
        &unit,
        &hirsel_lir::validate::Inputs {
            operations: &(),
            target: &target,
            layouts: &layouts,
            manifest: &manifest,
        },
    )
    .into_iter()
    .map(|v| v.kind)
    .collect()
}

#[test]
fn explicit_finalization_follows_the_cleanup_order() {
    pair(finalize_order, |k| matches!(k, K::CleanupOrder(_)));
}

/// Both arms empty the resource in different ways: moving it out, or
/// finalizing it in place and taking its counted field. Its sibling scalar
/// remains initialized, independently of whether the slot owns anything.
fn moved_or_finalized_resource_join(keep: u8) -> Vec<K> {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, FieldLayout, ScanStep, SizeSpec};
    let mut t = Lir::<Ready>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let mut parent = t.table.layouts[l::PAIR.0 as usize].clone();
    parent.size = SizeSpec::Fixed(24);
    parent.fields = vec![
        FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: l::RESCNT,
            unaligned: false,
        },
        FieldLayout {
            ordinal: 1,
            offset: 16,
            layout: l::I64,
            unaligned: false,
        },
    ];
    parent.copy = CopyRecipe::NotCopyable;
    parent.scan = vec![ScanStep::Counted { offset: 8 }];
    parent.drop = vec![DropStep::Child {
        offset: 0,
        layout: l::RESCNT,
    }];
    let layout = LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(parent);
    let slot = t.slot(layout, SlotKind::Local, ROOT);
    let whole = t.address(AddressBase::Slot(slot), &[], layout);
    let resource = t.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(0)],
        l::RESCNT,
    );
    let counted = t.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(0), ProjectionStep::Field(1)],
        l::BLK,
    );
    let scalar = t.address(AddressBase::Slot(slot), &[ProjectionStep::Field(1)], l::I64);
    let temporary = t.slot(l::RESCNT, SlotKind::Temporary, ROOT);
    let temporary_whole = t.address(AddressBase::Slot(temporary), &[], l::RESCNT);
    let temporary_counted = t.address(
        AddressBase::Slot(temporary),
        &[ProjectionStep::Field(1)],
        l::BLK,
    );
    for address in [
        whole,
        resource,
        counted,
        scalar,
        temporary_whole,
        temporary_counted,
    ] {
        t.prepare(ENTRY, address, &[x]);
    }
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let n = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RESCNT,
        &[(0, n), (1, x)],
        placement,
    );
    let r = t.owned(l::RESCNT);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct,
            failure,
        },
    );
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        layout,
        &[(0, r), (1, n)],
        placement,
    );
    let parent = t.owned(layout);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: parent,
            construct,
            failure,
        },
    );
    let version = t.store(ENTRY, whole, parent, slot);
    let moved = t.builder.block(BlockKind::Body, &[]);
    let finalized = t.builder.block(BlockKind::Body, &[]);
    let joined = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (moved, &[]), (finalized, &[]));
    let value = t.read(moved, resource, version, ReadMode::Take, None);
    t.builder.push(moved, Inst::StorageLive { slot: temporary });
    let temporary_version = t.store(moved, temporary_whole, value, temporary);
    let anchor = t.cleanup_anchor();
    t.builder.push(
        moved,
        Inst::FinalizeBegin {
            address: temporary_whole,
            anchor,
        },
    );
    t.builder.push(
        moved,
        Inst::FinalizeEnd {
            address: temporary_whole,
            anchor,
        },
    );
    let value = t.read(
        moved,
        temporary_counted,
        temporary_version,
        ReadMode::Take,
        None,
    );
    t.builder.push(
        moved,
        Inst::StorageDead {
            slot: temporary,
            if_live: false,
        },
    );
    t.release(moved, value, &[]);
    t.builder.jump(moved, joined, &[]);
    if keep != 1 {
        let anchor = t.cleanup_anchor();
        t.builder.push(
            finalized,
            Inst::FinalizeBegin {
                address: resource,
                anchor,
            },
        );
        t.builder.push(
            finalized,
            Inst::FinalizeEnd {
                address: resource,
                anchor,
            },
        );
        if keep != 2 {
            let value = t.read(finalized, counted, version, ReadMode::Take, None);
            t.release(finalized, value, &[]);
        }
    }
    t.builder.jump(finalized, joined, &[]);
    t.read(joined, scalar, version, ReadMode::Bits, None);
    t.builder.push(
        joined,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(joined, None);
    t.check()
}

#[test]
fn a_join_keeps_the_proof_that_every_arm_has_emptied_its_owners() {
    for keep in [1, 2] {
        pair(
            |bad| moved_or_finalized_resource_join(if bad { keep } else { 0 }),
            |k| matches!(k, K::LeakedStorage(_)),
        );
    }
}

/// FinalizeEnd establishes the active variant even when its resource contains
/// only plain fields. A join with a live arm must not inherit that proof.
fn finalized_plain_variant_join(keep_live: bool) -> Vec<K> {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep};
    let mut t = Lir::<Ready>::new(&[], None);
    let mut variant = t.table.layouts[l::OPT.0 as usize].clone();
    variant.tag.as_mut().unwrap().variants[1].fields[0].layout = l::RES;
    variant.copy = CopyRecipe::NotCopyable;
    variant.drop = vec![DropStep::ActiveVariant {
        tag_offset: 0,
        form: hirsel_lir::layout_types::TagForm::Ordinal,
        variants: vec![(
            1,
            vec![DropStep::Child {
                offset: 8,
                layout: l::RES,
            }],
        )],
    }];
    let layout = LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(variant);
    let slot = t.slot(layout, SlotKind::Local, ROOT);
    let whole = t.address(AddressBase::Slot(slot), &[], layout);
    let resource = t.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        l::RES,
    );
    t.prepare(ENTRY, whole, &[]);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let n = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RES,
        &[(0, n)],
        placement,
    );
    let r = t.owned(l::RES);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: r,
            construct,
            failure,
        },
    );
    let construct = t.construct_site(
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        layout,
        &[(0, r)],
        placement,
    );
    let v = t.owned(layout);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: v,
            construct,
            failure,
        },
    );
    t.store(ENTRY, whole, v, slot);
    // The tag check reads the stored variant. Its failure cleanup finalizes
    // the resource through its own preparation, retrying as the support
    // crate's slot drops do.
    let cleanup = t.builder.block(BlockKind::FailureCleanup, &[]);
    let again = t.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        l::RES,
    );
    let retry = t.failure(cleanup);
    t.builder.push(
        cleanup,
        Inst::Address {
            address: again,
            failure: retry,
        },
    );
    let anchor = t.cleanup_anchor();
    t.builder.push(
        cleanup,
        Inst::FinalizeBegin {
            address: again,
            anchor,
        },
    );
    t.builder.push(
        cleanup,
        Inst::FinalizeEnd {
            address: again,
            anchor,
        },
    );
    t.builder.term(cleanup, Terminator::Propagate);
    let failure = t.failure(cleanup);
    t.builder.push(
        ENTRY,
        Inst::Address {
            address: resource,
            failure,
        },
    );
    let a = t.builder.block(BlockKind::Body, &[]);
    let b = t.builder.block(BlockKind::Body, &[]);
    let joined = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (a, &[]), (b, &[]));
    for (index, block) in [a, b].into_iter().enumerate() {
        if !(keep_live && index == 1) {
            let anchor = t.cleanup_anchor();
            t.builder.push(
                block,
                Inst::FinalizeBegin {
                    address: resource,
                    anchor,
                },
            );
            t.builder.push(
                block,
                Inst::FinalizeEnd {
                    address: resource,
                    anchor,
                },
            );
        }
        t.builder.jump(block, joined, &[]);
    }
    t.builder.push(
        joined,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(joined, None);
    t.check()
}

#[test]
fn finalized_plain_variant_payloads_remain_empty_at_a_join() {
    pair(finalized_plain_variant_join, |k| {
        matches!(k, K::LeakedStorage(_))
    });
}

/// C derives None's empty-owner edge independently. The mutated None arm either
/// gets a new Some owner, carries a counted field, or joins a live Some alternative.
fn empty_option_switch(mutant: u8) -> Vec<K> {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, ScanStep, TagForm};
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::OPT)], None);
    let mut variant = t.table.layouts[l::OPT.0 as usize].clone();
    let mut counted = variant.tag.as_ref().unwrap().variants[1].fields[0].clone();
    counted.layout = l::BLK;
    variant.tag.as_mut().unwrap().variants[1].fields[0].layout = l::RES;
    if mutant == 2 {
        variant.tag.as_mut().unwrap().variants[0].fields = vec![counted];
    }
    variant.copy = CopyRecipe::NotCopyable;
    variant.scan = if mutant == 2 {
        vec![ScanStep::ActiveVariant {
            tag_offset: 0,
            form: TagForm::Ordinal,
            variants: vec![(0, vec![ScanStep::Counted { offset: 8 }])],
        }]
    } else {
        vec![]
    };
    let mut drops = vec![(
        1,
        vec![DropStep::Child {
            offset: 8,
            layout: l::RES,
        }],
    )];
    if mutant == 2 {
        drops.insert(0, (0, vec![DropStep::Release { offset: 8 }]));
    }
    variant.drop = vec![DropStep::ActiveVariant {
        tag_offset: 0,
        form: TagForm::Ordinal,
        variants: drops,
    }];
    t.table.layouts[l::OPT.0 as usize] = variant;
    let slot = t.slot(l::OPT, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let whole = t.at(ENTRY, slot, &[], &[Undo::Drop(slot)]);
    let loan = t.loan(whole, false);
    t.begin(ENTRY, loan);
    let view = t.read(ENTRY, whole, version, ReadMode::Borrow, Some(loan));
    let none = t.builder.block(BlockKind::Body, &[]);
    let some = t.builder.block(BlockKind::Body, &[]);
    let impossible = t.builder.block(BlockKind::Body, &[]);
    t.builder.term(impossible, Terminator::Unreachable);
    let joined = t.builder.block(BlockKind::Body, &[]);
    let none_edge = t.builder.edge(none, &[]);
    let some_edge = t.builder.edge(some, &[]);
    let otherwise = t.builder.edge(impossible, &[]);
    let arms = PoolRange::push(
        &mut t.builder.unit.pools.switch_arms,
        [
            SwitchArm {
                value: 0,
                edge: none_edge,
            },
            SwitchArm {
                value: 1,
                edge: some_edge,
            },
        ],
    );
    t.builder.term(
        ENTRY,
        Terminator::Switch {
            value: view,
            arms,
            otherwise,
        },
    );
    t.end(none, loan);
    t.end(some, loan);
    if mutant == 4 {
        let loan = t.loan(whole, true);
        t.begin(none, loan);
        t.end(none, loan);
    }
    if mutant == 1 {
        let old = t.read(none, whole, version, ReadMode::Take, None);
        let consumed = t.builder.block(BlockKind::Body, &[]);
        let failure = t.builder.propagate();
        t.call(
            none,
            &[(Passing::Take, l::OPT)],
            None,
            &[ArgumentValue::Value(old)],
            (consumed, &[]),
            failure,
        );
        let none = consumed;
        let n = t.int(none);
        let symbol = t.symbol(SymbolKind::Type);
        let placement = t.heap();
        let construct = t.construct_site(
            ConstructorKind::Record(symbol),
            l::RES,
            &[(0, n)],
            placement,
        );
        let resource = t.owned(l::RES);
        let failure = t.fails();
        t.builder.push(
            none,
            Inst::Construct {
                result: resource,
                construct,
                failure,
            },
        );
        let construct = t.construct_site(
            ConstructorKind::Variant {
                definition: symbol,
                variant: 1,
            },
            l::OPT,
            &[(0, resource)],
            placement,
        );
        let value = t.owned(l::OPT);
        let failure = t.fails();
        t.builder.push(
            none,
            Inst::Construct {
                result: value,
                construct,
                failure,
            },
        );
        t.store(none, whole, value, slot);
        t.builder.jump(none, joined, &[]);
    }
    if mutant != 1 {
        t.builder.jump(none, joined, &[]);
    }
    let left = t.builder.block(BlockKind::Body, &[]);
    let right = t.builder.block(BlockKind::Body, &[]);
    t.branch(some, (left, &[]), (right, &[]));
    for (index, block) in [left, right].into_iter().enumerate() {
        if mutant != 3 || index == 0 {
            let resource = t.at(
                block,
                slot,
                &[ProjectionStep::VariantField {
                    variant: 1,
                    field: 0,
                }],
                &[Undo::Drop(slot)],
            );
            let value = t.read(block, resource, version, ReadMode::Take, None);
            let failure = t.cleanup(&[Undo::Drop(slot)]);
            t.call(
                block,
                &[(Passing::Take, l::RES)],
                None,
                &[ArgumentValue::Value(value)],
                (joined, &[]),
                failure,
            );
            continue;
        }
        t.builder.jump(block, joined, &[]);
    }
    t.builder.push(
        joined,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(joined, None);
    t.check()
}
#[test]
fn a_switch_empty_option_fact_is_killed_by_writes_and_intersected_at_joins() {
    assert!(
        empty_option_switch(0).is_empty(),
        "{:?}",
        empty_option_switch(0)
    );
    for mutant in [1, 2, 3, 4] {
        assert!(
            empty_option_switch(mutant)
                .iter()
                .any(|kind| matches!(kind, K::LeakedStorage(_))),
            "mutant {mutant}: {:?}",
            empty_option_switch(mutant)
        );
    }
}
