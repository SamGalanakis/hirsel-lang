//! Ordered value containers. Mutations write their `mut` owner in place.
use crate::value::KeyKind;
use crate::{Error, Integer, OperationResult, Value, invalid};
use hirsel_types::BuiltinType;
pub fn sequence(kind: BuiltinType, values: impl IntoIterator<Item = Value>) -> Value {
    let elements = values.into_iter().map(Some).collect();
    match kind {
        BuiltinType::Map => Value::map(elements),
        BuiltinType::Set => Value::set(elements),
        _ => Value::sequence(kind, elements),
    }
}
pub fn elements(value: &Value, kind: BuiltinType) -> Result<Vec<Value>, Error> {
    let elements = match (value, kind) {
        (
            Value::Map {
                entries: elements, ..
            },
            BuiltinType::Map,
        )
        | (
            Value::Set {
                entries: elements, ..
            },
            BuiltinType::Set,
        ) => elements,
        (
            Value::Sequence {
                kind: k, elements, ..
            },
            _,
        ) if *k == kind => elements,
        _ => return Err(invalid("wrong collection kind")),
    };
    elements
        .iter()
        .map(|v| v.clone().ok_or_else(|| invalid("dead collection element")))
        .collect()
}

pub fn len(value: &Value) -> Result<u64, Error> {
    match value {
        Value::Sequence { elements, .. }
        | Value::Map {
            entries: elements, ..
        }
        | Value::Set {
            entries: elements, ..
        } => Ok(elements.len() as u64),
        _ => Err(invalid("expected collection")),
    }
}
/// A collection updates its `mut` owner in place; every owned value the update
/// displaced travels to the operation completion, which releases it.
#[must_use]
pub struct Displaced(pub Vec<Value>);
impl Displaced {
    pub fn into_result(self, consumed: Vec<usize>) -> OperationResult {
        OperationResult::Complete {
            value: Value::Unit,
            arguments: vec![],
            consumed,
            released: self.0,
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
pub fn push(list: &mut Value, item: Value) -> Result<(), Error> {
    let Value::Sequence {
        kind: BuiltinType::List,
        elements,
        ..
    } = list
    else {
        return Err(invalid("list.push requires List"));
    };
    elements.push(Some(item));
    Ok(())
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
pub(super) fn map_index(map: &Value, key: &Value) -> Result<(Option<usize>, u64), KeyedError> {
    let Value::Map { entries, .. } = map else {
        return Err(invalid("map update requires Map").into());
    };
    entries.find_key(key, KeyKind::Map)
}
pub fn insert(map: &mut Value, key: Value, item: Value) -> Result<Displaced, KeyedError> {
    let Value::Map {
        entries: elements, ..
    } = map
    else {
        return Err(invalid("map.insert requires Map").into());
    };
    let (index, hash) = elements.find_key(&key, KeyKind::Map)?;
    if let Some(index) = index {
        // The taken key drops after the displaced value, as natively.
        let old = elements
            .map_value_mut(index)?
            .replace(item)
            .ok_or_else(|| invalid("dead map value"))?;
        return Ok(Displaced(vec![old, key]));
    }
    // The borrowed key is copied into the map: a share of its arena Lists (MEM-6).
    key.share_arena_storage();
    elements.push_keyed(
        Value::Tuple(vec![Some(key), Some(item)]),
        hash,
        KeyKind::Map,
    );
    Ok(Displaced(vec![]))
}
pub fn set_insert(set: &mut Value, item: Value) -> Result<Displaced, KeyedError> {
    let Value::Set {
        entries: elements, ..
    } = set
    else {
        return Err(invalid("set.insert requires Set").into());
    };
    let (index, hash) = elements.find_key(&item, KeyKind::Set)?;
    Ok(Displaced(if index.is_some() {
        vec![item]
    } else {
        elements.push_keyed(item, hash, KeyKind::Set);
        vec![]
    }))
}
/// The value stored under an equal key, as a semantic copy.
pub fn get(map: &Value, key: &Value) -> Result<Option<Value>, KeyedError> {
    let Value::Map {
        entries: elements, ..
    } = map
    else {
        return Err(invalid("map.get requires Map").into());
    };
    let Some(index) = elements.find_key(key, KeyKind::Map)?.0 else {
        return Ok(None);
    };
    let Some(Value::Tuple(pair)) = &elements[index] else {
        return Err(invalid("invalid map entry").into());
    };
    let value = pair[1].clone().ok_or_else(|| invalid("dead map value"))?;
    // A counted local result shares the stored value's arena Lists (MEM-6).
    value.share_arena_storage();
    Ok(Some(value))
}
pub fn contains(collection: &Value, key: &Value) -> Result<bool, KeyedError> {
    let found = match collection {
        Value::Map {
            entries: elements, ..
        } => elements.find_key(key, KeyKind::Map)?.0,
        Value::Set {
            entries: elements, ..
        } => elements.find_key(key, KeyKind::Set)?.0,
        _ => return Err(invalid("contains requires Map or Set").into()),
    };
    Ok(found.is_some())
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
                ..
            },
            Value::Sequence {
                kind: b,
                elements: y,
                ..
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
        (Value::Map { entries: x, .. }, Value::Map { entries: y, .. }) => fields(x, y),
        (Value::Set { entries: x, .. }, Value::Set { entries: y, .. }) => {
            x.len() == y.len()
                && x.iter()
                    .flatten()
                    .all(|a| y.iter().flatten().any(|b| data_equal(a, b)))
        }
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Char(a), Value::Char(b)) => a == b,
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

/// Ordered eviction transfers V and separately releases the stored key.
pub fn map_remove(map: &mut Value, key: &Value) -> Result<OperationResult, KeyedError> {
    let Value::Map {
        entries: elements, ..
    } = map
    else {
        return Err(invalid("map.remove requires Map").into());
    };
    let (result, released) = if let Some(index) = elements.find_key(key, KeyKind::Map)?.0 {
        let Some(Value::Tuple(mut pair)) = elements.remove(index) else {
            return Err(invalid("invalid map entry").into());
        };
        let stored_key = pair[0].take().ok_or_else(|| invalid("dead map key"))?;
        let item = pair[1].take().ok_or_else(|| invalid("dead map value"))?;
        (super::some(item), vec![stored_key])
    } else {
        (super::none(), vec![])
    };
    Ok(OperationResult::Complete {
        value: result,
        arguments: vec![],
        consumed: vec![],
        released,
    })
}
pub fn set_remove(set: &mut Value, key: &Value) -> Result<OperationResult, KeyedError> {
    let Value::Set {
        entries: elements, ..
    } = set
    else {
        return Err(invalid("set.remove requires Set").into());
    };
    let index = elements.find_key(key, KeyKind::Set)?.0;
    let released = if let Some(index) = index {
        vec![
            elements
                .remove(index)
                .ok_or_else(|| invalid("dead set element"))?,
        ]
    } else {
        vec![]
    };
    Ok(OperationResult::Complete {
        value: Value::Bool(index.is_some()),
        arguments: vec![],
        consumed: vec![],
        released,
    })
}
