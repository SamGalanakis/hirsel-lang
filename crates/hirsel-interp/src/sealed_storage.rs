//! Semantic retained allocations, independent of the evaluator's live slots.
//! Snapshots carry payload and source identity but acquire no language owners.
use crate::inventory::{ArenaInventory, Charge, ChargeRoot, heap_charge, value_receipt};
use crate::{Error, Value, aggregate::RuntimeTypes, invalid};
use hirsel_types::{BuiltinType, GenericArgument, TypeId, TypeKind};
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Retained {
    pub ty: TypeId,
    pub value: Value,
    pub source: crate::observation::Anchor,
}

#[derive(Default, Debug)]
pub(crate) struct Storage {
    arenas: BTreeMap<u64, ArenaInventory<Retained>>,
}

impl Storage {
    pub fn open(&mut self, arena: u64) -> Result<(), Error> {
        if self.arenas.contains_key(&arena) {
            return Err(invalid("arena identity reused"));
        }
        self.arenas.insert(arena, ArenaInventory::default());
        Ok(())
    }

    /// Record a construction, deep copy or builder update after its operands
    /// have entered custody. Last use, shallow copy and borrowing never call
    /// this method. Children enroll before parents so every outgoing edge has
    /// an explicit receipt. A unique update preserves its original source site.
    pub fn record(
        &mut self,
        runtime: &RuntimeTypes,
        ty: TypeId,
        value: &mut Value,
        source: &crate::observation::Anchor,
        replace: bool,
    ) -> Result<(), Error> {
        if !needs_recording(value, replace) {
            return Ok(());
        }
        let mut pending = vec![(Vec::<usize>::new(), ty, false)];
        while let Some((path, ty, exit)) = pending.pop() {
            let current = at_mut(value, &path)?;
            if !exit {
                // Independent frozen contents are not retained ordinary edges.
                if matches!(current, Value::Shared(_) | Value::FunctionReference(_)) {
                    continue;
                }
                // Existing children already own their retained outgoing edges.
                if (!path.is_empty() || !replace)
                    && current
                        .arena_storage()
                        .is_some_and(|s| s.logical_block.is_some())
                {
                    continue;
                }
                let children = children(runtime, ty, current)?;
                pending.push((path.clone(), ty, true));
                for (index, ty) in children.into_iter().rev() {
                    let mut child = path.clone();
                    child.push(index);
                    pending.push((child, ty, false));
                }
                continue;
            }
            if !current.is_block() {
                continue;
            }
            let Some(tag) = current.arena_storage() else {
                continue;
            };
            let (arena, previous) = (tag.arena, tag.logical_block);
            if previous.is_some() && !(replace && path.is_empty()) {
                continue;
            }
            let inventory = self
                .arenas
                .get_mut(&arena)
                .ok_or_else(|| invalid("allocation into an ended arena grant"))?;
            if let Some(original) = current.arena_storage().and_then(|s| s.copied_from) {
                if !inventory.entries().contains_key(&original) {
                    return Err(invalid("COW of an unknown arena allocation"));
                }
                current.acquire_payload_roots();
            }
            let block = if let Some(block) = previous {
                block
            } else {
                let block = inventory.allocate(
                    Retained {
                        ty,
                        value: Value::Unit,
                        source: source.clone(),
                    },
                    Charge::ZERO,
                );
                current.enroll_block(block)?;
                block
            };
            let charge = heap_charge(
                runtime,
                ty,
                current,
                arena,
                &value_receipt(current)?,
                ChargeRoot::RetainedPayload,
            )?;
            let source = inventory
                .entries()
                .get(&block)
                .ok_or_else(|| invalid("unknown retained allocation"))?
                .payload
                .source
                .clone();
            let old = inventory
                .replace(
                    block,
                    Retained {
                        ty,
                        value: current.clone(),
                        source,
                    },
                    charge,
                )
                .map_err(|_| invalid("replacing an unknown retained allocation"))?;
            old.value.discard();
        }
        Ok(())
    }

    pub fn seal_charge(
        &self,
        runtime: &RuntimeTypes,
        arena: u64,
        ty: TypeId,
        root: &Value,
    ) -> Result<Charge, Error> {
        let inventory = self
            .arenas
            .get(&arena)
            .ok_or_else(|| invalid("consumed arena grant"))?;
        let extra = heap_charge(
            runtime,
            ty,
            root,
            arena,
            &value_receipt(root)?,
            ChargeRoot::Value,
        )?;
        Ok(inventory.seal_charge(extra))
    }

    /// Refresh only the containers on a projected write's path. Sibling
    /// materialized views keep their original retained extents.
    pub fn record_path(
        &mut self,
        runtime: &RuntimeTypes,
        root_ty: TypeId,
        root: &mut Value,
        path: &[usize],
        source: &crate::observation::Anchor,
    ) -> Result<(), Error> {
        let mut ancestors = vec![(vec![], root_ty)];
        let mut ty = root_ty;
        for (depth, index) in path.iter().enumerate() {
            let parent = at_mut(root, &path[..depth])?;
            ty = children(runtime, ty, parent)?
                .into_iter()
                .find(|(i, _)| i == index)
                .map(|(_, ty)| ty)
                .ok_or_else(|| invalid("typed retained update path"))?;
            ancestors.push((path[..=depth].to_vec(), ty));
        }
        for (path, ty) in ancestors.into_iter().rev() {
            self.record(runtime, ty, at_mut(root, &path)?, source, true)?;
        }
        Ok(())
    }

    pub fn close(&mut self, arena: u64) -> Option<ArenaInventory<Retained>> {
        self.arenas.remove(&arena)
    }

    pub fn logical_bytes(&self) -> u64 {
        self.arenas
            .values()
            .flat_map(|a| a.entries().values())
            .map(|entry| entry.payload.value.logical_bytes())
            .fold(0, u64::saturating_add)
    }

    /// Move, never clone, the retained ownership obligation at close/commit.
    pub fn take(&mut self, arena: u64) -> Result<ArenaInventory<Retained>, Error> {
        self.arenas
            .remove(&arena)
            .ok_or_else(|| invalid("consumed arena grant"))
    }
}

fn needs_recording(value: &Value, replace: bool) -> bool {
    let mut pending = vec![(value, true)];
    while let Some((value, root)) = pending.pop() {
        if let Some(storage) = value.arena_storage().filter(|_| value.is_block()) {
            if storage.logical_block.is_none() || root && replace {
                return true;
            }
            continue;
        }
        pending.extend(
            value
                .fields()
                .into_iter()
                .flatten()
                .flatten()
                .map(|value| (value, false)),
        );
    }
    false
}

fn at_mut<'a>(mut value: &'a mut Value, path: &[usize]) -> Result<&'a mut Value, Error> {
    for &index in path {
        value = value
            .fields_mut()
            .and_then(|fields| fields.get_mut(index))
            .and_then(Option::as_mut)
            .ok_or_else(|| invalid("retained payload path"))?;
    }
    Ok(value)
}

/// Close nominal representation parameters before retaining a child type.
/// Callable headers remain interfaces; environment metadata supplies owned types.
fn shape(runtime: &RuntimeTypes, mut ty: TypeId) -> Result<TypeKind, Error> {
    let mut seen = std::collections::HashSet::new();
    loop {
        if !seen.insert(ty) {
            return Err(invalid("cyclic inline retained representation"));
        }
        let kind = runtime.types.row(ty).kind.clone();
        let TypeKind::Named {
            definition,
            arguments,
        } = &kind
        else {
            return Ok(kind);
        };
        let signature = runtime.serialization.get(definition.path());
        let nominal = runtime
            .get(definition.path())
            .ok_or_else(|| invalid("missing retained nominal representation"))?;
        let representation = nominal
            .representation
            .ok_or_else(|| invalid("opaque retained payload lacks representation"))?;
        let default_roles = hirsel_types::NominalRoles::default();
        let roles = signature
            .map(|s| &s.metadata.roles)
            .unwrap_or(&default_roles);
        ty = hirsel_types::substitute(
            &runtime.types,
            &runtime.memo,
            representation,
            &hirsel_types::Substitution {
                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                roles,
                arguments,
                authorities: &vec![hirsel_types::AuthoritySummary::Unknown; arguments.len()],
            },
        )
        .map_err(|_| invalid("unclosed retained nominal payload"))?;
    }
}

fn children(
    runtime: &RuntimeTypes,
    ty: TypeId,
    value: &Value,
) -> Result<Vec<(usize, TypeId)>, Error> {
    let kind = shape(runtime, ty)?;
    let present = |types: Vec<TypeId>| -> Result<Vec<(usize, TypeId)>, Error> {
        let fields = value
            .fields()
            .ok_or_else(|| invalid("retained payload fields"))?;
        if fields.len() != types.len() {
            return Err(invalid("retained payload field count"));
        }
        Ok(types
            .into_iter()
            .enumerate()
            .filter(|(i, _)| fields[*i].is_some())
            .collect())
    };
    match kind {
        TypeKind::Primitive(_) => Ok(vec![]),
        TypeKind::Tuple(types) => {
            if types.is_empty() && matches!(value, Value::Unit) {
                Ok(vec![])
            } else {
                present(types.to_vec())
            }
        }
        TypeKind::Record(fields) => present(fields.iter().map(|f| f.ty).collect()),
        TypeKind::Variant(variants) => {
            let (Value::Variant { variant, .. } | Value::ResourceVariant { variant, .. }) = value
            else {
                return Err(invalid("retained active variant"));
            };
            present(
                variants
                    .get(*variant as usize)
                    .ok_or_else(|| invalid("retained variant ordinal"))?
                    .fields
                    .iter()
                    .map(|(_, ty)| *ty)
                    .collect(),
            )
        }
        TypeKind::Box(payload) => present(vec![payload]),
        TypeKind::Function(_) => match value {
            Value::Closure {
                captures,
                environment,
                ..
            } => {
                if environment.capture_types.len() != captures.len()
                    || environment.owned_captures.len() != captures.len()
                {
                    return Err(invalid("incomplete retained environment"));
                }
                Ok(environment
                    .capture_types
                    .iter()
                    .enumerate()
                    .filter_map(|(i, ty)| {
                        (captures[i].is_some() && environment.owned_captures[i]).then_some((i, *ty))
                    })
                    .collect())
            }
            Value::FunctionReference(_) => Ok(vec![]),
            _ => Err(invalid("retained callable payload")),
        },
        TypeKind::Builtin {
            kind: BuiltinType::Shared,
            ..
        } => Ok(vec![]),
        TypeKind::Builtin {
            kind: BuiltinType::Packed,
            arguments,
        } => {
            let Some(GenericArgument::Type(payload)) = arguments.first() else {
                return Err(invalid("retained Packed payload"));
            };
            children(runtime, *payload, value)
        }
        TypeKind::Builtin {
            kind:
                BuiltinType::List
                | BuiltinType::Array
                | BuiltinType::Set
                | BuiltinType::Inline
                | BuiltinType::Simd,
            arguments,
        } => {
            let Some(GenericArgument::Type(element)) = arguments.first() else {
                return Err(invalid("retained sequence element"));
            };
            let fields = value
                .fields()
                .ok_or_else(|| invalid("retained sequence fields"))?;
            Ok(fields
                .iter()
                .enumerate()
                .filter_map(|(i, v)| v.is_some().then_some((i, *element)))
                .collect())
        }
        TypeKind::Builtin {
            kind: BuiltinType::SortedMap,
            arguments,
        } => {
            let [
                GenericArgument::Type(key),
                GenericArgument::Type(item),
                GenericArgument::Type(comparator),
                ..,
            ] = arguments.as_ref()
            else {
                return Err(invalid("retained SortedMap arguments"));
            };
            let map = runtime
                .types
                .intern(hirsel_types::Type::with_kind(TypeKind::Builtin {
                    kind: BuiltinType::Map,
                    arguments: vec![GenericArgument::Type(*key), GenericArgument::Type(*item)]
                        .into(),
                }));
            present(vec![*comparator, map])
        }
        TypeKind::Builtin {
            kind: BuiltinType::Map,
            arguments,
        } => {
            let [GenericArgument::Type(key), GenericArgument::Type(item), ..] = arguments.as_ref()
            else {
                return Err(invalid("retained Map arguments"));
            };
            let pair = runtime
                .types
                .intern(hirsel_types::Type::with_kind(TypeKind::Tuple(
                    vec![*key, *item].into(),
                )));
            let fields = value
                .fields()
                .ok_or_else(|| invalid("retained Map entries"))?;
            Ok(fields
                .iter()
                .enumerate()
                .filter_map(|(i, v)| v.is_some().then_some((i, pair)))
                .collect())
        }
        _ => Err(invalid("unsupported or unclosed retained payload type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ArenaStorage;
    use hirsel_types::{Primitive, Type, TypeInterner};

    fn fixture() -> (RuntimeTypes, TypeId, TypeId, crate::observation::Anchor) {
        let runtime = RuntimeTypes::new(TypeInterner::new(None));
        let bytes = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
        let list = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(bytes)].into(),
        }));
        let scalar = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let source = crate::observation::Anchor {
            definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("arena.seal")),
            body: hirsel_tir::BodyId(0),
            site: Default::default(),
            cleanup: None,
        };
        (runtime, list, scalar, source)
    }
    fn list(count: usize) -> Value {
        Value::Sequence {
            kind: BuiltinType::List,
            elements: (0..count)
                .map(|_| Some(Value::Bytes(b"abc".to_vec())))
                .collect(),
            arena: Some(ArenaStorage::fresh(1, 8)),
        }
    }
    fn scalar() -> Value {
        Value::Integer(crate::Integer::new(Primitive::U64, 7).unwrap())
    }

    #[test]
    fn sealed_regions_33_recorder_retains_dead_blocks_and_avoids_root_double_charge() {
        let (runtime, list_ty, scalar_ty, source) = fixture();
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        let mut root = list(2);
        storage
            .record(&runtime, list_ty, &mut root, &source, false)
            .unwrap();
        let receipt = root.arena_storage().unwrap().logical_block;
        assert!(receipt.is_some());
        assert_eq!(
            storage.seal_charge(&runtime, 1, list_ty, &root).unwrap(),
            Charge::Bytes(6)
        );
        root.discard();
        assert_eq!(
            storage
                .seal_charge(&runtime, 1, scalar_ty, &scalar())
                .unwrap(),
            Charge::Bytes(6)
        );
        let retained = storage.take(1).unwrap();
        assert_eq!(retained.entries().len(), 1);
        assert_eq!(retained.entries()[&receipt.unwrap()].payload.ty, list_ty);
        assert!(storage.take(1).is_err());
    }

    #[test]
    fn sealed_regions_33_recorder_growth_replaces_but_cow_retains_both_payloads() {
        let (runtime, list_ty, _, source) = fixture();
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        let mut root = list(1);
        storage
            .record(&runtime, list_ty, &mut root, &source, false)
            .unwrap();
        let first = root.arena_storage().unwrap().logical_block;
        root.fields_mut()
            .unwrap()
            .push(Some(Value::Bytes(b"abc".to_vec())));
        storage
            .record(&runtime, list_ty, &mut root, &source, true)
            .unwrap();
        assert_eq!(root.arena_storage().unwrap().logical_block, first);
        assert_eq!(
            storage.seal_charge(&runtime, 1, list_ty, &root).unwrap(),
            Charge::Bytes(6)
        );
        let mut copied = root.clone();
        copied.retag_own_block(Some(ArenaStorage::fresh(1, 8)));
        copied.fields_mut().unwrap().pop();
        storage
            .record(&runtime, list_ty, &mut copied, &source, false)
            .unwrap();
        assert_ne!(copied.arena_storage().unwrap().logical_block, first);
        assert_eq!(
            storage.seal_charge(&runtime, 1, list_ty, &root).unwrap(),
            Charge::Bytes(9)
        );
        assert_eq!(storage.take(1).unwrap().entries().len(), 2);
    }

    #[test]
    fn sealed_regions_33_recorder_snapshots_do_not_acquire_shared_credits() {
        let (runtime, _, _, source) = fixture();
        let bytes = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
        let shared = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::Shared,
            arguments: vec![GenericArgument::Type(bytes)].into(),
        }));
        let ty = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(shared)].into(),
        }));
        let root = crate::value::SharedRoot::freeze(17, Value::Bytes(b"abc".to_vec()));
        let mut value = Value::Sequence {
            kind: BuiltinType::List,
            elements: vec![Some(Value::Shared(root.clone()))].into(),
            arena: Some(ArenaStorage::fresh(1, 8)),
        };
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        storage
            .record(&runtime, ty, &mut value, &source, false)
            .unwrap();
        assert_eq!(root.count(), 1);
        assert_eq!(
            storage.seal_charge(&runtime, 1, ty, &value).unwrap(),
            Charge::ZERO
        );
        let retained = storage.take(1).unwrap();
        assert_eq!(retained.entries().len(), 1);
        assert_eq!(root.count(), 1);
        value.discard();
        assert!(root.release().unwrap().is_some());
    }
}

#[cfg(test)]
mod custody_tests {
    use super::*;
    use crate::value::{ArenaStorage, SharedRoot};
    use hirsel_types::{Primitive, Type, TypeInterner};
    #[test]
    fn sealed_regions_33_enrolled_views_borrow_credits_and_cow_acquires_at_recording() {
        let runtime = RuntimeTypes::new(TypeInterner::new(None));
        let bytes = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
        let shared = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::Shared,
            arguments: vec![GenericArgument::Type(bytes)].into(),
        }));
        let list = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(shared)].into(),
        }));
        let source = crate::observation::Anchor {
            definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("arena.with")),
            body: hirsel_tir::BodyId(0),
            site: Default::default(),
            cleanup: None,
        };
        let root = SharedRoot::freeze(31, Value::Bytes(b"abc".to_vec()));
        let mut value = Value::Sequence {
            kind: BuiltinType::List,
            elements: vec![Some(Value::Shared(root.clone()))].into(),
            arena: Some(ArenaStorage::fresh(1, 8)),
        };
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        storage
            .record(&runtime, list, &mut value, &source, false)
            .unwrap();
        let mut view = value.copied();
        assert_eq!(
            root.count(),
            1,
            "an arena view owns no outgoing heap credit"
        );
        view.retag_own_block(Some(view.arena_storage().unwrap().detached()));
        assert_eq!(
            root.count(),
            1,
            "preflight has not accepted the COW payload"
        );
        storage
            .record(&runtime, list, &mut view, &source, false)
            .unwrap();
        assert_eq!(
            root.count(),
            2,
            "the two retained allocations each own one edge"
        );
        storage
            .record(&runtime, list, &mut view, &source, true)
            .unwrap();
        assert_eq!(
            root.count(),
            2,
            "replacing a snapshot is not an acquisition"
        );
        assert_eq!(storage.close(1).unwrap().entries().len(), 2);
        assert!(storage.close(1).is_none());
        value.discard();
        view.discard();
        assert!(root.release().unwrap().is_none());
        assert!(root.release().unwrap().is_some());
    }
}
