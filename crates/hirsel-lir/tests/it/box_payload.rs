//! Independent Box/Payload controls, with the same physical block layout in each twin.
use crate::support;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutKind;
use hirsel_lir::stage::{Rc, Value};
use support::*;

fn payload(boxed: bool, nested: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, l::SEQBLK)], None);
    if nested {
        t.table.layouts[l::SEQBLK.0 as usize].kind = LayoutKind::Box { payload: l::BLK };
        if boxed {
            t.table.layouts[l::BLK.0 as usize].kind = LayoutKind::Box { payload: l::PAY };
        }
    } else if boxed {
        t.table.layouts[l::SEQBLK.0 as usize].kind = LayoutKind::Box { payload: l::SEQ };
    }
    let s = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(s);
    t.param(version);
    let a = t.address(
        AddressBase::Slot(s),
        if nested {
            &[ProjectionStep::Payload, ProjectionStep::Payload]
        } else {
            &[ProjectionStep::Payload]
        },
        if nested { l::PAY } else { l::SEQ },
    );
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Address {
            address: a,
            failure,
        },
    );
    t.ret(ENTRY, None);
    t.check()
}

#[test]
fn payload_requires_box_and_rejects_an_identical_block_kind_list() {
    pair(
        |bad| payload(!bad, false),
        |k| matches!(k, K::AddressType(_)),
    );
}

#[test]
fn nested_payload_steps_each_require_a_box() {
    pair(
        |bad| payload(!bad, true),
        |k| matches!(k, K::AddressType(_)),
    );
}

fn partial_transfer(bad_mask: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::PAIRBLK)], None);
    t.table.layouts[l::PAIRBLK.0 as usize].kind = LayoutKind::Box { payload: l::PAIR };
    let s = t.slot(l::PAIRBLK, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(s);
    t.param(version);
    let field = t.at(
        ENTRY,
        s,
        &[ProjectionStep::Payload, ProjectionStep::Field(0)],
        &[Undo::Drop(s)],
    );
    let taken = t.read(ENTRY, field, version, ReadMode::Take, None);
    t.release_then(ENTRY, taken, &[Undo::Drop(s)]);
    let whole = t.at(ENTRY, s, &[], &[Undo::Drop(s)]);
    let pointer = t.read(ENTRY, whole, version, ReadMode::Take, None);
    t.push(
        ENTRY,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    let words = PoolRange::push(&mut t.unit.pools.mask_words, [2]);
    t.unit.tables.masks.push(FieldMask { fields: 2, words });
    let anchor = t.cleanup_anchor();
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Release {
            value: pointer,
            layout: l::PAIR,
            live: if bad_mask {
                MaskRef::All
            } else {
                MaskRef::Static(MaskId(0))
            },
            anchor,
            failure,
        },
    );
    t.ret(ENTRY, None);
    t.check()
}

#[test]
fn transferring_a_partially_moved_box_preserves_the_exact_release_mask() {
    pair(partial_transfer, |k| matches!(k, K::ReleaseMask(_)));
}
