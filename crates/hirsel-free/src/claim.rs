//! Issue ownership: claim versus withdraw (design §5 Claim versus withdraw).
//!
//! A host request is issued outside sections, so the gate alone cannot stop an issuer that
//! passed its checks and was descheduled before the adapter call. Each request therefore has a
//! state word, held by its record in the run's table and never on the task's stack:
//!
//! - the issuer runs the CAS `Prepared -> ClaimedForIssue` (in its claimed section, after
//!   `Prepared` is durable and its key predecessors ended); only a claimed request may call the
//!   adapter, and a failed CAS means the request was withdrawn and is abandoned uncalled;
//! - the stop's sweep runs the CAS `Prepared -> Withdrawn` on every unclaimed request (while
//!   no section is open, when recording). Exactly one of the two succeeds;
//! - a claimed request at a terminal cut is **in doubt**, even if its thread has not reached
//!   the syscall: the cutter never waits for a host call;
//! - a completion after the terminal cut is **quarantined**: never delivered, never recorded.
//!   The record owns the request's buffers and is shared by the issuer and the table, so they
//!   outlive the task and the stop and are freed by whichever holder leaves last.
//!
//! The home bridge uses the same records: the home thread claims a posted request by CAS, and
//! closing the bridge sweeps the unclaimed ones.

use crate::section::{Cut, Gate, Section, Stopped};
use crate::sync::{Arc, AtomicU8, AtomicU32, Mutex, Ordering};

/// A request's issue state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Claim {
    Prepared,
    ClaimedForIssue,
    Withdrawn,
    Done,
}

impl Claim {
    fn of(word: u8) -> Claim {
        [
            Claim::Prepared,
            Claim::ClaimedForIssue,
            Claim::Withdrawn,
            Claim::Done,
        ][usize::from(word)]
    }
}

/// A request record: its state word and the buffers its host call needs.
pub struct Request<B> {
    pub id: u64,
    word: AtomicU8,
    buffers: B,
}

impl<B> std::fmt::Debug for Request<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request {} ({:?})", self.id, self.state())
    }
}

impl<B> Request<B> {
    pub fn state(&self) -> Claim {
        Claim::of(self.word.load(Ordering::Acquire))
    }
    fn exchange(&self, from: Claim, to: Claim) -> bool {
        self.word
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    /// `Prepared -> ClaimedForIssue`: whether this caller may call the adapter.
    pub fn claim(&self) -> bool {
        self.exchange(Claim::Prepared, Claim::ClaimedForIssue)
    }
    /// `Prepared -> Withdrawn`: whether this caller withdrew it.
    pub fn withdraw(&self) -> bool {
        self.exchange(Claim::Prepared, Claim::Withdrawn)
    }
    /// `ClaimedForIssue -> Done`, in the request's done section.
    pub fn finish(&self) {
        assert!(
            self.exchange(Claim::ClaimedForIssue, Claim::Done),
            "finish of an unclaimed request"
        );
    }
    pub fn buffers(&self) -> &B {
        &self.buffers
    }
}

/// The requests a sweep found unfinished.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Sweep {
    /// Withdrawn by this sweep: they can never issue.
    pub withdrawn: Vec<u64>,
    /// Claimed: in doubt.
    pub in_doubt: Vec<u64>,
}

/// The run's (or the bridge's) table of unfinished requests.
pub struct Requests<B> {
    table: Mutex<Table<B>>,
}

struct Table<B> {
    live: Vec<Arc<Request<B>>>,
    /// Swept: a request prepared from now on is withdrawn at once.
    closed: bool,
}

impl<B> Default for Requests<B> {
    fn default() -> Self {
        Requests {
            table: Mutex::new(Table {
                live: Vec::new(),
                closed: false,
            }),
        }
    }
}

impl<B> Requests<B> {
    /// A new request in state `Prepared`, or already `Withdrawn` when the table was swept.
    pub fn prepare(&self, id: u64, buffers: B) -> Arc<Request<B>> {
        let mut table = self.table.lock().unwrap();
        let state = if table.closed {
            Claim::Withdrawn
        } else {
            Claim::Prepared
        };
        let request = Arc::new(Request {
            id,
            word: AtomicU8::new(state as u8),
            buffers,
        });
        if !table.closed {
            table.live.push(request.clone());
        }
        request
    }

    /// Drop the table's hold on a finished or abandoned request.
    pub fn retire(&self, id: u64) {
        self.table
            .lock()
            .unwrap()
            .live
            .retain(|request| request.id != id);
    }

    /// Close the table: withdraw every unclaimed request and report the claimed ones.
    pub fn sweep(&self) -> Sweep {
        let mut table = self.table.lock().unwrap();
        table.closed = true;
        let mut sweep = Sweep::default();
        for request in std::mem::take(&mut table.live) {
            if request.withdraw() {
                sweep.withdrawn.push(request.id);
            } else if request.state() == Claim::ClaimedForIssue {
                sweep.in_doubt.push(request.id);
            }
        }
        sweep
    }
}

/// A run's stop arbitration, with the section gate when the run records.
pub struct Arbiter {
    /// 0 while running, else the winning stop's cause + 1.
    stop: AtomicU32,
    gate: Option<Gate>,
}

/// What the stop winner found: the terminal cut (when recording) and the request sweep.
pub struct Terminal<'a> {
    pub cut: Option<Cut<'a>>,
    pub sweep: Sweep,
}

impl Arbiter {
    /// `slots` threads run sections when `recording`; without recording there is no gate.
    pub fn new(recording: bool, slots: usize) -> Arbiter {
        Arbiter {
            stop: AtomicU32::new(0),
            gate: recording.then(|| Gate::new(slots)),
        }
    }

    pub fn gate(&self) -> Option<&Gate> {
        self.gate.as_ref()
    }

    /// The winning stop's cause.
    pub fn stopped(&self) -> Option<u32> {
        self.stop.load(Ordering::Acquire).checked_sub(1)
    }

    /// Open a phase's section on `slot`: `Ok(None)` without recording, `Err` once stopped (a
    /// done phase then quarantines its completion).
    pub fn section(&self, slot: usize) -> Result<Option<Section<'_>>, Stopped> {
        match &self.gate {
            Some(gate) => gate.enter(slot).map(Some),
            None if self.stopped().is_some() => Err(Stopped),
            None => Ok(None),
        }
    }

    /// Stop the run with `cause`: `None` when another stop won. The winner closes the gate
    /// for good (when recording) and sweeps `requests` while no section is open; it never
    /// waits for a host call. The caller holds no open section.
    pub fn stop<'a, B>(&'a self, cause: u32, requests: &[&Requests<B>]) -> Option<Terminal<'a>> {
        self.stop
            .compare_exchange(0, cause + 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let cut = self
            .gate
            .as_ref()
            .map(|gate| gate.stop_cut().expect("the stop winner cuts"));
        let mut sweep = Sweep::default();
        for table in requests {
            let found = table.sweep();
            sweep.withdrawn.extend(found.withdrawn);
            sweep.in_doubt.extend(found.in_doubt);
        }
        Some(Terminal { cut, sweep })
    }
}
