//! OS wait adapter. Predicate ownership lives in parking, not in the syscall.
pub(crate) use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[derive(Default)]
pub(crate) struct Wait {
    _private: (),
}
impl Wait {
    pub(crate) fn wait(&self, word: &AtomicU32, expected: u32) {
        // SAFETY: AtomicU32 has futex alignment and remains live through the
        // call. EAGAIN, EINTR and spurious returns all recheck the predicate.
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
                expected,
                std::ptr::null::<libc::timespec>(),
            );
        }
    }
    #[cfg(test)]
    pub(crate) fn wait_timeout(
        &self,
        word: &AtomicU32,
        expected: u32,
        timeout: std::time::Duration,
    ) {
        let timeout = libc::timespec {
            tv_sec: timeout.as_secs().try_into().unwrap_or(libc::time_t::MAX),
            tv_nsec: timeout.subsec_nanos().into(),
        };
        // SAFETY: the word remains live and aligned; timeout is a valid
        // relative timespec. The caller rechecks its token and deadline.
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
                expected,
                &timeout,
            );
        }
    }
    pub(crate) fn wake(&self, word: &AtomicU32) {
        // SAFETY: the owner retains the word until every notifier has exited.
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
                i32::MAX,
            );
        }
    }
}
pub(crate) fn spin() {
    std::hint::spin_loop();
}
pub(crate) fn fence() {
    std::sync::atomic::fence(Ordering::SeqCst);
}

pub(crate) const SPINS: usize = 64;

#[cfg(test)]
pub(crate) use std::thread::spawn;

#[cfg(test)]
pub(crate) use std::sync::Arc;
#[cfg(test)]
pub(crate) fn model(f: impl FnOnce()) {
    f();
}
