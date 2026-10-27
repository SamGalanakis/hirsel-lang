//! Private compilation epochs follow rustc's owned session directory and atomic
//! finalize structure (persist/fs.rs); the owner lock also protects installed
//! payloads until the shared index publication completes. Drop never publishes.
use super::*;

const PRIVATE_BYTES: usize = 64 << 20;
const METADATA_BYTES: usize = 16 << 20;

pub struct Request {
    store: Option<Store>,
}
impl Request {
    /// The caller must have joined its producers before finishing the outer
    /// request. Nested requests share the owner and finish with that outer one.
    pub fn finish(mut self) -> io::Result<()> {
        self.store.take().expect("live request").end_request(true)
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        if let Some(store) = self.store.take() {
            let _ = store.end_request(false);
        }
    }
}

pub(super) struct Epoch {
    lease: EpochLease,
    pub(super) locations: BTreeMap<(u8, ContentHash), Location>,
    segments: BTreeMap<ContentHash, usize>,
    large: BTreeMap<ContentHash, usize>,
    bytes: usize,
}
impl Epoch {
    pub(super) fn segment_path(&self, digest: ContentHash) -> PathBuf {
        self.lease.directory.join(format!("S-{}", hex(digest)))
    }
    fn large_path(&self, digest: ContentHash) -> PathBuf {
        self.lease.directory.join(format!("O-{}", hex(digest)))
    }
    fn metadata(&self) -> usize {
        // Conservative charge for BTree nodes, keys, locators and per-file
        // metadata; payload backing and pending bytes are charged separately.
        self.locations.len() * 128 + (self.segments.len() + self.large.len()) * 256
    }
}

impl Store {
    /// Own a compilation request before encoding or V starts. Capacity seals
    /// private segments; only finish (or bounded admission pressure) publishes.
    /// Error/Drop aborts the unpublished tail. Legacy Batch remains available.
    pub fn request(&self) -> Request {
        let mut state = self.state();
        self.begin_scope(&mut state);
        state.requests += 1;
        Request {
            store: Some(self.clone()),
        }
    }
    fn abort_private(&self, state: &mut State) {
        self.count(Counter::Aborts, 1);
        state.pending.clear();
        state.bytes = 0;
        state.epoch = None;
        state.aborted = true;
    }
    fn end_request(&self, success: bool) -> io::Result<()> {
        let mut state = self.state();
        state.requests = state.requests.saturating_sub(1);
        state.depth = state.depth.saturating_sub(1);
        if !success {
            self.abort_private(&mut state);
        }
        if state.requests != 0 {
            return Ok(());
        }
        let result = if state.aborted {
            if success {
                Err(io::Error::other("aborted cache request"))
            } else {
                Ok(())
            }
        } else if state.depth != 0 {
            self.abort_private(&mut state);
            Err(io::Error::other("cache request producers have not joined"))
        } else {
            self.publish_epoch(&mut state, Counter::FinishCommits)
        };
        if result.is_err() && !state.aborted {
            self.abort_private(&mut state);
        }
        if state.depth == 0 {
            self.reset_read_scope(&mut state);
            drop(state);
            self.run_scheduled_collection();
        }
        result
    }
    fn ensure_epoch(&self, state: &mut State) -> io::Result<()> {
        if state.epoch.is_none() {
            state.epoch = Some(Epoch {
                lease: EpochLease::new(&self.root, self.durability)?,
                locations: BTreeMap::new(),
                segments: BTreeMap::new(),
                large: BTreeMap::new(),
                bytes: 0,
            });
        }
        Ok(())
    }
    fn private_limits(&self, state: &State) -> (usize, usize) {
        #[cfg(test)]
        if let Some(limits) = state.epoch_limits {
            return limits;
        }
        let _ = state;
        (PRIVATE_BYTES, METADATA_BYTES)
    }
    pub(super) fn admit_private(
        &self,
        state: &mut State,
        bytes: usize,
        metadata: usize,
    ) -> io::Result<()> {
        if state.aborted {
            return Err(io::Error::other("aborted cache request"));
        }
        let (byte_limit, meta_limit) = self.private_limits(state);
        if bytes > byte_limit || metadata > meta_limit {
            return Err(invalid());
        }
        self.ensure_epoch(state)?;
        let epoch = state.epoch.as_ref().unwrap();
        if epoch
            .bytes
            .saturating_add(state.bytes)
            .saturating_add(bytes)
            > byte_limit
            || epoch
                .metadata()
                .saturating_add(state.pending.len() * 128 + 256)
                .saturating_add(metadata)
                > meta_limit
        {
            self.publish_epoch(state, Counter::AdmissionCommits)?;
            self.ensure_epoch(state)?;
        }
        Ok(())
    }
    pub(super) fn private_peaks(&self, state: &State) {
        if let Some(statistics) = self.statistics.get()
            && let Some(epoch) = &state.epoch
        {
            statistics.peak(
                Counter::PrivatePeakBytes,
                (epoch.bytes + state.bytes) as u64,
            );
            statistics.peak(
                Counter::EpochMetadataPeakBytes,
                (epoch.metadata()
                    + state.pending.len() * 128
                    + usize::from(!state.pending.is_empty()) * 256) as u64,
            );
        }
    }
    pub(super) fn seal_private(&self, state: &mut State) -> io::Result<()> {
        if state.pending.is_empty() {
            return Ok(());
        }
        self.ensure_epoch(state)?;
        let (digest, segment, locations) = encode_segment(&state.pending)?;
        let epoch = state.epoch.as_mut().unwrap();
        if !epoch.segments.contains_key(&digest) {
            self.publish(&epoch.segment_path(digest), &[&segment])?;
            epoch.bytes += segment.len();
            epoch.segments.insert(digest, segment.len());
        }
        epoch.locations.extend(locations);
        state.pending.clear();
        state.bytes = 0;
        self.count(Counter::PrivateSpills, 1);
        self.count(Counter::SealedSegments, 1);
        self.private_peaks(state);
        Ok(())
    }
    pub(crate) fn private_output_path(&self, output: ContentHash) -> Option<PathBuf> {
        let state = self.state();
        let epoch = state.epoch.as_ref()?;
        epoch
            .large
            .contains_key(&output)
            .then(|| epoch.large_path(output))
    }
    pub(crate) fn put_large(&self, reference: Reference, parts: &[&[u8]]) -> io::Result<()> {
        let mut state = self.state();
        if state.depth == 0 {
            state.aborted = false;
        }
        if state.aborted {
            return Err(io::Error::other("aborted cache request"));
        }
        if state.depth == 0 {
            drop(state);
            let lease = EpochLease::new(&self.root, self.durability)?;
            let result = self.publish(&self.path(reference.output.0, "outputs"), parts);
            drop(lease);
            self.run_scheduled_collection();
            return result;
        }
        let len = reference.len as usize + HEADER;
        self.admit_private(&mut state, len, 256)?;
        let epoch = state.epoch.as_mut().unwrap();
        if epoch.large.contains_key(&reference.output.0) {
            // This exact content identity was issued by this owner from its
            // complete canonical bytes; no pathname probe is added to put.
            self.count(Counter::IdenticalUpdates, 1);
            return Ok(());
        }
        self.publish(&epoch.large_path(reference.output.0), parts)?;
        epoch.bytes += len;
        epoch.large.insert(reference.output.0, len);
        self.private_peaks(&state);
        Ok(())
    }
    pub(super) fn publish_epoch(&self, state: &mut State, cause: Counter) -> io::Result<()> {
        self.seal_private(state)?;
        let Some(mut epoch) = state.epoch.take() else {
            return Ok(());
        };
        self.count(Counter::Commits, 1);
        self.count(cause, 1);
        let entries: Vec<_> = std::mem::take(&mut epoch.locations)
            .into_iter()
            .map(|((tag, digest), location)| {
                Ok(index::Entry {
                    key: index_key(tag, digest).ok_or_else(invalid)?,
                    location,
                })
            })
            .collect::<io::Result<_>>()?;
        // Index encoding, checksums, dictionary and bulk writes precede the
        // install/publication lock. This never scans a committed layer.
        let delta = index::build(&self.root, &entries, self.durability)?;
        self.record_growth(delta.admission_bytes());
        if !entries.is_empty() {
            self.count(Counter::IndexLayerBuilds, 1);
        }
        // Ownership protects installed-but-unpublished payloads across this
        // short install lock and the subsequent index publisher's own lock.
        {
            let _publication = Lock::for_store(epoch.lease.root(), true, self.durability)?;
            for &digest in epoch.segments.keys() {
                self.install_private(&epoch.segment_path(digest), &self.path(digest, "packs"))?;
            }
            for &digest in epoch.large.keys() {
                self.install_private(&epoch.large_path(digest), &self.path(digest, "outputs"))?;
            }
        }
        #[cfg(test)]
        if std::mem::take(&mut state.crash_after_segment) {
            return Err(io::Error::other("injected crash before index publication"));
        }
        self.publish_index(state, delta, &epoch.lease)?;
        // Reader sees its own committed prefix; the next pin opens once. The
        // private lease is released only after the publication outcome is known.
        state.pinned = state.depth != 0 && state.index.is_some();
        drop(epoch);
        // Explicit owner-boundary maintenance, separately counted and bounded;
        // publish_delta itself never compacts or retries.
        self.maintain_index(state)?;
        self.run_scheduled_collection();
        Ok(())
    }
    fn install_private(&self, private: &Path, final_path: &Path) -> io::Result<()> {
        let parent = final_path.parent().ok_or_else(invalid)?;
        fs::create_dir_all(parent)?;
        fs::rename(private, final_path)?;
        if self.durability == Durability::Strict {
            self.count(Counter::DirectorySyncs, 1);
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
    /// Called with exclusive .pack-lock. Never wait for a producer while
    /// holding that lock, and never collect a live epoch's installed payloads.
    pub(crate) fn staging_is_live(&self) -> bool {
        let mut visited = 0;
        for namespace in ["staging", "index/staging"] {
            let entries = match fs::read_dir(self.root.join(namespace)) {
                Ok(entries) => entries,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return true,
            };
            for entry in entries {
                visited += 1;
                if visited > 1024 {
                    return true;
                }
                let Ok(entry) = entry else {
                    return true;
                };
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let owner = File::options()
                    .read(true)
                    .write(true)
                    .open(entry.path().join("owner.lock"));
                let _owner = match owner {
                    Ok(file) => match Lock::try_file(file, true) {
                        Ok(lock) => Some(lock),
                        Err(_) => {
                            self.count(Counter::GcProtectedEpochs, 1);
                            return true;
                        }
                    },
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(_) => return true,
                };
                // Ownership acquired, or owner creation never completed. The
                // exclusive publication lock excludes new stage construction.
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        false
    }
    #[cfg(test)]
    pub(crate) fn set_private_limits(&self, bytes: usize, metadata: usize) {
        self.state().epoch_limits = Some((bytes.min(PRIVATE_BYTES), metadata.min(METADATA_BYTES)));
    }
}
