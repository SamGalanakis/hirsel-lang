//! The free executor's one epoll driver and CAS-coalesced wake descriptor.
//!
//! Ported from mio a88e5e119172e4fb1638926faf65a4fe889e05b1:
//! `src/sys/unix/selector/epoll.rs:14-100,129-145` and
//! `src/sys/unix/waker/eventfd.rs:31-96`. Hirsel adds one try-lock driver
//! token (workers never queue behind an epoll wait), and coalesces wake writes
//! after publishing runnable work. After acknowledging the wake the caller
//! must check runnable work/stop again before a blocking poll. This recheck is
//! what covers a publisher racing the acknowledgment; the flag alone is not
//! a readiness predicate. No worker, task or physical fd identity is recorded.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const WAKE: u64 = u64::MAX;

/// One logical readiness token and the host readiness bits it observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    pub token: u64,
    pub readable: bool,
    pub writable: bool,
    pub closed: bool,
}

/// One driver per live free run; share it among compute workers and the home
/// bridge. The driver owns its descriptors and is Send + Sync without unsafe
/// impls. Host resource ownership is still the adapter's.
pub struct Driver {
    epoll: OwnedFd,
    wake: OwnedFd,
    notified: AtomicBool,
    token: Mutex<()>,
}

impl Driver {
    pub fn new() -> io::Result<Self> {
        // SAFETY: no pointer arguments; owned descriptors returned on success.
        let epoll = owned(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) })?;
        let wake = owned(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) })?;
        let driver = Self {
            epoll,
            wake,
            notified: AtomicBool::new(false),
            token: Mutex::new(()),
        };
        driver.control(
            libc::EPOLL_CTL_ADD,
            driver.wake.as_raw_fd(),
            WAKE,
            true,
            false,
        )?;
        Ok(driver)
    }

    /// Register or rearm a host descriptor. Like the existing readiness
    /// layer, MOD then ADD handles a provider closing and reusing an fd.
    pub fn register(&self, fd: RawFd, token: u64, read: bool, write: bool) -> io::Result<()> {
        if token == WAKE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reserved driver token",
            ));
        }
        match self.control(libc::EPOLL_CTL_MOD, fd, token, read, write) {
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                self.control(libc::EPOLL_CTL_ADD, fd, token, read, write)
            }
            result => result,
        }
    }

    pub fn remove(&self, fd: RawFd) -> io::Result<()> {
        // SAFETY: DEL ignores the event pointer.
        let result = unsafe {
            libc::epoll_ctl(
                self.epoll.as_raw_fd(),
                libc::EPOLL_CTL_DEL,
                fd,
                std::ptr::null_mut(),
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::EBADF)) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn control(&self, op: i32, fd: RawFd, token: u64, read: bool, write: bool) -> io::Result<()> {
        let mut event = libc::epoll_event {
            events: libc::EPOLLET as u32
                | if read {
                    libc::EPOLLIN as u32 | libc::EPOLLRDHUP as u32
                } else {
                    0
                }
                | if write { libc::EPOLLOUT as u32 } else { 0 },
            u64: token,
        };
        // SAFETY: the event is initialised and read by the kernel only in this call.
        if unsafe { libc::epoll_ctl(self.epoll.as_raw_fd(), op, fd, &mut event) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Publish work before calling this. One CAS coalesces all publishers
    /// until the driver acknowledges the eventfd. EINTR does not lose a wake.
    pub fn wake(&self) -> io::Result<()> {
        if self
            .notified
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        let value = 1u64;
        loop {
            // SAFETY: value is readable for the eight bytes eventfd requires.
            let n = unsafe { libc::write(self.wake.as_raw_fd(), (&raw const value).cast(), 8) };
            if n == 8 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                // A full counter is already readable, so its wake is pending.
                io::ErrorKind::WouldBlock => return Ok(()),
                _ => {
                    self.notified.store(false, Ordering::Release);
                    return Err(error);
                }
            }
        }
    }

    /// Claim the driver without waiting for another worker's poll. The token
    /// must be acquired outside an op section and returned before computation.
    pub fn try_claim(&self) -> Option<Turn<'_>> {
        self.token.try_lock().ok().map(|token| Turn {
            driver: self,
            _token: token,
        })
    }
}

/// One worker's driver turn. It carries no compute permit requirement; the
/// worker gives its permit back before a blocking poll.
pub struct Turn<'d> {
    driver: &'d Driver,
    _token: MutexGuard<'d, ()>,
}

impl Turn<'_> {
    /// Drain/coalesce the eventfd, then recheck every runnable/stop predicate
    /// before calling poll with a nonzero timeout.
    pub fn acknowledge(&self) -> io::Result<()> {
        let mut value = 0u64;
        loop {
            // SAFETY: value is writable for eight bytes.
            let n = unsafe { libc::read(self.driver.wake.as_raw_fd(), (&raw mut value).cast(), 8) };
            if n == 8 {
                break;
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => break,
                _ => return Err(error),
            }
        }
        self.driver.notified.store(false, Ordering::Release);
        Ok(())
    }

    /// Poll once under the token. `None` waits indefinitely. A wake descriptor
    /// returns promptly with no resource event; the caller acknowledges and
    /// searches work again. EINTR preserves the original deadline.
    pub fn poll(&self, timeout: Option<Duration>, out: &mut Vec<Event>) -> io::Result<()> {
        let deadline = timeout.and_then(|duration| Instant::now().checked_add(duration));
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 128];
        let n = loop {
            let millis = deadline.map_or(-1, |end| {
                let left = end.saturating_duration_since(Instant::now());
                left.as_millis()
                    .saturating_add(u128::from(left.subsec_nanos() % 1_000_000 != 0))
                    .min(i32::MAX as u128) as i32
            });
            // SAFETY: events has the declared, writable capacity.
            let n = unsafe {
                libc::epoll_wait(
                    self.driver.epoll.as_raw_fd(),
                    events.as_mut_ptr(),
                    events.len() as i32,
                    millis,
                )
            };
            if n >= 0 {
                break n as usize;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        };
        for event in &events[..n] {
            if event.u64 == WAKE {
                continue;
            }
            out.push(Event {
                token: event.u64,
                readable: event.events & libc::EPOLLIN as u32 != 0,
                writable: event.events & libc::EPOLLOUT as u32 != 0,
                closed: event.events & (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32
                    != 0,
            });
        }
        Ok(())
    }
}

fn owned(fd: RawFd) -> io::Result<OwnedFd> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: fd is newly created and ownership transfers exactly once.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Arc;

    #[test]
    fn one_driver_token_and_epoll_deliver_logical_tokens() {
        let driver = Driver::new().unwrap();
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        driver
            .register(reader.as_raw_fd(), 41, true, false)
            .unwrap();
        let turn = driver.try_claim().unwrap();
        assert!(driver.try_claim().is_none());
        writer.write_all(b"x").unwrap();
        let mut events = Vec::new();
        turn.poll(Some(Duration::from_secs(2)), &mut events)
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.token == 41 && event.readable)
        );
        drop(turn);
        assert!(driver.try_claim().is_some());
        driver.remove(reader.as_raw_fd()).unwrap();
    }

    #[test]
    fn simultaneous_wakes_coalesce_and_acknowledgment_rearms_the_descriptor() {
        let driver = Arc::new(Driver::new().unwrap());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let driver = driver.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        driver.wake().unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let turn = driver.try_claim().unwrap();
        let mut count = 0u64;
        assert_eq!(
            unsafe { libc::read(driver.wake.as_raw_fd(), (&raw mut count).cast(), 8) },
            8
        );
        assert_eq!(count, 1, "one wake syscall per acknowledgment interval");
        turn.acknowledge().unwrap();
        driver.wake().unwrap();
        let mut events = Vec::new();
        turn.poll(Some(Duration::from_secs(2)), &mut events)
            .unwrap();
        assert!(events.is_empty(), "wake is not a host observation");
        assert_eq!(
            unsafe { libc::read(driver.wake.as_raw_fd(), (&raw mut count).cast(), 8) },
            8
        );
        assert_eq!(count, 1);
    }
}

/// Whether ordinary configured launches admit production Free execution in
/// this runtime. Qualification builds change only the admission constant.
pub fn admission_enabled() -> bool {
    crate::free_launch::ENABLED
}
