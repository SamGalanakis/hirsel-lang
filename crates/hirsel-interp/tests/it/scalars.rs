use hirsel_interp::{
    Integer, Value,
    scalar::{self, ScalarError},
    value::{f32_value, f64_value, integer_layout},
};
use hirsel_syntax::{BinaryOp as B, UnaryOp as U};
use hirsel_tir::Constant;
use hirsel_types::{Primitive as P, TypeKind};
const KINDS: [P; 10] = [
    P::U8,
    P::U16,
    P::U32,
    P::U64,
    P::U128,
    P::I8,
    P::I16,
    P::I32,
    P::I64,
    P::I128,
];
fn mask(w: u32) -> u128 {
    u128::MAX >> (128 - w)
}
fn v(k: P, b: u128) -> Value {
    Value::Integer(Integer::new(k, b & mask(integer_layout(k).unwrap().0)).unwrap())
}
fn bin(k: P, op: B, a: u128, b: u128) -> Result<Value, ScalarError> {
    scalar::binary(op, v(k, a), v(k, b))
}

#[test]
fn integer_boundaries_cover_every_width_and_operator() {
    for k in KINDS {
        let (w, signed) = integer_layout(k).unwrap();
        let bits = mask(w);
        let max = if signed { bits >> 1 } else { bits };
        let min = if signed { 1u128 << (w - 1) } else { 0 };
        assert_eq!(bin(k, B::Add, max, 1), Err(ScalarError::Overflow), "{k:?}");
        assert_eq!(bin(k, B::Add, max - 1, 1), Ok(v(k, max)));
        assert_eq!(bin(k, B::Sub, min, 1), Err(ScalarError::Overflow));
        assert_eq!(bin(k, B::Sub, min + 1, 1), Ok(v(k, min)));
        assert_eq!(bin(k, B::Mul, max, 2), Err(ScalarError::Overflow));
        assert_eq!(bin(k, B::Mul, max / 2, 2), Ok(v(k, max / 2 * 2)));
        if signed {
            assert_eq!(scalar::unary(U::Neg, v(k, min)), Err(ScalarError::Overflow));
            assert_eq!(scalar::unary(U::Neg, v(k, min + 1)), Ok(v(k, max)));
            assert_eq!(bin(k, B::Div, min, bits), Err(ScalarError::Overflow));
            assert_eq!(bin(k, B::Rem, min, bits), Ok(v(k, 0))); // #32
            assert_eq!(
                bin(k, B::Div, (-7i128) as u128, 3),
                Ok(v(k, (-2i128) as u128))
            );
            assert_eq!(
                bin(k, B::Rem, (-7i128) as u128, 3),
                Ok(v(k, (-1i128) as u128))
            );
            assert_eq!(
                bin(k, B::Shr, min, 1),
                Ok(v(k, (Integer::new(k, min).unwrap().signed() >> 1) as u128))
            );
            assert_eq!(bin(k, B::Shl, 1, bits), Err(ScalarError::ShiftRange));
        } else {
            assert_eq!(scalar::unary(U::Neg, v(k, 1)), Err(ScalarError::Overflow));
            assert_eq!(bin(k, B::Div, max, 1), Ok(v(k, max)));
        }
        assert_eq!(scalar::unary(U::Neg, v(k, 0)), Ok(v(k, 0)));
        for op in [B::Div, B::Rem] {
            assert_eq!(bin(k, op, 1, 0), Err(ScalarError::DivisionByZero));
        }
        for op in [B::Shl, B::Shr] {
            for count in [w, w + 1] {
                assert_eq!(bin(k, op, 1, count as u128), Err(ScalarError::ShiftRange));
            }
            assert_eq!(bin(k, op, 1, 0), Ok(v(k, 1)));
        }
        assert_eq!(
            bin(k, B::Shl, 1, (w - 1) as u128),
            Ok(v(k, 1u128 << (w - 1)))
        );
        assert_eq!(bin(k, B::Shl, bits, 1), Ok(v(k, bits << 1))); // #32
        assert_eq!(scalar::unary(U::BitNot, v(k, 0)), Ok(v(k, bits)));
        assert_eq!(bin(k, B::BitAnd, 0xaa, 0x55), Ok(v(k, 0)));
        assert_eq!(bin(k, B::BitOr, 0xaa, 0x55), Ok(v(k, 0xff)));
        assert_eq!(bin(k, B::BitXor, 0xaa, 0xaa), Ok(v(k, 0)));
        for (op, expected) in [
            (B::Eq, false),
            (B::Ne, true),
            (B::Lt, true),
            (B::Le, true),
            (B::Gt, false),
            (B::Ge, false),
        ] {
            assert_eq!(bin(k, op, min, max), Ok(Value::Bool(expected)));
            assert_eq!(
                bin(k, op, 3, 3),
                Ok(Value::Bool(matches!(op, B::Eq | B::Le | B::Ge)))
            );
        }
    }
}
#[test]
fn literals_bool_utf8_bytes_and_unit() {
    assert_eq!(
        scalar::constant(&Constant::F32(0xff80_0001), &TypeKind::Primitive(P::F32)),
        Ok(f32_value(f32::NAN))
    );
    assert_eq!(
        scalar::constant(
            &Constant::F64(0xfff0_0000_0000_0001),
            &TypeKind::Primitive(P::F64)
        ),
        Ok(f64_value(f64::NAN))
    );
    assert_eq!(
        scalar::unary(U::Not, Value::Bool(false)),
        Ok(Value::Bool(true))
    );
    for (op, want) in [
        (B::Eq, false),
        (B::Ne, true),
        (B::Lt, true),
        (B::Le, true),
        (B::Gt, false),
        (B::Ge, false),
    ] {
        assert_eq!(
            scalar::binary(op, Value::Bool(false), Value::Bool(true)),
            Ok(Value::Bool(want))
        );
        assert_eq!(
            scalar::binary(op, Value::String("z".into()), Value::String("é".into())),
            Ok(Value::Bool(want))
        );
        assert_eq!(
            scalar::binary(op, Value::Bytes(vec![0, 0xff]), Value::Bytes(vec![1])),
            Ok(Value::Bool(want))
        );
    }
    assert_eq!(
        scalar::binary(
            B::Eq,
            Value::String("é".into()),
            Value::String("e\u{301}".into())
        ),
        Ok(Value::Bool(false))
    );
    for (c, kind, want) in [
        (Constant::Unit, TypeKind::Tuple(vec![].into()), Value::Unit),
        (
            Constant::Bool(true),
            TypeKind::Primitive(P::Bool),
            Value::Bool(true),
        ),
        (
            Constant::String("\u{202e}é😀".into()),
            TypeKind::Primitive(P::String),
            Value::String("\u{202e}é😀".into()),
        ),
        (
            Constant::Bytes(vec![0, 0xff, 0xc0]),
            TypeKind::Primitive(P::Bytes),
            Value::Bytes(vec![0, 0xff, 0xc0]),
        ),
    ] {
        assert_eq!(scalar::constant(&c, &kind), Ok(want));
    }
    assert_eq!(
        scalar::binary(B::Eq, Value::Unit, Value::Unit),
        Ok(Value::Bool(true))
    );
    assert!(Integer::new(P::U8, 256).is_none());
    assert!(Integer::new(P::F32, 0).is_none());
}
#[test]
fn operators_that_require_lowering_and_mixed_types_are_rejected() {
    for op in [B::And, B::Or, B::Range, B::RangeInclusive] {
        assert_eq!(bin(P::U8, op, 1, 2), Err(ScalarError::InvalidOperation));
    }
    assert_eq!(
        scalar::binary(B::Add, v(P::U8, 1), v(P::U16, 1)),
        Err(ScalarError::InvalidOperation)
    );
    for op in [U::Take, U::Copy, U::Mut, U::Unique, U::Box] {
        assert_eq!(
            scalar::unary(op, v(P::U8, 1)),
            Err(ScalarError::InvalidOperation)
        );
    }
}
