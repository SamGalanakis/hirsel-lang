//! Front query namespace in the shared artifact store. Hits retain mapped
//! ranges; no second payload allocation is made before checking query reads.
use super::{EntryKey, ResultStore};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// The store format version in the directory name.
pub const FORMAT: u32 = 11;
/// The default size cap.
pub const DEFAULT_CAP: u64 = 2 << 30;
/// After this many bytes written in a process, the store enforces its cap.
const GC_EVERY: u64 = 64 << 20;

#[derive(Debug)]
pub struct DirStore {
    root: PathBuf,
    cap: u64,
    written: AtomicU64,
    verifier: std::sync::Arc<hirsel_validate::VerifierCache>,
}

/// What a collection found and removed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GcReport {
    pub entries: u64,
    pub bytes: u64,
    pub removed: u64,
    pub removed_bytes: u64,
}

impl DirStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            verifier: std::sync::Arc::new(hirsel_validate::VerifierCache::directory(root.clone())),
            root,
            cap: DEFAULT_CAP,
            written: AtomicU64::new(0),
        }
    }
    pub fn with_cap(mut self, cap: u64) -> Self {
        self.cap = cap;
        self
    }
    /// `$XDG_CACHE_HOME/hirsel/results-v<N>`, else `$HOME/.cache/...`.
    pub fn default_location() -> Option<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .map(|h| PathBuf::from(h).join(".cache"))
            })?;
        // Like Go's os.UserCacheDir: an absent or relative location is not
        // authority to write a cache into the current working directory.
        if !base.is_absolute() {
            return None;
        }
        Some(Self::new(
            base.join("hirsel").join(format!("results-v{FORMAT}")),
        ))
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Collect immutable outputs and action roots with bounded scratch.
    pub fn gc(&self, cap: u64) -> GcReport {
        let report = hirsel_artifact_store::Store::new(self.root.clone()).collect(cap);
        GcReport {
            entries: report.entries,
            bytes: report.bytes,
            removed: report.removed,
            removed_bytes: report.removed_bytes,
        }
    }
    pub fn cap(&self) -> u64 {
        self.cap
    }
    fn account(&self, len: u64) {
        let before = self.written.fetch_add(len, Relaxed);
        let interval = GC_EVERY.min(self.cap.max(1));
        if before == 0 || before / interval != before.saturating_add(len) / interval {
            self.gc(self.cap);
        }
    }
    fn publish(&self, key: &EntryKey, action: super::Action, value: &[u8]) {
        use hirsel_artifact_store::{Address, Domain, Store};
        let store = Store::new(self.root.clone());
        let write = || -> std::io::Result<()> {
            let reference = store.put(
                Domain {
                    kind: key.2 as u32 + 256,
                    schema: FORMAT,
                },
                value,
            )?;
            store.associate(Address(key.0), action, reference)
        };
        if write().is_ok() {
            self.account(value.len() as u64 + 64 + 152);
        }
    }
}

impl ResultStore for DirStore {
    fn verifier(&self) -> Option<std::sync::Arc<hirsel_validate::VerifierCache>> {
        Some(self.verifier.clone())
    }
    fn get_content(&self, reference: super::Reference) -> Option<super::Bytes> {
        let store = hirsel_artifact_store::Store::new(self.root.clone());
        let bytes = store.get(reference)?;
        store.touch(
            hirsel_artifact_store::Address(reference.output.0),
            reference,
        );
        Some(bytes)
    }
    fn put_content(&self, domain: super::Domain, bytes: &[u8]) -> Option<super::Reference> {
        let reference = hirsel_artifact_store::Store::new(self.root.clone())
            .put(domain, bytes)
            .ok()?;
        self.account(bytes.len() as u64 + 64);
        Some(reference)
    }
    fn get(&self, key: &EntryKey) -> Option<super::Bytes> {
        let store = hirsel_artifact_store::Store::new(self.root.clone());
        let previous = store.lookup(hirsel_artifact_store::Address(key.0))?;
        // The address finds the previous action. The query validates its
        // explicit inputs and dependency outputs before decoding its payload.
        if previous.reference.domain
            != (hirsel_artifact_store::Domain {
                kind: key.2 as u32 + 256,
                schema: FORMAT,
            })
        {
            return None;
        }
        let bytes = match store.get(previous.reference) {
            Some(bytes) => bytes,
            None => {
                let _ = fs::remove_file(store.output_path(previous.reference));
                return None;
            }
        };
        store.touch(hirsel_artifact_store::Address(key.0), previous.reference);
        Some(bytes)
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.publish(
            key,
            super::Action(hirsel_syntax::ContentHash::of_parts(
                "hirsel.result.raw-action.v1",
                [key.0.0.as_slice(), key.1.0.as_slice()],
            )),
            value,
        );
    }
    fn put_query(
        &self,
        kind: crate::QueryKind,
        key: &EntryKey,
        action: super::Action,
        value: &[u8],
    ) {
        assert_eq!(kind, key.2);
        self.publish(key, action, value);
    }
    fn name(&self) -> &'static str {
        "dir@1"
    }
}
