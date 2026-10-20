//! `dir@1`: one checksummed file per entry under
//! `$XDG_CACHE_HOME/hirsel/results-v<N>`. Writes go to a temp file that is
//! renamed into place, so readers never see a partial entry and no lock is
//! taken. A size cap (2 GiB by default) is enforced by evicting the least
//! recently used entries by mtime; a hit refreshes its entry's mtime.
use super::{EntryKey, ResultStore, frame, unframe};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::SystemTime;

/// The store format version in the directory name.
pub const FORMAT: u32 = 2;
/// The default size cap.
pub const DEFAULT_CAP: u64 = 2 << 30;
/// After this many bytes written in a process, the store enforces its cap.
const GC_EVERY: u64 = 64 << 20;

#[derive(Debug)]
pub struct DirStore {
    root: PathBuf,
    cap: u64,
    written: AtomicU64,
    temp: AtomicU64,
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
            temp: AtomicU64::new(0),
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
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        Some(Self::new(
            base.join("hirsel").join(format!("results-v{FORMAT}")),
        ))
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self, key: &EntryKey) -> PathBuf {
        let hex = key.hex();
        self.root.join(&hex[..2]).join(&hex)
    }

    /// Evicts least recently used entries until the store is within `cap`,
    /// and removes temp files a crashed writer left behind.
    pub fn gc(&self, cap: u64) -> GcReport {
        let mut report = GcReport::default();
        let mut files: Vec<(SystemTime, u64, PathBuf)> = vec![];
        let Ok(shards) = fs::read_dir(&self.root) else {
            return report;
        };
        for shard in shards.flatten() {
            let shard = shard.path();
            if shard.file_name().is_some_and(|n| n == "tmp") {
                for stale in fs::read_dir(&shard).into_iter().flatten().flatten() {
                    let old = stale
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age.as_secs() > 3600);
                    if old {
                        let _ = fs::remove_file(stale.path());
                    }
                }
                continue;
            }
            for file in fs::read_dir(&shard).into_iter().flatten().flatten() {
                if let Ok(meta) = file.metadata()
                    && meta.is_file()
                {
                    let time = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    files.push((time, meta.len(), file.path()));
                }
            }
        }
        report.entries = files.len() as u64;
        report.bytes = files.iter().map(|f| f.1).sum();
        // Oldest first; ties by path, so the order does not depend on readdir.
        files.sort();
        let mut total = report.bytes;
        for (_, len, path) in files {
            if total <= cap {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                report.removed += 1;
                report.removed_bytes += len;
            }
        }
        report
    }
    pub fn cap(&self) -> u64 {
        self.cap
    }
}

impl ResultStore for DirStore {
    fn get(&self, key: &EntryKey) -> Option<Arc<[u8]>> {
        let path = self.path(key);
        let bytes = fs::read(&path).ok()?;
        match unframe(&bytes) {
            Some(payload) => {
                // Refresh the LRU clock; failure only ages the entry.
                if let Ok(file) = fs::File::options().append(true).open(&path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                Some(payload.into())
            }
            None => {
                let _ = fs::remove_file(&path);
                None
            }
        }
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        let path = self.path(key);
        let temp_dir = self.root.join("tmp");
        let framed = frame(value);
        let write = || -> std::io::Result<()> {
            fs::create_dir_all(path.parent().unwrap())?;
            fs::create_dir_all(&temp_dir)?;
            let temp = temp_dir.join(format!(
                "{}.{}.{}",
                key.hex(),
                std::process::id(),
                self.temp.fetch_add(1, Relaxed)
            ));
            fs::write(&temp, &framed)?;
            fs::rename(&temp, &path).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })
        };
        // A failed write is a later miss, never a compile failure.
        if write().is_ok() {
            let before = self.written.fetch_add(framed.len() as u64, Relaxed);
            if (before + framed.len() as u64) / GC_EVERY != before / GC_EVERY {
                self.gc(self.cap);
            }
        }
    }
    fn name(&self) -> &'static str {
        "dir@1"
    }
}
