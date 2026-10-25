//! Owned bounded host jobs (r6 §7, S5). A potentially blocking built-in file
//! call does not run on its compute worker: the owner reserves a job credit
//! before claim, parks its saved continuation, and one bounded pool thread
//! performs the call. The credit, request lease and result stay owned by the
//! job until the call actually returns, including after its run stopped.
//!
//! OTP classifies blocking work onto dirty I/O schedulers
//! (erts/doc/references/erl_nif.md:431-445,477-489); Trio's limiter token is
//! released only when the thread really returns, never on abandonment
//! (src/trio/_threads.py:303-326); completions are published as one batch of
//! wake tokens the home service merges, as Seastar batches cross-core
//! completions (src/core/reactor.cc:3753-3779). Hirsel adds the claim word,
//! a FIFO credit queue (no credit is ever taken by a task that still waits on
//! a key) and single-winner delivery against terminal quarantine.
use crate::io::{AdapterError, Completion};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Physical job capacity of one run, as OTP's default dirty I/O scheduler count.
pub(crate) const LIMIT: usize = 10;

pub(crate) type Output = (Result<Completion, AdapterError>, Option<u64>);
type Call = Box<dyn FnOnce() -> Output + Send>;
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

pub(crate) struct Jobs {
    inner: Arc<Inner>,
}

struct Inner {
    limit: usize,
    pool: Mutex<Pool>,
    work: Condvar,
    wake: OnceLock<Waker>,
    stats: Stats,
    /// The deterministic driver runs each queued job itself: no thread.
    #[cfg(test)]
    manual: bool,
}

#[derive(Default)]
struct Pool {
    /// Credits reserved, granted or held by running jobs.
    held: usize,
    threads: usize,
    idle: usize,
    queue: VecDeque<Job>,
    /// Owners waiting for capacity, in reservation order.
    waiters: VecDeque<u64>,
    /// Credits a release handed to a waiter that has not yet taken it.
    granted: Vec<u64>,
    /// Wait tokens whose credit or result became available.
    ready: Vec<u64>,
    closed: bool,
}

/// Counters for tests and occupancy reports; never semantic.
#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) submitted: AtomicU64,
    pub(crate) returned: AtomicU64,
    pub(crate) delivered: AtomicU64,
    pub(crate) quarantined: AtomicU64,
    pub(crate) reclaimed: AtomicU64,
    pub(crate) queued_credits: AtomicU64,
    pub(crate) peak: AtomicU64,
}

struct Job {
    token: u64,
    slot: Arc<Slot>,
    call: Call,
    credit: Credit,
}

/// One unit of physical job capacity. Dropping it returns the capacity, or
/// hands it directly to the oldest waiting owner.
pub(crate) struct Credit {
    inner: Arc<Inner>,
}

impl Drop for Credit {
    fn drop(&mut self) {
        let mut pool = self.inner.pool.lock().unwrap();
        if let Some(token) = pool.waiters.pop_front() {
            pool.granted.push(token);
            pool.ready.push(token);
            drop(pool);
            self.inner.wake();
        } else {
            pool.held -= 1;
        }
    }
}

/// A queued or granted credit reservation, owned by its request. Dropping an
/// untaken reservation (withdrawal, stop teardown) returns its place.
pub(crate) struct Reservation {
    inner: Arc<Inner>,
    token: u64,
    credit: Option<Credit>,
}

impl Reservation {
    /// Whether the credit is granted. Never blocks.
    pub(crate) fn granted(&mut self) -> bool {
        if self.credit.is_none() {
            let mut pool = self.inner.pool.lock().unwrap();
            if let Some(at) = pool.granted.iter().position(|&t| t == self.token) {
                pool.granted.swap_remove(at);
                drop(pool);
                self.credit = Some(Credit {
                    inner: self.inner.clone(),
                });
            }
        }
        self.credit.is_some()
    }

    /// The granted credit, moved into the job that will use it.
    pub(crate) fn take(mut self) -> Option<Credit> {
        self.granted();
        self.credit.take()
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.credit.is_some() {
            // The credit field's own drop returns the capacity.
            return;
        }
        let mut pool = self.inner.pool.lock().unwrap();
        if let Some(at) = pool.waiters.iter().position(|&t| t == self.token) {
            pool.waiters.remove(at);
        } else if let Some(at) = pool.granted.iter().position(|&t| t == self.token) {
            pool.granted.swap_remove(at);
            drop(pool);
            // Pass the granted capacity on as an ordinary release.
            drop(Credit {
                inner: self.inner.clone(),
            });
        }
    }
}

const RUNNING: u8 = 0;
const RETURNED: u8 = 1;
const DELIVERED: u8 = 2;
/// Abandoned while running: the job quarantines its own late result.
const ABANDONED: u8 = 3;
const QUARANTINED: u8 = 4;

/// A request-owned result. Exactly one of owner delivery or terminal
/// quarantine wins; a quarantined result is retained until the last holder
/// (job or request owner) reclaims it, and is never recorded or delivered.
pub(crate) struct Slot {
    word: AtomicU8,
    result: Mutex<Option<Output>>,
    stats: Arc<Inner>,
}

impl Slot {
    pub(crate) fn returned(&self) -> bool {
        self.word.load(Ordering::Acquire) == RETURNED
    }

    /// The owner takes the returned result: the single delivery winner.
    pub(crate) fn deliver(&self) -> Option<Output> {
        self.word
            .compare_exchange(RETURNED, DELIVERED, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        self.stats.stats.delivered.fetch_add(1, Ordering::Relaxed);
        self.result.lock().unwrap().take()
    }

    /// The owner will never deliver (stop teardown or abandoned frame).
    /// Returns whether this call quarantined a result or the pending call.
    pub(crate) fn abandon(&self) -> bool {
        let quarantined = self
            .word
            .compare_exchange(RETURNED, QUARANTINED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if quarantined {
            self.stats.stats.quarantined.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        self.word
            .compare_exchange(RUNNING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn finish(&self, output: Output) {
        *self.result.lock().unwrap() = Some(output);
        if self
            .word
            .compare_exchange(RUNNING, RETURNED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            assert!(
                self.word
                    .compare_exchange(ABANDONED, QUARANTINED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok(),
                "one job return"
            );
            self.stats.stats.quarantined.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.result.get_mut().unwrap().is_some() {
            self.stats.stats.reclaimed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The owner's job state inside its Pending request. Dropping it before
/// delivery (teardown after a stop) is the only quarantine path.
pub(crate) struct Owned {
    pub(crate) reservation: Option<Reservation>,
    pub(crate) slot: Option<Arc<Slot>>,
    /// The destination task (S1): its slot is not reused while the request
    /// owns a job, so a late completion never reaches a later occupant.
    pub(crate) destination: crate::tasks::registry::TaskLease,
}

impl Owned {
    /// # Safety
    /// The running owner holds the role.
    pub(crate) unsafe fn new(ctx: *mut crate::TaskCtx) -> Owned {
        let slot = unsafe { (*ctx).task_id };
        Owned {
            reservation: None,
            slot: None,
            destination: crate::tasks::role::with_role(ctx, |s| s.lease(slot)),
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.abandon();
        }
    }
}

impl Inner {
    fn wake(&self) {
        if let Some(wake) = self.wake.get() {
            wake();
        }
    }
}

impl Default for Jobs {
    fn default() -> Self {
        Jobs::new(LIMIT)
    }
}

impl Jobs {
    pub(crate) fn new(limit: usize) -> Jobs {
        assert!(limit > 0, "a job pool needs capacity");
        Jobs {
            inner: Arc::new(Inner {
                limit,
                pool: Default::default(),
                work: Condvar::new(),
                wake: OnceLock::new(),
                stats: Stats::default(),
                #[cfg(test)]
                manual: false,
            }),
        }
    }

    /// Installed once, before the first reservation; it must not take a role.
    pub(crate) fn set_waker(&self, wake: Waker) {
        let _ = self.inner.wake.set(wake);
    }

    pub(crate) fn stats(&self) -> &Stats {
        &self.inner.stats
    }

    /// Reserve one credit for `token`, before claim. A full pool queues the
    /// reservation FIFO; its grant arrives as a ready `token`.
    pub(crate) fn reserve(&self, token: u64) -> Reservation {
        let mut pool = self.inner.pool.lock().unwrap();
        let credit = if pool.held < self.inner.limit && pool.waiters.is_empty() {
            pool.held += 1;
            Some(Credit {
                inner: self.inner.clone(),
            })
        } else {
            pool.waiters.push_back(token);
            self.inner
                .stats
                .queued_credits
                .fetch_add(1, Ordering::Relaxed);
            None
        };
        Reservation {
            inner: self.inner.clone(),
            token,
            credit,
        }
    }

    /// Run `call` on a pool thread. It already owns its credit, so this never
    /// waits; the wake token is published only after the result is stored.
    pub(crate) fn submit(
        &self,
        credit: Credit,
        token: u64,
        call: impl FnOnce() -> Output + Send + 'static,
    ) -> Arc<Slot> {
        assert!(Arc::ptr_eq(&credit.inner, &self.inner));
        let slot = Arc::new(Slot {
            word: AtomicU8::new(RUNNING),
            result: Mutex::new(None),
            stats: self.inner.clone(),
        });
        let stats = &self.inner.stats;
        stats.submitted.fetch_add(1, Ordering::Relaxed);
        let mut pool = self.inner.pool.lock().unwrap();
        stats.peak.fetch_max(pool.held as u64, Ordering::Relaxed);
        pool.queue.push_back(Job {
            token,
            slot: slot.clone(),
            call: Box::new(call),
            credit,
        });
        #[cfg(test)]
        if self.inner.manual {
            return slot;
        }
        if pool.idle >= pool.queue.len() {
            self.inner.work.notify_one();
        } else {
            // Credits bound queued plus running jobs, hence the thread count.
            pool.threads += 1;
            let inner = self.inner.clone();
            std::thread::Builder::new()
                .name("hirsel-host-job".into())
                .spawn(move || serve(inner))
                .expect("a host job thread");
        }
        slot
    }

    /// Wait tokens made ready since the last drain, in publication order.
    pub(crate) fn drain(&self) -> Vec<u64> {
        std::mem::take(&mut self.inner.pool.lock().unwrap().ready)
    }

    /// Whether some credit is held: an outstanding event source for quiescence.
    pub(crate) fn busy(&self) -> bool {
        let pool = self.inner.pool.lock().unwrap();
        pool.held != 0 || !pool.ready.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.inner.pool.lock().unwrap().held
    }

    /// An observer that outlives the run's service, as a running job does.
    #[cfg(test)]
    pub(crate) fn probe(&self) -> Probe {
        Probe(self.inner.clone())
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Probe(Arc<Inner>);

#[cfg(test)]
impl Probe {
    pub(crate) fn held(&self) -> usize {
        self.0.pool.lock().unwrap().held
    }
    pub(crate) fn stats(&self) -> &Stats {
        &self.0.stats
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        // Idle threads exit; running jobs keep their capacity until return.
        self.inner.pool.lock().unwrap().closed = true;
        self.inner.work.notify_all();
    }
}

fn serve(inner: Arc<Inner>) {
    let mut pool = inner.pool.lock().unwrap();
    loop {
        if let Some(job) = pool.queue.pop_front() {
            drop(pool);
            run(&inner, job);
            pool = inner.pool.lock().unwrap();
            continue;
        }
        if pool.closed {
            pool.threads -= 1;
            return;
        }
        pool.idle += 1;
        pool = inner.work.wait(pool).unwrap();
        pool.idle -= 1;
    }
}

/// One job's actual call, then its result, its ready token and its credit.
fn run(inner: &Inner, job: Job) {
    let Job {
        token,
        slot,
        call,
        credit,
    } = job;
    let output = call();
    inner.stats.returned.fetch_add(1, Ordering::Relaxed);
    slot.finish(output);
    drop(slot);
    inner.pool.lock().unwrap().ready.push(token);
    // Physical capacity ends only after the actual call returned.
    drop(credit);
    inner.wake();
}

/// The deterministic runtime driver's adapter: a manual pool whose queued
/// jobs the driver's pool threads take in `serve`'s order and run when the
/// driver chooses, so returns interleave as seeded transitions.
#[cfg(test)]
pub(crate) struct Taken(Job);

#[cfg(test)]
impl Jobs {
    pub(crate) fn manual(limit: usize) -> Jobs {
        let mut jobs = Jobs::new(limit);
        Arc::get_mut(&mut jobs.inner).unwrap().manual = true;
        jobs
    }

    /// A pool thread takes the oldest queued job.
    pub(crate) fn take(&self) -> Option<Taken> {
        assert!(self.inner.manual);
        self.inner.pool.lock().unwrap().queue.pop_front().map(Taken)
    }

    pub(crate) fn queued(&self) -> usize {
        self.inner.pool.lock().unwrap().queue.len()
    }
}

#[cfg(test)]
impl Taken {
    pub(crate) fn token(&self) -> u64 {
        self.0.token
    }

    /// The call returns on its pool thread.
    pub(crate) fn run(self) {
        let inner = self.0.credit.inner.clone();
        run(&inner, self.0);
    }
}

#[cfg(test)]
mod tests;
