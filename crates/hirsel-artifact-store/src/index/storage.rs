//! Existing store's read-only libc mapping / flock / rename lifetime pattern.
use super::{MAX_BYTES, format::Descriptor, invalid};
use crate::Durability;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) struct Lock {
    _file: File,
}
impl Lock {
    fn open(path: &Path, exclusive: bool, durability: Durability) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let guard = Self { _file: file };
        if durability == Durability::Strict {
            if exclusive {
                guard._file.lock()?;
            } else {
                guard._file.lock_shared()?;
            }
            return Ok(Some(guard));
        }
        #[cfg(unix)]
        {
            let mode = if exclusive {
                libc::LOCK_EX
            } else {
                libc::LOCK_SH
            };
            // SAFETY: valid owned fd; nonblocking flock has no pointer arguments.
            if unsafe { libc::flock(guard._file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    return Ok(None);
                }
                return Err(error);
            }
            Ok(Some(guard))
        }
        #[cfg(not(unix))]
        {
            let _ = (guard, exclusive);
            Ok(None)
        }
    }
    pub(super) fn publication(root: &Path, exclusive: bool) -> io::Result<Option<Self>> {
        Self::for_store(root, exclusive, Durability::Rebuildable)
    }
    pub(super) fn for_store(
        root: &Path,
        exclusive: bool,
        durability: Durability,
    ) -> io::Result<Option<Self>> {
        Self::open(&root.join(".pack-lock"), exclusive, durability)
    }
    pub(super) fn maintenance(root: &Path) -> io::Result<Option<Self>> {
        Self::open(
            &root.join(".index-maintenance-lock"),
            true,
            Durability::Rebuildable,
        )
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // close alone can leave the lock held by a forked child's duplicate
        // open file description. Unlock also covers acquisition error paths.
        #[cfg(unix)]
        // SAFETY: the guard owns a live descriptor until after this destructor.
        unsafe {
            libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
        }
        #[cfg(not(unix))]
        let _ = self._file.unlock();
    }
}

static SERIAL: AtomicU64 = AtomicU64::new(0);
pub(super) struct Stage {
    pub(super) path: PathBuf,
    _owner: Lock,
}
impl Stage {
    pub(super) fn new(root: &Path, durability: Durability) -> io::Result<Self> {
        // Only owner discovery is locked; never bulk construction or writes.
        let _lock = Lock::for_store(root, false, durability)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::WouldBlock))?;
        let staging = root.join("index/staging");
        fs::create_dir_all(&staging)?;
        loop {
            let name = format!(
                "{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            );
            let path = staging.join(name);
            match fs::create_dir(&path) {
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
                Ok(()) => {}
            }
            let owner = Lock::open(&path.join("owner.lock"), true, Durability::Rebuildable);
            match owner {
                Ok(Some(owner)) => {
                    return Ok(Self {
                        path,
                        _owner: owner,
                    });
                }
                other => {
                    let _ = fs::remove_dir_all(&path);
                    return Err(other
                        .err()
                        .unwrap_or_else(|| io::Error::from(io::ErrorKind::WouldBlock)));
                }
            }
        }
    }
    pub(super) fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path.join(name))?
            .write_all(bytes)
    }
    pub(super) fn install(&self, root: &Path, descriptor: Descriptor) -> io::Result<bool> {
        match fs::hard_link(self.path.join("layer"), layer_path(root, descriptor)) {
            Ok(()) => Ok(true),
            // Never replace an old mapping or compare old bodies under lock.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(super) fn layer_path(root: &Path, descriptor: Descriptor) -> PathBuf {
    use std::fmt::Write;
    let mut name = String::with_capacity(66);
    name.push_str("L-");
    for b in descriptor.id.0 {
        let _ = write!(name, "{b:02x}");
    }
    root.join("index").join(name)
}

pub(super) struct Mapping {
    #[cfg(unix)]
    address: *mut libc::c_void,
    len: usize,
}
// SAFETY: same immutable-inode contract as the payload Region. No pointer or
// unchecked mapping escapes the index; all callers retain this owner.
unsafe impl Send for Mapping {}
// SAFETY: no mutation is permitted and shared status uses atomics.
unsafe impl Sync for Mapping {}
impl Mapping {
    pub(super) fn open(path: &Path, expected: u64) -> io::Result<Self> {
        if !(128..=MAX_BYTES as u64).contains(&expected) {
            return Err(invalid());
        }
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != expected {
            return Err(invalid());
        }
        let len = usize::try_from(expected).map_err(|_| invalid())?;
        #[cfg(unix)]
        {
            // SAFETY: checked nonempty regular file, immutable read-only map.
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
            Ok(Self { address, len })
        }
        #[cfg(not(unix))]
        {
            let _ = (file, len);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "index mappings require Unix",
            ))
        }
    }
    #[inline]
    pub(super) fn bytes(&self) -> &[u8] {
        #[cfg(unix)]
        // SAFETY: this owner retains exactly len immutable mapped bytes.
        unsafe {
            std::slice::from_raw_parts(self.address.cast(), self.len)
        }
        #[cfg(not(unix))]
        {
            &[]
        }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: all borrowed views end before this exclusive destructor.
        unsafe {
            libc::munmap(self.address, self.len);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn released_index_lock_does_not_remain_held_by_an_inherited_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lock");
        for durability in [Durability::Strict, Durability::Rebuildable] {
            for exclusive in [false, true] {
                let guard = Lock::open(&path, exclusive, durability).unwrap().unwrap();
                // dup preserves the same open file description, just like the
                // child's descriptor between fork and its close-on-exec step.
                let inherited = guard._file.try_clone().unwrap();
                drop(guard);
                let contender = Lock::open(&path, true, Durability::Rebuildable)
                    .unwrap()
                    .expect("guard drop must release the lock while the inherited fd lives");
                drop(contender);
                drop(inherited);
            }
        }
    }
}
