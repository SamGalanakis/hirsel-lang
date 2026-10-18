//! Ordered value containers. Mutations publish a new value through Operations.
use crate::{Error, Integer, OperationResult, Value, invalid};
use hirsel_types::BuiltinType;
pub fn sequence(kind: BuiltinType, values: impl IntoIterator<Item = Value>) -> Value {
    let elements = values.into_iter().map(Some).collect();
    match kind {
        BuiltinType::Map => Value::Map(elements),
        BuiltinType::Set => Value::Set(elements),
        _ => Value::Sequence { kind, elements },
    }
}
pub fn elements(value: &Value, kind: BuiltinType) -> Result<Vec<Value>, Error> {
    let elements = match (value, kind) {
        (Value::Map(elements), BuiltinType::Map) | (Value::Set(elements), BuiltinType::Set) => {
            elements
        }
        (Value::Sequence { kind: k, elements }, _) if *k == kind => elements,
        _ => return Err(invalid("wrong collection kind")),
    };
    elements
        .iter()
        .map(|v| v.clone().ok_or_else(|| invalid("dead collection element")))
        .collect()
}

pub fn len(value: &Value) -> Result<u64, Error> {
    if let Value::Sequence { elements, .. } = value {
        Ok(elements.len() as u64)
    } else {
        Err(invalid("expected sequence"))
    }
}
/// A collection update and every owned value it displaced travel together.
/// Callers can only publish through the operation completion ownership boundary.
#[must_use]
pub struct Mutation {
    value: Value,
    released: Vec<Value>,
}
impl Mutation {
    pub fn into_result(self, parameter: usize, consumed: Vec<usize>) -> OperationResult {
        OperationResult::Complete {
            value: Value::Unit,
            arguments: vec![(parameter, self.value)],
            consumed,
            released: self.released,
        }
    }
}
pub(super) fn replace_entry_value(entry: &mut Value, value: Value) -> Result<Value, Error> {
    let Value::Tuple(pair) = entry else {
        return Err(invalid("invalid map entry"));
    };
    if pair.len() != 2 {
        return Err(invalid("invalid map pair"));
    }
    pair[1]
        .replace(value)
        .ok_or_else(|| invalid("dead map value"))
}
pub fn push(mut value: Value, item: Value) -> Result<Mutation, Error> {
    if let Value::Sequence {
        kind: BuiltinType::List,
        elements,
    } = &mut value
    {
        elements.push(Some(item));
        Ok(Mutation {
            value,
            released: vec![],
        })
    } else {
        Err(invalid("list.push requires List"))
    }
}
/// Every keyed operation reads its key as a hashing engine would, so a key past
/// the structural depth limit is MEM-3 Depth whatever the container holds.
#[derive(Debug)]
pub enum KeyedError {
    Depth,
    Invalid(Error),
}
impl From<Error> for KeyedError {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}
fn entry_key(entry: &Value) -> Result<&Value, Error> {
    let Value::Tuple(pair) = entry else {
        return Err(invalid("invalid map entry"));
    };
    if pair.len() != 2 {
        return Err(invalid("invalid map pair"));
    }
    pair[0].as_ref().ok_or_else(|| invalid("dead map key"))
}
/// The one keyed lookup: insertion and keyed reads cannot disagree on which
/// element a key selects. `key_of` projects an element's key.
fn find(
    elements: &[Option<Value>],
    key: &Value,
    key_of: fn(&Value) -> Result<&Value, Error>,
) -> Result<Option<usize>, KeyedError> {
    match crate::structural::hash(key, 0) {
        Ok(_) => (),
        Err(crate::structural::StructuralError::Depth) => return Err(KeyedError::Depth),
        Err(crate::structural::StructuralError::InvalidValue) => {
            return Err(invalid("key is not key data").into());
        }
    }
    for (index, element) in elements.iter().enumerate() {
        let element = element
            .as_ref()
            .ok_or_else(|| invalid("dead collection element"))?;
        if data_equal(key_of(element)?, key) {
            return Ok(Some(index));
        }
    }
    Ok(None)
}
pub fn insert(mut value: Value, key: Value, item: Value) -> Result<Mutation, KeyedError> {
    let Value::Map(elements) = &mut value else {
        return Err(invalid("map.insert requires Map").into());
    };
    if let Some(index) = find(elements, &key, entry_key)? {
        // The taken key drops after the displaced value, as natively.
        let old = replace_entry_value(elements[index].as_mut().unwrap(), item)?;
        return Ok(Mutation {
            value,
            released: vec![old, key],
        });
    }
    elements.push(Some(Value::Tuple(vec![Some(key), Some(item)])));
    Ok(Mutation {
        value,
        released: vec![],
    })
}
pub fn set_insert(mut value: Value, item: Value) -> Result<Mutation, KeyedError> {
    let Value::Set(elements) = &mut value else {
        return Err(invalid("set.insert requires Set").into());
    };
    let released = if find(elements, &item, |member| Ok(member))?.is_some() {
        vec![item]
    } else {
        elements.push(Some(item));
        vec![]
    };
    Ok(Mutation { value, released })
}
/// The value stored under an equal key, as a semantic copy.
pub fn get(map: &Value, key: &Value) -> Result<Option<Value>, KeyedError> {
    let Value::Map(elements) = map else {
        return Err(invalid("map.get requires Map").into());
    };
    let Some(index) = find(elements, key, entry_key)? else {
        return Ok(None);
    };
    let Some(Value::Tuple(pair)) = &elements[index] else {
        return Err(invalid("invalid map entry").into());
    };
    Ok(Some(
        pair[1].clone().ok_or_else(|| invalid("dead map value"))?,
    ))
}
pub fn contains(set: &Value, value: &Value) -> Result<bool, KeyedError> {
    let Value::Set(elements) = set else {
        return Err(invalid("set.contains requires Set").into());
    };
    Ok(find(elements, value, |member| Ok(member))?.is_some())
}
/// Data equality uses IEEE float comparisons and structural declaration order.
pub fn data_equal(a: &Value, b: &Value) -> bool {
    fn fields(a: &[Option<Value>], b: &[Option<Value>]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => data_equal(a, b),
                _ => false,
            })
    }
    match (a, b) {
        (Value::F32(a), Value::F32(b)) => f32::from_bits(*a) == f32::from_bits(*b),
        (Value::F64(a), Value::F64(b)) => f64::from_bits(*a) == f64::from_bits(*b),
        (Value::Tuple(a), Value::Tuple(b)) | (Value::Box(a), Value::Box(b)) => fields(a, b),
        (
            Value::Variant {
                variant: a,
                fields: x,
            },
            Value::Variant {
                variant: b,
                fields: y,
            },
        ) => a == b && fields(x, y),
        (
            Value::Record {
                definition: a,
                fields: x,
            },
            Value::Record {
                definition: b,
                fields: y,
            },
        )
        | (
            Value::Distinct {
                definition: a,
                fields: x,
            },
            Value::Distinct {
                definition: b,
                fields: y,
            },
        ) => a == b && fields(x, y),
        (
            Value::Sequence {
                kind: a,
                elements: x,
            },
            Value::Sequence {
                kind: b,
                elements: y,
            },
        ) if a == b => {
            if *a == BuiltinType::Set {
                x.len() == y.len()
                    && x.iter()
                        .flatten()
                        .all(|a| y.iter().flatten().any(|b| data_equal(a, b)))
            } else {
                fields(x, y)
            }
        }
        (Value::Map(x), Value::Map(y)) => fields(x, y),
        (Value::Set(x), Value::Set(y)) => {
            x.len() == y.len()
                && x.iter()
                    .flatten()
                    .all(|a| y.iter().flatten().any(|b| data_equal(a, b)))
        }
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bytes(a), Value::Bytes(b)) => a == b,
        // Functions/capabilities/resources are not data, even if handles match.
        _ => false,
    }
}
pub fn order(a: Integer, b: Integer) -> Option<i32> {
    if a.kind() != b.kind() {
        return None;
    }
    let cmp = if a.is_signed() {
        a.signed().cmp(&b.signed())
    } else {
        a.bits().cmp(&b.bits())
    };
    Some(match cmp {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}
