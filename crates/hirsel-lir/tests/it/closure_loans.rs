//! A scoped closure called in a loop with no per-call retain or release
//! (bcb67a10): the call borrows the closure's slot under an ordinary read
//! loan, and C's loan and holder-credit replay is the lifetime proof. No LIR
//! fact claims it, so a forgery is a unit that elides RC work the closure's
//! lifetime still needs. Each forgery here is built by the same function as
//! its accepted twin.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Rc;
use support::*;

/// Elisions a forged unit claims although the closure's lifetime needs them.
#[derive(Clone, Copy, PartialEq)]
enum Forgery {
    /// The escaping callee takes the loop's only credit instead of a copy.
    MovesOut,
    /// The slot's credit is released inside the loop while the loan is
    /// active, and again at the exit.
    ReleasesUnderLoan,
    /// The loan ends before the call that uses the borrowed callable.
    EndsLoanBeforeCall,
}

/// `let f = fn() { x }; while _ { f() }`, the closure in slot `s`: entry
/// builds it, the header joins its version, the body borrows it for a bound
/// call, and the exit releases it once. With `escape`, every iteration also
/// passes a copy (a Dup) to a `take` callee.
fn kernel(escape: bool, forgery: Option<Forgery>) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], None);
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    let code = t.symbol(SymbolKind::Function);
    let x = t.int(ENTRY);
    let captures = PoolRange::push(
        &mut t.unit.pools.captures,
        [CaptureValue {
            value: x,
            usage: CaptureUse::Copy,
        }],
    );
    t.unit.tables.closures.push(ClosureSite {
        code: Some(ClosureCode { symbol: code, abi }),
        environment: l::ENV,
        captures,
        destination: DestinationClass::Heap,
        placement: t.heap(),
        anchor: SourceAnchorId(0),
    });
    let closure = ClosureId(t.unit.tables.closures.len() as u32 - 1);
    let made = t.owned(l::CLOSURE);
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Closure {
            result: made,
            closure,
            failure,
        },
    );
    let s = t.slot(l::CLOSURE, SlotKind::Local, ROOT);
    t.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[Undo::Release(made)]);
    let first = t.store(ENTRY, a, made, s);

    let at_head = t.version_of(s);
    let head = t.block(BlockKind::Body, &[at_head]);
    let at_body = t.version_of(s);
    let body = t.block(BlockKind::Body, &[at_body]);
    let at_exit = t.version_of(s);
    let exit = t.block(BlockKind::Body, &[at_exit]);
    t.jump(ENTRY, head, &[first]);
    t.branch(head, (body, &[at_head]), (exit, &[at_head]));

    let loan = t.loan(a, false);
    t.begin(body, loan);
    let f = t.read(body, a, at_body, ReadMode::Borrow, Some(loan));
    if forgery == Some(Forgery::ReleasesUnderLoan) {
        let taken = t.read(body, a, at_body, ReadMode::Take, None);
        t.release_then(body, taken, &[Undo::End(loan)]);
    }
    if forgery == Some(Forgery::EndsLoanBeforeCall) {
        t.end(body, loan);
    }
    let after = t.block(BlockKind::Body, &[]);
    let failed = t.cleanup(&[Undo::End(loan), Undo::Drop(s)]);
    t.call_to(
        body,
        Callee::Bound { code, closure: f },
        abi,
        &[],
        None,
        &[],
        (after, &[]),
        failed,
    );
    if forgery != Some(Forgery::EndsLoanBeforeCall) {
        t.end(after, loan);
    }
    let mut tail = after;
    if escape {
        let escaping = if forgery == Some(Forgery::MovesOut) {
            t.read(after, a, at_body, ReadMode::Take, None)
        } else {
            let copy = t.owned(l::CLOSURE);
            let rest = t.cleanup(&[Undo::Drop(s)]);
            let failure = t.failure(rest);
            t.push(
                after,
                Inst::Copy {
                    result: copy,
                    address: a,
                    reaching: at_body,
                    failure,
                },
            );
            copy
        };
        tail = t.block(BlockKind::Body, &[]);
        let failed = t.cleanup(&[Undo::Drop(s)]);
        t.call(
            after,
            &[(Passing::Take, l::CLOSURE)],
            None,
            &[ArgumentValue::Value(escaping)],
            (tail, &[]),
            failed,
        );
    }
    t.jump(tail, head, &[at_body]);

    let done = t.read(exit, a, at_exit, ReadMode::Take, None);
    t.release(exit, done, &[]);
    t.push(
        exit,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.ret(exit, None);
    t.check()
}

/// The scoped kernel itself: no Dup or Release in the loop, accepted.
#[test]
fn a_scoped_closure_borrowed_by_every_iteration_is_accepted() {
    let violations = kernel(false, None);
    assert!(violations.is_empty(), "{violations:#?}");
}

/// An escaping closure needs a credit per iteration: its copy is accepted,
/// and a forged elision moving the loop's only credit out leaves the next
/// iteration borrowing an uninitialized slot.
#[test]
fn an_escaping_closure_keeps_its_per_iteration_copy() {
    pair(
        |mutant| kernel(true, mutant.then_some(Forgery::MovesOut)),
        |k| matches!(k, K::Uninitialized(_)),
    );
}

/// Releasing the closure inside the loop while its call's loan is active is
/// rejected; the twin releases it once after the loop.
#[test]
fn a_closure_released_under_its_call_loan_is_rejected() {
    pair(
        |mutant| kernel(false, mutant.then_some(Forgery::ReleasesUnderLoan)),
        |k| matches!(k, K::HolderUnderLoan(_)),
    );
}

/// A borrowed callable used after its loan ended is rejected; the twin ends
/// the loan after the call.
#[test]
fn a_callable_called_after_its_loan_ends_is_rejected() {
    pair(
        |mutant| kernel(false, mutant.then_some(Forgery::EndsLoanBeforeCall)),
        |k| matches!(k, K::OutsideLoan(_)),
    );
}
