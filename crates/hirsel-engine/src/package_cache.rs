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
    /// The shared cache uses the toolchain's prechecks; `prechecked` names others.
    prechecks: Option<Arc<ToolchainPrechecks>>,
}
/// The toolchain's prechecks: `$HIRSEL_PRECHECK`, or `hirsel-std.precheck` beside the
/// executable. A missing or unreadable precheck only means checking from source.
pub(crate) struct ToolchainPrechecks {
    pub prechecks: Vec<Arc<crate::Precheck>>,
    /// Why there is no precheck, for `--verbose`.
    pub missing: Option<String>,
}
/// Write each later load's precheck notes (`LoadedProgram::precheck_notes`) to stderr.
static REPORT_PRECHECKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The process's shared package cache; sessions without V (`Verify::Off`)
/// share `UNVERIFIED` instead, since V never validated its packages.
static SHARED: OnceLock<PackageCache> = OnceLock::new();
static UNVERIFIED: OnceLock<PackageCache> = OnceLock::new();
/// With a heap meter (`--mem-usage`), drop the process's shared package
/// cache and retained signature inputs, recording the bytes each held. Call
/// it once compiling is over: later programs check their packages afresh.
pub fn release_shared_caches() {
    if !hirsel_intern::memory::metered() {
        return;
    }
    for shared in [&SHARED, &UNVERIFIED] {
        if let Some(entries) = shared.get().and_then(|cache| cache.entries.as_ref()) {
            let storage = std::mem::take(&mut *entries.lock().unwrap_or_else(|e| e.into_inner()));
            hirsel_intern::memory::released("package cache", storage);
        }
    }
    crate::queries::release_signature_inputs();
}
pub fn report_prechecks(on: bool) {
    REPORT_PRECHECKS.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn reporting_prechecks() -> bool {
    REPORT_PRECHECKS.load(std::sync::atomic::Ordering::Relaxed)
}
fn toolchain_prechecks() -> Arc<ToolchainPrechecks> {
    static PRECHECKS: OnceLock<Arc<ToolchainPrechecks>> = OnceLock::new();
    PRECHECKS
        .get_or_init(|| Arc::new(read_toolchain_prechecks()))
        .clone()
}
fn read_toolchain_prechecks() -> ToolchainPrechecks {
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::ImageOpen);
    let path = match std::env::var_os("HIRSEL_PRECHECK") {
        Some(path) => std::path::PathBuf::from(path),
        None => match std::env::current_exe() {
            Ok(exe) => exe.with_file_name("hirsel-std.precheck"),
            Err(e) => {
                return ToolchainPrechecks {
                    prechecks: vec![],
                    missing: Some(format!(
                        "no precheck: the executable's path is unknown ({e})"
                    )),
                };
            }
        },
    };
    match hirsel_intern::memory::retained("precheck", || crate::Precheck::read(&path)) {
        Ok(precheck) => ToolchainPrechecks {
            prechecks: vec![Arc::new(precheck)],
            missing: None,
        },
        Err(e) => ToolchainPrechecks {
            prechecks: vec![],
            missing: Some(format!("no precheck: {e}")),
        },
    }
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
/// Where one publishing engine's anchor stands: in the package closure's
/// source (package, manifest ordinal), at a base offset. A receiving engine
/// places the anchor at its own FileId for that source, so published spans
/// keep their meaning without being rewritten.
pub(crate) type AnchorSite = (hirsel_span::Anchor, PackageId, usize, u32);
pub(crate) struct PublishedLowering {
    pub snapshot: crate::queries::PackageSnapshot,
    pub sites: Arc<[AnchorSite]>,
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
            prechecks: None,
        }
    }
    /// A fresh-check oracle. It neither reads nor publishes checked packages.
    pub fn disabled() -> Self {
        Self {
            entries: None,
            prechecks: None,
        }
    }
    /// The process's cache for sessions with V `verify`: a session without V
    /// never publishes into the cache sessions with V read.
    pub(crate) fn shared(verify: Verify) -> Self {
        if std::env::var_os("HIRSEL_PACKAGE_CACHE").is_some_and(|value| value == "off") {
            return Self::disabled();
        }
        let cache = match verify {
            Verify::On => &SHARED,
            Verify::Off => &UNVERIFIED,
        };
        cache
            .get_or_init(|| Self {
                prechecks: Some(toolchain_prechecks()),
                ..Self::new()
            })
            .clone()
    }
    /// An independent cache whose programs adopt the matching `prechecks`.
    pub fn prechecked(prechecks: Vec<Arc<crate::Precheck>>) -> Self {
        Self {
            prechecks: Some(Arc::new(ToolchainPrechecks {
                prechecks,
                missing: None,
            })),
            ..Self::new()
        }
    }
    pub(crate) fn prechecks(&self) -> Option<&ToolchainPrechecks> {
        self.prechecks.as_deref()
    }
    fn key(revision: ContentHash, limits: hirsel_interp::Limits) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.checked-package.limits.v1",
            [
                revision.0.as_slice(),
                limits
                    .steps
                    .map(|steps| steps.to_le_bytes().to_vec())
                    .unwrap_or_default()
                    .as_slice(),
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
    pub sites: Arc<[AnchorSite]>,
    pub preflight: Vec<Vec<Diagnostic>>,
    pub files: Vec<Vec<Diagnostic>>,
    /// The widest check-time lowering this package's queries satisfy.
    pub lowered: Option<LoweringPolicy>,
    pub last_phase: Option<QueryKind>,
    pub environment: QueryEnvironment,
    pub limits: hirsel_interp::Limits,
}
impl Artifact {
    pub(crate) fn check_complete(&self, required: LoweringPolicy) -> bool {
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
