//! Source-free controls for persistent capture live bits. Every rejected
//! mutation has the unchanged accepted fixture as its twin.
use crate::support;
use hirsel_lir::abi_types::{CallMultiplicity, CallableShape};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::stage::Ready;
use support::*;

#[derive(Clone, Copy)]
enum Mutation {
    None,
    MissingStore,
    WrongBit,
    DoubleMove,
    ReadAfterMove,
    ManyBody,
    Rearm,
}

fn capture_take(mutation: Mutation) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    t.table.fn_abis[abi.0 as usize].multiplicity = if matches!(mutation, Mutation::ManyBody) {
        CallMultiplicity::Many
    } else {
        CallMultiplicity::Once
    };
    t.builder.unit.header.abi = abi;
    let mut environment = t.table.layouts[l::PAIR.0 as usize].clone();
    environment.kind = LayoutKind::Environment { movable: vec![0] };
    environment.size = SizeSpec::Fixed(24);
    environment.fields = vec![
        FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: l::ERASED_ONCE,
            unaligned: false,
        },
        FieldLayout {
            ordinal: 1,
            offset: 16,
            layout: l::U64,
            unaligned: false,
        },
    ];
    environment.copy = CopyRecipe::NotCopyable;
    environment.scan = vec![ScanStep::ActiveVariant {
        tag_offset: 16,
        form: TagForm::LiveBit { bit: 0 },
        variants: vec![(1, vec![ScanStep::Environment { offset: 8 }])],
    }];
    environment.drop = vec![DropStep::ActiveVariant {
        tag_offset: 16,
        form: TagForm::LiveBit { bit: 0 },
        variants: vec![(1, vec![DropStep::Environment { offset: 8 }])],
    }];
    let id = LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(environment);
    let capture = t.slot(l::ERASED_ONCE, SlotKind::Capture(0), ROOT);
    let flags = t.slot(l::U64, SlotKind::Capture(1), ROOT);
    t.builder.unit.tables.init_flags.push(InitFlag {
        path: PoolRange::EMPTY,
        value: t.builder.unit.captures[1],
        slot: flags,
        environment: Some(id),
    });
    let at = t.address(AddressBase::Slot(capture), &[], l::ERASED_ONCE);
    let word = t.address(AddressBase::Slot(flags), &[], l::U64);
    t.prepare(ENTRY, at, &[]);
    t.prepare(ENTRY, word, &[]);
    let reaching = t.builder.unit.captures[0];
    let moved = t.read(ENTRY, at, reaching, ReadMode::Take, None);
    let old = t.read(
        ENTRY,
        word,
        t.builder.unit.captures[1],
        ReadMode::Bits,
        None,
    );
    let next = t.plain(l::U64);
    t.builder.push(
        ENTRY,
        Inst::UpdateMask {
            result: next,
            mask: old,
            field: u32::from(matches!(mutation, Mutation::WrongBit)),
            initialized: matches!(mutation, Mutation::Rearm),
        },
    );
    let flag_version = if !matches!(mutation, Mutation::MissingStore) {
        t.store(ENTRY, word, next, flags)
    } else {
        t.builder.unit.captures[1]
    };
    t.release(ENTRY, moved, &[]);
    if matches!(mutation, Mutation::DoubleMove) {
        let value = t.read(ENTRY, at, reaching, ReadMode::Take, None);
        let old = t.read(ENTRY, word, flag_version, ReadMode::Bits, None);
        let next = t.plain(l::U64);
        t.builder.push(
            ENTRY,
            Inst::UpdateMask {
                result: next,
                mask: old,
                field: 0,
                initialized: false,
            },
        );
        t.store(ENTRY, word, next, flags);
        t.release(ENTRY, value, &[]);
    }
    if matches!(mutation, Mutation::ReadAfterMove) {
        let loan = t.loan(at, false);
        t.builder.push(
            ENTRY,
            Inst::BeginLoan {
                loan,
                address: at,
                mutable: false,
                if_inactive: false,
            },
        );
        t.read(ENTRY, at, reaching, ReadMode::Borrow, Some(loan));
        t.builder.push(
            ENTRY,
            Inst::EndLoan {
                loan,
                if_active: false,
            },
        );
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_capture_take_commits_its_clear_to_the_environment() {
    for mutation in [
        Mutation::MissingStore,
        Mutation::WrongBit,
        Mutation::Rearm,
        Mutation::ManyBody,
    ] {
        assert!(capture_take(Mutation::None).is_empty());
        assert!(capture_take(mutation).contains(&K::Type("capture live mask")));
    }
}

#[test]
fn a_capture_cannot_move_twice() {
    assert!(capture_take(Mutation::None).is_empty());
    let rejected = capture_take(Mutation::DoubleMove);
    assert!(
        rejected
            .iter()
            .any(|k| matches!(k, K::Uninitialized(_) | K::MovedPart(_))),
        "{rejected:?}"
    );
}

#[test]
fn a_capture_cannot_be_read_after_it_moves() {
    assert!(capture_take(Mutation::None).is_empty());
    let rejected = capture_take(Mutation::ReadAfterMove);
    assert!(
        rejected
            .iter()
            .any(|k| matches!(k, K::Uninitialized(_) | K::MovedPart(_))),
        "{rejected:?}"
    );
}

#[test]
fn a_stored_live_word_update_is_exactly_one_u64_bit() {
    for (layout, field) in [(l::I64, 0), (l::U64, 64)] {
        pair(
            |mutant| {
                let mut t = Lir::<Ready>::new(&[], None);
                let layout = if mutant { layout } else { l::U64 };
                let word = t.plain(layout);
                let constant = ConstantId(t.builder.unit.tables.constants.len() as u32);
                t.builder.unit.tables.constants.push(ConstantData {
                    ty: t.builder.unit.values[word.index()].ty,
                    value: ConstantValue::Integer(1),
                });
                t.builder.push(
                    ENTRY,
                    Inst::Const {
                        result: word,
                        constant,
                    },
                );
                let next = t.plain(layout);
                t.builder.push(
                    ENTRY,
                    Inst::UpdateMask {
                        result: next,
                        mask: word,
                        field: if mutant { field } else { 0 },
                        initialized: false,
                    },
                );
                t.builder.ret(ENTRY, None);
                t.check()
            },
            |kind| *kind == K::Type("mask update"),
        );
    }
}
