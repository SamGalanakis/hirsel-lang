use crate::Type;
use hirsel_intern::work::{self, Counter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// An index plus ephemeral row provenance. Never encode provenance in semantic hashes.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct TypeId {
    index: u32,
    provenance: u64,
}
impl TypeId {
    pub fn index(self) -> usize {
        self.index as usize
    }
}
impl std::fmt::Debug for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypeId({})", self.index)
    }
}
/// Append-only immutable rows. Clones share existing rows; append detaches them. Every new row
/// has fresh provenance, so independently extended clones cannot alias new indices.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypeStore {
    rows: Arc<Vec<(TypeId, Type)>>,
}
/// A closed immutable prefix for caller-owned temporary graph operations.
#[derive(Clone, Copy, Debug)]
pub struct TypeStoreMark {
    len: usize,
    boundary: Option<TypeId>,
}

impl TypeStore {
    /// Mark a closed graph prefix once, before appending operation-local rows.
    pub fn mark(&self) -> TypeStoreMark {
        debug_assert!(
            self.iter().all(|(_, row)| row
                .kind
                .graph_edges()
                .into_iter()
                .all(|edge| self.id(edge.ty.index()) == Some(edge.ty))),
            "type store prefix is not closed"
        );
        TypeStoreMark {
            len: self.len(),
            boundary: self.len().checked_sub(1).and_then(|i| self.id(i)),
        }
    }
    /// Discard operation-local rows. Immutable surviving rows cannot acquire
    /// edges to the discarded suffix; fresh provenance prevents reuse of its IDs.
    pub fn truncate_to(&mut self, mark: TypeStoreMark) {
        assert!(mark.len <= self.len(), "type store mark exceeds its store");
        assert_eq!(
            mark.boundary,
            mark.len.checked_sub(1).and_then(|i| self.id(i)),
            "type store mark belongs to another immutable prefix"
        );
        if mark.len != self.len() {
            if Arc::strong_count(&self.rows) > 1 {
                work::count_by(Counter::CopiedTypeRows, self.rows.len() as u64);
            }
            Arc::make_mut(&mut self.rows).truncate(mark.len);
        }
    }

    pub fn from_graph(count: usize, build: impl FnOnce(&[TypeId]) -> Vec<Type>) -> Self {
        let ids: Vec<_> = (0..count).map(Self::fresh_id).collect();
        let types = build(&ids);
        assert_eq!(types.len(), count, "type graph row count changed");
        Self {
            rows: Arc::new(ids.into_iter().zip(types).collect()),
        }
    }
    pub(crate) fn fresh_id(index: usize) -> TypeId {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let provenance = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("type provenance exhausted");
        TypeId {
            index: u32::try_from(index).expect("type arena exceeds u32"),
            provenance,
        }
    }
    pub fn push(&mut self, ty: Type) -> TypeId {
        let id = Self::fresh_id(self.rows.len());
        if Arc::strong_count(&self.rows) > 1 {
            work::count_by(Counter::CopiedTypeRows, self.rows.len() as u64);
        }
        Arc::make_mut(&mut self.rows).push((id, ty));
        id
    }
    pub fn get(&self, id: TypeId) -> Option<&Type> {
        let (owned, ty) = self.rows.get(id.index())?;
        debug_assert_eq!(
            owned.provenance, id.provenance,
            "TypeId belongs to another TypeStore snapshot"
        );
        (owned.provenance == id.provenance).then_some(ty)
    }
    /// Obtain an ID from this store, for indexed iteration and hand-built fixtures.
    pub fn id(&self, index: usize) -> Option<TypeId> {
        self.rows.get(index).map(|(id, _)| *id)
    }
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (TypeId, &Type)> {
        self.rows.iter().map(|(id, ty)| (*id, ty))
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}
