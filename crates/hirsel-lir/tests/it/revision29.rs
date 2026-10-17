//! LIR schema revision 29 (SER-2): `Encode` and `Decode` against their codec
//! plans. Each mutation is paired with its valid twin and is rejected by the
//! rule it names; with that rule removed, C accepts it.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::codec::{CodecNode, CodecNodeId, CodecPlan, DECODE_ERRORS};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::stage::Value;
use support::*;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mutant {
    None,
    /// The bytes node is String, not Bytes.
    Text,
    /// DecodeError's variant lacks a class.
    Classes,
    /// An encode plan with a decode result, or a decode plan without one.
    Direction,
    /// An operand at another layout than the plan's.
    Operand,
    /// A result at another layout than the plan's.
    Result,
}

/// Push `record` onto the fixture table.
fn push(t: &mut Lir<Value>, record: LayoutRecord) -> LayoutId {
    let id = LayoutId(t.table.layouts.len() as u32);
    t.table.layouts.push(record);
    id
}

/// A variant at `size` and `align` with an ordinal tag at 0 and `arms`.
fn variant(t: &mut Lir<Value>, size: u64, align: u32, arms: Vec<Vec<FieldLayout>>) -> LayoutId {
    let mut r = t.table.layouts[l::OPT.0 as usize].clone();
    r.size = SizeSpec::Fixed(size);
    r.align = align;
    r.tag = Some(TagEncoding {
        offset: 0,
        form: TagForm::Ordinal,
        variants: arms
            .into_iter()
            .enumerate()
            .map(|(ordinal, fields)| VariantLayout {
                ordinal: ordinal as u32,
                fields,
            })
            .collect(),
    });
    push(t, r)
}

fn at(offset: u64, layout: LayoutId) -> FieldLayout {
    FieldLayout {
        ordinal: 0,
        offset,
        layout,
        unaligned: false,
    }
}

/// The plan of a u64 root: its scalar, the Bytes slice over a u8 sequence and,
/// decoding, `Result[u64, DecodeError]`. Returns the plan and the result layout.
fn plan(t: &mut Lir<Value>, decode: bool, m: Mutant) -> (CodecPlan, LayoutId) {
    let mut storage = t.table.layouts[l::SEQ.0 as usize].clone();
    storage.kind = LayoutKind::Sequence { element: l::U8 };
    storage.size = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(16),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Constant(1),
        SizeTerm::Multiply,
        SizeTerm::Add,
    ]);
    let storage = push(t, storage);
    let classes = DECODE_ERRORS.len() - usize::from(m == Mutant::Classes);
    let error = variant(t, 4, 4, vec![Vec::new(); classes]);
    let result = variant(t, 16, 8, vec![vec![at(8, l::U64)], vec![at(4, error)]]);
    let mut nodes = vec![
        CodecNode::Scalar(l::U64),
        CodecNode::Text {
            layout: l::SLICE,
            storage,
            utf8: m == Mutant::Text,
        },
    ];
    let decoding = decode != (m == Mutant::Direction);
    if decoding {
        nodes.push(CodecNode::Variant {
            layout: error,
            arms: vec![Vec::new(); classes],
        });
        nodes.push(CodecNode::Variant {
            layout: result,
            arms: vec![vec![CodecNodeId(0)], vec![CodecNodeId(2)]],
        });
    }
    let plan = CodecPlan {
        descriptor: vec![4],
        nodes,
        root: CodecNodeId(0),
        bytes: CodecNodeId(1),
        result: decoding.then_some(CodecNodeId(3)),
    };
    (plan, result)
}

/// `codec.encode(x)` of a borrowed u64 parameter, returning its bytes.
fn encode(m: Mutant) -> Vec<K> {
    let operand = if m == Mutant::Operand { l::I64 } else { l::U64 };
    let produced = if m == Mutant::Result {
        l::BLK
    } else {
        l::SLICE
    };
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, operand)], Some(produced));
    let x = t.plain(operand);
    t.param(x);
    let (plan, _) = plan(&mut t, false, m);
    t.builder.unit.tables.codecs.push(plan);
    let r = t.owned(produced);
    t.builder.push(
        ENTRY,
        Inst::Encode {
            result: r,
            value: x,
            codec: CodecId(0),
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

/// `codec.decode(bytes)` of a borrowed Bytes parameter, returning its result.
fn decode(m: Mutant) -> Vec<K> {
    let operand = if m == Mutant::Operand {
        l::BLK
    } else {
        l::SLICE
    };
    let mut t = Lir::<Value>::new(&[(Passing::Borrow, operand)], None);
    let x = t.plain(operand);
    t.param(x);
    let (plan, result) = plan(&mut t, true, m);
    t.builder.unit.tables.codecs.push(plan);
    let produced = if m == Mutant::Result { l::OPT } else { result };
    let abi = t.abi(&[(Passing::Borrow, operand)], Some(produced));
    t.builder.unit.header.abi = abi;
    let r = t.plain(produced);
    t.builder.push(
        ENTRY,
        Inst::Decode {
            result: r,
            value: x,
            codec: CodecId(0),
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

fn either(case: fn(Mutant) -> Vec<K>, mutant: Mutant) -> impl Fn(bool) -> Vec<K> {
    move |m| case(if m { mutant } else { Mutant::None })
}

#[test]
fn a_codec_plan_agrees_with_the_layout_table() {
    for mutant in [Mutant::Text, Mutant::Direction] {
        pair(either(encode, mutant), |k| *k == K::Type("codec plan"));
    }
    for mutant in [Mutant::Text, Mutant::Classes, Mutant::Direction] {
        pair(either(decode, mutant), |k| *k == K::Type("codec plan"));
    }
}

#[test]
fn an_encode_reads_the_root_and_yields_bytes() {
    for mutant in [Mutant::Operand, Mutant::Result] {
        pair(either(encode, mutant), |k| *k == K::Type("encode"));
    }
}

#[test]
fn a_decode_reads_bytes_and_yields_the_result() {
    for mutant in [Mutant::Operand, Mutant::Result] {
        pair(either(decode, mutant), |k| *k == K::Type("decode"));
    }
}
