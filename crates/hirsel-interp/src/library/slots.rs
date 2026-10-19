//! Generational slots. Generation zero is reserved for retired vacancies.
use super::{none, some, u64_value};
use crate::{Error, OperationResult, Value, invalid};
use hirsel_types::BuiltinType;

fn number(v: &Value) -> Result<u64, Error> {
    match v {
        Value::Integer(n) if n.kind() == hirsel_types::Primitive::U64 => Ok(n.bits() as u64),
        _ => Err(invalid("slot word")),
    }
}
fn pair(v: &Value) -> Result<(&Value, &Value), Error> {
    match v {
        Value::Tuple(f) if f.len() == 2 => Ok((
            f[0].as_ref().ok_or_else(|| invalid("slot generation"))?,
            f[1].as_ref().ok_or_else(|| invalid("slot value"))?,
        )),
        _ => Err(invalid("slot pair")),
    }
}
fn tuple(a: Value, b: Value) -> Value {
    Value::Tuple(vec![Some(a), Some(b)])
}
pub(super) fn call(name: &str, args: &[Value]) -> Result<OperationResult, Error> {
    let arity = if name == "slots.empty" { 0 } else { 2 };
    if args.len() != arity {
        return Err(invalid("slot operation arity"));
    }
    if name == "slots.empty" {
        return Ok(OperationResult::returned(Value::Sequence {
            kind: BuiltinType::Slots,
            elements: Default::default(),
            arena: None,
        }));
    }
    let Value::Sequence {
        kind: BuiltinType::Slots,
        elements,
        ..
    } = &args[0]
    else {
        return Err(invalid("Slots representation"));
    };
    if name == "slots.insert" {
        let mut elements = elements.clone();
        let mut reusable = None;
        for (index, entry) in elements.iter().enumerate() {
            let (generation, value) =
                pair(entry.as_ref().ok_or_else(|| invalid("dead slot entry"))?)?;
            if number(generation)? != 0 && matches!(value, Value::Variant { variant: 0, .. }) {
                reusable = Some(index);
                break;
            }
        }
        let index = reusable.unwrap_or(elements.len());
        let generation = if let Some(index) = reusable {
            number(pair(elements[index].as_ref().unwrap())?.0)?
        } else {
            1
        };
        let entry = Some(tuple(u64_value(generation), some(args[1].clone())));
        if reusable.is_some() {
            elements[index] = entry;
        } else {
            elements.push(entry);
        }
        return Ok(OperationResult::Complete {
            value: tuple(u64_value(index as u64), u64_value(generation)),
            arguments: vec![(
                0,
                Value::Sequence {
                    kind: BuiltinType::Slots,
                    elements,
                    arena: None,
                },
            )],
            consumed: vec![1],
            released: vec![],
        });
    }
    let (index, generation) = pair(&args[1])?;
    let index = usize::try_from(number(index)?).ok();
    let generation = number(generation)?;
    let entry = index.and_then(|i| elements.get(i)).and_then(Option::as_ref);
    let found = match entry.map(pair).transpose()? {
        Some((g, value)) if number(g)? == generation => Some((g, value)),
        _ => None,
    };
    let value = found.map(|(_, v)| v.clone()).unwrap_or_else(none);
    if name == "slots.get" {
        // A counted local result shares the stored value's arena Lists (MEM-6).
        value.share_arena_storage();
        return Ok(OperationResult::returned(value));
    }
    let mut elements = elements.clone();
    if let Some((g, _)) = found {
        let g = number(g)?;
        elements[index.unwrap()] = Some(tuple(u64_value(g.wrapping_add(1)), none()));
    }
    Ok(OperationResult::Complete {
        value,
        arguments: vec![(
            0,
            Value::Sequence {
                kind: BuiltinType::Slots,
                elements,
                arena: None,
            },
        )],
        consumed: vec![],
        released: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // A source program cannot execute 2^64 removals within the test budget.
    #[test]
    fn exhausted_generation_retires_the_vacancy() {
        let slots = Value::Sequence {
            kind: BuiltinType::Slots,
            elements: vec![Some(tuple(u64_value(u64::MAX - 1), some(u64_value(7))))].into(),
            arena: None,
        };
        let stale = tuple(u64_value(0), u64_value(u64::MAX - 1));
        let OperationResult::Complete {
            value,
            mut arguments,
            ..
        } = call("slots.remove", &[slots, stale.clone()]).unwrap()
        else {
            panic!("remove completion")
        };
        assert_eq!(value, some(u64_value(7)));
        let slots = arguments.remove(0).1;
        let OperationResult::Complete {
            value: last,
            mut arguments,
            ..
        } = call("slots.insert", &[slots, u64_value(9)]).unwrap()
        else {
            panic!("insert completion")
        };
        assert_eq!(last, tuple(u64_value(0), u64_value(u64::MAX)));
        let slots = arguments.remove(0).1;
        let OperationResult::Complete {
            value,
            mut arguments,
            ..
        } = call("slots.remove", &[slots, last.clone()]).unwrap()
        else {
            panic!("remove completion")
        };
        assert_eq!(value, some(u64_value(9)));
        let slots = arguments.remove(0).1;
        let OperationResult::Complete {
            value: fresh,
            mut arguments,
            ..
        } = call("slots.insert", &[slots, u64_value(11)]).unwrap()
        else {
            panic!("insert completion")
        };
        assert_eq!(fresh, tuple(u64_value(1), u64_value(1)));
        let slots = arguments.remove(0).1;
        for stale in [stale, last] {
            let OperationResult::Complete { value, .. } =
                call("slots.get", &[slots.clone(), stale]).unwrap()
            else {
                panic!("get completion")
            };
            assert_eq!(value, none());
        }
    }
}
