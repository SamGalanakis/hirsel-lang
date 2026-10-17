//! Host readiness for the interpreter's cooperative scheduler. A host operation that
//! would block (a socket, a resolver wakeup, stdin) records its descriptor interest under
//! the scheduler's opaque wait token; the scheduler polls one token at suspension points
//! and, with no runnable task, waits on every recorded interest at once.
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
            return Ok(true);
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
        if rows.is_empty() {
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
