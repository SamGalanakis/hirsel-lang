//! Core library integer kernels (docs/spec/core-library.md); all widths including i128/u128 use checked arithmetic.
use crate::{
    Integer, Value, conversion,
    scalar::{self, ScalarError},
    value::{integer_layout, mask, signed_bounds},
};
use hirsel_syntax::BinaryOp;
use hirsel_types::Primitive;

pub fn widening_pair(source: Primitive, target: Primitive) -> bool {
    match (integer_layout(source), integer_layout(target)) {
        (Some((a, sa)), Some((b, sb))) => match (sa, sb) {
            (true, false) => false,
            (false, true) => b > a,
            _ => b >= a,
        },
        _ => false,
    }
}
pub fn convert(
    name: &str,
    value: Integer,
    target: Primitive,
) -> Result<Option<Integer>, ScalarError> {
    match name {
        "widen" if widening_pair(value.kind(), target) => conversion::exact(value, target),
        "widen" => Err(ScalarError::InvalidOperation),
        "try_narrow" => conversion::exact(value, target),
        "narrow" => conversion::exact(value, target)?
            .map(Some)
            .ok_or(ScalarError::Overflow),
        "truncate" if integer_layout(target).is_some_and(|(w, _)| w <= value.width()) => {
            Ok(Some(conversion::truncate(value, target)?))
        }
        _ => Err(ScalarError::InvalidOperation),
    }
}
/// The named `_add`, `_sub` and `_mul` operations; `None` is `checked_`'s overflow.
pub fn arith(name: &str, a: Integer, b: Integer) -> Result<Option<Integer>, ScalarError> {
    if a.kind() != b.kind() {
        return Err(ScalarError::InvalidOperation);
    }
    let (mode, op) = name.split_once('_').ok_or(ScalarError::InvalidOperation)?;
    let (op, wrapped) = match op {
        "add" => (BinaryOp::Add, a.bits().wrapping_add(b.bits())),
        "sub" => (BinaryOp::Sub, a.bits().wrapping_sub(b.bits())),
        "mul" => (BinaryOp::Mul, a.bits().wrapping_mul(b.bits())),
        _ => return Err(ScalarError::InvalidOperation),
    };
    let checked = || match scalar::binary(op, Value::Integer(a), Value::Integer(b))? {
        Value::Integer(i) => Ok(i),
        _ => unreachable!(),
    };
    match mode {
        "wrapping" => Ok(Some(
            Integer::new(a.kind(), wrapped & mask(a.width())).unwrap(),
        )),
        "checked" => match checked() {
            Ok(v) => Ok(Some(v)),
            Err(ScalarError::Overflow) => Ok(None),
            Err(e) => Err(e),
        },
        "saturating" if op != BinaryOp::Mul => Ok(Some(match checked() {
            Ok(v) => v,
            Err(ScalarError::Overflow) => {
                // Addition overflows toward the right operand's sign, subtraction away
                // from it; an unsigned difference only below zero.
                let bits = if a.is_signed() {
                    let (min, max) = signed_bounds(a.width());
                    if (b.signed() < 0) == (op == BinaryOp::Add) {
                        min as u128
                    } else {
                        max as u128
                    }
                } else if op == BinaryOp::Sub {
                    0
                } else {
                    mask(a.width())
                };
                Integer::new(a.kind(), bits & mask(a.width())).unwrap()
            }
            Err(e) => return Err(e),
        })),
        "unchecked" if op == BinaryOp::Add => checked().map(Some), // Valid Unsafe calls must satisfy their documented fit contract.
        _ => Err(ScalarError::InvalidOperation),
    }
}
/// The bit counts and reversals of a two's complement word at the kind's width.
pub fn bits(name: &str, a: Integer) -> Result<Integer, ScalarError> {
    let width = a.width();
    let word = a.bits() & mask(width);
    let high = word << (128 - width);
    let bits = match name {
        "leading_zeros" => u128::from(high.leading_zeros().min(width)),
        "trailing_zeros" => u128::from(word.trailing_zeros().min(width)),
        "count_ones" => u128::from(word.count_ones()),
        "swap_bytes" => high.swap_bytes(),
        "reverse_bits" => high.reverse_bits(),
        _ => return Err(ScalarError::InvalidOperation),
    };
    Integer::new(a.kind(), bits).ok_or(ScalarError::InvalidOperation)
}
pub fn divide(name: &str, a: Integer, b: Integer) -> Result<Integer, ScalarError> {
    if a.kind() != b.kind() {
        return Err(ScalarError::InvalidOperation);
    }
    if b.bits() == 0 {
        return Err(ScalarError::DivisionByZero);
    }
    let bits = if a.is_signed() {
        let (x, y) = (a.signed(), b.signed());
        let (min, _) = signed_bounds(a.width());
        // Unlike operator %, all three library operations reject MIN/-1 (docs/spec/core-library.md).
        if x == min && y == -1 {
            return Err(ScalarError::Overflow);
        }
        let (q, r) = (x / y, x % y);
        let adjust = r != 0 && (r < 0) != (y < 0);
        (match name {
            "div_floor" => {
                if adjust {
                    q - 1
                } else {
                    q
                }
            }
            "mod" => {
                if adjust {
                    r + y
                } else {
                    r
                }
            }
            "div_exact" if r == 0 => q,
            "div_exact" => return Err(ScalarError::InvalidOperation),
            _ => return Err(ScalarError::InvalidOperation),
        }) as u128
    } else {
        match name {
            "div_floor" => a.bits() / b.bits(),
            "mod" => a.bits() % b.bits(),
            "div_exact" if a.bits() % b.bits() == 0 => a.bits() / b.bits(),
            _ => return Err(ScalarError::InvalidOperation),
        }
    };
    Ok(Integer::new(a.kind(), bits & mask(a.width())).unwrap())
}
