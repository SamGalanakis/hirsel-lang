//! Scalar operations (SEM-1, SEM-2), as `hirsel-llvm`'s `scalar` lowers them.
//!
//! Integer `+ - *` and unary `-` use the overflow instructions; division, remainder and
//! shifts are guarded before the instruction runs, so no path reaches a trapping Cranelift
//! instruction. 128-bit division, remainder and integer-to-float conversion call the
//! toolchain's compiler runtime (`TOOLCHAIN_IMPORTS`), and float remainder calls `fmod`.

use cranelift_codegen::ir::{
    InstBuilder, Value,
    condcodes::{FloatCC, IntCC},
    immediates::{Ieee32, Ieee64},
    types,
};
use hirsel_lir::ir::{ArithMode, BinaryOp, ConvertMode, FailureId, ScalarUnaryOp, UnaryOp};
use hirsel_lir::layout_types::ScalarKind;

use crate::function::{FnEmitter, flags};
use crate::module::{Emitted, scalar_type};

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

fn mask(width: u32) -> u128 {
    if width == 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    }
}

impl FnEmitter<'_, '_, '_> {
    /// `left op right` on two operands of `kind`; comparisons yield a Bool. A failing
    /// operation takes `failure`.
    pub(crate) fn binary(
        &mut self,
        op: BinaryOp,
        kind: ScalarKind,
        left: Value,
        right: Value,
        failure: FailureId,
    ) -> Emitted<Value> {
        use BinaryOp as B;
        match kind {
            ScalarKind::Bool => Ok(match op {
                B::Eq => self.b.ins().icmp(IntCC::Equal, left, right),
                B::Ne => self.b.ins().icmp(IntCC::NotEqual, left, right),
                B::BitAnd => self.b.ins().band(left, right),
                B::BitOr => self.b.ins().bor(left, right),
                B::BitXor => self.b.ins().bxor(left, right),
                _ => return self.invalid(format!("{op:?} on Bool")),
            }),
            ScalarKind::F32 | ScalarKind::F64 => Ok(match op {
                B::Add => self.b.ins().fadd(left, right),
                B::Sub => self.b.ins().fsub(left, right),
                B::Mul => self.b.ins().fmul(left, right),
                B::Div => self.b.ins().fdiv(left, right),
                B::Rem => {
                    let (name, ty) = if kind == ScalarKind::F32 {
                        ("fmodf", types::F32)
                    } else {
                        ("fmod", types::F64)
                    };
                    self.toolchain_call(name, &[ty, ty], ty, &[left, right])?
                }
                B::Eq => self.b.ins().fcmp(FloatCC::Equal, left, right),
                B::Ne => self.b.ins().fcmp(FloatCC::NotEqual, left, right),
                B::Lt => self.b.ins().fcmp(FloatCC::LessThan, left, right),
                B::Le => self.b.ins().fcmp(FloatCC::LessThanOrEqual, left, right),
                B::Gt => self.b.ins().fcmp(FloatCC::GreaterThan, left, right),
                B::Ge => self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, left, right),
                B::Min | B::Max | B::MinNumber | B::MaxNumber => {
                    let ordered = if matches!(op, B::Min | B::MinNumber) {
                        self.b.ins().fmin(left, right)
                    } else {
                        self.b.ins().fmax(left, right)
                    };
                    if matches!(op, B::Min | B::Max) {
                        ordered
                    } else {
                        // A NaN operand yields the other operand.
                        let left_nan = self.b.ins().fcmp(FloatCC::Unordered, left, left);
                        let right_nan = self.b.ins().fcmp(FloatCC::Unordered, right, right);
                        let number = self.b.ins().select(right_nan, left, ordered);
                        self.b.ins().select(left_nan, right, number)
                    }
                }
                B::Shl
                | B::Shr
                | B::BitAnd
                | B::BitOr
                | B::BitXor
                | B::DivFloor
                | B::Mod
                | B::DivExact => {
                    return self.invalid(format!("{op:?} on a float"));
                }
            }),
            ScalarKind::Pointer => self.invalid(format!("{op:?} on a pointer")),
            _ => {
                let (width, signed) = integer(kind).expect("an integer kind");
                let ty = scalar_type(kind);
                if matches!(op, B::Shl | B::Shr) {
                    // Unsigned `count >= width` also catches every negative signed count.
                    let limit = self.int_const(ty, u128::from(width));
                    let outside =
                        self.b
                            .ins()
                            .icmp(IntCC::UnsignedGreaterThanOrEqual, right, limit);
                    self.guard(outside, failure)?;
                    return Ok(match (op, signed) {
                        (B::Shl, _) => self.b.ins().ishl(left, right),
                        (_, true) => self.b.ins().sshr(left, right),
                        (_, false) => self.b.ins().ushr(left, right),
                    });
                }
                if matches!(op, B::DivFloor | B::Mod | B::DivExact) {
                    return self.divide(op, width, signed, left, right, failure);
                }
                let compare = |this: &mut Self, s: IntCC, u: IntCC| {
                    this.b.ins().icmp(if signed { s } else { u }, left, right)
                };
                Ok(match op {
                    B::Add => {
                        let (sum, overflow) = if signed {
                            self.b.ins().sadd_overflow(left, right)
                        } else {
                            self.b.ins().uadd_overflow(left, right)
                        };
                        self.guard(overflow, failure)?;
                        sum
                    }
                    B::Sub => {
                        let (difference, overflow) = if signed {
                            self.b.ins().ssub_overflow(left, right)
                        } else {
                            self.b.ins().usub_overflow(left, right)
                        };
                        self.guard(overflow, failure)?;
                        difference
                    }
                    B::Mul => {
                        let (product, overflow) = self.mul_overflow(width, signed, left, right)?;
                        self.guard(overflow, failure)?;
                        product
                    }
                    B::Div | B::Rem => {
                        let by_zero = self.icmp_const(IntCC::Equal, right, 0);
                        let minus_one = self.int_const(ty, mask(width));
                        let is_minus_one = self.b.ins().icmp(IntCC::Equal, right, minus_one);
                        if op == B::Div {
                            // `/0` and signed `MIN / -1` fail SEM-1.
                            let failed = if signed {
                                let min = self.int_const(ty, 1u128 << (width - 1));
                                let is_min = self.b.ins().icmp(IntCC::Equal, left, min);
                                let overflows = self.b.ins().band(is_min, is_minus_one);
                                self.b.ins().bor(by_zero, overflows)
                            } else {
                                by_zero
                            };
                            self.guard(failed, failure)?;
                            self.divide_raw(width, signed, false, left, right)?
                        } else {
                            // `% 0` fails; signed `MIN % -1` is zero (SEM-1), as `x % 1` is.
                            self.guard(by_zero, failure)?;
                            let divisor = if signed {
                                let one = self.int_const(ty, 1);
                                self.b.ins().select(is_minus_one, one, right)
                            } else {
                                right
                            };
                            self.divide_raw(width, signed, true, left, divisor)?
                        }
                    }
                    B::BitAnd => self.b.ins().band(left, right),
                    B::BitOr => self.b.ins().bor(left, right),
                    B::BitXor => self.b.ins().bxor(left, right),
                    B::Eq => self.b.ins().icmp(IntCC::Equal, left, right),
                    B::Ne => self.b.ins().icmp(IntCC::NotEqual, left, right),
                    B::Lt => compare(self, IntCC::SignedLessThan, IntCC::UnsignedLessThan),
                    B::Le => compare(
                        self,
                        IntCC::SignedLessThanOrEqual,
                        IntCC::UnsignedLessThanOrEqual,
                    ),
                    B::Gt => compare(self, IntCC::SignedGreaterThan, IntCC::UnsignedGreaterThan),
                    B::Ge => compare(
                        self,
                        IntCC::SignedGreaterThanOrEqual,
                        IntCC::UnsignedGreaterThanOrEqual,
                    ),
                    B::Shl | B::Shr => unreachable!("shifts return above"),
                    B::DivFloor | B::Mod | B::DivExact => unreachable!("divisions return above"),
                    B::Min | B::Max | B::MinNumber | B::MaxNumber => {
                        return self.invalid(format!("{op:?} on an integer"));
                    }
                })
            }
        }
    }

    /// The wrapped product and its overflow flag.
    fn mul_overflow(
        &mut self,
        width: u32,
        signed: bool,
        left: Value,
        right: Value,
    ) -> Emitted<(Value, Value)> {
        if width <= 64 {
            return Ok(if signed {
                self.b.ins().smul_overflow(left, right)
            } else {
                self.b.ins().umul_overflow(left, right)
            });
        }
        // 128 bits: the product overflows exactly when dividing it by a nonzero left
        // operand does not give the right one back, or for signed `-1 * MIN`.
        let ty = types::I128;
        let product = self.b.ins().imul(left, right);
        let zero = self.icmp_const(IntCC::Equal, left, 0);
        let one = self.int_const(ty, 1);
        let divisor = self.b.ins().select(zero, one, left);
        let quotient = self.divide_raw(width, signed, false, product, divisor)?;
        let differs = self.b.ins().icmp(IntCC::NotEqual, quotient, right);
        let nonzero = self.icmp_const(IntCC::NotEqual, left, 0);
        let mut overflow = self.b.ins().band(nonzero, differs);
        if signed {
            let minus_one = self.int_const(ty, u128::MAX);
            let min = self.int_const(ty, 1u128 << 127);
            let a = self.b.ins().icmp(IntCC::Equal, left, minus_one);
            let b = self.b.ins().icmp(IntCC::Equal, right, min);
            let c = self.b.ins().icmp(IntCC::Equal, right, minus_one);
            let d = self.b.ins().icmp(IntCC::Equal, left, min);
            let ab = self.b.ins().band(a, b);
            let cd = self.b.ins().band(c, d);
            overflow = self.b.ins().bor(overflow, ab);
            overflow = self.b.ins().bor(overflow, cd);
        }
        Ok((product, overflow))
    }

    /// `left / right` or `left % right` for a divisor already made safe.
    fn divide_raw(
        &mut self,
        width: u32,
        signed: bool,
        remainder: bool,
        left: Value,
        right: Value,
    ) -> Emitted<Value> {
        if width == 128 {
            let name = match (signed, remainder) {
                (true, false) => "__divti3",
                (true, true) => "__modti3",
                (false, false) => "__udivti3",
                (false, true) => "__umodti3",
            };
            let ty = types::I128;
            return self.toolchain_call(name, &[ty, ty], ty, &[left, right]);
        }
        Ok(match (signed, remainder) {
            (true, false) => self.b.ins().sdiv(left, right),
            (true, true) => self.b.ins().srem(left, right),
            (false, false) => self.b.ins().udiv(left, right),
            (false, true) => self.b.ins().urem(left, right),
        })
    }

    /// `div_floor`, `mod` and `div_exact`: a zero divisor and signed `MIN / -1` fail SEM-1
    /// for all three, and a nonzero remainder for `div_exact`. The divisor is replaced by 1
    /// on a failing path before dividing.
    fn divide(
        &mut self,
        op: BinaryOp,
        width: u32,
        signed: bool,
        left: Value,
        right: Value,
        failure: FailureId,
    ) -> Emitted<Value> {
        let ty = left_type(width);
        let one = self.int_const(ty, 1);
        let minus_one = self.int_const(ty, mask(width));
        let mut failed = self.icmp_const(IntCC::Equal, right, 0);
        if signed {
            let min = self.int_const(ty, 1u128 << (width - 1));
            let is_min = self.b.ins().icmp(IntCC::Equal, left, min);
            let is_minus_one = self.b.ins().icmp(IntCC::Equal, right, minus_one);
            let overflows = self.b.ins().band(is_min, is_minus_one);
            failed = self.b.ins().bor(failed, overflows);
        }
        let divisor = self.b.ins().select(failed, one, right);
        let quotient = self.divide_raw(width, signed, false, left, divisor)?;
        let remainder = self.divide_raw(width, signed, true, left, divisor)?;
        let inexact = self.icmp_const(IntCC::NotEqual, remainder, 0);
        if op == BinaryOp::DivExact {
            failed = self.b.ins().bor(failed, inexact);
        }
        self.guard(failed, failure)?;
        if !signed || op == BinaryOp::DivExact {
            return Ok(if op == BinaryOp::Mod {
                remainder
            } else {
                quotient
            });
        }
        // A nonzero remainder whose sign differs from the divisor's rounds toward negative
        // infinity: the quotient one less, the remainder plus the divisor.
        let signs = self.b.ins().bxor(remainder, divisor);
        let zero = self.int_const(ty, 0);
        let differ = self.b.ins().icmp(IntCC::SignedLessThan, signs, zero);
        let adjust = self.b.ins().band(inexact, differ);
        Ok(if op == BinaryOp::DivFloor {
            let lower = self.b.ins().iadd(quotient, minus_one);
            self.b.ins().select(adjust, lower, quotient)
        } else {
            let raised = self.b.ins().iadd(remainder, divisor);
            self.b.ins().select(adjust, raised, remainder)
        })
    }

    /// The IEEE bits of `x` as the same-width integer, with every NaN read as the kind's
    /// one observable NaN (SEM-2).
    fn canonical_bits(&mut self, kind: ScalarKind, x: Value) -> Value {
        let (ty, nan) = if kind == ScalarKind::F32 {
            (types::I32, 0x7fc0_0000u64)
        } else {
            (types::I64, 0x7ff8_0000_0000_0000u64)
        };
        let is_nan = self.b.ins().fcmp(FloatCC::Unordered, x, x);
        let bits = self.b.ins().bitcast(ty, flags(), x);
        let nan = self.b.ins().iconst(ty, nan as i64);
        self.b.ins().select(is_nan, nan, bits)
    }

    /// `op operand`.
    pub(crate) fn unary(
        &mut self,
        op: UnaryOp,
        kind: ScalarKind,
        operand: Value,
        failure: FailureId,
    ) -> Emitted<Value> {
        match (op, kind) {
            (UnaryOp::Not, ScalarKind::Bool) => Ok(self.b.ins().bxor_imm_s(operand, 1)),
            (UnaryOp::Neg, ScalarKind::F32 | ScalarKind::F64) => Ok(self.b.ins().fneg(operand)),
            (UnaryOp::Not | UnaryOp::BitNot, k) if integer(k).is_some() => {
                Ok(self.b.ins().bnot(operand))
            }
            // Negation is signed or float only.
            (UnaryOp::Neg, k) if integer(k).is_some_and(|(_, signed)| signed) => {
                let zero = self.int_const(scalar_type(k), 0);
                let (negated, overflow) = self.b.ins().ssub_overflow(zero, operand);
                self.guard(overflow, failure)?;
                Ok(negated)
            }
            _ => self.invalid(format!("{op:?} on {kind:?}")),
        }
    }

    /// `Convert` of `operand` of kind `from` into kind `to`: the result and, for
    /// `TryNarrow` and `FloatToI32`, the flag.
    pub(crate) fn convert(
        &mut self,
        mode: ConvertMode,
        from: ScalarKind,
        to: ScalarKind,
        operand: Value,
    ) -> Emitted<(Value, Option<Value>)> {
        let refuse = || format!("{mode:?} of {from:?} into {to:?}");
        if mode == ConvertMode::IntToFloat {
            let Some((width, signed)) = integer(from) else {
                return self.invalid(refuse());
            };
            let target = match to {
                ScalarKind::F32 => types::F32,
                ScalarKind::F64 => types::F64,
                _ => return self.invalid(refuse()),
            };
            if width == 128 {
                let name = match (signed, to) {
                    (true, ScalarKind::F32) => "__floattisf",
                    (true, _) => "__floattidf",
                    (false, ScalarKind::F32) => "__floatuntisf",
                    (false, _) => "__floatuntidf",
                };
                let value = self.toolchain_call(name, &[types::I128], target, &[operand])?;
                return Ok((value, None));
            }
            let wide = if width < 32 {
                if signed {
                    self.b.ins().sextend(types::I32, operand)
                } else {
                    self.b.ins().uextend(types::I32, operand)
                }
            } else {
                operand
            };
            let value = if signed {
                self.b.ins().fcvt_from_sint(target, wide)
            } else {
                self.b.ins().fcvt_from_uint(target, wide)
            };
            return Ok((value, None));
        }
        if mode == ConvertMode::Reinterpret {
            return match (from, to) {
                (ScalarKind::F32, ScalarKind::U32) | (ScalarKind::F64, ScalarKind::U64) => {
                    Ok((self.canonical_bits(from, operand), None))
                }
                (ScalarKind::U32, ScalarKind::F32) | (ScalarKind::U64, ScalarKind::F64) => {
                    let float = self.b.ins().bitcast(scalar_type(to), flags(), operand);
                    let bits = self.canonical_bits(to, float);
                    Ok((self.b.ins().bitcast(scalar_type(to), flags(), bits), None))
                }
                _ => self.invalid(refuse()),
            };
        }
        if mode == ConvertMode::FloatResize {
            return match (from, to) {
                (ScalarKind::F32, ScalarKind::F64) => {
                    Ok((self.b.ins().fpromote(types::F64, operand), None))
                }
                (ScalarKind::F64, ScalarKind::F32) => {
                    Ok((self.b.ins().fdemote(types::F32, operand), None))
                }
                _ => self.invalid(refuse()),
            };
        }
        if mode == ConvertMode::FloatToI32 {
            if !matches!(from, ScalarKind::F32 | ScalarKind::F64) || to != ScalarKind::I32 {
                return self.invalid(refuse());
            }
            let x = if from == ScalarKind::F32 {
                self.b.ins().fpromote(types::F64, operand)
            } else {
                operand
            };
            let float =
                |this: &mut Self, v: f64| this.b.ins().f64const(Ieee64::with_bits(v.to_bits()));
            let magnitude = self.b.ins().fabs(x);
            let infinity = float(self, f64::INFINITY);
            // NaN compares unordered, so it is not finite either.
            let finite = self.b.ins().fcmp(FloatCC::LessThan, magnitude, infinity);
            let low = float(self, f64::from(i32::MIN) - 1.0);
            let high = float(self, f64::from(i32::MAX) + 1.0);
            let below = self.b.ins().fcmp(FloatCC::LessThanOrEqual, x, low);
            let above = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, x, high);
            let outside = self.b.ins().bor(below, above);
            let two = self.b.ins().iconst(types::I32, 2);
            let zero = self.b.ins().iconst(types::I32, 0);
            let one = self.b.ins().iconst(types::I32, 1);
            let range = self.b.ins().select(outside, two, zero);
            let status = self.b.ins().select(finite, range, one);
            let value = self.b.ins().fcvt_to_sint_sat(types::I32, x);
            return Ok((value, Some(status)));
        }
        let ((wi, si), (wj, sj)) = match (integer(from), integer(to)) {
            (Some(i), Some(j)) => (i, j),
            _ => return self.invalid(refuse()),
        };
        let target = scalar_type(to);
        let value = if wj < wi {
            self.b.ins().ireduce(target, operand)
        } else if wj > wi {
            if si {
                self.b.ins().sextend(target, operand)
            } else {
                self.b.ins().uextend(target, operand)
            }
        } else {
            operand
        };
        match mode {
            ConvertMode::Widen | ConvertMode::Truncate => Ok((value, None)),
            ConvertMode::TryNarrow => {
                let fits = self.representable(wi, si, wj, sj, operand);
                Ok((value, Some(fits)))
            }
            ConvertMode::FloatToI32
            | ConvertMode::IntToFloat
            | ConvertMode::Reinterpret
            | ConvertMode::FloatResize => {
                unreachable!("handled above")
            }
        }
    }

    /// Whether `operand`, an integer of width `wi` (signed `si`), is a value of the integer
    /// format `wj`/`sj`: a range check in the operand's own domain.
    fn representable(&mut self, wi: u32, si: bool, wj: u32, sj: bool, operand: Value) -> Value {
        let ty = left_type(wi);
        let max_j = if sj { mask(wj) >> 1 } else { mask(wj) };
        let max_i = if si { mask(wi) >> 1 } else { mask(wi) };
        let mut fits = self.b.ins().iconst(types::I8, 1);
        if max_j < max_i {
            let bound = self.int_const(ty, max_j);
            let cc = if si {
                IntCC::SignedLessThanOrEqual
            } else {
                IntCC::UnsignedLessThanOrEqual
            };
            let below = self.b.ins().icmp(cc, operand, bound);
            fits = self.b.ins().band(fits, below);
        }
        // A negative source needs a signed destination at least as deep.
        if si && (!sj || wj < wi) {
            let lowest = if sj {
                (1u128 << (wj - 1)).wrapping_neg() & mask(wi)
            } else {
                0
            };
            let bound = self.int_const(ty, lowest);
            let above = self
                .b
                .ins()
                .icmp(IntCC::SignedGreaterThanOrEqual, operand, bound);
            fits = self.b.ins().band(fits, above);
        }
        fits
    }

    /// `IntArith`: `Add`, `Sub` or `Mul` of two integers of `kind` in `mode`; `Checked`
    /// also yields the overflow flag.
    pub(crate) fn int_arith(
        &mut self,
        op: BinaryOp,
        mode: ArithMode,
        kind: ScalarKind,
        left: Value,
        right: Value,
    ) -> Emitted<(Value, Option<Value>)> {
        let Some((width, signed)) = integer(kind) else {
            return self.invalid(format!("{op:?} of {kind:?}"));
        };
        Ok(match (op, mode) {
            (BinaryOp::Add, ArithMode::Wrapping) => (self.b.ins().iadd(left, right), None),
            (BinaryOp::Sub, ArithMode::Wrapping) => (self.b.ins().isub(left, right), None),
            (BinaryOp::Mul, ArithMode::Wrapping) => (self.b.ins().imul(left, right), None),
            (BinaryOp::Add, ArithMode::Saturating) => {
                (self.saturating_add(width, signed, left, right), None)
            }
            (BinaryOp::Sub, ArithMode::Saturating) => {
                (self.saturating_sub(width, signed, left, right), None)
            }
            (BinaryOp::Add, ArithMode::Checked) => {
                let (sum, overflow) = if signed {
                    self.b.ins().sadd_overflow(left, right)
                } else {
                    self.b.ins().uadd_overflow(left, right)
                };
                (sum, Some(overflow))
            }
            (BinaryOp::Sub, ArithMode::Checked) => {
                let (difference, overflow) = if signed {
                    self.b.ins().ssub_overflow(left, right)
                } else {
                    self.b.ins().usub_overflow(left, right)
                };
                (difference, Some(overflow))
            }
            (BinaryOp::Mul, ArithMode::Checked) => {
                let (product, overflow) = self.mul_overflow(width, signed, left, right)?;
                (product, Some(overflow))
            }
            _ => return self.invalid(format!("{op:?} in IntArith {mode:?}")),
        })
    }

    /// `left - right`, saturated at the kind's bounds.
    pub(crate) fn saturating_sub(
        &mut self,
        width: u32,
        signed: bool,
        left: Value,
        right: Value,
    ) -> Value {
        let ty = left_type(width);
        if !signed {
            let (difference, overflow) = self.b.ins().usub_overflow(left, right);
            let zero = self.int_const(ty, 0);
            return self.b.ins().select(overflow, zero, difference);
        }
        let (difference, overflow) = self.b.ins().ssub_overflow(left, right);
        // Signed subtraction overflows only toward the left operand's sign.
        let zero = self.int_const(ty, 0);
        let negative = self.b.ins().icmp(IntCC::SignedLessThan, left, zero);
        let min = self.int_const(ty, 1u128 << (width - 1));
        let max = self.int_const(ty, mask(width) >> 1);
        let bound = self.b.ins().select(negative, min, max);
        self.b.ins().select(overflow, bound, difference)
    }

    /// `left + right`, saturated at the kind's bounds.
    pub(crate) fn saturating_add(
        &mut self,
        width: u32,
        signed: bool,
        left: Value,
        right: Value,
    ) -> Value {
        let ty = left_type(width);
        if !signed {
            let (sum, overflow) = self.b.ins().uadd_overflow(left, right);
            let max = self.int_const(ty, mask(width));
            return self.b.ins().select(overflow, max, sum);
        }
        let (sum, overflow) = self.b.ins().sadd_overflow(left, right);
        // Signed addition overflows only toward the left operand's sign.
        let zero = self.int_const(ty, 0);
        let negative = self.b.ins().icmp(IntCC::SignedLessThan, left, zero);
        let min = self.int_const(ty, 1u128 << (width - 1));
        let max = self.int_const(ty, mask(width) >> 1);
        let bound = self.b.ins().select(negative, min, max);
        self.b.ins().select(overflow, bound, sum)
    }

    /// `MulAdd`: `left * right + addend` rounded once.
    pub(crate) fn mul_add(&mut self, kind: ScalarKind, operands: [Value; 3]) -> Emitted<Value> {
        if !matches!(kind, ScalarKind::F32 | ScalarKind::F64) {
            return self.invalid(format!("mul_add of {kind:?}"));
        }
        Ok(self.b.ins().fma(operands[0], operands[1], operands[2]))
    }

    /// `ScalarUnary`: an exact float or integer operation of one operand of `kind`.
    /// Ties-away rounding has no Cranelift instruction and calls the C library's `round`.
    pub(crate) fn scalar_unary(
        &mut self,
        op: ScalarUnaryOp,
        kind: ScalarKind,
        operand: Value,
    ) -> Emitted<Value> {
        use ScalarUnaryOp as U;
        let float = matches!(kind, ScalarKind::F32 | ScalarKind::F64);
        if op.is_float() != float || !float && integer(kind).is_none() {
            return self.invalid(format!("{op:?} of {kind:?}"));
        }
        Ok(match op {
            U::Sqrt => self.b.ins().sqrt(operand),
            U::Floor => self.b.ins().floor(operand),
            U::Ceil => self.b.ins().ceil(operand),
            U::Trunc => self.b.ins().trunc(operand),
            U::Round => {
                let (name, ty) = if kind == ScalarKind::F32 {
                    ("roundf", types::F32)
                } else {
                    ("round", types::F64)
                };
                self.toolchain_call(name, &[ty], ty, &[operand])?
            }
            U::Abs => self.b.ins().fabs(operand),
            U::LeadingZeros => self.b.ins().clz(operand),
            U::TrailingZeros => self.b.ins().ctz(operand),
            U::CountOnes => self.b.ins().popcnt(operand),
            // One byte is its own byte order.
            U::SwapBytes if matches!(kind, ScalarKind::U8 | ScalarKind::I8) => operand,
            U::SwapBytes => self.b.ins().bswap(operand),
            U::ReverseBits => self.b.ins().bitrev(operand),
        })
    }

    /// `v cc c`, for a value of any integer type, `i128` included.
    pub(crate) fn icmp_const(&mut self, cc: IntCC, v: Value, c: u128) -> Value {
        let ty = self.b.func.dfg.value_type(v);
        if ty == types::I128 {
            let k = self.int_const(ty, c);
            return self.b.ins().icmp(cc, v, k);
        }
        self.b.ins().icmp_imm_s(cc, v, c as u64 as i64)
    }

    /// The float constant of `kind` with IEEE bits `bits`.
    pub(crate) fn float_const(&mut self, kind: ScalarKind, bits: u64) -> Value {
        if kind == ScalarKind::F32 {
            self.b.ins().f32const(Ieee32::with_bits(bits as u32))
        } else {
            self.b.ins().f64const(Ieee64::with_bits(bits))
        }
    }
}

fn left_type(width: u32) -> cranelift_codegen::ir::Type {
    match width {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        64 => types::I64,
        _ => types::I128,
    }
}

impl FnEmitter<'_, '_, '_> {
    /// Same integer-root SipHash-1-3 lowering as LLVM's scalar::hash_integer.
    /// Rust core/src/hash/sip.rs:245-335, f6c1107f (MIT OR Apache-2.0).
    pub(crate) fn hash_integer(&mut self, value: Value, seed: Value, bits: u32) -> Value {
        let k1 = 0x6869_7273_656c_2d37;
        let a = self.u64c(0x736f6d6570736575);
        let a = self.b.ins().bxor(a, seed);
        let b = self.u64c(0x646f72616e646f6d ^ k1);
        let c = self.u64c(0x6c7967656e657261);
        let c = self.b.ins().bxor(c, seed);
        let d = self.u64c(0x7465646279746573 ^ k1);
        let mut v = [a, b, c, d];
        let low = if bits < 64 {
            self.b.ins().uextend(types::I64, value)
        } else if bits > 64 {
            self.b.ins().ireduce(types::I64, value)
        } else {
            value
        };
        if bits >= 64 {
            self.hash_compress(&mut v, low);
        }
        if bits == 128 {
            let high = self.b.ins().ushr_imm(value, 64);
            let high = self.b.ins().ireduce(types::I64, high);
            self.hash_compress(&mut v, high);
        }
        let length = u64::from(bits.div_ceil(8));
        let last = self.u64c(length << 56);
        let last = if bits < 64 {
            self.b.ins().bor(low, last)
        } else {
            last
        };
        self.hash_compress(&mut v, last);
        v[2] = self.b.ins().bxor_imm(v[2], 0xff);
        for _ in 0..3 {
            self.hash_round(&mut v);
        }
        let a = self.b.ins().bxor(v[0], v[1]);
        let b = self.b.ins().bxor(v[2], v[3]);
        self.b.ins().bxor(a, b)
    }
    fn hash_compress(&mut self, v: &mut [Value; 4], word: Value) {
        v[3] = self.b.ins().bxor(v[3], word);
        self.hash_round(v);
        v[0] = self.b.ins().bxor(v[0], word);
    }
    fn hash_round(&mut self, v: &mut [Value; 4]) {
        v[0] = self.b.ins().iadd(v[0], v[1]);
        v[1] = self.b.ins().rotl_imm(v[1], 13);
        v[1] = self.b.ins().bxor(v[1], v[0]);
        v[0] = self.b.ins().rotl_imm(v[0], 32);
        v[2] = self.b.ins().iadd(v[2], v[3]);
        v[3] = self.b.ins().rotl_imm(v[3], 16);
        v[3] = self.b.ins().bxor(v[3], v[2]);
        v[0] = self.b.ins().iadd(v[0], v[3]);
        v[3] = self.b.ins().rotl_imm(v[3], 21);
        v[3] = self.b.ins().bxor(v[3], v[0]);
        v[2] = self.b.ins().iadd(v[2], v[1]);
        v[1] = self.b.ins().rotl_imm(v[1], 17);
        v[1] = self.b.ins().bxor(v[1], v[2]);
        v[2] = self.b.ins().rotl_imm(v[2], 32);
    }
}
