//! Query observations share immutable names while retaining their original
//! structural wire forms. Weak canonicalization never owns a source payload.
use crate::*;
use std::{
    collections::{HashMap, VecDeque, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    num::NonZeroU64,
    ops::Deref,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

/// One retained-allocation inspection. Epochs never repeat; exhaustion falls
/// back to charging every occurrence. Concurrent inspections may overcharge
/// when their marks interleave, but can never omit a first observation.
#[derive(Clone, Copy)]
pub struct ReadAllocationEpoch(Option<NonZeroU64>);
impl Default for ReadAllocationEpoch {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(
            NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .ok()
            .and_then(NonZeroU64::new),
        )
    }
}
struct Marked<T> {
    value: T,
    inspected: AtomicU64,
}
impl<T> Marked<T> {
    fn first(&self, epoch: ReadAllocationEpoch) -> bool {
        epoch
            .0
            .is_none_or(|epoch| self.inspected.swap(epoch.get(), Ordering::Relaxed) != epoch.get())
    }
}
impl<T> Deref for Marked<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl<T: PartialEq> PartialEq for Marked<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T: Eq> Eq for Marked<T> {}
impl<T: std::fmt::Debug> std::fmt::Debug for Marked<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(formatter)
    }
}
impl<T: serde::Serialize> serde::Serialize for Marked<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}

struct Names<T> {
    entries: HashMap<u64, Vec<Weak<Marked<T>>>>,
    rows: usize,
    recent: VecDeque<u64>,
}
struct NamePool<T>(OnceLock<Mutex<Names<T>>>);
impl<T: Eq + Hash> NamePool<T> {
    const fn new() -> Self {
        Self(OnceLock::new())
    }
    fn intern(&self, value: T) -> Arc<Marked<T>> {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        let hash = hasher.finish();
        let mut names = self
            .0
            .get_or_init(|| {
                Mutex::new(Names {
                    entries: HashMap::new(),
                    rows: 0,
                    recent: VecDeque::with_capacity(4096),
                })
            })
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(bucket) = names.entries.get_mut(&hash) {
            let before = bucket.len();
            let mut found = None;
            bucket.retain(|weak| {
                let Some(candidate) = weak.upgrade() else {
                    return false;
                };
                if candidate.value == value {
                    found = Some(candidate);
                }
                true
            });
            let removed = before - bucket.len();
            names.rows -= removed;
            if let Some(found) = found {
                return found;
            }
        }
        // Rust's bounded FIFO keeps nearby observations available instead of
        // discarding every live match when one new name fills the weak index.
        // Eviction changes sharing only; readers own every payload strongly.
        if names.recent.len() == 4096 {
            let oldest = names.recent.pop_front().unwrap();
            if let Some(evicted) = names.entries.remove(&oldest) {
                names.rows -= evicted.len();
            }
        }
        names.recent.push_back(hash);
        let value = Arc::new(Marked {
            value,
            inspected: AtomicU64::new(0),
        });
        names
            .entries
            .entry(hash)
            .or_default()
            .push(Arc::downgrade(&value));
        names.rows += 1;
        value
    }
}
fn arc_bytes<T>(value: &T) -> usize {
    std::alloc::Layout::new::<[usize; 2]>()
        .extend(std::alloc::Layout::for_value(value))
        .expect("allocated Arc layout")
        .0
        .pad_to_align()
        .size()
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReadContext(Arc<Marked<FileContext>>);
impl From<FileContext> for ReadContext {
    fn from(value: FileContext) -> Self {
        static CONTEXTS: NamePool<FileContext> = NamePool::new();
        Self(CONTEXTS.intern(value))
    }
}
impl ReadContext {
    pub fn first_in(&self, epoch: ReadAllocationEpoch) -> bool {
        self.0.first(epoch)
    }
    pub fn retained_allocations(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let context = &*self.0;
        [
            (Arc::as_ptr(&self.0) as usize, arc_bytes(context)),
            (
                context.package.name.as_ptr() as usize,
                context.package.name.capacity(),
            ),
            (
                context.package.version.as_ptr() as usize,
                context.package.version.capacity(),
            ),
            (
                context.modules.as_ptr() as usize,
                context.modules.capacity() * size_of::<String>(),
            ),
        ]
        .into_iter()
        .chain(
            context
                .modules
                .iter()
                .map(|name| (name.as_ptr() as usize, name.capacity())),
        )
    }
}
impl Deref for ReadContext {
    type Target = FileContext;
    fn deref(&self) -> &FileContext {
        &self.0
    }
}
impl std::fmt::Debug for ReadContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
impl PartialEq<FileContext> for ReadContext {
    fn eq(&self, other: &FileContext) -> bool {
        &self.0.value == other
    }
}
impl serde::Serialize for ReadContext {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for ReadContext {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        FileContext::deserialize(deserializer).map(Into::into)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReadSegments(Arc<Marked<Vec<String>>>);
impl From<Vec<String>> for ReadSegments {
    fn from(value: Vec<String>) -> Self {
        static SEGMENTS: NamePool<Vec<String>> = NamePool::new();
        Self(SEGMENTS.intern(value))
    }
}
impl ReadSegments {
    pub fn first_in(&self, epoch: ReadAllocationEpoch) -> bool {
        self.0.first(epoch)
    }
    pub fn retained_allocations(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        [
            (Arc::as_ptr(&self.0) as usize, arc_bytes(&*self.0)),
            (
                self.0.as_ptr() as usize,
                self.0.capacity() * size_of::<String>(),
            ),
        ]
        .into_iter()
        .chain(
            self.0
                .iter()
                .map(|name| (name.as_ptr() as usize, name.capacity())),
        )
    }
}
impl Deref for ReadSegments {
    type Target = [String];
    fn deref(&self) -> &[String] {
        &self.0
    }
}
impl std::fmt::Debug for ReadSegments {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
impl<const N: usize> PartialEq<[&str; N]> for ReadSegments {
    fn eq(&self, other: &[&str; N]) -> bool {
        self.iter().map(String::as_str).eq(other.iter().copied())
    }
}
impl serde::Serialize for ReadSegments {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for ReadSegments {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<String>::deserialize(deserializer).map(Into::into)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ReadPath {
    definition: hirsel_syntax::DefId,
}
impl From<DefPath> for ReadPath {
    fn from(path: DefPath) -> Self {
        Self {
            definition: hirsel_syntax::DefId::intern(&path),
        }
    }
}
impl Deref for ReadPath {
    type Target = DefPath;
    fn deref(&self) -> &DefPath {
        self.definition.path()
    }
}
impl std::fmt::Debug for ReadPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.definition.path().fmt(formatter)
    }
}
impl AsRef<DefPath> for ReadPath {
    fn as_ref(&self) -> &DefPath {
        self
    }
}
impl std::borrow::Borrow<DefPath> for ReadPath {
    fn borrow(&self) -> &DefPath {
        self
    }
}
impl PartialEq<DefPath> for ReadPath {
    fn eq(&self, other: &DefPath) -> bool {
        self.definition.path() == other
    }
}
impl serde::Serialize for ReadPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.definition.path().serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for ReadPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        DefPath::deserialize(deserializer).map(Into::into)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Eq, PartialEq)]
pub struct ReadLookupOutput {
    pub manifest: ContentHash,
    pub result: Result<ReadPath, QueryError>,
}
impl std::fmt::Debug for ReadLookupOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LookupOutput")
            .field("manifest", &self.manifest)
            .field("result", &self.result)
            .finish()
    }
}
impl From<LookupOutput> for ReadLookupOutput {
    fn from(output: LookupOutput) -> Self {
        Self {
            manifest: output.manifest,
            result: output.result.map(Into::into),
        }
    }
}

impl Hash for DependencyRead {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::ProviderLookup {
                context,
                package,
                segments,
                output,
            } => {
                (**context).hash(state);
                package.hash(state);
                segments.deref().hash(state);
                output.manifest.0.hash(state);
            }
            Self::Lookup {
                context,
                package,
                segments,
                manifest,
                ..
            } => {
                (**context).hash(state);
                package.hash(state);
                segments.deref().hash(state);
                manifest.0.hash(state);
            }
            Self::ModuleHead {
                context,
                head,
                manifest,
                exposed,
            } => {
                (**context).hash(state);
                head.hash(state);
                manifest.0.hash(state);
                exposed.hash(state);
            }
            Self::Signature {
                definition,
                fingerprint,
            } => {
                definition.hash(state);
                fingerprint.map(|hash| hash.0).hash(state);
            }
        }
    }
}
impl AsRef<DependencyRead> for DependencyRead {
    fn as_ref(&self) -> &DependencyRead {
        self
    }
}
impl DependencyRead {
    pub fn shared(self) -> SharedRead {
        static READS: NamePool<DependencyRead> = NamePool::new();
        SharedRead(READS.intern(self))
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SharedRead(Arc<Marked<DependencyRead>>);
impl SharedRead {
    pub fn first_in(&self, epoch: ReadAllocationEpoch) -> bool {
        self.0.first(epoch)
    }
    pub fn retained_allocation(&self) -> (usize, usize) {
        (Arc::as_ptr(&self.0) as usize, arc_bytes(&*self.0))
    }
}
impl Deref for SharedRead {
    type Target = DependencyRead;
    fn deref(&self) -> &DependencyRead {
        &self.0
    }
}
impl AsRef<DependencyRead> for SharedRead {
    fn as_ref(&self) -> &DependencyRead {
        self
    }
}
impl std::fmt::Debug for SharedRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
impl serde::Serialize for SharedRead {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for SharedRead {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        DependencyRead::deserialize(deserializer).map(DependencyRead::shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_epochs_charge_each_first_visit_and_concurrent_interference_conservatively() {
        let value = Marked {
            value: (),
            inspected: AtomicU64::new(0),
        };
        let a = ReadAllocationEpoch::default();
        let b = ReadAllocationEpoch::default();
        assert!(value.first(a));
        assert!(!value.first(a));
        assert!(value.first(b));
        assert!(value.first(a));
        assert!(value.first(b));
        assert!(!value.first(b));
        assert!(value.first(ReadAllocationEpoch(None)));
        assert!(value.first(ReadAllocationEpoch(None)));
    }

    #[test]
    fn bounded_weak_index_replaces_one_old_bucket_and_keeps_nearby_matches() {
        let pool = NamePool::new();
        let live: Vec<_> = (0..4096)
            .map(|i| pool.intern(format!("name_{i}")))
            .collect();
        let next = pool.intern("next".to_owned());
        assert!(Arc::ptr_eq(
            &live[4095],
            &pool.intern((*live[4095]).clone())
        ));
        assert!(!Arc::ptr_eq(&live[0], &pool.intern((*live[0]).clone())));
        let names = pool.0.get().unwrap().lock().unwrap();
        assert!(names.rows <= 4096);
        assert!(names.entries.len() <= 4096);
        assert_eq!(names.recent.len(), 4096);
        drop(names);
        let weak = Arc::downgrade(&next);
        drop(next);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn complete_read_sharing_compares_results_and_releases_the_last_owner() {
        let reads = NamePool::new();
        let context = FileContext {
            package: DefPath::core("read_names.collision").0.package.clone(),
            modules: vec![],
        };
        let missing = DependencyRead::Lookup {
            context: context.into(),
            package: None,
            segments: vec!["value".into()].into(),
            manifest: ContentHash::of_bytes(b"same manifest"),
            result: Err(LookupError::Missing),
        };
        let mut found = missing.clone();
        if let DependencyRead::Lookup { result, .. } = &mut found {
            *result = Ok(DefPath::core("read_names.value").into());
        }
        let hash = |read: &DependencyRead| {
            let mut hasher = DefaultHasher::new();
            read.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash(&missing), hash(&found));
        let first = SharedRead(reads.intern(missing.clone()));
        let again = SharedRead(reads.intern(missing));
        let different = SharedRead(reads.intern(found));
        assert!(Arc::ptr_eq(&first.0, &again.0));
        assert!(!Arc::ptr_eq(&first.0, &different.0));
        assert_ne!(first, different);
        let weak = Arc::downgrade(&first.0);
        drop(first);
        assert!(weak.upgrade().is_some());
        drop(again);
        assert!(weak.upgrade().is_none());
        let weak = Arc::downgrade(&different.0);
        drop(different);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn read_names_share_original_owners_and_keep_segment_and_context_boundaries() {
        let path = DefPath::core("read_names.owner");
        let context = FileContext {
            package: path.0.package.clone(),
            modules: vec!["a.b".into(), "c".into()],
        };
        let contexts = NamePool::new();
        let first = ReadContext(contexts.intern(context.clone()));
        let again = ReadContext(contexts.intern(context.clone()));
        assert!(Arc::ptr_eq(&first.0, &again.0));
        let weak_context = Arc::downgrade(&first.0);
        let other: ReadContext = FileContext {
            modules: vec!["a".into(), "b.c".into()],
            ..context
        }
        .into();
        assert_ne!(first, other);
        drop((first, again));
        assert!(weak_context.upgrade().is_none());
        let segments = NamePool::new();
        let first = ReadSegments(segments.intern(vec!["a.b".into(), "c".into()]));
        let again = ReadSegments(segments.intern(vec!["a.b".into(), "c".into()]));
        assert!(Arc::ptr_eq(&first.0, &again.0));
        let weak_segments = Arc::downgrade(&first.0);
        assert_ne!(first, ReadSegments::from(vec!["a".into(), "b.c".into()]));
        drop((first, again));
        assert!(weak_segments.upgrade().is_none());
        let name: ReadPath = path.clone().into();
        assert!(std::ptr::eq(
            &*name,
            hirsel_syntax::DefId::intern(&path).path()
        ));
    }
}
