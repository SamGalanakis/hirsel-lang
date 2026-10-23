//! Front query namespace in the shared artifact store. Hits retain mapped
//! ranges; no second payload allocation is made before checking query reads.
use super::{EntryKey, ResultStore};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// The store format version in the directory name.
pub const FORMAT: u32 = 3;
/// The default size cap.
pub const DEFAULT_CAP: u64 = 2 << 30;
/// After this many bytes written in a process, the store enforces its cap.
const GC_EVERY: u64 = 64 << 20;

#[derive(Debug)]
pub struct DirStore {
    root: PathBuf,
    cap: u64,
    written: AtomicU64,
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
}

impl ResultStore for DirStore {
    fn get(&self, key: &EntryKey) -> Option<super::Bytes> {
        let store = hirsel_artifact_store::Store::new(self.root.clone());
        let previous = store.lookup(hirsel_artifact_store::Address(key.0))?;
        if previous.action != hirsel_artifact_store::Action(key.0)
            || previous.reference.domain
                != (hirsel_artifact_store::Domain {
                    kind: 1,
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
        use hirsel_artifact_store::{Action, Address, Domain, Store};
        let store = Store::new(self.root.clone());
        let write = || -> std::io::Result<()> {
            let reference = store.put(
                Domain {
                    kind: 1,
                    schema: FORMAT,
                },
                value,
            )?;
            store.associate(Address(key.0), Action(key.0), reference)
        };
        if write().is_ok() {
            // Include both the immutable record header and its action root.
            let written = value.len() as u64 + 64 + 152;
            let before = self.written.fetch_add(written, Relaxed);
            let interval = GC_EVERY.min(self.cap.max(1));
            if before == 0 || before / interval != (before + written) / interval {
                self.gc(self.cap);
            }
        }
    }
    fn name(&self) -> &'static str {
        "dir@1"
    }
}
