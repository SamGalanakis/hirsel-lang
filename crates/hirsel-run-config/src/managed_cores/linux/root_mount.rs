//! Linux v6.8 fs/kernfs/dir.c:621-633,962-980 assigns inode 1 to the
//! hierarchy root before any descendants. A cgroup2 directory with that inode
//! exposes the complete hierarchy, including all visible ancestor quotas.
use std::{os::fd::AsRawFd, os::unix::fs::MetadataExt, path::PathBuf};

pub(super) fn root_mount() -> Option<PathBuf> {
    let directory = std::fs::File::open("/sys/fs/cgroup").ok()?;
    let metadata = directory.metadata().ok()?;
    // SAFETY: zero initializes a valid output buffer. The live descriptor
    // selects the same directory whose inode was inspected above.
    let mut filesystem: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(directory.as_raw_fd(), &mut filesystem) } != 0
        || !complete_root(filesystem.f_type, metadata.ino(), metadata.is_dir())
    {
        return None;
    }
    Some(PathBuf::from("/sys/fs/cgroup"))
}

fn complete_root(kind: libc::c_long, inode: u64, directory: bool) -> bool {
    kind == libc::CGROUP2_SUPER_MAGIC && inode == 1 && directory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegated_roots_and_other_filesystems_cannot_hide_ancestor_quotas() {
        assert!(complete_root(libc::CGROUP2_SUPER_MAGIC, 1, true));
        assert!(!complete_root(libc::CGROUP2_SUPER_MAGIC, 2, true));
        assert!(!complete_root(libc::CGROUP_SUPER_MAGIC, 1, true));
        assert!(!complete_root(libc::SYSFS_MAGIC, 1, true));
        assert!(!complete_root(libc::TMPFS_MAGIC, 1, true));
        assert!(!complete_root(libc::CGROUP2_SUPER_MAGIC, 1, false));
    }
}
