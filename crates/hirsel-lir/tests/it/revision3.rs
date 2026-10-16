//! LIR schema revision 3: the
//! uniqueness test, structural comparison and callable weakening. Every check
//! is a hand-built mutation paired with its accepted twin.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::{Placed, Rc, Stage, Value};
use support::*;

/// `Compare` of two borrowed parameters of `operand` at `at`, defining a
/// value of `result`.
fn compare_params(operand: LayoutId, at: LayoutId, result: LayoutId) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Borrow, operand), (Passing::Borrow, operand)],
        Some(result),
    );
    let x = t.plain(operand);
    t.param(x);
    let y = t.plain(operand);
    t.param(y);
    let r = t.plain(result);
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Compare {
            result: r,
            op: CompareOp::Lt,
            left: x,
            right: y,
            layout: at,
            failure,
        },
    );
    t.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_scalar_layout_compares_with_binary_not_compare() {
    pair(
        |mutant| {
            let layout = if mutant { l::I64 } else { l::BLK };
            compare_params(layout, layout, l::BOOL)
        },
        |k| *k == K::Type("structural comparison of a scalar layout"),
    );
}

#[test]
fn a_layout_holding_a_callable_is_not_comparable() {
    pair(
        |mutant| {
            let layout = if mutant { l::ERASED } else { l::PAY };
            compare_params(layout, layout, l::BOOL)
        },
        |k| *k == K::Type("structural comparison of an incomparable layout"),
    );
}

#[test]
fn both_operands_have_the_compared_layout() {
    pair(
        |mutant| compare_params(l::BLK, if mutant { l::PAIRBLK } else { l::BLK }, l::BOOL),
        |k| *k == K::Type("structural comparison operands"),
    );
}

#[test]
fn a_comparison_defines_a_bool() {
    pair(
        |mutant| compare_params(l::OPT, l::OPT, if mutant { l::I64 } else { l::BOOL }),
        |k| *k == K::Type("structural comparison result"),
    );
}

/// Two fresh blocks compared, then released; the mutant releases the left
/// operand before the comparison reads it.
fn compare_owned(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::BOOL));
    let a = t.new_block(ENTRY, &[]);
    let b = t.new_block(ENTRY, &[Undo::Release(a)]);
    if mutant {
        t.release(ENTRY, a, &[b]);
    }
    let r = t.plain(l::BOOL);
    let held = if mutant { vec![b] } else { vec![a, b] };
    let target = t.releasing(&held);
    let failure = t.failure(target);
    t.push(
        ENTRY,
        Inst::Compare {
            result: r,
            op: CompareOp::Eq,
            left: a,
            right: b,
            layout: l::BLK,
            failure,
        },
    );
    if !mutant {
        t.release(ENTRY, a, &[b]);
    }
    t.release(ENTRY, b, &[]);
    t.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_compared_operand_must_still_be_held() {
    pair(compare_owned, |k| matches!(k, K::NoCredit(_)));
}

/// A borrowed field view compared with itself; the mutant ends its loan first.
fn compare_view(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::PAIRBLK)], Some(l::BOOL));
    let u = t.owned(l::PAIRBLK);
    t.param(u);
    let au = t.through(ENTRY, u, &[ProjectionStep::Field(0)], &[Undo::Release(u)]);
    let loan = t.loan(au, false);
    t.begin(ENTRY, loan);
    let w = t.read(ENTRY, au, u, ReadMode::Borrow, Some(loan));
    if mutant {
        t.end(ENTRY, loan);
    }
    let r = t.plain(l::BOOL);
    let undo = if mutant {
        vec![Undo::Release(u)]
    } else {
        vec![Undo::End(loan), Undo::Release(u)]
    };
    let target = t.cleanup(&undo);
    let failure = t.failure(target);
    t.push(
        ENTRY,
        Inst::Compare {
            result: r,
            op: CompareOp::Ge,
            left: w,
            right: w,
            layout: l::BLK,
            failure,
        },
    );
    if !mutant {
        t.end(ENTRY, loan);
    }
    t.release(ENTRY, u, &[]);
    t.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_compared_view_is_read_inside_its_loan() {
    pair(compare_view, |k| matches!(k, K::OutsideLoan(_)));
}

/// `IsUnique` of a fresh block, which is then released; the mutants test a
/// released block or an inline record.
#[derive(Clone, Copy, PartialEq)]
enum Unique {
    Valid,
    Released,
    Uncounted,
    NotBool,
}

fn is_unique<S: Stage>(case: Unique) -> Vec<K> {
    let layout = if case == Unique::Uncounted {
        l::PAY
    } else {
        l::BLK
    };
    let answer = if case == Unique::NotBool {
        l::U64
    } else {
        l::BOOL
    };
    let mut t = Lir::<S>::new(&[(Passing::Borrow, layout)], Some(answer));
    let x = t.plain(layout);
    t.param(x);
    let a = t.new_block(ENTRY, &[]);
    if case == Unique::Released {
        t.release(ENTRY, a, &[]);
    }
    let value = if case == Unique::Uncounted { x } else { a };
    let after = t.block(BlockKind::Body, &[]);
    let r = t.plain(answer);
    t.push(ENTRY, Inst::IsUnique { result: r, value });
    t.jump(ENTRY, after, &[]);
    if case != Unique::Released {
        t.release(after, a, &[]);
    }
    t.ret(after, Some(r));
    t.check()
}

#[test]
fn the_uniqueness_test_reads_a_held_counted_block() {
    pair(
        |mutant| {
            is_unique::<Rc>(if mutant {
                Unique::Released
            } else {
                Unique::Valid
            })
        },
        |k| matches!(k, K::NoCredit(_)),
    );
    pair(
        |mutant| {
            is_unique::<Rc>(if mutant {
                Unique::Uncounted
            } else {
                Unique::Valid
            })
        },
        |k| *k == K::Type("uniqueness test of an uncounted value"),
    );
    pair(
        |mutant| {
            is_unique::<Rc>(if mutant {
                Unique::NotBool
            } else {
                Unique::Valid
            })
        },
        |k| *k == K::Type("uniqueness test result"),
    );
}

#[test]
fn the_uniqueness_test_is_an_instruction_from_rc_on() {
    pair(
        |mutant| {
            if mutant {
                is_unique::<Placed>(Unique::Valid)
            } else {
                is_unique::<Rc>(Unique::Valid)
            }
        },
        |k| *k == K::Vocabulary(Opcode::IsUnique),
    );
}

/// A taken callable of `from` weakened to `to` and returned; `twice` weakens
/// the same value again and returns that result.
fn weaken(from: LayoutId, to: LayoutId, twice: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, from)], Some(to));
    let f = t.owned(from);
    t.param(f);
    let g = t.owned(to);
    t.push(
        ENTRY,
        Inst::Weaken {
            result: g,
            value: f,
        },
    );
    let mut returned = g;
    if twice {
        returned = t.owned(to);
        t.push(
            ENTRY,
            Inst::Weaken {
                result: returned,
                value: f,
            },
        );
    }
    t.ret(ENTRY, Some(returned));
    t.check()
}

#[test]
fn only_a_many_callable_weakens_to_the_once_callable_of_its_environment() {
    pair(
        |mutant| {
            if mutant {
                weaken(l::ERASED_ONCE, l::ERASED, false)
            } else {
                weaken(l::ERASED, l::ERASED_ONCE, false)
            }
        },
        |k| *k == K::Type("weakened callable"),
    );
    pair(
        |mutant| {
            weaken(
                if mutant { l::CLOSURE } else { l::ERASED },
                l::ERASED_ONCE,
                false,
            )
        },
        |k| *k == K::Type("weakened callable"),
    );
}

#[test]
fn weakening_consumes_its_operand() {
    pair(
        |mutant| weaken(l::ERASED, l::ERASED_ONCE, mutant),
        |k| *k == K::NoCredit(ValueId(0)),
    );
}

/// A borrowed view of either field of a `mut` record parameter, chosen by a
/// branch, joins into one block parameter that a call then borrows. The
/// mutants declare the greater loan, write the other branch's source while
/// the joined view is live, or end one of its loans first.
#[derive(Clone, Copy, PartialEq)]
enum Join {
    Valid,
    Declared,
    WriteSource,
    EndedLoan,
}

fn joined_view(case: Join) -> Vec<K> {
    joined_holder(case, false)
}

fn joined_holder(case: Join, holder: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, l::PAIR)], None);
    let s = t.slot(l::PAIR, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let first = t.at(ENTRY, s, &[ProjectionStep::Field(0)], &[]);
    let second = t.at(ENTRY, s, &[ProjectionStep::Field(1)], &[]);
    let l0 = t.loan(first, false);
    let l1 = t.loan(second, false);
    let p = t.borrowed(l::BLK, if case == Join::Declared { l1 } else { l0 });
    let join = t.block(BlockKind::Body, &[p]);
    let yes = t.block(BlockKind::Body, &[]);
    let no = t.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    t.begin(yes, l0);
    let a = t.read(yes, first, sv, ReadMode::Borrow, Some(l0));
    t.jump(yes, join, &[a]);
    t.begin(no, l1);
    let b = t.read(no, second, sv, ReadMode::Borrow, Some(l1));
    t.jump(no, join, &[b]);
    let end = |t: &mut Lir<Rc>, block: BlockId, loan: LoanId| {
        t.push(
            block,
            Inst::EndLoan {
                loan,
                if_active: true,
            },
        );
    };
    let fail = t.block(BlockKind::FailureCleanup, &[]);
    end(&mut t, fail, l0);
    end(&mut t, fail, l1);
    t.term(fail, Terminator::Propagate);
    let (argument, layout) = if holder {
        use hirsel_lir::layout_types::{CopyRecipe, FieldLayout, ScanStep, SizeSpec};
        let mut record = t.table.layouts[l::PAIR.0 as usize].clone();
        record.size = SizeSpec::Fixed(8);
        record.fields = vec![FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: l::BLK,
            unaligned: false,
        }];
        record.scan = vec![ScanStep::Uncounted { offset: 0 }];
        record.drop.clear();
        record.copy = CopyRecipe::NotCopyable;
        let layout = hirsel_lir::layout_types::LayoutId(t.table.layouts.len() as u32);
        t.table.layouts.push(record);
        let symbol = t.symbol(SymbolKind::Type);
        let construct =
            t.construct_site(ConstructorKind::Record(symbol), layout, &[(0, p)], t.heap());
        let result = t.plain(layout);
        let failure = t.failure(fail);
        t.push(
            join,
            Inst::Construct {
                result,
                construct,
                failure,
            },
        );
        (result, layout)
    } else {
        (p, l::BLK)
    };
    let mut at = join;
    match case {
        Join::EndedLoan => end(&mut t, join, l0),
        Join::WriteSource => {
            let written = t.block(BlockKind::Body, &[]);
            let version = t.version_of(s);
            t.call(
                join,
                &[(Passing::Mut, l::BLK)],
                None,
                &[ArgumentValue::Address {
                    address: second,
                    version,
                }],
                (written, &[]),
                fail,
            );
            at = written;
        }
        Join::Valid | Join::Declared => {}
    }
    let after = t.block(BlockKind::Body, &[]);
    t.call(
        at,
        &[(Passing::Borrow, layout)],
        None,
        &[ArgumentValue::Value(argument)],
        (after, &[]),
        fail,
    );
    end(&mut t, after, l0);
    end(&mut t, after, l1);
    t.ret(after, None);
    t.check()
}

#[test]
fn a_view_joined_from_two_loans_is_covered_by_both() {
    pair(
        |mutant| joined_view(if mutant { Join::Declared } else { Join::Valid }),
        |k| matches!(k, K::Holding(_)),
    );
}

#[test]
fn a_joined_view_is_stale_after_a_write_to_either_source() {
    pair(
        |mutant| {
            joined_view(if mutant {
                Join::WriteSource
            } else {
                Join::Valid
            })
        },
        |k| matches!(k, K::StaleView(_)),
    );
}

#[test]
fn a_joined_view_is_stale_once_any_of_its_loans_ends() {
    pair(
        |mutant| joined_view(if mutant { Join::EndedLoan } else { Join::Valid }),
        |k| matches!(k, K::StaleView(_)),
    );
}

#[test]
fn a_holder_preserves_a_joined_view_and_rejects_its_expired_or_written_sources() {
    for case in [Join::EndedLoan, Join::WriteSource] {
        pair(
            |mutant| joined_holder(if mutant { case } else { Join::Valid }, true),
            |k| matches!(k, K::OutsideLoan(_)),
        );
    }
}

/// A write to a field of a flat `mut` parameter, by a `mut` call or by
/// storing back what was read: `n` may be rewritten in place; the variable
/// `text` is fixed at construction (MEM-12), since rewriting it would move the
/// cursor of every later field.
fn flat_write(variable: bool, store: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, l::FLAT_ITEM)], None);
    let s = t.slot(l::FLAT_ITEM, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let (field, layout) = if variable {
        (0, l::FLAT_TEXT)
    } else {
        (1, l::U64)
    };
    let at = t.at(ENTRY, s, &[ProjectionStep::Field(field)], &[]);
    let after = t.block(BlockKind::Body, &[]);
    if store {
        let v = t.read(ENTRY, at, sv, ReadMode::Take, None);
        t.store(ENTRY, at, v, s);
        t.jump(ENTRY, after, &[]);
    } else {
        let fail = t.block(BlockKind::FailureCleanup, &[]);
        t.term(fail, Terminator::Propagate);
        let version = t.version_of(s);
        t.call(
            ENTRY,
            &[(Passing::Mut, layout)],
            None,
            &[ArgumentValue::Address {
                address: at,
                version,
            }],
            (after, &[]),
            fail,
        );
    }
    t.ret(after, None);
    t.check()
}

#[test]
fn a_variable_part_of_a_flat_value_is_never_rewritten() {
    for store in [false, true] {
        pair(
            |variable| flat_write(variable, store),
            |k| *k == K::Type("write into a variable part of a flat value"),
        );
    }
}

/// A closure borrowing its parameter into `ENV`'s one i64 field: a borrowed
/// capture fills its environment field as the captured view itself, so the
/// view has the field's layout (a mut capture is an address field instead).
fn borrowed_capture(captured: LayoutId) -> Vec<K> {
    use hirsel_lir::abi_types::CallableShape;
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, captured)], Some(l::CLOSURE));
    let x = t.plain(captured);
    t.param(x);
    let code = t.symbol(SymbolKind::Function);
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    let captures = PoolRange::push(
        &mut t.unit.pools.captures,
        [CaptureValue {
            value: x,
            usage: CaptureUse::Borrow,
        }],
    );
    t.unit.tables.closures.push(ClosureSite {
        code: Some(ClosureCode {
            symbol: code,
            abi: abi,
        }),
        environment: l::ENV,
        captures,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: SourceAnchorId(0),
    });
    let closure = ClosureId(t.unit.tables.closures.len() as u32 - 1);
    let result = t.owned(l::CLOSURE);
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Closure {
            result,
            closure,
            failure,
        },
    );
    t.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_borrowed_capture_has_its_environment_fields_layout() {
    pair(
        |mutant| borrowed_capture(if mutant { l::U64 } else { l::I64 }),
        |k| *k == K::Type("captured value"),
    );
}
