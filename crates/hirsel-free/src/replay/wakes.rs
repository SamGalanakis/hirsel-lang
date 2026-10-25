//! A replay-pending wait's wake: register on the one dependency it waits for and recheck under
//! the registration lock, while a publication advances the dependency first and then takes
//! exactly the registrations it enabled (Tokio `sync/notify.rs:1138-1197`). `proofs/free`
//! compiles this module against loom through [`crate::sync`].

use crate::sync::{Arc, AtomicU64, Mutex, Ordering};
use std::task::Waker;

/// A replay-pending wait's registration on one dependency. `waiter` names the wait by its task
/// row and that row's generation, so a reused row registers as a new waiter while registering
/// the same wait again replaces its entry; `waker` makes the task runnable without the
/// scheduler.
pub struct Subscriber {
    pub waiter: u64,
    pub waker: Waker,
}

impl std::fmt::Debug for Subscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Subscriber({:#x})", self.waiter)
    }
}

/// Replace `subscriber`'s earlier registration in `waiters`, waiting for `at`.
pub fn register(waiters: &mut Vec<(u64, Subscriber)>, at: u64, subscriber: Subscriber) {
    waiters.retain(|(_, s)| s.waiter != subscriber.waiter);
    waiters.push((at, subscriber));
}

/// Take the registrations `ready` enables, counting every registration examined.
pub fn take_ready(
    waiters: &mut Vec<(u64, Subscriber)>,
    visits: &AtomicU64,
    wakes: &mut Vec<Waker>,
    ready: impl Fn(u64) -> bool,
) {
    if waiters.is_empty() {
        return;
    }
    visits.fetch_add(waiters.len() as u64, Ordering::Relaxed);
    let mut i = 0;
    while i < waiters.len() {
        if ready(waiters[i].0) {
            wakes.push(waiters.swap_remove(i).1.waker);
        } else {
            i += 1;
        }
    }
}

/// An object's replayed version: the dependency cell its gapped sub-events wait on. A
/// sub-event advances it by one, from exactly the version it requires.
#[derive(Debug)]
pub struct Version {
    value: AtomicU64,
    /// Pending waits and the version each waits for.
    waiters: Mutex<Vec<(u64, Subscriber)>>,
}

impl Version {
    pub fn new(version: u64) -> Self {
        Self {
            value: AtomicU64::new(version),
            waiters: Mutex::new(Vec::new()),
        }
    }

    /// Drop every registration: the replay ended.
    pub fn clear(&self) {
        self.waiters.lock().unwrap().clear();
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }

    /// Advance from `from` to `from + 1`, then take the waits that version enables. A wait
    /// registering after the take sees the new version on its recheck under the same lock.
    pub fn advance(&self, from: u64, visits: &AtomicU64, wakes: &mut Vec<Waker>) -> bool {
        if self
            .value
            .compare_exchange(from, from + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        // A wait for a version this one passed diverges when it rechecks, never hangs.
        take_ready(&mut self.waiters.lock().unwrap(), visits, wakes, |at| {
            at <= from + 1
        });
        true
    }
}

/// A pending sub-event's predecessor: ready once its object reaches `at`.
#[derive(Clone, Debug)]
pub struct DependencyToken {
    cell: Arc<Version>,
    at: u64,
}

impl DependencyToken {
    /// The dependency on `cell` reaching version `at`.
    pub fn new(cell: Arc<Version>, at: u64) -> Self {
        Self { cell, at }
    }

    pub fn ready(&self) -> bool {
        self.cell.get() >= self.at
    }
    /// The version the dependency waits for.
    pub fn at(&self) -> u64 {
        self.at
    }

    /// Register `subscriber` for this dependency, then recheck under the registration lock:
    /// `true` means it is ready and nothing stays registered.
    pub fn subscribe(&self, subscriber: Subscriber) -> bool {
        let mut waiters = self.cell.waiters.lock().unwrap();
        if self.ready() {
            waiters.retain(|(_, s)| s.waiter != subscriber.waiter);
            return true;
        }
        register(&mut waiters, self.at, subscriber);
        false
    }

    /// Registrations waiting on this dependency's cell.
    pub fn waiting(&self) -> usize {
        self.cell.waiters.lock().unwrap().len()
    }
}
