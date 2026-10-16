//! Scalar operations as LLVM instructions (SEM-1, SEM-2).
//!
//! Integer `+ - *` and unary `-` use the overflow intrinsics; division, remainder and shifts
//! are guarded before the instruction runs, so no path reaches poison or undefined behaviour.
//! Each fallible operation calls `guard` with an `i1` that is true exactly when the operation
//! fails; the caller branches to the failure edge there and continues on the other arm. Float
//! operations are single IEEE instructions without fast-math flags; `mul_add` is the only
//! fused operation.
//!
//! The named library operations (LIR schema revision 6) are native too: `div_floor`, `mod`
//! and `div_exact` divide by a divisor made safe before the one guard, conversions are
//! `trunc`/`sext`/`zext` with a range check in the operand's own domain, `float.to_i32` is
//! `llvm.fptosi.sat` with its status from compares, wrapping/saturating/checked addition are
//! `add`, `llvm.{s,u}add.sat` and `llvm.{s,u}add.with.overflow`, `min`/`max` are
//! `llvm.minimum`/`llvm.maximum`, `min_number`/`max_number` select the
//! other operand of a NaN around them (IEEE 754-2019, -0 < +0), `total_order` compares the
//! sign-magnitude keys of NaN-canonical bits, and `mul_add` is `llvm.fma`.

use hirsel_lir::ir::{ArithMode, BinaryOp, ConvertMode, UnaryOp};
use hirsel_lir::layout_types::ScalarKind;

use crate::context::{BinOp, Builder, Cast, FloatPredicate, IntPredicate, Type, Value};

/// Width and signedness of an integer kind.
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

/// The SSA type of a scalar: `i1` for Bool, the exact integer width, IEEE floats, `ptr`.
pub(crate) fn scalar_type<'m>(b: &Builder<'m>, kind: ScalarKind) -> Type<'m> {
    let module = b.module();
    match kind {
        ScalarKind::Bool => module.int_type(1),
        ScalarKind::F32 => module.f32_type(),
        ScalarKind::F64 => module.f64_type(),
        ScalarKind::Pointer => module.ptr_type(),
        _ => module.int_type(integer(kind).expect("an integer kind").0),
    }
}

/// Emits `guard(failed)`: the caller branches to the failure edge when `failed` is true and
/// leaves the builder on the continuing arm.
pub(crate) type Guard<'g, 'm> = &'g mut dyn FnMut(&Builder<'m>, Value<'m>);

/// The scalar operation a LIR operator names on its operand kinds, or why it has none.
fn unsupported(what: impl Into<String>) -> Result<Value<'static>, String> {
    Err(what.into())
}

fn overflow<'m>(
    b: &Builder<'m>,
    name: &str,
    ty: Type<'m>,
    left: Value<'m>,
    right: Value<'m>,
    guard: Guard<'_, 'm>,
) -> Result<Value<'m>, String> {
    let intrinsic = b.module().intrinsic(name, &[ty])?;
    let pair = b.call(intrinsic, &[left, right]);
    let result = b.extract(pair, 0);
    guard(b, b.extract(pair, 1));
    Ok(result)
}

/// `left op right` on two operands of `kind`; comparisons yield `i1`. A shift count has the
/// left operand's type (SEM-1).
pub(crate) fn binary<'m>(
    b: &Builder<'m>,
    op: BinaryOp,
    kind: ScalarKind,
    left: Value<'m>,
    right: Value<'m>,
    guard: Guard<'_, 'm>,
) -> Result<Value<'m>, String> {
    use BinaryOp as B;
    match kind {
        ScalarKind::Bool => Ok(match op {
            B::Eq => b.icmp(IntPredicate::Eq, left, right),
            B::Ne => b.icmp(IntPredicate::Ne, left, right),
            B::BitAnd => b.binary(BinOp::And, left, right),
            B::BitOr => b.binary(BinOp::Or, left, right),
            B::BitXor => b.binary(BinOp::Xor, left, right),
            _ => return unsupported(format!("{op:?} on Bool")),
        }),
        ScalarKind::F32 | ScalarKind::F64 => Ok(match op {
            B::Add => b.binary(BinOp::FAdd, left, right),
            B::Sub => b.binary(BinOp::FSub, left, right),
            B::Mul => b.binary(BinOp::FMul, left, right),
            B::Div => b.binary(BinOp::FDiv, left, right),
            B::Rem => b.binary(BinOp::FRem, left, right),
            B::Eq => b.fcmp(FloatPredicate::Oeq, left, right),
            B::Ne => b.fcmp(FloatPredicate::Une, left, right),
            B::Lt => b.fcmp(FloatPredicate::Olt, left, right),
            B::Le => b.fcmp(FloatPredicate::Ole, left, right),
            B::Gt => b.fcmp(FloatPredicate::Ogt, left, right),
            B::Ge => b.fcmp(FloatPredicate::Oge, left, right),
            B::Min | B::Max | B::MinNumber | B::MaxNumber => {
                let name = match op {
                    B::Min | B::MinNumber => "llvm.minimum",
                    _ => "llvm.maximum",
                };
                let ty = scalar_type(b, kind);
                let ordered = b.call(b.module().intrinsic(name, &[ty])?, &[left, right]);
                if matches!(op, B::Min | B::Max) {
                    ordered
                } else {
                    // A NaN operand yields the other operand. Not `llvm.minimumnum`: LLVM
                    // 21.1.8 on x86-64 unoptimized returns +0 for (+0, -0).
                    let left_nan = b.fcmp(FloatPredicate::Une, left, left);
                    let right_nan = b.fcmp(FloatPredicate::Une, right, right);
                    let number = b.select(right_nan, left, ordered);
                    b.select(left_nan, right, number)
                }
            }
            B::TotalOrder => total_order(b, kind, left, right),
            B::Shl
            | B::Shr
            | B::BitAnd
            | B::BitOr
            | B::BitXor
            | B::DivFloor
            | B::Mod
            | B::DivExact => {
                return unsupported(format!("{op:?} on a float"));
            }
        }),
        ScalarKind::Pointer => unsupported(format!("{op:?} on a pointer")),
        _ => {
            let (width, signed) = integer(kind).expect("an integer kind");
            let ty = b.module().int_type(width);
            let zero = b.module().const_int(ty, 0);
            if matches!(op, B::Shl | B::Shr) {
                return Ok(shift(b, op, width, signed, left, right, guard));
            }
            if matches!(op, B::DivFloor | B::Mod | B::DivExact) {
                return Ok(divide(b, op, width, signed, left, right, guard));
            }
            let s = if signed { 's' } else { 'u' };
            let compare = |signed_p, unsigned_p| {
                b.icmp(if signed { signed_p } else { unsigned_p }, left, right)
            };
            Ok(match op {
                B::Add => overflow(
                    b,
                    &format!("llvm.{s}add.with.overflow"),
                    ty,
                    left,
                    right,
                    guard,
                )?,
                B::Sub => overflow(
                    b,
                    &format!("llvm.{s}sub.with.overflow"),
                    ty,
                    left,
                    right,
                    guard,
                )?,
                B::Mul => overflow(
                    b,
                    &format!("llvm.{s}mul.with.overflow"),
                    ty,
                    left,
                    right,
                    guard,
                )?,
                B::Div | B::Rem => {
                    let by_zero = b.icmp(IntPredicate::Eq, right, zero);
                    let minus_one = b.module().const_int(ty, u128::MAX);
                    let is_minus_one = b.icmp(IntPredicate::Eq, right, minus_one);
                    if op == B::Div {
                        // `/0` and signed `MIN / -1` fail SEM-1.
                        let failed = if signed {
                            let min = b.module().const_int(ty, 1u128 << (width - 1));
                            let is_min = b.icmp(IntPredicate::Eq, left, min);
                            let overflows = b.binary(BinOp::And, is_min, is_minus_one);
                            b.binary(BinOp::Or, by_zero, overflows)
                        } else {
                            by_zero
                        };
                        guard(b, failed);
                        b.binary(if signed { BinOp::SDiv } else { BinOp::UDiv }, left, right)
                    } else {
                        // `% 0` fails; signed `MIN % -1` is zero (SEM-1). `srem` by -1 is
                        // undefined for MIN, and `x % -1` is 0 for every x, so divide by 1.
                        guard(b, by_zero);
                        if signed {
                            let one = b.module().const_int(ty, 1);
                            let divisor = b.select(is_minus_one, one, right);
                            b.binary(BinOp::SRem, left, divisor)
                        } else {
                            b.binary(BinOp::URem, left, right)
                        }
                    }
                }
                B::BitAnd => b.binary(BinOp::And, left, right),
                B::BitOr => b.binary(BinOp::Or, left, right),
                B::BitXor => b.binary(BinOp::Xor, left, right),
                B::Eq => b.icmp(IntPredicate::Eq, left, right),
                B::Ne => b.icmp(IntPredicate::Ne, left, right),
                B::Lt => compare(IntPredicate::Slt, IntPredicate::Ult),
                B::Le => compare(IntPredicate::Sle, IntPredicate::Ule),
                B::Gt => compare(IntPredicate::Sgt, IntPredicate::Ugt),
                B::Ge => compare(IntPredicate::Sge, IntPredicate::Uge),
                B::Shl | B::Shr => unreachable!("shifts return above"),
                B::DivFloor | B::Mod | B::DivExact => unreachable!("divisions return above"),
                B::Min | B::Max | B::MinNumber | B::MaxNumber | B::TotalOrder => {
                    return unsupported(format!("{op:?} on an integer"));
                }
            })
        }
    }
}

/// Shifts (SEM-1): a negative count or one at least the width fails; left shift discards high
/// bits; right shift sign-extends signed values and zero-extends unsigned ones.
fn shift<'m>(
    b: &Builder<'m>,
    op: BinaryOp,
    width: u32,
    signed: bool,
    left: Value<'m>,
    count: Value<'m>,
    guard: Guard<'_, 'm>,
) -> Value<'m> {
    let ty = b.module().int_type(width);
    // Unsigned `count >= width` also catches every negative signed count.
    let limit = b.module().const_int(ty, u128::from(width));
    guard(b, b.icmp(IntPredicate::Uge, count, limit));
    match (op, signed) {
        (BinaryOp::Shl, _) => b.binary(BinOp::Shl, left, count),
        (_, true) => b.binary(BinOp::AShr, left, count),
        (_, false) => b.binary(BinOp::LShr, left, count),
    }
}

/// `op operand`.
pub(crate) fn unary<'m>(
    b: &Builder<'m>,
    op: UnaryOp,
    kind: ScalarKind,
    operand: Value<'m>,
    guard: Guard<'_, 'm>,
) -> Result<Value<'m>, String> {
    let module = b.module();
    match (op, kind) {
        (UnaryOp::Not, ScalarKind::Bool) => {
            let one = module.const_int(module.int_type(1), 1);
            Ok(b.binary(BinOp::Xor, operand, one))
        }
        (UnaryOp::Neg, ScalarKind::F32 | ScalarKind::F64) => Ok(b.fneg(operand)),
        // `Not` and `BitNot` of an integer are its bitwise complement.
        (UnaryOp::Not | UnaryOp::BitNot, k) if integer(k).is_some() => {
            let ty = module.int_type(integer(k).unwrap().0);
            let ones = module.const_int(ty, u128::MAX);
            Ok(b.binary(BinOp::Xor, operand, ones))
        }
        // Negation is signed or float only; the validator rejects an unsigned operand.
        (UnaryOp::Neg, k) if integer(k).is_some_and(|(_, signed)| signed) => {
            let ty = module.int_type(integer(k).unwrap().0);
            let zero = module.const_int(ty, 0);
            overflow(b, "llvm.ssub.with.overflow", ty, zero, operand, guard)
        }
        _ => Err(format!("{op:?} on {kind:?}")),
    }
}

/// `div_floor`, `mod` and `div_exact` (docs/spec/core-library.md). A zero divisor and signed `MIN / -1` fail SEM-1
/// for all three, and a nonzero remainder for `div_exact`. The divisor is replaced by 1 on a
/// failing path before dividing, so no `sdiv`/`srem` reaches undefined behaviour, and the one
/// guard covers every failure.
fn divide<'m>(
    b: &Builder<'m>,
    op: BinaryOp,
    width: u32,
    signed: bool,
    left: Value<'m>,
    right: Value<'m>,
    guard: Guard<'_, 'm>,
) -> Value<'m> {
    let module = b.module();
    let ty = module.int_type(width);
    let zero = module.const_int(ty, 0);
    let one = module.const_int(ty, 1);
    let minus_one = module.const_int(ty, u128::MAX);
    let mut failed = b.icmp(IntPredicate::Eq, right, zero);
    if signed {
        let min = module.const_int(ty, 1u128 << (width - 1));
        let is_min = b.icmp(IntPredicate::Eq, left, min);
        let is_minus_one = b.icmp(IntPredicate::Eq, right, minus_one);
        let overflows = b.binary(BinOp::And, is_min, is_minus_one);
        failed = b.binary(BinOp::Or, failed, overflows);
    }
    let divisor = b.select(failed, one, right);
    let (div, rem) = if signed {
        (BinOp::SDiv, BinOp::SRem)
    } else {
        (BinOp::UDiv, BinOp::URem)
    };
    let quotient = b.binary(div, left, divisor);
    let remainder = b.binary(rem, left, divisor);
    let inexact = b.icmp(IntPredicate::Ne, remainder, zero);
    if op == BinaryOp::DivExact {
        failed = b.binary(BinOp::Or, failed, inexact);
    }
    guard(b, failed);
    if !signed || op == BinaryOp::DivExact {
        return if op == BinaryOp::Mod {
            remainder
        } else {
            quotient
        };
    }
    // A nonzero remainder whose sign differs from the divisor's rounds toward negative
    // infinity: the quotient one less, the remainder plus the divisor.
    let signs = b.binary(BinOp::Xor, remainder, divisor);
    let differ = b.icmp(IntPredicate::Slt, signs, zero);
    let adjust = b.binary(BinOp::And, inexact, differ);
    if op == BinaryOp::DivFloor {
        let lower = b.binary(BinOp::Add, quotient, minus_one);
        b.select(adjust, lower, quotient)
    } else {
        let raised = b.binary(BinOp::Add, remainder, divisor);
        b.select(adjust, raised, remainder)
    }
}

/// The IEEE bits of `x`, a float of `kind`, as the same-width integer, with every NaN read as
/// the kind's one observable NaN, the positive quiet NaN with a zero payload (SEM-2).
fn canonical_bits<'m>(b: &Builder<'m>, kind: ScalarKind, x: Value<'m>) -> Value<'m> {
    let module = b.module();
    let (width, nan) = if kind == ScalarKind::F32 {
        (32, 0x7fc0_0000u128)
    } else {
        (64, 0x7ff8_0000_0000_0000u128)
    };
    let bits_ty = module.int_type(width);
    let is_nan = b.fcmp(FloatPredicate::Une, x, x);
    let bits = b.cast(Cast::Bits, x, bits_ty);
    b.select(is_nan, module.const_int(bits_ty, nan), bits)
}

/// IEEE totalOrder of two floats after canonicalizing NaN to the positive quiet NaN (SEM-2), as
/// an i32 -1, 0 or 1: the bits' sign-magnitude keys compared as signed integers.
fn total_order<'m>(
    b: &Builder<'m>,
    kind: ScalarKind,
    left: Value<'m>,
    right: Value<'m>,
) -> Value<'m> {
    let module = b.module();
    let width = if kind == ScalarKind::F32 { 32 } else { 64 };
    let bits_ty = module.int_type(width);
    let key = |x: Value<'m>| {
        let bits = canonical_bits(b, kind, x);
        let top = module.const_int(bits_ty, u128::from(width - 1));
        let sign = b.binary(BinOp::AShr, bits, top);
        let one = module.const_int(bits_ty, 1);
        let magnitude = b.binary(BinOp::LShr, sign, one);
        b.binary(BinOp::Xor, bits, magnitude)
    };
    let (a, c) = (key(left), key(right));
    let i32_ty = module.int_type(32);
    let less = b.icmp(IntPredicate::Slt, a, c);
    let greater = b.icmp(IntPredicate::Sgt, a, c);
    let greater = b.cast(Cast::ZExt, greater, i32_ty);
    b.select(less, module.const_int(i32_ty, u128::MAX), greater)
}

/// `Convert` of `operand` of kind `from` into kind `to`: the result and, for `TryNarrow` and
/// `FloatToI32`, the flag (an `i1` and an `i32` status).
pub(crate) fn convert<'m>(
    b: &Builder<'m>,
    mode: ConvertMode,
    from: ScalarKind,
    to: ScalarKind,
    operand: Value<'m>,
) -> Result<(Value<'m>, Option<Value<'m>>), String> {
    let module = b.module();
    if mode == ConvertMode::IntToFloat {
        let Some((_, signed)) = integer(from) else {
            return Err(format!("{mode:?} of {from:?} into {to:?}"));
        };
        let target = match to {
            ScalarKind::F32 => module.f32_type(),
            ScalarKind::F64 => module.f64_type(),
            _ => return Err(format!("{mode:?} of {from:?} into {to:?}")),
        };
        let cast = if signed { Cast::SIToFP } else { Cast::UIToFP };
        return Ok((b.cast(cast, operand, target), None));
    }
    if mode == ConvertMode::Reinterpret {
        return match (from, to) {
            (ScalarKind::F32, ScalarKind::U32) | (ScalarKind::F64, ScalarKind::U64) => {
                Ok((canonical_bits(b, from, operand), None))
            }
            (ScalarKind::U32, ScalarKind::F32) | (ScalarKind::U64, ScalarKind::F64) => {
                let float = b.cast(Cast::Bits, operand, scalar_type(b, to));
                Ok((
                    b.cast(Cast::Bits, canonical_bits(b, to, float), scalar_type(b, to)),
                    None,
                ))
            }
            _ => Err(format!("{mode:?} of {from:?} into {to:?}")),
        };
    }
    if mode == ConvertMode::FloatToI32 {
        if !matches!(from, ScalarKind::F32 | ScalarKind::F64) || to != ScalarKind::I32 {
            return Err(format!("{mode:?} of {from:?} into {to:?}"));
        }
        let f64_ty = module.f64_type();
        let x = if from == ScalarKind::F32 {
            b.cast(Cast::FPExt, operand, f64_ty)
        } else {
            operand
        };
        let float = |v: f64| {
            b.cast(
                Cast::Bits,
                module.const_int(module.int_type(64), u128::from(v.to_bits())),
                f64_ty,
            )
        };
        let magnitude = b.call(module.intrinsic("llvm.fabs", &[f64_ty])?, &[x]);
        // NaN compares unordered, so it is not finite either.
        let finite = b.fcmp(FloatPredicate::Olt, magnitude, float(f64::INFINITY));
        // The truncation toward zero is below i32::MIN exactly when x <= i32::MIN - 1 and
        // above i32::MAX exactly when x >= i32::MAX + 1, both exact in f64: no `trunc`,
        // which baseline x86-64 lowers to a C library call.
        let below = b.fcmp(FloatPredicate::Ole, x, float(f64::from(i32::MIN) - 1.0));
        let above = b.fcmp(FloatPredicate::Oge, x, float(f64::from(i32::MAX) + 1.0));
        let outside = b.binary(BinOp::Or, below, above);
        let i32_ty = module.int_type(32);
        let range = b.select(
            outside,
            module.const_int(i32_ty, 2),
            module.const_int(i32_ty, 0),
        );
        let status = b.select(finite, range, module.const_int(i32_ty, 1));
        let saturating = module.intrinsic("llvm.fptosi.sat", &[i32_ty, f64_ty])?;
        return Ok((b.call(saturating, &[x]), Some(status)));
    }
    let ((wi, si), (wj, sj)) = match (integer(from), integer(to)) {
        (Some(i), Some(j)) => (i, j),
        _ => return Err(format!("{mode:?} of {from:?} into {to:?}")),
    };
    let target = module.int_type(wj);
    let value = if wj < wi {
        b.cast(Cast::Trunc, operand, target)
    } else if wj > wi {
        b.cast(if si { Cast::SExt } else { Cast::ZExt }, operand, target)
    } else {
        operand
    };
    match mode {
        ConvertMode::Widen | ConvertMode::Truncate => Ok((value, None)),
        ConvertMode::TryNarrow => Ok((value, Some(representable(b, wi, si, wj, sj, operand)))),
        ConvertMode::FloatToI32 | ConvertMode::IntToFloat | ConvertMode::Reinterpret => {
            unreachable!("handled above")
        }
    }
}

/// Whether `operand`, an integer of width `wi` (signed `si`), is a value of the integer format
/// `wj`/`sj`: a range check in the operand's own domain, with the bounds that the operand's
/// own range already implies left out.
fn representable<'m>(
    b: &Builder<'m>,
    wi: u32,
    si: bool,
    wj: u32,
    sj: bool,
    operand: Value<'m>,
) -> Value<'m> {
    let module = b.module();
    let ty = module.int_type(wi);
    let mask = |w: u32| {
        if w == 128 {
            u128::MAX
        } else {
            (1u128 << w) - 1
        }
    };
    // The destination's maximum against the source's, both nonnegative.
    let max_j = if sj { mask(wj) >> 1 } else { mask(wj) };
    let max_i = if si { mask(wi) >> 1 } else { mask(wi) };
    let mut fits = module.const_int(module.int_type(1), 1);
    if max_j < max_i {
        let bound = module.const_int(ty, max_j);
        let below = b.icmp(
            if si {
                IntPredicate::Sle
            } else {
                IntPredicate::Ule
            },
            operand,
            bound,
        );
        fits = b.binary(BinOp::And, fits, below);
    }
    // A negative source needs a signed destination at least as deep.
    if si && (!sj || wj < wi) {
        let lowest = if sj {
            (1u128 << (wj - 1)).wrapping_neg() & mask(wi)
        } else {
            0
        };
        let bound = module.const_int(ty, lowest);
        let above = b.icmp(IntPredicate::Sge, operand, bound);
        fits = b.binary(BinOp::And, fits, above);
    }
    fits
}

/// `IntArith`: `Add` of two integers of `kind` in `mode`; `Checked` also yields the `i1`
/// overflow flag.
pub(crate) fn int_arith<'m>(
    b: &Builder<'m>,
    op: BinaryOp,
    mode: ArithMode,
    kind: ScalarKind,
    left: Value<'m>,
    right: Value<'m>,
) -> Result<(Value<'m>, Option<Value<'m>>), String> {
    let Some((width, signed)) = integer(kind) else {
        return Err(format!("{op:?} of {kind:?}"));
    };
    if op != BinaryOp::Add {
        return Err(format!("{op:?} in IntArith"));
    }
    let ty = b.module().int_type(width);
    let s = if signed { 's' } else { 'u' };
    Ok(match mode {
        ArithMode::Wrapping => (b.binary(BinOp::Add, left, right), None),
        ArithMode::Saturating => {
            let sat = b.module().intrinsic(&format!("llvm.{s}add.sat"), &[ty])?;
            (b.call(sat, &[left, right]), None)
        }
        ArithMode::Checked => {
            let checked = b
                .module()
                .intrinsic(&format!("llvm.{s}add.with.overflow"), &[ty])?;
            let pair = b.call(checked, &[left, right]);
            (b.extract(pair, 0), Some(b.extract(pair, 1)))
        }
    })
}

/// `MulAdd`: `left * right + addend` rounded once (SEM-2's one fused operation).
pub(crate) fn mul_add<'m>(
    b: &Builder<'m>,
    kind: ScalarKind,
    operands: [Value<'m>; 3],
) -> Result<Value<'m>, String> {
    if !matches!(kind, ScalarKind::F32 | ScalarKind::F64) {
        return Err(format!("mul_add of {kind:?}"));
    }
    let fma = b.module().intrinsic("llvm.fma", &[scalar_type(b, kind)])?;
    Ok(b.call(fma, &operands))
}
