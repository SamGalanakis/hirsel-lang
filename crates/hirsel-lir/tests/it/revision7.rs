//! LIR schema revision 7: the native library's shapes. Each of the seven C rules has
//! its rejected mutation and accepted twin built by one function.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::{Rc, Value};
use support::*;

/// Mark a fixture unit as built by `hirsel_rc::library`.
fn library<S>(t: &mut Lir<S>) {
    t.builder.unit.header.origin = UnitOrigin::Library;
}

/// `SeqLen` (or `SeqCapacity`) of a borrowed parameter of `layout`, into a value of
/// `result`.
fn length(capacity: bool, layout: LayoutId, result: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, layout)], Some(result));
    let x = t.plain(layout);
    t.param(x);
    let r = t.plain(result);
    t.builder.push(
        ENTRY,
        if capacity {
            Inst::SeqCapacity {
                result: r,
                value: x,
            }
        } else {
            Inst::SeqLen {
                result: r,
                value: x,
            }
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

/// Rule 1: the operand is a counted sequence block (or, for `SeqLen`, a slice) and the
/// result a Plain u64.
#[test]
fn a_sequence_length_reads_a_sequence_block_into_a_u64() {
    let bad = |k: &K| *k == K::Type("sequence length");
    pair(
        |m| length(false, if m { l::BLK } else { l::SEQBLK }, l::U64),
        bad,
    );
    pair(
        |m| length(false, l::SEQBLK, if m { l::I64 } else { l::U64 }),
        bad,
    );
    // A slice has a length but no capacity.
    pair(|m| length(m, l::SLICE, l::U64), bad);
    pair(
        |m| length(true, if m { l::SLICE } else { l::SEQBLK }, l::U64),
        bad,
    );
}

/// What a sequence-write fixture varies.
#[derive(Clone, Copy, PartialEq)]
enum Write {
    Valid,
    /// The place holds a block of a record, not of a sequence.
    NotSequence,
    /// The amount is an i64.
    SignedAmount,
    /// The unit was imported from TIR.
    Source,
    /// The written block's slot is a borrowed parameter.
    Borrowed,
    /// A `Store` through a `Slot` step of a borrowed parameter.
    SlotStore,
}

/// `SeqReserve` (or `SeqSetLen`) through a `mut` parameter of a sequence block, by a
/// u64 parameter; with `Write::SlotStore`, a store of an i64 into slot `n` instead.
fn write(set_len: bool, case: Write) -> Vec<K> {
    let layout = if case == Write::NotSequence {
        l::BLK
    } else {
        l::SEQBLK
    };
    let amount = if case == Write::SignedAmount {
        l::I64
    } else {
        l::U64
    };
    let passing = if matches!(case, Write::Borrowed | Write::SlotStore) {
        Passing::Borrow
    } else {
        Passing::Mut
    };
    let mut t = Lir::<Rc>::new(
        &[
            (passing, layout),
            (Passing::Borrow, amount),
            (Passing::Borrow, l::I64),
        ],
        None,
    );
    if case != Write::Source {
        library(&mut t);
    }
    let s = t.slot(layout, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let n = t.plain(amount);
    t.param(n);
    let v = t.plain(l::I64);
    t.param(v);
    let at = t.at(ENTRY, s, &[], &[]);
    if case == Write::SlotStore {
        let element = t.at(ENTRY, s, &[ProjectionStep::Slot(n)], &[]);
        t.store(ENTRY, element, v, s);
    } else {
        let version = t.version_of(s);
        t.builder.push(
            ENTRY,
            if set_len {
                Inst::SeqSetLen {
                    address: at,
                    len: n,
                    version,
                }
            } else {
                Inst::SeqReserve {
                    address: at,
                    capacity: n,
                    version,
                }
            },
        );
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

/// Rule 2: the place holds a counted sequence block and the amount is a Plain u64.
#[test]
fn a_sequence_write_names_a_sequence_block_and_a_u64() {
    let bad = |k: &K| *k == K::Type("sequence write");
    for set_len in [false, true] {
        pair(
            |m| write(set_len, if m { Write::NotSequence } else { Write::Valid }),
            bad,
        );
        pair(
            |m| write(set_len, if m { Write::SignedAmount } else { Write::Valid }),
            bad,
        );
    }
}

/// Rule 5: a `Slot` step, `SeqReserve`, `SeqSetLen` or `Recast` is library vocabulary.
#[test]
fn sequence_writes_and_slot_steps_belong_to_library_units() {
    pair(
        |m| write(false, if m { Write::Source } else { Write::Valid }),
        |k| *k == K::Vocabulary(Opcode::SeqReserve),
    );
    pair(
        |m| write(true, if m { Write::Source } else { Write::Valid }),
        |k| *k == K::Vocabulary(Opcode::SeqSetLen),
    );
    pair(
        |m| recast(m, false),
        |k| *k == K::Vocabulary(Opcode::Recast),
    );
    pair(|m| slot_read(m), |k| *k == K::Vocabulary(Opcode::Address));
}

/// Rule 7: a sequence write, and a store through a `Slot` step, write the block at
/// their place, under the rule of a store through an `Index` step: never through a
/// borrowed parameter's storage.
#[test]
fn sequence_writes_are_writes_of_their_block() {
    for set_len in [false, true] {
        pair(
            |m| write(set_len, if m { Write::Borrowed } else { Write::Valid }),
            |k| matches!(k, K::BorrowedStorage(_)),
        );
    }
    pair(
        |m| {
            if m {
                write(false, Write::SlotStore)
            } else {
                slot_store_mut()
            }
        },
        |k| matches!(k, K::BorrowedStorage(_)),
    );
}

/// A store of an i64 into slot `n` of a `mut` sequence parameter: the twin of
/// `Write::SlotStore`.
fn slot_store_mut() -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[
            (Passing::Mut, l::SEQBLK),
            (Passing::Borrow, l::U64),
            (Passing::Borrow, l::I64),
        ],
        None,
    );
    library(&mut t);
    let s = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let n = t.plain(l::U64);
    t.param(n);
    let v = t.plain(l::I64);
    t.param(v);
    let element = t.at(ENTRY, s, &[ProjectionStep::Slot(n)], &[]);
    t.store(ENTRY, element, v, s);
    t.builder.ret(ENTRY, None);
    t.check()
}

/// A read of slot `n` of a borrowed sequence parameter, in a unit imported from TIR
/// when `source`.
fn slot_read(source: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Borrow, l::SEQBLK), (Passing::Borrow, l::U64)],
        Some(l::I64),
    );
    if !source {
        library(&mut t);
    }
    let x = t.plain(l::SEQBLK);
    t.param(x);
    let n = t.plain(l::U64);
    t.param(n);
    let at = t.through(ENTRY, x, &[ProjectionStep::Slot(n)], &[]);
    let r = t.read(ENTRY, at, x, ReadMode::Bits, None);
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

/// `Hash` of a borrowed parameter of `layout` at `layout`, into a value of `result`.
fn hash(layout: LayoutId, result: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, layout)], Some(result));
    let x = t.plain(layout);
    t.param(x);
    let r = t.plain(result);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Hash {
            result: r,
            value: x,
            layout,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

/// Rule 3: the layout is hashable (no callable, iterator, handle, resource or header
/// reachable) and the result a Plain u64.
#[test]
fn a_hash_reads_a_hashable_value_into_a_u64() {
    let bad = |k: &K| *k == K::Type("hash");
    pair(|m| hash(if m { l::ERASED } else { l::PAY }, l::U64), bad);
    pair(|m| hash(if m { l::RES } else { l::I64 }, l::U64), bad);
    pair(|m| hash(l::SEQBLK, if m { l::I64 } else { l::U64 }), bad);
}

/// `Recast` of a borrowed sequence block to the opaque source, or (`both_sequence`) to
/// the same sequence layout; in a unit imported from TIR when `source`.
fn recast(source: bool, both_sequence: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, l::SEQBLK)], Some(l::U64));
    if !source {
        library(&mut t);
    }
    let s = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let at = t.at(ENTRY, s, &[], &[]);
    let loan = t.loan(at, false);
    t.begin(ENTRY, loan);
    let view = t.read(ENTRY, at, sv, ReadMode::Borrow, Some(loan));
    let to = if both_sequence { l::SEQBLK } else { l::SOURCE };
    let opaque = t.borrowed(to, loan);
    t.builder.push(
        ENTRY,
        Inst::Recast {
            result: opaque,
            value: view,
        },
    );
    let back = t.borrowed(l::SEQBLK, loan);
    t.builder.push(
        ENTRY,
        Inst::Recast {
            result: back,
            value: opaque,
        },
    );
    let n = t.plain(l::U64);
    t.builder.push(
        ENTRY,
        Inst::SeqLen {
            result: n,
            value: back,
        },
    );
    t.end(ENTRY, loan);
    t.builder.ret(ENTRY, Some(n));
    t.check()
}

/// Rule 4: exactly one side of a recast is the opaque source, under the operand's loan.
#[test]
fn a_recast_moves_a_view_to_or_from_the_opaque_source() {
    pair(|m| recast(false, m), |k| *k == K::Type("recast"));
}

/// A closure producing an `Iter[i64]` whose step thunk returns `result`.
fn iterator_closure(result: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::I64)], Some(l::ITER));
    let x = t.plain(l::I64);
    t.param(x);
    let code = t.symbol(SymbolKind::Function);
    let abi = t.abi_shaped(CallableShape::Closure, &[], Some(result));
    let captures = PoolRange::push(
        &mut t.builder.unit.pools.captures,
        [CaptureValue {
            value: x,
            usage: CaptureUse::Borrow,
        }],
    );
    t.builder.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
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
    let closure = ClosureId(t.builder.unit.tables.closures.len() as u32 - 1);
    let iterator = t.owned(l::ITER);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Closure {
            result: iterator,
            closure,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(iterator));
    t.check()
}

/// One step of a borrowed `Iter[i64]` through a thunk returning `result`.
fn iterator_call(result: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::ITER)], Some(result));
    let it = t.plain(l::ITER);
    t.param(it);
    let abi = t.abi_shaped(CallableShape::Closure, &[], Some(result));
    let after = t.builder.block(BlockKind::Body, &[]);
    let fb = t.builder.propagate();
    let r = t
        .call_to(
            ENTRY,
            Callee::Indirect(it),
            abi,
            &[],
            Some(result),
            &[],
            (after, &[]),
            fb,
        )
        .unwrap();
    t.builder.ret(after, Some(r));
    t.check()
}

/// Rule 6: an iterator's thunk takes only its environment and returns `Option` of the
/// iterator's element, at its closure and at every step.
#[test]
fn an_iterator_steps_through_a_thunk_returning_an_option_of_its_element() {
    pair(
        |m| iterator_closure(if m { l::I64 } else { l::OPT }),
        |k| *k == K::Type("iterator closure"),
    );
    pair(
        |m| iterator_call(if m { l::I64 } else { l::OPT }),
        |k| *k == K::Type("iterator call"),
    );
}

fn early_unique(case: u8) -> Vec<K> {
    use hirsel_lir::layout_types::*;
    let mut t = Lir::<Value>::new(
        &[(Passing::Take, l::SEQBLK), (Passing::Borrow, l::U64)],
        Some(l::RESCNT),
    );
    library(&mut t);
    if case == 1 {
        t.builder.unit.header.origin = UnitOrigin::Source;
    }
    let r = &mut t.table.layouts[l::RESCNT.0 as usize];
    let mut finalizer = support::path("owned_cursor_drop");
    finalizer.0.package = hirsel_syntax::PackageId {
        name: "core".into(),
        version: "1".into(),
    };
    finalizer.0.modules = vec!["$native".into()];
    r.finalizer.as_mut().unwrap().definition = finalizer;
    r.size = SizeSpec::Fixed(24);
    r.fields = vec![
        FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: l::SEQBLK,
            unaligned: false,
        },
        FieldLayout {
            ordinal: 1,
            offset: 8,
            layout: l::U64,
            unaligned: false,
        },
        FieldLayout {
            ordinal: 2,
            offset: 16,
            layout: l::BOOL,
            unaligned: false,
        },
    ];
    r.scan = vec![ScanStep::Counted { offset: 0 }];
    r.drop = vec![DropStep::Finalize, DropStep::Release { offset: 0 }];
    let s = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let at = t.at(ENTRY, s, &[], &[Undo::Drop(s)]);
    let loan = if case == 2 {
        let l = t.loan(at, false);
        t.begin(ENTRY, l);
        Some(l)
    } else {
        None
    };
    let source = t.read(
        ENTRY,
        at,
        sv,
        if case == 2 {
            ReadMode::Borrow
        } else {
            ReadMode::Take
        },
        loan,
    );
    let flag = t.plain(l::BOOL);
    t.builder.push(
        ENTRY,
        Inst::IsUnique {
            result: flag,
            value: source,
        },
    );
    if case == 3 {
        let len = t.plain(l::U64);
        t.builder.push(
            ENTRY,
            Inst::SeqLen {
                result: len,
                value: source,
            },
        );
    }
    let zero = t.plain(l::U64);
    t.param(zero);
    let symbol = t.symbol(SymbolKind::Type);
    let mut path = support::path("OwnedCursor");
    path.0.package = hirsel_syntax::PackageId {
        name: "core".into(),
        version: "1".into(),
    };
    path.0.modules = vec!["$intrinsic".into()];
    t.builder.unit.tables.symbols[symbol.index()].definition = path;
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RESCNT,
        &[(0, source), (1, zero), (2, flag)],
        None,
    );
    let cursor = t.owned(l::RESCNT);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: cursor,
            construct,
            failure,
        },
    );
    if let Some(l) = loan {
        t.end(ENTRY, l);
    }
    t.builder.ret(ENTRY, Some(cursor));
    t.check()
}

#[test]
fn a_library_may_test_only_the_owned_source_it_moves_into_a_cursor() {
    pair(
        |m| early_unique(if m { 1 } else { 0 }),
        |k| *k == K::Vocabulary(Opcode::IsUnique),
    );
    for case in [2, 3] {
        pair(
            |m| early_unique(if m { case } else { 0 }),
            |k| *k == K::Type("library uniqueness ownership"),
        );
    }
}

fn moving_recast(foreign: bool, repeated: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Take, l::SEQBLK)], Some(l::SOURCE));
    library(&mut t);
    t.table.layouts[l::SOURCE.0 as usize] = t.table.layouts[l::SEQBLK.0 as usize].clone();
    if foreign {
        t.table.layouts[l::SOURCE.0 as usize] = t.table.layouts[l::BLK.0 as usize].clone();
    }
    let s = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let at = t.at(ENTRY, s, &[], &[Undo::Drop(s)]);
    let source = t.read(ENTRY, at, sv, ReadMode::Take, None);
    let result = t.owned(l::SOURCE);
    t.builder.push(
        ENTRY,
        Inst::Recast {
            result,
            value: source,
        },
    );
    if repeated {
        let len = t.plain(l::U64);
        t.builder.push(
            ENTRY,
            Inst::SeqLen {
                result: len,
                value: source,
            },
        );
    }
    t.builder.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn an_owned_sequence_recast_transfers_its_credit_only_at_the_same_payload() {
    pair(|m| moving_recast(m, false), |k| *k == K::Type("recast"));
    pair(|m| moving_recast(false, m), |k| matches!(k, K::NoCredit(_)));
}

fn live_resource_slot_take(source_field: bool, borrowed: bool) -> Vec<K> {
    let passing = if borrowed {
        Passing::Borrow
    } else {
        Passing::Mut
    };
    let mut t = Lir::<Value>::new(
        &[(passing, l::RESCNT), (Passing::Borrow, l::U64)],
        Some(if source_field { l::SEQBLK } else { l::I64 }),
    );
    library(&mut t);
    t.table.layouts[l::RESCNT.0 as usize].fields[1].layout = l::SEQBLK;
    let s = t.slot(l::RESCNT, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(s);
    t.param(version);
    let index = t.plain(l::U64);
    t.param(index);
    let mut steps = vec![ProjectionStep::Field(1)];
    if !source_field {
        steps.push(ProjectionStep::Slot(index));
    }
    let at = t.at(ENTRY, s, &steps, &[]);
    let result = t.read(ENTRY, at, version, ReadMode::Take, None);
    t.builder.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_live_cursor_may_move_an_element_but_keeps_its_inline_source_field() {
    pair(
        |m| live_resource_slot_take(m, false),
        |k| matches!(k, K::FinalizeState(_)),
    );
    pair(
        |m| live_resource_slot_take(false, m),
        |k| matches!(k, K::BorrowedStorage(_)),
    );
}

/// A failure cleanup may drain through the same temporary on every iteration.
/// The other mutations preserve the old-content and same-incarnation ordering guards.
fn cleanup_slot_incarnation(case: u8) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::BOOL)], None);
    let again = t.plain(l::BOOL);
    t.param(again);
    let s = t.slot(l::BLK, SlotKind::Temporary, ROOT);
    let other = t.slot(l::BLK, SlotKind::Temporary, ROOT);
    let a = t.at(ENTRY, s, &[], &[]);
    let oa = t.at(ENTRY, other, &[], &[]);
    let head = t.builder.block(BlockKind::FailureCleanup, &[]);
    let stop = t.builder.propagate();
    // A failing address operation enters the cleanup loop; no owned state yet.
    let trigger = t.address(AddressBase::Slot(s), &[], l::BLK);
    let failure = t.failure(head);
    t.builder.push(
        ENTRY,
        Inst::Address {
            address: trigger,
            failure,
        },
    );
    t.builder.ret(ENTRY, None);
    t.builder.push(head, Inst::StorageLive { slot: s });
    let x = t.new_block(head, &[]);
    t.store(head, a, x, s);
    if case == 1 {
        t.builder.push(head, Inst::StorageLive { slot: other });
        let y = t.new_block(head, &[Undo::Drop(s)]);
        t.store(head, oa, y, other);
    }
    let retire = t.builder.block(BlockKind::FailureCleanup, &[]);
    if case != 2 {
        let failure = if case == 1 {
            let rest = t.cleanup(&[Undo::Drop(other)]);
            t.failure(rest)
        } else {
            t.failure(retire)
        };
        let anchor = t.cleanup_anchor();
        t.builder.push(
            head,
            Inst::Drop {
                address: a,
                if_live: false,
                anchor,
                failure,
            },
        );
        if case == 1 {
            let failure = t.failure(retire);
            let anchor = t.cleanup_anchor();
            t.builder.push(
                head,
                Inst::Drop {
                    address: oa,
                    if_live: false,
                    anchor,
                    failure,
                },
            );
            t.builder.push(
                head,
                Inst::StorageDead {
                    slot: other,
                    if_live: false,
                },
            );
        }
        t.builder.jump(head, retire, &[]);
        t.builder.push(
            retire,
            Inst::StorageDead {
                slot: s,
                if_live: false,
            },
        );
    } else {
        t.builder.jump(head, retire, &[]);
    }
    let yes = t.builder.edge(head, &[]);
    let no = t.builder.edge(stop, &[]);
    t.builder.term(
        retire,
        Terminator::Branch {
            condition: again,
            yes,
            no,
        },
    );
    t.check()
}

#[test]
fn failure_cleanup_reuses_slots_without_reusing_their_release_order() {
    let valid = cleanup_slot_incarnation(0);
    assert!(valid.is_empty(), "{valid:?}");
    assert!(
        cleanup_slot_incarnation(1)
            .iter()
            .any(|k| matches!(k, K::CleanupOrder(_)))
    );
    assert!(
        cleanup_slot_incarnation(2)
            .iter()
            .any(|k| matches!(k, K::LeakedStorage(_)))
    );
}

fn inline_constant_moves(case: u8) -> Vec<K> {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, LayoutKind, SizeSpec};
    let mut t = Lir::<Rc>::new(&[], None);
    library(&mut t);
    let inline = LayoutId(t.table.layouts.len() as u32);
    let mut r = t.table.layouts[l::PAIR.0 as usize].clone();
    r.kind = LayoutKind::Inline {
        element: l::RES,
        count: 3,
    };
    r.size = SizeSpec::Fixed(24);
    r.fields.clear();
    r.scan.clear();
    r.drop = vec![DropStep::RepeatReverse {
        offset: 0,
        count: 3,
        stride: 8,
        element: l::RES,
    }];
    r.copy = CopyRecipe::NotCopyable;
    t.table.layouts.push(r);
    t.builder.unit.header.abi = t.abi(&[(Passing::Take, inline)], None);
    let slot = t.slot(inline, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let mut addresses = Vec::new();
    for i in 0..3 {
        let index = t.plain(l::U64);
        let ty = t.builder.unit.values[index.index()].ty;
        let constant = ConstantId(t.builder.unit.tables.constants.len() as u32);
        t.builder.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Integer(if case == 1 && i == 2 { 1 } else { i }),
        });
        t.builder.push(
            ENTRY,
            Inst::Const {
                result: index,
                constant,
            },
        );
        let address = t.address(
            AddressBase::Slot(slot),
            &[ProjectionStep::Index(index)],
            l::RES,
        );
        t.prepare_undo(ENTRY, address, &[Undo::Drop(slot)]);
        addresses.push(address);
    }
    for &address in addresses.iter().take(if case == 2 { 2 } else { 3 }) {
        let value = t.read(ENTRY, address, version, ReadMode::Take, None);
        t.builder.push(ENTRY, Inst::Forget { value });
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn constant_inline_positions_have_distinct_move_state() {
    let valid = inline_constant_moves(0);
    assert!(valid.is_empty(), "{valid:?}");
    assert!(
        inline_constant_moves(1)
            .iter()
            .any(|k| matches!(k, K::MovedPart(_)))
    );
    assert!(
        inline_constant_moves(2)
            .iter()
            .any(|k| matches!(k, K::LeakedStorage(_)))
    );
}

/// Hand-built borrowed-holder return: the record scans its block field as
/// uncounted, owns no credit, and must stay under the source loan.
fn borrowed_holder_return(mutation: u8) -> Vec<K> {
    use hirsel_lir::layout_types::{CopyRecipe, FieldLayout, ScanStep, SizeSpec};
    let mut t = Lir::<Rc>::new(
        &[(Passing::Borrow, l::BLK), (Passing::Borrow, l::BLK)],
        None,
    );
    library(&mut t);
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
    let holder = LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(record);
    let abi = t.abi(
        &[(Passing::Borrow, l::BLK), (Passing::Borrow, l::BLK)],
        Some(holder),
    );
    t.table.fn_abis[abi.0 as usize]
        .result
        .as_mut()
        .unwrap()
        .borrowed_parameters = vec![0];
    t.builder.unit.header.abi = abi;
    let mut parameters = Vec::new();
    for ordinal in 0..2 {
        let slot = t.slot(l::BLK, SlotKind::Parameter(ordinal), ROOT);
        let version = t.version_of(slot);
        t.param(version);
        parameters.push((slot, version));
    }
    let (slot, version) = if mutation == 3 {
        let value = t.new_block(ENTRY, &[]);
        let slot = t.slot(l::BLK, SlotKind::Local, ROOT);
        t.builder.push(ENTRY, Inst::StorageLive { slot });
        let at = t.at(ENTRY, slot, &[], &[Undo::Release(value)]);
        let version = t.store(ENTRY, at, value, slot);
        (slot, version)
    } else {
        parameters[usize::from(mutation == 1)]
    };
    let at = t.at(ENTRY, slot, &[], &[]);
    let loan = t.loan(at, false);
    t.begin(ENTRY, loan);
    let source = t.read(ENTRY, at, version, ReadMode::Borrow, Some(loan));
    let symbol = t.symbol(SymbolKind::Type);
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        holder,
        &[(0, source)],
        t.heap(),
    );
    let value = t.plain(holder);
    let cleanup = t.cleanup(&[Undo::End(loan)]);
    let failure = t.failure(cleanup);
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: value,
            construct,
            failure,
        },
    );
    if mutation >= 8 {
        if mutation >= 9 {
            t.end(ENTRY, loan);
        }
        let forwarded = t.plain(holder);
        let next = t.builder.block(BlockKind::Body, &[forwarded]);
        t.builder.jump(ENTRY, next, &[value]);
        if mutation == 10 {
            t.begin(next, loan);
        }
        t.builder.ret(next, Some(forwarded));
        return t.check();
    }
    if mutation == 2 {
        t.end(ENTRY, loan);
    }
    if mutation >= 4 {
        t.builder.unit.header.abi = t.abi(
            &[(Passing::Borrow, l::BLK), (Passing::Borrow, l::BLK)],
            None,
        );
        let cursor = t.slot(holder, SlotKind::Local, ROOT);
        t.builder.push(ENTRY, Inst::StorageLive { slot: cursor });
        let at = t.address(AddressBase::Slot(cursor), &[], holder);
        t.prepare_undo(ENTRY, at, &[Undo::End(loan)]);
        let stored = t.store(ENTRY, at, value, cursor);
        if mutation >= 6 {
            t.end(ENTRY, loan);
            let joined = t.version_of(cursor);
            let next = t.builder.block(BlockKind::Body, &[joined]);
            t.builder.jump(ENTRY, next, &[stored]);
            if mutation == 7 {
                let cursor_loan = t.loan(at, false);
                t.begin(next, cursor_loan);
                t.read(next, at, joined, ReadMode::Borrow, None);
                t.end(next, cursor_loan);
            }
            t.builder.push(
                next,
                Inst::StorageDead {
                    slot: cursor,
                    if_live: false,
                },
            );
            t.builder.ret(next, None);
            return t.check();
        }
        if mutation == 5 {
            t.end(ENTRY, loan);
        }
        let abi = t.abi(&[(Passing::Mut, holder)], None);
        let version = t.version_of(cursor);
        let normal = t.builder.block(BlockKind::Body, &[]);
        let failed = if mutation == 5 {
            t.builder.propagate()
        } else {
            t.cleanup(&[Undo::End(loan)])
        };
        let symbol = t.symbol(SymbolKind::Function);
        t.call_to(
            ENTRY,
            Callee::Direct(symbol),
            abi,
            &[(Passing::Mut, holder)],
            None,
            &[ArgumentValue::Address {
                address: at,
                version,
            }],
            (normal, &[]),
            failed,
        );
        if mutation != 5 {
            t.end(normal, loan);
        }
        t.builder.push(
            normal,
            Inst::StorageDead {
                slot: cursor,
                if_live: false,
            },
        );
        t.builder.ret(normal, None);
        return t.check();
    }
    t.builder.ret(ENTRY, Some(value));
    t.check()
}

#[test]
fn borrowed_holder_return_declares_its_parameter_and_keeps_its_loan() {
    pair(
        |m| borrowed_holder_return(u8::from(m)),
        |k| matches!(k, K::BorrowedReturn(_)),
    );
    pair(
        |m| borrowed_holder_return(if m { 2 } else { 0 }),
        |k| matches!(k, K::OutsideLoan(_)),
    );
    pair(
        |m| borrowed_holder_return(if m { 3 } else { 0 }),
        |k| matches!(k, K::BorrowedReturn(_)),
    );
    pair(
        |m| borrowed_holder_return(if m { 5 } else { 4 }),
        |k| matches!(k, K::OutsideLoan(_)),
    );
    pair(
        |m| borrowed_holder_return(if m { 7 } else { 6 }),
        |k| matches!(k, K::OutsideLoan(_)),
    );
}

/// Passive edge transport preserves an expired view, including across a new
/// epoch of the same loan. Neither a query nor a return can revive that view.
fn forwarded_expired_view(read: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::BLK)], None);
    let slot = t.slot(l::BLK, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let at = t.at(ENTRY, slot, &[], &[]);
    let loan = t.loan(at, false);
    t.begin(ENTRY, loan);
    let source = t.read(ENTRY, at, version, ReadMode::Borrow, Some(loan));
    t.end(ENTRY, loan);
    let forwarded = t.borrowed(l::BLK, loan);
    let next = t.builder.block(BlockKind::Body, &[forwarded]);
    t.builder.jump(ENTRY, next, &[source]);
    t.begin(next, loan);
    if read {
        let result = t.plain(l::BOOL);
        t.builder.push(
            next,
            Inst::IsUnique {
                result,
                value: forwarded,
            },
        );
    }
    t.end(next, loan);
    t.builder.ret(next, None);
    t.check()
}

#[test]
fn forwarding_an_expired_view_is_not_a_read_and_preserves_its_invalidity() {
    pair(forwarded_expired_view, |k| {
        matches!(k, K::OutsideLoan(_) | K::StaleView(_))
    });
    for expired in [9, 10] {
        pair(
            |bad| borrowed_holder_return(if bad { expired } else { 8 }),
            |k| matches!(k, K::OutsideLoan(_) | K::StaleView(_)),
        );
    }
}

#[test]
fn an_expired_join_cannot_revive_when_forwarded_to_itself() {
    pair(
        |expired| {
            let mut t = Lir::<Rc>::new(
                &[(Passing::Borrow, l::BLK), (Passing::Borrow, l::BLK)],
                None,
            );
            let mut sources = Vec::new();
            let mut loans = Vec::new();
            for ordinal in 0..2 {
                let slot = t.slot(l::BLK, SlotKind::Parameter(ordinal), ROOT);
                let version = t.version_of(slot);
                t.param(version);
                let undo: Vec<_> = loans.iter().copied().map(Undo::End).collect();
                let at = t.at(ENTRY, slot, &[], &undo);
                let loan = t.loan(at, false);
                t.begin(ENTRY, loan);
                sources.push(t.read(ENTRY, at, version, ReadMode::Borrow, Some(loan)));
                loans.push(loan);
            }
            let joined = t.borrowed(l::BLK, loans[0]);
            let repeat = t.builder.block(BlockKind::Body, &[joined]);
            t.branch(ENTRY, (repeat, &[sources[0]]), (repeat, &[sources[1]]));
            let result = t.plain(l::BOOL);
            t.builder.push(
                repeat,
                Inst::IsUnique {
                    result,
                    value: joined,
                },
            );
            if expired {
                t.end(repeat, loans[0]);
                t.begin(repeat, loans[0]);
            }
            t.builder.jump(repeat, repeat, &[joined]);
            t.check()
        },
        |k| matches!(k, K::StaleView(_)),
    );
}

fn map_cursor_uniqueness(case: u8) -> Vec<K> {
    use hirsel_lir::layout_types::*;
    let passing = if case == 2 {
        Passing::Borrow
    } else {
        Passing::Take
    };
    let mut t = Lir::<Value>::new(&[(passing, l::PAIRBLK)], Some(l::RESCNT));
    library(&mut t);
    if case == 5 {
        t.builder.unit.header.origin = UnitOrigin::Source;
    }
    let core = |module: Option<&str>, name: &str| {
        let mut p = support::path(name);
        p.0.package = hirsel_syntax::PackageId {
            name: "core".into(),
            version: "1".into(),
        };
        p.0.modules = module.into_iter().map(str::to_owned).collect();
        p
    };
    let map = &mut t.table.layouts[l::PAIR.0 as usize];
    map.kind = LayoutKind::Opaque {
        owner: core(None, "Map"),
    };
    for field in &mut map.fields {
        field.layout = l::SEQBLK;
    }
    let cursor = &mut t.table.layouts[l::RESCNT.0 as usize];
    cursor.finalizer.as_mut().unwrap().definition = core(Some("$native"), "owned_map_cursor_drop");
    cursor.size = SizeSpec::Fixed(24);
    cursor.fields = vec![
        FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: l::PAIRBLK,
            unaligned: false,
        },
        FieldLayout {
            ordinal: 1,
            offset: 8,
            layout: l::U64,
            unaligned: false,
        },
        FieldLayout {
            ordinal: 2,
            offset: 16,
            layout: l::BOOL,
            unaligned: false,
        },
    ];
    cursor.scan = vec![ScanStep::Counted { offset: 0 }];
    cursor.drop = vec![DropStep::Finalize, DropStep::Release { offset: 0 }];
    let slot = t.slot(l::PAIRBLK, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let root = t.at(ENTRY, slot, &[], &[Undo::Drop(slot)]);
    let entries = t.at(
        ENTRY,
        slot,
        &[ProjectionStep::Field(if case == 3 { 1 } else { 0 })],
        &[Undo::Drop(slot)],
    );
    let mut flags = Vec::new();
    for address in [root, entries] {
        let loan = t.loan(address, false);
        t.begin(ENTRY, loan);
        let view = t.read(ENTRY, address, version, ReadMode::Borrow, Some(loan));
        let flag = t.plain(l::BOOL);
        t.builder.push(
            ENTRY,
            Inst::IsUnique {
                result: flag,
                value: view,
            },
        );
        t.end(ENTRY, loan);
        flags.push(flag);
    }
    let flag = t.plain(l::BOOL);
    let cleanup = t.cleanup(&[Undo::Drop(slot)]);
    let failure = t.failure(cleanup);
    t.builder.push(
        ENTRY,
        Inst::Binary {
            result: flag,
            op: if case == 1 {
                BinaryOp::BitOr
            } else {
                BinaryOp::BitAnd
            },
            left: flags[0],
            right: flags[1],
            failure,
        },
    );
    if case == 4 {
        let extra = t.plain(l::BOOL);
        let failure = t.failure(cleanup);
        t.builder.push(
            ENTRY,
            Inst::Binary {
                result: extra,
                op: BinaryOp::Eq,
                left: flags[0],
                right: flags[0],
                failure,
            },
        );
    }
    let source = t.read(ENTRY, root, version, ReadMode::Take, None);
    let zero = t.plain(l::U64);
    let constant = ConstantId(t.builder.unit.tables.constants.len() as u32);
    t.builder.unit.tables.constants.push(ConstantData {
        ty: t.builder.unit.values[zero.index()].ty,
        value: ConstantValue::Integer(0),
    });
    t.builder.push(
        ENTRY,
        Inst::Const {
            result: zero,
            constant,
        },
    );
    let symbol = t.symbol(SymbolKind::Type);
    t.builder.unit.tables.symbols[symbol.index()].definition =
        core(Some("$intrinsic"), "OwnedMapCursor");
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RESCNT,
        &[(0, source), (1, zero), (2, flag)],
        None,
    );
    let result = t.owned(l::RESCNT);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result,
            construct,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_map_cursor_tests_exactly_its_owned_root_and_entries() {
    for case in 1..5 {
        pair(
            |mutant| map_cursor_uniqueness(if mutant { case } else { 0 }),
            |k| *k == K::Type("library uniqueness ownership"),
        );
    }
    pair(
        |mutant| map_cursor_uniqueness(if mutant { 5 } else { 0 }),
        |k| *k == K::Vocabulary(Opcode::IsUnique),
    );
}
