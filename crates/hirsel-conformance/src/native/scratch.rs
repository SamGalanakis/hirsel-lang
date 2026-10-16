//! Scratch directories of the native tools. Each process works under its own
//! run root, so concurrent runs in one checkout (two `gate-native-full`
//! commands, an agreement campaign beside a hash sweep) never share a path. A
//! run root is held by an OS file lock for its owner's lifetime; a later run
//! removes every root whose lock is free, so a finished run's reports and
//! mismatch artifacts stay until the next run and disk use stays bounded.
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The directory of the native tools beside the running executable.
fn base() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(exe
        .parent()
        .ok_or("the executable has no directory")?
        .join("native-gate"))
}

/// One process's scratch root: `<parent>/<id>`, locked by `<parent>/<id>.lock`.
pub struct RunRoot {
    dir: PathBuf,
    _lock: File,
}

impl RunRoot {
    /// A new run root under `parent`, after removing every root there whose
    /// owner has exited. No two live runs get the same directory: the name is
    /// the process ID and a timestamp, and it is created exclusively.
    pub fn create_in(parent: &Path) -> Result<Self, String> {
        let error = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
        std::fs::create_dir_all(parent).map_err(|e| error(parent, e))?;
        prune(parent);
        let mut attempt = 0u32;
        loop {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let id = format!("{}-{nanos}-{attempt}", std::process::id());
            let dir = parent.join(&id);
            // The lock is taken before the directory exists, so a pruning run
            // never sees a live root unlocked.
            let lock_path = parent.join(format!("{id}.lock"));
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
                .map_err(|e| error(&lock_path, e))?;
            lock.lock().map_err(|e| error(&lock_path, e))?;
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self { dir, _lock: lock }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 16 => {
                    attempt += 1;
                }
                Err(e) => return Err(error(&dir, e)),
            }
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `name` under this root, created.
    pub fn join(&self, name: &str) -> Result<PathBuf, String> {
        let dir = self.dir.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(dir)
    }
}

/// Remove every run root under `parent` whose lock no live process holds. A
/// root without a lock file is left by an interrupted removal.
fn prune(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let lock_path = dir.with_extension("lock");
        let lock = match OpenOptions::new().write(true).open(&lock_path) {
            Ok(lock) => match lock.try_lock() {
                Ok(()) => Some(lock),
                Err(_) => continue,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => continue,
        };
        let _ = std::fs::remove_dir_all(&dir);
        if lock.is_some() {
            let _ = std::fs::remove_file(&lock_path);
        }
    }
}

/// This process's run root under `native-gate/runs` beside the executable.
fn run_root() -> Result<&'static RunRoot, String> {
    static ROOT: OnceLock<Result<RunRoot, String>> = OnceLock::new();
    ROOT.get_or_init(|| RunRoot::create_in(&base()?.join("runs")))
        .as_ref()
        .map_err(Clone::clone)
}

/// A scratch directory `name` of this run.
pub fn scratch(name: &str) -> Result<PathBuf, String> {
    run_root()?.join(name)
}

/// The runtime archive cache beside the executable. Every run shares it: its
/// archives are content-addressed and built under a lock
/// (`hirsel_driver::native::runtime_cache`).
pub fn runtime_cache() -> Result<PathBuf, String> {
    let dir = base()?.join("runtime").join("cache");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}
