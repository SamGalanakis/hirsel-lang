//! Scalar semantics: exact-width two's-complement integers with checked
//! arithmetic, and IEEE f32/f64 with no reassociation. A checked failure is a
//! language failure taken along the operation's failure edge. The named
//! library operations (LIR schema revision 6) are computed here on their
//! mathematical definitions (SEM-1, SEM-2, docs/spec/core-library.md), independently of the
//! interpreter and of emission.
use hirsel_lir::ir::{ArithMode, BinaryOp, ConvertMode, ScalarUnaryOp, UnaryOp};
use hirsel_lir::layout_types::ScalarKind;
use std::cmp::Ordering;

/// Bits of an integer kind and whether it is signed.
pub(crate) fn integer(kind: ScalarKind) -> Option<(u32, bool)> {
    use ScalarKind as K;
    Some(match kind {
        K::U8 => (8, false),
        K::U16 => (16, false),
        K::U32 => (32, false),
        K::U64 => (64, false),
        K::U128 => (128, false),
        K::I8 => (8, true),
        K::I16 => (16, true),
        K::I32 => (32, true),
        K::I64 => (64, true),
        K::I128 => (128, true),
        K::Bool | K::F32 | K::F64 | K::Pointer => return None,
    })
}

fn mask(width: u32) -> u128 {
    if width == 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    }
}

/// Canonical bits of `kind`: the low `width` bits, zero above.
pub(crate) fn canonical(kind: ScalarKind, bits: u128) -> u128 {
    match kind {
        ScalarKind::Bool => bits & 1,
        ScalarKind::F32 => bits & u128::from(u32::MAX),
        ScalarKind::F64 => bits & u128::from(u64::MAX),
        ScalarKind::Pointer => bits & u128::from(u64::MAX),
        _ => bits & mask(integer(kind).map_or(128, |(w, _)| w)),
    }
}

fn signed_value(bits: u128, width: u32) -> i128 {
    let shift = 128 - width;
    ((bits << shift) as i128) >> shift
}

/// Whether the operation can fail on this kind (an injection site).
pub(crate) fn binary_may_fail(op: BinaryOp, kind: ScalarKind) -> bool {
    integer(kind).is_some()
        && matches!(
            op,
            BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Rem
                | BinaryOp::Shl
                | BinaryOp::Shr
                | BinaryOp::DivFloor
                | BinaryOp::Mod
                | BinaryOp::DivExact
        )
}

pub(crate) fn unary_may_fail(op: UnaryOp, kind: ScalarKind) -> bool {
    op == UnaryOp::Neg && integer(kind).is_some()
}

/// The checked integer or float result, or None when the operation fails.
/// Comparisons yield a Bool. `Err` is a representation error (wrong kind).
pub(crate) fn binary(
    op: BinaryOp,
    kind: ScalarKind,
    left: u128,
    right_kind: ScalarKind,
    right: u128,
) -> Result<Option<(ScalarKind, u128)>, String> {
    use BinaryOp as B;
    let shift = matches!(op, B::Shl | B::Shr);
    if !shift && right_kind != kind {
        return Err(format!("{op:?} of {kind:?} and {right_kind:?}"));
    }
    let boolean = |b: bool| Ok(Some((ScalarKind::Bool, u128::from(b))));
    match kind {
        ScalarKind::Bool => match op {
            B::Eq => boolean(left == right),
            B::Ne => boolean(left != right),
            B::BitAnd => boolean(left & right & 1 == 1),
            B::BitOr => boolean((left | right) & 1 == 1),
            B::BitXor => boolean((left ^ right) & 1 == 1),
            _ => Err(format!("{op:?} on Bool")),
        },
        ScalarKind::F32 => {
            let (a, b) = (f32::from_bits(left as u32), f32::from_bits(right as u32));
            let bits = |v: f32| Ok(Some((kind, u128::from(v.to_bits()))));
            match op {
                B::Min | B::Max | B::MinNumber | B::MaxNumber => bits(minmax_f32(op, a, b)),
                _ => float(op, a, b, bits, boolean),
            }
        }
        ScalarKind::F64 => {
            let (a, b) = (f64::from_bits(left as u64), f64::from_bits(right as u64));
            let bits = |v: f64| Ok(Some((kind, u128::from(v.to_bits()))));
            match op {
                B::Min | B::Max | B::MinNumber | B::MaxNumber => bits(minmax_f64(op, a, b)),
                _ => float(op, a, b, bits, boolean),
            }
        }
        ScalarKind::Pointer => Err(format!("{op:?} on a pointer")),
        _ => {
            let (width, signed) = integer(kind).expect("integer kind");
            let result = |v: Option<u128>| Ok(v.map(|v| (kind, v & mask(width))));
            if shift {
                let Some((_, amount_signed)) = integer(right_kind) else {
                    return Err(format!("shift amount of kind {right_kind:?}"));
                };
                if amount_signed && signed_value(right, integer(right_kind).unwrap().0) < 0
                    || right >= u128::from(width)
                {
                    return Ok(None);
                }
                let amount = right as u32;
                return match op {
                    B::Shl => result(Some(left << amount)),
                    _ if signed => result(Some((signed_value(left, width) >> amount) as u128)),
                    _ => result(Some(left >> amount)),
                };
            }
            if matches!(op, B::DivFloor | B::Mod | B::DivExact) {
                return result(divide(op, width, signed, left, right));
            }
            if signed {
                let (a, b) = (signed_value(left, width), signed_value(right, width));
                let (min, max) = if width == 128 {
                    (i128::MIN, i128::MAX)
                } else {
                    (-(1i128 << (width - 1)), (1i128 << (width - 1)) - 1)
                };
                let fit =
                    |v: Option<i128>| v.filter(|v| (min..=max).contains(v)).map(|v| v as u128);
                match op {
                    B::Add => result(fit(a.checked_add(b))),
                    B::Sub => result(fit(a.checked_sub(b))),
                    B::Mul => result(fit(a.checked_mul(b))),
                    B::Div => result(if b == 0 { None } else { fit(a.checked_div(b)) }),
                    // SEM-1: signed MIN % -1 is zero at every width, although MIN / -1 fails.
                    B::Rem => result(if b == 0 {
                        None
                    } else {
                        Some(a.wrapping_rem(b) as u128)
                    }),
                    B::BitAnd => result(Some(left & right)),
                    B::BitOr => result(Some(left | right)),
                    B::BitXor => result(Some(left ^ right)),
                    B::Eq => boolean(a == b),
                    B::Ne => boolean(a != b),
                    B::Lt => boolean(a < b),
                    B::Le => boolean(a <= b),
                    B::Gt => boolean(a > b),
                    B::Ge => boolean(a >= b),
                    B::Shl | B::Shr => unreachable!("shifts handled above"),
                    B::DivFloor | B::Mod | B::DivExact => unreachable!("divisions handled above"),
                    B::Min | B::Max | B::MinNumber | B::MaxNumber => {
                        Err(format!("{op:?} on an integer"))
                    }
                }
            } else {
                let fit = |v: Option<u128>| v.filter(|v| *v <= mask(width));
                match op {
                    B::Add => result(fit(left.checked_add(right))),
                    B::Sub => result(fit(left.checked_sub(right))),
                    B::Mul => result(fit(left.checked_mul(right))),
                    B::Div => result(left.checked_div(right)),
                    B::Rem => result(left.checked_rem(right)),
                    B::BitAnd => result(Some(left & right)),
                    B::BitOr => result(Some(left | right)),
                    B::BitXor => result(Some(left ^ right)),
                    B::Eq => boolean(left == right),
                    B::Ne => boolean(left != right),
                    B::Lt => boolean(left < right),
                    B::Le => boolean(left <= right),
                    B::Gt => boolean(left > right),
                    B::Ge => boolean(left >= right),
                    B::Shl | B::Shr => unreachable!("shifts handled above"),
                    B::DivFloor | B::Mod | B::DivExact => unreachable!("divisions handled above"),
                    B::Min | B::Max | B::MinNumber | B::MaxNumber => {
                        Err(format!("{op:?} on an integer"))
                    }
                }
            }
        }
    }
}

type Outcome = Result<Option<(ScalarKind, u128)>, String>;

fn float<F>(
    op: BinaryOp,
    a: F,
    b: F,
    bits: impl Fn(F) -> Outcome,
    boolean: impl Fn(bool) -> Outcome,
) -> Outcome
where
    F: Copy
        + PartialOrd
        + std::ops::Add<Output = F>
        + std::ops::Sub<Output = F>
        + std::ops::Mul<Output = F>
        + std::ops::Div<Output = F>
        + std::ops::Rem<Output = F>,
{
    use BinaryOp as B;
    match op {
        B::Add => bits(a + b),
        B::Sub => bits(a - b),
        B::Mul => bits(a * b),
        B::Div => bits(a / b),
        B::Rem => bits(a % b),
        B::Eq => boolean(a == b),
        B::Ne => boolean(a != b),
        B::Lt => boolean(a < b),
        B::Le => boolean(a <= b),
        B::Gt => boolean(a > b),
        B::Ge => boolean(a >= b),
        B::Shl | B::Shr | B::BitAnd | B::BitOr | B::BitXor | B::DivFloor | B::Mod | B::DivExact => {
            Err(format!("{op:?} on a float"))
        }
        B::Min | B::Max | B::MinNumber | B::MaxNumber => {
            unreachable!("named float operations handled by the caller")
        }
    }
}

/// `div_floor`, `mod` and `div_exact` (docs/spec/core-library.md): the quotient rounded toward
/// negative infinity, its remainder (the divisor's sign), and the exact
/// quotient. A zero divisor, signed `MIN / -1` (for all three) and a nonzero
/// remainder of `div_exact` fail SEM-1 (None).
fn divide(op: BinaryOp, width: u32, signed: bool, left: u128, right: u128) -> Option<u128> {
    if right == 0 {
        return None;
    }
    if !signed {
        return match op {
            BinaryOp::DivFloor => Some(left / right),
            BinaryOp::Mod => Some(left % right),
            _ => (left % right == 0).then(|| left / right),
        };
    }
    let (a, b) = (signed_value(left, width), signed_value(right, width));
    let min = if width == 128 {
        i128::MIN
    } else {
        -(1i128 << (width - 1))
    };
    if a == min && b == -1 {
        return None;
    }
    let (q, r) = (a / b, a % b);
    let adjust = r != 0 && (r < 0) != (b < 0);
    let v = match op {
        BinaryOp::DivFloor => q - i128::from(adjust),
        BinaryOp::Mod => r + if adjust { b } else { 0 },
        _ if r == 0 => q,
        _ => return None,
    };
    Some(v as u128 & mask(width))
}

macro_rules! float_named {
    ($minmax:ident, $t:ty) => {
        /// IEEE 754-2019 minimum/maximum (NaN propagates) and
        /// minimumNumber/maximumNumber (NaN is ignored), with -0 < +0 (SEM-2).
        fn $minmax(op: BinaryOp, a: $t, b: $t) -> $t {
            let number = matches!(op, BinaryOp::MinNumber | BinaryOp::MaxNumber);
            let min = matches!(op, BinaryOp::Min | BinaryOp::MinNumber);
            match (a.is_nan(), b.is_nan()) {
                (true, true) => a,
                (true, false) => {
                    if number {
                        b
                    } else {
                        a
                    }
                }
                (false, true) => {
                    if number {
                        a
                    } else {
                        b
                    }
                }
                (false, false) => {
                    // totalOrder agrees with `<` on non-NaN values and puts -0 first.
                    let less = a.total_cmp(&b) == Ordering::Less;
                    if less == min { a } else { b }
                }
            }
        }
    };
}
float_named!(minmax_f32, f32);
float_named!(minmax_f64, f64);

/// `Convert` of `bits` of kind `from` in `mode` into kind `to`: the result
/// bits and, for `TryNarrow` and `FloatToI32`, the flag. `Err` is a
/// representation error (kinds the mode does not admit).
pub(crate) fn convert(
    mode: ConvertMode,
    from: ScalarKind,
    bits: u128,
    to: ScalarKind,
) -> Result<(u128, Option<(ScalarKind, u128)>), String> {
    let invalid = || Err(format!("{mode:?} of {from:?} into {to:?}"));
    if mode == ConvertMode::IntToFloat {
        let Some((width, signed)) = integer(from) else {
            return invalid();
        };
        let value = match (to, signed) {
            (ScalarKind::F32, true) => u128::from((signed_value(bits, width) as f32).to_bits()),
            (ScalarKind::F32, false) => u128::from((bits as f32).to_bits()),
            (ScalarKind::F64, true) => u128::from((signed_value(bits, width) as f64).to_bits()),
            (ScalarKind::F64, false) => u128::from((bits as f64).to_bits()),
            _ => return invalid(),
        };
        return Ok((value, None));
    }
    if mode == ConvertMode::Reinterpret {
        // SEM-2: every NaN, as operand or result, is the kind's one observable NaN.
        let value = match (from, to) {
            (ScalarKind::F32, ScalarKind::U32) | (ScalarKind::U32, ScalarKind::F32) => {
                let x = f32::from_bits(bits as u32);
                u128::from(if x.is_nan() { f32::NAN } else { x }.to_bits())
            }
            (ScalarKind::F64, ScalarKind::U64) | (ScalarKind::U64, ScalarKind::F64) => {
                let x = f64::from_bits(bits as u64);
                u128::from(if x.is_nan() { f64::NAN } else { x }.to_bits())
            }
            _ => return invalid(),
        };
        return Ok((value, None));
    }
    if mode == ConvertMode::FloatResize {
        // SEM-2: a NaN result is the result kind's one observable NaN.
        let value = match (from, to) {
            (ScalarKind::F32, ScalarKind::F64) => {
                let x = f64::from(f32::from_bits(bits as u32));
                u128::from(if x.is_nan() { f64::NAN } else { x }.to_bits())
            }
            (ScalarKind::F64, ScalarKind::F32) => {
                let x = f64::from_bits(bits as u64) as f32;
                u128::from(if x.is_nan() { f32::NAN } else { x }.to_bits())
            }
            _ => return invalid(),
        };
        return Ok((value, None));
    }
    if mode == ConvertMode::FloatToI32 {
        let x = match from {
            ScalarKind::F32 => f64::from(f32::from_bits(bits as u32)),
            ScalarKind::F64 => f64::from_bits(bits as u64),
            _ => return invalid(),
        };
        if to != ScalarKind::I32 {
            return invalid();
        }
        // Truncation toward zero; NotFinite (1) before OutOfRange (2).
        let status = if !x.is_finite() {
            1
        } else if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&x.trunc()) {
            2
        } else {
            0
        };
        // Rust's float-to-int `as` saturates and maps NaN to 0.
        let value = x as i32;
        return Ok((u128::from(value as u32), Some((ScalarKind::I32, status))));
    }
    let (Some((wi, si)), Some((wj, sj))) = (integer(from), integer(to)) else {
        return invalid();
    };
    let wide = if si {
        signed_value(bits, wi) as u128
    } else {
        bits
    };
    let wrapped = wide & mask(wj);
    let max_j = if sj { mask(wj) >> 1 } else { mask(wj) };
    let fits = if si && signed_value(bits, wi) < 0 {
        let v = signed_value(bits, wi);
        sj && (wj == 128 || v >= -(1i128 << (wj - 1)))
    } else {
        wide <= max_j
    };
    match mode {
        ConvertMode::Widen if fits_every(wi, si, wj, sj) => Ok((wrapped, None)),
        ConvertMode::Truncate if wj <= wi => Ok((wrapped, None)),
        ConvertMode::TryNarrow => Ok((wrapped, Some((ScalarKind::Bool, u128::from(fits))))),
        _ => invalid(),
    }
}

/// Whether every value of the first integer format is representable in the second.
fn fits_every(wi: u32, si: bool, wj: u32, sj: bool) -> bool {
    match (si, sj) {
        (true, false) => false,
        (false, true) => wj > wi,
        _ => wj >= wi,
    }
}

/// `IntArith`: unchecked integer operators or named Add modes; the result bits and,
/// in `Checked` mode, the overflow flag (SEM-1 named behaviours).
pub(crate) fn int_arith(
    op: BinaryOp,
    mode: ArithMode,
    kind: ScalarKind,
    left: u128,
    right: u128,
) -> Result<(u128, Option<(ScalarKind, u128)>), String> {
    let Some((width, signed)) = integer(kind) else {
        return Err(format!("{op:?} of {kind:?}"));
    };
    if mode == ArithMode::Wrapping {
        let value = match op {
            BinaryOp::Add => left.wrapping_add(right),
            BinaryOp::Sub => left.wrapping_sub(right),
            BinaryOp::Mul => left.wrapping_mul(right),
            BinaryOp::Div | BinaryOp::Rem | BinaryOp::Shl | BinaryOp::Shr => {
                // These unchecked forms require the accepted obligation's
                // domain conditions. A violated proof is an H error, never a
                // language failure or a Rust panic. Rem retains MIN % -1 = 0.
                binary(op, kind, left, kind, right)?
                    .ok_or_else(|| format!("invalid unchecked {op:?}"))?
                    .1
            }
            _ => return Err(format!("{op:?} in IntArith")),
        };
        return Ok((value & mask(width), None));
    }
    let wrapped = match (op, mode) {
        (BinaryOp::Add, _) => left.wrapping_add(right),
        (BinaryOp::Sub, _) => left.wrapping_sub(right),
        (BinaryOp::Mul, ArithMode::Checked) => left.wrapping_mul(right),
        _ => return Err(format!("{op:?} in IntArith {mode:?}")),
    } & mask(width);
    let exact = binary(op, kind, left, kind, right)?.map(|(_, v)| v);
    Ok(match mode {
        ArithMode::Wrapping => (wrapped, None),
        ArithMode::Checked => (
            wrapped,
            Some((ScalarKind::Bool, u128::from(exact.is_none()))),
        ),
        ArithMode::Saturating => (
            exact.unwrap_or_else(|| {
                // Signed addition overflows toward the sign both operands share, and
                // subtraction away from the right operand's sign; unsigned subtraction
                // only below zero.
                let negative = signed && signed_value(right, width) < 0;
                let toward_min = if op == BinaryOp::Sub {
                    !negative
                } else {
                    negative
                };
                if !signed {
                    if op == BinaryOp::Sub { 0 } else { mask(width) }
                } else if toward_min {
                    1 << (width - 1)
                } else {
                    mask(width) >> 1
                }
            }),
            None,
        ),
    })
}

/// `ScalarUnary`: the exact float or integer operation of one operand of `kind` (SEM-2:
/// a NaN result reads as the kind's one observable NaN).
pub(crate) fn scalar_unary(
    op: ScalarUnaryOp,
    kind: ScalarKind,
    bits: u128,
) -> Result<u128, String> {
    use ScalarUnaryOp as U;
    let invalid = || Err(format!("{op:?} of {kind:?}"));
    if op.is_float() {
        let f64_op = |x: f64| match op {
            U::Sqrt => x.sqrt(),
            U::Floor => x.floor(),
            U::Ceil => x.ceil(),
            U::Trunc => x.trunc(),
            U::Round => x.round(),
            _ => x.abs(),
        };
        let f32_op = |x: f32| match op {
            U::Sqrt => x.sqrt(),
            U::Floor => x.floor(),
            U::Ceil => x.ceil(),
            U::Trunc => x.trunc(),
            U::Round => x.round(),
            _ => x.abs(),
        };
        return Ok(match kind {
            ScalarKind::F32 => {
                let x = f32_op(f32::from_bits(bits as u32));
                u128::from(if x.is_nan() { f32::NAN } else { x }.to_bits())
            }
            ScalarKind::F64 => {
                let x = f64_op(f64::from_bits(bits as u64));
                u128::from(if x.is_nan() { f64::NAN } else { x }.to_bits())
            }
            _ => return invalid(),
        });
    }
    let Some((width, _)) = integer(kind) else {
        return invalid();
    };
    let word = bits & mask(width);
    // The operand's bits at the top of a u128, so the u128 operations count and reverse
    // exactly the kind's width.
    let high = word << (128 - width);
    Ok(match op {
        U::LeadingZeros => u128::from(high.leading_zeros().min(width)),
        U::TrailingZeros => u128::from(word.trailing_zeros().min(width)),
        U::CountOnes => u128::from(word.count_ones()),
        U::SwapBytes => high.swap_bytes(),
        U::ReverseBits => high.reverse_bits(),
        _ => return invalid(),
    })
}

/// `MulAdd`: `a * b + c` rounded once.
pub(crate) fn mul_add(kind: ScalarKind, a: u128, b: u128, c: u128) -> Result<u128, String> {
    Ok(match kind {
        ScalarKind::F32 => {
            let f = |x: u128| f32::from_bits(x as u32);
            u128::from(f(a).mul_add(f(b), f(c)).to_bits())
        }
        ScalarKind::F64 => {
            let f = |x: u128| f64::from_bits(x as u64);
            u128::from(f(a).mul_add(f(b), f(c)).to_bits())
        }
        _ => return Err(format!("mul_add of {kind:?}")),
    })
}

/// The order of two canonical scalars of one kind, as the interpreter's
/// structural leaves read it: integers by signed or unsigned value, Bool with
/// false first, floats by IEEE (None for an unordered pair, -0 equal to +0).
pub(crate) fn order(kind: ScalarKind, a: u128, b: u128) -> Result<Option<Ordering>, String> {
    Ok(match kind {
        ScalarKind::Bool => Some(a.cmp(&b)),
        ScalarKind::F32 => f32::from_bits(a as u32).partial_cmp(&f32::from_bits(b as u32)),
        ScalarKind::F64 => f64::from_bits(a as u64).partial_cmp(&f64::from_bits(b as u64)),
        ScalarKind::Pointer => return Err("structural order of a raw pointer".into()),
        k => match integer(k) {
            Some((width, true)) => Some(signed_value(a, width).cmp(&signed_value(b, width))),
            _ => Some(a.cmp(&b)),
        },
    })
}

pub(crate) fn unary(op: UnaryOp, kind: ScalarKind, bits: u128) -> Result<Option<u128>, String> {
    match (op, kind) {
        (UnaryOp::Not, ScalarKind::Bool) => Ok(Some((bits & 1) ^ 1)),
        (UnaryOp::Neg, ScalarKind::F32) => Ok(Some(bits ^ (1 << 31))),
        (UnaryOp::Neg, ScalarKind::F64) => Ok(Some(bits ^ (1 << 63))),
        (UnaryOp::BitNot, k) if integer(k).is_some() => {
            Ok(Some(!bits & mask(integer(k).unwrap().0)))
        }
        (UnaryOp::Neg, k) if integer(k).is_some() => {
            let (width, signed) = integer(k).unwrap();
            if !signed {
                return Ok((bits == 0).then_some(0));
            }
            let v = signed_value(bits, width);
            let min = if width == 128 {
                i128::MIN
            } else {
                -(1i128 << (width - 1))
            };
            Ok((v != min).then(|| (-v) as u128 & mask(width)))
        }
        _ => Err(format!("{op:?} on {kind:?}")),
    }
}
