//! Request-private std filesystem spooling. No durable index, cache lookup or
//! verifier capability is created. BTree/atomic admission stays in the parent.
use super::*;
#[cfg(not(unix))]
use std::io::{Read, Write};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn index_bytes(entries: usize) -> u64 {
    let mut weight = crate::NativeMetadataWeight::default();
    weight.tree::<ContentHash, (Reference, u64)>(entries);
    weight.bytes()
}
struct Files {
    disk: u64,
    metadata: u64,
    entries: BTreeMap<ContentHash, (Reference, u64)>,
    spool: fs::File,
}
pub(super) struct ScratchStore {
    selected: Arc<dyn ResultStore>,
    // Close the spool before the last directory owner can remove its path.
    files: Mutex<Files>,
    admission: Arc<Admission>,
    root: Arc<Root>,
    disk_limit: u64,
}
struct ReadOwner {
    admission: Arc<Admission>,
    _root: Arc<Root>,
    bytes: u64,
}
impl Drop for ReadOwner {
    fn drop(&mut self) {
        self.admission.release(self.bytes);
    }
}
impl ScratchStore {
    pub(super) fn new(
        selected: Arc<dyn ResultStore>,
        admission: Arc<Admission>,
        parent: &Path,
        disk_limit: u64,
    ) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = parent.join(format!(
            ".hirsel-request-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        let root = Arc::new(Root(path));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let spool = options.open(root.0.join("contents"))?;
        let metadata = ENTRY_CHARGE
            .saturating_add(root.0.capacity() as u64)
            .saturating_add(index_bytes(0));
        if !admission.charge(metadata) {
            return Err(std::io::Error::other(
                "scratch metadata exceeds admitted transport",
            ));
        }
        let _ = admission.scratch.set(root.clone());
        Ok(Self {
            selected,
            admission,
            root,
            disk_limit,
            files: Mutex::new(Files {
                disk: 0,
                metadata,
                entries: BTreeMap::new(),
                spool,
            }),
        })
    }
}
impl Drop for ScratchStore {
    fn drop(&mut self) {
        self.admission.release(
            self.files
                .get_mut()
                .unwrap_or_else(|e| e.into_inner())
                .metadata,
        );
    }
}
impl ResultStore for ScratchStore {
    fn query_cache_enabled(&self) -> bool {
        self.selected.query_cache_enabled()
    }
    fn read_scope(&self) -> Option<Box<dyn Send + Sync>> {
        self.selected.read_scope()
    }
    fn name(&self) -> &'static str {
        self.selected.name()
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.selected.local_product_store()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.selected.verifier()
    }
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.selected.get(key)
    }
    fn put(&self, key: &EntryKey, bytes: &[u8]) {
        self.selected.put(key, bytes);
    }
    fn demand(&self, keys: &[EntryKey]) {
        self.selected.demand(keys);
    }
    fn lookup_product(&self, address: Address) -> Option<Association> {
        self.selected.lookup_product(address)
    }
    fn associate_product(&self, address: Address, action: Action, reference: Reference) {
        self.selected.associate_product(address, action, reference);
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        let expected = domain.reference(bytes)?;
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((actual, _)) = files.entries.get(&expected.output.0) {
            return (*actual == expected).then_some(expected);
        }
        if expected.len > self.disk_limit.saturating_sub(files.disk) {
            return None;
        }
        // One append-only private file owns the payloads. Exact ranges and
        // content hashes isolate records; no per-record path is retained.
        let metadata = index_bytes(files.entries.len().checked_add(1)?)
            .checked_sub(index_bytes(files.entries.len()))?;
        if !self.admission.charge(metadata) {
            return None;
        }
        let offset = files.disk;
        if write_at(&files.spool, bytes, offset).is_err() {
            let _ = files.spool.set_len(offset);
            self.admission.release(metadata);
            return None;
        }
        files.disk += expected.len;
        files.metadata += metadata;
        files.entries.insert(expected.output.0, (expected, offset));
        drop(files);
        // A successful cache put is still evictable. The current request owns
        // its transport independently of later durable/cache reclamation.
        let _ = self.selected.put_content(domain, bytes);
        Some(expected)
    }
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        let files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let Some((actual, offset)) = files.entries.get(&reference.output.0) else {
            drop(files);
            return self.selected.get_content(reference);
        };
        if *actual != reference {
            return None;
        }
        if files.spool.metadata().ok()?.len() != files.disk {
            return None;
        }
        // Serialize allocation of the temporary Vec/Arc conversion. Afterwards
        // each live Bytes owns only one payload plus its retained admission.
        let retained = reference.len.checked_add(ENTRY_CHARGE)?;
        let peak = retained.checked_add(reference.len)?;
        if !self.admission.charge(peak) {
            return None;
        }
        let read = || -> Option<Bytes> {
            let mut value = vec![0; usize::try_from(reference.len).ok()?];
            read_at(&files.spool, &mut value, *offset).ok()?;
            if reference.domain.reference(&value)? != reference {
                return None;
            }
            Some(Bytes::from(value))
        };
        let value = read();
        if value.is_none() {
            self.admission.release(peak);
            return None;
        }
        self.admission.release(reference.len);
        let owner = Arc::new(ReadOwner {
            admission: self.admission.clone(),
            _root: self.root.clone(),
            bytes: retained,
        });
        Some(value.unwrap().retain_owner(owner))
    }
}

// Rust std FileExt supplies retry/short-I/O handling without reopening or
// changing a shared cursor. Non-Unix hosts serialize their cursor under Files.
fn write_at(file: &fs::File, bytes: &[u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(bytes, offset)
    }
    #[cfg(not(unix))]
    {
        use std::io::{Seek, SeekFrom};
        let mut file = file;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)
    }
}
fn read_at(file: &fs::File, bytes: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(bytes, offset)
    }
    #[cfg(not(unix))]
    {
        use std::io::{Seek, SeekFrom};
        let mut file = file;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ProcessBudget;
    fn domain() -> Domain {
        Domain {
            kind: 0x4e00001f,
            schema: 1,
        }
    }
    #[test]
    fn scratch_transports_more_than_ram_and_last_child_owns_cleanup() {
        let parent = tempfile::tempdir().unwrap();
        let ram = 256 << 10;
        let budget = ProcessBudget::new(ram);
        let request = RequestStore::with_scratch(
            Arc::new(NoneStore),
            budget.reserve(ram).unwrap(),
            parent.path(),
            2 << 20,
        )
        .unwrap();
        let directory = fs::read_dir(parent.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut references = Vec::new();
        for n in 0..20u8 {
            references.push(request.put_content(domain(), &vec![n; 32 << 10]).unwrap());
        }
        assert!(request.retained_bytes() < ram);
        assert!(request.local_product_store().is_none());
        assert!(request.verifier().is_none());
        let held = request
            .get_content(references[19])
            .unwrap()
            .range(7, 13, 1)
            .unwrap();
        assert_eq!(&*held, &[19; 13]);
        assert!(request.retained_bytes() <= ram);
        drop(request);
        assert!(directory.exists());
        assert_eq!(budget.reserved(), ram);
        assert_eq!(&*held, &[19; 13]);
        drop(held);
        assert!(!directory.exists());
        assert_eq!(budget.reserved(), 0);
    }
    #[test]
    fn scratch_many_small_records_use_their_index_layout_bound() {
        let parent = tempfile::tempdir().unwrap();
        let ram = 256 << 10;
        let budget = ProcessBudget::new(ram);
        let request = RequestStore::with_scratch(
            Arc::new(NoneStore),
            budget.reserve(ram).unwrap(),
            parent.path(),
            2 << 20,
        )
        .unwrap();
        let mut last = None;
        for number in 0u32..400 {
            let mut value = vec![0; 256];
            value[..4].copy_from_slice(&number.to_le_bytes());
            last = Some(
                request
                    .put_content(domain(), &value)
                    .expect("bounded descriptor index"),
            );
        }
        let held = request.get_content(last.unwrap()).unwrap();
        assert_eq!(&held[..4], &399u32.to_le_bytes());
        assert!(request.retained_bytes() <= ram);
        assert!(budget.reserve(1).is_err());
        drop(request);
        assert_eq!(budget.reserved(), ram);
        drop(held);
        assert_eq!(budget.reserved(), 0);
    }
    #[test]
    fn scratch_live_reads_disk_and_corruption_never_expand_admission() {
        let parent = tempfile::tempdir().unwrap();
        let ram = 128 << 10;
        let budget = ProcessBudget::new(ram);
        let request = RequestStore::with_scratch(
            Arc::new(NoneStore),
            budget.reserve(ram).unwrap(),
            parent.path(),
            64 << 10,
        )
        .unwrap();
        let first = request.put_content(domain(), &vec![1; 32 << 10]).unwrap();
        let second = request.put_content(domain(), &vec![2; 32 << 10]).unwrap();
        let directory = fs::read_dir(parent.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let disk: u64 = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        assert_eq!(disk, 64 << 10);
        // Files are untrusted bytes, never acceptance authority.
        let spool = fs::OpenOptions::new()
            .write(true)
            .open(directory.join("contents"))
            .unwrap();
        write_at(&spool, b"corrupted", 0).unwrap();
        let before = request.retained_bytes();
        assert!(request.get_content(first).is_none());
        assert_eq!(request.retained_bytes(), before);
        let held = request.get_content(second).unwrap();
        assert_eq!(&*held, &vec![2; 32 << 10]);
        spool.set_len((64 << 10) - 1).unwrap();
        assert!(
            request.get_content(second).is_none(),
            "truncated last record"
        );
        assert_eq!(
            &*held,
            &vec![2; 32 << 10],
            "held bytes survive spool damage"
        );
        assert!(request.retained_bytes() <= ram);
        assert!(request.put_content(domain(), &vec![3; 128 << 10]).is_none());
        assert!(request.retained_bytes() <= ram);
        assert!(budget.reserve(1).is_err());
    }
    #[test]
    fn scratch_preserves_selected_backend_capabilities_and_zero_refuses() {
        let parent = tempfile::tempdir().unwrap();
        let empty = ProcessBudget::new(0);
        assert!(
            RequestStore::with_scratch(
                Arc::new(NoneStore),
                empty.reserve(0).unwrap(),
                parent.path(),
                1024
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
        let selected = Arc::new(MemStore::default());
        let verifier = selected.verifier().unwrap();
        let budget = ProcessBudget::new(8 << 10);
        let request = RequestStore::with_scratch(
            selected.clone(),
            budget.reserve(8 << 10).unwrap(),
            parent.path(),
            0,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&request.verifier().unwrap(), &verifier));
        let before = request.retained_bytes();
        let reference = request.put_content(domain(), b"selected provider").unwrap();
        assert!(request.retained_bytes() > before);
        drop(selected.take());
        assert_eq!(
            &*request.get_content(reference).unwrap(),
            b"selected provider"
        );
        drop(request);
        let request = RequestStore::with_scratch(
            selected.clone(),
            budget.reserve(8 << 10).unwrap(),
            parent.path(),
            1024,
        )
        .unwrap();
        let reference = request.put_content(domain(), b"selected provider").unwrap();
        // Successful provider publication must not make an evictable cache
        // the only owner of a settled request's original bytes.
        assert_eq!(
            &*selected.get_content(reference).unwrap(),
            b"selected provider"
        );
        drop(selected.take());
        assert!(selected.get_content(reference).is_none());
        assert_eq!(
            &*request.get_content(reference).unwrap(),
            b"selected provider"
        );
        assert!(request.retained_bytes() <= 8 << 10);
        assert!(!request.exhausted());
    }
}
