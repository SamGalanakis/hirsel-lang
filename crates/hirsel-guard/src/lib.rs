//! The startup guard every hirsel binary and test harness installs.
//!
//! `install!()` makes [`Guarded`] the binary's global allocator. Its first
//! allocation, before `main` and before any work, sets a hard `RLIMIT_DATA` of
//! `HIRSEL_MEMORY_LIMIT` (bytes, or a count of binary `K`, `M`, `G` or `T`;
//! default 4 GiB), never above a limit the process inherited. An allocation the
//! limit refuses ends the process at once with exit status [`EXIT_STATUS`] and
//! `hirsel: memory limit 4G reached (HIRSEL_MEMORY_LIMIT); ...` on stderr,
//! instead of growing until the host thrashes. Child processes, native
//! executables included, inherit the limit.
//!
//! The data limit counts private writable mappings: heap, anonymous `mmap`
//! and thread stacks, but not file mappings or reserved `PROT_NONE` ranges, so
//! a compiler that reserves address space it never touches stays inside it,
//! where an address-space limit would not.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, Ordering};

/// The limit without `HIRSEL_MEMORY_LIMIT`: 4 GiB.
pub const DEFAULT_LIMIT: u64 = 4 << 30;
/// Exit status of a process that reached its limit (sysexits' `EX_OSERR`).
pub const EXIT_STATUS: i32 = 71;
/// Worker threads or processes without `HIRSEL_JOBS`.
pub const DEFAULT_JOBS: usize = 2;
/// Exit status of a malformed `HIRSEL_MEMORY_LIMIT`.
const USAGE_STATUS: i32 = 2;

/// Make [`Guarded`] the global allocator: `install!()` over the system
/// allocator, or `install!(Counting)` over a unit-struct allocator a test
/// harness defines.
#[macro_export]
macro_rules! install {
    () => {
        $crate::install!(::std::alloc::System: ::std::alloc::System);
    };
    ($inner:ident) => {
        $crate::install!($inner: $inner);
    };
    ($ty:ty: $inner:expr) => {
        #[global_allocator]
        static HIRSEL_GUARD: $crate::Guarded<$ty> = $crate::Guarded($inner);
    };
}

/// An allocator that sets the limit before its first allocation and ends the
/// process when the limit refuses one.
pub struct Guarded<A = System>(pub A);

const UNSET: u8 = 0;
const SETTING: u8 = 1;
const SET: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(UNSET);

unsafe impl<A: GlobalAlloc> GlobalAlloc for Guarded<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        limit_once();
        let block = unsafe { self.0.alloc(layout) };
        if block.is_null() {
            exhausted(layout.size());
        }
        block
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        limit_once();
        let block = unsafe { self.0.alloc_zeroed(layout) };
        if block.is_null() {
            exhausted(layout.size());
        }
        block
    }
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { self.0.realloc(block, layout, size) };
        if moved.is_null() {
            exhausted(size);
        }
        moved
    }
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        unsafe { self.0.dealloc(block, layout) }
    }
}

fn limit_once() {
    if STATE.load(Ordering::Acquire) == SET
        || STATE
            .compare_exchange(UNSET, SETTING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    // getenv and the rlimit calls do not allocate, so they are safe here.
    let text = unsafe { libc::getenv(c"HIRSEL_MEMORY_LIMIT".as_ptr()) };
    let requested = if text.is_null() {
        DEFAULT_LIMIT
    } else {
        let text = unsafe { std::ffi::CStr::from_ptr(text) }.to_bytes();
        parse_limit(text).unwrap_or_else(|| malformed(text))
    };
    let mut current = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut current) };
    let limit = requested.min(current.rlim_max);
    let limited = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    unsafe { libc::setrlimit(libc::RLIMIT_DATA, &limited) };
    STATE.store(SET, Ordering::Release);
}

/// `HIRSEL_MEMORY_LIMIT`'s bytes: positive decimal digits with an optional
/// binary `K`, `M`, `G` or `T`, the syntax systemd's `MemoryMax` and
/// `tools/capped` read too.
pub fn parse_limit(text: &[u8]) -> Option<u64> {
    let (digits, unit) = match text.split_last()? {
        (b'K', digits) => (digits, 1u64 << 10),
        (b'M', digits) => (digits, 1 << 20),
        (b'G', digits) => (digits, 1 << 30),
        (b'T', digits) => (digits, 1 << 40),
        _ => (text, 1),
    };
    if digits.is_empty() || digits[0] == b'0' {
        return None;
    }
    let mut count: u64 = 0;
    for digit in digits {
        if !digit.is_ascii_digit() {
            return None;
        }
        count = count
            .checked_mul(10)?
            .checked_add(u64::from(digit - b'0'))?;
    }
    count.checked_mul(unit)
}

/// The worker count for a runner's threads or processes: `HIRSEL_JOBS`, which
/// the Buck2 test launcher sets to the action's CPU reservation, else
/// [`DEFAULT_JOBS`]. A malformed or zero value is an error naming the variable.
pub fn jobs() -> usize {
    match std::env::var("HIRSEL_JOBS") {
        Err(std::env::VarError::NotPresent) => DEFAULT_JOBS,
        Ok(text) => match text.parse::<usize>() {
            Ok(jobs) if jobs > 0 => jobs,
            _ => {
                eprintln!("hirsel: HIRSEL_JOBS={text} is not a positive count of workers");
                std::process::exit(USAGE_STATUS);
            }
        },
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("hirsel: HIRSEL_JOBS is not a positive count of workers");
            std::process::exit(USAGE_STATUS);
        }
    }
}

/// Text built on the stack: the reports run where allocation has failed.
struct Line {
    bytes: [u8; 320],
    len: usize,
}
impl Line {
    fn new() -> Self {
        Line {
            bytes: [0; 320],
            len: 0,
        }
    }
    fn text(&mut self, text: &[u8]) -> &mut Self {
        let n = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&text[..n]);
        self.len += n;
        self
    }
    fn number(&mut self, mut n: u64) -> &mut Self {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        self.text(&digits[at..])
    }
    /// A size in the limit's own syntax: its largest exact binary unit.
    fn size(&mut self, bytes: u64) -> &mut Self {
        for (unit, suffix) in [
            (1u64 << 40, b"T"),
            (1 << 30, b"G"),
            (1 << 20, b"M"),
            (1 << 10, b"K"),
        ] {
            if bytes >= unit && bytes % unit == 0 {
                return self.number(bytes / unit).text(suffix);
            }
        }
        self.number(bytes)
    }
    fn exit(&self, status: i32) -> ! {
        let mut written = 0;
        while written < self.len {
            let n = unsafe {
                libc::write(
                    2,
                    self.bytes[written..self.len].as_ptr().cast(),
                    self.len - written,
                )
            };
            if n <= 0 {
                break;
            }
            written += n as usize;
        }
        unsafe { libc::_exit(status) }
    }
}

/// A thread that could not start: when its `stack` would not fit under the
/// data limit beside what the process already holds (pthread reports that as
/// `EAGAIN`, like a process-count limit), the process ends as the allocator
/// ends it, naming the limit; any other failure panics with its error.
#[cold]
pub fn spawn_failed(stack: usize, error: std::io::Error) -> ! {
    let used = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status.lines().find(|l| l.starts_with("VmData:"))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        })
        .map(|kib| kib << 10);
    let mut current = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut current) };
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::OutOfMemory
    ) && used.is_some_and(|used| used.saturating_add(stack as u64) > current.rlim_cur)
    {
        exhausted(stack);
    }
    panic!("thread creation failed: {error}");
}

#[cold]
fn exhausted(size: usize) -> ! {
    let mut current = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut current) };
    let mut line = Line::new();
    line.text(b"hirsel: memory limit ");
    if current.rlim_cur == libc::RLIM_INFINITY {
        line.text(b"unlimited");
    } else {
        line.size(current.rlim_cur);
    }
    line.text(b" reached (HIRSEL_MEMORY_LIMIT); allocating ")
        .number(size as u64)
        .text(
            b" more bytes failed. Under Buck2 the limit is the test's measured reservation \
              (tools/buck2/test_sizes.bzl); report the program that grew rather than raising it.\n",
        )
        .exit(EXIT_STATUS)
}

#[cold]
fn malformed(text: &[u8]) -> ! {
    Line::new()
        .text(b"hirsel: HIRSEL_MEMORY_LIMIT=")
        .text(text)
        .text(b" is not a positive byte count with an optional K, M, G or T, such as 4G\n")
        .exit(USAGE_STATUS)
}

#[cfg(test)]
install!();

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Output};

    /// The child half of the exhaustion tests: allocates past its limit.
    const CHILD: &str = "HIRSEL_GUARD_TEST_CHILD";

    fn child(test: &str, env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([test, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    #[test]
    fn limits_parse_as_bytes_or_binary_units() {
        assert_eq!(parse_limit(b"4G"), Some(4 << 30));
        assert_eq!(parse_limit(b"512M"), Some(512 << 20));
        assert_eq!(parse_limit(b"2883584K"), Some(2883584 << 10));
        assert_eq!(parse_limit(b"1T"), Some(1 << 40));
        assert_eq!(parse_limit(b"4096"), Some(4096));
        for bad in [
            &b""[..],
            b"0",
            b"04G",
            b"4GB",
            b"4g",
            b"G",
            b"-1",
            b"4 G",
            b"99999999999T",
        ] {
            assert_eq!(parse_limit(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn sizes_print_in_their_largest_exact_unit() {
        for (bytes, text) in [
            (4u64 << 30, "4G"),
            (5632 << 20, "5632M"),
            (1536 << 10, "1536K"),
            (1000, "1000"),
            (0, "0"),
        ] {
            let mut line = Line::new();
            line.size(bytes);
            assert_eq!(&line.bytes[..line.len], text.as_bytes());
        }
    }

    #[test]
    fn exhaustion_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        let mut blocks = vec![];
        for _ in 0..64 {
            let block = vec![1u8; 4 << 20];
            blocks.push(block);
        }
        println!("allocated {} blocks", blocks.len());
    }

    #[test]
    fn an_allocation_past_the_limit_ends_the_process_with_the_message() {
        let output = child(
            "tests::exhaustion_child",
            &[("HIRSEL_MEMORY_LIMIT", "128M")],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(EXIT_STATUS), "{stderr}");
        assert!(
            stderr.contains("hirsel: memory limit 128M reached (HIRSEL_MEMORY_LIMIT); allocating "),
            "{stderr}"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("allocated"));
    }

    #[test]
    fn the_same_allocations_fit_under_a_larger_limit() {
        let output = child(
            "tests::exhaustion_child",
            &[("HIRSEL_MEMORY_LIMIT", "768M")],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("allocated 64 blocks"));
    }

    #[test]
    fn a_malformed_limit_is_refused_before_any_work() {
        let output = child("tests::exhaustion_child", &[("HIRSEL_MEMORY_LIMIT", "4GB")]);
        assert_eq!(output.status.code(), Some(USAGE_STATUS));
        assert!(String::from_utf8_lossy(&output.stderr).contains("HIRSEL_MEMORY_LIMIT=4GB is not"));
    }

    #[test]
    fn jobs_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        println!("jobs {}", jobs());
    }

    #[test]
    fn workers_come_from_hirsel_jobs_and_default_to_two() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "tests::jobs_child",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env_remove("HIRSEL_JOBS");
        let output = command.output().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("jobs 2\n"));
        let output = child("tests::jobs_child", &[("HIRSEL_JOBS", "3")]);
        assert!(String::from_utf8_lossy(&output.stdout).contains("jobs 3\n"));
        for bad in ["0", "-1", "all"] {
            let output = child("tests::jobs_child", &[("HIRSEL_JOBS", bad)]);
            assert_eq!(output.status.code(), Some(USAGE_STATUS), "{bad}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("not a positive count"));
        }
    }

    #[test]
    fn a_thread_stack_past_the_limit_ends_the_process_with_the_message() {
        let output = child("tests::stack_child", &[("HIRSEL_MEMORY_LIMIT", "64M")]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(EXIT_STATUS), "{stderr}");
        assert!(
            stderr.contains(
                "hirsel: memory limit 64M reached (HIRSEL_MEMORY_LIMIT); allocating 134217728 more"
            ),
            "{stderr}"
        );
        let output = child("tests::stack_child", &[("HIRSEL_MEMORY_LIMIT", "768M")]);
        assert!(String::from_utf8_lossy(&output.stdout).contains("stack accepted"));
    }

    #[test]
    fn stack_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .stack_size(128 << 20)
            .spawn(|| ());
        match spawned {
            Ok(thread) => {
                thread.join().unwrap();
                println!("stack accepted");
            }
            Err(error) => spawn_failed(128 << 20, error),
        }
    }

    #[test]
    fn child_processes_inherit_the_limit_and_cannot_raise_it() {
        let output = child(
            "tests::inheritance_child",
            &[("HIRSEL_MEMORY_LIMIT", "96M")],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("soft 98304 hard 98304\n"), "{stdout}");
    }

    #[test]
    fn inheritance_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        // A child asking for more keeps the inherited limit.
        let shell = Command::new("/bin/sh")
            .args([
                "-c",
                "ulimit -d 1048576 2>/dev/null; echo soft $(ulimit -S -d) hard $(ulimit -H -d)",
            ])
            .env("HIRSEL_MEMORY_LIMIT", "8G")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&shell.stdout));
    }
}
