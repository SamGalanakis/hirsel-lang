//! Semantic retained allocations, independent of the evaluator's live slots.
//! Snapshots carry payload and source identity but acquire no language owners.
use crate::inventory::{ArenaInventory, Charge, ChargeRoot, heap_charge, value_receipt};
use crate::{Error, Value, aggregate::RuntimeTypes, invalid};
use hirsel_types::{BuiltinType, GenericArgument, TypeId, TypeKind};
use std::collections::BTreeMap;
mod owner;
pub(crate) use owner::RootLoan;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AllocationSite {
    pub anchor: crate::observation::Anchor,
    pub block: hirsel_tir::BlockId,
    pub statement: u32,
    pub path: Vec<u32>,
}
impl AllocationSite {
    fn below(&self, path: &[usize]) -> Result<Self, Error> {
        let mut source = self.clone();
        for &index in path {
            source.path.push(
                u32::try_from(index)
                    .map_err(|_| invalid("retained source field ordinal overflow"))?,
            );
        }
        Ok(source)
    }
    #[cfg(test)]
    fn test(anchor: crate::observation::Anchor, statement: u32) -> Self {
        Self {
            anchor,
            block: hirsel_tir::BlockId(0),
            statement,
            path: vec![],
        }
    }
}

#[derive(Debug)]
pub(crate) struct Retained {
    pub ty: TypeId,
    pub value: Value,
    pub source: AllocationSite,
}

#[derive(Default, Debug)]
pub(crate) struct Storage {
    arenas: BTreeMap<u64, ArenaInventory<Retained>>,
    owners: Vec<Option<owner::Owner>>,
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
        source: &AllocationSite,
        replace: bool,
    ) -> Result<(), Error> {
        if !needs_recording(value, replace) {
            return Ok(());
        }
        let mut pending = vec![(Vec::<usize>::new(), ty, false, replace)];
        while let Some((path, ty, exit, replace_here)) = pending.pop() {
            let current = at_mut(value, &path)?;
            if !exit {
                // Independent frozen contents are not retained ordinary edges.
                if matches!(current, Value::Shared(_) | Value::FunctionReference(_)) {
                    continue;
                }
                // Existing children already own their retained outgoing edges.
                if !replace_here && current.enrolled_arena_block() {
                    continue;
                }
                let children = children(runtime, ty, current)?;
                // Inline wrappers do not own a semantic block. A mutation
                // refreshes their first allocated descendants (SortedMap's
                // entry array, for example), then stops at that block boundary.
                let replace_children = replace_here && !current.is_block();
                pending.push((path.clone(), ty, true, replace_here));
                for (index, ty) in children.into_iter().rev() {
                    let mut child = path.clone();
                    child.push(index);
                    pending.push((child, ty, false, replace_children));
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
            if previous.is_some() && !replace_here {
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
                        source: source.below(&path)?,
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
        source: &AllocationSite,
    ) -> Result<(), Error> {
        if !root.contains_arena_storage() {
            return Ok(());
        }
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
            self.record(
                runtime,
                ty,
                at_mut(root, &path)?,
                &source.below(&path)?,
                true,
            )?;
        }
        Ok(())
    }

    /// A once invocation takes the environment's owned captures. Its retained
    /// allocation remains, with empty payload slots and no outgoing credits.
    pub fn environment_entry(&mut self, value: &Value) -> Result<(), Error> {
        let Value::Closure {
            invoked: Some(_),
            environment,
            ..
        } = value
        else {
            return Ok(());
        };
        let Some(tag) = environment.storage.as_ref() else {
            return Ok(());
        };
        let Some(block) = tag.logical_block else {
            return Ok(());
        };
        let inventory = self
            .arenas
            .get_mut(&tag.arena)
            .ok_or_else(|| invalid("invoking an ended arena environment"))?;
        let old = inventory
            .entries()
            .get(&block)
            .ok_or_else(|| invalid("unknown invoked environment"))?;
        let mut retained = Retained {
            ty: old.payload.ty,
            value: old.payload.value.clone(),
            source: old.payload.source.clone(),
        };
        let Value::Closure {
            captures,
            environment,
            ..
        } = &mut retained.value
        else {
            return Err(invalid("retained environment kind"));
        };
        for capture in captures.iter_mut().filter_map(Option::take) {
            capture.discard();
        }
        environment.owned_captures.fill(false);
        let old = inventory
            .replace(block, retained, Charge::ZERO)
            .map_err(|_| invalid("invoked environment custody"))?;
        old.value.discard();
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
            .chain(
                self.owners
                    .iter()
                    .flatten()
                    .map(owner::Owner::logical_bytes),
            )
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
    let mut pending = vec![(value, replace)];
    while let Some((value, replace_here)) = pending.pop() {
        if let Some(storage) = value.arena_storage().filter(|_| value.is_block()) {
            if storage.logical_block.is_none() || replace_here {
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
                .map(|child| (child, replace_here && !value.is_block())),
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
            .map(|s| &*s.metadata.roles)
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
            let [GenericArgument::Type(key), GenericArgument::Type(item)] = arguments.as_ref()
            else {
                return Err(invalid("retained SortedMap arguments"));
            };
            let pair = runtime
                .types
                .intern(hirsel_types::Type::with_kind(TypeKind::Tuple(
                    vec![*key, *item].into(),
                )));
            let entries = runtime
                .types
                .intern(hirsel_types::Type::with_kind(TypeKind::Builtin {
                    kind: BuiltinType::Array,
                    arguments: vec![GenericArgument::Type(pair)].into(),
                }));
            // SortedMap[K,V] stores the declared Order[K] dictionary plus its
            // entry array. The dictionary is not a third generic actual.
            let comparator = runtime
                .types
                .intern(hirsel_types::Type::with_kind(TypeKind::Named {
                    definition: hirsel_syntax::DefPath::core("Order").into(),
                    arguments: vec![GenericArgument::Type(*key)].into(),
                }));
            present(vec![comparator, entries])
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

    fn fixture() -> (RuntimeTypes, TypeId, TypeId, AllocationSite) {
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
        let source = AllocationSite::test(source, 0);
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
    fn sealed_regions_33_allocation_identity_distinguishes_statements_blocks_and_cow() {
        let (runtime, list_ty, _, first_site) = fixture();
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        let mut first = list(1);
        storage
            .record(&runtime, list_ty, &mut first, &first_site, false)
            .unwrap();
        let first_id = first.arena_storage().unwrap().logical_block.unwrap();

        // All source spans match, as can happen for synthesized instructions.
        let mut growth_site = first_site.clone();
        growth_site.statement = 1;
        first
            .fields_mut()
            .unwrap()
            .push(Some(Value::Bytes(b"abc".to_vec())));
        storage
            .record(&runtime, list_ty, &mut first, &growth_site, true)
            .unwrap();
        assert_eq!(first.arena_storage().unwrap().logical_block, Some(first_id));

        let mut second = list(1);
        storage
            .record(&runtime, list_ty, &mut second, &growth_site, false)
            .unwrap();
        let second_id = second.arena_storage().unwrap().logical_block.unwrap();
        let mut third_site = first_site.clone();
        third_site.block = hirsel_tir::BlockId(1);
        let mut third = list(1);
        storage
            .record(&runtime, list_ty, &mut third, &third_site, false)
            .unwrap();
        let third_id = third.arena_storage().unwrap().logical_block.unwrap();

        let mut cow = first.clone();
        cow.retag_own_block(Some(first.arena_storage().unwrap().detached()));
        let mut cow_site = third_site.clone();
        cow_site.statement = 2;
        cow_site.path = vec![3];
        storage
            .record(&runtime, list_ty, &mut cow, &cow_site, false)
            .unwrap();
        let cow_id = cow.arena_storage().unwrap().logical_block.unwrap();
        assert_ne!(cow_id, first_id);
        let retained = storage.take(1).unwrap();
        assert_eq!(retained.entries().len(), 4);
        for (id, expected) in [
            (first_id, first_site),
            (second_id, growth_site),
            (third_id, third_site),
            (cow_id, cow_site),
        ] {
            assert_eq!(retained.entries()[&id].payload.source, expected);
        }
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
        let source = AllocationSite::test(source, 0);
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

#[cfg(test)]
mod sorted_tests {
    use super::*;
    use crate::value::{ArenaStorage, NamedFunction};
    use hirsel_types::{Primitive, Type, TypeInterner};
    #[test]
    fn sealed_regions_33_sorted_mutation_refreshes_the_typed_entry_array() {
        let mut runtime = RuntimeTypes::new(TypeInterner::new(None));
        let number = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let bytes = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
        let result = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::I32)));
        let comparator = runtime.types.intern(Type::with_kind(TypeKind::Function(
            hirsel_types::FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: (0..2)
                    .map(|_| hirsel_types::Parameter {
                        name: None,
                        label: None,
                        passing: hirsel_syntax::PassingMode::Borrow,
                        ty: number,
                    })
                    .collect(),
                result,
                error: None,
                allocation: hirsel_types::Allocation::NoAlloc,
                authority: hirsel_types::AuthoritySummary::inert(),
                uses: hirsel_types::Uses::Only(hirsel_types::AuthoritySummary::inert()),
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            },
        )));
        let order = hirsel_syntax::DefPath::core("Order");
        let representation = runtime.types.intern(Type::with_kind(TypeKind::Record(
            vec![hirsel_types::Field {
                growable: false,
                name: "compare".into(),
                ty: comparator,
            }]
            .into(),
        )));
        runtime.nominals.push(crate::aggregate::Nominal {
            definition: order.clone(),
            resource: false,
            distinct: false,
            finalizer: None,
            fields: 1,
            representation: Some(representation),
        });
        let ty = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::SortedMap,
            arguments: vec![GenericArgument::Type(number), GenericArgument::Type(bytes)].into(),
        }));
        let entries = Value::Sequence {
            kind: BuiltinType::Array,
            elements: vec![].into(),
            arena: Some(ArenaStorage::fresh(1, 16)),
        };
        let mut root = Value::Record {
            definition: hirsel_syntax::DefPath::core("SortedMap").into(),
            fields: vec![
                Some(Value::Record {
                    definition: order.into(),
                    fields: vec![Some(Value::FunctionReference(Box::new(NamedFunction {
                        definition: hirsel_syntax::DefPath::core("compare"),
                        arguments: vec![],
                        types: runtime.types.clone(),
                        ty: comparator,
                    })))],
                }),
                Some(entries),
            ],
        };
        let source = crate::observation::Anchor {
            definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("arena.with")),
            body: hirsel_tir::BodyId(0),
            site: Default::default(),
            cleanup: None,
        };
        let source = AllocationSite::test(source, 0);
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        storage
            .record(&runtime, ty, &mut root, &source, false)
            .unwrap();
        let entry = root.fields_mut().unwrap()[1].as_mut().unwrap();
        entry.fields_mut().unwrap().push(Some(Value::Tuple(vec![
            Some(Value::Integer(
                crate::Integer::new(Primitive::U64, 7).unwrap(),
            )),
            Some(Value::Bytes(b"abc".to_vec())),
        ])));
        storage
            .record(&runtime, ty, &mut root, &source, true)
            .unwrap();
        let inventory = storage.take(1).unwrap();
        assert_eq!(inventory.entries().len(), 1);
        let entry = inventory.entries().values().next().unwrap();
        assert!(matches!(
            runtime.types.row(entry.payload.ty).kind,
            TypeKind::Builtin {
                kind: BuiltinType::Array,
                ..
            }
        ));
        assert_eq!(entry.outgoing_heap, Charge::Bytes(3));
        assert_eq!(entry.payload.source, source.below(&[1]).unwrap());
    }
}

#[cfg(test)]
mod invocation_tests {
    use super::*;
    use crate::value::{ArenaStorage, Environment};
    use hirsel_types::{
        Allocation, AuthoritySummary, FunctionType, Multiplicity, Primitive, Type, TypeInterner,
        Uses,
    };
    #[test]
    fn sealed_regions_33_once_environment_transfers_retained_outgoing_edges() {
        let runtime = RuntimeTypes::new(TypeInterner::new(None));
        let bytes = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
        let scalar = runtime
            .types
            .intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let mut row = Type::with_kind(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result: scalar,
            error: None,
            allocation: Allocation::NoAlloc,
            authority: AuthoritySummary::inert(),
            uses: Uses::Only(AuthoritySummary::inert()),
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }));
        row.modes.multiplicity = Multiplicity::Once;
        let callable = runtime.types.intern(row);
        let mut value = Value::Closure {
            invoked: Some(Default::default()),
            definition: hirsel_syntax::DefPath::core("owned").into(),
            body: hirsel_tir::BodyId(0),
            captures: vec![Some(Value::Bytes(b"abc".to_vec()))],
            environment: Box::new(Environment {
                storage: Some(ArenaStorage::fresh(1, 0)),
                capture_types: vec![bytes],
                owned_captures: vec![true],
                sizes: Default::default(),
                type_arguments: Default::default(),
            }),
        };
        let source = crate::observation::Anchor {
            definition: hirsel_syntax::DefPath::core("arena.with").into(),
            body: hirsel_tir::BodyId(0),
            site: Default::default(),
            cleanup: None,
        };
        let source = AllocationSite::test(source, 0);
        let root = Value::Integer(crate::Integer::new(Primitive::U64, 7).unwrap());
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        storage
            .record(&runtime, callable, &mut value, &source, false)
            .unwrap();
        assert_eq!(
            storage.seal_charge(&runtime, 1, scalar, &root).unwrap(),
            Charge::Bytes(3)
        );
        storage.environment_entry(&value).unwrap();
        value.begin_call();
        assert_eq!(
            storage.seal_charge(&runtime, 1, scalar, &root).unwrap(),
            Charge::ZERO
        );
        let retained = storage.take(1).unwrap();
        assert_eq!(retained.entries().len(), 1);
        assert!(
            retained
                .entries()
                .values()
                .next()
                .unwrap()
                .payload
                .value
                .fields()
                .unwrap()
                .iter()
                .all(Option::is_none)
        );
        value.discard();
    }
}
