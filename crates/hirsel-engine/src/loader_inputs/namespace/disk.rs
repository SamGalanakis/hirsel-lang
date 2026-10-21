//! Descriptor-relative capture: Rust std's openat-style component traversal,
//! no following of links while collecting topology. Symlinks are interpreted
//! later by the pure resolver. No device/FIFO/socket is opened for bytes.
use super::*;
use std::ffi::{CString, OsString};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;

fn name(s: &std::ffi::OsStr) -> io::Result<CString> {
    CString::new(s.as_encoded_bytes()).map_err(|_| invalid("NUL in capture path"))
}
fn open_at(parent: &std::fs::File, name: &CString, flags: i32) -> io::Result<std::fs::File> {
    // SAFETY: a live descriptor and terminated name; no creation flags/mode.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat returned this owned fresh descriptor.
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }
}
fn parent(path: &Path) -> io::Result<(std::fs::File, CString)> {
    if !strict_absolute(path) {
        return Err(invalid("non-normal capture path"));
    }
    let mut directory = std::fs::File::open("/")?;
    let components: Vec<_> = path.components().skip(1).collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        directory = open_at(
            &directory,
            &name(component.as_os_str())?,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
    }
    let leaf = components
        .last()
        .map(|c| c.as_os_str())
        .unwrap_or(std::ffi::OsStr::new("."));
    Ok((directory, name(leaf)?))
}
fn open(path: &Path, flags: i32) -> io::Result<std::fs::File> {
    let (directory, leaf) = parent(path)?;
    open_at(&directory, &leaf, flags)
}
impl CaptureSource for DiskCapture {
    fn facts(&self, path: &Path) -> io::Result<Facts> {
        let (directory, leaf) = parent(path)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: live fd/name and valid writable stat buffer.
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                leaf.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstatat initialized stat on success.
        let stat = unsafe { stat.assume_init() };
        let kind = match stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => InputKind::File,
            libc::S_IFDIR => InputKind::Directory,
            libc::S_IFLNK => InputKind::Symlink,
            _ => InputKind::Other,
        };
        Ok(Facts {
            kind,
            device: stat.st_dev,
            inode: stat.st_ino,
            links: stat.st_nlink,
            len: stat.st_size.max(0) as u64,
        })
    }
    fn read(&self, path: &Path, maximum: usize) -> io::Result<Arc<[u8]>> {
        // O_NONBLOCK prevents a raced FIFO from blocking before fstat refuses.
        let file = open(path, libc::O_RDONLY | libc::O_NONBLOCK)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("not a regular capture input"));
        }
        if metadata.size() > maximum as u64 {
            return Err(limit());
        }
        let mut bytes = Vec::new();
        file.take(maximum.saturating_add(1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > maximum {
            return Err(limit());
        }
        Ok(bytes.into())
    }
    fn directory(&self, path: &Path, maximum: usize) -> io::Result<Vec<DirectoryEntry>> {
        let directory = open(path, libc::O_RDONLY | libc::O_DIRECTORY)?;
        // The live descriptor anchors this read_dir even if the original path
        // is renamed/replaced. /proc is used only to enumerate that descriptor.
        let mut entries = Vec::new();
        let mut charge = 0usize;
        for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
            let entry = entry?;
            let name = entry.file_name();
            charge = charge
                .checked_add(name.len())
                .and_then(|n| n.checked_add(128))
                .ok_or_else(limit)?;
            if charge > maximum {
                return Err(limit());
            }
            entries.push(DirectoryEntry {
                name,
                kind: crate::loader_inputs::kind(entry.file_type()?),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
    fn read_link(&self, path: &Path, maximum: usize) -> io::Result<PathBuf> {
        let (directory, leaf) = parent(path)?;
        // Linux's link text is at most PATH_MAX bytes. An extra byte detects
        // truncation without accepting a partially captured link.
        let capacity = maximum.min(libc::PATH_MAX as usize + 1);
        let mut bytes = vec![0u8; capacity];
        // SAFETY: live fd/name and buffer of exactly capacity writable bytes.
        let len = unsafe {
            libc::readlinkat(
                directory.as_raw_fd(),
                leaf.as_ptr(),
                bytes.as_mut_ptr().cast(),
                capacity,
            )
        };
        if len < 0 {
            return Err(io::Error::last_os_error());
        }
        if len as usize >= capacity {
            return Err(limit());
        }
        bytes.truncate(len as usize);
        Ok(PathBuf::from(OsString::from_vec(bytes)))
    }
    fn case_sensitive(&self, path: &Path) -> io::Result<bool> {
        let directory = open(path, libc::O_RDONLY | libc::O_DIRECTORY)?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: live fd and writable statfs buffer.
        if unsafe { libc::fstatfs(directory.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstatfs initialized the buffer on success.
        let ty = unsafe { stat.assume_init() }.f_type;
        // Conservative supported policies. ext4/f2fs can enable per-directory
        // casefold, so inspect flags; unknown filesystems refuse atomically.
        if ty == libc::TMPFS_MAGIC || ty == libc::XFS_SUPER_MAGIC || ty == libc::BTRFS_SUPER_MAGIC {
            return Ok(true);
        }
        if ty == libc::EXT4_SUPER_MAGIC
            || ty == libc::OVERLAYFS_SUPER_MAGIC
            || ty == libc::F2FS_SUPER_MAGIC
        {
            let mut flags: libc::c_long = 0;
            // SAFETY: FS_IOC_GETFLAGS writes one long to this live buffer.
            if unsafe { libc::ioctl(directory.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) } != 0
            {
                return Err(io::Error::last_os_error());
            }
            return Ok(flags & 0x40000000 == 0); // FS_CASEFOLD_FL
        }
        Ok(false)
    }
    fn failed_probe(&self, op: &Operation, maximum: usize) -> io::Result<()> {
        // Fixed failures are explicitly requested capture effects, not lookup
        // fallback. The ordinary adapter preserves exact OS error categories.
        match op {
            Operation::Read(p) => {
                if self.facts(p)?.kind != InputKind::File {
                    return Err(invalid("fixed read probe is not a regular file"));
                }
                self.read(p, maximum).map(|_| ())
            }
            Operation::Canonicalize(p) => std::fs::canonicalize(p).map(|_| ()),
            Operation::Metadata(p) => std::fs::metadata(p).map(|_| ()),
            Operation::Directory(p) => self.directory(p, maximum).map(|_| ()),
        }
    }
}
