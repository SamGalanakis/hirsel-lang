//! `DefId`: the process-global session identity of a `DefPath`. The path stays
//! the persisted, hashed, printed and mangled form; the id is only a cheap key.
use crate::{ContentHash, DefPath};
use hirsel_intern::{AppendTable, IdHasher, Scramble, persist};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering as Atomic};
use std::sync::{Arc, PoisonError, RwLock};

/// An interned definition path. Eq and Hash compare ids; Ord compares paths,
/// so ordered maps iterate in DefPath order whatever order ids were assigned in.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct DefId(NonZeroU32);

/// A hot table keyed by definition, in unspecified order: sort before output.
pub type DefMap<T> = HashMap<DefId, T, IdHasher>;

/// One interned path with the forms derived from it, computed once.
struct Entry {
    path: DefPath,
    fingerprint: ContentHash,
    display: Box<str>,
}

const SHARDS: usize = 16;

struct Defs {
    shards: [RwLock<HashMap<&'static DefPath, DefId, IdHasher>>; SHARDS],
    assigned: AtomicU32,
    entries: AppendTable<&'static Entry>,
    scramble: Scramble,
}

static DEFS: Defs = Defs {
    shards: [const { RwLock::new(HashMap::with_hasher(BuildHasherDefault::new())) }; SHARDS],
    assigned: AtomicU32::new(0),
    entries: AppendTable::new(),
    scramble: Scramble::new("def"),
};

fn shard(path: &DefPath) -> &'static RwLock<HashMap<&'static DefPath, DefId, IdHasher>> {
    let hash = IdHasher::default().hash_one(path);
    &DEFS.shards[(hash >> 60) as usize % SHARDS]
}

impl DefId {
    /// The id for `path`, assigning one the first time this process sees it.
    pub fn intern(path: &DefPath) -> DefId {
        let shard = shard(path);
        if let Some(&id) = shard
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(path)
        {
            return id;
        }
        let mut index = shard.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(&id) = index.get(path) {
            return id;
        }
        let position = DEFS.assigned.fetch_add(1, Atomic::Relaxed);
        let entry: &'static Entry = Box::leak(Box::new(Entry {
            path: path.clone(),
            fingerprint: path.fingerprint(),
            display: path.to_string().into(),
        }));
        DEFS.entries.set(position, entry);
        let id = DefId(DEFS.scramble.encode(position));
        index.insert(&entry.path, id);
        id
    }

    /// The id already assigned to `path`, if any.
    pub fn get(path: &DefPath) -> Option<DefId> {
        shard(path)
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(path)
            .copied()
    }

    fn entry(self) -> &'static Entry {
        DEFS.entries
            .get(DEFS.scramble.decode(self.0))
            .expect("a DefId names a path interned in this process")
    }

    pub fn path(self) -> &'static DefPath {
        &self.entry().path
    }

    /// `DefPath::fingerprint`, cached.
    pub fn fingerprint(self) -> ContentHash {
        self.entry().fingerprint
    }

    /// The `DefPath` Display form, cached.
    pub fn as_str(self) -> &'static str {
        &self.entry().display
    }

    /// How many paths this process has interned.
    pub fn count() -> usize {
        DEFS.assigned.load(Atomic::Relaxed) as usize
    }

    /// `DefPath::is_core` of the path.
    pub fn is_core(self, name: &str) -> bool {
        self.path().is_core(name)
    }
}

impl From<&DefPath> for DefId {
    fn from(path: &DefPath) -> Self {
        DefId::intern(path)
    }
}
impl From<DefPath> for DefId {
    fn from(path: DefPath) -> Self {
        DefId::intern(&path)
    }
}

impl Ord for DefId {
    fn cmp(&self, other: &Self) -> Ordering {
        if self == other {
            return Ordering::Equal;
        }
        self.path().cmp(other.path())
    }
}

impl PartialOrd for DefId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for DefId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for DefId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DefId").field(&self.as_str()).finish()
    }
}

#[cfg(test)]
mod tests;

thread_local! {
    static PERSIST: persist::Scopes<DefId> = const { std::cell::RefCell::new(vec![]) };
}

/// Run `f` with every `DefId` serializing as its number in `numbering`: a
/// persisted form that writes its definition table beside it (the std image).
pub fn with_def_numbering<R>(
    numbering: &mut persist::Numbering<DefId>,
    f: impl FnOnce() -> R,
) -> R {
    persist::with_numbering(&PERSIST, numbering, f)
}

/// Run `f` with every `DefId` deserializing from a number through `source`.
pub fn with_def_source<R>(source: Arc<dyn persist::Source<DefId>>, f: impl FnOnce() -> R) -> R {
    persist::with_source(&PERSIST, source, f)
}

/// Run `f` with definitions persisted as complete paths, even inside a table.
pub fn with_def_paths<R>(f: impl FnOnce() -> R) -> R {
    persist::with_inline(&PERSIST, f)
}

/// Persisted as its DefPath, or as its number in a persisted definition table
/// (`with_def_numbering`); never as a session id.
impl serde::Serialize for DefId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match persist::encoded(&PERSIST, *self) {
            Some(number) => s.serialize_u32(number),
            None => serde::Serialize::serialize(self.path(), s),
        }
    }
}
impl<'de> serde::Deserialize<'de> for DefId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if let Some(source) = persist::decoding(&PERSIST) {
            let number = <u32 as serde::Deserialize>::deserialize(d)?;
            return source
                .id(number)
                .ok_or_else(|| serde::de::Error::custom("definition number out of range"));
        }
        let path: DefPath = serde::Deserialize::deserialize(d)?;
        Ok(DefId::intern(&path))
    }
}
