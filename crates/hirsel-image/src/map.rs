//! A read-only file mapping. An image is replaced only by renaming a new file
//! over it (`write_atomically`), never rewritten in place, so a mapping keeps
//! seeing the file it opened.
use std::path::Path;

pub(crate) enum Bytes {
    #[cfg(unix)]
    Mapped {
        address: *const u8,
        len: usize,
    },
    Owned(Vec<u8>),
}

// SAFETY: a mapping is read-only and lives until `drop`; nothing writes through it.
unsafe impl Send for Bytes {}
// SAFETY: as above; shared reads of immutable memory.
unsafe impl Sync for Bytes {}

impl Bytes {
    #[cfg(unix)]
    pub(crate) fn map(path: &Path) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open(path)?;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| std::io::Error::other("image larger than the address space"))?;
        if len == 0 {
            return Ok(Bytes::Owned(vec![]));
        }
        // SAFETY: a fresh private read-only mapping of an open file; the
        // descriptor may close afterwards, the mapping keeps the file.
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
            return Err(std::io::Error::last_os_error());
        }
        Ok(Bytes::Mapped {
            address: address.cast(),
            len,
        })
    }
    #[cfg(not(unix))]
    pub(crate) fn map(path: &Path) -> std::io::Result<Self> {
        Ok(Bytes::Owned(std::fs::read(path)?))
    }
    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            #[cfg(unix)]
            // SAFETY: the mapping covers `len` readable bytes until `drop`.
            Bytes::Mapped { address, len } => unsafe { std::slice::from_raw_parts(*address, *len) },
            Bytes::Owned(bytes) => bytes,
        }
    }
}

impl Drop for Bytes {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Bytes::Mapped { address, len } = *self {
            // SAFETY: unmapping the mapping `map` made; no slice of it outlives `self`.
            unsafe { libc::munmap(address.cast_mut().cast(), len) };
        }
    }
}
