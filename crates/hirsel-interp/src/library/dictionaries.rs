//! The explicitly named integer Order comparators; no dictionary derivation.
use crate::{Error, Integer, OperationResult, Value, invalid};
use hirsel_types::Primitive;

pub(super) const INTEGER_ORDERS: &[(&str, Primitive)] = &[
    ("$intrinsic.OrderCompare_u8", Primitive::U8),
    ("$intrinsic.OrderCompare_u16", Primitive::U16),
    ("$intrinsic.OrderCompare_u32", Primitive::U32),
    ("$intrinsic.OrderCompare_u64", Primitive::U64),
    ("$intrinsic.OrderCompare_u128", Primitive::U128),
    ("$intrinsic.OrderCompare_i8", Primitive::I8),
    ("$intrinsic.OrderCompare_i16", Primitive::I16),
    ("$intrinsic.OrderCompare_i32", Primitive::I32),
    ("$intrinsic.OrderCompare_i64", Primitive::I64),
    ("$intrinsic.OrderCompare_i128", Primitive::I128),
];

pub(super) fn call(name: &str, arguments: &[Value]) -> Result<Option<OperationResult>, Error> {
    let Some((_, kind)) = INTEGER_ORDERS.iter().find(|(path, _)| *path == name) else {
        return Ok(None);
    };
    let kind = *kind;
    let [Value::Integer(a), Value::Integer(b)] = arguments else {
        return Err(invalid("integer Order comparator argument types"));
    };
    if a.kind() != kind || b.kind() != kind {
        return Err(invalid("integer Order comparator width"));
    }
    let order = crate::structural::compare(&arguments[0], &arguments[1])
        .map_err(|_| invalid("integer Order comparison"))?
        .ok_or_else(|| invalid("unordered integer comparison"))?;
    let result = match order {
        std::cmp::Ordering::Less => u32::MAX as u128,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    Ok(Some(OperationResult::returned(Value::Integer(
        Integer::new(Primitive::I32, result).unwrap(),
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_dictionaries_compare_every_width_with_signed_ordering() {
        for (name, kind) in [
            ("u8", Primitive::U8),
            ("u16", Primitive::U16),
            ("u32", Primitive::U32),
            ("u64", Primitive::U64),
            ("u128", Primitive::U128),
            ("i8", Primitive::I8),
            ("i16", Primitive::I16),
            ("i32", Primitive::I32),
            ("i64", Primitive::I64),
            ("i128", Primitive::I128),
        ] {
            let name = format!("$intrinsic.OrderCompare_{name}");
            let low = Value::Integer(Integer::new(kind, 0).unwrap());
            let high = Value::Integer(Integer::new(kind, 1).unwrap());
            for (a, b, result) in [
                (&low, &high, u32::MAX as u128),
                (&low, &low, 0),
                (&high, &low, 1),
            ] {
                assert!(
                    matches!(call(&name,&[a.clone(),b.clone()]).unwrap(),Some(OperationResult::Complete { value: Value::Integer(n), .. }) if n.kind()==Primitive::I32 && n.bits()==result)
                );
            }
            if name.contains("_i") {
                let minus_one = Value::Integer(
                    Integer::new(
                        kind,
                        crate::value::mask(crate::value::integer_layout(kind).unwrap().0),
                    )
                    .unwrap(),
                );
                assert!(
                    matches!(call(&name,&[minus_one,low]).unwrap(),Some(OperationResult::Complete { value: Value::Integer(n), .. }) if n.bits()==u32::MAX as u128)
                );
            }
        }
        assert!(
            call(
                "$intrinsic.OrderCompare_u32",
                &[Value::Bool(false), Value::Bool(true)]
            )
            .is_err()
        );
        assert!(
            call("$intrinsic.OrderCompare_float", &[])
                .unwrap()
                .is_none()
        );
    }
}
