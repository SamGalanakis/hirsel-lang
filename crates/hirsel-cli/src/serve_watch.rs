//! File events wake the existing byte-comparison authority. Directory watches
//! survive editor rename-over saves and observe absent loader sidecars. File
//! watches additionally follow symlinks. As in fsnotify/backend_inotify.go
//! (20b1e15, AddWith:185-221 and readEvents:353-410), CLOEXEC owns the descriptor
//! and every read, including queue overflow, causes reconciliation. Unlike a
//! general watcher we need no event codec: the session reads current bytes.
use std::collections::BTreeSet;
use std::ffi::CString;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

pub(super) struct Watch(std::fs::File);
impl Watch {
    pub(super) fn new(paths: impl IntoIterator<Item = PathBuf>) -> io::Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let watch = Self(unsafe { std::fs::File::from_raw_fd(fd) });
        let mut directories = BTreeSet::new();
        for path in paths {
            let path = std::path::absolute(path)?;
            // Existing sources can name symlinks whose targets live elsewhere.
            if path.exists() {
                watch.add(
                    &path,
                    libc::IN_MODIFY
                        | libc::IN_CLOSE_WRITE
                        | libc::IN_ATTRIB
                        | libc::IN_DELETE_SELF
                        | libc::IN_MOVE_SELF,
                )?;
            }
            if let Some(parent) = path
                .parent()
                .and_then(|parent| parent.ancestors().find(|p| p.is_dir()))
            {
                directories.insert(parent.to_path_buf());
            }
        }
        if directories.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no input directory to watch",
            ));
        }
        for directory in directories {
            watch.add(
                &directory,
                libc::IN_CLOSE_WRITE
                    | libc::IN_ATTRIB
                    | libc::IN_CREATE
                    | libc::IN_DELETE
                    | libc::IN_MOVED_FROM
                    | libc::IN_MOVED_TO
                    | libc::IN_DELETE_SELF
                    | libc::IN_MOVE_SELF
                    | libc::IN_ONLYDIR,
            )?;
        }
        Ok(watch)
    }
    fn add(&self, path: &std::path::Path, flags: u32) -> io::Result<()> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        if unsafe {
            libc::inotify_add_watch(self.0.as_raw_fd(), path.as_ptr(), flags | libc::IN_MASK_ADD)
        } < 0
        {
            let error = io::Error::last_os_error();
            // An editor can rename/remove an input between discovery and
            // installation. Its nearest surviving directory still wakes us;
            // the session comparison immediately after installation closes
            // the race. Other errors (permissions, watch limits) stay visible.
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        Ok(())
    }
    pub(super) fn wait(&mut self) -> io::Result<()> {
        let mut events = [0; 64 * 1024];
        loop {
            match self.0.read(&mut events) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "file event stream ended",
                    ));
                }
                result => return result.map(|_| ()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hirsel-watch-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    // The same vectors as fsnotify's integration tests: write, rename-over,
    // create absent path, remove, and write through a symlink's target.
    #[test]
    fn events_cover_writes_atomic_saves_missing_sidecars_deletion_and_symlinks() {
        let dir = fixture();
        let target_dir = dir.join("target");
        std::fs::create_dir(&target_dir).unwrap();
        let source = dir.join("main.hir");
        let sidecar = dir.join("main.hir.json");
        let target = target_dir.join("actual.hir");
        std::fs::write(&source, "old").unwrap();
        std::fs::write(&target, "old").unwrap();
        let link = dir.join("link.hir");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        for edit in 0..5 {
            let mut watch = Watch::new([source.clone(), sidecar.clone(), link.clone()]).unwrap();
            match edit {
                0 => std::fs::write(&source, "new").unwrap(),
                1 => {
                    let temp = dir.join("saved");
                    std::fs::write(&temp, "replaced").unwrap();
                    std::fs::rename(temp, &source).unwrap();
                }
                2 => std::fs::write(&sidecar, "{}").unwrap(),
                3 => std::fs::remove_file(&source).unwrap(),
                _ => std::fs::write(&target, "new target").unwrap(),
            }
            // A bounded wait verifies kernel readiness, without latency claims
            // from a test runner's scheduling or a wall-clock threshold.
            let mut poll = libc::pollfd {
                fd: watch.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            assert_eq!(
                unsafe { libc::poll(&mut poll, 1, Duration::from_secs(1).as_millis() as i32) },
                1,
                "edit {edit}"
            );
            watch.wait().unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
