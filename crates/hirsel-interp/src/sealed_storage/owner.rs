//! Private transaction snapshots. No language credit changes during preparation.
use super::*;
use std::{cell::Cell, collections::BTreeSet, rc::Rc};

#[derive(Debug, Default)]
struct LoanState {
    alive: Cell<bool>,
    active: Cell<bool>,
    generation: Cell<u64>,
}

#[derive(Debug)]
pub(crate) struct Owner {
    pub root_type: TypeId,
    pub root: Value,
    pub retained: BTreeMap<crate::inventory::LogicalBlockId, Retained>,
    loan: Rc<LoanState>,
}
impl Owner {
    pub fn logical_bytes(&self) -> u64 {
        // The root and nested enrolled values are views. Every retained block
        // contributes its payload once, including blocks unreachable from root.
        let mut total = 0u64;
        let mut work = vec![(&self.root, false)];
        work.extend(self.retained.values().map(|r| (&r.value, true)));
        while let Some((value, payload)) = work.pop() {
            if !payload && value.enrolled_arena_block() {
                continue;
            }
            if let Some(fields) = value.fields() {
                total = total.saturating_add(crate::value::NODE_BYTES);
                work.extend(fields.iter().flatten().map(|v| (v, false)));
            } else {
                total = total.saturating_add(value.logical_bytes());
            }
        }
        total
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        std::mem::replace(&mut self.root, Value::Unit).discard();
        for (_, entry) in std::mem::take(&mut self.retained) {
            entry.value.discard();
        }
    }
}

pub(crate) struct RootLoan {
    state: Rc<LoanState>,
    generation: u64,
    check: Option<(crate::checked::Shared, u32)>,
}
impl std::fmt::Debug for RootLoan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootLoan")
            .field("generation", &self.generation)
            .finish()
    }
}
impl RootLoan {
    pub fn shadow(&self) -> crate::checked::Shadow {
        crate::checked::Shadow::of(
            self.check
                .as_ref()
                .map(|(_, id)| vec![*id])
                .unwrap_or_default(),
        )
    }
}
impl Drop for RootLoan {
    fn drop(&mut self) {
        if let Some((check, instance)) = &self.check {
            check.borrow_mut().close(*instance);
        }
        if self.state.generation.get() == self.generation {
            self.state.active.set(false);
        }
    }
}

/// A fully closed candidate, still private. Dropping it discards snapshots;
/// the open arena and incoming taken root remain authoritative until commit.
pub(crate) struct Prepared {
    arena: u64,
    owner: Option<Owner>,
}

impl Storage {
    pub fn prepare_owner(
        &self,
        runtime: &RuntimeTypes,
        arena: u64,
        root_type: TypeId,
        root: &Value,
        source: &AllocationSite,
    ) -> Result<Prepared, Error> {
        let original = self
            .arenas
            .get(&arena)
            .ok_or_else(|| invalid("consumed arena grant"))?;
        let mut retained = original
            .entries()
            .iter()
            .map(|(id, entry)| {
                (
                    *id,
                    Retained {
                        ty: entry.payload.ty,
                        value: entry.payload.value.clone(),
                        source: entry.payload.source.clone(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut next = retained.last_key_value().map_or(Ok(0), |(id, _)| {
            id.0.checked_add(1)
                .ok_or_else(|| invalid("sealed allocation identity overflow"))
        })?;
        let mut candidate = Owner {
            root_type,
            root: root.clone(),
            retained: BTreeMap::new(),
            loan: Rc::new(LoanState::default()),
        };
        let original_ids = retained.keys().copied().collect::<Vec<_>>();
        for id in original_ids {
            let mut entry = retained.remove(&id).unwrap();
            evacuate(
                runtime,
                arena,
                entry.ty,
                &mut entry.value,
                &entry.source,
                true,
                &mut next,
                &mut retained,
            )?;
            retained.insert(id, entry);
        }
        evacuate(
            runtime,
            arena,
            root_type,
            &mut candidate.root,
            source,
            false,
            &mut next,
            &mut retained,
        )?;
        // A copied structural view must resolve to the transformed payload,
        // including heap descendants now held by separate retained entries.
        let mut completed = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        let mut pending = retained.keys().map(|id| (*id, false)).collect::<Vec<_>>();
        while let Some((id, exit)) = pending.pop() {
            if completed.contains_key(&id) {
                continue;
            }
            if exit {
                let mut entry = retained
                    .remove(&id)
                    .ok_or_else(|| invalid("missing sealed payload"))?;
                rebase(&mut entry.value, true, arena, &completed)?;
                visiting.remove(&id);
                completed.insert(id, entry);
            } else {
                if !visiting.insert(id) {
                    return Err(invalid("cyclic sealed storage graph"));
                }
                let entry = retained
                    .get(&id)
                    .ok_or_else(|| invalid("missing sealed allocation"))?;
                pending.push((id, true));
                for child in dependencies(&entry.value) {
                    if child != id {
                        pending.push((child, false));
                    } else {
                        return Err(invalid("self-referential sealed storage"));
                    }
                }
            }
        }
        rebase(&mut candidate.root, false, arena, &completed)?;
        candidate.retained = completed;
        Ok(Prepared {
            arena,
            owner: Some(candidate),
        })
    }

    pub fn commit_owner(&mut self, mut prepared: Prepared) -> Result<u64, Error> {
        // Reserve before disarming the original ownership obligation.
        self.owners.reserve(1);
        let original = self.take(prepared.arena)?;
        let owner = prepared
            .owner
            .take()
            .ok_or_else(|| invalid("owner already committed"))?;
        owner.loan.alive.set(true);
        let id = self.owners.len() as u64;
        self.owners.push(Some(owner));
        for (_, entry) in original.into_entries() {
            entry.payload.value.discard();
        }
        Ok(id)
    }

    pub fn borrow_owner(
        &self,
        id: u64,
        expected_root_type: TypeId,
        check: Option<&crate::checked::Shared>,
    ) -> Result<(Value, RootLoan), Error> {
        let owner = self
            .owners
            .get(id as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| invalid("read of a dead Region owner"))?;
        if owner.root_type != expected_root_type {
            return Err(invalid(
                "Region read root type differs from its sealed payload",
            ));
        }
        if !owner.loan.alive.get() || owner.loan.active.get() {
            return Err(invalid("Region owner already has an active read loan"));
        }
        let generation = owner
            .loan
            .generation
            .get()
            .checked_add(1)
            .ok_or_else(|| invalid("Region read generation overflow"))?;
        let value = owner.root.clone();
        let check = check.map(|state| (state.clone(), state.borrow_mut().sealed_view()));
        owner.loan.generation.set(generation);
        owner.loan.active.set(true);
        Ok((
            value,
            RootLoan {
                state: owner.loan.clone(),
                generation,
                check,
            },
        ))
    }

    pub fn release_owner(&mut self, id: u64) -> Result<Owner, Error> {
        let slot = self
            .owners
            .get_mut(id as usize)
            .ok_or_else(|| invalid("unknown Region owner"))?;
        if slot.as_ref().is_some_and(|owner| owner.loan.active.get()) {
            return Err(invalid("drop of a Region owner with an active read loan"));
        }
        let owner = slot
            .take()
            .ok_or_else(|| invalid("Region owner already released"))?;
        owner.loan.alive.set(false);
        Ok(owner)
    }
}

fn evacuate(
    runtime: &RuntimeTypes,
    arena: u64,
    ty: TypeId,
    value: &mut Value,
    source: &AllocationSite,
    payload: bool,
    next: &mut u64,
    retained: &mut BTreeMap<crate::inventory::LogicalBlockId, Retained>,
) -> Result<(), Error> {
    let mut work = vec![(Vec::<usize>::new(), ty, false)];
    while let Some((path, ty, exit)) = work.pop() {
        let current = at_mut(value, &path)?;
        if matches!(current, Value::Shared(_) | Value::FunctionReference(_)) {
            continue;
        }
        if matches!(
            current,
            Value::Handle { .. }
                | Value::Resource { .. }
                | Value::ResourceVariant { .. }
                | Value::Mutex(_)
        ) {
            return Err(invalid("resource in sealed payload"));
        }
        if !exit {
            if let Some(storage) = current.arena_storage() {
                if storage.arena != arena || storage.logical_block.is_none() {
                    return Err(invalid("unproved sealed allocation placement"));
                }
                if current.is_block() && !(payload && path.is_empty()) {
                    continue;
                }
            }
            work.push((path.clone(), ty, true));
            for (index, child_ty) in children(runtime, ty, current)?.into_iter().rev() {
                let mut child = path.clone();
                child.push(index);
                work.push((child, child_ty, false));
            }
        } else if current.is_block() && current.arena_storage().is_none() {
            let id = crate::inventory::LogicalBlockId(*next);
            *next = next
                .checked_add(1)
                .ok_or_else(|| invalid("sealed allocation identity overflow"))?;
            current.retag_own_block(Some(crate::value::ArenaStorage::fresh(arena, 0)));
            current.enroll_block(id)?;
            retained.insert(
                id,
                Retained {
                    ty,
                    value: current.clone(),
                    source: source.below(&path)?,
                },
            );
        }
    }
    Ok(())
}

fn dependencies(value: &Value) -> BTreeSet<crate::inventory::LogicalBlockId> {
    let mut out = BTreeSet::new();
    let mut pending = value
        .fields()
        .into_iter()
        .flatten()
        .flatten()
        .collect::<Vec<_>>();
    while let Some(value) = pending.pop() {
        if matches!(value, Value::Shared(_) | Value::FunctionReference(_)) {
            continue;
        }
        if value.is_block()
            && let Some(id) = value.arena_storage().and_then(|s| s.logical_block)
        {
            out.insert(id);
            continue;
        }
        pending.extend(value.fields().into_iter().flatten().flatten());
    }
    out
}

fn rebase(
    value: &mut Value,
    payload: bool,
    arena: u64,
    completed: &BTreeMap<crate::inventory::LogicalBlockId, Retained>,
) -> Result<(), Error> {
    let mut pending = vec![(value, payload)];
    while let Some((value, own)) = pending.pop() {
        if matches!(value, Value::Shared(_) | Value::FunctionReference(_)) {
            continue;
        }
        if !own
            && value.is_block()
            && let Some(tag) = value.arena_storage()
        {
            if tag.arena != arena {
                return Err(invalid("foreign sealed view"));
            }
            let id = tag
                .logical_block
                .ok_or_else(|| invalid("unenrolled sealed view"))?;
            let entry = completed
                .get(&id)
                .ok_or_else(|| invalid("missing rebased sealed view"))?;
            // Materialized byte views already retain their exact extent.
            if !matches!(value, Value::ArenaBytes { .. } | Value::ArenaString { .. }) {
                std::mem::replace(value, entry.value.clone()).discard();
            }
            continue;
        }
        if let Some(fields) = value.fields_mut() {
            pending.extend(fields.iter_mut().flatten().map(|v| (v, false)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Charge;
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
        let source = AllocationSite::test(
            crate::observation::Anchor {
                definition: hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core(
                    "arena.seal",
                )),
                body: hirsel_tir::BodyId(0),
                site: Default::default(),
                cleanup: None,
            },
            0,
        );
        (runtime, bytes, list, source)
    }
    #[test]
    fn sealed_regions_33_owner_preparation_is_private_and_rebases_retained_views() {
        let (runtime, bytes, list, source) = fixture();
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        let mut retained = Value::Sequence {
            kind: BuiltinType::List,
            elements: vec![
                Some(Value::Bytes(b"abc".to_vec())),
                Some(Value::Bytes(b"abc".to_vec())),
            ]
            .into(),
            arena: Some(ArenaStorage::fresh(1, 8)),
        };
        storage
            .record(&runtime, list, &mut retained, &source, false)
            .unwrap();
        assert_eq!(
            storage.seal_charge(&runtime, 1, list, &retained).unwrap(),
            Charge::Bytes(6)
        );
        let prepared = storage
            .prepare_owner(&runtime, 1, list, &retained, &source)
            .unwrap();
        assert!(storage.owners.is_empty());
        assert_eq!(
            storage.seal_charge(&runtime, 1, list, &retained).unwrap(),
            Charge::Bytes(6)
        );
        assert!(matches!(
            retained.fields().unwrap()[0],
            Some(Value::Bytes(_))
        ));
        let id = storage.commit_owner(prepared).unwrap();
        assert!(storage.seal_charge(&runtime, 1, list, &retained).is_err());
        let (view, loan) = storage.borrow_owner(id, list, None).unwrap();
        assert!(view.enrolled_arena_block());
        assert!(
            view.fields()
                .unwrap()
                .iter()
                .flatten()
                .all(Value::enrolled_arena_block)
        );
        let owner = storage.owners[id as usize].as_ref().unwrap();
        assert_eq!(owner.root_type, list);
        assert_eq!(owner.retained.len(), 3);
        assert_eq!(owner.logical_bytes(), 3 * crate::value::NODE_BYTES + 6);
        assert!(storage.borrow_owner(id, bytes, None).is_err());
        assert_eq!(owner.retained.values().filter(|r| r.ty == bytes).count(), 2);
        assert!(storage.borrow_owner(id, list, None).is_err());
        assert!(storage.release_owner(id).is_err());
        view.discard();
        drop(loan);
        let (again, loan) = storage.borrow_owner(id, list, None).unwrap();
        again.discard();
        drop(loan);
        let owner = storage.release_owner(id).unwrap();
        assert!(storage.borrow_owner(id, list, None).is_err());
        assert!(storage.release_owner(id).is_err());
        drop(owner);
        retained.discard();
    }
    #[test]
    fn sealed_regions_33_owner_abort_keeps_credits_and_commit_transfers_them_once() {
        let (runtime, bytes, _, source) = fixture();
        let shared_ty = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::Shared,
            arguments: vec![GenericArgument::Type(bytes)].into(),
        }));
        let list = runtime.types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(shared_ty)].into(),
        }));
        let shared = crate::value::SharedRoot::freeze(73, Value::Bytes(b"abc".to_vec()));
        let root = Value::Sequence {
            kind: BuiltinType::List,
            elements: vec![Some(Value::Shared(shared.clone()))].into(),
            arena: None,
        };
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        let abandoned = storage
            .prepare_owner(&runtime, 1, list, &root, &source)
            .unwrap();
        assert_eq!(shared.count(), 1);
        drop(abandoned);
        assert_eq!(shared.count(), 1);
        assert!(storage.arenas.contains_key(&1));
        let prepared = storage
            .prepare_owner(&runtime, 1, list, &root, &source)
            .unwrap();
        let id = storage.commit_owner(prepared).unwrap();
        root.discard();
        assert_eq!(shared.count(), 1);
        let (view, loan) = storage.borrow_owner(id, list, None).unwrap();
        view.discard();
        drop(loan);
        assert_eq!(shared.count(), 1);
        let mut owner = storage.release_owner(id).unwrap();
        assert_eq!(owner.retained.len(), 1);
        // The evaluator opens each retained payload once; enrolled root views
        // are discarded without releasing that payload a second time.
        std::mem::replace(&mut owner.root, Value::Unit).discard();
        for (_, mut entry) in std::mem::take(&mut owner.retained) {
            for child in entry.value.fields_mut().unwrap().iter_mut() {
                let Some(Value::Shared(root)) = child.take() else {
                    panic!("missing credit")
                };
                assert!(root.release().unwrap().is_some());
            }
            entry.value.discard();
        }
        assert_eq!(shared.count(), 0);
    }
    #[test]
    fn sealed_regions_33_owner_keeps_byte_view_extents_and_poisoned_loan_generation() {
        let (runtime, bytes, _, source) = fixture();
        let mut storage = Storage::default();
        storage.open(1).unwrap();
        let mut full = Value::ArenaBytes {
            bytes: b"abcdef".to_vec(),
            storage: ArenaStorage::fresh(1, 0),
        };
        storage
            .record(&runtime, bytes, &mut full, &source, false)
            .unwrap();
        let mut view = full.clone();
        let Value::ArenaBytes { bytes: extent, .. } = &mut view else {
            unreachable!()
        };
        *extent = b"bcd".to_vec();
        let id = storage
            .commit_owner(
                storage
                    .prepare_owner(&runtime, 1, bytes, &view, &source)
                    .unwrap(),
            )
            .unwrap();
        let check = crate::checked::state(Default::default());
        let (read, loan) = storage.borrow_owner(id, bytes, Some(&check)).unwrap();
        assert!(matches!(read, Value::ArenaBytes { ref bytes, .. } if bytes == b"bcd"));
        let shadow = loan.shadow();
        assert!(check.borrow().access(&shadow, "live read").is_ok());
        read.discard();
        drop(loan);
        assert!(check.borrow().access(&shadow, "stale read").is_err());
        let (read, loan) = storage.borrow_owner(id, bytes, Some(&check)).unwrap();
        assert!(
            check
                .borrow()
                .access(&loan.shadow(), "new generation")
                .is_ok()
        );
        read.discard();
        drop(loan);
        storage.release_owner(id).unwrap();
        full.discard();
        view.discard();
    }
}
