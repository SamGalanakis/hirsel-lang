//! The session's one canonical type interner (design §1a). Every row of every
//! phase and thread is interned here exactly as written, so one row has one id
//! and promotion between phases cannot fork identity. Ids depend on scheduling
//! and never reach output: persisted forms encode canonical per-definition
//! tables (`table`), and ordered output sorts by content, never by `TypeId`.
use crate::*;
use hirsel_intern::{AppendTable, IdHash, IdHasher, Scramble};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
mod projection;
mod retained;
pub use projection::TypeProjection;

/// A row of the session interner. It carries no provenance and has no order.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(u32);

static SCRAMBLE: Scramble = Scramble::new("type");

impl TypeId {
    fn at(position: u32) -> Self {
        Self(SCRAMBLE.encode(position).get())
    }
    fn position(self) -> Option<u32> {
        NonZeroU32::new(self.0).map(|id| SCRAMBLE.decode(id))
    }
    pub(crate) const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub(crate) const fn to_raw(self) -> u32 {
        self.0
    }
    /// An id no interner assigns: for recovery placeholders and tests of
    /// missing rows. Every read of it reports `GraphError::MissingType`.
    pub const DANGLING: TypeId = TypeId(0);
}
impl std::fmt::Debug for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypeId({})", self.0)
    }
}

/// Normal-form ids belong to the interner that allocated them, including
/// when the input row belongs to a shared immutable image base.
#[derive(Default)]
pub(crate) struct NormalForms {
    pub(crate) sem: OnceLock<TypeId>,
    pub(crate) strip_modes: OnceLock<TypeId>,
    pub(crate) shape: OnceLock<TypeId>,
    pub(crate) contract: OnceLock<TypeId>,
}

/// One immutable row and content-only facts. The inline normal forms are used
/// for session rows; shared base rows use the owning interner's separate slots.
pub(crate) struct Entry {
    pub(crate) row: Type,
    normals: NormalForms,
    pub(crate) valid: OnceLock<Result<(), GraphError>>,
    pub(crate) actual_key: OnceLock<Option<Arc<str>>>,
    pub(crate) rewrite_features: OnceLock<u8>,
}
impl Entry {
    fn new(row: Type) -> Self {
        Self {
            row,
            normals: NormalForms::default(),
            valid: OnceLock::new(),
            actual_key: OnceLock::new(),
            rewrite_features: OnceLock::new(),
        }
    }
}

/// The ids whose rows hash to one value: almost always one.
#[derive(Clone)]
enum Ids {
    One(TypeId),
    Many(Vec<TypeId>),
}
impl Ids {
    fn iter(&self) -> impl Iterator<Item = TypeId> + '_ {
        let (one, many) = match self {
            Ids::One(id) => (Some(*id), &[][..]),
            Ids::Many(ids) => (None, &ids[..]),
        };
        one.into_iter().chain(many.iter().copied())
    }
    fn push(&mut self, id: TypeId) {
        match self {
            Ids::One(first) => *self = Ids::Many(vec![*first, id]),
            Ids::Many(ids) => ids.push(id),
        }
    }
}

fn row_hash(row: &Type) -> u64 {
    let mut hasher = IdHash::default();
    row.hash(&mut hasher);
    hasher.finish()
}

/// A lazy immutable component provider. `rows` returns a checked component with
/// edges expressed as ids obtained from `base.id_at`, without demanding rows.
/// `candidates` is an index only; TypeBase compares complete rows after loading.
pub trait TypeBaseSource: Send + Sync {
    fn rows(&self, position: u32, base: &TypeBase) -> Option<Vec<(u32, Type)>>;
    fn candidates(&self, row: &Type, base: &TypeBase) -> Vec<u32>;
}

/// An immutable shared prefix. A source-backed base loads only demanded
/// components; decoded rows have exactly one owner across session interners.
pub struct TypeBase {
    entries: Box<[OnceLock<Entry>]>,
    index: HashMap<u64, Ids, IdHasher>,
    source: Option<Arc<dyn TypeBaseSource>>,
    loading: Mutex<()>,
}
impl TypeBase {
    /// Build a base of `count` rows. `build` receives the ids the rows will
    /// have, so rows may name each other in any order.
    pub fn build(count: usize, build: impl FnOnce(&[TypeId]) -> Vec<Type>) -> Arc<Self> {
        let count = u32::try_from(count).expect("type base exceeds u32");
        let ids: Vec<_> = (0..count).map(TypeId::at).collect();
        let rows = build(&ids);
        assert_eq!(rows.len(), ids.len(), "type base row count changed");
        let mut index: HashMap<u64, Ids, IdHasher> = HashMap::default();
        for (id, row) in ids.iter().zip(&rows) {
            let hash = row_hash(row);
            let duplicate = index
                .get(&hash)
                .is_some_and(|ids| ids.iter().any(|other| rows[position(other)] == *row));
            if !duplicate {
                match index.get_mut(&hash) {
                    Some(ids) => ids.push(*id),
                    None => {
                        index.insert(hash, Ids::One(*id));
                    }
                }
            }
        }
        fn position(id: TypeId) -> usize {
            id.position().expect("a base id") as usize
        }
        Arc::new(Self {
            entries: rows
                .into_iter()
                .map(|row| OnceLock::from(Entry::new(row)))
                .collect(),
            index,
            source: None,
            loading: Mutex::new(()),
        })
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// The id of base row `position`: the same in every interner over a base.
    pub(crate) fn id(position: u32) -> TypeId {
        TypeId::at(position)
    }
    /// Construct the compact index without decoding any type rows.
    pub fn from_source(count: u32, source: Arc<dyn TypeBaseSource>) -> Arc<Self> {
        Arc::new(Self {
            entries: (0..count).map(|_| OnceLock::new()).collect(),
            index: HashMap::default(),
            source: Some(source),
            loading: Mutex::new(()),
        })
    }
    pub fn id_at(&self, position: u32) -> Option<TypeId> {
        ((position as usize) < self.len()).then(|| Self::id(position))
    }
    pub fn position(&self, id: TypeId) -> Option<u32> {
        id.position().filter(|&p| (p as usize) < self.len())
    }
    fn entry(&self, position: u32) -> Option<&Entry> {
        let slot = self.entries.get(position as usize)?;
        if let Some(entry) = slot.get() {
            return Some(entry);
        }
        // std OnceLock owns publication. Serialize only first component loads;
        // providers never recursively demand another row while holding this lock.
        let _loading = self.loading.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.get().is_none() {
            let rows = self.source.as_ref()?.rows(position, self)?;
            let mut seen = std::collections::HashSet::new();
            if !rows.iter().any(|(p, _)| *p == position)
                || rows.iter().any(|(p, row)| {
                    self.id_at(*p).is_none()
                        || !seen.insert(*p)
                        || row
                            .kind
                            .graph_edges()
                            .iter()
                            .any(|e| self.position(e.ty).is_none())
                        || self.entries[*p as usize]
                            .get()
                            .is_some_and(|e| e.row != *row)
                })
            {
                return None;
            }
            for (p, row) in rows {
                self.entries[p as usize].get_or_init(|| Entry::new(row));
            }
        }
        slot.get()
    }
    fn find(&self, hash: u64, row: &Type) -> Option<TypeId> {
        if let Some(source) = &self.source {
            return source
                .candidates(row, self)
                .into_iter()
                .find_map(|p| (self.entry(p)?.row == *row).then(|| Self::id(p)));
        }
        self.index.get(&hash)?.iter().find(|id| {
            self.entry(id.position().expect("a base id"))
                .is_some_and(|e| e.row == *row)
        })
    }
}

const SHARDS: usize = 64;

/// Interning statistics for the lane report and `--timings`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InternerStats {
    pub rows: u64,
    pub hits: u64,
    pub row_bytes: u64,
    /// Nanoseconds spent waiting for a shard's write lock.
    pub lock_wait_ns: u64,
    /// Rows whose semantic normal form was computed, and `sem` calls the
    /// memo answered.
    pub sem_computed: u64,
    pub sem_hits: u64,
    /// Graphs validated, and `validate` calls the memo answered.
    pub valid_computed: u64,
    pub valid_hits: u64,
}

/// The session's canonical, concurrent type interner. `Sync`: one per session,
/// shared by every phase and thread.
pub struct TypeInterner {
    base: Option<Arc<TypeBase>>,
    base_len: u32,
    local_start: u32,
    frozen: projection::FrozenRows,
    // Like rustc15c1044 ty/context.rs:140-145,759-763, ids and caches that
    // can allocate ids stay within one owning context. Base TypeIds are stable;
    // their normal forms can refer to private rows beyond the shared prefix.
    base_normals: AppendTable<NormalForms>,
    entries: AppendTable<Entry>,
    assigned: AtomicU32,
    shards: [RwLock<HashMap<u64, Ids, IdHasher>>; SHARDS],
    counters: Counters,
}
#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    row_bytes: AtomicU64,
    lock_wait_ns: AtomicU64,
    sem_computed: AtomicU64,
    sem_hits: AtomicU64,
    valid_computed: AtomicU64,
    valid_hits: AtomicU64,
}

impl TypeInterner {
    pub fn new(base: Option<Arc<TypeBase>>) -> Arc<Self> {
        let base_len = base.as_ref().map_or(0, |b| b.len() as u32);
        Arc::new(Self {
            base,
            base_len,
            local_start: base_len,
            frozen: Default::default(),
            base_normals: AppendTable::new(),
            entries: AppendTable::new(),
            assigned: AtomicU32::new(0),
            shards: [const { RwLock::new(HashMap::with_hasher(IdHasher::new())) }; SHARDS],
            counters: Counters::default(),
        })
    }

    /// The immutable base this interner was built over, if any.
    pub fn base(&self) -> Option<&Arc<TypeBase>> {
        self.base.as_ref()
    }

    fn shard(&self, hash: u64) -> &RwLock<HashMap<u64, Ids, IdHasher>> {
        &self.shards[(hash >> 58) as usize % SHARDS]
    }

    fn find(&self, index: &HashMap<u64, Ids, IdHasher>, hash: u64, row: &Type) -> Option<TypeId> {
        index
            .get(&hash)?
            .iter()
            .find(|id| self.get(*id).is_some_and(|other| other == row))
    }

    fn find_base(
        &self,
        index: &HashMap<u64, Ids, IdHasher>,
        hash: u64,
        row: &Type,
    ) -> Option<TypeId> {
        index.get(&hash)?.iter().find(|id| {
            id.position()
                .is_some_and(|position| position < self.base_len)
                && self.get(*id).is_some_and(|other| other == row)
        })
    }

    /// The id of `row`, exact and lossless: equal rows (Rust `==`) get one id,
    /// and every field, parameter names and labels included, survives.
    pub fn intern(&self, row: Type) -> TypeId {
        let hash = row_hash(&row);
        let source_backed = self.base.as_ref().is_some_and(|base| base.source.is_some());
        if source_backed
            && let Some(id) = self.find_base(
                &self
                    .shard(hash)
                    .read()
                    .unwrap_or_else(PoisonError::into_inner),
                hash,
                &row,
            )
        {
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            return id;
        }
        if let Some(id) = self.base.as_ref().and_then(|b| b.find(hash, &row)) {
            if source_backed {
                // Reuse only a complete canonical base match. Misses and local
                // rows still check the provider, including after missing input
                // becomes available. At most one entry per matched base id is
                // added to the existing shard index; no rows are copied.
                let start = std::time::Instant::now();
                let mut index = self
                    .shard(hash)
                    .write()
                    .unwrap_or_else(PoisonError::into_inner);
                self.counters
                    .lock_wait_ns
                    .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
                match index.get_mut(&hash) {
                    Some(ids) => {
                        if !ids.iter().any(|cached| cached == id) {
                            ids.push(id);
                        }
                    }
                    None => {
                        index.insert(hash, Ids::One(id));
                    }
                }
            }
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            return id;
        }
        let shard = self.shard(hash);
        if let Some(id) = self.find(
            &shard.read().unwrap_or_else(PoisonError::into_inner),
            hash,
            &row,
        ) {
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            return id;
        }
        let start = std::time::Instant::now();
        let mut index = shard.write().unwrap_or_else(PoisonError::into_inner);
        self.counters
            .lock_wait_ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if let Some(id) = self.find(&index, hash, &row) {
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            return id;
        }
        let id = self.publish(row);
        match index.get_mut(&hash) {
            Some(ids) => ids.push(id),
            None => {
                index.insert(hash, Ids::One(id));
            }
        }
        id
    }

    fn publish(&self, row: Type) -> TypeId {
        let local = self.assigned.fetch_add(1, Ordering::Relaxed);
        let position = local
            .checked_add(self.local_start)
            .expect("type interner exceeds u32");
        self.counters
            .row_bytes
            .fetch_add(row_bytes(&row), Ordering::Relaxed);
        self.entries.set(local, Entry::new(row));
        hirsel_intern::work::count(hirsel_intern::work::Counter::InternedTypeRows);
        TypeId::at(position)
    }

    /// Rows that name each other before they exist: a hand-built graph, an
    /// invalid cyclic one included. `build` receives the ids its rows get.
    /// Each row is indexed unless an equal row already has an id, so later
    /// `intern` calls find the first id of each content.
    pub fn intern_graph(
        &self,
        count: usize,
        build: impl FnOnce(&[TypeId]) -> Vec<Type>,
    ) -> Vec<TypeId> {
        let count = u32::try_from(count).expect("type graph exceeds u32");
        let local = self.assigned.fetch_add(count, Ordering::Relaxed);
        let first = local
            .checked_add(self.local_start)
            .and_then(|p| p.checked_add(count))
            .expect("type interner exceeds u32")
            - count;
        let ids: Vec<_> = (first..first + count).map(TypeId::at).collect();
        let rows = build(&ids);
        assert_eq!(rows.len(), ids.len(), "type graph row count changed");
        let hashes: Vec<_> = rows.iter().map(row_hash).collect();
        for (offset, row) in rows.into_iter().enumerate() {
            self.counters
                .row_bytes
                .fetch_add(row_bytes(&row), Ordering::Relaxed);
            self.entries.set(local + offset as u32, Entry::new(row));
        }
        for (id, hash) in ids.iter().zip(hashes) {
            let row = self.row(*id);
            if self.base.as_ref().and_then(|b| b.find(hash, row)).is_some() {
                continue;
            }
            let mut index = self
                .shard(hash)
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if self.find(&index, hash, row).is_none() {
                match index.get_mut(&hash) {
                    Some(ids) => ids.push(*id),
                    None => {
                        index.insert(hash, Ids::One(*id));
                    }
                }
            }
        }
        ids
    }

    pub(crate) fn entry(&self, id: TypeId) -> Option<&Entry> {
        let position = id.position()?;
        if position < self.base_len {
            return self.base.as_ref()?.entry(position);
        }
        self.local_entry(position)
    }

    fn local_entry(&self, position: u32) -> Option<&Entry> {
        if position < self.local_start {
            let index = self.frozen.positions.get(&TypeId::at(position))?;
            self.frozen
                .rows
                .get(*index as usize)
                .map(|(_, entry)| entry)
        } else {
            self.entries.get(position - self.local_start)
        }
    }

    pub(crate) fn normal_forms(&self, id: TypeId) -> Option<&NormalForms> {
        let position = id.position()?;
        if position < self.base_len {
            return Some(
                self.base_normals
                    .get_or_init(position, NormalForms::default),
            );
        }
        self.local_entry(position).map(|entry| &entry.normals)
    }

    /// Inspect an already materialized row without calling a lazy base source.
    /// A metadata visitor must not turn sizing into provider I/O.
    pub fn get_if_loaded(&self, id: TypeId) -> Option<&Type> {
        let position = id.position()?;
        if position < self.base_len {
            self.base
                .as_ref()?
                .entries
                .get(position as usize)?
                .get()
                .map(|entry| &entry.row)
        } else {
            self.local_entry(position).map(|entry| &entry.row)
        }
    }
    /// The row of `id`, or None for an id this interner never assigned.
    pub fn get(&self, id: TypeId) -> Option<&Type> {
        self.entry(id).map(|e| &e.row)
    }

    /// The row of `id`. Panics on an id this interner never assigned; use
    /// `get` or `valid` where a graph may be malformed.
    pub fn row(&self, id: TypeId) -> &Type {
        self.get(id)
            .unwrap_or_else(|| panic!("{id:?} is not a row of this interner"))
    }

    /// Whether `id` names a row of this interner.
    pub fn contains(&self, id: TypeId) -> bool {
        self.entry(id).is_some()
    }

    /// How many rows the session holds, the base included.
    pub fn len(&self) -> usize {
        self.base_len as usize
            + self.frozen.rows.len()
            + self.assigned.load(Ordering::Relaxed) as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// A snapshot of assigned positions, including the immutable prefix.
    pub fn ids(&self) -> impl Iterator<Item = TypeId> {
        (0..self.base_len)
            .map(TypeId::at)
            .chain(self.frozen.rows.iter().map(|(id, _)| *id))
            .chain(
                (0..self.assigned.load(Ordering::Acquire))
                    .map(|local| TypeId::at(self.local_start + local)),
            )
    }

    pub fn stats(&self) -> InternerStats {
        let c = &self.counters;
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        InternerStats {
            rows: self.frozen.rows.len() as u64 + u64::from(self.assigned.load(Ordering::Relaxed)),
            hits: load(&c.hits),
            row_bytes: load(&c.row_bytes) + self.base_normals.allocated_bytes() as u64,
            lock_wait_ns: load(&c.lock_wait_ns),
            sem_computed: load(&c.sem_computed),
            sem_hits: load(&c.sem_hits),
            valid_computed: load(&c.valid_computed),
            valid_hits: load(&c.valid_hits),
        }
    }

    /// Graph validity of `id`: every reachable row exists, none is a recovery
    /// row, and no structural cycle is reachable. Memoized per id.
    pub fn validate(&self, id: TypeId) -> Result<(), GraphError> {
        if let Some(verdict) = self.entry(id).and_then(|e| e.valid.get()) {
            self.counters.valid_hits.fetch_add(1, Ordering::Relaxed);
            return verdict.clone();
        }
        crate::normal::validate(self, id)
    }
    pub(crate) fn count_valid(&self) {
        self.counters.valid_computed.fetch_add(1, Ordering::Relaxed);
    }
    pub(crate) fn count_sem(&self, hit: bool) {
        if hit {
            self.counters.sem_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.sem_computed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `validate(id).is_ok()`.
    pub fn valid(&self, id: TypeId) -> bool {
        self.validate(id).is_ok()
    }

    /// The semantic normal form of `id`: callable parameter names erased,
    /// erased argument captures sorted and de-duplicated, `Known(Unknown)`
    /// captures mapped to None, at every reachable row. Memoized per id. An
    /// invalid graph is its own normal form; `semantic_eq` checks validity.
    pub fn sem(&self, id: TypeId) -> TypeId {
        crate::normal::sem(self, id)
    }

    /// Rooted semantic equality: both graphs valid and equal semantic normal
    /// forms. Nominal identity is the definition; this is not alias expansion,
    /// subtyping or proof search.
    pub fn semantic_eq(&self, a: TypeId, b: TypeId) -> bool {
        self.valid(a) && self.valid(b) && (a == b || self.sem(a) == self.sem(b))
    }

    /// The semantic normal form with every reachable row's modes, crossing
    /// requirement and brand erased: equality modulo modes.
    pub fn strip_modes(&self, id: TypeId) -> TypeId {
        crate::normal::strip_modes(self, id)
    }

    /// The type-shape normal form `shape_eq` compares: constructors, nominal
    /// identity, arguments, generic binders, labels, passing conventions and
    /// layout flags, with every other fact erased.
    pub fn shape(&self, id: TypeId) -> TypeId {
        crate::normal::shape(self, id)
    }

    /// H's rooted type-shape judgment: both graphs valid and equal shape
    /// normal forms. It retains constructors, nominal identity, generic
    /// declarations and arguments, labels, passing conventions and layout
    /// flags; modes, origins, placement, cleanup, capture and allocator
    /// evidence and callable authority, allocation and contracts are judged by
    /// later phases. Semantic equality and receiving relations stay strict.
    pub fn shape_eq(&self, a: TypeId, b: TypeId) -> Result<bool, GraphError> {
        self.validate(a)?;
        self.validate(b)?;
        Ok(a == b || self.shape(a) == self.shape(b))
    }
}

/// Approximate retained bytes of one row: its inline size plus its slices.
fn row_bytes(row: &Type) -> u64 {
    let slices = match &row.kind {
        TypeKind::Tuple(ts) => std::mem::size_of_val(&ts[..]),
        TypeKind::Record(fs) => std::mem::size_of_val(&fs[..]),
        TypeKind::Variant(vs) => {
            std::mem::size_of_val(&vs[..])
                + vs.iter()
                    .map(|v| std::mem::size_of_val(&v.fields[..]))
                    .sum::<usize>()
        }
        TypeKind::Function(f) => {
            std::mem::size_of_val(&f.parameters[..])
                + std::mem::size_of_val(&f.generics[..])
                + std::mem::size_of_val(&f.contracts[..])
                + std::mem::size_of_val(&f.state_promises[..])
        }
        TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
            std::mem::size_of_val(&arguments[..])
        }
        _ => 0,
    };
    (std::mem::size_of::<Entry>() + slices) as u64
}

impl std::fmt::Debug for TypeInterner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeInterner")
            .field("base", &self.base_len)
            .field("rows", &self.assigned.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(test)]
mod source_base_lookup_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    struct Counted {
        candidates: AtomicUsize,
        available: AtomicBool,
    }
    impl TypeBaseSource for Counted {
        fn rows(&self, _: u32, _: &TypeBase) -> Option<Vec<(u32, Type)>> {
            self.available.load(Ordering::Relaxed).then(|| {
                vec![
                    (0, Type::unit()),
                    (1, Type::with_kind(TypeKind::Primitive(Primitive::U8))),
                ]
            })
        }
        fn candidates(&self, _: &Type, _: &TypeBase) -> Vec<u32> {
            self.candidates.fetch_add(1, Ordering::Relaxed);
            if self.available.load(Ordering::Relaxed) {
                vec![0, 1]
            } else {
                vec![]
            }
        }
    }
    fn fixture(available: bool) -> (Arc<Counted>, Arc<TypeBase>, Arc<TypeInterner>) {
        let source = Arc::new(Counted {
            candidates: AtomicUsize::new(0),
            available: AtomicBool::new(available),
        });
        let base = TypeBase::from_source(2, source.clone());
        let types = TypeInterner::new(Some(base.clone()));
        (source, base, types)
    }
    #[test]
    fn successful_base_lookup_is_reused_only_for_equal_rows() {
        let (source, base, types) = fixture(true);
        for _ in 0..32 {
            assert_eq!(types.intern(Type::unit()), base.id_at(0).unwrap());
        }
        assert_eq!(source.candidates.load(Ordering::Relaxed), 1);
        for _ in 0..32 {
            assert_eq!(
                types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8))),
                base.id_at(1).unwrap()
            );
        }
        assert_eq!(source.candidates.load(Ordering::Relaxed), 2);
        assert_eq!(
            types.len(),
            base.len(),
            "matching rows allocate no private types"
        );
    }
    #[test]
    fn base_misses_refresh_and_later_base_match_precedes_existing_private_row() {
        let (source, base, types) = fixture(false);
        let private = types.intern(Type::unit());
        assert_ne!(private, base.id_at(0).unwrap());
        assert_eq!(types.intern(Type::unit()), private);
        assert_eq!(
            source.candidates.load(Ordering::Relaxed),
            2,
            "misses are not retained"
        );
        source.available.store(true, Ordering::Relaxed);
        assert_eq!(types.intern(Type::unit()), base.id_at(0).unwrap());
        for _ in 0..32 {
            assert_eq!(types.intern(Type::unit()), base.id_at(0).unwrap());
        }
        assert_eq!(source.candidates.load(Ordering::Relaxed), 3);
    }
}
