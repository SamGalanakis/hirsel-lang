//! Accepted twins and one-field forgeries of the total byte instructions.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::{ir::*, layout_types::*, stage::Value};

fn source(t: &mut Lir<Value>, layout: LayoutId) -> ValueId {
    let slot = t.slot(layout, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.at(ENTRY, slot, &[], &[]);
    let loan = t.loan(address, false);
    t.begin(ENTRY, loan);
    t.read(ENTRY, address, version, ReadMode::Borrow, Some(loan))
}
fn end_source(t: &mut Lir<Value>, bytes: ValueId) {
    let Holding::Borrowed(loan) = t.builder.unit.values[bytes.index()].holding else {
        unreachable!()
    };
    t.end(ENTRY, loan);
}
fn scalar(mutation: u8) -> Vec<K> {
    let input = if mutation == 1 { l::PAIR } else { l::SLICE };
    let result_layout = if mutation == 3 { l::U32 } else { l::U64 };
    let mut t = Lir::<Value>::new(
        &[
            (Passing::Borrow, input),
            (Passing::Borrow, if mutation == 2 { l::I64 } else { l::U64 }),
        ],
        Some(result_layout),
    );
    let bytes = source(&mut t, input);
    let offset = t.plain(if mutation == 2 { l::I64 } else { l::U64 });
    t.param(offset);
    let result = t.plain(result_layout);
    let valid = t.plain(if mutation == 4 { l::U64 } else { l::BOOL });
    t.builder.push(
        ENTRY,
        Inst::BytesRead {
            result,
            valid,
            bytes,
            offset,
            width: BytesWidth::U64,
            endian: ByteOrder::Little,
        },
    );
    end_source(&mut t, bytes);
    t.builder.ret(ENTRY, Some(result));
    t.check()
}
#[test]
fn scalar_operand_result_and_status_layouts_are_checked() {
    for mutation in 1..=4 {
        pair(
            |bad| scalar(if bad { mutation } else { 0 }),
            |k| *k == K::Type("total byte read"),
        );
    }
}
fn vector(mutation: u8, load: bool, operation: u8) -> Vec<K> {
    let mut t = Lir::<Value>::new(
        &[(Passing::Borrow, l::SLICE), (Passing::Borrow, l::U64)],
        None,
    );
    let bytes = source(&mut t, l::SLICE);
    let offset = t.plain(l::U64);
    t.param(offset);
    let vector = LayoutId(t.table.layouts.len() as u32);
    let mut record = t.table.layouts[l::PAY.0 as usize].clone();
    record.kind = LayoutKind::Simd {
        element: if mutation == 3 { l::U32 } else { l::U8 },
        lanes: 16,
    };
    record.size = SizeSpec::Fixed(if mutation == 3 { 64 } else { 16 });
    record.align = 16;
    record.fields.clear();
    t.table.layouts.push(record);
    let n = match mutation {
        1 => 0,
        2 => 65,
        4 => 17,
        _ => 16,
    };
    let result = t.value_as(ValueRepr::Aggregate, vector, Holding::Plain);
    let status = t.plain(if mutation == 5 { l::U64 } else { l::BOOL });
    if load {
        t.builder.push(
            ENTRY,
            Inst::BytesVectorRead {
                result,
                valid: status,
                bytes,
                offset,
                lanes: n,
            },
        );
    } else {
        // A valid input is first read into its existing Simd layout.
        t.builder.push(
            ENTRY,
            Inst::BytesVectorRead {
                result,
                valid: status,
                bytes,
                offset,
                lanes: 16,
            },
        );
        let output = if operation == 2 {
            t.plain(if mutation == 6 { l::U32 } else { l::U64 })
        } else {
            t.value_as(
                ValueRepr::Aggregate,
                if mutation == 6 { l::PAY } else { vector },
                Holding::Plain,
            )
        };
        let op = match operation {
            0 => ByteVectorOp::Eq {
                left: result,
                right: result,
            },
            1 => ByteVectorOp::Or {
                left: result,
                right: result,
            },
            _ => ByteVectorOp::Bitmask { value: result },
        };
        t.builder.push(
            ENTRY,
            Inst::ByteVector {
                result: output,
                op,
                lanes: n,
            },
        );
    }
    end_source(&mut t, bytes);
    t.builder.ret(ENTRY, None);
    t.check()
}
#[test]
fn vector_rejects_zero_wide_and_mismatched_lanes_layouts_and_status() {
    for mutation in 1..=5 {
        pair(
            |bad| vector(if bad { mutation } else { 0 }, true, 0),
            |k| *k == K::Type("total byte vector read"),
        );
    }
    for op in 0..3 {
        for mutation in [1, 2, 4, 6] {
            pair(
                |bad| vector(if bad { mutation } else { 0 }, false, op),
                |k| *k == K::Type("byte vector operation"),
            );
        }
    }
}
#[test]
fn width_and_endian_are_closed_in_the_wire_schema() {
    assert!(hirsel_codec::from_text::<BytesWidth>("(BytesWidth U128)").is_err());
    assert!(hirsel_codec::from_text::<ByteOrder>("(ByteOrder Native)").is_err());
    for width in [BytesWidth::U16, BytesWidth::U32, BytesWidth::U64] {
        assert_eq!(
            hirsel_codec::from_text::<BytesWidth>(&hirsel_codec::to_text(&width)).unwrap(),
            width
        );
    }
    for endian in [ByteOrder::Little, ByteOrder::Big] {
        assert_eq!(
            hirsel_codec::from_text::<ByteOrder>(&hirsel_codec::to_text(&endian)).unwrap(),
            endian
        );
    }
}
