//! The scalar library instructions of LIR schema revision 6 (`Convert`, `IntArith`, `MulAdd`
//! and the named `Binary` operations) at every width and every edge, three ways: the
//! interpreter's kernels (`hirsel_interp::library`), the heap checker executing the unit, and
//! the emitted code on x86-64 and on AArch64 under qemu. The interpreter writes the expected
//! lines; the heap checker is compared in the ordinary gate and native code by
//! `tools/check-native.sh`.

use crate::support;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use hirsel_heapcheck::{Arg, Config, NoOperations, Observed};
use hirsel_interp::library::{float, integer};
use hirsel_interp::{Integer, Value};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::{Ready, ValidatedUnit};
use hirsel_llvm::{EmitInputs, OptLevel, emit_object, site_code};
use hirsel_syntax::PassingMode;
use support::native::*;
use support::programs::*;
use support::*;

const FLOATS: [ScalarKind; 2] = [ScalarKind::F32, ScalarKind::F64];

#[derive(Clone, Copy, Debug)]
enum Op {
    Convert(ConvertMode),
    /// `int.narrow`: `TryNarrow`, then a SEM-1 `Check` of its flag, as legalization spells it.
    Narrow,
    Arith(ArithMode),
    Binary(BinaryOp),
    MulAdd,
}

/// One instruction exported as `name(operands) -> returned`: its result, or its flag when
/// `flag` (flagged instructions get one unit for each).
#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    op: Op,
    kind: ScalarKind,
    result: ScalarKind,
    flag: bool,
}

impl Case {
    fn arity(&self) -> usize {
        match self.op {
            Op::Convert(_) | Op::Narrow => 1,
            Op::Arith(_) | Op::Binary(_) => 2,
            Op::MulAdd => 3,
        }
    }

    /// The kind of the instruction's flag, when it has one.
    fn flag_kind(&self) -> Option<ScalarKind> {
        match self.op {
            Op::Convert(ConvertMode::TryNarrow) | Op::Arith(ArithMode::Checked) => {
                Some(ScalarKind::Bool)
            }
            Op::Convert(ConvertMode::FloatToI32) => Some(ScalarKind::I32),
            _ => None,
        }
    }

    fn returned(&self) -> ScalarKind {
        if self.flag {
            self.flag_kind().unwrap()
        } else {
            self.result
        }
    }
}

fn width(kind: ScalarKind) -> (u32, bool) {
    use ScalarKind as K;
    match kind {
        K::Bool => (1, false),
        K::U8 => (8, false),
        K::U16 => (16, false),
        K::U32 | K::F32 => (32, false),
        K::U64 | K::F64 => (64, false),
        K::U128 => (128, false),
        K::I8 => (8, true),
        K::I16 => (16, true),
        K::I32 => (32, true),
        K::I64 => (64, true),
        K::I128 => (128, true),
        K::Pointer => panic!("no pointer operands"),
    }
}

fn mask(w: u32) -> u128 {
    if w == 128 { u128::MAX } else { (1 << w) - 1 }
}

fn signed(bits: u128, w: u32) -> i128 {
    ((bits << (128 - w)) as i128) >> (128 - w)
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// Whether every value of `from` is a value of `to` (`int.widen`'s pairs).
fn widens(from: ScalarKind, to: ScalarKind) -> bool {
    let ((a, sa), (b, sb)) = (width(from), width(to));
    match (sa, sb) {
        (true, false) => false,
        (false, true) => b > a,
        _ => b >= a,
    }
}

/// Every conversion between every pair of integer kinds its mode admits, the three additions
/// and three divisions of every integer kind, and every float operation at both widths.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut push = |stem: String, op: Op, kind, result, flagged: bool| {
        cases.push(Case {
            name: leak(stem.clone()),
            op,
            kind,
            result,
            flag: false,
        });
        if flagged {
            cases.push(Case {
                name: leak(format!("{stem}_flag")),
                op,
                kind,
                result,
                flag: true,
            });
        }
    };
    for from in INTEGERS {
        for to in INTEGERS {
            let pair = format!("{}_{}", kind_name(from), kind_name(to));
            if widens(from, to) {
                let op = Op::Convert(ConvertMode::Widen);
                push(format!("widen_{pair}"), op, from, to, false);
            }
            if width(to).0 <= width(from).0 {
                let op = Op::Convert(ConvertMode::Truncate);
                push(format!("truncate_{pair}"), op, from, to, false);
            }
            let op = Op::Convert(ConvertMode::TryNarrow);
            push(format!("try_narrow_{pair}"), op, from, to, true);
            push(format!("narrow_{pair}"), Op::Narrow, from, to, false);
        }
    }
    for kind in INTEGERS {
        let k = kind_name(kind);
        for to in FLOATS {
            push(
                format!("from_int_{k}_{}", kind_name(to)),
                Op::Convert(ConvertMode::IntToFloat),
                kind,
                to,
                false,
            );
        }
        for (name, mode) in [
            ("wrapping_add", ArithMode::Wrapping),
            ("saturating_add", ArithMode::Saturating),
            ("checked_add", ArithMode::Checked),
        ] {
            let flagged = mode == ArithMode::Checked;
            push(format!("{name}_{k}"), Op::Arith(mode), kind, kind, flagged);
        }
        for op in [BinaryOp::DivFloor, BinaryOp::Mod, BinaryOp::DivExact] {
            let name = format!("{}_{k}", op_name(op));
            push(name, Op::Binary(op), kind, kind, false);
        }
    }
    for kind in FLOATS {
        let k = kind_name(kind);
        for op in [
            BinaryOp::Min,
            BinaryOp::Max,
            BinaryOp::MinNumber,
            BinaryOp::MaxNumber,
        ] {
            let name = format!("{}_{k}", op_name(op));
            push(name, Op::Binary(op), kind, kind, false);
        }
        let order = Op::Binary(BinaryOp::TotalOrder);
        push(
            format!("total_order_{k}"),
            order,
            kind,
            ScalarKind::I32,
            false,
        );
        push(format!("mul_add_{k}"), Op::MulAdd, kind, kind, false);
        let to_i32 = Op::Convert(ConvertMode::FloatToI32);
        push(format!("to_i32_{k}"), to_i32, kind, ScalarKind::I32, true);
        let reinterpret = Op::Convert(ConvertMode::Reinterpret);
        let word = unsigned_word(kind);
        push(format!("bits_{k}"), reinterpret, kind, word, false);
        push(format!("from_bits_{k}"), reinterpret, word, kind, false);
    }
    cases
}

fn dedup(values: Vec<u128>) -> Vec<u128> {
    let mut out = Vec::new();
    for v in values {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// The edges of an integer kind as canonical bits: 0, 1, -1, MIN, MAX and their neighbours.
fn integer_edges(kind: ScalarKind) -> Vec<u128> {
    let (w, s) = width(kind);
    let m = mask(w);
    let min = if s { 1u128 << (w - 1) } else { 0 };
    let max = if s { m >> 1 } else { m };
    dedup(vec![
        0,
        1,
        2,
        3,
        7,
        min,
        min + 1,
        max,
        max - 1,
        m,
        m - 1,
        m - 6,
    ])
}

/// The edges of an integer kind and every other integer kind's bounds it holds, each with
/// both neighbours: where conversions out of it change behaviour.
fn integer_bounds(kind: ScalarKind) -> Vec<u128> {
    let (w, s) = width(kind);
    let (lo, hi) = if s {
        (signed(1 << (w - 1), w), (mask(w) >> 1) as i128)
    } else {
        (0, i128::MAX)
    };
    let mut values = integer_edges(kind);
    let mut bound = |v: Option<i128>| {
        for v in [
            v,
            v.and_then(|v| v.checked_sub(1)),
            v.and_then(|v| v.checked_add(1)),
            v.and_then(|v| v.checked_add(3)),
        ] {
            if let Some(v) = v.filter(|v| (lo..=hi).contains(v)) {
                values.push(v as u128 & mask(w));
            }
        }
    };
    // Rust core/src/num/{f32,f64}.rs exact-integer examples: include the
    // significand boundaries and both even tie directions for from_int.
    for bits in [8u32, 16, 24, 32, 53, 64] {
        bound(Some((1i128 << bits) - 1));
        bound(Some((1i128 << (bits - 1)) - 1));
        bound(Some(-(1i128 << (bits - 1))));
    }
    bound(Some(i128::MAX));
    bound(Some(i128::MIN));
    if !s && w == 128 {
        values.extend([1u128 << 127, (1 << 127) + 1, (1 << 127) - 1]);
    }
    dedup(values)
}

fn f32s(values: &[f32]) -> Vec<u128> {
    dedup(values.iter().map(|v| u128::from(v.to_bits())).collect())
}

fn f64s(values: &[f64]) -> Vec<u128> {
    dedup(values.iter().map(|v| u128::from(v.to_bits())).collect())
}

/// Float edges: signed zeros, subnormals, the least normal, the extremes, infinities, NaN,
/// ordinary values of both signs and ties.
fn float_edges(kind: ScalarKind) -> Vec<u128> {
    if kind == ScalarKind::F32 {
        f32s(&[
            0.0,
            -0.0,
            1.0,
            -1.0,
            -1.5,
            3.0,
            0.1,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            -f32::MIN_POSITIVE,
            1e-45,
            -1e-45,
            f32::from_bits(0x007f_ffff),
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ])
    } else {
        f64s(&[
            0.0,
            -0.0,
            1.0,
            -1.0,
            -1.5,
            3.0,
            0.1,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            5e-324,
            -5e-324,
            f64::from_bits(0x000f_ffff_ffff_ffff),
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ])
    }
}

/// `to_i32` operands: the float edges and both sides of each i32 bound, where truncation
/// toward zero decides between `Ok` and `OutOfRange`.
fn float_integers(kind: ScalarKind) -> Vec<u128> {
    let mut values = float_edges(kind);
    if kind == ScalarKind::F32 {
        // f32 has no value strictly between 2147483520 and 2147483648.
        values.extend(f32s(&[
            0.5,
            -0.5,
            -0.99,
            3.9,
            -3.9,
            2147483520.0,
            2147483648.0,
            2147483904.0,
            -2147483648.0,
            -2147483904.0,
            16777217.0,
            1e10,
            -1e10,
        ]));
    } else {
        values.extend(f64s(&[
            0.5,
            -0.5,
            -0.99,
            3.9,
            -3.9,
            2147483647.0,
            2147483647.5,
            2147483647.9999995,
            2147483648.0,
            -2147483648.0,
            -2147483648.5,
            -2147483648.9999995,
            -2147483649.0,
            4294967296.0,
            1e300,
            -1e300,
        ]));
    }
    dedup(values)
}

/// The unsigned word of a float's width: `bits`' result and `from_bits`' operand.
fn unsigned_word(kind: ScalarKind) -> ScalarKind {
    if kind == ScalarKind::F32 {
        ScalarKind::U32
    } else {
        ScalarKind::U64
    }
}

/// `Reinterpret` operands, as float or as word: each IEEE class, both signed zeros and
/// infinities, the subnormal and normal boundaries, and NaNs with the canonical, a negative
/// and a signalling payload (the runtime leaves' former test patterns).
fn ieee_patterns(kind: ScalarKind) -> Vec<u128> {
    if matches!(kind, ScalarKind::F32 | ScalarKind::U32) {
        vec![
            0,
            0x8000_0000,
            1,
            0x007f_ffff,
            0x0080_0000,
            0x3f80_0000,
            0x7f7f_ffff,
            0x7f80_0000,
            0xff80_0000,
            0x7f80_0001,
            0xffc1_2345,
            0x7fc0_0000,
            0xffc0_0000,
            0xffff_ffff,
        ]
    } else {
        vec![
            0,
            0x8000_0000_0000_0000,
            1,
            0x000f_ffff_ffff_ffff,
            0x0010_0000_0000_0000,
            0x3ff0_0000_0000_0000,
            0x7fef_ffff_ffff_ffff,
            0x7ff0_0000_0000_0000,
            0xfff0_0000_0000_0000,
            0x7ff0_0000_0000_0001,
            0xfff8_1234_5678_9000,
            0x7ff8_0000_0000_0000,
            0xfff8_0000_0000_0000,
            0xffff_ffff_ffff_ffff,
        ]
    }
}

/// `mul_add` operands: values whose product needs the unrounded intermediate, the extremes
/// that overflow only when rounded twice, a subnormal, signed zeros, an infinity and NaN.
fn float_fused(kind: ScalarKind) -> Vec<u128> {
    if kind == ScalarKind::F32 {
        f32s(&[
            0.0,
            -0.0,
            0.1,
            10.0,
            -1.0,
            1.0 + f32::EPSILON,
            1.0 - f32::EPSILON / 2.0,
            f32::MAX,
            1e-45,
            f32::INFINITY,
            f32::NAN,
        ])
    } else {
        f64s(&[
            0.0,
            -0.0,
            0.1,
            10.0,
            -1.0,
            1.0 + f64::EPSILON,
            1.0 - f64::EPSILON / 2.0,
            f64::MAX,
            5e-324,
            f64::INFINITY,
            f64::NAN,
        ])
    }
}

/// The operand table a case loops over (every operand position takes every entry) and the
/// table's C name.
fn table(case: &Case) -> (&'static str, Vec<u128>) {
    let name = kind_name(case.kind);
    match case.op {
        Op::Convert(ConvertMode::FloatToI32) => {
            (leak(format!("{name}_integers")), float_integers(case.kind))
        }
        Op::Convert(ConvertMode::Reinterpret) => {
            (leak(format!("{name}_patterns")), ieee_patterns(case.kind))
        }
        Op::Convert(_) | Op::Narrow => (leak(format!("{name}_bounds")), integer_bounds(case.kind)),
        Op::MulAdd => (leak(format!("{name}_fused")), float_fused(case.kind)),
        _ if FLOATS.contains(&case.kind) => (leak(format!("{name}_edges")), float_edges(case.kind)),
        _ => (leak(format!("{name}_edges")), integer_edges(case.kind)),
    }
}

/// Every operand tuple of a case, in the harness's loop order.
fn tuples(case: &Case) -> Vec<Vec<u128>> {
    let values = table(case).1;
    let mut out: Vec<Vec<u128>> = vec![Vec::new()];
    for _ in 0..case.arity() {
        out = out
            .iter()
            .flat_map(|prefix| {
                values.iter().map(move |v| {
                    let mut t = prefix.clone();
                    t.push(*v);
                    t
                })
            })
            .collect();
    }
    out
}

/// Each case as an exported unit taking its operands by value.
fn program(target: &TargetSpec) -> Program {
    let cases = cases();
    let mut schema = Schema::new();
    for case in &cases {
        let a = schema.types.p(primitive(case.kind));
        let r = schema.types.p(primitive(case.returned()));
        // Every kind an instruction defines has a layout, returned or not.
        schema.types.p(primitive(case.result));
        let parameters = vec![(PassingMode::Borrow, a); case.arity()];
        schema.signature(case.name, &parameters, r);
    }
    let table = schema.build(target);
    let manifest = manifest();
    let units = cases
        .iter()
        .map(|case| {
            let mut u = Unit::new(&table, case.name, case.name);
            let operands: Vec<_> = (0..case.arity())
                .map(|_| u.param(table.scalar(case.kind)))
                .collect();
            let result = u.plain(table.scalar(case.result));
            let flag = case.flag_kind().map(|kind| u.plain(table.scalar(kind)));
            match case.op {
                Op::Convert(mode) => u.push(
                    ENTRY,
                    Inst::Convert {
                        result,
                        flag,
                        operand: operands[0],
                        mode,
                    },
                ),
                Op::Narrow => {
                    let fits = u.plain(table.scalar(ScalarKind::Bool));
                    u.push(
                        ENTRY,
                        Inst::Convert {
                            result,
                            flag: Some(fits),
                            operand: operands[0],
                            mode: ConvertMode::TryNarrow,
                        },
                    );
                    let failure = u.fails(Some("SEM-1"));
                    u.push(
                        ENTRY,
                        Inst::Check {
                            condition: fits,
                            failure,
                        },
                    );
                }
                Op::Arith(mode) => u.push(
                    ENTRY,
                    Inst::IntArith {
                        result,
                        flag,
                        op: BinaryOp::Add,
                        mode,
                        left: operands[0],
                        right: operands[1],
                    },
                ),
                Op::Binary(op) => {
                    // The float operations never fail; their edge carries no rule.
                    let rule = (!FLOATS.contains(&case.kind)).then_some("SEM-1");
                    let failure = u.fails(rule);
                    u.push(
                        ENTRY,
                        Inst::Binary {
                            result,
                            op,
                            left: operands[0],
                            right: operands[1],
                            failure,
                        },
                    );
                }
                Op::MulAdd => u.push(
                    ENTRY,
                    Inst::MulAdd {
                        result,
                        left: operands[0],
                        right: operands[1],
                        addend: operands[2],
                    },
                ),
            }
            let returned = if case.flag { flag.unwrap() } else { result };
            u.ret(ENTRY, Some(returned));
            seal(&table, &manifest, u.finish())
        })
        .collect();
    Program {
        table,
        manifest,
        units,
        statics: Vec::new(),
        finalizers: BTreeMap::new(),
    }
}

fn float_value(kind: ScalarKind, bits: u128) -> Value {
    if kind == ScalarKind::F32 {
        Value::F32(bits as u32)
    } else {
        Value::F64(bits as u64)
    }
}

fn float_bits(value: Value) -> u128 {
    match value {
        Value::F32(bits) => u128::from(bits),
        Value::F64(bits) => u128::from(bits),
        other => panic!("not a float: {other:?}"),
    }
}

/// What the interpreter's kernel makes of one case: the instruction's result and flag as
/// canonical bits, or None where the operation fails with SEM-1.
///
/// The interpreter gives no value where `try_narrow` or `checked_add` is `None` or `to_i32`
/// is an error; there the instruction's own definition supplies it (the operand or sum modulo
/// 2^width, and the truncation saturated to i32 with NaN as 0), computed here with Rust's
/// casts.
fn interpreted(case: &Case, operands: &[u128]) -> Option<(u128, Option<u128>)> {
    let integer = |bits: u128| Integer::new(primitive(case.kind), bits).unwrap();
    let (to_width, _) = width(case.result);
    let modulo = |bits: u128| signed(bits, width(case.kind).0) as u128 & mask(to_width);
    match case.op {
        Op::Convert(ConvertMode::IntToFloat) => {
            let value = float::from_int(integer(operands[0]), primitive(case.result)).unwrap();
            Some((float_bits(value), None))
        }
        Op::Convert(ConvertMode::Reinterpret) if FLOATS.contains(&case.kind) => {
            let Value::Integer(bits) = float::bits(&float_value(case.kind, operands[0])).unwrap()
            else {
                panic!("bits of {:?} is not an integer", case.kind)
            };
            Some((bits.bits(), None))
        }
        Op::Convert(ConvertMode::Reinterpret) => {
            let value = float::from_bits(integer(operands[0])).unwrap();
            Some((float_bits(value), None))
        }
        Op::Convert(ConvertMode::Widen) => {
            let v = integer::convert("widen", integer(operands[0]), primitive(case.result));
            Some((v.unwrap().unwrap().bits(), None))
        }
        Op::Convert(ConvertMode::Truncate) => {
            let v = integer::convert("truncate", integer(operands[0]), primitive(case.result));
            Some((v.unwrap().unwrap().bits(), None))
        }
        Op::Convert(ConvertMode::TryNarrow) => {
            let v = integer::convert("try_narrow", integer(operands[0]), primitive(case.result));
            Some(match v.unwrap() {
                Some(v) => (v.bits(), Some(1)),
                None => (modulo(operands[0]), Some(0)),
            })
        }
        Op::Narrow => integer::convert("narrow", integer(operands[0]), primitive(case.result))
            .ok()
            .map(|v| (v.unwrap().bits(), None)),
        Op::Convert(ConvertMode::FloatToI32) => {
            let value = float_value(case.kind, operands[0]);
            let saturated = if case.kind == ScalarKind::F32 {
                f32::from_bits(operands[0] as u32) as i32
            } else {
                f64::from_bits(operands[0] as u64) as i32
            };
            Some(match float::to_i32(&value).unwrap() {
                Ok(v) => {
                    assert_eq!(v, saturated);
                    (u128::from(v as u32), Some(0))
                }
                // FloatIntError: NotFinite = 0, OutOfRange = 1; the status is one more.
                Err(ordinal) => (u128::from(saturated as u32), Some(u128::from(ordinal) + 1)),
            })
        }
        Op::Arith(mode) => {
            let name = match mode {
                ArithMode::Wrapping => "wrapping_add",
                ArithMode::Saturating => "saturating_add",
                ArithMode::Checked => "checked_add",
            };
            let sum = integer::add(name, integer(operands[0]), integer(operands[1])).unwrap();
            Some(match (mode, sum) {
                (ArithMode::Checked, Some(v)) => (v.bits(), Some(0)),
                (ArithMode::Checked, None) => (
                    operands[0].wrapping_add(operands[1]) & mask(to_width),
                    Some(1),
                ),
                (_, v) => (v.unwrap().bits(), None),
            })
        }
        Op::Binary(op @ (BinaryOp::DivFloor | BinaryOp::Mod | BinaryOp::DivExact)) => {
            integer::divide(op_name(op), integer(operands[0]), integer(operands[1]))
                .ok()
                .map(|v| (v.bits(), None))
        }
        Op::Binary(BinaryOp::TotalOrder) => {
            let (a, b) = (
                float_value(case.kind, operands[0]),
                float_value(case.kind, operands[1]),
            );
            Some((u128::from(float::total_order(a, b).unwrap() as u32), None))
        }
        Op::Binary(op) => {
            let (a, b) = (
                float_value(case.kind, operands[0]),
                float_value(case.kind, operands[1]),
            );
            Some((float_bits(float::minmax(op_name(op), a, b).unwrap()), None))
        }
        Op::MulAdd => {
            let [a, b, c] = [0, 1, 2].map(|i| float_value(case.kind, operands[i]));
            Some((float_bits(float::mul_add(a, b, c).unwrap()), None))
        }
    }
}

fn hex(bits: u128) -> String {
    format!("{:016x}{:016x}", (bits >> 64) as u64, bits as u64)
}

/// A returned value as every side prints it: canonical bits, and `nan` for any NaN (the
/// payload is not observable: SEM-2 names one NaN).
fn shown(kind: ScalarKind, bits: u128) -> String {
    let nan = match kind {
        ScalarKind::F32 => f32::from_bits(bits as u32).is_nan(),
        ScalarKind::F64 => f64::from_bits(bits as u64).is_nan(),
        _ => false,
    };
    if nan { "nan".into() } else { hex(bits) }
}

fn failure_line(name: &str) -> String {
    let code = failure_codes().codes["SEM-1"];
    let site = site_code(
        &path(name),
        &revision(name),
        SiteRange { start: 10, end: 20 },
    );
    format!("fail {} {} {site:016x}", code.kind, code.rule)
}

fn line(case: &Case, operands: &[u128], outcome: &str) -> String {
    let operands: Vec<String> = operands.iter().map(|v| hex(*v)).collect();
    format!("{} {} {outcome}\n", case.name, operands.join(" "))
}

/// The interpreter's lines: one per case and operand tuple.
fn expected() -> String {
    let mut out = String::new();
    for case in &cases() {
        for operands in tuples(case) {
            let outcome = match interpreted(case, &operands) {
                Some((value, flag)) => {
                    let bits = if case.flag { flag.unwrap() } else { value };
                    shown(case.returned(), bits)
                }
                None => failure_line(case.name),
            };
            out += &line(case, &operands, &outcome);
        }
    }
    out
}

fn first_difference(actual: &str, expected: &str) -> String {
    actual
        .lines()
        .zip(expected.lines())
        .find(|(a, e)| a != e)
        .map(|(a, e)| format!("got         `{a}`\ninterpreter `{e}`"))
        .unwrap_or_else(|| {
            format!(
                "{} lines against the interpreter's {}",
                actual.lines().count(),
                expected.lines().count()
            )
        })
}

/// The heap checker's lines for one case: its unit run at every operand tuple.
fn heap_lines(program: &Program, case: &Case, unit: &ValidatedUnit<Ready>) -> String {
    let mut actual = String::new();
    // One unit per program: a run validates the whole program it is given.
    let heap = hirsel_heapcheck::Program::<Ready> {
        target: program.table.target.clone(),
        layouts: program.table.table.clone(),
        units: vec![unit.unit().clone()],
        statics: Vec::new(),
    };
    for operands in tuples(case) {
        let args: Vec<Arg> = operands
            .iter()
            .map(|&bits| Arg::Bits {
                scalar: case.kind,
                bits,
            })
            .collect();
        let run = hirsel_heapcheck::run(
            &heap,
            &path(case.name),
            &args,
            &mut NoOperations,
            &Config::default(),
        );
        let observation = run
            .result
            .unwrap_or_else(|v| panic!("{} {operands:x?}: {v:?}", case.name))
            .observation;
        let outcome = if let Some(failure) = observation.failures.first() {
            assert_eq!(failure.rule, "SEM-1", "{}", case.name);
            assert_eq!(failure.site, (10, 20), "{}", case.name);
            failure_line(&failure.definition.0.name)
        } else {
            let bits = match observation.returned {
                Some(Observed::Bool(b)) => u128::from(b),
                Some(Observed::Integer { bits, .. }) => bits,
                Some(Observed::F32(bits)) => u128::from(bits),
                Some(Observed::F64(bits)) => u128::from(bits),
                other => panic!("{}: an unexpected observation {other:?}", case.name),
            };
            shown(case.returned(), bits)
        };
        actual += &line(case, &operands, &outcome);
    }
    actual
}

/// The heap checker executes every case at every operand tuple and returns, or fails with
/// SEM-1 at the instruction's site, exactly as the interpreter's kernels do (SEM-1, SEM-2,
/// docs/spec/core-library.md).
#[test]
fn the_heap_checker_agrees_with_the_interpreter_on_every_scalar_library_operation() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let program = program(&target);
    let cases = cases();
    let work: Vec<(&Case, &ValidatedUnit<Ready>)> = cases.iter().zip(&program.units).collect();
    let threads = std::env::var("HIRSEL_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse::<usize>().ok())
        .filter(|&jobs| jobs > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    let chunk = work.len().div_ceil(threads);
    let program = &program;
    // Cases are independent; each thread's lines join in case order.
    let actual: String = std::thread::scope(|scope| {
        let handles: Vec<_> = work
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut lines = String::new();
                    for &(case, unit) in chunk {
                        lines += &heap_lines(program, case, unit);
                    }
                    lines
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let expected = expected();
    assert!(expected.lines().count() > 20_000, "the matrix shrank");
    assert!(
        actual == expected,
        "the heap checker's first difference:\n{}",
        first_difference(&actual, &expected)
    );
}

fn c_type(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::Bool | ScalarKind::U8 => "uint8_t",
        ScalarKind::U16 => "uint16_t",
        ScalarKind::U32 => "uint32_t",
        ScalarKind::U64 => "uint64_t",
        ScalarKind::U128 => "unsigned __int128",
        ScalarKind::I8 => "int8_t",
        ScalarKind::I16 => "int16_t",
        ScalarKind::I32 => "int32_t",
        ScalarKind::I64 => "int64_t",
        ScalarKind::I128 => "__int128",
        ScalarKind::F32 => "float",
        ScalarKind::F64 => "double",
        ScalarKind::Pointer => "void *",
    }
}

/// The unsigned C type of a kind's width: a cast through it prints canonical bits.
fn c_bits(kind: ScalarKind) -> &'static str {
    match width(kind).0 {
        1 | 8 => "uint8_t",
        16 => "uint16_t",
        32 => "uint32_t",
        64 => "uint64_t",
        _ => "unsigned __int128",
    }
}

const C_PRELUDE: &str = r#"#include <inttypes.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "hirsel_rt_v2.h"

static float f32_of(uint32_t b) { float f; memcpy(&f, &b, 4); return f; }
static double f64_of(uint64_t b) { double f; memcpy(&f, &b, 8); return f; }
static void bits(unsigned __int128 v) { printf("%016" PRIx64 "%016" PRIx64, (uint64_t)(v >> 64), (uint64_t)v); }
static void print_f32(float v) { uint32_t b; memcpy(&b, &v, 4); if (isnan(v)) printf("nan"); else bits(b); }
static void print_f64(double v) { uint64_t b; memcpy(&b, &v, 8); if (isnan(v)) printf("nan"); else bits(b); }
static hirsel_rt_v2_ctx *ctx;
static void outcome(uint8_t status) {
  hirsel_rt_v2_failure primary;
  if (status != 1 || !hirsel_rt_v2_failure_primary(ctx, &primary)) { printf("bad status %u", status); return; }
  printf("fail %u %u %016" PRIx64, primary.kind, primary.rule, primary.site);
  hirsel_rt_v2_failure_clear(ctx);
}
"#;

/// The C harness: per case, nested loops over its operand table calling the emitted entry
/// and printing the line format of `expected`.
fn harness(program: &Program) -> String {
    let mut c = String::from(C_PRELUDE);
    let mut tables: Vec<&str> = Vec::new();
    let cases = cases();
    for case in &cases {
        let (name, values) = table(case);
        if tables.contains(&name) {
            continue;
        }
        tables.push(name);
        let words = |f: fn(u128) -> u64| {
            values
                .iter()
                .map(|v| format!("0x{:016x}ull", f(*v)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            c,
            "static const uint64_t {name}_hi[] = {{{}}};\nstatic const uint64_t {name}_lo[] = {{{}}};\n#define {name}_n {}",
            words(|v| (v >> 64) as u64),
            words(|v| v as u64),
            values.len()
        );
    }
    let mut main = String::from(
        "int main(void) {\n  hirsel_rt_v2_config config;\n  memset(&config, 0, sizeof config);\n  config.abi_version = HIRSEL_RT_V2_ABI_VERSION;\n  if (hirsel_rt_v2_ctx_create(&config, &ctx) != 0) { printf(\"no context\\n\"); return 1; }\n",
    );
    for case in &cases {
        let (arg, result) = (c_type(case.kind), c_type(case.returned()));
        let (name, _) = table(case);
        let indexes = &["i", "j", "k"][..case.arity()];
        c.push_str(&program.c_call(case.name, &format!("call_{}", case.name)));
        let raw = |i: &str| format!("(((unsigned __int128){name}_hi[{i}] << 64) | {name}_lo[{i}])");
        let value = |i: &str| match case.kind {
            ScalarKind::F32 => format!("f32_of((uint32_t){name}_lo[{i}])"),
            ScalarKind::F64 => format!("f64_of({name}_lo[{i}])"),
            _ => format!("({arg}){}", raw(i)),
        };
        let loops: String = indexes
            .iter()
            .map(|i| format!("for (int {i} = 0; {i} < {name}_n; {i}++) "))
            .collect();
        let arguments: String = indexes.iter().map(|i| format!(", {}", value(i))).collect();
        let operands: String = indexes
            .iter()
            .map(|i| format!(" printf(\" \"); bits({});", raw(i)))
            .collect();
        let print = match case.returned() {
            ScalarKind::F32 => "print_f32(r)".to_string(),
            ScalarKind::F64 => "print_f64(r)".to_string(),
            kind => format!("bits(({})r)", c_bits(kind)),
        };
        let call = format!(
            "{result} r = 0;\n    uint8_t s = call_{}(ctx, &r{arguments});",
            case.name
        );
        let _ = writeln!(
            main,
            "  {loops}{{\n    {call}\n    printf(\"{}\");{operands}\n    printf(\" \"); if (s == 0) {print}; else outcome(s); printf(\"\\n\");\n  }}",
            case.name
        );
    }
    main.push_str("  hirsel_rt_v2_ctx_destroy(ctx);\n  return 0;\n}\n");
    c.push_str(&main);
    c
}

/// The emitted instructions, unoptimized and optimized, on x86-64 and on AArch64 under qemu,
/// print the interpreter's line for every case and operand tuple.
#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn every_target_agrees_with_the_interpreter_on_every_scalar_library_operation() {
    let expected = expected();
    let codes = failure_codes();
    let input = input();
    for target in TargetSpec::executed() {
        let program = program(&target);
        let harness = harness(&program);
        for opt in [OptLevel::None, OptLevel::Speed] {
            let dir = scratch(&format!("scalar-library-{}-{opt:?}", target.triple));
            let bytes = {
                let inputs = EmitInputs {
                    input: &input,
                    target: &program.table.target,
                    layouts: &program.table.table,
                    manifest: &program.manifest,
                    failures: &codes,
                    statics: &program.statics,
                    finalizers: &program.finalizers,
                    inline_instances: &std::collections::BTreeSet::new(),
                    available_externally: &std::collections::BTreeSet::new(),
                    registry: hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                        &program.table.table,
                        &program.finalizers,
                    ),
                    opt,
                    measurement: false,
                };
                emit_object(&program.units(), &inputs).unwrap().bytes
            };
            let source = dir.join("harness.c");
            std::fs::write(&source, &harness).unwrap();
            let executable = link(&target, &dir, &[("scalar.o", &bytes)], &[source]);
            let actual = run(&target, &executable);
            assert!(
                actual == expected,
                "{} {opt:?}: first difference:\n{}",
                target.triple,
                first_difference(&actual, &expected)
            );
        }
    }
}
