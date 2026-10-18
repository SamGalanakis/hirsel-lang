//! Library continuations are data; callbacks run on the evaluator's frame stack.
use super::{Library, collection, none, some};
use crate::{CallbackWriteback, Error, OperationContext, OperationResult, Value, invalid};
use hirsel_types::{BuiltinType, Primitive};

pub(super) enum Comparison {
    Insert(Value),
    Get,
    Remove,
}

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
        storage: Option<crate::value::ArenaStorage>,
        key: Value,
        action: Comparison,
        lo: usize,
        hi: usize,
        index: usize,
    },
    MapUpdated {
        inserted: bool,
    },
    MapInitialized {
        key: Value,
        hash: u64,
    },
    /// `shared.with`: the body's result is the operation's.
    Read,
    /// `mapped.with`: the body's result is `Ok`, unless the check after it finds the
    /// mapped file truncated.
    Mapped {
        mapping: u64,
        input: Vec<u8>,
    },
}
fn record(name: &str, fields: Vec<Value>) -> Value {
    Value::Record {
        definition: (&hirsel_syntax::DefPath::core(name)).into(),
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
    if !definition.is_core(name) || fields.len() != 2 {
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
    if !definition.is_core("Order") || fields.len() != 1 {
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
    /// `shared.with(s, body)`: call `body` with a borrowed view of the frozen value.
    pub(super) fn shared_with(&mut self, args: &[Value]) -> Result<OperationResult, Error> {
        let [Value::Shared(root), body] = args else {
            return Err(invalid("shared.with arguments"));
        };
        let view = root.view()?;
        Ok(self.schedule(Pending::Read, body.clone(), vec![view], vec![], vec![]))
    }
    /// `mapped.with(d, m, body)`: record the view, the mapped identity's current bytes and
    /// zeros past their end, and call `body` with it; the check follows on resume.
    pub(super) fn mapped_with(
        &mut self,
        args: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        use super::simulation::Grant;
        let [d, m, body] = args else {
            return Err(invalid("mapped.with arguments"));
        };
        self.tasks.yield_now = true;
        let (tree, _, dir) = self.simulation.directory(d)?;
        let mapping = self.simulation.id(m, "Mapped")?;
        let Some(Grant::Mapped {
            tree: owner_tree,
            dir: owner,
            identity,
            length,
            ..
        }) = self.simulation.grants.get(&mapping).cloned()
        else {
            return Err(invalid("mapped.with grant"));
        };
        if owner_tree != tree || owner != dir {
            return Err(invalid("mapped.with directory"));
        }
        let contents = &self.simulation.file_contents[&identity];
        let mut view = contents[..contents.len().min(length as usize)].to_vec();
        view.resize(length as usize, 0);
        let view = Value::Bytes(view);
        let input = super::dispatch::encode_arguments(&args[..2]);
        self.simulation.record(
            "Dir",
            "mapped.view",
            input.clone(),
            super::dispatch::encode_value(&view),
            context,
        )?;
        Ok(self.schedule(
            Pending::Mapped { mapping, input },
            body.clone(),
            vec![view],
            vec![],
            vec![],
        ))
    }
    pub(super) fn next_iterator(&mut self, mut it: Value) -> Result<OperationResult, Error> {
        let mut maps = vec![];
        loop {
            if matches!(&it, Value::Record { definition, .. } if definition.is_core("IterMap")) {
                let mut fields = unpack(it, "IterMap")?;
                maps.push(fields.pop().unwrap());
                it = fields.pop().unwrap();
            } else {
                break;
            }
        }
        if matches!(&it, Value::Record { definition, .. } if definition.is_core("IterState")) {
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
    /// One lookup; hit callbacks borrow the existing value, including on failure.
    pub(super) fn map_edit(
        &mut self,
        args: &[Value],
        insert: bool,
    ) -> Result<OperationResult, collection::KeyedError> {
        let map = args
            .first()
            .ok_or_else(|| invalid("map update arguments"))?;
        let key = args.get(1).ok_or_else(|| invalid("map update key"))?;
        let (found, hash) = collection::map_index(map, key)?;
        if !insert && found.is_none() {
            return Ok(OperationResult::returned(Value::Bool(false)));
        }
        let mut prepared = map.clone();
        let count = collection::len(map)?;
        let charge = prepared
            .arena_storage_mut()
            .unwrap()
            .as_mut()
            .map(|storage| {
                let copied = if storage.shared() { count } else { 0 };
                let charge = (
                    storage.arena,
                    storage
                        .element
                        .saturating_mul(copied.saturating_add(u64::from(found.is_none()))),
                );
                if storage.shared() {
                    map.fields()
                        .unwrap()
                        .iter()
                        .flatten()
                        .for_each(Value::share_arena_storage);
                    *storage = crate::value::ArenaStorage::fresh(storage.arena, storage.element);
                }
                charge
            });
        let mut result = if let Some(index) = found {
            let value = prepared.fields().unwrap()[index]
                .as_ref()
                .and_then(Value::fields)
                .and_then(|pair| pair.get(1))
                .and_then(Option::as_ref)
                .ok_or_else(|| invalid("dead map value"))?
                .clone();
            let callee = args
                .get(if insert { 3 } else { 2 })
                .ok_or_else(|| invalid("map update callback"))?
                .clone();
            let mut result = self.schedule(
                Pending::MapUpdated { inserted: insert },
                callee,
                vec![value],
                vec![0],
                vec![],
            );
            let OperationResult::Call { writebacks, .. } = &mut result else {
                unreachable!()
            };
            writebacks.push(CallbackWriteback {
                parameter: 0,
                argument: 0,
                fields: vec![index, 1],
            });
            result
        } else {
            // Native insertion copies the unretained key into independent heap storage.
            let mut key = key.clone();
            let mut work = vec![&mut key];
            while let Some(value) = work.pop() {
                if let Some(storage) = value.arena_storage_mut() {
                    *storage = None;
                }
                if let Some(fields) = value.fields_mut() {
                    work.extend(fields.iter_mut().flatten());
                }
            }
            self.schedule(
                Pending::MapInitialized { key, hash },
                args.get(2)
                    .ok_or_else(|| invalid("map initializer"))?
                    .clone(),
                vec![],
                vec![],
                vec![],
            )
        };
        let OperationResult::Call { updates, .. } = &mut result else {
            unreachable!()
        };
        updates.push((0, prepared));
        Ok(match charge {
            Some((arena, bytes)) => OperationResult::ArenaCharge {
                arena,
                bytes,
                result: Box::new(result),
            },
            None => result,
        })
    }
    pub(super) fn sorted_insert(&mut self, args: &[Value]) -> Result<OperationResult, Error> {
        let [by, map, key, value] = args else {
            return Err(invalid("sorted_map.insert arguments"));
        };
        let mut fields = unpack(map.clone(), "SortedMap")?;
        let sequence = fields.pop().unwrap();
        let storage = sequence.arena_storage().cloned();
        let entries = collection::elements(&sequence, BuiltinType::Array)?;
        // The dictionary brand is checked statically; runtime also keeps its value.
        if fields[0] != *by {
            return Err(invalid("sorted map comparator brand mismatch"));
        }
        let hi = entries.len();
        self.compare_next(
            by.clone(),
            entries,
            storage,
            key.clone(),
            Comparison::Insert(value.clone()),
            0..hi,
        )
    }
    pub(super) fn sorted_access(
        &mut self,
        args: &[Value],
        remove: bool,
    ) -> Result<OperationResult, Error> {
        let [by, map, key] = args else {
            return Err(invalid("sorted map access arguments"));
        };
        let mut fields = unpack(map.clone(), "SortedMap")?;
        let sequence = fields.pop().unwrap();
        let storage = sequence.arena_storage().cloned();
        let entries = collection::elements(&sequence, BuiltinType::Array)?;
        if fields[0] != *by {
            return Err(invalid("sorted map comparator brand mismatch"));
        }
        let hi = entries.len();
        self.compare_next(
            by.clone(),
            entries,
            storage,
            key.clone(),
            if remove {
                Comparison::Remove
            } else {
                Comparison::Get
            },
            0..hi,
        )
    }
    fn compare_next(
        &mut self,
        by: Value,
        entries: Vec<Value>,
        storage: Option<crate::value::ArenaStorage>,
        key: Value,
        action: Comparison,
        range: std::ops::Range<usize>,
    ) -> Result<OperationResult, Error> {
        let (lo, hi) = (range.start, range.end);
        if lo == hi {
            return finish_comparison(by, entries, storage, key, action, None, lo);
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
                storage,
                key: key.clone(),
                action,
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
        arguments: &mut [Value],
        context: &mut OperationContext<'_>,
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
            Pending::MapUpdated { inserted } => Ok(OperationResult::returned(if inserted {
                Value::Unit
            } else {
                Value::Bool(true)
            })),
            Pending::MapInitialized { key, hash } => {
                let OperationResult::Complete { value, .. } = result else {
                    return Err(invalid("map initializer result"));
                };
                let Some(Value::Map { entries, .. }) = arguments.first_mut() else {
                    return Err(invalid("map initializer lost its mut argument"));
                };
                entries.push_keyed(
                    Value::Tuple(vec![Some(key), Some(value)]),
                    hash,
                    crate::value::KeyKind::Map,
                );
                Ok(OperationResult::returned(Value::Unit))
            }
            Pending::Mapped { mapping, input } => {
                let OperationResult::Complete {
                    value,
                    mut released,
                    ..
                } = result
                else {
                    return Ok(result);
                };
                self.tasks.yield_now = true;
                let Some(super::simulation::Grant::Mapped {
                    identity,
                    length,
                    truncated,
                    ..
                }) = self.simulation.grants.get_mut(&mapping)
                else {
                    return Err(invalid("mapped.with grant"));
                };
                *truncated |= (self.simulation.file_contents[identity].len() as u64) < *length;
                let check = if *truncated {
                    super::dispatch::error(super::dispatch::variant(10, vec![]))
                } else {
                    super::dispatch::ok(Value::Unit)
                };
                self.simulation.record(
                    "Dir",
                    "mapped.check",
                    input,
                    super::dispatch::encode_value(&check),
                    context,
                )?;
                // A truncated mapping's body result is dropped after the check.
                let value = match check {
                    Value::Variant { variant: 0, .. } => super::dispatch::ok(value),
                    error => {
                        released.push(value);
                        error
                    }
                };
                Ok(OperationResult::Complete {
                    value,
                    arguments: vec![],
                    consumed: vec![],
                    released,
                })
            }
            Pending::Read => match result {
                OperationResult::Complete {
                    value, released, ..
                } => Ok(OperationResult::Complete {
                    value,
                    arguments: vec![],
                    consumed: vec![],
                    released,
                }),
                other => Ok(other),
            },
            Pending::Compare {
                by,
                entries,
                storage,
                key,
                action,
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
                    return self.compare_next(by, entries, storage, key, action, index + 1..hi);
                }
                if order.signed() < 0 {
                    return self.compare_next(by, entries, storage, key, action, lo..index);
                }
                finish_comparison(by, entries, storage, key, action, Some(index), index)
            }
        }
    }
}

fn finish_comparison(
    by: Value,
    mut entries: Vec<Value>,
    mut storage: Option<crate::value::ArenaStorage>,
    key: Value,
    action: Comparison,
    found: Option<usize>,
    vacant: usize,
) -> Result<OperationResult, Error> {
    let mut consumed = vec![];
    let mut released = vec![];
    let writes = matches!(action, Comparison::Insert(_))
        || matches!(action, Comparison::Remove) && found.is_some();
    let charge = storage.as_mut().filter(|_| writes).map(|storage| {
        let adds = u64::from(matches!(action, Comparison::Insert(_)) && found.is_none());
        let copied = if storage.shared() {
            entries.len() as u64
        } else {
            0
        };
        let charge = (
            storage.arena,
            storage.element.saturating_mul(adds.saturating_add(copied)),
        );
        if storage.shared() {
            entries.iter().for_each(Value::share_arena_storage);
            *storage = crate::value::ArenaStorage::fresh(storage.arena, storage.element);
        }
        charge
    });
    let update = !matches!(action, Comparison::Get);
    let value = match action {
        Comparison::Insert(value) => {
            consumed.extend([2, 3]);
            if let Some(index) = found {
                released.push(collection::replace_entry_value(&mut entries[index], value)?);
                released.push(key);
            } else {
                // Stored key payloads keep their arena sharing metadata (MEM-6).
                key.share_arena_storage();
                entries.insert(vacant, Value::Tuple(vec![Some(key), Some(value)]));
            }
            Value::Unit
        }
        Comparison::Get => {
            if let Some(index) = found {
                let Value::Tuple(pair) = &entries[index] else {
                    return Err(invalid("invalid sorted map entry"));
                };
                let value = pair
                    .get(1)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| invalid("dead sorted map value"))?
                    .clone();
                // Lookup copies the stored value, including any arena Lists (MEM-6).
                value.share_arena_storage();
                some(value)
            } else {
                none()
            }
        }
        Comparison::Remove => {
            if let Some(index) = found {
                let Value::Tuple(mut pair) = entries.remove(index) else {
                    return Err(invalid("invalid sorted map entry"));
                };
                if pair.len() != 2 {
                    return Err(invalid("invalid sorted map pair"));
                }
                released.push(
                    pair[0]
                        .take()
                        .ok_or_else(|| invalid("dead sorted map key"))?,
                );
                some(
                    pair[1]
                        .take()
                        .ok_or_else(|| invalid("dead sorted map value"))?,
                )
            } else {
                none()
            }
        }
    };
    let mut updated = sorted(by, entries);
    *updated.arena_storage_mut().unwrap() = storage;
    let result = OperationResult::Complete {
        value,
        consumed,
        released,
        arguments: if update { vec![(1, updated)] } else { vec![] },
    };
    Ok(match charge {
        Some((arena, bytes)) => OperationResult::ArenaCharge {
            arena,
            bytes,
            result: Box::new(result),
        },
        None => result,
    })
}

pub(super) fn sorted_len(args: &[Value]) -> Result<Value, Error> {
    let [by, map] = args else {
        return Err(invalid("sorted map length arguments"));
    };
    let Value::Record { definition, fields } = map else {
        return Err(invalid("expected sorted map"));
    };
    if !definition.is_core("SortedMap") || fields.len() != 2 || fields[0].as_ref() != Some(by) {
        return Err(invalid("sorted map comparator brand mismatch"));
    }
    let entries = fields[1]
        .as_ref()
        .ok_or_else(|| invalid("dead sorted map entries"))?;
    Ok(super::u64_value(collection::len(entries)?))
}
