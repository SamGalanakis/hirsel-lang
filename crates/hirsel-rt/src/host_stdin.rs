//! The interpreter's cooperative stdin (`stdin.bytes@1` under `tasks.cooperative@1`).
//! Admission pins a close-on-exec duplicate of descriptor 0. A pollable stream (pipe,
//! socket, terminal or other character device) is switched to nonblocking mode, and a
//! read that would block records POLLIN interest in the scheduler's Readiness instead of
//! suspending every task. Regular files and block devices cannot block, so they are read
//! directly and are always ready. The duplicate shares the open file description, so
//! dropping the stdin restores the blocking mode it found. A descriptor 0 that is closed
//! at admission reads as end of input, as Rust std's `Stdin` does (`handle_ebadf`).
use crate::readiness::Readiness;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub struct CooperativeStdin {
    fd: Option<OwnedFd>,
    restore_blocking: bool,
    readiness: Readiness,
    /// The run's hold on the process's standard input, released with this pin.
    token: Option<crate::host::ProcessStdin>,
}
impl CooperativeStdin {
    pub fn pin(readiness: Readiness) -> std::io::Result<Self> {
        // SAFETY: F_DUPFD_CLOEXEC returns a fresh descriptor or -1.
        let raw = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        if raw == -1 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EBADF) {
                return Ok(Self {
                    fd: None,
                    restore_blocking: false,
                    readiness,
                    token: None,
                });
            }
            return Err(e);
        }
        // SAFETY: raw is a descriptor this call just created and nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: an all-zero stat is a valid out-parameter for fstat.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fd is live and stat is writable.
        if unsafe { libc::fstat(raw, &mut stat) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let pollable = matches!(
            stat.st_mode & libc::S_IFMT,
            libc::S_IFIFO | libc::S_IFSOCK | libc::S_IFCHR
        );
        let mut restore_blocking = false;
        if pollable {
            let flags = status_flags(raw)?;
            if flags & libc::O_NONBLOCK == 0 {
                set_status_flags(raw, flags | libc::O_NONBLOCK)?;
                restore_blocking = true;
            }
        }
        Ok(Self {
            fd: Some(fd),
            restore_blocking,
            readiness,
            token: None,
        })
    }
    pub(crate) fn storage(&self) -> crate::io_keys::Storage {
        self.fd
            .as_ref()
            .map_or(crate::io_keys::Storage::Shared(0), |fd| {
                crate::io_keys::Storage::of_descriptor(fd.as_raw_fd())
            })
    }
    /// This pin, holding `token` for as long as it lives.
    pub fn holding(mut self, token: crate::host::ProcessStdin) -> Self {
        self.token = Some(token);
        self
    }
    /// `None` when no byte is available yet: POLLIN interest is then recorded under
    /// the Readiness token the caller entered.
    pub(crate) fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<Option<usize>> {
        let Some(fd) = &self.fd else {
            return Ok(Some(0));
        };
        loop {
            // SAFETY: buffer is writable for its length and fd is live.
            let n = unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if n >= 0 {
                return Ok(Some(n as usize));
            }
            let e = std::io::Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::EINTR) => {}
                Some(libc::EAGAIN) => {
                    self.readiness.pending().interest = Some((fd.as_raw_fd(), libc::POLLIN));
                    return Ok(None);
                }
                _ => return Err(e),
            }
        }
    }
}
impl Drop for CooperativeStdin {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.as_ref().filter(|_| self.restore_blocking) {
            // Clear only the flag admission set; a failure leaves nothing else to undo.
            if let Ok(flags) = status_flags(fd.as_raw_fd()) {
                let _ = set_status_flags(fd.as_raw_fd(), flags & !libc::O_NONBLOCK);
            }
        }
    }
}

fn status_flags(fd: libc::c_int) -> std::io::Result<libc::c_int> {
    // SAFETY: F_GETFL takes no argument.
    match unsafe { libc::fcntl(fd, libc::F_GETFL) } {
        -1 => Err(std::io::Error::last_os_error()),
        flags => Ok(flags),
    }
}
fn set_status_flags(fd: libc::c_int, flags: libc::c_int) -> std::io::Result<()> {
    // SAFETY: F_SETFL reads one int.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
