//! IEEE binary32/64 named operations and a fixed-width exact superaccumulator.
use crate::{
    Value,
    value::{f32_value, f64_value},
};
use std::cmp::Ordering;

pub fn minmax(name: &str, a: Value, b: Value) -> Option<Value> {
    macro_rules! kernel {
        ($a:expr,$b:expr,$make:ident) => {{
            let (a, b) = ($a, $b);
            let number = name.ends_with("_number");
            let min = name.starts_with("min");
            let r = if a.is_nan() {
                if number { b } else { a }
            } else if b.is_nan() {
                if number { a } else { b }
            } else if a == b && a == 0.0 {
                if min {
                    if a.is_sign_negative() || b.is_sign_negative() {
                        -0.0
                    } else {
                        0.0
                    }
                } else if a.is_sign_positive() || b.is_sign_positive() {
                    0.0
                } else {
                    -0.0
                }
            } else if (a < b) == min {
                a
            } else {
                b
            };
            Some($make(r))
        }};
    }
    match (a, b) {
        (Value::F32(a), Value::F32(b)) => kernel!(f32::from_bits(a), f32::from_bits(b), f32_value),
        (Value::F64(a), Value::F64(b)) => kernel!(f64::from_bits(a), f64::from_bits(b), f64_value),
        _ => None,
    }
}
pub fn total_order(mut a: Value, mut b: Value) -> Option<i32> {
    a.canonicalize();
    b.canonicalize();
    let order = match (a, b) {
        (Value::F32(a), Value::F32(b)) => f32::from_bits(a).total_cmp(&f32::from_bits(b)),
        (Value::F64(a), Value::F64(b)) => f64::from_bits(a).total_cmp(&f64::from_bits(b)),
        _ => return None,
    };
    Some(match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    })
}
pub fn mul_add(a: Value, b: Value, c: Value) -> Option<Value> {
    match (a, b, c) {
        (Value::F32(a), Value::F32(b), Value::F32(c)) => Some(f32_value(
            f32::from_bits(a).mul_add(f32::from_bits(b), f32::from_bits(c)),
        )),
        (Value::F64(a), Value::F64(b), Value::F64(c)) => Some(f64_value(
            f64::from_bits(a).mul_add(f64::from_bits(b), f64::from_bits(c)),
        )),
        _ => None,
    }
}
/// Error ordinals are NotFinite=0, OutOfRange=1, independently of i32 bounds.
pub fn to_i32(value: &Value) -> Option<Result<i32, u32>> {
    let x = match value {
        Value::F32(b) => f32::from_bits(*b) as f64,
        Value::F64(b) => f64::from_bits(*b),
        _ => return None,
    };
    Some(if !x.is_finite() {
        Err(0)
    } else {
        let t = x.trunc();
        if t < i32::MIN as f64 || t > i32::MAX as f64 {
            Err(1)
        } else {
            Ok(t as i32)
        }
    })
}

/// `f32.bits`/`f64.bits` (docs/spec/core-library.md): the IEEE bits as the same-width unsigned integer, every NaN
/// read as the one observable NaN (SEM-2).
pub fn bits(value: &Value) -> Option<Value> {
    let mut value = value.clone();
    value.canonicalize();
    match value {
        Value::F32(bits) => Some(super::dispatch::u32_value(bits)),
        Value::F64(bits) => Some(super::dispatch::u64_value(bits)),
        _ => None,
    }
}
/// `f32.from_bits`/`f64.from_bits` (docs/spec/core-library.md): the float with the bits of a u32 or u64, a NaN
/// pattern giving the one observable NaN (SEM-2).
pub fn from_bits(value: crate::Integer) -> Option<Value> {
    use hirsel_types::Primitive;
    Some(match value.kind() {
        Primitive::U32 => f32_value(f32::from_bits(value.bits() as u32)),
        Primitive::U64 => f64_value(f64::from_bits(value.bits() as u64)),
        _ => return None,
    })
}
/// Integer-to-float conversion (docs/spec/core-library.md). Rust's integer `as` casts supply IEEE
/// round-to-nearest, ties-to-even, directly at the destination width.
pub fn from_int(value: crate::Integer, target: hirsel_types::Primitive) -> Option<Value> {
    use hirsel_types::Primitive;
    Some(match (target, value.is_signed()) {
        (Primitive::F32, true) => f32_value(value.signed() as f32),
        (Primitive::F32, false) => f32_value(value.bits() as f32),
        (Primitive::F64, true) => f64_value(value.signed() as f64),
        (Primitive::F64, false) => f64_value(value.bits() as f64),
        _ => return None,
    })
}
