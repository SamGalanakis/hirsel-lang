//! Scalar operators defined by TIR; named library operations belong to the dispatcher.
use crate::value::{Integer, Value, f32_value, f64_value, signed_bounds};
use hirsel_syntax::{BinaryOp, UnaryOp};
use hirsel_tir::Constant;
use hirsel_types::{Primitive, TypeKind};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarError {
    Overflow,
    DivisionByZero,
    ShiftRange,
    InvalidOperation,
}

pub fn constant(value: &Constant, ty: &TypeKind) -> Result<Value, ScalarError> {
    use Constant::*;
    match (value, ty) {
        (Unit, TypeKind::Tuple(fields)) if fields.is_empty() => Ok(Value::Unit),
        (Bool(v), TypeKind::Primitive(Primitive::Bool)) => Ok(Value::Bool(*v)),
        (Integer(v), TypeKind::Primitive(k)) => crate::value::Integer::new(*k, *v)
            .map(Value::Integer)
            .ok_or(ScalarError::InvalidOperation),
        (F32(v), TypeKind::Primitive(Primitive::F32)) => Ok(f32_value(f32::from_bits(*v))),
        (F64(v), TypeKind::Primitive(Primitive::F64)) => Ok(f64_value(f64::from_bits(*v))),
        (String(v), TypeKind::Primitive(Primitive::String)) => Ok(Value::String(v.clone())),
        (Bytes(v), TypeKind::Primitive(Primitive::Bytes)) => Ok(Value::Bytes(v.clone())),
        _ => Err(ScalarError::InvalidOperation),
    }
}
fn signed_result(a: Integer, value: Option<i128>) -> Result<Value, ScalarError> {
    let value = value.ok_or(ScalarError::Overflow)?;
    let (min, max) = signed_bounds(a.width());
    if value < min || value > max {
        return Err(ScalarError::Overflow);
    }
    Ok(Value::Integer(a.masked(value as u128)))
}
fn unsigned_result(a: Integer, value: Option<u128>) -> Result<Value, ScalarError> {
    let value = value.ok_or(ScalarError::Overflow)?;
    if value > crate::value::mask(a.width()) {
        return Err(ScalarError::Overflow);
    }
    Ok(Value::Integer(a.masked(value)))
}
pub fn unary(op: UnaryOp, value: Value) -> Result<Value, ScalarError> {
    use UnaryOp::*;
    match (op, value) {
        (Neg, Value::Integer(a)) if a.is_signed() => signed_result(a, a.signed().checked_neg()),
        (Neg, Value::Integer(a)) => unsigned_result(a, 0u128.checked_sub(a.bits())),
        (BitNot, Value::Integer(a)) => Ok(Value::Integer(a.masked(!a.bits()))),
        (Not, Value::Bool(a)) => Ok(Value::Bool(!a)),
        (Neg, Value::F32(a)) => Ok(f32_value(-f32::from_bits(a))),
        (Neg, Value::F64(a)) => Ok(f64_value(-f64::from_bits(a))),
        _ => Err(ScalarError::InvalidOperation),
    }
}
fn comparison(op: BinaryOp, order: Option<Ordering>) -> Option<Value> {
    use BinaryOp::*;
    Some(Value::Bool(match op {
        Eq => order == Some(Ordering::Equal),
        Ne => order != Some(Ordering::Equal),
        Lt => order == Some(Ordering::Less),
        Le => matches!(order, Some(Ordering::Less | Ordering::Equal)),
        Gt => order == Some(Ordering::Greater),
        Ge => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        _ => return None,
    }))
}
pub fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, ScalarError> {
    use BinaryOp::*;
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) if a.kind() == b.kind() => {
            let order = if a.is_signed() {
                a.signed().cmp(&b.signed())
            } else {
                a.bits().cmp(&b.bits())
            };
            if let Some(result) = comparison(op, Some(order)) {
                return Ok(result);
            }
            if matches!(op, Shl | Shr) {
                // SEM-1 issue #32: bit-vector left shift, arithmetic signed right shift;
                // negative counts fail, MIN % -1 is the representable mathematical zero.
                if (b.is_signed() && b.signed() < 0) || b.bits() >= a.width() as u128 {
                    return Err(ScalarError::ShiftRange);
                }
                let count = b.bits() as u32;
                let bits = match op {
                    Shl => a.bits() << count,
                    Shr if a.is_signed() => (a.signed() >> count) as u128,
                    _ => a.bits() >> count,
                };
                return Ok(Value::Integer(a.masked(bits)));
            }
            if matches!(op, BitAnd | BitOr | BitXor) {
                return Ok(Value::Integer(a.masked(match op {
                    BitAnd => a.bits() & b.bits(),
                    BitOr => a.bits() | b.bits(),
                    _ => a.bits() ^ b.bits(),
                })));
            }
            if matches!(op, Div | Rem) && b.bits() == 0 {
                return Err(ScalarError::DivisionByZero);
            }
            if a.is_signed() {
                let (x, y) = (a.signed(), b.signed());
                signed_result(
                    a,
                    match op {
                        Add => x.checked_add(y),
                        Sub => x.checked_sub(y),
                        Mul => x.checked_mul(y),
                        Div => x.checked_div(y),
                        Rem if y == -1 => Some(0),
                        Rem => x.checked_rem(y),
                        _ => return Err(ScalarError::InvalidOperation),
                    },
                )
            } else {
                let (x, y) = (a.bits(), b.bits());
                unsigned_result(
                    a,
                    match op {
                        Add => x.checked_add(y),
                        Sub => x.checked_sub(y),
                        Mul => x.checked_mul(y),
                        Div => x.checked_div(y),
                        Rem => x.checked_rem(y),
                        _ => return Err(ScalarError::InvalidOperation),
                    },
                )
            }
        }
        (Value::F32(a), Value::F32(b)) => {
            let (a, b) = (f32::from_bits(a), f32::from_bits(b));
            if let Some(v) = comparison(op, a.partial_cmp(&b)) {
                return Ok(v);
            }
            Ok(f32_value(match op {
                Add => a + b,
                Sub => a - b,
                Mul => a * b,
                Div => a / b,
                Rem => a % b,
                _ => return Err(ScalarError::InvalidOperation),
            }))
        }
        (Value::F64(a), Value::F64(b)) => {
            let (a, b) = (f64::from_bits(a), f64::from_bits(b));
            if let Some(v) = comparison(op, a.partial_cmp(&b)) {
                return Ok(v);
            }
            Ok(f64_value(match op {
                Add => a + b,
                Sub => a - b,
                Mul => a * b,
                Div => a / b,
                Rem => a % b,
                _ => return Err(ScalarError::InvalidOperation),
            }))
        }
        (Value::Bool(a), Value::Bool(b)) => {
            comparison(op, Some(a.cmp(&b))).ok_or(ScalarError::InvalidOperation)
        }
        (Value::String(a), Value::String(b)) => {
            comparison(op, Some(a.as_bytes().cmp(b.as_bytes())))
                .ok_or(ScalarError::InvalidOperation)
        }
        (Value::Bytes(a), Value::Bytes(b)) => {
            comparison(op, Some(a.cmp(&b))).ok_or(ScalarError::InvalidOperation)
        }
        (Value::Unit, Value::Unit) => {
            comparison(op, Some(Ordering::Equal)).ok_or(ScalarError::InvalidOperation)
        }
        _ => Err(ScalarError::InvalidOperation),
    }
}
