//! Linear raw reuse tokens: one hand-built mutation per obligation, each
//! paired with its valid twin.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::{Reuse, Stage};
use support::*;

const TAKE_BLK: (Passing, LayoutId) = (Passing::Take, l::BLK);

/// Reset `donor` into a token; a failing reset has finished its cleanup and
/// runs `undo`.
fn reset_then<S: Stage>(t: &mut Lir<S>, b: BlockId, donor: ValueId, undo: &[Undo]) -> ValueId {
    let layout = t.payload(donor);
    let result = t.owned(l::HDR);
    let anchor = t.cleanup_anchor();
    let target = t.cleanup(undo);
    let failure = t.failure(target);
    t.builder.push(
        b,
        Inst::Reset {
            result,
            value: donor,
            layout,
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
    result
}

/// Install a `payload` construction in `token`'s storage; a failure runs `undo`.
fn reuse<S: Stage>(
    t: &mut Lir<S>,
    b: BlockId,
    token: ValueId,
    payload: LayoutId,
    placement: Option<Placement>,
    undo: &[Undo],
) -> ValueId {
    let ordinals: Vec<_> = t.table.layouts[payload.0 as usize]
        .fields
        .iter()
        .map(|f| f.ordinal)
        .collect();
    let fields: Vec<_> = ordinals.into_iter().map(|o| (o, t.int(b))).collect();
    let symbol = t.symbol(SymbolKind::Type);
    let construct = t.construct_site(ConstructorKind::Record(symbol), payload, &fields, placement);
    let block = match payload {
        l::PAY => l::BLK,
        l::BIG => l::BIGBLK,
        l::WIDE => l::WIDEBLK,
        _ => unreachable!(),
    };
    let result = t.owned(block);
    let target = t.cleanup(undo);
    let failure = t.failure(target);
    t.builder.push(
        b,
        Inst::Reuse {
            result,
            token,
            construct,
            kept: None,
            failure,
        },
    );
    result
}

fn reset<S: Stage>(t: &mut Lir<S>, b: BlockId, donor: ValueId) -> ValueId {
    reset_then(t, b, donor, &[])
}

fn none<S: Stage>(t: &mut Lir<S>, b: BlockId) -> ValueId {
    let result = t.owned(l::HDR);
    t.builder.push(b, Inst::TokenNone { result });
    result
}

fn heap(domain: u32) -> Option<Placement> {
    Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(domain))))
}

/// One token reused twice: the second reuse needs its own token.
fn copied(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let tok = reset(&mut t, ENTRY, x);
    let first = reuse(&mut t, ENTRY, tok, l::PAY, heap(0), &[]);
    let second = if mutant { tok } else { none(&mut t, ENTRY) };
    let y = reuse(
        &mut t,
        ENTRY,
        second,
        l::PAY,
        heap(0),
        &[Undo::Release(first)],
    );
    t.release(ENTRY, first, &[y]);
    t.release(ENTRY, y, &[]);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_copied_token_is_rejected() {
    pair(copied, |k| *k == K::NoCredit(ValueId(1)));
}

fn disposed(times: usize) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let tok = reset(&mut t, ENTRY, x);
    for _ in 0..times {
        t.builder.push(ENTRY, Inst::Dispose { token: tok });
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_token_disposed_twice_is_rejected() {
    pair(
        |mutant| disposed(if mutant { 2 } else { 1 }),
        |k| *k == K::NoCredit(ValueId(1)),
    );
}

#[test]
fn a_leaked_token_is_rejected() {
    pair(
        |mutant| disposed(if mutant { 0 } else { 1 }),
        |k| *k == K::Leak(ValueId(1)),
    );
}

/// A token live around a loop with two entries crosses its feedback edges.
/// The twin disposes of it before the loop.
fn irreducible(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let tok = reset(&mut t, ENTRY, x);
    if !mutant {
        t.builder.push(ENTRY, Inst::Dispose { token: tok });
    }
    let a = t.builder.block(BlockKind::Body, &[]);
    let b = t.builder.block(BlockKind::Body, &[]);
    let exit = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (a, &[]), (b, &[]));
    t.branch(a, (b, &[]), (exit, &[]));
    t.branch(b, (a, &[]), (exit, &[]));
    if mutant {
        t.builder.push(exit, Inst::Dispose { token: tok });
    }
    t.builder.ret(exit, None);
    t.check()
}

#[test]
fn a_token_across_an_irreducible_feedback_edge_is_rejected() {
    pair(irreducible, |k| matches!(k, K::TokenFeedback(_)));
}

#[test]
fn a_token_within_one_loop_iteration_is_accepted() {
    // header -> body (new block, reset, reuse, release) -> header; header -> exit.
    let mut t = Lir::<Reuse>::new(&[], None);
    let header = t.builder.block(BlockKind::Body, &[]);
    let body = t.builder.block(BlockKind::Body, &[]);
    let exit = t.builder.block(BlockKind::Body, &[]);
    t.builder.jump(ENTRY, header, &[]);
    t.branch(header, (body, &[]), (exit, &[]));
    let y = t.new_block(body, &[]);
    let tok = reset(&mut t, body, y);
    let z = reuse(&mut t, body, tok, l::PAY, heap(0), &[]);
    t.release(body, z, &[]);
    t.builder.jump(body, header, &[]);
    t.builder.ret(exit, None);
    assert_eq!(t.check(), vec![]);
}

/// Reuse in another task heap than the donor's.
fn wrong_owner(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    t.builder.unit.tables.task_domains = 2;
    let x = t.owned(l::BLK);
    t.param(x);
    let tok = reset(&mut t, ENTRY, x);
    let y = reuse(&mut t, ENTRY, tok, l::PAY, heap(u32::from(mutant)), &[]);
    t.release(ENTRY, y, &[]);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn reuse_in_another_storage_owner_is_rejected() {
    pair(wrong_owner, |k| matches!(k, K::TokenOwner(_)));
}

fn reuse_as(payload: LayoutId) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let tok = reset(&mut t, ENTRY, x);
    let y = reuse(&mut t, ENTRY, tok, payload, heap(0), &[]);
    t.release(ENTRY, y, &[]);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn reuse_of_another_capacity_is_rejected() {
    pair(
        |mutant| reuse_as(if mutant { l::BIG } else { l::PAY }),
        |k| matches!(k, K::TokenCapacity(_)),
    );
}

#[test]
fn reuse_of_another_alignment_is_rejected() {
    let mutant = reuse_as(l::WIDE);
    assert!(
        mutant.iter().any(|k| matches!(k, K::TokenAlignment(_))),
        "{mutant:?}"
    );
    assert!(
        !mutant.iter().any(|k| matches!(k, K::TokenCapacity(_))),
        "{mutant:?}"
    );
    assert_eq!(reuse_as(l::PAY), vec![]);
}

/// A failure while a token is live disposes of it on the failure edge.
fn failure_disposal(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK, TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let w = t.owned(l::BLK);
    t.param(w);
    let tok = reset_then(&mut t, ENTRY, x, &[Undo::Release(w)]);
    let dispose: &[ValueId] = if mutant { &[] } else { &[tok] };
    let p = t.builder.propagate();
    let failure = t.failure_disposing(p, &[], dispose);
    t.release_with(ENTRY, w, failure);
    t.builder.push(ENTRY, Inst::Dispose { token: tok });
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_live_token_missing_from_a_failure_disposal_list_is_rejected() {
    pair(failure_disposal, |k| *k == K::TokenFailure(ValueId(2)));
}

/// Tokens are never passed to calls, stored, returned or captured.
fn token_argument(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let tok = reset(&mut t, ENTRY, x);
    let n = t.builder.block(BlockKind::Body, &[]);
    if mutant {
        let fail = t.builder.propagate();
        t.call(
            ENTRY,
            &[(Passing::Take, l::HDR)],
            None,
            &[ArgumentValue::Value(tok)],
            (n, &[]),
            fail,
        );
    } else {
        t.builder.push(ENTRY, Inst::Dispose { token: tok });
        let fail = t.builder.propagate();
        t.call(ENTRY, &[], None, &[], (n, &[]), fail);
    }
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn a_token_passed_to_a_call_is_rejected() {
    pair(token_argument, |k| *k == K::TokenUse(ValueId(1)));
}

/// Static storage never donates: a constant block is immortal.
fn static_donor(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let donor = if mutant {
        let ty = t.ty(ValueRepr::Block, l::BLK);
        t.builder.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Bytes(vec![0; 16]),
        });
        let constant = ConstantId(t.builder.unit.tables.constants.len() as u32 - 1);
        let c = t.owned(l::BLK);
        t.builder.push(
            ENTRY,
            Inst::Const {
                result: c,
                constant,
            },
        );
        t.release(ENTRY, x, &[c]);
        c
    } else {
        x
    };
    let tok = reset(&mut t, ENTRY, donor);
    t.builder.push(ENTRY, Inst::Dispose { token: tok });
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_static_block_is_never_a_reuse_donor() {
    pair(static_donor, |k| matches!(k, K::TokenDonor(_)));
}

/// A token join parameter receives tokens of two capacities.
fn token_join(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let p = t.owned(l::HDR);
    let join = t.builder.block(BlockKind::Body, &[p]);
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    let small = reset(&mut t, yes, x);
    t.builder.jump(yes, join, &[small]);
    let other = if mutant {
        let big = t.new_block_of(no, l::BIG, &[Undo::Release(x)]);
        t.release(no, x, &[big]);
        reset(&mut t, no, big)
    } else {
        t.release(no, x, &[]);
        none(&mut t, no)
    };
    t.builder.jump(no, join, &[other]);
    let y = reuse(&mut t, join, p, l::PAY, heap(0), &[]);
    t.release(join, y, &[]);
    t.builder.ret(join, None);
    t.check()
}

#[test]
fn a_token_join_of_two_capacities_is_rejected_and_none_joins_any() {
    pair(token_join, |k| matches!(k, K::TokenJoin(_)));
}

// --- LIR schema revision 5: moved fields, release masks, kept fields and
// closure donors. Each rule's mutant passes every other check, so the rule
// alone rejects it.

/// A static mask over `fields` fields with `live` set.
fn mask<S: Stage>(t: &mut Lir<S>, fields: u32, live: &[u32]) -> MaskId {
    let mut word = 0u64;
    for &f in live {
        word |= 1 << f;
    }
    let words = PoolRange::push(&mut t.builder.unit.pools.mask_words, [word]);
    t.builder
        .unit
        .tables
        .masks
        .push(FieldMask { fields, words });
    MaskId(t.builder.unit.tables.masks.len() as u32 - 1)
}

/// How a `PAIRBLK` donor is taken apart and rebuilt.
#[derive(Clone, Copy, PartialEq)]
enum Rebuild {
    /// Take field 0, reset with field 1 live, rebuild with field 0 kept.
    Kept,
    /// Take field 0 twice.
    TakeTwice,
    /// Read field 0 after it was taken.
    ReadMoved,
    /// Reset with every field live although field 0 was taken.
    MaskAll,
    /// Reset with both fields cleared although only field 0 was taken.
    MaskBoth,
    /// Swap the labelled fields and keep field 1: its operand came from field 0.
    KeptSwapped,
    /// Keep field 0 although the reset released it (no take).
    KeptUntaken,
}

fn rebuild(how: Rebuild) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[(Passing::Take, l::PAIRBLK)], None);
    let x = t.owned(l::PAIRBLK);
    t.param(x);
    let n = t.new_block(ENTRY, &[Undo::Release(x)]);
    let (taken, live) = if how == Rebuild::KeptUntaken {
        let b = t.through(
            ENTRY,
            x,
            &[ProjectionStep::Field(0)],
            &[Undo::Release(x), Undo::Release(n)],
        );
        let c = t.owned(l::BLK);
        let target = t.releasing(&[x, n]);
        let failure = t.failure(target);
        t.builder.push(
            ENTRY,
            Inst::Copy {
                result: c,
                address: b,
                reaching: x,
                failure,
            },
        );
        (c, mask(&mut t, 2, &[0, 1]))
    } else {
        let a = t.through(
            ENTRY,
            x,
            &[ProjectionStep::Field(0)],
            &[Undo::Release(x), Undo::Release(n)],
        );
        let taken = t.read(ENTRY, a, x, ReadMode::Take, None);
        if how == Rebuild::TakeTwice {
            let again = t.read(ENTRY, a, x, ReadMode::Take, None);
            t.builder.push(ENTRY, Inst::Forget { value: again });
        }
        if how == Rebuild::ReadMoved {
            let loan = t.loan(a, false);
            t.begin(ENTRY, loan);
            t.read(ENTRY, a, x, ReadMode::Borrow, Some(loan));
            t.end(ENTRY, loan);
        }
        let live = match how {
            Rebuild::MaskAll => mask(&mut t, 2, &[0, 1]),
            Rebuild::MaskBoth => mask(&mut t, 2, &[]),
            _ => mask(&mut t, 2, &[1]),
        };
        (taken, live)
    };
    let result = t.owned(l::HDR);
    let anchor = t.cleanup_anchor();
    let target = t.releasing(&[taken, n]);
    let failure = t.failure(target);
    t.builder.push(
        ENTRY,
        Inst::Reset {
            result,
            value: x,
            layout: l::PAIR,
            live: MaskRef::Static(live),
            anchor,
            failure,
        },
    );
    let symbol = t.symbol(SymbolKind::Type);
    let (fields, kept) = match how {
        Rebuild::KeptSwapped => ([(0, n), (1, taken)], [1]),
        _ => ([(0, taken), (1, n)], [0]),
    };
    let placement = heap(0);
    let construct = t.construct_site(ConstructorKind::Record(symbol), l::PAIR, &fields, placement);
    let kept = mask(&mut t, 2, &kept);
    let y = t.owned(l::PAIRBLK);
    let target = t.cleanup(&[]);
    let failure = t.failure(target);
    t.builder.push(
        ENTRY,
        Inst::Reuse {
            result: y,
            token: result,
            construct,
            kept: Some(kept),
            failure,
        },
    );
    t.release(ENTRY, y, &[]);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_field_taken_out_of_an_owned_block_twice_is_rejected() {
    pair(
        |mutant| {
            rebuild(if mutant {
                Rebuild::TakeTwice
            } else {
                Rebuild::Kept
            })
        },
        |k| *k == K::MovedValuePart(ValueId(0)),
    );
}

#[test]
fn a_field_read_after_its_move_is_rejected() {
    pair(
        |mutant| {
            rebuild(if mutant {
                Rebuild::ReadMoved
            } else {
                Rebuild::Kept
            })
        },
        |k| *k == K::MovedValuePart(ValueId(0)),
    );
}

#[test]
fn a_reset_mask_must_clear_exactly_the_moved_fields() {
    for wrong in [Rebuild::MaskAll, Rebuild::MaskBoth] {
        pair(
            |mutant| rebuild(if mutant { wrong } else { Rebuild::Kept }),
            |k| *k == K::ReleaseMask(ValueId(0)),
        );
    }
}

#[test]
fn a_kept_field_needs_the_take_of_the_same_physical_slot() {
    pair(
        |mutant| {
            rebuild(if mutant {
                Rebuild::KeptSwapped
            } else {
                Rebuild::Kept
            })
        },
        |k| matches!(k, K::KeptField { field: 1, .. }),
    );
    pair(
        |mutant| {
            rebuild(if mutant {
                Rebuild::KeptUntaken
            } else {
                Rebuild::Kept
            })
        },
        |k| matches!(k, K::KeptField { field: 0, .. }),
    );
}

/// A take through a borrowed block, which its owner still holds.
fn take_borrowed(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[(Passing::Take, l::PAIRBLK)], None);
    let x = t.owned(l::PAIRBLK);
    t.param(x);
    let base = if mutant {
        let s = t.slot(l::PAIRBLK, SlotKind::Local, ScopeId(0));
        t.builder.push(ENTRY, Inst::StorageLive { slot: s });
        let whole = t.at(ENTRY, s, &[], &[Undo::Release(x)]);
        t.store(ENTRY, whole, x, s);
        let loan = t.loan(whole, false);
        t.begin(ENTRY, loan);
        let v = t.borrowed(l::PAIRBLK, loan);
        let version = ValueId(t.builder.unit.values.len() as u32 - 2);
        t.builder.push(
            ENTRY,
            Inst::Read {
                result: v,
                address: whole,
                reaching: version,
                mode: ReadMode::Borrow,
            },
        );
        v
    } else {
        x
    };
    let undo: &[Undo] = if mutant { &[] } else { &[Undo::Release(x)] };
    let a = t.through(ENTRY, base, &[ProjectionStep::Field(0)], undo);
    let taken = t.read(ENTRY, a, base, ReadMode::Take, None);
    if !mutant {
        // The donor's rest goes with a mask that clears the moved field.
        let live = mask(&mut t, 2, &[1]);
        let anchor = t.cleanup_anchor();
        let target = t.releasing(&[taken]);
        let failure = t.failure(target);
        t.builder.push(
            ENTRY,
            Inst::Release {
                value: x,
                layout: l::PAIR,
                live: MaskRef::Static(live),
                anchor,
                failure,
            },
        );
    }
    t.release(ENTRY, taken, &[]);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_take_through_a_borrowed_block_is_rejected() {
    assert_eq!(take_borrowed(false), vec![]);
    // The mutant's other faults (its slot) are not what this rule is about.
    let mutant = take_borrowed(true);
    assert!(
        mutant.iter().any(|k| matches!(k, K::WriteThroughValue(_))),
        "{mutant:?}"
    );
}

/// A closure donates its environment: the token's layout is the layout of
/// the environment the closure was built with, never another payload, and
/// never a callable whose closure is unknown (a parameter).
fn closure_donor(mutant: bool) -> Vec<K> {
    closure_donor_typed(mutant, false)
}

fn closure_donor_typed(mutant: bool, erased: bool) -> Vec<K> {
    let mut t = Lir::<Reuse>::new(&[], None);
    if erased {
        t.table.layouts[l::CLOSURE.0 as usize].kind =
            hirsel_lir::layout_types::LayoutKind::Callable { environment: None };
    }
    let k = t.int(ENTRY);
    let env = t.symbol(SymbolKind::Function);
    let abi = t.abi_shaped(hirsel_lir::abi_types::CallableShape::Closure, &[], None);
    let captures = PoolRange::push(
        &mut t.builder.unit.pools.captures,
        [CaptureValue {
            value: k,
            usage: CaptureUse::Copy,
        }],
    );
    t.builder.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode {
            symbol: env,
            abi: abi,
        }),
        environment: l::ENV,
        captures,
        destination: DestinationClass::Heap,
        placement: heap(0),
        anchor: SourceAnchorId(0),
    });
    let c = t.owned(l::CLOSURE);
    let fails = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Closure {
            result: c,
            closure: ClosureId(t.builder.unit.tables.closures.len() as u32 - 1),
            failure: fails,
        },
    );
    // An erased callable may carry a Shared root, so its semantic cleanup
    // anchor still owns the release even when its environment is known here.
    let at = if erased {
        let cleanup = t.builder.block(BlockKind::Cleanup, &[]);
        t.builder.jump(ENTRY, cleanup, &[]);
        cleanup
    } else {
        ENTRY
    };
    let result = t.owned(l::HDR);
    let anchor = t.cleanup_anchor();
    let failure = t.fails();
    t.builder.push(
        at,
        Inst::Reset {
            result,
            value: c,
            layout: if mutant { l::PAY } else { l::ENV },
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
    let y = reuse(&mut t, at, result, l::PAY, heap(0), &[]);
    t.release(at, y, &[]);
    t.builder.ret(at, None);
    t.check()
}

#[test]
fn a_closure_donates_by_its_environment_layout() {
    pair(closure_donor, |k| matches!(k, K::TokenDonor(_)));
}

#[test]
fn an_erased_closure_donor_requires_its_actual_environment() {
    pair(
        |mutant| closure_donor_typed(mutant, true),
        |k| matches!(k, K::TokenDonor(_)),
    );
}
