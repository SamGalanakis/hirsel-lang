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
use std::io::{self, Read, Write};
use std::ops::{Deref, Range};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The largest independently mapped record. Oversized queries must split their
/// output into referenced records; packing never builds another whole program.
pub const MAX_RECORD: usize = 16 << 20;
const MAGIC: &[u8; 8] = b"HIRART01";
const HEADER: usize = 64;
const ROOT: &[u8; 8] = b"HIRACT01";
const ROOT_LEN: usize = 152;

/// Stable query address, used before its previous dependencies are evaluated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Address(pub ContentHash);
/// Complete computation identity, including all observed dependency outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action(pub ContentHash);
/// Canonical output identity. Equal recomputation retains this identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Output(pub ContentHash);

/// A content domain. Every referenced record checks both kind and schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Domain {
    pub kind: u32,
    pub schema: u32,
}

/// A reference cannot supply proof authority. Producers and verifier-owned
/// associations use separate roots; imported bytes create no action association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reference {
    pub output: Output,
    pub domain: Domain,
    pub len: u64,
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
        Self::from(Arc::<[u8]>::from(bytes))
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
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn output_path(&self, reference: Reference) -> PathBuf {
        self.path(reference.output.0, "outputs")
    }
    /// Cache recency is eviction policy, never source freshness or proof.
    pub fn touch(&self, address: Address, reference: Reference) {
        for path in [self.path(address.0, "actions"), self.output_path(reference)] {
            if let Ok(file) = File::options().append(true).open(path) {
                let _ = file.set_modified(std::time::SystemTime::now());
            }
        }
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
        publish(&self.path(reference.output.0, "outputs"), &[&header, bytes])?;
        Ok(reference)
    }
    /// Map a demanded record and validate its complete content at this boundary.
    /// Mapping alone reads no payload pages; integrity checking necessarily does.
    pub fn get(&self, reference: Reference) -> Option<Bytes> {
        let region = Arc::new(Region::open(&self.path(reference.output.0, "outputs")).ok()?);
        let bytes = region.bytes();
        if bytes.get(..8)? != MAGIC
            || word(bytes, 8)? != reference.domain.kind
            || word(bytes, 12)? != reference.domain.schema
            || number(bytes, 16)? != reference.len
            || bytes.get(24..56)? != reference.output.0.0
            || bytes.get(56..64)? != [0; 8]
            || usize::try_from(reference.len).ok()?.checked_add(HEADER)? != bytes.len()
            || reference.domain.output(&bytes[HEADER..]) != reference.output
        {
            return None;
        }
        let end = bytes.len();
        Some(Bytes {
            region,
            range: HEADER..end,
        })
    }
    /// Read a previous association before demanding its output bytes. The query
    /// validates its recorded reads/action before calling get(reference).
    pub fn lookup(&self, address: Address) -> Option<Association> {
        let mut bytes = Vec::with_capacity(ROOT_LEN + 1);
        File::open(self.path(address.0, "actions"))
            .ok()?
            .take((ROOT_LEN + 1) as u64)
            .read_to_end(&mut bytes)
            .ok()?;
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
        (fs::metadata(self.output_path(association.reference))
            .ok()?
            .len()
            == association.reference.len.checked_add(HEADER as u64)?)
        .then_some(association)
    }
    /// Atomically expose an association after every referenced output exists.
    /// Callers own verification authority and must not publish imported claims.
    pub fn associate(
        &self,
        address: Address,
        action: Action,
        reference: Reference,
    ) -> io::Result<()> {
        if fs::metadata(self.output_path(reference))?.len()
            != reference
                .len
                .checked_add(HEADER as u64)
                .ok_or_else(invalid)?
        {
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
        publish(&self.path(address.0, "actions"), &[&bytes])
    }
    /// Bounded collection scratch. A cache may evict referenced content: a later
    /// lookup becomes a miss, while existing mappings keep their inode alive.
    pub fn collect(&self, cap: u64) -> GcReport {
        use std::collections::BinaryHeap;
        use std::time::SystemTime;
        const BATCH: usize = 1024;
        let mut report = GcReport::default();
        loop {
            let mut oldest = BinaryHeap::new();
            let mut entries = 0;
            let mut total = 0u64;
            for kind in ["actions", "outputs"] {
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
            if report.entries == 0 {
                report.entries = entries;
                report.bytes = total;
            }
            if total <= cap {
                return report;
            }
            let before = total;
            for (_, len, path) in oldest.into_sorted_vec() {
                if total <= cap {
                    break;
                }
                if fs::remove_file(path).is_ok() {
                    total = total.saturating_sub(len);
                    report.removed += 1;
                    report.removed_bytes += len;
                }
            }
            if total == before {
                return report;
            }
        }
    }
}

/// Publish through a fresh inode. Concurrent readers keep the prior immutable
/// inode, and a crash before rename cannot publish a partial root or chunk.
fn publish(path: &Path, parts: &[&[u8]]) -> io::Result<()> {
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
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests;
