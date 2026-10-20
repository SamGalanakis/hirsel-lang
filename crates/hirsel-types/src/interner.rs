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
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

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

/// One interned row and its pure per-id memos. Rows are immutable, so each memo
/// is computed at most once per id, by whichever thread first asks.
pub(crate) struct Entry {
    pub(crate) row: Type,
    pub(crate) sem: OnceLock<TypeId>,
    pub(crate) valid: OnceLock<Result<(), GraphError>>,
    pub(crate) strip_modes: OnceLock<TypeId>,
    pub(crate) shape: OnceLock<TypeId>,
    pub(crate) contract: OnceLock<TypeId>,
    pub(crate) actual_key: OnceLock<Option<Arc<str>>>,
}
impl Entry {
    fn new(row: Type) -> Self {
        Self {
            row,
            sem: OnceLock::new(),
            valid: OnceLock::new(),
            strip_modes: OnceLock::new(),
            shape: OnceLock::new(),
            contract: OnceLock::new(),
            actual_key: OnceLock::new(),
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

/// An immutable bottom layer, supplied by the std image: its rows occupy the
/// first positions and its index is read without locks.
pub struct TypeBase {
    entries: Box<[Entry]>,
    index: HashMap<u64, Ids, IdHasher>,
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
            entries: rows.into_iter().map(Entry::new).collect(),
            index,
        })
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    fn find(&self, hash: u64, row: &Type) -> Option<TypeId> {
        self.index
            .get(&hash)?
            .iter()
            .find(|id| self.entries[id.position().expect("a base id") as usize].row == *row)
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
            entries: AppendTable::new(),
            assigned: AtomicU32::new(0),
            shards: [const { RwLock::new(HashMap::with_hasher(IdHasher::new())) }; SHARDS],
            counters: Counters::default(),
        })
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

    /// The id of `row`, exact and lossless: equal rows (Rust `==`) get one id,
    /// and every field, parameter names and labels included, survives.
    pub fn intern(&self, row: Type) -> TypeId {
        let hash = row_hash(&row);
        if let Some(id) = self.base.as_ref().and_then(|b| b.find(hash, &row)) {
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
            .checked_add(self.base_len)
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
            .checked_add(self.base_len)
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
            return self.base.as_ref()?.entries.get(position as usize);
        }
        self.entries.get(position - self.base_len)
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
        self.base_len as usize + self.assigned.load(Ordering::Relaxed) as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stats(&self) -> InternerStats {
        let c = &self.counters;
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        InternerStats {
            rows: u64::from(self.assigned.load(Ordering::Relaxed)),
            hits: load(&c.hits),
            row_bytes: load(&c.row_bytes),
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
