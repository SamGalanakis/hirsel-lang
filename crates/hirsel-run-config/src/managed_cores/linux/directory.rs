//! Pin every visible cgroup level while reading its controls. Relative opens
//! follow Rust std sys/fs/unix/dir.rs:118-133 at 92ffebeceaa0.
//! FileExt::read_at uses pread, sys/fd/unix.rs:191-202. Interrupted reads
//! retry as read_to_string did; common control files need one stack buffer.
use crate::{RunRefusal, invalid};
use std::{
    ffi::{CStr, CString},
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};

pub(super) struct Directory {
    path: PathBuf,
    file: File,
}

impl Directory {
    pub(super) fn walk(
        root: &Path,
        leaf: &Path,
        opened: Option<File>,
    ) -> Result<Vec<Self>, RunRefusal> {
        let file = match opened {
            Some(file) => file,
            None => OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
                .open(root)
                .map_err(|e| failure(root, e))?,
        };
        let mut levels = vec![Self {
            path: root.into(),
            file,
        }];
        for name in leaf
            .strip_prefix(root)
            .expect("membership below its mount")
            .components()
        {
            let parent = levels.last().unwrap();
            let path = parent.path.join(name);
            let name = CString::new(name.as_os_str().as_bytes()).map_err(|_| {
                failure(
                    &path,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "file name contained an unexpected NUL byte",
                    ),
                )
            })?;
            let file = relative(&parent.file, &name, libc::O_PATH | libc::O_DIRECTORY)
                .map_err(|e| failure(&path, e))?;
            levels.push(Self { path, file });
        }
        Ok(levels)
    }

    pub(super) fn read(&self, name: &CStr) -> Result<Option<String>, RunRefusal> {
        let failed = |e| {
            failure(
                &self.path.join(std::ffi::OsStr::from_bytes(name.to_bytes())),
                e,
            )
        };
        let file = match relative(&self.file, name, 0) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(failed(e)),
        };
        let mut buffer = [0u8; 8192];
        let size = read_at(&file, &mut buffer, 0).map_err(failed)?;
        let mut bytes = buffer[..size].to_vec();
        // No new size cap: unusually long cpulists and fixture files retain
        // the old reader's complete contents and diagnostic behaviour.
        if size == buffer.len() {
            loop {
                let size = read_at(&file, &mut buffer, bytes.len() as u64).map_err(failed)?;
                bytes.extend_from_slice(&buffer[..size]);
                if size < buffer.len() {
                    break;
                }
            }
        }
        String::from_utf8(bytes).map(Some).map_err(|_| {
            failed(io::Error::new(
                io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            ))
        })
    }
}

fn relative(parent: &File, name: &CStr, flags: i32) -> io::Result<File> {
    // SAFETY: parent remains live, name is terminated, and success returns
    // one new descriptor whose sole owner becomes File.
    loop {
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | flags,
            )
        };
        if fd >= 0 {
            return Ok(unsafe { File::from_raw_fd(fd) });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
fn failure(path: &Path, error: io::Error) -> RunRefusal {
    invalid("host limits", &format!("{}: {error}", path.display()))
}

fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    loop {
        match file.read_at(buffer, offset) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}
