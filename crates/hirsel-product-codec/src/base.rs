//! Shared immutable type positions reference the same canonical components as
//! ordinary products. Opening the index never deserializes component rows.
use crate::{TypeEdge, TypeRecords, TypeRef, types};
use hirsel_artifact_store::Contents;
use hirsel_intern::IdHash;
use hirsel_types::{Type, TypeBase, TypeBaseSource, TypeId, TypeInterner};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct BaseIndex {
    references: Vec<TypeRef>,
    /// Sorted, collision-checked index of row labels. Edges are omitted here:
    /// complete row equality, including every edge, decides interner identity.
    candidates: Vec<(u64, u32)>,
}
fn label(row: &impl Serialize) -> Option<u64> {
    // bincode::serialize visits every row twice to size its allocation. The
    // writer API emits the same bytes directly, with one table-collection pass.
    let (bytes, _) = hirsel_types::with_table_collection(|| {
        let mut bytes = Vec::new();
        crate::indexed_types::inline_names(|| bincode::serialize_into(&mut bytes, row)).ok()?;
        Some(bytes)
    });
    let mut hash = IdHash::default();
    hash.write(&bytes?);
    Some(hash.finish())
}
impl BaseIndex {
    /// Preserve the supplied shared prefix's positions; its closure must be
    /// included so every outgoing reference can be resolved without loading it.
    pub fn capture(records: &TypeRecords, roots: &[TypeId]) -> Option<Self> {
        let references = records.encode(roots)?;
        let mut candidates = roots
            .iter()
            .enumerate()
            .map(|(i, &id)| Some((label(records.types.get(id)?)?, u32::try_from(i).ok()?)))
            .collect::<Option<Vec<_>>>()?;
        candidates.sort_unstable();
        Some(Self {
            references,
            candidates,
        })
    }
}
struct Source {
    index: BaseIndex,
    positions: HashMap<TypeRef, u32>,
    store: Arc<dyn Contents>,
    members: HashMap<(hirsel_syntax::ContentHash, u64), Vec<(u32, u32)>>,
}
impl TypeBaseSource for Source {
    fn rows(&self, position: u32, base: &TypeBase) -> Option<Vec<(u32, Type)>> {
        let reference = *self.index.references.get(position as usize)?;
        let component = types::component(self.store.as_ref(), reference)?;
        let rows = (0..component.len())
            .map(|member| {
                component.row(member)?.materialize(|edge| {
                    let reference = match edge {
                        TypeEdge::Member(member) => TypeRef {
                            member,
                            ..reference
                        },
                        TypeEdge::External(reference) => reference,
                    };
                    base.id_at(*self.positions.get(&reference)?)
                })
            })
            .collect::<Option<Vec<_>>>()?;
        self.members
            .get(&reference.key())?
            .iter()
            .map(|&(p, member)| Some((p, rows.get(member as usize)?.clone())))
            .collect()
    }
    fn candidates(&self, row: &Type, base: &TypeBase) -> Vec<u32> {
        if row
            .kind
            .graph_edges()
            .iter()
            .any(|e| base.position(e.ty).is_none())
        {
            return vec![];
        }
        let Some(key) = label(row) else { return vec![] };
        let start = self
            .index
            .candidates
            .partition_point(|&(hash, _)| hash < key);
        self.index.candidates[start..]
            .iter()
            .take_while(|&&(hash, _)| hash == key)
            .map(|&(_, p)| p)
            .collect()
    }
}
/// One shared base plus the references needed to decode ordinary product
/// records over it. V authority remains with the consumer's verifier.
pub struct SharedTypes {
    pub base: Arc<TypeBase>,
    positions: HashMap<TypeRef, u32>,
    store: Arc<dyn Contents>,
}
impl SharedTypes {
    pub fn open(index: BaseIndex, store: Arc<dyn Contents>) -> Option<Self> {
        let count = u32::try_from(index.references.len()).ok()?;
        if index.candidates.len() != index.references.len()
            || index.candidates.windows(2).any(|w| w[0] >= w[1])
        {
            return None;
        }
        let mut seen = vec![false; count as usize];
        for &(_, p) in &index.candidates {
            if *seen.get(p as usize)? {
                return None;
            }
            seen[p as usize] = true;
        }
        let mut positions = HashMap::new();
        let mut members: HashMap<_, Vec<_>> = HashMap::new();
        for (p, &reference) in index.references.iter().enumerate() {
            if reference.len > hirsel_artifact_store::MAX_RECORD as u64 {
                return None;
            }
            positions.entry(reference).or_insert(p as u32);
            members
                .entry(reference.key())
                .or_default()
                .push((p as u32, reference.member));
        }
        let source = Source {
            members,
            index,
            positions: positions.clone(),
            store: store.clone(),
        };
        Some(Self {
            base: TypeBase::from_source(count, Arc::new(source)),
            positions,
            store,
        })
    }
    pub fn records(&self, types: Arc<TypeInterner>) -> Option<Arc<TypeRecords>> {
        if !Arc::ptr_eq(types.base()?, &self.base) {
            return None;
        }
        let base = self
            .positions
            .iter()
            .map(|(reference, &p)| Some((*reference, self.base.id_at(p)?)))
            .collect::<Option<_>>()?;
        Some(TypeRecords::with_base(types, self.store.clone(), base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_artifact_store::{Bytes, Domain, Memory, Reference};
    use hirsel_types::TypeKind;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn row_label_visits_the_serialized_row_once() {
        struct Row {
            value: Type,
            visits: std::cell::Cell<u32>,
        }
        impl Serialize for Row {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.visits.set(self.visits.get() + 1);
                self.value.serialize(serializer)
            }
        }
        let types = TypeInterner::new(None);
        let unit = types.intern(Type::unit());
        for value in [
            Type::unit(),
            Type::with_kind(TypeKind::Box(unit)),
            Type::with_kind(TypeKind::Tuple(vec![unit, unit].into())),
        ] {
            let row = Row {
                value,
                visits: std::cell::Cell::new(0),
            };
            let bytes = hirsel_types::with_table_collection(|| bincode::serialize(&row.value))
                .0
                .unwrap();
            let mut expected = IdHash::default();
            expected.write(&bytes);
            assert_eq!(
                label(&row),
                Some(expected.finish()),
                "existing labels stay exact"
            );
            assert_eq!(
                row.visits.get(),
                1,
                "labeling needs no serialized-size pass"
            );
        }
    }

    #[derive(Default)]
    struct Counted {
        memory: Memory,
        reads: AtomicUsize,
    }
    impl Contents for Counted {
        fn get_content(&self, reference: Reference) -> Option<Bytes> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.memory.get_content(reference)
        }
        fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
            self.memory.put_content(domain, bytes)
        }
    }
    fn fixture() -> (Arc<Counted>, BaseIndex, Vec<u8>) {
        let store = Arc::new(Counted::default());
        let types = TypeInterner::new(None);
        let unit = types.intern(Type::unit());
        let byte = types.intern(Type::with_kind(TypeKind::Primitive(
            hirsel_types::Primitive::U8,
        )));
        let left = types.intern(Type::with_kind(TypeKind::Box(unit)));
        types.intern(Type::with_kind(TypeKind::Box(byte)));
        let roots: Vec<_> = types.ids().collect();
        let records = TypeRecords::new(types, store.clone());
        let index = BaseIndex::capture(&records, &roots).unwrap();
        let record = records.encode_record(&left, |_| None).unwrap();
        (store, index, record)
    }
    #[test]
    fn opening_and_decoding_references_load_no_rows_and_first_access_loads_one_component() {
        let (store, index, record) = fixture();
        let shared = SharedTypes::open(index, store.clone()).unwrap();
        let a = TypeInterner::new(Some(shared.base.clone()));
        let b = TypeInterner::new(Some(shared.base.clone()));
        let records = shared.records(a.clone()).unwrap();
        let root: TypeId = records.decode_record(&record, |_, _| None).unwrap();
        assert_eq!(store.reads.load(Ordering::Relaxed), 0);
        assert!(shared.records(TypeInterner::new(None)).is_none());
        assert!(matches!(a.row(root).kind, TypeKind::Box(_)));
        assert_eq!(
            store.reads.load(Ordering::Relaxed),
            1,
            "outgoing rows stay lazy"
        );
        assert!(std::ptr::eq(a.row(root), b.row(root)));
        assert_eq!(b.intern(a.row(root).clone()), root);
        assert_eq!(store.reads.load(Ordering::Relaxed), 1);
        let other = Type::with_kind(TypeKind::Box(shared.base.id_at(1).unwrap()));
        let other = b.intern(other);
        assert_ne!(other, root, "equal row labels do not erase unequal edges");
        assert_eq!(other, shared.base.id_at(3).unwrap());
    }
    #[test]
    fn concurrent_shared_access_loads_once_and_missing_component_never_publishes() {
        let (store, index, _) = fixture();
        let shared = SharedTypes::open(index, store.clone()).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let shared = &shared;
                scope.spawn(move || {
                    let types = TypeInterner::new(Some(shared.base.clone()));
                    assert_eq!(types.row(shared.base.id_at(0).unwrap()), &Type::unit());
                });
            }
        });
        assert_eq!(store.reads.load(Ordering::Relaxed), 1);
        let (_, index, _) = fixture();
        let missing = SharedTypes::open(index, Arc::new(Memory::default())).unwrap();
        let types = TypeInterner::new(Some(missing.base.clone()));
        assert!(types.get(missing.base.id_at(0).unwrap()).is_none());
    }
    #[test]
    fn cyclic_components_and_duplicate_reference_positions_keep_complete_edges() {
        let store = Arc::new(Counted::default());
        let types = TypeInterner::new(None);
        let ids = types.intern_graph(2, |ids| {
            vec![
                Type::with_kind(TypeKind::Box(ids[1])),
                Type::with_kind(TypeKind::Tuple(vec![ids[0]].into())),
            ]
        });
        let records = TypeRecords::new(types, store.clone());
        let index = BaseIndex::capture(&records, &[ids[0], ids[1], ids[0]]).unwrap();
        let shared = SharedTypes::open(index, store.clone()).unwrap();
        let types = TypeInterner::new(Some(shared.base.clone()));
        let id = |p| shared.base.id_at(p).unwrap();
        assert_eq!(types.row(id(0)).kind.graph_edges()[0].ty, id(1));
        assert_eq!(types.row(id(1)).kind.graph_edges()[0].ty, id(0));
        assert_eq!(types.row(id(2)), types.row(id(0)));
        assert_eq!(store.reads.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn malformed_candidate_indices_are_rejected_without_loading_types() {
        let (store, mut index, _) = fixture();
        index.candidates[0].1 = u32::MAX;
        assert!(SharedTypes::open(index, store.clone()).is_none());
        assert_eq!(store.reads.load(Ordering::Relaxed), 0);
    }
}
