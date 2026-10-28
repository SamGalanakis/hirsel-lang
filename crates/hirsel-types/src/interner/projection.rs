use super::*;
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct FrozenRows {
    pub(super) rows: Box<[(TypeId, Entry)]>,
    pub(super) positions: HashMap<TypeId, u32, IdHasher>,
}

/// A source-owned subset of an existing type context. Its immutable rows keep
/// their original ids; new rows start beyond the original context's high water.
/// Derived normal forms and validation caches belong to the new context.
pub struct TypeProjection {
    pub(super) source: Arc<TypeInterner>,
    pub(super) ids: Box<[TypeId]>,
    pub(super) positions: HashMap<TypeId, u32, IdHasher>,
    pub(super) shards: [HashMap<u64, Ids, IdHasher>; SHARDS],
    high_water: u32,
}

impl TypeInterner {
    /// Prepare the local closure of `roots`, using the ordinary type-table
    /// graph edges. Base rows remain with their original immutable owner and
    /// are never demanded. Preparation copies index metadata, not type rows.
    pub fn project_reachable(
        self: &Arc<Self>,
        roots: impl IntoIterator<Item = TypeId>,
    ) -> TypeProjection {
        let mut seen = HashSet::<TypeId, IdHasher>::default();
        let mut pending: Vec<_> = roots.into_iter().collect();
        let mut ids = Vec::new();
        while let Some(id) = pending.pop() {
            if id
                .position()
                .is_none_or(|position| position < self.base_len)
                || !seen.insert(id)
            {
                continue;
            }
            let Some(row) = self.get_if_loaded(id) else {
                continue;
            };
            ids.push(id);
            pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
        }
        ids.sort_unstable_by_key(|id| id.position());
        let positions = ids
            .iter()
            .enumerate()
            .map(|(position, id)| (*id, position as u32))
            .collect();
        let mut shards: [HashMap<u64, Ids, IdHasher>; SHARDS] =
            std::array::from_fn(|_| HashMap::default());
        for id in &ids {
            let row = self.get_if_loaded(*id).expect("collected local row");
            let hash = row_hash(row);
            let index = &mut shards[(hash >> 58) as usize % SHARDS];
            if index.get(&hash).is_some_and(|matches| {
                matches
                    .iter()
                    .any(|other| self.get_if_loaded(other) == Some(row))
            }) {
                continue;
            }
            match index.get_mut(&hash) {
                Some(matches) => matches.push(*id),
                None => {
                    index.insert(hash, Ids::One(*id));
                }
            }
        }
        TypeProjection {
            source: self.clone(),
            ids: ids.into(),
            positions,
            shards,
            high_water: self
                .local_start
                .checked_add(self.assigned.load(Ordering::Acquire))
                .expect("existing type interner high water"),
        }
    }
}

impl TypeProjection {
    /// Construct under the caller's admitted reservation. All existing
    /// contexts and externally held rows remain untouched and readable.
    pub fn into_interner(self) -> Arc<TypeInterner> {
        let mut bytes = 0;
        let rows = self
            .ids
            .iter()
            .map(|id| {
                let row = self
                    .source
                    .get_if_loaded(*id)
                    .expect("projected local row remains immutable")
                    .clone();
                bytes += row_bytes(&row);
                (*id, Entry::new(row))
            })
            .collect();
        Arc::new(TypeInterner {
            base: self.source.base.clone(),
            base_len: self.source.base_len,
            local_start: self.high_water,
            frozen: FrozenRows {
                rows,
                positions: self.positions,
            },
            base_normals: AppendTable::new(),
            entries: AppendTable::new(),
            assigned: AtomicU32::new(0),
            shards: self.shards.map(RwLock::new),
            counters: Counters {
                row_bytes: AtomicU64::new(bytes),
                ..Default::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::DefId;

    #[test]
    fn projections_keep_sparse_ids_cycles_and_new_interning_without_body_rows() {
        let source = TypeInterner::new(None);
        let omitted = (0..4096)
            .map(|index| {
                source.intern(Type::with_kind(TypeKind::Named {
                    definition: DefId::intern(&hirsel_syntax::DefPath::core(&format!(
                        "discarded_{index}"
                    ))),
                    arguments: Default::default(),
                }))
            })
            .collect::<Vec<_>>();
        let roots = source.intern_graph(2, |ids| {
            ids.iter()
                .rev()
                .map(|id| Type::with_kind(TypeKind::Tuple(vec![*id].into())))
                .collect()
        });
        let expected = roots
            .iter()
            .map(|id| source.row(*id).clone())
            .collect::<Vec<_>>();
        let verdict = source.validate(roots[0]);
        let before = source.retained_local_bytes();
        let weak = Arc::downgrade(&source);
        let plan = source.project_reachable([roots[0]]);
        let bound = plan.retained_bytes();
        assert!(bound < before / 4);
        drop(source);
        assert!(weak.upgrade().is_some());
        let projected = plan.into_interner();
        assert!(weak.upgrade().is_none());
        assert!(projected.retained_local_bytes() <= bound);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected.ids().collect::<Vec<_>>(), roots);
        assert_eq!(projected.validate(roots[0]), verdict);
        for (id, row) in roots.iter().zip(expected) {
            assert_eq!(projected.row(*id), &row);
            assert_eq!(projected.intern(row), *id);
        }
        assert!(omitted.iter().all(|id| projected.get(*id).is_none()));
        let new = projected.intern(Type::unit());
        assert!(!omitted.contains(&new) && !roots.contains(&new));
        assert_eq!(projected.len(), 3);
        let again = projected.project_reachable([new]).into_interner();
        assert_eq!(again.ids().collect::<Vec<_>>(), [new]);
        assert_eq!(again.intern(Type::unit()), new);
        assert!(again.get(roots[0]).is_none());
    }

    #[test]
    fn projecting_base_roots_never_calls_the_lazy_source() {
        struct Unread;
        impl TypeBaseSource for Unread {
            fn rows(&self, _: u32, _: &TypeBase) -> Option<Vec<(u32, Type)>> {
                panic!("projection must not load the base")
            }
            fn candidates(&self, _: &Type, _: &TypeBase) -> Vec<u32> {
                panic!("projection must not search the base")
            }
        }
        let base = TypeBase::from_source(1, Arc::new(Unread));
        let root = base.id_at(0).unwrap();
        let source = TypeInterner::new(Some(base.clone()));
        let projection = source.project_reachable([root]);
        let bound = projection.retained_bytes();
        let projected = projection.into_interner();
        assert!(Arc::ptr_eq(projected.base().unwrap(), &base));
        assert_eq!(projected.ids().collect::<Vec<_>>(), [root]);
        assert!(projected.get_if_loaded(root).is_none());
        assert!(projected.retained_local_bytes() <= bound);
    }
}
