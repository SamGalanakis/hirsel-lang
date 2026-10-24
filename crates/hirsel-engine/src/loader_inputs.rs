//! Frozen loading observations. Snapshot shape follows TypeScript-Go
//! 89d5d5b internal/project/snapshotfs.go:20-30,56-104; unlike its fallback,
//! a missing frozen operation is an input error, never a disk read.
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Internal candidate namespace contract. This is not a protocol version.
#[doc(hidden)]
pub mod namespace;

/// Loading reads only this surface; adapters preserve ordinary I/O categories.
pub trait LoaderInputs: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Arc<[u8]>>;
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;
    fn metadata(&self, path: &Path) -> io::Result<InputKind>;
    fn directory(&self, path: &Path) -> io::Result<Vec<DirectoryEntry>>;
    /// An explicit frozen adapter overrides this; ordinary/v1 loading keeps
    /// the toolchain's existing discovery behavior.
    fn toolchain_std_manifest(&self) -> Option<io::Result<PathBuf>> {
        None
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKind {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub name: std::ffi::OsString,
    pub kind: InputKind,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Operation {
    Read(PathBuf),
    Canonicalize(PathBuf),
    Metadata(PathBuf),
    Directory(PathBuf),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputUnavailable(pub Operation);
impl fmt::Display for InputUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uncaptured loader input: {:?}", self.0)
    }
}
impl std::error::Error for InputUnavailable {}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputLimit;
impl fmt::Display for InputLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("loader input allowance exhausted")
    }
}
impl std::error::Error for InputLimit {}
#[derive(Clone, Debug, Eq, PartialEq)]
struct IoFailure {
    kind: io::ErrorKind,
    raw: Option<i32>,
    text: String,
}
impl IoFailure {
    fn capture(e: io::Error) -> Self {
        Self {
            kind: e.kind(),
            raw: e.raw_os_error(),
            text: e.to_string(),
        }
    }
    fn error(&self) -> io::Error {
        match self.raw {
            Some(raw) => io::Error::from_raw_os_error(raw),
            None => io::Error::new(self.kind, self.text.clone()),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Value {
    Bytes(Arc<[u8]>),
    Path(PathBuf),
    Kind(InputKind),
    Directory(Vec<DirectoryEntry>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Observation(Result<Value, IoFailure>);
impl Observation {
    fn result(&self) -> io::Result<Value> {
        self.0.clone().map_err(|e| e.error())
    }
    fn charge(&self) -> usize {
        std::mem::size_of::<Self>()
            + match &self.0 {
                Ok(Value::Bytes(b)) => b.len(),
                Ok(Value::Path(p)) => p.as_os_str().len(),
                Ok(Value::Directory(entries)) => entries
                    .iter()
                    .map(|e| std::mem::size_of::<DirectoryEntry>() + e.name.len())
                    .sum(),
                Err(e) => e.text.len(),
                _ => 0,
            }
    }
}
impl Operation {
    /// The observed path, including failed and negative input observations.
    pub fn path(&self) -> &Path {
        match self {
            Self::Read(p) | Self::Canonicalize(p) | Self::Metadata(p) | Self::Directory(p) => p,
        }
    }
    fn charge(&self) -> usize {
        std::mem::size_of::<Self>() + self.path().as_os_str().len()
    }
}
fn kind(ty: std::fs::FileType) -> InputKind {
    if ty.is_file() {
        InputKind::File
    } else if ty.is_dir() {
        InputKind::Directory
    } else if ty.is_symlink() {
        InputKind::Symlink
    } else {
        InputKind::Other
    }
}
fn read_disk(op: &Operation, maximum: usize) -> io::Result<Value> {
    Ok(match op {
        Operation::Read(p) => {
            // Bound allocation before reading an adversarial/very large input.
            let file = std::fs::File::open(p)?;
            let mut bytes = Vec::new();
            file.take(maximum.saturating_add(1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > maximum {
                return Err(io::Error::other(InputLimit));
            }
            Value::Bytes(bytes.into())
        }
        Operation::Canonicalize(p) => Value::Path(std::fs::canonicalize(p)?),
        Operation::Metadata(p) => Value::Kind(kind(std::fs::metadata(p)?.file_type())),
        Operation::Directory(p) => {
            let mut entries = Vec::new();
            let mut bytes = 0usize;
            for entry in std::fs::read_dir(p)? {
                let entry = entry?;
                let name = entry.file_name();
                bytes = bytes
                    .checked_add(std::mem::size_of::<DirectoryEntry>() + name.len())
                    .ok_or_else(|| io::Error::other(InputLimit))?;
                if bytes > maximum {
                    return Err(io::Error::other(InputLimit));
                }
                entries.push(DirectoryEntry {
                    name,
                    kind: kind(entry.file_type()?),
                });
            }
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            Value::Directory(entries)
        }
    })
}
/// Normal public loader behavior, with no retained observations.
pub struct DiskInputs;
/// Published snapshot; all observations and source bytes are immutable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenInputs {
    observations: Arc<BTreeMap<Operation, Observation>>,
    bytes: usize,
}
/// Read-once capture. All loader reads must go through this adapter, not be
/// collected from watched paths after a separate load.
pub struct CaptureInputs {
    state: Mutex<CaptureState>,
    limits: InputLimits,
}
#[derive(Clone, Copy, Debug)]
pub struct InputLimits {
    pub bytes: usize,
    pub operations: usize,
}
#[derive(Default)]
struct CaptureState {
    observations: BTreeMap<Operation, Observation>,
    bytes: usize,
}
impl CaptureInputs {
    pub fn new(limits: InputLimits) -> Self {
        Self {
            state: Mutex::new(CaptureState::default()),
            limits,
        }
    }
    pub fn freeze(self) -> FrozenInputs {
        let state = self
            .state
            .into_inner()
            .expect("capture input state poisoned");
        FrozenInputs {
            observations: Arc::new(state.observations),
            bytes: state.bytes,
        }
    }
    fn observe(&self, op: Operation) -> io::Result<Value> {
        let mut state = self.state.lock().expect("capture input state poisoned");
        if let Some(value) = state.observations.get(&op) {
            return value.result();
        }
        if state.observations.len() >= self.limits.operations {
            return Err(io::Error::other(InputLimit));
        }
        let available = self.limits.bytes.saturating_sub(state.bytes);
        if op.charge() > available {
            return Err(io::Error::other(InputLimit));
        }
        let value = read_disk(&op, available - op.charge());
        if value
            .as_ref()
            .is_err_and(|e| e.get_ref().is_some_and(|e| e.is::<InputLimit>()))
        {
            return value;
        }
        let observation = Observation(value.map_err(IoFailure::capture));
        let charge = op
            .charge()
            .checked_add(observation.charge())
            .ok_or_else(|| io::Error::other(InputLimit))?;
        if charge > available {
            return Err(io::Error::other(InputLimit));
        }
        let result = observation.result();
        state.bytes += charge;
        state.observations.insert(op, observation);
        result
    }
}
impl FrozenInputs {
    /// Transport/watch metadata for this exact snapshot. The iterator includes
    /// failed reads, negative metadata, directories and canonicalization aliases.
    /// Callers must still use LoaderInputs for bytes; paths confer no validity.
    pub fn operations(&self) -> impl Iterator<Item = &Operation> {
        self.observations.keys()
    }

    /// Resolve only an alias observed by this snapshot, including a canonical
    /// path that appeared as the result of another observed resolution.
    pub fn known_path(&self, path: &Path) -> Option<PathBuf> {
        if let Some(Observation(Ok(Value::Path(target)))) = self
            .observations
            .get(&Operation::Canonicalize(path.to_owned()))
        {
            return Some(target.clone());
        }
        self.observations
            .values()
            .any(|o| matches!(&o.0, Ok(Value::Path(p)) if p==path))
            .then(|| path.to_owned())
    }
    /// Private overlay metadata plus byte allocations absent from the parent.
    /// The caller must retain the parent charge/lifetime while using this value.
    pub fn private_bytes_over(&self, parent: &Self) -> usize {
        let old: std::collections::BTreeSet<_> = parent
            .observations
            .values()
            .filter_map(|v| match &v.0 {
                Ok(Value::Bytes(bytes)) => Some(Arc::as_ptr(bytes) as *const u8 as usize),
                _ => None,
            })
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        self.observations
            .iter()
            .map(|(op, value)| {
                let bytes = match &value.0 {
                    Ok(Value::Bytes(bytes)) => {
                        let pointer = Arc::as_ptr(bytes) as *const u8 as usize;
                        if old.contains(&pointer) || !seen.insert(pointer) {
                            bytes.len()
                        } else {
                            0
                        }
                    }
                    _ => 0,
                };
                op.charge() + value.charge() - bytes + 64
            })
            .sum()
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes + self.observations.len() * 64
    }
    pub fn changed_on_disk(&self) -> Vec<Operation> {
        self.observations
            .iter()
            .filter_map(|(op, old)| {
                let now = Observation(read_disk(op, self.bytes).map_err(IoFailure::capture));
                (now != *old).then(|| op.clone())
            })
            .collect()
    }
    /// Source replacements are admitted by the driver only after all source
    /// keys/digests/ranges have validated. Every already captured read alias
    /// receives the same bytes; no new observation is synthesized here.
    pub fn replace_sources(
        &self,
        replacements: &BTreeMap<PathBuf, Arc<[u8]>>,
        limit: usize,
    ) -> io::Result<Self> {
        let mut observations = (*self.observations).clone();
        let mut matched = std::collections::BTreeSet::new();
        for (op, observation) in &mut observations {
            let Operation::Read(path) = op else { continue };
            if !matches!(observation.0, Ok(Value::Bytes(_))) {
                continue;
            }
            let canonical = self
                .observations
                .get(&Operation::Canonicalize(path.clone()));
            let identity = match canonical {
                Some(Observation(Ok(Value::Path(target)))) => target,
                _ => path,
            };
            if let Some(bytes) = replacements.get(identity) {
                observation.0 = Ok(Value::Bytes(bytes.clone()));
                matched.insert(identity.clone());
            }
        }
        if matched.len() != replacements.len() {
            return Err(io::Error::other(
                "replacement is not an existing captured source read",
            ));
        }
        let bytes = observations
            .iter()
            .try_fold(0usize, |n, (op, value)| {
                n.checked_add(op.charge())?.checked_add(value.charge())
            })
            .filter(|bytes| *bytes <= limit)
            .ok_or_else(|| io::Error::other(InputLimit))?;
        Ok(Self {
            observations: Arc::new(observations),
            bytes,
        })
    }
    /// Length-framed content for the engine's existing ContentHash::of_parts.
    /// Tooling identity is host-local and must also bind explicit compiler,
    /// environment, selection, limits, policy and immutable image identity.
    pub fn identity_parts(&self) -> Vec<Vec<u8>> {
        fn field(out: &mut Vec<u8>, bytes: &[u8]) {
            out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        self.observations
            .iter()
            .map(|(op, value)| {
                let mut out = vec![match op {
                    Operation::Read(_) => 0,
                    Operation::Canonicalize(_) => 1,
                    Operation::Metadata(_) => 2,
                    Operation::Directory(_) => 3,
                }];
                field(&mut out, op.path().as_os_str().as_encoded_bytes());
                match &value.0 {
                    Ok(Value::Bytes(bytes)) => {
                        out.push(0);
                        field(&mut out, bytes);
                    }
                    Ok(Value::Path(path)) => {
                        out.push(1);
                        field(&mut out, path.as_os_str().as_encoded_bytes());
                    }
                    Ok(Value::Kind(kind)) => {
                        out.extend_from_slice(&[2, *kind as u8]);
                    }
                    Ok(Value::Directory(entries)) => {
                        out.push(3);
                        out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
                        for entry in entries {
                            field(&mut out, entry.name.as_encoded_bytes());
                            out.push(entry.kind as u8);
                        }
                    }
                    Err(error) => {
                        out.push(4);
                        field(&mut out, format!("{:?}", error.kind).as_bytes());
                        out.push(u8::from(error.raw.is_some()));
                        out.extend_from_slice(&error.raw.unwrap_or_default().to_le_bytes());
                        field(&mut out, error.text.as_bytes());
                    }
                }
                out
            })
            .collect()
    }
    fn observe(&self, op: Operation) -> io::Result<Value> {
        self.observations
            .get(&op)
            .ok_or_else(|| io::Error::other(InputUnavailable(op)))?
            .result()
    }
}
macro_rules! inputs {
    ($ty:ty,$get:expr) => {
        impl LoaderInputs for $ty {
            fn read(&self, p: &Path) -> io::Result<Arc<[u8]>> {
                match ($get)(self, Operation::Read(p.to_owned()))? {
                    Value::Bytes(v) => Ok(v),
                    _ => unreachable!("read observation"),
                }
            }
            fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
                match ($get)(self, Operation::Canonicalize(p.to_owned()))? {
                    Value::Path(v) => Ok(v),
                    _ => unreachable!("path observation"),
                }
            }
            fn metadata(&self, p: &Path) -> io::Result<InputKind> {
                match ($get)(self, Operation::Metadata(p.to_owned()))? {
                    Value::Kind(v) => Ok(v),
                    _ => unreachable!("metadata observation"),
                }
            }
            fn directory(&self, p: &Path) -> io::Result<Vec<DirectoryEntry>> {
                match ($get)(self, Operation::Directory(p.to_owned()))? {
                    Value::Directory(v) => Ok(v),
                    _ => unreachable!("directory observation"),
                }
            }
        }
    };
}
inputs!(DiskInputs, |_: &DiskInputs, op| read_disk(
    &op,
    usize::MAX - 1
));
inputs!(CaptureInputs, |s: &CaptureInputs, op| s.observe(op));
inputs!(FrozenInputs, |s: &FrozenInputs, op| s.observe(op));

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static IDS: AtomicU64 = AtomicU64::new(1);
    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "hirsel-candidate-input-draft-{}-{}",
                std::process::id(),
                IDS.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn capture() -> CaptureInputs {
        CaptureInputs::new(InputLimits {
            bytes: 64 << 10,
            operations: 100,
        })
    }
    #[test]
    fn frozen_bytes_absence_and_unexpected_read_are_independent_of_disk() {
        let tree = Tree::new();
        let source = tree.0.join("main.hir");
        let sidecar = tree.0.join("main.hir.json");
        std::fs::write(&source, b"old").unwrap();
        let input = capture();
        assert_eq!(&*input.read(&source).unwrap(), b"old");
        assert_eq!(
            input.metadata(&sidecar).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(input.canonicalize(&source).unwrap(), source);
        let frozen = input.freeze();
        assert!(frozen.changed_on_disk().is_empty());
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&sidecar, b"{}").unwrap();
        assert_eq!(&*frozen.read(&source).unwrap(), b"old");
        assert_eq!(
            frozen.metadata(&sidecar).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let absent = frozen.read(&sidecar).unwrap_err();
        assert!(absent.get_ref().unwrap().is::<InputUnavailable>());
        let changed = frozen.changed_on_disk();
        assert_eq!(changed.len(), 2);
        assert!(changed.contains(&Operation::Read(source)));
        assert!(changed.contains(&Operation::Metadata(sidecar)));
    }
    #[test]
    fn captured_failure_is_not_absence_and_directory_probe_is_frozen() {
        let tree = Tree::new();
        let source = tree.0.join("main.hir");
        std::fs::write(&source, b"a").unwrap();
        let input = capture();
        let bad = source.join("child");
        let original = input.metadata(&bad).unwrap_err();
        assert_ne!(original.kind(), io::ErrorKind::NotFound);
        let entries = input.directory(&tree.0).unwrap();
        let frozen = input.freeze();
        assert_eq!(frozen.metadata(&bad).unwrap_err().kind(), original.kind());
        std::fs::write(tree.0.join("another.hir"), b"b").unwrap();
        assert_eq!(frozen.directory(&tree.0).unwrap(), entries);
        assert!(
            frozen
                .changed_on_disk()
                .contains(&Operation::Directory(tree.0.clone()))
        );
    }
    #[test]
    fn capture_repeated_read_never_mixes_versions_and_quota_refuses_admission() {
        let tree = Tree::new();
        let source = tree.0.join("main.hir");
        std::fs::write(&source, b"before").unwrap();
        let input = capture();
        let before = input.read(&source).unwrap();
        std::fs::write(&source, b"after").unwrap();
        assert_eq!(input.read(&source).unwrap(), before);
        assert!(!input.freeze().changed_on_disk().is_empty());
        let tiny = CaptureInputs::new(InputLimits {
            bytes: 0,
            operations: 0,
        });
        assert!(
            tiny.read(&source)
                .unwrap_err()
                .get_ref()
                .unwrap()
                .is::<InputLimit>()
        );
        assert_eq!(tiny.freeze().retained_bytes(), 0);
    }
    #[cfg(unix)]
    #[test]
    fn overlay_replaces_every_captured_symlink_alias_without_disk_writes() {
        let tree = Tree::new();
        let source = tree.0.join("main.hir");
        let alias = tree.0.join("alias.hir");
        std::fs::write(&source, b"before").unwrap();
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let input = capture();
        for path in [&source, &alias] {
            input.canonicalize(path).unwrap();
            input.read(path).unwrap();
        }
        let frozen = input.freeze();
        let edits = BTreeMap::from([(source.clone(), Arc::<[u8]>::from(&b"after"[..]))]);
        let overlay = frozen.replace_sources(&edits, 64 << 10).unwrap();
        assert_eq!(&*overlay.read(&source).unwrap(), b"after");
        assert_eq!(&*overlay.read(&alias).unwrap(), b"after");
        assert_eq!(&*frozen.read(&alias).unwrap(), b"before");
        assert_eq!(std::fs::read(&source).unwrap(), b"before");
        assert_ne!(frozen.identity_parts(), overlay.identity_parts());
        let unknown = BTreeMap::from([(tree.0.join("unknown"), Arc::<[u8]>::from(&b"x"[..]))]);
        assert!(frozen.replace_sources(&unknown, 64 << 10).is_err());
    }
}
