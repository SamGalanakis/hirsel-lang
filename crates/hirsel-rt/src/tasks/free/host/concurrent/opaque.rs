//! An opaque home wait can watch wake descriptors without exposing its driver.
//! Keep every descriptor in Service until the last physical continuation exits;
//! no adapter, Binding, task context, role or callback loan escapes to a thread.
use super::*;
use crate::io::WaitSources;
use crate::readiness::{Readiness, WaitSource};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Instant;

pub(super) struct Wake {
    timer: OwnedFd,
    drivers: Vec<(Readiness, OwnedFd)>,
}
impl Wake {
    fn new() -> Stopped<Self> {
        // nix 0.30.1 src/sys/timerfd.rs:105-113: owned CLOEXEC/NONBLOCK timerfd.
        let fd = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_CLOEXEC | libc::TFD_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(Stop::Refused("opaque wait timer descriptor".into()));
        }
        Ok(Self {
            timer: unsafe { OwnedFd::from_raw_fd(fd) },
            drivers: Vec::new(),
        })
    }

    fn prepare(&mut self, drivers: &[Readiness], stop: RawFd) -> Stopped<Option<Vec<RawFd>>> {
        let mut deadline: Option<Instant> = None;
        let mut fds = vec![stop, self.timer.as_raw_fd()];
        for driver in drivers {
            match driver.wait_source(true).map_err(Stop::refusal)? {
                WaitSource::Ready => return Ok(None),
                WaitSource::Empty => {}
                WaitSource::Driver(fd, end, _) => {
                    if let Some(end) = end {
                        deadline = Some(deadline.map_or(end, |prior| prior.min(end)));
                    }
                    let index = if let Some(index) = self
                        .drivers
                        .iter()
                        .position(|(known, _)| known.same_driver(driver))
                    {
                        index
                    } else {
                        self.drivers.push((driver.clone(), fd));
                        self.drivers.len() - 1
                    };
                    fds.push(self.drivers[index].1.as_raw_fd());
                }
            }
        }
        // A nonblocking drain consumes a preceding one-shot's expiry. Re-arm
        // with the remaining duration, so preparation never extends a deadline.
        let mut expired = 0u64;
        unsafe {
            libc::read(self.timer.as_raw_fd(), (&raw mut expired).cast(), 8);
        }
        let left = deadline.map(|end| end.saturating_duration_since(Instant::now()));
        if left.is_some_and(|left| left.is_zero()) {
            return Ok(None);
        }
        let value = left.map_or(
            libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            |left| libc::timespec {
                tv_sec: left.as_secs().min(libc::time_t::MAX as u64) as libc::time_t,
                tv_nsec: left.subsec_nanos() as libc::c_long,
            },
        );
        // nix 0.30.1 src/sys/timerfd.rs:158-175: relative, one-shot settime;
        // zero disables the timer when the concurrent set has no deadline.
        let spec = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: value,
        };
        if unsafe { libc::timerfd_settime(self.timer.as_raw_fd(), 0, &spec, std::ptr::null_mut()) }
            != 0
        {
            return Err(Stop::Refused("opaque wait timer arm".into()));
        }
        Ok(Some(fds))
    }
}

/// Home only, idle and outside every role/adapter loan. The mutex guards just
/// source preparation; adapter callbacks and the blocking wait hold no lock.
pub(super) unsafe fn wait(
    run: *mut RunShared,
    io: &Service,
    concurrent: WaitSources,
) -> Stopped<bool> {
    let WaitSources::Drivers(drivers) = concurrent else {
        return Ok(true);
    };
    let stop = unsafe { (*run).control.wake_fd() }
        .ok_or_else(|| Stop::Refused("opaque wait stop descriptor".into()))?;
    let fds = {
        let mut wake = io.opaque.lock().unwrap();
        if wake.is_none() {
            *wake = Some(Wake::new()?);
        }
        wake.as_mut().unwrap().prepare(&drivers, stop)?
    };
    let Some(fds) = fds else {
        return Ok(true);
    };
    let home = unsafe { crate::host_waits::wait_opaque_home(run, &fds) }?;
    if let Some(stop) = unsafe { (*run).control.stopped() } {
        return Err(stop);
    }
    let waited = if home {
        true
    } else {
        WaitSources::Drivers(drivers)
            .wait()
            .map_err(Stop::refusal)?
    };
    if let Some(stop) = unsafe { (*run).control.stopped() } {
        return Err(stop);
    }
    Ok(waited)
}
