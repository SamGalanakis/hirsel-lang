//! S1's coordinator (design §1c, conditional lane C1): one code path for
//! compile-time evaluation, generation and staged instances at any number of
//! workers, with `--jobs 1` as its degenerate case.
//!
//! Each worker evaluates depth-first on its own stack of frames, exactly as
//! serial S1 does: a request for a key already on the worker's stack is a
//! cycle (CT-4 over that stack's sites), a request at stack length L or more
//! overflows, and a frame composes its requests' `DepthEvidence` in request
//! order. Across workers the coordinator keeps, under one lock, which worker
//! runs each key and which worker each parked worker waits for:
//! - a key nobody runs is claimed and run inline by the requester;
//! - a key another worker runs is waited for, unless that worker already
//!   waits, transitively, for the requester. The would-be wait-for cycle is
//!   detected before parking, and the requester then runs the key inline
//!   itself (a duplicate, deterministic evaluation) instead of parking, so the
//!   pool never deadlocks and every wait chain ends at a running worker;
//! - a parked worker wakes when the key finishes and rereads its memo: a
//!   success is there to share, a failure is recomputed on its own stack.
//!
//! The verdict of every request is therefore that of a serial depth-first
//! evaluation on the requester's own stack, whatever the other workers do:
//! successes are context-free (a success never reached its own stack, so it
//! reaches no stack) and are shared, with their depth decided from evidence;
//! a failure that met a cycle or the depth limit depends on the stack it met
//! it under (requested from inside the same cycle, a serial evaluation meets
//! the requester's frame first; requested at another stack length, it
//! overflows elsewhere), so it must not be stored or shared
//! (`failure_is_own`).
//! Design §1c's protocol reports CT-4 for a cycle in the global wait-for
//! graph; here a cycle through another worker's stack is not reported, it is
//! evaluated on the requester's stack, because the requester's serial
//! evaluation would not have met that worker's frames (lane deviation, in
//! followups.md).
//!
//! Borrowed from salsa: a thread-level wait-for graph checked before
//! blocking (/tmp/ref-salsa/src/runtime.rs:316-334, `depends_on` before
//! `block`; runtime/dependency_graph.rs:49-110, `add_edge`/`block_on`).
//! Differs because salsa reports the cross-thread cycle and recovers by
//! fixpoint, while Hirsel's verdict must be a serial depth-first evaluation's,
//! so the requester runs the key itself. Not storing a failure that met a
//! cycle follows tsrs's rule that a diagnostic must not depend on what the
//! checker visited before (/tmp/ref-tsrs/notes/fix-history-dependent-diagnostics.md).
use super::depth::DepthEvidence;
use hirsel_intern::timings::{self, Count};
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

pub type WorkerId = u32;

/// One frame of a worker's stack: the key it evaluates, the site it was
/// requested at, and its completed requests in order.
#[derive(Debug)]
pub struct Frame<K, S> {
    pub key: K,
    pub site: S,
    requests: Vec<(DepthEvidence<S>, S)>,
    /// The worker owns the key's claim (else it runs a duplicate).
    claimed: bool,
}

/// What `S1Worker::enter` decided.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Entered {
    /// A frame was pushed: evaluate, then `leave`.
    Run,
    /// Another worker finished the key while this one waited: reread the
    /// memo, and request again when it holds no success.
    Settled,
}

/// Counters, summed over the coordinator's life.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoordinatorStats {
    /// Keys claimed by the worker that first requested them.
    pub claims: u64,
    /// Requests that waited for another worker.
    pub parks: u64,
    /// Requests that would have closed a wait-for cycle and ran inline.
    pub duplicates: u64,
    /// Time spent acquiring the coordinator's lock, in nanoseconds.
    pub lock_wait_ns: u64,
    /// Time spent parked, in nanoseconds.
    pub park_ns: u64,
}

#[derive(Default)]
struct State<K> {
    /// The worker running each claimed key.
    running: HashMap<K, WorkerId>,
    /// The worker each parked worker waits for.
    waits: HashMap<WorkerId, WorkerId>,
    next: WorkerId,
}

/// The global claim table and wait-for graph (design §3 `S1Coordinator`).
pub struct S1Coordinator<K> {
    state: Mutex<State<K>>,
    finished: Condvar,
    claims: AtomicU64,
    parks: AtomicU64,
    duplicates: AtomicU64,
    lock_wait_ns: AtomicU64,
    park_ns: AtomicU64,
}

impl<K: Clone + Eq + Hash> Default for S1Coordinator<K> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                running: HashMap::new(),
                waits: HashMap::new(),
                next: 0,
            }),
            finished: Condvar::new(),
            claims: AtomicU64::new(0),
            parks: AtomicU64::new(0),
            duplicates: AtomicU64::new(0),
            lock_wait_ns: AtomicU64::new(0),
            park_ns: AtomicU64::new(0),
        }
    }
}

impl<K: Clone + Eq + Hash> S1Coordinator<K> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// A new worker with an empty stack.
    pub fn worker<S>(self: &Arc<Self>) -> S1Worker<K, S> {
        let id = {
            let mut state = self.lock();
            state.next += 1;
            state.next - 1
        };
        S1Worker {
            coordinator: self.clone(),
            id,
            frames: vec![],
            contextual: false,
        }
    }
    pub fn stats(&self) -> CoordinatorStats {
        CoordinatorStats {
            claims: self.claims.load(Ordering::Relaxed),
            parks: self.parks.load(Ordering::Relaxed),
            duplicates: self.duplicates.load(Ordering::Relaxed),
            lock_wait_ns: self.lock_wait_ns.load(Ordering::Relaxed),
            park_ns: self.park_ns.load(Ordering::Relaxed),
        }
    }
    fn lock(&self) -> MutexGuard<'_, State<K>> {
        let start = Instant::now();
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed = start.elapsed().as_nanos() as u64;
        self.lock_wait_ns.fetch_add(elapsed, Ordering::Relaxed);
        timings::count_by(Count::S1LockWaitNs, elapsed);
        guard
    }
    fn release(&self, key: &K, worker: WorkerId) {
        let mut state = self.lock();
        if state.running.get(key) == Some(&worker) {
            state.running.remove(key);
        }
        drop(state);
        self.finished.notify_all();
    }
}

/// One worker's depth-first stack over a shared coordinator.
pub struct S1Worker<K: Clone + Eq + Hash, S> {
    coordinator: Arc<S1Coordinator<K>>,
    id: WorkerId,
    frames: Vec<Frame<K, S>>,
    /// A cycle or the depth limit failed a request since the outermost
    /// evaluation began.
    contextual: bool,
}

impl<K: Clone + Eq + Hash, S: Copy> S1Worker<K, S> {
    pub fn coordinator(&self) -> &Arc<S1Coordinator<K>> {
        &self.coordinator
    }
    /// The stack length a request made now is made at.
    pub fn len(&self) -> u32 {
        self.frames.len() as u32
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn frames(&self) -> &[Frame<K, S>] {
        &self.frames
    }
    /// The position of `key` on this worker's stack: a request for it is a
    /// cycle through the frames above it.
    pub fn on_stack(&self, key: &K) -> Option<usize> {
        self.frames.iter().position(|f| f.key == *key)
    }
    /// The sites of the cycle closed by requesting the key at stack position
    /// `start` from `site`: each later frame's request site, then `site`.
    pub fn cycle_sites(&self, start: usize, site: S) -> impl Iterator<Item = S> + '_ {
        self.frames[start + 1..]
            .iter()
            .map(|f| f.site)
            .chain([site])
    }
    /// Records that a request failed on a cycle.
    pub fn fail_cycle(&mut self) {
        self.contextual = true;
    }
    /// Whether a failure now leaving a query is the query's own, so it may be
    /// stored or shared: no cycle or overflow failed a request since the
    /// outermost evaluation began. (Conservative: a later failure of the same
    /// outermost evaluation is recomputed too; failures are error paths.)
    pub fn failure_is_own(&self) -> bool {
        !self.contextual
    }
    /// Requests a result with `evidence` at `site` from the current stack
    /// length: `Err` with the overflowing site, as a cache-free serial
    /// evaluation would fail.
    pub fn admit(&mut self, evidence: &DepthEvidence<S>, site: S) -> Result<(), S> {
        let verdict = evidence.request(self.len(), site);
        if verdict.is_err() {
            self.contextual = true;
        }
        verdict
    }
    /// Records a completed request in the requesting frame, in request order.
    pub fn requested(&mut self, evidence: &DepthEvidence<S>, site: S) {
        if let Some(frame) = self.frames.last_mut() {
            frame.requests.push((evidence.clone(), site));
        }
    }
    /// The evidence of the evaluation on top, composed from its requests.
    pub fn composed(&self, limit: u32, bounded: bool) -> DepthEvidence<S> {
        let mut evidence = match self.frames.last() {
            Some(frame) => {
                DepthEvidence::compose(limit, frame.requests.iter().map(|(e, s)| (e, *s)))
            }
            None => DepthEvidence::leaf(limit),
        };
        evidence.bounded = bounded;
        evidence
    }
    /// Claims `key` and pushes its frame, or waits for the worker running it.
    /// The caller has checked `on_stack` and its memo first.
    pub fn enter(&mut self, key: K, site: S) -> Entered {
        if self.frames.is_empty() {
            self.contextual = false;
        }
        let coordinator = self.coordinator.clone();
        let mut state = coordinator.lock();
        let claimed = loop {
            let Some(&owner) = state.running.get(&key) else {
                state.running.insert(key.clone(), self.id);
                coordinator.claims.fetch_add(1, Ordering::Relaxed);
                timings::count(Count::S1Claims);
                break true;
            };
            if owner == self.id {
                // A duplicate below on this stack still runs it.
                break false;
            }
            // Would waiting for `owner` close a wait-for cycle?
            let mut next = owner;
            let deadlock = loop {
                match state.waits.get(&next) {
                    Some(&w) if w == self.id => break true,
                    Some(&w) => next = w,
                    None => break false,
                }
            };
            if deadlock {
                coordinator.duplicates.fetch_add(1, Ordering::Relaxed);
                timings::count(Count::S1Duplicates);
                break false;
            }
            state.waits.insert(self.id, owner);
            coordinator.parks.fetch_add(1, Ordering::Relaxed);
            timings::count(Count::S1Parks);
            let parked = Instant::now();
            state = coordinator
                .finished
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
            let elapsed = parked.elapsed().as_nanos() as u64;
            coordinator.park_ns.fetch_add(elapsed, Ordering::Relaxed);
            timings::count_by(Count::S1ParkNs, elapsed);
            state.waits.remove(&self.id);
            if state.running.get(&key) != Some(&owner) {
                return Entered::Settled;
            }
        };
        drop(state);
        self.frames.push(Frame {
            key,
            site,
            requests: vec![],
            claimed,
        });
        Entered::Run
    }
    /// Pops the top frame, releasing its claim and waking waiters. The caller
    /// has stored a success in its memo first, so a waiter finds it.
    pub fn leave(&mut self) -> Frame<K, S> {
        let frame = self.frames.pop().expect("an S1 frame to leave");
        if frame.claimed {
            self.coordinator.release(&frame.key, self.id);
        }
        frame
    }
}

impl<K: Clone + Eq + Hash, S> Drop for S1Worker<K, S> {
    fn drop(&mut self) {
        for frame in self.frames.drain(..).rev() {
            if frame.claimed {
                self.coordinator.release(&frame.key, self.id);
            }
        }
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
