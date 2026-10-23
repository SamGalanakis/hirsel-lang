//! Immutable aligned payload segments and explicitly owned request publication.
//! Git 7e34d8174dcda3435d1404c9a30bef46afe6c13e pack format informs
//! record separation; LMDB ba47717cc87753f86f24135c17bfaaa82d5daf53
//! mdb.c:4652,4881 motivates data before committed roots. Index routing lives
//! in index.rs; demanded content and association checks remain authoritative.
use super::*;
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

mod epoch;

use crate::index::{self, Key, KeyKind, Location, PinnedGeneration};
pub use epoch::Request;

pub(super) const SMALL: usize = 4096;
const LIMIT: usize = 1 << 20;
const RECORDS: usize = 1024;
const SEGMENT: &[u8; 8] = b"HIRPAK02";
#[derive(Default)]
pub(super) struct State {
    pub(super) depth: usize,
    bytes: usize,
    pending: BTreeMap<(u8, ContentHash), Bytes>,
    index: Option<Index>,
    pinned: bool,
    maintenance_due: bool,
    requests: usize,
    aborted: bool,
    epoch: Option<epoch::Epoch>,
    #[cfg(test)]
    epoch_limits: Option<(usize, usize)>,
    #[cfg(test)]
    crash_after_segment: bool,
}
#[derive(Clone)]
struct Index {
    generation: PinnedGeneration,
    stamp: Option<Stamp>,
    _backing: Option<Arc<statistics::Backing>>,
    diagnostics: Option<Arc<IndexObservation>>,
}
// Cloned Index handles share one observation owner. Snapshot only at read
// scope boundaries or final pin release, never on the lookup path.
struct IndexObservation {
    generation: PinnedGeneration,
    statistics: Arc<statistics::Statistics>,
    seen: Mutex<index::IndexDiagnostics>,
}
impl IndexObservation {
    fn record(&self) {
        let now = self.generation.diagnostics();
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        for (counter, delta) in [
            (
                Counter::InvalidIndexRoots,
                u64::from(now.invalid_root && !seen.invalid_root),
            ),
            (
                Counter::BadIndexBlocks,
                now.bad_blocks.saturating_sub(seen.bad_blocks),
            ),
            (
                Counter::BadIndexBuckets,
                now.bad_buckets.saturating_sub(seen.bad_buckets),
            ),
        ] {
            self.statistics.add(counter, delta);
        }
        *seen = now;
    }
}
impl Drop for IndexObservation {
    fn drop(&mut self) {
        self.record();
    }
}
impl Index {
    fn record_diagnostics(&self) {
        if let Some(observation) = &self.diagnostics {
            observation.record();
        }
    }
}
type Stamp = (u64, std::time::SystemTime, u64);
fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let inode = 0;
    Some((meta.len(), meta.modified().ok()?, inode))
}
fn index_key(tag: u8, digest: ContentHash) -> Option<Key> {
    let kind = match tag {
        0 => KeyKind::Content,
        1 => KeyKind::Association,
        _ => return None,
    };
    Some(Key { kind, digest })
}
/// A bounded publication transaction. Drop commits its last batch; callers that
/// need an I/O result use `finish`. Failed publication supplies no durable hit.
pub struct Batch {
    store: Option<Store>,
}
impl Batch {
    pub fn finish(mut self) -> io::Result<()> {
        self.store
            .take()
            .expect("live batch")
            .end_batch(Counter::FinishCommits)
    }
}
impl Drop for Batch {
    fn drop(&mut self) {
        if let Some(store) = self.store.take() {
            let _ = store.end_batch(Counter::DropCommits);
        }
    }
}

impl Store {
    fn state(&self) -> MutexGuard<'_, State> {
        self.packed.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Publish small records together, bounded to 1024 records or 1 MiB of
    /// scratch. Nested scopes share the same transaction; readers of another
    /// store see only its committed index. No verifier authority is created.
    pub fn batch(&self) -> Batch {
        let mut state = self.state();
        self.begin_scope(&mut state);
        Batch {
            store: Some(self.clone()),
        }
    }
    fn begin_scope(&self, state: &mut State) {
        if state.depth == 0 {
            self.count(Counter::Epochs, 1);
            state.index = None;
            state.pinned = false;
            state.aborted = false;
            self.regions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .reset(true);
        }
        state.depth += 1;
    }
    fn reset_read_scope(&self, state: &mut State) {
        if let Some(index) = &state.index {
            index.record_diagnostics();
        }
        state.pinned = false;
        self.regions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset(false);
    }
    fn end_batch(&self, cause: Counter) -> io::Result<()> {
        let mut state = self.state();
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0 {
            let result = if state.aborted {
                Ok(())
            } else {
                self.commit(&mut state, cause)
            };
            self.reset_read_scope(&mut state);
            drop(state);
            self.run_scheduled_collection();
            result?;
        }
        Ok(())
    }
    fn load_index(&self, observe_stamp: bool) -> Option<Index> {
        self.count(Counter::IndexLoadAttempts, 1);
        let generation = index::load_pinned(&self.root).unwrap_or_default();
        self.count(Counter::IndexPins, 1);
        let diagnostics = self.statistics.get().map(|statistics| {
            Arc::new(IndexObservation {
                generation: generation.clone(),
                statistics: statistics.clone(),
                seen: Mutex::new(index::IndexDiagnostics::default()),
            })
        });
        let bytes = generation.retained_bytes()
            + diagnostics.as_ref().map_or(0, |_| {
                (size_of::<IndexObservation>() + 2 * size_of::<usize>()) as u64
            });
        let backing = self.statistics.get().map(|statistics| {
            statistics.add(Counter::IndexBackingBytesLoaded, bytes);
            Arc::new(statistics::Backing::new(statistics.clone(), bytes))
        });
        Some(Index {
            generation,
            stamp: if observe_stamp {
                self.index_stamp()
            } else {
                None
            },
            _backing: backing,
            diagnostics,
        })
    }
    fn index_stamp(&self) -> Option<Stamp> {
        self.count(Counter::IndexStamps, 1);
        stamp(&self.root.join("index/MANIFEST"))
    }
    fn refresh_index(&self, state: &mut State) {
        if state.depth != 0 {
            if !state.pinned {
                state.index = self.load_index(state.depth == 0);
                state.pinned = true;
            }
        } else if state
            .index
            .as_ref()
            .is_none_or(|index| index.stamp != self.index_stamp())
        {
            // Legacy individual operations refresh between requests. Within a
            // scope, even an absent or invalid index is one retained snapshot.
            state.index = self.load_index(state.depth == 0);
        }
    }
    pub(super) fn packed_path(&self, tag: u8, key: ContentHash) -> Option<PathBuf> {
        let mut state = self.state();
        if let Some(epoch) = &state.epoch
            && let Some(location) = epoch.locations.get(&(tag, key))
        {
            return Some(epoch.segment_path(location.segment));
        }
        self.refresh_index(&mut state);
        index::find(&state.index.as_ref()?.generation, index_key(tag, key)?)
            .map(|l| self.path(l.segment, "packs"))
    }
    pub(super) fn packed_get(&self, tag: u8, key: ContentHash) -> Option<Bytes> {
        let mut state = self.state();
        if let Some(bytes) = state.pending.get(&(tag, key)) {
            return Some(bytes.clone());
        }
        let (location, path) = if let Some(epoch) = &state.epoch
            && let Some(location) = epoch.locations.get(&(tag, key))
        {
            (*location, epoch.segment_path(location.segment))
        } else {
            self.refresh_index(&mut state);
            let location = index::find(&state.index.as_ref()?.generation, index_key(tag, key)?)?;
            (location, self.path(location.segment, "packs"))
        };
        drop(state);
        let region = self.open_region(&path, Counter::SegmentOpens).ok()?;
        let bytes = region.bytes();
        if bytes.len() > LIMIT
            || bytes.get(..8)? != SEGMENT
            || number(bytes, 8)? != bytes.len() as u64
            || bytes.get(24..32)? != [0; 8]
            || bytes.get(32..64)?
                != ContentHash::of_parts("hirsel.artifact.pack-header.v2", [&bytes[..32]]).0
        {
            return None;
        }
        let len = bytes.len();
        Bytes {
            region,
            range: 0..len,
        }
        .range(u64::from(location.offset), u64::from(location.len), 8)
    }
    pub(super) fn packed_put(&self, tag: u8, key: ContentHash, bytes: Bytes) -> io::Result<()> {
        let mut state = self.state();
        if state.depth == 0 {
            state.aborted = false;
        }
        if state
            .pending
            .get(&(tag, key))
            .is_some_and(|old| *old == bytes)
        {
            self.count(Counter::IdenticalUpdates, 1);
            return Ok(());
        }
        if state.aborted {
            return Err(io::Error::other("aborted cache request"));
        }
        let len = bytes.len();
        if state.requests != 0 {
            self.admit_private(&mut state, len + HEADER + 7, 128)?;
        }
        let cause = if state.bytes.checked_add(len + 7).ok_or_else(invalid)? > LIMIT - HEADER {
            Some((Counter::ByteLimitCommits, Counter::ByteLimitSpills))
        } else if state.pending.len() >= RECORDS {
            Some((Counter::RecordLimitCommits, Counter::RecordLimitSpills))
        } else {
            None
        };
        if let Some((commit, spill)) = cause {
            if state.requests != 0 {
                self.seal_private(&mut state)?;
                self.count(spill, 1);
            } else {
                self.commit(&mut state, commit)?;
            }
        }
        if let Some(previous) = state.pending.insert((tag, key), bytes) {
            state.bytes = state.bytes.saturating_sub(previous.len() + 7);
        }
        state.bytes += len + 7;
        self.private_peaks(&state);
        if let Some(statistics) = self.statistics.get() {
            statistics.peak(Counter::PendingPeakBytes, state.bytes as u64);
            statistics.peak(Counter::PendingPeakRecords, state.pending.len() as u64);
        }
        if state.depth == 0 {
            self.commit(&mut state, Counter::FinishCommits)?;
        }
        Ok(())
    }
    fn commit(&self, state: &mut State, cause: Counter) -> io::Result<()> {
        self.publish_epoch(state, cause)
    }
    fn publish_index(
        &self,
        state: &mut State,
        delta: index::PreparedDelta,
        epoch: &EpochLease,
    ) -> io::Result<()> {
        self.count(Counter::IndexPublicationAttempts, 1);
        match index::publish_delta(&self.root, delta, epoch)? {
            index::PublishOutcome::Published {
                maintenance_due, ..
            } => {
                self.count(Counter::IndexPublications, 1);
                // One immutable layer installation and one atomic manifest.
                self.count(Counter::Publications, 2);
                state.maintenance_due |= maintenance_due;
                self.sync_strict_index()?;
                state.index = self.load_index(state.depth == 0);
                state.pinned = state.depth != 0;
            }
            index::PublishOutcome::Skipped(_) => {
                self.count(Counter::SkippedPersistence, 1);
                self.count(Counter::IndexPublicationSkips, 1);
            }
            index::PublishOutcome::Empty => {}
        }
        Ok(())
    }
    fn sync_strict_index(&self) -> io::Result<()> {
        if self.durability != Durability::Strict {
            return Ok(());
        }
        let _lock = Lock::try_root(&self.root, false)?;
        let directory = self.root.join("index");
        for id in index::current_layer_ids_locked(&self.root)? {
            self.count(Counter::FileSyncs, 1);
            File::open(directory.join(format!("L-{}", hex(id))))?.sync_all()?;
        }
        // Referenced immutable files and their names precede the root barrier.
        self.count(Counter::DirectorySyncs, 1);
        File::open(&directory)?.sync_all()?;
        self.count(Counter::FileSyncs, 1);
        File::open(directory.join("MANIFEST"))?.sync_all()?;
        self.count(Counter::DirectorySyncs, 1);
        File::open(&directory)?.sync_all()
    }
    fn maintain_index(&self, state: &mut State) -> io::Result<()> {
        if !state.maintenance_due {
            return Ok(());
        }
        self.count(Counter::IndexMaintenanceAttempts, 1);
        let outcome = index::compact(
            &self.root,
            index::MaintenanceBudget {
                max_input_bytes: index::MAX_BYTES as u64,
                max_rows: index::MAX_ROWS as u32,
                max_scratch_bytes: index::MAX_SCRATCH as u64,
            },
        )?;
        match outcome {
            index::CompactOutcome::Published { .. } => {
                self.count(Counter::IndexMaintenancePublications, 1);
                self.count(Counter::Publications, 2);
                self.sync_strict_index()?;
                state.maintenance_due = false;
                state.index = self.load_index(state.depth == 0);
                if let Some(pin) = &state.index {
                    // Full backing/status allocation is a conservative upper
                    // bound on the compacted file and bounded manifest bytes.
                    self.record_growth(pin.generation.retained_bytes() + 420);
                }
            }
            index::CompactOutcome::NotNeeded => state.maintenance_due = false,
            index::CompactOutcome::Deferred | index::CompactOutcome::Conflict => {}
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn crash_before_next_index(&self) {
        self.state().crash_after_segment = true;
    }
    #[cfg(test)]
    pub(crate) fn publish_test_routes(&self, entries: &[index::Entry]) -> io::Result<()> {
        let lease = EpochLease::new(&self.root)?;
        let delta = index::build(&self.root, entries)?;
        match index::publish_delta(&self.root, delta, &lease)? {
            index::PublishOutcome::Published { .. } => Ok(()),
            outcome => Err(io::Error::other(format!(
                "test route publication: {outcome:?}"
            ))),
        }
    }
}

fn encode_segment(
    pending: &BTreeMap<(u8, ContentHash), Bytes>,
) -> io::Result<(ContentHash, Vec<u8>, BTreeMap<(u8, ContentHash), Location>)> {
    let mut segment = vec![0u8; HEADER];
    let mut locations = Vec::with_capacity(pending.len());
    for (&key, bytes) in pending {
        segment.resize(segment.len().next_multiple_of(8), 0);
        let offset = segment.len() as u64;
        segment.extend(bytes.iter());
        locations.push((key, offset, bytes.len() as u64));
    }
    if segment.len() > LIMIT {
        return Err(invalid());
    }
    segment[..8].copy_from_slice(SEGMENT);
    let segment_len = segment.len() as u64;
    segment[8..16].copy_from_slice(&segment_len.to_le_bytes());
    segment[16..24].copy_from_slice(&(locations.len() as u64).to_le_bytes());
    let check = ContentHash::of_parts("hirsel.artifact.pack-header.v2", [&segment[..32]]);
    segment[32..64].copy_from_slice(&check.0);
    let digest = ContentHash::of_parts("hirsel.artifact.pack.v2", [&segment[..]]);
    let new: BTreeMap<_, _> = locations
        .into_iter()
        .map(|(key, offset, len)| {
            (
                key,
                Location {
                    segment: digest,
                    offset: offset as u32,
                    len: len as u32,
                },
            )
        })
        .collect();
    Ok((digest, segment, new))
}

/// This lock is independent of .pack-lock and remains held through final
/// payload installation and index publication. It creates no V authority.
pub(crate) struct EpochLease {
    root: PathBuf,
    directory: PathBuf,
    _owner: Lock,
}
#[cfg(test)]
pub(crate) fn index_benchmark_epoch(root: &Path) -> io::Result<EpochLease> {
    EpochLease::new(root)
}

impl EpochLease {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    fn new(root: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // GC cannot discover the directory between creation and owner locking.
        let _publication = Lock::try_root(root, false)?;
        let staging = root.join("staging");
        fs::create_dir_all(&staging)?;
        let directory = loop {
            let path = staging.join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        let owner = (|| {
            let file = File::options()
                .read(true)
                .write(true)
                .create_new(true)
                .open(directory.join("owner.lock"))?;
            Lock::try_file(file, true)
        })();
        match owner {
            Ok(owner) => Ok(Self {
                root: root.to_owned(),
                directory,
                _owner: owner,
            }),
            Err(e) => {
                let _ = fs::remove_dir_all(directory);
                Err(e)
            }
        }
    }
}
impl Drop for EpochLease {
    fn drop(&mut self) {
        // Private paths only. Installed immutable payloads may remain orphaned;
        // a missing committed index is never reconstructed from those files.
        let _ = fs::remove_dir_all(&self.directory);
    }
}

pub(super) struct Lock(File);
impl Lock {
    pub(super) fn try_root(root: &Path, exclusive: bool) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".pack-lock"))?;
        Self::try_file(file, exclusive)
    }
    fn try_file(file: File, exclusive: bool) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mode = if exclusive {
                libc::LOCK_EX
            } else {
                libc::LOCK_SH
            };
            // SAFETY: an owned live descriptor; nonblocking advisory locking.
            if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(file))
        }
        #[cfg(not(unix))]
        {
            let _ = (file, exclusive);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "cache ownership locks unavailable",
            ))
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: exactly the live fd locked in new.
            unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_index_misses_retain_the_checked_snapshot_and_observe_later_commits() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_owned());
        let domain = Domain { kind: 1, schema: 1 };
        let known = store.put(domain, b"known").unwrap();
        assert!(store.get(known).is_some());
        let before = store.enable_statistics().snapshot();
        let absent = domain.reference(b"later").unwrap();
        for _ in 0..8 {
            assert!(store.get(absent).is_none());
            assert_eq!(
                store.statistics().unwrap().snapshot().counters["IndexLoadAttempts"],
                before.counters["IndexLoadAttempts"]
            );
        }
        let writer = Store::new(dir.path().to_owned());
        assert_eq!(writer.put(domain, b"later").unwrap(), absent);
        assert_eq!(&*store.get(absent).unwrap(), b"later");
    }
}
