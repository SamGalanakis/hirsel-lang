//! Object storage: immutable objects by key, written create-if-absent. This
//! is the storage seam the code store and the object-store-backed providers
//! share: [`ObjectStorage`] is a conditional PUT (`If-None-Match: *`) and a
//! GET, which a directory, an S3-compatible bucket and a simulation all
//! implement.

#[cfg(test)]
hirsel_guard::install!();
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The outcome of [`ObjectStorage::put_new`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Put {
    /// The object did not exist and now holds the bytes.
    Created,
    /// An object already existed under the key; it is unchanged, whatever
    /// its bytes.
    Exists,
}

/// Why an object operation did not complete. A failed `put_new` may or may
/// not have created the object; a later `get` or `put_new` says which.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageError {
    /// The key is not a valid object key.
    Key(String),
    /// The storage did not answer (a transient failure).
    Unavailable(String),
    /// Any other failure of the medium.
    Io(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Key(key) => write!(f, "invalid object key {key:?}"),
            Self::Unavailable(message) => write!(f, "object storage unavailable: {message}"),
            Self::Io(message) => write!(f, "object storage failed: {message}"),
        }
    }
}

impl std::error::Error for StorageError {}

/// Immutable objects by key. Keys are `/`-separated segments of lowercase
/// ASCII letters, digits, `.`, `_` and `-`, none empty or starting with `.`
/// ([`check_key`]).
pub trait ObjectStorage: Send + Sync {
    /// The object under `key`, if one exists.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    /// Create the object under `key` with `bytes` when none exists, whole or
    /// not at all, durably before returning `Created`; an existing object
    /// is left unchanged.
    fn put_new(&self, key: &str, bytes: &[u8]) -> Result<Put, StorageError>;
}

/// Refuse a key outside the object key grammar.
pub fn check_key(key: &str) -> Result<(), StorageError> {
    let segment = |s: &str| {
        !s.is_empty()
            && !s.starts_with('.')
            && s.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            })
    };
    if key.split('/').all(segment) {
        Ok(())
    } else {
        Err(StorageError::Key(key.into()))
    }
}

/// Objects as files under a directory (`objects.dir@1`). A put writes a
/// temporary file, syncs it, hard-links it to its key (which fails when the
/// key exists, so creation is atomic and never replaces) and syncs the
/// directory.
#[derive(Clone, Debug)]
pub struct DirObjects {
    root: PathBuf,
}

static TEMPORARY: AtomicU64 = AtomicU64::new(0);

impl DirObjects {
    /// Objects under `root`, which is created when absent.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| io(&root, e))?;
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn io(path: &Path, error: std::io::Error) -> StorageError {
    StorageError::Io(format!("{}: {error}", path.display()))
}

impl ObjectStorage for DirObjects {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        check_key(key)?;
        let path = self.root.join(key);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(&path, e)),
        }
    }
    fn put_new(&self, key: &str, bytes: &[u8]) -> Result<Put, StorageError> {
        check_key(key)?;
        let path = self.root.join(key);
        let parent = path
            .parent()
            .expect("an object key names a file under the root");
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        if path.exists() {
            return Ok(Put::Exists);
        }
        // A leading '.' keeps temporary names out of the key space.
        let temporary = parent.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            let mut file = File::create_new(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&temporary);
            return Err(io(&temporary, e));
        }
        let linked = fs::hard_link(&temporary, &path);
        let _ = fs::remove_file(&temporary);
        match linked {
            Ok(()) => {
                File::open(parent)
                    .and_then(|dir| dir.sync_all())
                    .map_err(|e| io(parent, e))?;
                Ok(Put::Created)
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(Put::Exists),
            Err(e) => Err(io(&path, e)),
        }
    }
}

/// In-memory objects with an S3 bucket's conditional-PUT semantics
/// (`objects.sim@1`), optionally failing operations at seeded points. A
/// faulted put fails before or after its write, as a lost reply does, so
/// callers see both outcomes of an uncertain write.
#[derive(Debug, Default)]
pub struct MemoryObjects {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    faults: Mutex<Option<Faults>>,
}

#[derive(Debug)]
struct Faults {
    state: u64,
    one_in: u64,
}

impl Faults {
    /// SplitMix64.
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

impl MemoryObjects {
    pub fn new() -> Self {
        Self::default()
    }
    /// Objects of which each operation fails with probability `1/one_in`,
    /// decided by a SplitMix64 stream from `seed`; `one_in` 0 never fails.
    pub fn with_faults(seed: u64, one_in: u64) -> Self {
        let objects = Self::new();
        if one_in > 0 {
            *objects.faults.lock().expect("faults") = Some(Faults {
                state: seed,
                one_in,
            });
        }
        objects
    }
    /// The next fault decision: None, or whether the faulted operation's
    /// effect still happens.
    fn fault(&self) -> Option<bool> {
        let mut faults = self.faults.lock().expect("faults");
        let faults = faults.as_mut()?;
        let draw = faults.next();
        (draw % faults.one_in == 0).then_some(draw & (1 << 63) != 0)
    }
    /// Replace an object's bytes, as a hostile or broken medium would.
    pub fn overwrite(&self, key: &str, bytes: Vec<u8>) {
        self.objects
            .lock()
            .expect("objects")
            .insert(key.into(), bytes);
    }
    /// Every object, read past any fault.
    pub fn contents(&self) -> BTreeMap<String, Vec<u8>> {
        self.objects.lock().expect("objects").clone()
    }
}

fn unavailable(operation: &str, key: &str) -> StorageError {
    StorageError::Unavailable(format!("simulated fault on {operation} {key}"))
}

impl ObjectStorage for MemoryObjects {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        check_key(key)?;
        if self.fault().is_some() {
            return Err(unavailable("get", key));
        }
        Ok(self.objects.lock().expect("objects").get(key).cloned())
    }
    fn put_new(&self, key: &str, bytes: &[u8]) -> Result<Put, StorageError> {
        check_key(key)?;
        let fault = self.fault();
        if fault == Some(false) {
            return Err(unavailable("put", key));
        }
        let mut objects = self.objects.lock().expect("objects");
        let put = if objects.contains_key(key) {
            Put::Exists
        } else {
            objects.insert(key.into(), bytes.to_vec());
            Put::Created
        };
        match fault {
            Some(_) => Err(unavailable("put reply", key)),
            None => Ok(put),
        }
    }
}

/// Open an object storage provider entry: `{"provider": "objects.dir@1",
/// "path": "<absolute directory>"}` or `{"provider": "objects.sim@1",
/// "seed": <u64>, "fail_one_in": <u64>}` (both members optional, default
/// 0). Unknown members and providers are refused.
pub fn open(entry: &serde_json::Value) -> Result<Arc<dyn ObjectStorage>, String> {
    let members = entry
        .as_object()
        .ok_or("an object storage entry is a JSON object")?;
    let provider = members
        .get("provider")
        .and_then(|p| p.as_str())
        .ok_or("an object storage entry names its provider")?;
    let allowed: &[&str] = match provider {
        "objects.dir@1" => &["provider", "path"],
        "objects.sim@1" => &["provider", "seed", "fail_one_in"],
        other => return Err(format!("unsupported object storage provider {other}")),
    };
    if let Some(unknown) = members.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("{provider} has no member {unknown:?}"));
    }
    let uint = |name: &str| match members.get(name) {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{provider} {name} is an unsigned integer")),
    };
    match provider {
        "objects.dir@1" => {
            let path = members
                .get("path")
                .and_then(|p| p.as_str())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or("objects.dir@1 path is an absolute directory")?;
            Ok(Arc::new(DirObjects::open(path).map_err(|e| e.to_string())?))
        }
        _ => Ok(Arc::new(MemoryObjects::with_faults(
            uint("seed")?,
            uint("fail_one_in")?,
        ))),
    }
}
