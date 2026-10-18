//! Read-only host file mappings behind `file.map` (`io_real.rs`). A mapping reserves one
//! anonymous page in front of the file's pages and writes a static `Bytes` block there
//! (extent, immortal header, count and capacity words) whose elements are the mapped bytes,
//! so `mapped.with` lends them in place with no copy, and dups and releases of the view
//! never write.
//!
//! A read past the end of a file that another process (or the program) truncated would
//! raise SIGBUS. One process-wide handler, installed with the first mapping, looks the
//! faulting address up in a fixed table of live mapping ranges; for one of them it maps a
//! zero page over the faulting page and marks the range, so the read sees zeros and the
//! enclosing `mapped.with` returns `Truncated()`. Any other SIGBUS goes to the previously
//! installed disposition.
use crate::alloc::{Extent, HEADER_SIZE, Header, KIND_SHIFT, KIND_STATIC};
use crate::io::IoError;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Live mapping ranges the SIGBUS handler answers for. A slot is free while `start` is 0.
const SLOTS: usize = 4096;
struct Slot {
    start: AtomicUsize,
    end: AtomicUsize,
    hit: AtomicBool,
}
#[allow(clippy::declare_interior_mutable_const)]
const FREE: Slot = Slot {
    start: AtomicUsize::new(0),
    end: AtomicUsize::new(0),
    hit: AtomicBool::new(false),
};
static RANGES: [Slot; SLOTS] = [FREE; SLOTS];
static INSTALL: std::sync::Once = std::sync::Once::new();
static PAGE: AtomicUsize = AtomicUsize::new(0);
/// The SIGBUS disposition in place before ours, written once before ours is installed.
static mut PREVIOUS: std::mem::MaybeUninit<libc::sigaction> = std::mem::MaybeUninit::zeroed();

/// The static block's bytes in front of the elements: its extent, header, count and capacity.
const PREFIX: usize = size_of::<Extent>() + HEADER_SIZE + 16;

fn page() -> usize {
    match PAGE.load(Ordering::Relaxed) {
        0 => {
            // SAFETY: sysconf has no preconditions.
            let size = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
                .ok()
                .filter(|n| n.is_power_of_two() && *n >= PREFIX)
                .unwrap_or_else(|| crate::fault("no usable page size"));
            PAGE.store(size, Ordering::Relaxed);
            size
        }
        size => size,
    }
}

extern "C" fn on_sigbus(
    signal: libc::c_int,
    info: *mut libc::siginfo_t,
    context: *mut libc::c_void,
) {
    // SAFETY: the kernel passes a valid siginfo for an SA_SIGINFO handler.
    let address = unsafe { (*info).si_addr() } as usize;
    let page = PAGE.load(Ordering::Relaxed);
    for slot in &RANGES {
        let start = slot.start.load(Ordering::Acquire);
        if start == 0 || address < start || address >= slot.end.load(Ordering::Acquire) {
            continue;
        }
        // SAFETY: the page lies in a live mapping range this module reserved; replacing it
        // with zeros changes no memory outside it. mmap is async-signal-safe.
        let zero = unsafe {
            libc::mmap(
                (address & !(page - 1)) as *mut libc::c_void,
                page,
                libc::PROT_READ,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if zero != libc::MAP_FAILED {
            slot.hit.store(true, Ordering::Release);
            return;
        }
    }
    // SAFETY: PREVIOUS was written before this handler was installed and never after.
    let previous = unsafe { (*std::ptr::addr_of!(PREVIOUS)).assume_init_ref() };
    let handler = previous.sa_sigaction;
    if handler == libc::SIG_DFL || handler == libc::SIG_IGN {
        // Restore it and return: the faulting access repeats under the default action.
        // SAFETY: sigaction is async-signal-safe and `previous` is a valid action.
        unsafe { libc::sigaction(libc::SIGBUS, previous, std::ptr::null_mut()) };
    } else if previous.sa_flags & libc::SA_SIGINFO != 0 {
        // SAFETY: an SA_SIGINFO handler takes these three arguments.
        let handler: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
            unsafe { std::mem::transmute(handler) };
        handler(signal, info, context);
    } else {
        // SAFETY: a plain handler takes the signal number.
        let handler: extern "C" fn(libc::c_int) = unsafe { std::mem::transmute(handler) };
        handler(signal);
    }
}

fn install() {
    INSTALL.call_once(|| {
        page();
        // SAFETY: a zeroed sigaction is valid; PREVIOUS is written before any handler
        // that reads it can run, and Once orders the write before every later mapping.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_sigbus as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut action.sa_mask);
            let previous = (*std::ptr::addr_of_mut!(PREVIOUS)).as_mut_ptr();
            if libc::sigaction(libc::SIGBUS, &action, previous) != 0 {
                crate::fault("cannot install the mapping SIGBUS handler");
            }
        }
    });
}

/// One live read-only mapping of a regular file's first `length` bytes.
#[derive(Debug)]
pub(crate) struct Mapping {
    base: *mut u8,
    total: usize,
    slot: Option<usize>,
    length: u64,
    file: File,
}
impl Mapping {
    /// Map `file`'s first `length` bytes behind a static Bytes block.
    pub(crate) fn new(file: File, length: u64) -> Result<Self, IoError> {
        install();
        let page = page();
        let span = usize::try_from(length)
            .ok()
            .and_then(|n| n.checked_add(page - 1))
            .map(|n| n & !(page - 1))
            .filter(|n| *n <= isize::MAX as usize - page)
            .ok_or(IoError::Fault(1))?;
        let total = span + page;
        // SAFETY: a fresh anonymous reservation; nothing else refers to it.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                total,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(IoError::Fault(1));
        }
        let base = base.cast::<u8>();
        let mut mapping = Self {
            base,
            total,
            slot: None,
            length,
            file,
        };
        if span > 0 {
            // SAFETY: the file pages replace this reservation's own pages after the first.
            let pages = unsafe {
                libc::mmap(
                    base.add(page).cast(),
                    span,
                    libc::PROT_READ,
                    libc::MAP_SHARED | libc::MAP_FIXED,
                    mapping.file.as_raw_fd(),
                    0,
                )
            };
            if pages == libc::MAP_FAILED {
                return Err(IoError::Fault(1));
            }
            let start = base as usize + page;
            mapping.slot = Some(
                RANGES
                    .iter()
                    .position(|slot| {
                        slot.start
                            .compare_exchange(0, usize::MAX, Ordering::AcqRel, Ordering::Relaxed)
                            .is_ok()
                    })
                    .ok_or(IoError::Fault(1))?,
            );
            let slot = &RANGES[mapping.slot.unwrap()];
            slot.hit.store(false, Ordering::Relaxed);
            slot.end.store(start + span, Ordering::Release);
            slot.start.store(start, Ordering::Release);
        }
        // SAFETY: the prefix lies in the reservation's writable first page.
        unsafe {
            let prefix = base.add(page - PREFIX);
            prefix.cast::<Extent>().write(Extent {
                capacity: 16 + length,
                align: 8,
            });
            prefix
                .add(size_of::<Extent>())
                .cast::<Header>()
                .write(Header {
                    count: crate::COUNT_IMMORTAL,
                    descriptor: KIND_STATIC << KIND_SHIFT,
                });
            let words = prefix.add(size_of::<Extent>() + HEADER_SIZE).cast::<u64>();
            words.write(length);
            words.add(1).write(length);
            if libc::mprotect(base.cast(), page, libc::PROT_READ) != 0 {
                return Err(IoError::Fault(1));
            }
        }
        Ok(mapping)
    }
    /// The address of the static block over the mapped bytes.
    pub(crate) fn block(&self) -> usize {
        self.base as usize + page() - PREFIX + size_of::<Extent>()
    }
    /// Whether a read past the file's end was answered with zeros, or the file is now
    /// shorter than the mapping.
    pub(crate) fn truncated(&self) -> bool {
        self.slot
            .is_some_and(|slot| RANGES[slot].hit.load(Ordering::Acquire))
            || self
                .file
                .metadata()
                .map_or(true, |meta| meta.len() < self.length)
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        if let Some(slot) = self.slot {
            RANGES[slot].start.store(0, Ordering::Release);
        }
        // SAFETY: the whole reservation belongs to this mapping and no view outlives it.
        unsafe { libc::munmap(self.base.cast(), self.total) };
    }
}

/// The view of a mapping's static block: a slice over its whole sequence.
pub(crate) fn view(block: usize, length: u64) -> crate::views::Slice {
    crate::views::Slice {
        owner: block as *mut Header,
        start: crate::alloc::sequence_slice_start(1),
        length,
    }
}

/// A copy of the `length` mapped bytes after the static block at `block`, which a live
/// mapping holds (`crate::io::MapView::contents`).
pub(crate) fn contents(block: usize, length: u64) -> Vec<u8> {
    // SAFETY: the caller reads a view of a mapping that is still open; the elements follow
    // the block's header and count and capacity words.
    unsafe {
        std::slice::from_raw_parts(
            (block as *const u8).add(HEADER_SIZE + 16),
            usize::try_from(length).unwrap_or_else(|_| crate::fault("mapping length overflow")),
        )
    }
    .to_vec()
}
