//! A thread's creation attributes are stable. Cache their exact pthread
//! bounds rather than rediscovering the main mapping on every admission.
//! The rlimit remains live: a host limit change invalidates the cached size.
use crate::environmental;

thread_local! {
    static BOUNDS: std::cell::Cell<Option<(libc::rlim_t, u64)>> = const { std::cell::Cell::new(None) };
}

pub(super) fn bytes() -> u64 {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: limit is writable and getrlimit initializes it on success.
    let limit = unsafe {
        if libc::getrlimit(libc::RLIMIT_STACK, limit.as_mut_ptr()) != 0 {
            return pthread_bytes();
        }
        limit.assume_init().rlim_cur
    };
    BOUNDS.with(|cache| {
        if let Some((seen, bytes)) = cache.get()
            && seen == limit
        {
            return bytes;
        }
        let bytes = initial_bytes(limit).unwrap_or_else(pthread_bytes);
        cache.set(Some((limit, bytes)));
        bytes
    })
}

/// glibc's initial-thread branch, nptl/pthread_getattr_np.c. Only the two
/// address columns matter; scanf, affinity discovery and pthread attributes
/// carry no additional stack-bound information. Other threads use pthread's
/// recorded creation attributes, which include their actual guard size.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn initial_bytes(limit: libc::rlim_t) -> Option<u64> {
    use std::io::BufRead;
    unsafe extern "C" {
        static __libc_stack_end: *const u8;
    }
    // SAFETY: these queries read this thread's IDs and glibc's immutable
    // initial stack pointer. Never apply the initial mapping to a worker.
    let end = unsafe {
        if libc::syscall(libc::SYS_gettid) != libc::getpid() as libc::c_long {
            return None;
        }
        __libc_stack_end as usize
    };
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page <= 0 || !(page as usize).is_power_of_two() {
        return None;
    }
    let page = page as usize;
    let stack_end = (end & !(page - 1)).checked_add(page)?;
    let here = (&limit as *const libc::rlim_t) as usize;
    let mut maps = std::io::BufReader::new(std::fs::File::open("/proc/self/maps").ok()?);
    let mut line = Vec::new();
    let mut previous = 0;
    while maps.read_until(b'\n', &mut line).ok()? != 0 {
        if let Some((from, to)) = mapping(&line) {
            if from <= end && end < to {
                // A fork from a pthread also has tid == pid. Its actual
                // native stack still has that pthread's creation attributes.
                if !(from <= here && here < to) {
                    return None;
                }
                // glibc uses unsigned subtraction, including RLIM_INFINITY.
                let bytes = (limit as usize).wrapping_sub(to.checked_sub(stack_end)?);
                return Some(((bytes & !(page - 1)).min(stack_end.checked_sub(previous)?)) as u64);
            }
            previous = to;
        }
        line.clear();
    }
    None
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn initial_bytes(_limit: libc::rlim_t) -> Option<u64> {
    None
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn mapping(line: &[u8]) -> Option<(usize, usize)> {
    fn address(line: &[u8], at: &mut usize) -> Option<usize> {
        let start = *at;
        let mut value = 0usize;
        while let Some(&byte) = line.get(*at) {
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => break,
            };
            value = value.checked_mul(16)?.checked_add(usize::from(digit))?;
            *at += 1;
        }
        (*at != start).then_some(value)
    }
    let mut at = 0;
    let from = address(line, &mut at)?;
    if line.get(at) != Some(&b'-') {
        return None;
    }
    at += 1;
    let to = address(line, &mut at)?;
    (from < to && line.get(at) == Some(&b' ')).then_some((from, to))
}

fn pthread_bytes() -> u64 {
    let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    let (mut stack, mut guard) = (0usize, 0usize);
    // SAFETY: attributes is initialized by pthread_getattr_np and destroyed once.
    unsafe {
        if libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()) != 0 {
            environmental("the admission stack size could not be read");
        }
        let mut attributes = attributes.assume_init();
        let status = libc::pthread_attr_getstacksize(&attributes, &mut stack)
            | libc::pthread_attr_getguardsize(&attributes, &mut guard);
        libc::pthread_attr_destroy(&mut attributes);
        if status != 0 {
            environmental("the admission stack bounds could not be read");
        }
    }
    (stack as u64)
        .checked_add(guard as u64)
        .unwrap_or_else(|| environmental("the admission stack bounds overflowed"))
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    fn mapping_addresses_ignore_pathnames_and_reject_truncated_or_overflowing_ranges() {
        assert_eq!(
            super::mapping(b"7ff000-800000 rw-p 0 0:0 0 [stack]\n"),
            Some((0x7ff000, 0x800000))
        );
        for invalid in [
            b"7ff000-".as_slice(),
            b"-800000 ",
            b"800000-7ff000 ",
            b"7ff000-800000",
            b"10000000000000000-20000000000000000 ",
        ] {
            assert_eq!(super::mapping(invalid), None);
        }
    }
    #[test]
    fn cached_native_bounds_preserve_host_selected_worker_stacks_and_guards() {
        for size in [1 << 20, 3 << 20] {
            std::thread::Builder::new()
                .stack_size(size)
                .spawn(|| {
                    let expected = super::pthread_bytes();
                    assert_eq!(super::bytes(), expected);
                    assert_eq!(super::bytes(), expected);
                })
                .unwrap()
                .join()
                .unwrap();
        }
    }
}
