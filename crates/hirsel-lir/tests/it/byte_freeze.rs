//! A byte owner can be frozen only with its exact, complete initialized interval.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutKind, SizeSpec, SizeTerm};
use hirsel_lir::stage::Value;

fn frozen(mutant: u8) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Take, l::SEQBLK)], Some(l::SLICE));
    if mutant != 1 {
        let payload = &mut t.table.layouts[l::SEQ.0 as usize];
        payload.kind = LayoutKind::Sequence { element: l::U8 };
        payload.size = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(16),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: hirsel_lir::layout_types::ScalarKind::U64,
            },
            SizeTerm::Constant(1),
            SizeTerm::Multiply,
            SizeTerm::Add,
        ]);
    }
    let owner = t.owned(l::SEQBLK);
    t.param(owner);
    let start = t.plain(l::U64);
    let ty = t.builder.unit.values[start.index()].ty;
    let constant = ConstantId(t.builder.unit.tables.constants.len() as u32);
    t.builder.unit.tables.constants.push(ConstantData {
        ty,
        // A whole byte sequence's slice starts at its first element, 8 bytes past the
        // slice data offset (runtime ABI 5); a mutant forges one more.
        value: ConstantValue::Integer(8 + u128::from(mutant == 2)),
    });
    t.builder.push(
        ENTRY,
        Inst::Const {
            result: start,
            constant,
        },
    );
    let length = t.plain(l::U64);
    if mutant == 3 {
        t.builder.push(
            ENTRY,
            Inst::Const {
                result: length,
                constant,
            },
        );
    } else {
        t.builder.push(
            ENTRY,
            Inst::SeqLen {
                result: length,
                value: owner,
            },
        );
    }
    let construct = t.construct_site(
        ConstructorKind::Tuple,
        l::SLICE,
        &[(0, owner), (1, start), (2, length)],
        None,
    );
    let result = t.owned(l::SLICE);
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
fn a_slice_owner_requires_byte_storage_its_whole_start_and_its_own_length() {
    for mutation in 1..=3 {
        pair(
            |bad| frozen(if bad { mutation } else { 0 }),
            |k| *k == K::Type("constructed fields"),
        );
    }
}
