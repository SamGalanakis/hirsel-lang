//! Nominal runtime facts are supplied by the driver from declared signatures.
use crate::{Error, Value, invalid};
use hirsel_syntax::DefPath;
use hirsel_tir::{BodyId, Constructor};
use hirsel_types::{BuiltinType, TypeKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Nominal {
    pub definition: DefPath,
    pub resource: bool,
    pub distinct: bool,
    /// A resource's owner definition contains its drop body (revision 1).
    pub finalizer: Option<(DefPath, BodyId)>,
    pub fields: usize,
    /// The private representation's shape in its own store, which a bounded
    /// arena's logical charge reads (MEM-6); None for an opaque nominal.
    pub representation: Option<(hirsel_types::TypeStore, hirsel_types::TypeId)>,
}
#[derive(Clone, Debug, Default)]
pub struct RuntimeTypes {
    /// Declared snapshots used to prove and specialize codec schemas.
    pub serialization: std::collections::BTreeMap<DefPath, hirsel_types::SignatureRef>,
    /// Full for diagnostic clients; Effects for ordinary observed execution.
    pub trace_mode: crate::TraceMode,
    pub nominals: Vec<Nominal>,
    pub forget: Option<DefPath>,
    /// Present for checked execution; absent runs normally.
    pub checked: Option<crate::checked::CheckedRuntime>,
    /// The host's run budgets; the default bounds nothing.
    pub budgets: crate::RunBudgets,
}
impl RuntimeTypes {
    pub fn get(&self, path: &DefPath) -> Option<&Nominal> {
        self.nominals.iter().find(|n| n.definition == *path)
    }
}
pub(crate) fn construct(
    constructor: &Constructor,
    values: Vec<Option<Value>>,
    kind: &TypeKind,
    types: &RuntimeTypes,
    sizes: Vec<Option<u64>>,
) -> Result<Value, Error> {
    Ok(match constructor {
        Constructor::Tuple if values.is_empty() => Value::Unit,
        Constructor::Tuple => Value::Tuple(values),
        Constructor::Variant {
            definition,
            variant,
        } => {
            if matches!(kind, TypeKind::Named { .. }) && types.get(definition).is_none() {
                return Err(Error::Unavailable(
                    "nominal runtime metadata required".into(),
                ));
            }
            if types.get(definition).is_some_and(|n| n.resource) {
                Value::ResourceVariant {
                    sizes,
                    definition: definition.clone(),
                    variant: *variant,
                    fields: values,
                }
            } else {
                Value::Variant {
                    variant: *variant,
                    fields: values,
                }
            }
        }
        Constructor::Record(definition) => {
            if let Some(n) = types.get(definition) {
                if n.fields != values.len() {
                    return Err(invalid("nominal constructor arity"));
                }
                if n.resource {
                    Value::Resource {
                        sizes,
                        definition: definition.clone(),
                        fields: values,
                    }
                } else if n.distinct {
                    Value::Distinct {
                        definition: definition.clone(),
                        fields: values,
                    }
                } else {
                    Value::Record {
                        definition: definition.clone(),
                        fields: values,
                    }
                }
            } else {
                if matches!(kind, TypeKind::Named { .. }) {
                    return Err(Error::Unavailable(
                        "nominal runtime metadata required".into(),
                    ));
                }
                Value::Record {
                    definition: definition.clone(),
                    fields: values,
                }
            }
        }
        Constructor::Sequence => {
            let TypeKind::Builtin { kind, .. } = kind else {
                return Err(invalid("sequence requires builtin type"));
            };
            if !matches!(
                kind,
                BuiltinType::Array | BuiltinType::List | BuiltinType::Inline | BuiltinType::Simd
            ) {
                return Err(invalid("unsupported sequence type"));
            }
            Value::Sequence {
                kind: *kind,
                elements: values.into(),
                arena: None,
            }
        }
        Constructor::SortedMap { .. } => {
            if !matches!(
                kind,
                TypeKind::Builtin {
                    kind: BuiltinType::SortedMap,
                    ..
                }
            ) || values.len() != 1
                || values[0].is_none()
            {
                return Err(invalid(
                    "sorted map constructor requires Order and SortedMap type",
                ));
            }
            Value::Record {
                definition: DefPath::core("SortedMap"),
                fields: vec![
                    values.into_iter().next().unwrap(),
                    Some(Value::Sequence {
                        kind: BuiltinType::Array,
                        elements: Default::default(),
                        arena: None,
                    }),
                ],
            }
        }
        Constructor::Map => {
            if !matches!(
                kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Map,
                    ..
                }
            ) {
                return Err(invalid("map constructor requires Map type"));
            }
            if values.iter().any(|value| !matches!(value, Some(Value::Tuple(fields)) if fields.len() == 2 && fields.iter().all(Option::is_some))) {
                return Err(invalid("map constructor requires complete key/value entries"));
            }
            Value::map(values)
        }
        Constructor::Set => {
            if !matches!(
                kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Set,
                    ..
                }
            ) || values.iter().any(Option::is_none)
            {
                return Err(invalid("set constructor requires complete Set elements"));
            }
            Value::set(values)
        }
        Constructor::Box => {
            if values.len() != 1 {
                return Err(invalid("box arity"));
            }
            Value::Box(values)
        }
    })
}
/// Suppress finalizers and owned-field destruction, while freeing simulation storage.
pub(crate) fn forget(value: Value) -> Result<(), Error> {
    if !matches!(
        value,
        Value::Resource { .. } | Value::ResourceVariant { .. }
    ) {
        return Err(invalid("forget requires resource"));
    }
    let mut work = vec![value];
    while let Some(mut v) = work.pop() {
        if let Some(fields) = v.fields_mut() {
            work.extend(fields.iter_mut().filter_map(Option::take));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_collection_constructors_preserve_order_and_reject_partial_entries() {
        let runtime = RuntimeTypes::default();
        let map = TypeKind::Builtin {
            kind: BuiltinType::Map,
            arguments: vec![].into(),
        };
        let entries = vec![
            Some(Value::Tuple(vec![
                Some(Value::Bool(true)),
                Some(Value::Unit),
            ])),
            Some(Value::Tuple(vec![
                Some(Value::Bool(false)),
                Some(Value::Unit),
            ])),
        ];
        assert_eq!(
            construct(&Constructor::Map, entries.clone(), &map, &runtime, vec![]).unwrap(),
            Value::map(entries)
        );
        assert!(
            construct(
                &Constructor::Map,
                vec![Some(Value::Tuple(vec![Some(Value::Bool(true)), None]))],
                &map,
                &runtime,
                vec![]
            )
            .is_err()
        );
        let set = TypeKind::Builtin {
            kind: BuiltinType::Set,
            arguments: vec![].into(),
        };
        let elements = vec![Some(Value::Bool(true)), Some(Value::Bool(false))];
        assert_eq!(
            construct(&Constructor::Set, elements.clone(), &set, &runtime, vec![]).unwrap(),
            Value::set(elements)
        );
        assert!(construct(&Constructor::Set, vec![None], &set, &runtime, vec![]).is_err());
        assert!(construct(&Constructor::Map, vec![], &set, &runtime, vec![]).is_err());
        assert!(construct(&Constructor::Set, vec![], &map, &runtime, vec![]).is_err());
    }
}
