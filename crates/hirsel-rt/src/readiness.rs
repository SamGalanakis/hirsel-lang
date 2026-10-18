//! Host readiness for the interpreter's cooperative scheduler. A host operation that
//! would block (a socket, a resolver wakeup, stdin) records its descriptor interest under
//! the scheduler's opaque wait token; the scheduler polls one token at suspension points
//! and, with no runnable task, waits on every recorded interest at once. A timer wait
//! is a deadline with no descriptor interest.
use crate::io::AdapterError;
use crate::net::{NetError, SocketAddr};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

#[derive(Clone, Default)]
pub struct Readiness(Rc<RefCell<ReadyState>>);
#[derive(Default)]
struct ReadyState {
    current: u64,
    pending: BTreeMap<u64, Pending>,
}
/// One parked operation's interest plus the progress a retried network
/// operation keeps across readiness waits (CooperativeNet).
#[derive(Default)]
pub(crate) struct Pending {
    pub(crate) interest: Option<(libc::c_int, libc::c_short)>,
    pub(crate) deadline: Option<Instant>,
    pub(crate) resolver: Option<(Receiver<Result<Vec<SocketAddr>, NetError>>, UnixStream)>,
    pub(crate) addresses: Option<VecDeque<SocketAddr>>,
    pub(crate) connecting: Option<OwnedFd>,
    pub(crate) last_error: Option<NetError>,
    pub(crate) offset: usize,
    pub(crate) committed: bool,
}
impl Readiness {
    pub(crate) fn pending(&self) -> std::cell::RefMut<'_, Pending> {
        std::cell::RefMut::map(self.0.borrow_mut(), |state| {
            state.pending.entry(state.current).or_default()
        })
    }

    /// Select the opaque operation token before asking RealIo to execute a request.
    pub fn enter(&self, token: u64) {
        self.0.borrow_mut().current = token;
    }
    pub fn finish(&self, token: u64) {
        self.0.borrow_mut().pending.remove(&token);
    }
    /// After a packet is sent or a write is accepted, the operation must complete
    /// with its own outcome rather than report an effect-free cancellation.
    pub fn cancellable(&self, token: u64) -> bool {
        self.0
            .borrow()
            .pending
            .get(&token)
            .is_none_or(|p| !p.committed)
    }
    pub fn ready(&self, token: u64) -> Result<bool, AdapterError> {
        let state = self.0.borrow();
        let Some(pending) = state.pending.get(&token) else {
            return Ok(true);
        };
        if pending.deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(true);
        }
        let Some((fd, events)) = pending.interest else {
            // A timer is ready only at its deadline.
            return Ok(pending.deadline.is_none());
        };
        let mut row = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        loop {
            // SAFETY: row contains a live descriptor retained by the provider or Pending.
            let n = unsafe { libc::poll(&mut row, 1, 0) };
            if n >= 0 {
                return Ok(n != 0);
            }
            if errno() != libc::EINTR {
                return Err(AdapterError::Unsupported("host readiness poll".into()));
            }
        }
    }
    /// A timer wait for CLOCK_MONOTONIC `deadline` nanoseconds under the entered token:
    /// true once it has passed (the token's interest is then finished), otherwise the
    /// deadline is recorded as the token's interest and the caller parks.
    pub(crate) fn timer(&self, deadline: u64) -> Result<bool, AdapterError> {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: now is writable; CLOCK_MONOTONIC is the clock std::time::Instant reads.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 {
            return Err(AdapterError::Unsupported("host monotonic clock".into()));
        }
        let ticks = (now.tv_sec as u64) * 1_000_000_000 + now.tv_nsec as u64;
        let token = self.0.borrow().current;
        if ticks >= deadline {
            self.finish(token);
            return Ok(true);
        }
        let left = std::time::Duration::from_nanos(deadline - ticks);
        self.pending().deadline = Some(Instant::now() + left);
        Ok(false)
    }
    /// Called only with no runnable task. A false result means this provider has
    /// no external readiness source (e.g. the remaining waits are scripted).
    pub fn wait(&self) -> Result<bool, AdapterError> {
        let state = self.0.borrow();
        let mut rows: Vec<_> = state
            .pending
            .values()
            .filter_map(|p| {
                p.interest.map(|(fd, events)| libc::pollfd {
                    fd,
                    events,
                    revents: 0,
                })
            })
            .collect();
        // No descriptor and no timer: nothing external can make progress. With only
        // timers left, the poll below sleeps until the earliest is due.
        if rows.is_empty() && state.pending.values().all(|p| p.deadline.is_none()) {
            return Ok(false);
        }
        loop {
            let deadline = state.pending.values().filter_map(|p| p.deadline).min();
            let timeout = deadline.map_or(-1, |d| {
                let left = d.saturating_duration_since(Instant::now());
                left.as_millis()
                    .saturating_add(u128::from(!left.is_zero()))
                    .min(libc::c_int::MAX as u128) as libc::c_int
            });
            // SAFETY: all rows borrow descriptors whose owners outlive this poll.
            if unsafe { libc::poll(rows.as_mut_ptr(), rows.len() as libc::nfds_t, timeout) } >= 0 {
                return Ok(true);
            }
            if errno() != libc::EINTR {
                return Err(AdapterError::Unsupported("host readiness poll".into()));
            }
        }
    }
}

fn errno() -> libc::c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The deterministic identity of cooperative waits, kept alike by live recording (`RealIo`)
/// and replay (`SimIo`): the request that first blocks under a token gets the run's next
/// creation ordinal, and the failed polls since it parked or was last ready are counted.
/// Tapes carry `blocked` completions and `ready <ordinal> <polls>` events from it (DET-1).
#[derive(Clone, Debug, Default)]
pub(crate) struct WaitLog {
    current: u64,
    next: u64,
    rows: BTreeMap<u64, WaitRow>,
}
#[derive(Clone, Debug)]
pub(crate) struct WaitRow {
    pub(crate) ordinal: u64,
    pub(crate) polls: u64,
    pub(crate) committed: bool,
}
impl WaitLog {
    pub(crate) fn enter(&mut self, token: u64) {
        self.current = token;
    }
    pub(crate) fn finish(&mut self, token: u64) {
        self.rows.remove(&token);
    }
    pub(crate) fn current(&self) -> u64 {
        self.current
    }
    /// The entered request blocked: its row, created with the next ordinal the first time.
    pub(crate) fn park(&mut self, committed: bool) -> &mut WaitRow {
        let next = &mut self.next;
        let row = self.rows.entry(self.current).or_insert_with(|| {
            *next += 1;
            WaitRow {
                ordinal: *next - 1,
                polls: 0,
                committed,
            }
        });
        row.committed = committed;
        row
    }
    pub(crate) fn rows_of(&self, token: u64) -> Option<&WaitRow> {
        self.rows.get(&token)
    }
    pub(crate) fn row(&mut self, token: u64) -> Option<&mut WaitRow> {
        self.rows.get_mut(&token)
    }
    pub(crate) fn rows(&self) -> impl Iterator<Item = &WaitRow> {
        self.rows.values()
    }
}
