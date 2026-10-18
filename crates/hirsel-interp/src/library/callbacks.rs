//! Library continuations are data; callbacks run on the evaluator's frame stack.
use super::{Library, collection, none, some};
use crate::{CallbackWriteback, Error, OperationContext, OperationResult, Value, invalid};
use hirsel_types::{BuiltinType, Primitive};

pub(super) enum Pending {
    Pull {
        next: Value,
        maps: Vec<Value>,
    },
    Map {
        iterator: Value,
        maps: Vec<Value>,
    },
    Compare {
        by: Value,
        entries: Vec<Value>,
        key: Value,
        value: Value,
        lo: usize,
        hi: usize,
        index: usize,
    },
}
fn record(name: &str, fields: Vec<Value>) -> Value {
    Value::Record {
        definition: hirsel_syntax::DefPath::core(name),
        fields: fields.into_iter().map(Some).collect(),
    }
}
pub(super) fn iterator(state: Value, next: Value) -> Value {
    record("IterState", vec![state, next])
}
pub(super) fn mapped(it: Value, f: Value) -> Value {
    record("IterMap", vec![it, f])
}
pub(super) fn sorted(by: Value, entries: Vec<Value>) -> Value {
    record(
        "SortedMap",
        vec![by, collection::sequence(BuiltinType::Array, entries)],
    )
}
fn unpack(value: Value, name: &str) -> Result<Vec<Value>, Error> {
    let Value::Record { definition, fields } = value else {
        return Err(invalid("invalid library state"));
    };
    if definition != hirsel_syntax::DefPath::core(name) || fields.len() != 2 {
        return Err(invalid("invalid library state tag"));
    }
    fields
        .into_iter()
        .map(|v| v.ok_or_else(|| invalid("dead library state")))
        .collect()
}
fn option(value: Value) -> Result<Option<Value>, Error> {
    match value {
        Value::Variant { variant: 0, fields } if fields.is_empty() => Ok(None),
        Value::Variant {
            variant: 1,
            mut fields,
        } if fields.len() == 1 => fields
            .pop()
            .unwrap()
            .map(Some)
            .ok_or_else(|| invalid("dead Some")),
        _ => Err(invalid("iterator callback must return Option")),
    }
}
fn comparator(by: &Value) -> Result<Value, Error> {
    let Value::Record { definition, fields } = by else {
        return Err(invalid("expected Order dictionary"));
    };
    if *definition != hirsel_syntax::DefPath::core("Order") || fields.len() != 1 {
        return Err(invalid("invalid Order dictionary"));
    }
    fields[0].clone().ok_or_else(|| invalid("dead comparator"))
}
impl Library {
    fn schedule(
        &mut self,
        pending: Pending,
        callee: Value,
        arguments: Vec<Value>,
        mut_arguments: Vec<usize>,
        owned: Vec<Value>,
    ) -> OperationResult {
        let continuation = self.next_callback;
        self.next_callback += 1;
        self.callbacks.insert(continuation, pending);
        OperationResult::Call {
            callee,
            arguments,
            mut_arguments,
            writebacks: vec![],
            updates: vec![],
            owned,
            consumed: vec![],
            continuation,
        }
    }
    pub(super) fn next_iterator(&mut self, mut it: Value) -> Result<OperationResult, Error> {
        let mut maps = vec![];
        loop {
            if matches!(&it, Value::Record { definition, .. } if *definition == hirsel_syntax::DefPath::core("IterMap"))
            {
                let mut fields = unpack(it, "IterMap")?;
                maps.push(fields.pop().unwrap());
                it = fields.pop().unwrap();
            } else {
                break;
            }
        }
        if matches!(&it, Value::Record { definition, .. } if *definition == hirsel_syntax::DefPath::core("IterState"))
        {
            let mut fields = unpack(it, "IterState")?;
            let next = fields.pop().unwrap();
            let state = fields.pop().unwrap();
            let fields = vec![0; maps.len() + 1];
            let mut result = self.schedule(
                Pending::Pull {
                    next: next.clone(),
                    maps,
                },
                next,
                vec![state],
                vec![0],
                vec![],
            );
            let OperationResult::Call { writebacks, .. } = &mut result else {
                unreachable!()
            };
            writebacks.push(CallbackWriteback {
                parameter: 0,
                argument: 0,
                fields,
            });
            return Ok(result);
        }
        let mut items = collection::elements(&it, BuiltinType::Iter)?;
        let item = if items.is_empty() {
            None
        } else {
            Some(items.remove(0))
        };
        self.map_item(collection::sequence(BuiltinType::Iter, items), maps, item)
    }
    fn map_item(
        &mut self,
        mut it: Value,
        mut maps: Vec<Value>,
        item: Option<Value>,
    ) -> Result<OperationResult, Error> {
        for f in maps.iter().rev() {
            it = mapped(it, f.clone());
        }
        let Some(item) = item else {
            return Ok(OperationResult::Complete {
                consumed: vec![],
                released: vec![],
                value: none(),
                arguments: vec![(0, it)],
            });
        };
        if let Some(f) = maps.pop() {
            Ok(self.schedule_mapper(it, maps, f, item))
        } else {
            Ok(OperationResult::Complete {
                consumed: vec![],
                released: vec![],
                value: some(item),
                arguments: vec![(0, it)],
            })
        }
    }
    fn schedule_mapper(
        &mut self,
        iterator: Value,
        maps: Vec<Value>,
        callee: Value,
        item: Value,
    ) -> OperationResult {
        let saved = iterator.clone();
        let mut result = self.schedule(
            Pending::Map { iterator, maps },
            callee,
            vec![item.clone()],
            vec![],
            vec![item],
        );
        let OperationResult::Call { updates, .. } = &mut result else {
            unreachable!()
        };
        // The moved item belongs only to scheduler-owned cleanup, including on
        // callback failure/abandonment; restoration keeps the advanced iterator.
        updates.push((0, saved));
        result
    }
    pub(super) fn sorted_insert(&mut self, args: &[Value]) -> Result<OperationResult, Error> {
        let [by, map, key, value] = args else {
            return Err(invalid("sorted_map.insert arguments"));
        };
        let mut fields = unpack(map.clone(), "SortedMap")?;
        let entries = collection::elements(&fields.pop().unwrap(), BuiltinType::Array)?;
        // The dictionary brand is checked statically; runtime also keeps its value.
        if fields[0] != *by {
            return Err(invalid("sorted map comparator brand mismatch"));
        }
        let hi = entries.len();
        self.compare_next(by.clone(), entries, key.clone(), value.clone(), 0, hi)
    }
    fn compare_next(
        &mut self,
        by: Value,
        mut entries: Vec<Value>,
        key: Value,
        value: Value,
        lo: usize,
        hi: usize,
    ) -> Result<OperationResult, Error> {
        if lo == hi {
            entries.insert(lo, Value::Tuple(vec![Some(key), Some(value)]));
            return Ok(OperationResult::Complete {
                consumed: vec![2, 3],
                released: vec![],
                value: Value::Unit,
                arguments: vec![(1, sorted(by, entries))],
            });
        }
        // SEM-5, spec #94: the fixed binary-search probes are observable through failure.
        let index = lo + (hi - lo) / 2;
        let Value::Tuple(pair) = &entries[index] else {
            return Err(invalid("invalid sorted map entry"));
        };
        let existing = pair
            .first()
            .and_then(Option::as_ref)
            .ok_or_else(|| invalid("invalid sorted map key"))?
            .clone();
        let compare = comparator(&by)?;
        Ok(self.schedule(
            Pending::Compare {
                by,
                entries,
                key: key.clone(),
                value,
                lo,
                hi,
                index,
            },
            compare,
            vec![key, existing],
            vec![],
            vec![],
        ))
    }
    pub(super) fn resume_callback(
        &mut self,
        token: u64,
        result: OperationResult,
        _context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let pending = self
            .callbacks
            .remove(&token)
            .ok_or_else(|| invalid("unknown library continuation"))?;
        if let OperationResult::Failed(failure) = result {
            return Ok(OperationResult::Failed(failure));
        }
        match pending {
            Pending::Pull { next, maps } => {
                let OperationResult::Complete {
                    value, arguments, ..
                } = result
                else {
                    return Err(invalid("iterator next must preserve mut state"));
                };
                let mut arguments = arguments.into_iter();
                let Some((0, state)) = arguments.next() else {
                    return Err(invalid("iterator next must return mut state"));
                };
                if arguments.next().is_some() {
                    return Err(invalid("iterator mut parameter ordinal"));
                }
                self.map_item(iterator(state, next), maps, option(value)?)
            }
            Pending::Map { iterator, mut maps } => {
                let OperationResult::Complete { value: item, .. } = result else {
                    return Err(invalid("mapper result"));
                };
                if let Some(f) = maps.pop() {
                    Ok(self.schedule_mapper(iterator, maps, f, item))
                } else {
                    Ok(OperationResult::Complete {
                        consumed: vec![],
                        released: vec![],
                        value: some(item),
                        arguments: vec![(0, iterator)],
                    })
                }
            }
            Pending::Compare {
                by,
                mut entries,
                key,
                value,
                lo,
                hi,
                index,
            } => {
                let OperationResult::Complete {
                    value: Value::Integer(order),
                    ..
                } = result
                else {
                    return Err(invalid("comparator must return i32"));
                };
                if order.kind() != Primitive::I32 {
                    return Err(invalid("comparator width"));
                }
                if order.signed() > 0 {
                    return self.compare_next(by, entries, key, value, index + 1, hi);
                }
                if order.signed() < 0 {
                    return self.compare_next(by, entries, key, value, lo, index);
                }
                let old = collection::replace_entry_value(&mut entries[index], value)?;
                Ok(OperationResult::Complete {
                    consumed: vec![2, 3],
                    released: vec![old, key],
                    value: Value::Unit,
                    arguments: vec![(1, sorted(by, entries))],
                })
            }
        }
    }
}
