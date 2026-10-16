//! Bounded, process-local checked-package storage. Paths locate inputs; digests validate them.
use crate::*;
use hirsel_syntax::PackageId;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone)]
pub struct PackageCache {
    entries: Option<Arc<Mutex<Storage>>>,
}
#[derive(Default)]
struct Storage {
    artifacts: BTreeMap<ContentHash, CachedPackage>,
    recent: VecDeque<ContentHash>,
    /// Shared dependency state by `ImageInput::key`, most recent last.
    images: VecDeque<(ContentHash, Arc<crate::queries::PackageImage>)>,
}
#[derive(Clone, Default)]
pub(crate) struct CachedPackage {
    pub artifact: Option<Arc<Artifact>>,
    pub lowerings: SharedLowerings,
}
pub(crate) type SharedLowerings = Arc<Mutex<BTreeMap<DefPath, Arc<PublishedLowering>>>>;
pub(crate) struct PublishedLowering {
    pub snapshot: crate::queries::PackageSnapshot,
    pub sites: Arc<[(FileId, PackageId, usize, u32)]>,
}
impl Storage {
    fn reserve(&mut self, key: ContentHash) {
        // Live engines can retain an evicted publication handle, but cannot
        // resurrect the entry or replace a newer revision through that handle.
        if self.artifacts.len() >= 32 && !self.artifacts.contains_key(&key) {
            let oldest = self.recent.pop_front().unwrap();
            self.artifacts.remove(&oldest);
        }
    }
    fn retain(&mut self, key: ContentHash) {
        self.recent.retain(|old| *old != key);
        self.recent.push_back(key);
    }
}
impl Default for PackageCache {
    fn default() -> Self {
        Self::new()
    }
}
impl PackageCache {
    pub(crate) fn enabled(&self) -> bool {
        self.entries.is_some()
    }
    /// An independent cache, useful for explicit long-lived build sessions.
    pub fn new() -> Self {
        Self {
            entries: Some(Arc::new(Mutex::new(Storage::default()))),
        }
    }
    /// A fresh-check oracle. It neither reads nor publishes checked packages.
    pub fn disabled() -> Self {
        Self { entries: None }
    }
    pub(crate) fn shared() -> Self {
        if std::env::var_os("HIRSEL_PACKAGE_CACHE").is_some_and(|value| value == "off") {
            return Self::disabled();
        }
        static CACHE: OnceLock<PackageCache> = OnceLock::new();
        CACHE.get_or_init(Self::new).clone()
    }
    fn key(revision: ContentHash, limits: hirsel_interp::Limits) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.checked-package.limits.v1",
            [
                revision.0.as_slice(),
                limits.steps.to_le_bytes().as_slice(),
                (limits.call_depth as u64).to_le_bytes().as_slice(),
            ],
        )
    }
    pub(crate) fn select(
        &self,
        key: ContentHash,
        limits: hirsel_interp::Limits,
    ) -> Option<CachedPackage> {
        let mut storage = self.entries.as_ref()?.lock().unwrap();
        let key = Self::key(key, limits);
        storage.reserve(key);
        let package = storage.artifacts.entry(key).or_default().clone();
        storage.retain(key);
        Some(package)
    }
    pub(crate) fn insert(&self, key: ContentHash, artifact: Artifact) {
        let Some(entries) = &self.entries else {
            return;
        };
        let key = Self::key(key, artifact.limits);
        let mut entries = entries.lock().unwrap();
        // Retention is bounded independently of corpus/variation size. Published
        // artifacts remain alive while adopted programs retain their handles.
        entries.reserve(key);
        entries.retain(key);
        entries.artifacts.entry(key).or_default().artifact = Some(Arc::new(artifact));
    }
}
impl PackageCache {
    pub(crate) fn image(&self, key: ContentHash) -> Option<Arc<crate::queries::PackageImage>> {
        let storage = self.entries.as_ref()?.lock().unwrap();
        storage
            .images
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, image)| image.clone())
    }
    /// Retention is bounded; a live engine keeps its own image alive.
    pub(crate) fn store_image(&self, key: ContentHash, image: Arc<crate::queries::PackageImage>) {
        let Some(entries) = &self.entries else {
            return;
        };
        let mut storage = entries.lock().unwrap();
        storage.images.retain(|(k, _)| *k != key);
        if storage.images.len() >= 8 {
            storage.images.pop_front();
        }
        storage.images.push_back((key, image));
    }
}
pub(crate) struct Artifact {
    pub snapshot: crate::queries::PackageSnapshot,
    pub sites: Vec<(FileId, PackageId, usize, u32)>,
    pub preflight: Vec<Vec<Diagnostic>>,
    pub files: Vec<Vec<Diagnostic>>,
    /// The widest check-time lowering this package's queries satisfy.
    pub lowered: Option<crate::queries::Lowering>,
    pub last_phase: Option<QueryKind>,
    pub environment: QueryEnvironment,
    pub limits: hirsel_interp::Limits,
}
impl Artifact {
    pub(crate) fn check_complete(&self, required: crate::queries::Lowering) -> bool {
        // A program-wide prerequisite error can withhold lowering in a fresh
        // check. Replay rejected queries through the ordinary phase order;
        // replaying their file diagnostics could expose a cached CT-1 early.
        self.lowered >= Some(required) && !self.files.iter().any(|d| crate::queries::has_errors(d))
    }
}
pub(crate) struct PackageRevision {
    pub package: PackageId,
    pub key: ContentHash,
    pub closure: BTreeSet<PackageId>,
    pub files: Vec<(FileId, FileContext)>,
    pub artifact: Option<Arc<Artifact>>,
    pub lowerings: Option<SharedLowerings>,
    pub reused: bool,
    /// The program's image validated this package with closed reads.
    pub proven: bool,
    pub diagnostics: Option<PackageDiagnostics>,
}
pub(crate) struct PackageDiagnostics {
    pub preflight: Vec<Vec<Diagnostic>>,
    pub files: Vec<Vec<Diagnostic>>,
}
