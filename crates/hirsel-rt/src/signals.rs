//! The signal sources behind `io.signals` and `signal.wait` (docs/spec/core-library.md).
//!
//! `signals.host@1` observes the process's SIGINT, SIGTERM and SIGHUP. The handler runs no
//! user code: it marks its kind pending with the next arrival number and writes one byte to a
//! nonblocking self-pipe whose read end is the readiness interest of a parked wait (the
//! self-pipe trick, as libuv and Tokio's signal drivers use). A second SIGINT while one is
//! pending restores the default disposition and re-raises, so the process ends by the signal
//! when the handler returns. Handlers are installed at the run's first projection, for the
//! kinds its grant names, and the dispositions they replaced are restored when the run's
//! [`ProcessSignals`] hold is dropped; arrivals still pending then are discarded. One run of a
//! process holds the host signals at a time.
//!
//! `signals.script@1` delivers its script's arrivals one per idle point of the run, through
//! a pipe of its own, so both worlds park and wake a wait alike.
use hirsel_run_schema::SignalKind;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

/// What a delivery did ([`SignalSource::deliver`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Delivery {
    /// No scripted arrival is left.
    Exhausted,
    /// The arrival is pending.
    Pending,
    /// The arrival takes the default action: the run ends `Signaled`.
    Default(SignalKind),
}

/// A nonblocking close-on-exec pipe: a byte written wakes the read end.
struct Pipe {
    read: OwnedFd,
    write: OwnedFd,
}
impl Pipe {
    fn new() -> std::io::Result<Pipe> {
        let mut fds = [0; 2];
        // SAFETY: fds holds two writable descriptors.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: both descriptors are fresh and owned by nobody else.
        Ok(unsafe {
            Pipe {
                read: OwnedFd::from_raw_fd(fds[0]),
                write: OwnedFd::from_raw_fd(fds[1]),
            }
        })
    }
}
/// Read every byte waiting in a nonblocking pipe.
fn drain(fd: RawFd) {
    let mut buffer = [0u8; 64];
    // SAFETY: buffer is writable; a nonblocking read returns -1 once the pipe is empty.
    while unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) } > 0 {}
}
fn wake(fd: RawFd) {
    // SAFETY: a one-byte write to a nonblocking pipe; a full pipe already wakes its reader.
    unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
}

// The process's host signal state, shared with the handler, which may touch only atomics
// and async-signal-safe calls.
static HELD: AtomicBool = AtomicBool::new(false);
/// Per kind (variant ordinal): 0, or the arrival number of its pending arrival.
static PENDING: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static ARRIVALS: AtomicU64 = AtomicU64::new(0);
static WAKE: AtomicI32 = AtomicI32::new(-1);

fn kind_of(signal: libc::c_int) -> Option<SignalKind> {
    SignalKind::ALL.into_iter().find(|k| k.number() == signal)
}

extern "C" fn handler(signal: libc::c_int) {
    // SAFETY: errno is this thread's; the handler restores it for the code it interrupted.
    let errno = unsafe { *libc::__errno_location() };
    if let Some(kind) = kind_of(signal) {
        let arrival = ARRIVALS.fetch_add(1, Ordering::AcqRel) + 1;
        let pending = PENDING[kind.ordinal() as usize]
            .compare_exchange(0, arrival, Ordering::AcqRel, Ordering::Acquire)
            .is_err();
        if pending && kind == SignalKind::Interrupt {
            // A second Interrupt keeps the default kill: the signal, blocked while its
            // handler runs, is delivered under SIG_DFL when the handler returns.
            // SAFETY: signal(2) and raise(3) are async-signal-safe.
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
        }
        let fd = WAKE.load(Ordering::Acquire);
        if fd >= 0 {
            wake(fd);
        }
    }
    // SAFETY: as above.
    unsafe { *libc::__errno_location() = errno };
}

/// One run's hold on the process's signals, from admission until the run's Io is dropped.
pub struct ProcessSignals {
    observed: u8,
    installed: bool,
    pipe: Pipe,
    /// The dispositions the installed handlers replaced, by kind.
    replaced: Vec<(SignalKind, libc::sigaction)>,
}
impl ProcessSignals {
    /// The hold for `observed` kinds (bits), or `None` while another run holds it.
    pub fn take(observed: u8) -> std::io::Result<Option<Self>> {
        if HELD.swap(true, Ordering::AcqRel) {
            return Ok(None);
        }
        match Pipe::new() {
            Ok(pipe) => Ok(Some(Self {
                observed,
                installed: false,
                pipe,
                replaced: Vec::new(),
            })),
            Err(error) => {
                HELD.store(false, Ordering::Release);
                Err(error)
            }
        }
    }
    /// Install the enqueueing handlers; again, nothing.
    fn install(&mut self) {
        if std::mem::replace(&mut self.installed, true) {
            return;
        }
        for pending in &PENDING {
            pending.store(0, Ordering::Release);
        }
        drain(self.pipe.read.as_raw_fd());
        WAKE.store(self.pipe.write.as_raw_fd(), Ordering::Release);
        for kind in hirsel_run_schema::signal_kinds(self.observed) {
            // SAFETY: zeroed sigaction structures are valid; the handler is async-signal-safe.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler as extern "C" fn(libc::c_int) as usize;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(kind.number(), &action, &mut old) == 0 {
                    self.replaced.push((kind, old));
                }
            }
        }
    }
    fn take_arrival(&self, bits: u8) -> Option<SignalKind> {
        loop {
            let (kind, arrival) = hirsel_run_schema::signal_kinds(bits)
                .filter_map(|kind| {
                    let arrival = PENDING[kind.ordinal() as usize].load(Ordering::Acquire);
                    (arrival != 0).then_some((kind, arrival))
                })
                .min_by_key(|(_, arrival)| *arrival)?;
            if PENDING[kind.ordinal() as usize]
                .compare_exchange(arrival, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(kind);
            }
        }
    }
}
impl Drop for ProcessSignals {
    fn drop(&mut self) {
        for (kind, old) in self.replaced.drain(..).rev() {
            // SAFETY: restores the disposition this hold replaced.
            unsafe { libc::sigaction(kind.number(), &old, std::ptr::null_mut()) };
        }
        WAKE.store(-1, Ordering::Release);
        for pending in &PENDING {
            pending.store(0, Ordering::Release);
        }
        HELD.store(false, Ordering::Release);
    }
}

/// A run's signal source: the host's or its script's.
pub(crate) enum SignalSource {
    Host(ProcessSignals),
    Script {
        observed: u8,
        installed: bool,
        arrivals: std::collections::VecDeque<SignalKind>,
        /// Pending arrivals, earliest first.
        pending: Vec<SignalKind>,
        pipe: Pipe,
    },
}
impl SignalSource {
    pub(crate) fn script(observed: u8, arrivals: &[SignalKind]) -> std::io::Result<Self> {
        Ok(Self::Script {
            observed,
            installed: false,
            arrivals: arrivals.iter().copied().collect(),
            pending: Vec::new(),
            pipe: Pipe::new()?,
        })
    }
    /// The run projected its Signals: observed kinds are pending from now on.
    pub(crate) fn install(&mut self) {
        match self {
            Self::Host(signals) => signals.install(),
            Self::Script { installed, .. } => *installed = true,
        }
    }
    /// Consume the earliest pending arrival among `bits`.
    pub(crate) fn take(&mut self, bits: u8) -> Option<SignalKind> {
        match self {
            Self::Host(signals) => signals.take_arrival(bits),
            Self::Script { pending, .. } => {
                let index = pending.iter().position(|k| bits & k.bit() != 0)?;
                Some(pending.remove(index))
            }
        }
    }
    /// Empty the wake pipe; [`pending`](Self::pending) then says what woke it.
    pub(crate) fn drain(&self) {
        drain(self.interest());
    }
    /// The kinds with a pending arrival, as bits.
    pub(crate) fn pending(&self) -> u8 {
        match self {
            Self::Host(_) => SignalKind::ALL
                .into_iter()
                .filter(|k| PENDING[k.ordinal() as usize].load(Ordering::Acquire) != 0)
                .fold(0, |bits, k| bits | k.bit()),
            Self::Script { pending, .. } => pending.iter().fold(0, |bits, k| bits | k.bit()),
        }
    }
    pub(crate) fn is_host(&self) -> bool {
        matches!(self, Self::Host(_))
    }
    /// The descriptor that is readable while an arrival may be pending.
    pub(crate) fn interest(&self) -> RawFd {
        match self {
            Self::Host(signals) => signals.pipe.read.as_raw_fd(),
            Self::Script { pipe, .. } => pipe.read.as_raw_fd(),
        }
    }
    /// Whether scripted arrivals remain: an outstanding event source at idle points.
    pub(crate) fn scripted(&self) -> bool {
        matches!(self, Self::Script { arrivals, .. } if !arrivals.is_empty())
    }
    /// An idle point: deliver the script's next arrival.
    pub(crate) fn deliver(&mut self) -> Delivery {
        let Self::Script {
            observed,
            installed,
            arrivals,
            pending,
            pipe,
        } = self
        else {
            return Delivery::Exhausted;
        };
        let Some(kind) = arrivals.pop_front() else {
            return Delivery::Exhausted;
        };
        let held = pending.contains(&kind);
        if !*installed || *observed & kind.bit() == 0 || (held && kind == SignalKind::Interrupt) {
            return Delivery::Default(kind);
        }
        if !held {
            pending.push(kind);
        }
        wake(pipe.write.as_raw_fd());
        Delivery::Pending
    }
    /// Block the thread until a host arrival among `bits` is pending (no cooperative
    /// scheduler); a script's wait parks instead.
    pub(crate) fn block(&mut self, bits: u8) -> SignalKind {
        loop {
            // Drain first: an arrival after the scan writes another byte.
            self.drain();
            if let Some(kind) = self.take(bits) {
                return kind;
            }
            let mut fd = libc::pollfd {
                fd: self.interest(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd; an interrupted poll is retried.
            unsafe { libc::poll(&mut fd, 1, -1) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SignalKind::*;

    #[test]
    fn a_script_delivers_pending_arrivals_coalesces_and_keeps_the_default_kill() {
        let observed = Interrupt.bit() | Terminate.bit();
        let script = [Hangup, Terminate, Terminate, Interrupt, Interrupt];
        // Before the projection every arrival takes the default action.
        let mut source = SignalSource::script(observed, &script).unwrap();
        assert!(source.scripted());
        assert_eq!(source.deliver(), Delivery::Default(Hangup));
        let mut source = SignalSource::script(observed, &script).unwrap();
        source.install();
        // An unobserved kind still does.
        assert_eq!(source.deliver(), Delivery::Default(Hangup));
        assert_eq!(source.deliver(), Delivery::Pending);
        // A second Terminate coalesces with the pending one.
        assert_eq!(source.deliver(), Delivery::Pending);
        assert_eq!(source.pending(), Terminate.bit());
        assert_eq!(source.deliver(), Delivery::Pending);
        assert_eq!(source.pending(), Terminate.bit() | Interrupt.bit());
        // The earliest pending arrival among the waited kinds is consumed; others stay.
        assert_eq!(source.take(Interrupt.bit()), Some(Interrupt));
        assert_eq!(source.take(Interrupt.bit()), None);
        assert_eq!(source.take(observed), Some(Terminate));
        assert_eq!(source.pending(), 0);
        // A consumed Interrupt leaves the next one pending, not fatal.
        assert_eq!(source.deliver(), Delivery::Pending);
        assert!(!source.scripted());
        assert_eq!(source.deliver(), Delivery::Exhausted);
        // An unhandled Interrupt makes the next one fatal.
        let mut source = SignalSource::script(observed, &[Interrupt, Interrupt]).unwrap();
        source.install();
        assert_eq!(source.deliver(), Delivery::Pending);
        assert_eq!(source.deliver(), Delivery::Default(Interrupt));
    }

    #[test]
    fn the_host_hold_is_taken_once_enqueues_arrivals_in_order_and_restores_dispositions() {
        let mut source = SignalSource::Host(
            ProcessSignals::take(Terminate.bit() | Hangup.bit())
                .unwrap()
                .expect("no other hold"),
        );
        assert!(ProcessSignals::take(Terminate.bit()).unwrap().is_none());
        source.install();
        // SAFETY: both kinds are handled now; raise delivers to this thread before returning.
        unsafe {
            libc::raise(libc::SIGHUP);
            libc::raise(libc::SIGTERM);
            libc::raise(libc::SIGHUP);
        }
        assert_eq!(source.pending(), Terminate.bit() | Hangup.bit());
        assert_eq!(source.block(Terminate.bit() | Hangup.bit()), Hangup);
        assert_eq!(source.take(Hangup.bit()), None);
        assert_eq!(source.block(Terminate.bit()), Terminate);
        drop(source);
        // SAFETY: reads the current disposition only.
        let restored = unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(libc::SIGTERM, std::ptr::null(), &mut old);
            old.sa_sigaction
        };
        assert_ne!(restored, handler as extern "C" fn(libc::c_int) as usize);
        let again = ProcessSignals::take(Terminate.bit()).unwrap();
        assert!(again.is_some());
    }
}
