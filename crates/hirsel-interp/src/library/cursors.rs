//! Compiler-only borrowed and consuming loop state with explicit capture ownership.
use super::{collection, none, some, u64_value};
use crate::{Error, OperationResult, Value, invalid};
use hirsel_tir::BodyId;
use hirsel_types::{BuiltinType, Primitive};
pub(super) const PROTOCOLS: &[&str] = &["SetElements", "UnicodeScalars", "ScalarPositions"];
const CONSUMING_PROTOCOLS: &[(&str, BuiltinType)] = &[
    ("MapInsertionOrder", BuiltinType::Map),
    ("SortedMapOrder", BuiltinType::SortedMap),
    ("ArrayConsume", BuiltinType::Array),
    ("ListConsume", BuiltinType::List),
    ("InlineConsume", BuiltinType::Inline),
];
#[derive(Clone, Copy)]
enum Step {
    Open,
    Next,
}
const STEPS: &[(&str, Step)] = &[("open", Step::Open), ("next", Step::Next)];
fn step(name: &str) -> Option<Step> {
    STEPS
        .iter()
        .find(|(path, _)| *path == name)
        .map(|(_, step)| *step)
}
fn entries(source: &Value, protocol: &str) -> Result<Vec<Value>, Error> {
    match protocol {
        "SetElements" => collection::elements(source, BuiltinType::Set),
        _ => collection::elements(source, BuiltinType::Array),
    }
}
pub(super) fn call(name: &str, args: &[Value]) -> Result<Option<OperationResult>, Error> {
    if name == "$intrinsic.MapInsertionOrder_advance" {
        let [source, index] = args else {
            return Err(invalid("map advance arguments"));
        };
        let count = collection::elements(source, BuiltinType::Map)?.len() as u64;
        let Value::Integer(index) = index else {
            return Err(invalid("map advance index"));
        };
        if index.kind() != Primitive::U64 {
            return Err(invalid("map advance index width"));
        }
        return Ok(Some(OperationResult::returned(u64_value(
            (index.bits() as u64).min(count),
        ))));
    }
    if let Some(result) = consuming(name, args)? {
        return Ok(Some(result));
    }
    let Some((protocol, operation)) = name
        .strip_prefix("$intrinsic.")
        .and_then(|s| s.rsplit_once('_'))
    else {
        return Ok(None);
    };
    if !PROTOCOLS.contains(&protocol) {
        return Ok(None);
    }
    let [source] = args else {
        return Err(invalid("cursor argument count"));
    };
    let Some(operation) = step(operation) else {
        return Ok(None);
    };
    match operation {
        Step::Open => {
            let values = entries(source, protocol)?;
            for v in &values {
                let valid = match protocol {
                    "UnicodeScalars" => {
                        matches!(v, Value::Char(_))
                    }
                    "ScalarPositions" => {
                        matches!(v,Value::Record {definition,..} if definition.is_core("StrPos"))
                    }
                    _ => true,
                };
                if !valid {
                    return Err(invalid("cursor source element type"));
                }
            }
            Ok(Some(OperationResult::returned(Value::Closure {
                invoked: Default::default(),
                definition: (&hirsel_syntax::DefPath::core(&format!(
                    "$intrinsic.{protocol}_cursor"
                )))
                    .into(),
                body: BodyId(0),
                captures: vec![Some(source.clone()), Some(u64_value(0))],
                environment: Box::new(crate::value::Environment {
                    owned_captures: vec![false, true],
                    sizes: Default::default(),
                    type_arguments: Default::default(),
                }),
            })))
        }
        Step::Next => {
            let Value::Closure {
                definition,
                body,
                captures,
                environment,
                ..
            } = source
            else {
                return Err(invalid("expected intrinsic cursor"));
            };
            if !definition.is_core(&format!("$intrinsic.{protocol}_cursor"))
                || *body != BodyId(0)
                || captures.len() != 2
                || environment.owned_captures != [false, true]
            {
                return Err(invalid("cursor protocol mismatch"));
            }
            let source = captures[0]
                .as_ref()
                .ok_or_else(|| invalid("dead cursor source"))?;
            let Some(Value::Integer(index)) = &captures[1] else {
                return Err(invalid("cursor index"));
            };
            if index.kind() != Primitive::U64 {
                return Err(invalid("cursor index width"));
            }
            let values = entries(source, protocol)?;
            let index = usize::try_from(index.bits()).map_err(|_| invalid("cursor index range"))?;
            if index > values.len() {
                return Err(invalid("cursor index past end"));
            }
            let value = values.get(index).cloned().map(some).unwrap_or_else(none);
            let next = if index == values.len() {
                index
            } else {
                index + 1
            };
            let cursor = Value::Closure {
                invoked: Default::default(),
                definition: *definition,
                body: *body,
                captures: vec![Some(source.clone()), Some(u64_value(next as u64))],
                environment: environment.clone(),
            };
            Ok(Some(OperationResult::Complete {
                consumed: vec![],
                released: vec![],
                value,
                arguments: vec![(0, cursor)],
            }))
        }
    }
}

/// The owned capture is the only owner of the remaining source elements. Each
/// successful next removes its slot before publishing the updated cursor.
fn consuming(name: &str, args: &[Value]) -> Result<Option<OperationResult>, Error> {
    let Some((protocol, operation)) = name
        .strip_prefix("$intrinsic.")
        .and_then(|s| s.rsplit_once('_'))
    else {
        return Ok(None);
    };
    let Some((_, kind)) = CONSUMING_PROTOCOLS
        .iter()
        .find(|(name, _)| *name == protocol)
    else {
        return Ok(None);
    };
    let kind = *kind;
    let [argument] = args else {
        return Err(invalid("owning cursor argument count"));
    };
    if operation == "len" && matches!(kind, BuiltinType::Map | BuiltinType::SortedMap) {
        let elements = source_elements(argument, kind)?;
        if elements.iter().any(Option::is_none) {
            return Err(invalid("dead map entry"));
        }
        return Ok(Some(OperationResult::returned(u64_value(
            elements.len() as u64
        ))));
    }
    let Some(operation) = step(operation) else {
        return Ok(None);
    };
    match operation {
        Step::Open => {
            let elements = source_elements(argument, kind)?;
            if !elements.iter().all(Option::is_some) || matches!(kind, BuiltinType::Map | BuiltinType::SortedMap) && !elements.iter().all(|e| matches!(e, Some(Value::Tuple(pair)) if pair.len()==2 && pair.iter().all(Option::is_some))) {
                return Err(invalid("owning cursor source"));
            }
            // MEM-6: an arena List is never unique, so consuming it copies each element
            // out, which the List still holds too.
            if let Value::Sequence { arena: Some(_), .. } = argument {
                argument.share_arena_storage();
            }
            Ok(Some(
                OperationResult::returned(Value::Closure {
                    invoked: Default::default(),
                    definition: (&hirsel_syntax::DefPath::core(&format!(
                        "$intrinsic.{protocol}_cursor"
                    )))
                        .into(),
                    body: BodyId(0),
                    captures: vec![Some(argument.clone()), Some(u64_value(0))],
                    environment: Box::new(crate::value::Environment {
                        owned_captures: vec![true, true],
                        sizes: Default::default(),
                        type_arguments: Default::default(),
                    }),
                })
                .transferring([0]),
            ))
        }
        Step::Next => {
            let Value::Closure {
                definition,
                body,
                captures,
                environment,
                ..
            } = argument
            else {
                return Err(invalid("expected owning cursor"));
            };
            if !definition.is_core(&format!("$intrinsic.{protocol}_cursor"))
                || *body != BodyId(0)
                || captures.len() != 2
                || environment.owned_captures != [true, true]
            {
                return Err(invalid("owning cursor protocol"));
            }
            let mut captures = captures.clone();
            let Some(Value::Integer(index)) = captures[1].as_ref() else {
                return Err(invalid("owning cursor index"));
            };
            if index.kind() != Primitive::U64 {
                return Err(invalid("owning cursor index width"));
            }
            let index =
                usize::try_from(index.bits()).map_err(|_| invalid("owning cursor index range"))?;
            let elements = owned_elements(
                captures[0]
                    .as_mut()
                    .ok_or_else(|| invalid("dead owning cursor source"))?,
                kind,
            )?;
            if index > elements.len() {
                return Err(invalid("owning cursor source state"));
            }
            let value = if index == elements.len() {
                none()
            } else {
                let item = elements[index]
                    .take()
                    .ok_or_else(|| invalid("owning cursor element already removed"))?;
                captures[1] = Some(u64_value(index as u64 + 1));
                some(item)
            };
            Ok(Some(OperationResult::Complete {
                consumed: vec![],
                released: vec![],
                value,
                arguments: vec![(
                    0,
                    Value::Closure {
                        invoked: Default::default(),
                        definition: *definition,
                        body: *body,
                        captures,
                        environment: environment.clone(),
                    },
                )],
            }))
        }
    }
}

fn source_elements(source: &Value, kind: BuiltinType) -> Result<&[Option<Value>], Error> {
    match (kind, source) {
        (BuiltinType::Map, Value::Map { entries, .. }) => Ok(entries),
        (BuiltinType::SortedMap, Value::Record { definition, fields })
            if definition.is_core("SortedMap") && fields.len() == 2 && fields[0].is_some() =>
        {
            match &fields[1] {
                Some(Value::Sequence {
                    kind: BuiltinType::Array,
                    elements,
                    ..
                }) => Ok(elements),
                _ => Err(invalid("SortedMap entry storage")),
            }
        }
        (expected, Value::Sequence { kind, elements, .. }) if expected == *kind => Ok(elements),
        _ => Err(invalid("owning cursor source layout")),
    }
}
fn owned_elements(source: &mut Value, kind: BuiltinType) -> Result<&mut Vec<Option<Value>>, Error> {
    if kind == BuiltinType::SortedMap {
        let Value::Record { definition, fields } = source else {
            return Err(invalid("SortedMap entry storage"));
        };
        if !definition.is_core("SortedMap") || fields.len() != 2 || fields[0].is_none() {
            return Err(invalid("SortedMap entry storage"));
        }
        return match &mut fields[1] {
            Some(Value::Sequence {
                kind: BuiltinType::Array,
                elements,
                ..
            }) => Ok(elements),
            _ => Err(invalid("SortedMap entry storage")),
        };
    }
    match source {
        Value::Map { entries, .. } if kind == BuiltinType::Map => Ok(entries),
        Value::Sequence {
            kind: actual,
            elements,
            ..
        } if kind == *actual => Ok(elements),
        _ => Err(invalid("owning cursor source layout")),
    }
}

pub(super) fn operation_names() -> Vec<String> {
    PROTOCOLS
        .iter()
        .copied()
        .chain(CONSUMING_PROTOCOLS.iter().map(|(name, _)| *name))
        .flat_map(|protocol| {
            STEPS
                .iter()
                .map(move |(operation, _)| format!("$intrinsic.{protocol}_{operation}"))
        })
        .chain([
            "$intrinsic.MapInsertionOrder_len".to_owned(),
            "$intrinsic.MapInsertionOrder_advance".to_owned(),
            "$intrinsic.SortedMapOrder_len".to_owned(),
        ])
        .collect()
}
