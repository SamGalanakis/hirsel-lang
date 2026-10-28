//! Immutable compiler products, independently addressed actions and checked maps.
//!
//! Go ebe2feefc400f813aefdc8a036e66d2619da6649, cache.go:28-34,200-255:
//! an action association is separate from the content it names. Lean
//! 64942e142f2cbd3576ad89c3df41f8b6ccee91d9, compact.cpp:137,183 and
//! module.cpp:500: retain immutable shared regions and map only demanded ones.
//! Unlike Lean, references are checked portable offsets, never saved addresses.
//! Integrity is not verifier authority: this module does not issue V receipts.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_syntax::ContentHash;
use std::fs::{self, File};
use std::io::{self, Write};
use std::ops::{Deref, Range};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The largest independently mapped record. Oversized queries must split their
/// output into referenced records; packing never builds another whole program.
pub const MAX_RECORD: usize = 16 << 20;
const MAGIC: &[u8; 8] = b"HIRART02";
const HEADER: usize = 64;
const ROOT: &[u8; 8] = b"HIRACT02";
const ROOT_LEN: usize = 152;

mod group;
mod index;
mod pack;
pub use pack::{Batch, Request};
mod statistics;
use statistics::Counter;
pub use statistics::{Statistics, StatisticsSnapshot};

/// Stable query address, used before its previous dependencies are evaluated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Address(pub ContentHash);
/// Complete computation identity, including all observed dependency outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action(pub ContentHash);
/// Canonical output identity. Equal recomputation retains this identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Output(pub ContentHash);

/// A content domain. Every referenced record checks both kind and schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Domain {
    pub kind: u32,
    pub schema: u32,
}

/// A reference cannot supply proof authority. Producers and verifier-owned
/// associations use separate roots; imported bytes create no action association.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Reference {
    pub output: Output,
    pub domain: Domain,
    pub len: u64,
}

/// Content access supplies integrity, never query or verifier authority.
pub trait Contents: Send + Sync {
    fn get_content(&self, reference: Reference) -> Option<Bytes>;
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference>;
}

#[derive(Default)]
pub struct Memory {
    entries: std::sync::Mutex<std::collections::HashMap<ContentHash, (Reference, Bytes)>>,
}
impl Memory {
    /// Snapshot references and shared payload views for bounded toolchain packing.
    pub fn entries(&self) -> Vec<(Reference, Bytes)> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }
}
/// Ordinary store containers embedded by the toolchain build. Entries are
/// sorted by output digest; complete container validation occurs on demand.
pub struct Embedded(pub &'static [(Reference, &'static [u8])]);
impl Contents for Embedded {
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        let n = self
            .0
            .binary_search_by_key(&reference.output.0, |(r, _)| r.output.0)
            .ok()?;
        let (actual, bytes) = self.0[n];
        if actual != reference {
            return None;
        }
        checked_content(Arc::new(Region::Static(bytes)), reference)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        let reference = domain.reference(bytes)?;
        self.get_content(reference).map(|_| reference)
    }
}
impl Contents for Memory {
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let (actual, bytes) = entries.get(&reference.output.0)?;
        (*actual == reference).then(|| bytes.clone())
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        let reference = domain.reference(bytes)?;
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reference.output.0, (reference, bytes.into()));
        Some(reference)
    }
}

impl Domain {
    /// The canonical reference to these bytes, without publication or authority.
    pub fn reference(self, bytes: &[u8]) -> Option<Reference> {
        (bytes.len() <= MAX_RECORD).then(|| Reference {
            output: self.output(bytes),
            domain: self,
            len: bytes.len() as u64,
        })
    }
    fn output(self, bytes: &[u8]) -> Output {
        Output(ContentHash::of_parts(
            "hirsel.artifact.output.v1",
            [
                self.kind.to_le_bytes().as_slice(),
                self.schema.to_le_bytes().as_slice(),
                bytes,
            ],
        ))
    }
}

enum Region {
    #[cfg(unix)]
    Map {
        address: *mut libc::c_void,
        len: usize,
    },
    Owned(Arc<[u8]>),
    Buffer(Box<[u8]>),
    Static(&'static [u8]),
    Checked(Box<CheckedRegion>),
    Retained {
        region: Arc<Region>,
        _owner: Arc<dyn Send + Sync>,
    },
}

// Rust std 1.90.0, sync/once_lock.rs:151,309: a successful initialization
// belongs to its immutable owner. Slots only avoid repeated integrity work;
// collisions fall back to the complete SHA-256 check. No action is memoized.
struct CheckedRegion {
    region: Arc<Region>,
    records: [std::sync::OnceLock<CheckedContainer>; 16],
    statistics: Option<Arc<Statistics>>,
}
struct CheckedContainer {
    range: Range<usize>,
    reference: Reference,
}
// SAFETY: mappings are read-only; publication replaces files rather than
// modifying a mapped inode. Every view retains an Arc to its mapping.
unsafe impl Send for Region {}
// SAFETY: no mutable mapping or pointer escapes this module.
unsafe impl Sync for Region {}

impl Region {
    fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = usize::try_from(file.metadata()?.len()).map_err(|_| invalid())?;
        if !(HEADER..=HEADER + MAX_RECORD).contains(&len) {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: nonempty regular file, read-only mapping, bounded length.
            let address = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ,
                    libc::MAP_PRIVATE,
                    file.as_raw_fd(),
                    0,
                )
            };
            if address == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            Ok(Self::Map { address, len })
        }
        #[cfg(not(unix))]
        {
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take((HEADER + MAX_RECORD + 1) as u64)
                .read_to_end(&mut bytes)?;
            (bytes.len() == len)
                .then_some(Self::Owned(bytes.into()))
                .ok_or_else(invalid)
        }
    }
    fn bytes(&self) -> &[u8] {
        match self {
            #[cfg(unix)]
            Self::Map { address, len } => {
                // SAFETY: this object owns the mapping; returned borrow is bounded
                // by both its length and lifetime.
                unsafe { std::slice::from_raw_parts(address.cast(), *len) }
            }
            Self::Owned(bytes) => bytes,
            Self::Buffer(bytes) => bytes,
            Self::Static(bytes) => bytes,
            Self::Checked(checked) => checked.region.bytes(),
            Self::Retained { region, .. } => region.bytes(),
        }
    }
    fn checks(&self) -> Option<&CheckedRegion> {
        match self {
            Self::Checked(checked) => Some(checked),
            Self::Retained { region, .. } => region.checks(),
            _ => None,
        }
    }
}
impl Drop for Region {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            if let Self::Map { address, len } = *self {
                // SAFETY: exactly the mapping allocated in open, with no live views.
                unsafe { libc::munmap(address, len) };
            }
        }
    }
}

/// An immutable, borrowed range of a shared mapping. Cloning does not read pages
/// or allocate a payload. The semantic decoder decides which bytes it needs.
#[derive(Clone)]
pub struct Bytes {
    region: Arc<Region>,
    range: Range<usize>,
}
impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bytes").field("len", &self.len()).finish()
    }
}
impl PartialEq for Bytes {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}
impl Eq for Bytes {}
impl From<Arc<[u8]>> for Bytes {
    fn from(bytes: Arc<[u8]>) -> Self {
        let len = bytes.len();
        Self {
            region: Arc::new(Region::Owned(bytes)),
            range: 0..len,
        }
    }
}
impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        Self {
            region: Arc::new(Region::Buffer(bytes.into_boxed_slice())),
            range: 0..len,
        }
    }
}
impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self::from(Arc::<[u8]>::from(bytes))
    }
}
impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.region.bytes()[self.range.clone()]
    }
}
impl Bytes {
    /// Retain an admission reservation or other lifetime owner through clones
    /// and child ranges without copying the underlying immutable bytes.
    pub fn retain_owner(self, owner: Arc<dyn Send + Sync>) -> Self {
        Self {
            region: Arc::new(Region::Retained {
                region: self.region,
                _owner: owner,
            }),
            range: self.range,
        }
    }

    /// A checked child range; both the offset and length are portable integers.
    /// Alignment describes the archive layout, not a cast to a Rust heap type.
    pub fn range(&self, offset: u64, len: u64, alignment: u64) -> Option<Self> {
        let absolute = (self.range.start as u64).checked_add(offset)?;
        if alignment == 0 || !alignment.is_power_of_two() || absolute % alignment != 0 {
            return None;
        }
        let start = usize::try_from(offset).ok()?;
        let end = usize::try_from(offset.checked_add(len)?).ok()?;
        if end > self.len() {
            return None;
        }
        Some(Self {
            region: self.region.clone(),
            range: self.range.start.checked_add(start)?..self.range.start.checked_add(end)?,
        })
    }
}

/// One physical store. A namespace is an authority boundary for associations,
/// not part of content addressing: the same immutable bytes may be shared.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    durability: Durability,
    packed: Arc<std::sync::Mutex<pack::State>>,
    statistics: Arc<std::sync::OnceLock<Arc<Statistics>>>,
    regions: Arc<std::sync::Mutex<RegionCache>>,
    collection_cap: Arc<AtomicU64>,
    cache_cap: Arc<AtomicU64>,
    disk_estimate: Arc<AtomicU64>,
}
const REGION_BYTES: usize = 32 << 20;
const REGION_COUNT: usize = 64;
#[derive(Default)]
struct RegionCache {
    active: bool,
    bytes: usize,
    entries: std::collections::BTreeMap<PathBuf, Arc<Region>>,
}
impl RegionCache {
    fn reset(&mut self, active: bool) {
        self.active = active;
        self.entries.clear();
        self.bytes = 0;
    }
    fn insert(&mut self, path: PathBuf, region: Arc<Region>) {
        let len = region.bytes().len();
        if len > REGION_BYTES {
            return;
        }
        while self.entries.len() >= REGION_COUNT || self.bytes + len > REGION_BYTES {
            let Some((_, evicted)) = self.entries.pop_first() else {
                return;
            };
            self.bytes -= evicted.bytes().len();
        }
        self.bytes += len;
        self.entries.insert(path, region);
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Durability {
    Strict,
    Rebuildable,
}
impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}
impl Contents for Store {
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        self.get(reference)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        self.put(domain, bytes).ok()
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GcReport {
    pub entries: u64,
    pub bytes: u64,
    pub removed: u64,
    pub removed_bytes: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Association {
    pub action: Action,
    pub reference: Reference,
}
/// A routing association and its already-open output container. Integrity is
/// checked by `get`; query actions and verifier authority remain caller-owned.
pub struct OpenedAssociation {
    pub association: Association,
    container: Bytes,
}
impl OpenedAssociation {
    pub fn get(self) -> Option<Bytes> {
        checked_container(self.container, self.association.reference)
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid artifact store record")
}
fn hex(hash: ContentHash) -> String {
    hash.0.iter().map(|b| format!("{b:02x}")).collect()
}
fn number(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}
fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

impl Store {
    /// Start an independent operation over the same physical namespace. Only
    /// clones of this handle join its epochs and pins. Independent operations
    /// share capacity policy and counters, never finish or abort authority.
    pub fn for_operation(&self) -> Self {
        Self {
            packed: Arc::default(),
            regions: Arc::default(),
            ..self.clone()
        }
    }
    pub fn new(root: PathBuf) -> Self {
        Self::with_durability(root, Durability::Strict)
    }
    /// A derivable cache: complete writes and atomic replacement, without file
    /// or directory sync. Losing any publication must be a checked cache miss.
    /// Code distribution and sole-copy storage must use `new` instead.
    pub fn rebuildable(root: PathBuf) -> Self {
        Self::with_durability(root, Durability::Rebuildable)
    }
    fn with_durability(root: PathBuf, durability: Durability) -> Self {
        Self {
            root,
            durability,
            packed: Arc::default(),
            statistics: Arc::default(),
            regions: Arc::default(),
            collection_cap: Arc::new(AtomicU64::new(u64::MAX)),
            cache_cap: Arc::new(AtomicU64::new(u64::MAX)),
            disk_estimate: Arc::new(AtomicU64::new(u64::MAX)),
        }
    }
    /// Opt into counters on this physical owner and all of its existing clones.
    /// Normal providers do not allocate or update counters.
    pub fn enable_statistics(&self) -> Arc<Statistics> {
        self.statistics
            .get_or_init(|| Arc::new(Statistics::new()))
            .clone()
    }
    pub fn statistics(&self) -> Option<Arc<Statistics>> {
        self.statistics.get().cloned()
    }
    fn count(&self, counter: Counter, value: u64) {
        if let Some(statistics) = self.statistics.get() {
            statistics.add(counter, value);
        }
    }
    fn open_region(&self, path: &Path, kind: Counter) -> io::Result<Arc<Region>> {
        let mut cache = self.regions.lock().unwrap_or_else(|e| e.into_inner());
        // Index generations retain their own backing. Payload paths are
        // immutable during a scope; a later scope starts with fresh handles.
        let retain = cache.active && !matches!(kind, Counter::IndexOpens);
        if retain && let Some(region) = cache.entries.get(path) {
            self.count(Counter::RegionCacheHits, 1);
            return Ok(region.clone());
        }
        self.count(kind, 1);
        let mut region = Arc::new(Region::open(path)?);
        let bytes = region.bytes().len() as u64;
        if let Some(statistics) = self.statistics.get() {
            statistics.add(Counter::RegionLoads, 1);
            statistics.add(Counter::RegionBytesLoaded, bytes);
            region = Arc::new(Region::Retained {
                region,
                _owner: Arc::new(statistics::Backing::new(statistics.clone(), bytes)),
            });
        }
        if !matches!(kind, Counter::IndexOpens) {
            region = Arc::new(Region::Checked(Box::new(CheckedRegion {
                region,
                records: std::array::from_fn(|_| std::sync::OnceLock::new()),
                statistics: self.statistics.get().cloned(),
            })));
        }
        if retain {
            cache.insert(path.to_owned(), region.clone());
            if let Some(statistics) = self.statistics.get() {
                statistics.peak(Counter::RetainedRegionPeakBytes, cache.bytes as u64);
                statistics.peak(Counter::RetainedRegionPeakCount, cache.entries.len() as u64);
            }
        }
        Ok(region)
    }
    fn publish(&self, path: &Path, parts: &[&[u8]]) -> io::Result<()> {
        self.count(Counter::PublicationAttempts, 1);
        let result = publish(
            path,
            parts,
            self.durability,
            self.statistics.get().map(Arc::as_ref),
        );
        if result.is_ok() {
            self.record_growth(parts.iter().map(|part| part.len() as u64).sum());
        }
        if result.is_err() {
            self.count(Counter::SkippedPersistence, 1);
        }
        result
    }
    fn resident_equals(&self, path: &Path, parts: &[&[u8]]) -> bool {
        let cache = self.regions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(region) = cache.entries.get(path).filter(|_| cache.active) else {
            return false;
        };
        let mut remaining = region.bytes();
        for part in parts {
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        }
        remaining.is_empty()
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn output_path(&self, reference: Reference) -> PathBuf {
        if reference.len <= pack::SMALL as u64 {
            self.packed_path(0, reference.output.0)
                .unwrap_or_else(|| self.root.join("missing-output"))
        } else {
            self.private_output_path(reference.output.0)
                .unwrap_or_else(|| self.path(reference.output.0, "outputs"))
        }
    }
    /// The immutable backing segment for an association, for local corruption
    /// quarantine. A segment may hold several records; dropping it evicts them.
    pub fn association_path(&self, address: Address) -> Option<PathBuf> {
        self.packed_path(1, address.0)
    }
    /// Cache recency is eviction policy, never source freshness or proof.
    pub fn touch(&self, _address: Address, _reference: Reference) {
        // Recency is per immutable segment publication. Per-record filesystem
        // touches would recreate the small-record syscall amplification.
    }
    fn path(&self, hash: ContentHash, kind: &str) -> PathBuf {
        let hex = hex(hash);
        self.root.join(kind).join(&hex[..2]).join(hex)
    }
    /// Publish a bounded immutable output. No action/verification claim is
    /// accepted here: imported content gets the same integrity checks.
    pub fn put(&self, domain: Domain, bytes: &[u8]) -> io::Result<Reference> {
        if bytes.len() > MAX_RECORD {
            return Err(invalid());
        }
        let reference = Reference {
            output: domain.output(bytes),
            domain,
            len: bytes.len() as u64,
        };
        let mut header = Vec::with_capacity(HEADER);
        header.extend(MAGIC);
        header.extend(domain.kind.to_le_bytes());
        header.extend(domain.schema.to_le_bytes());
        header.extend(reference.len.to_le_bytes());
        header.extend(reference.output.0.0);
        header.extend([0; 8]);
        if bytes.len() <= pack::SMALL {
            header.extend(bytes);
            self.packed_put(0, reference.output.0, header.into())?;
        } else {
            let path = self.path(reference.output.0, "outputs");
            if self.resident_equals(&path, &[&header, bytes]) {
                self.count(Counter::IdenticalUpdates, 1);
            } else {
                self.put_large(reference, &[&header, bytes])?;
            }
        }
        Ok(reference)
    }
    /// Map a demanded record and validate its complete content at this boundary.
    /// Mapping alone reads no payload pages; integrity checking necessarily does.
    pub fn get(&self, reference: Reference) -> Option<Bytes> {
        checked_container(self.raw_container(reference)?, reference)
    }
    fn raw_container(&self, reference: Reference) -> Option<Bytes> {
        if reference.len <= pack::SMALL as u64 {
            self.packed_get(0, reference.output.0)
        } else {
            let region = self
                .open_region(&self.output_path(reference), Counter::StandaloneOpens)
                .ok()?;
            let len = region.bytes().len();
            Some(Bytes {
                region,
                range: 0..len,
            })
        }
    }
    /// A checked ordinary container, including its header, for embedding the
    /// same representation in a toolchain build. No second codec is involved.
    pub fn container(&self, reference: Reference) -> Option<Bytes> {
        let bytes = self.raw_container(reference)?;
        checked_container(bytes.clone(), reference)?;
        Some(bytes)
    }

    /// Read a previous association before demanding its output bytes. The query
    /// validates its recorded reads/action before calling get(reference).
    pub fn lookup(&self, address: Address) -> Option<Association> {
        self.lookup_open(address).map(|opened| opened.association)
    }
    /// Carry the same opened output through action checking to consumption,
    /// avoiding an output reopen between association lookup and validation.
    pub fn lookup_open(&self, address: Address) -> Option<OpenedAssociation> {
        let bytes = self.packed_get(1, address.0)?;
        if bytes.len() != ROOT_LEN
            || bytes[..8] != *ROOT
            || bytes[8..40] != address.0.0
            || ContentHash::of_parts("hirsel.artifact.association.v1", [&bytes[..120]]).0
                != bytes[120..]
        {
            return None;
        }
        let association = Association {
            action: Action(ContentHash(bytes[40..72].try_into().ok()?)),
            reference: Reference {
                output: Output(ContentHash(bytes[72..104].try_into().ok()?)),
                domain: Domain {
                    kind: word(&bytes, 104)?,
                    schema: word(&bytes, 108)?,
                },
                len: number(&bytes, 112)?,
            },
        };
        self.raw_container(association.reference)
            .filter(|bytes| {
                bytes.len() as u64
                    == association
                        .reference
                        .len
                        .checked_add(HEADER as u64)
                        .unwrap_or(u64::MAX)
            })
            .map(|container| OpenedAssociation {
                association,
                container,
            })
    }
    /// Atomically expose an association after every referenced output exists.
    /// Callers own verification authority and must not publish imported claims.
    pub fn associate(
        &self,
        address: Address,
        action: Action,
        reference: Reference,
    ) -> io::Result<()> {
        if self.raw_container(reference).is_none_or(|bytes| {
            bytes.len() as u64 != reference.len.checked_add(HEADER as u64).unwrap_or(u64::MAX)
        }) {
            return Err(invalid());
        }
        let mut bytes = Vec::with_capacity(ROOT_LEN);
        bytes.extend(ROOT);
        bytes.extend(address.0.0);
        bytes.extend(action.0.0);
        bytes.extend(reference.output.0.0);
        bytes.extend(reference.domain.kind.to_le_bytes());
        bytes.extend(reference.domain.schema.to_le_bytes());
        bytes.extend(reference.len.to_le_bytes());
        bytes.extend(ContentHash::of_parts("hirsel.artifact.association.v1", [&bytes[..]]).0);
        self.packed_put(1, address.0, bytes.into())
    }
    /// Bounded collection scratch. A cache may evict referenced content: a later
    /// lookup becomes a miss, while existing mappings keep their inode alive.
    /// Schedule upkeep without flushing a producer or stealing live staging.
    /// The outer finished/aborted scope performs one attempt after releasing
    /// its publication state. No scope means collection can run immediately.
    pub fn schedule_collection(&self, cap: u64) {
        self.collection_cap.fetch_min(cap, Ordering::Relaxed);
        let idle = self.packed.lock().unwrap_or_else(|e| e.into_inner()).depth == 0;
        if idle {
            self.run_scheduled_collection();
        }
    }
    /// Configure this physical cache owner's retained-disk budget. Payload,
    /// index and maintenance writes share its accounting; clones share policy.
    /// Actual collection waits for a safe owner boundary.
    pub fn set_cache_cap(&self, cap: u64) {
        self.cache_cap.store(cap, Ordering::Relaxed);
        self.collection_cap.fetch_min(cap, Ordering::Relaxed);
    }
    fn record_growth(&self, bytes: u64) {
        self.count(Counter::PublicationAdmissionBytes, bytes);
        let estimate = self
            .disk_estimate
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                Some(old.saturating_add(bytes))
            })
            .unwrap_or_else(|old| old)
            .saturating_add(bytes);
        let cap = self.cache_cap.load(Ordering::Relaxed);
        if cap != u64::MAX && estimate > cap {
            self.collection_cap.fetch_min(cap, Ordering::Relaxed);
        }
    }
    fn run_scheduled_collection(&self) {
        let cap = self.collection_cap.swap(u64::MAX, Ordering::Relaxed);
        if cap != u64::MAX && self.try_collect(cap).is_none() {
            self.collection_cap.fetch_min(cap, Ordering::Relaxed);
        }
    }
    /// Explicit collection never commits pending work. Missing cache files
    /// remain permissible misses; held byte views retain their backing.
    pub fn collect(&self, cap: u64) -> GcReport {
        self.try_collect(cap).unwrap_or_default()
    }
    fn try_collect(&self, cap: u64) -> Option<GcReport> {
        use std::collections::BinaryHeap;
        use std::time::SystemTime;
        const BATCH: usize = 1024;
        // Collection cannot publish another scope's pending writes. A live
        // lease also protects payloads already installed but not yet indexed.
        let Ok(_publication) = pack::Lock::try_root(&self.root, true) else {
            return None;
        };
        if self.staging_is_live() {
            return None;
        }
        let mut report = GcReport::default();
        loop {
            self.count(Counter::GcScans, 1);
            let mut oldest = BinaryHeap::new();
            let mut entries = 0;
            let mut total = 0u64;
            for kind in ["packs", "outputs", "actions"] {
                for shard in fs::read_dir(self.root.join(kind))
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    if !shard.file_type().is_ok_and(|kind| kind.is_dir()) {
                        continue;
                    }
                    for file in fs::read_dir(shard.path()).into_iter().flatten().flatten() {
                        if !file.file_type().is_ok_and(|kind| kind.is_file()) {
                            continue;
                        }
                        let Ok(meta) = file.metadata() else { continue };
                        if !meta.is_file() || file.file_name().to_string_lossy().starts_with('.') {
                            continue;
                        }
                        entries += 1;
                        self.count(Counter::GcEntries, 1);
                        total = total.saturating_add(meta.len());
                        oldest.push((
                            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                            meta.len(),
                            file.path(),
                        ));
                        if oldest.len() > BATCH {
                            oldest.pop();
                        }
                    }
                }
            }
            let protected: Vec<_> = index::current_layer_ids_locked(&self.root)
                .ok()?
                .into_iter()
                .map(|id| format!("L-{}", hex(id)))
                .collect();
            for file in fs::read_dir(self.root.join("index"))
                .into_iter()
                .flatten()
                .flatten()
            {
                let name = file.file_name().to_string_lossy().into_owned();
                if name != "MANIFEST" && !name.starts_with("L-") {
                    continue;
                }
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                entries += 1;
                self.count(Counter::GcEntries, 1);
                total = total.saturating_add(meta.len());
                // Current layer names remain protected until an explicit root
                // eviction. Next bounded scan can then reclaim those orphans.
                if protected.contains(&name) {
                    continue;
                }
                oldest.push((
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    meta.len(),
                    file.path(),
                ));
                if oldest.len() > BATCH {
                    oldest.pop();
                }
            }
            if report.entries == 0 {
                report.entries = entries;
                report.bytes = total;
            }
            if total <= cap {
                self.disk_estimate.store(total, Ordering::Relaxed);
                return Some(report);
            }
            let before = total;
            for (_, len, path) in oldest.into_sorted_vec() {
                if total <= cap {
                    break;
                }
                if fs::remove_file(path).is_ok() {
                    self.count(Counter::GcUnlinks, 1);
                    total = total.saturating_sub(len);
                    report.removed += 1;
                    report.removed_bytes += len;
                }
            }
            if total == before {
                self.disk_estimate.store(total, Ordering::Relaxed);
                return Some(report);
            }
        }
    }
}

/// Publish through a fresh inode. Concurrent readers keep the prior immutable
/// inode, and a crash before rename cannot publish a partial root or chunk.
fn publish(
    path: &Path,
    parts: &[&[u8]],
    durability: Durability,
    statistics: Option<&Statistics>,
) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(invalid)?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        for part in parts {
            file.write_all(part)?;
        }
        if durability == Durability::Strict {
            if let Some(statistics) = statistics {
                statistics.add(Counter::FileSyncs, 1);
            }
            file.sync_all()?;
        }
        fs::rename(&temporary, path)?;
        if let Some(statistics) = statistics {
            statistics.add(Counter::Publications, 1);
        }
        if durability == Durability::Strict {
            if let Some(statistics) = statistics {
                statistics.add(Counter::DirectorySyncs, 1);
            }
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests;

fn checked_content(region: Arc<Region>, reference: Reference) -> Option<Bytes> {
    let len = region.bytes().len();
    checked_container(
        Bytes {
            region,
            range: 0..len,
        },
        reference,
    )
}
fn checked_container(bytes: Bytes, reference: Reference) -> Option<Bytes> {
    let checks = bytes.region.checks();
    let slot = checks
        .map(|checks| &checks.records[reference.output.0.0[0] as usize % checks.records.len()]);
    if let Some(checked) = slot.and_then(std::sync::OnceLock::get)
        && checked.range == bytes.range
        && checked.reference == reference
    {
        if let Some(statistics) = checks.and_then(|checks| checks.statistics.as_ref()) {
            statistics.add(Counter::ContentReuses, 1);
        }
        return bytes.range(HEADER as u64, reference.len, 1);
    }
    if let Some(statistics) = checks.and_then(|checks| checks.statistics.as_ref()) {
        statistics.add(Counter::ContentChecks, 1);
    }
    if reference.len > MAX_RECORD as u64
        || bytes.get(..8)? != MAGIC
        || word(&bytes, 8)? != reference.domain.kind
        || word(&bytes, 12)? != reference.domain.schema
        || number(&bytes, 16)? != reference.len
        || bytes.get(24..56)? != reference.output.0.0
        || bytes.get(56..64)? != [0; 8]
        || usize::try_from(reference.len).ok()?.checked_add(HEADER)? != bytes.len()
        || reference.domain.output(&bytes[HEADER..]) != reference.output
    {
        return None;
    }
    if let Some(slot) = slot {
        let _ = slot.set(CheckedContainer {
            range: bytes.range.clone(),
            reference,
        });
    }
    bytes.range(HEADER as u64, reference.len, 1)
}
