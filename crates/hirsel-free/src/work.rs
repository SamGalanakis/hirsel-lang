//! Ready work shared by free-mode workers. Suspended, started fibers use the
//! same queues as new tasks: the identifier is published only after the fiber
//! saves its stack. Foreign-affine tasks use their owner's inbox instead.
//!
//! Worker policy follows Tokio `multi_thread/worker.rs:1090-1155,1168-1205`
//! and its injection queue: local LIFO, local FIFO, width-aware batched
//! injection refill, then steal-half from another worker. Every
//! [`GLOBAL_INTERVAL`]th dispatch serves the injection queue first, so remote
//! wakes are dispatched within a bounded number of dispatch opportunities
//! even while local queues never empty. A full local queue injects its newer
//! half. A retiring worker drains its FIFO and LIFO; no task identity changes.

use crate::queue::{self, CAPACITY, Local, Steal};
use crate::schedule::Lifo;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// B1's queue headroom counts. Collection is optional; the uninstrumented path
/// reads no clock and does no counter atomic operation.
#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    queue_ops: AtomicU64,
    steals: AtomicU64,
    stolen: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            queue_ops: AtomicU64::new(0),
            steals: AtomicU64::new(0),
            stolen: AtomicU64::new(0),
        }
    }
}

impl Metrics {
    /// Raw counts let the runtime combine queue work with its semantic op
    /// count. B1's queue_ops_per_op must not count scheduling attempts as ops.
    pub fn counts(&self) -> (u64, u64, u64) {
        (
            self.queue_ops.load(Ordering::Relaxed),
            self.steals.load(Ordering::Relaxed),
            self.stolen.load(Ordering::Relaxed),
        )
    }

    pub fn values(&self, ops: u64) -> Vec<(&'static str, f64)> {
        let (queue_ops, steals, _) = self.counts();
        let mut out = Vec::new();
        if ops != 0 {
            out.push(("queue_ops_per_op", queue_ops as f64 / ops as f64));
        }
        let seconds = self.started.elapsed().as_secs_f64();
        if seconds > 0.0 {
            out.push(("steals_per_s", steals as f64 / seconds));
        }
        out
    }
}

/// Tokio's initial `global_queue_interval` (`worker.rs:1090-1101`). A bound on
/// dispatch opportunities, not on time spent in user code; tuning it is a
/// physical choice and never changes semantic order.
pub const GLOBAL_INTERVAL: u32 = 61;

/// Where a dispatch came from. Only a LIFO-slot dispatch continues the
/// immediate chain's operation budget; any other source starts a fresh one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Lifo,
    Local,
    Inject,
    Steal,
}

/// The shared part of the ready scheduler. It owns no running fiber.
pub struct Ready<T> {
    inject: Mutex<VecDeque<T>>,
    thieves: Vec<Steal<T>>,
    width: AtomicU32,
    metrics: Option<Arc<Metrics>>,
}

/// One worker's owner-local state; move it to that worker's idle fiber once.
/// It is Send for Send identifiers, but not Sync. The worker must give up its
/// compute permit when an empty search parks it.
pub struct Worker<T> {
    index: usize,
    local: Local<T>,
    lifo: Lifo<T>,
    victim: usize,
    /// Dispatch opportunities since the last forced injection check.
    tick: u32,
}

pub fn workers<T>(width: u32, metrics: Option<Arc<Metrics>>) -> (Arc<Ready<T>>, Vec<Worker<T>>) {
    assert!(width != 0);
    let mut thieves = Vec::new();
    let workers = (0..width as usize)
        .map(|index| {
            let (local, steal) = queue::local();
            thieves.push(steal);
            Worker {
                index,
                local,
                lifo: Lifo::default(),
                victim: index + 1,
                tick: 0,
            }
        })
        .collect();
    (
        Arc::new(Ready {
            inject: Mutex::new(VecDeque::new()),
            thieves,
            width: AtomicU32::new(width),
            metrics,
        }),
        workers,
    )
}

impl<T> Ready<T> {
    fn count(&self, count: usize) {
        if let Some(metrics) = &self.metrics {
            metrics.queue_ops.fetch_add(count as u64, Ordering::Relaxed);
        }
    }

    /// Publish a remote wake or a batch spilled from a local queue. The caller
    /// wakes an idle driver *after* this returns, even if it saw no sleeper
    /// before publication. The driver eventfd coalesces those wakes.
    pub fn inject(&self, tasks: impl IntoIterator<Item = T>) {
        let mut queue = self.inject.lock().unwrap();
        let before = queue.len();
        queue.extend(tasks);
        self.count(queue.len() - before);
    }

    /// Identifiers waiting in the injection queue.
    pub fn injected(&self) -> usize {
        self.inject.lock().unwrap().len()
    }

    pub fn width(&self) -> u32 {
        self.width.load(Ordering::Acquire)
    }

    /// The live compute width, which sizes each injection refill's fair share.
    /// Retirement is explicit: a retiring owner calls [`Worker::drain`]. Thieves
    /// keep searching every existing queue, so no queued fiber waits for a grant.
    pub fn set_width(&self, width: u32) {
        assert!(width != 0 && width as usize <= self.thieves.len());
        self.width.store(width, Ordering::Release);
    }
}

impl<T> Worker<T> {
    /// Prepare ring storage on the idle owner, before scheduler roles can push.
    pub fn prepare(&mut self) {
        self.local.prepare();
    }

    /// An immediate wake uses the LIFO slot; a voluntary yield goes to FIFO.
    pub fn push(&mut self, shared: &Ready<T>, task: T, yielded: bool) {
        shared.count(1);
        let fifo = if yielded {
            Some(task)
        } else {
            self.lifo.push(task)
        };
        if let Some(task) = fifo {
            let overflow = self.local.push(task);
            if !overflow.is_empty() {
                shared.inject(overflow);
            }
        }
    }

    /// Expose every owner-local identifier during shrink or shutdown.
    pub fn drain(&mut self, shared: &Ready<T>) {
        if let Some(task) = self.lifo.to_fifo() {
            shared.inject([task]);
        }
        shared.inject(self.local.drain());
    }

    /// A bounded work search. `None` carries no claim of CONC-8 quiescence:
    /// another worker may still run, save a fiber, publish, or hold a steal
    /// reservation. Termination uses the executor's activity snapshot.
    pub fn next(&mut self, shared: &Ready<T>) -> Option<(T, Source)> {
        self.tick += 1;
        if self.tick == GLOBAL_INTERVAL {
            self.tick = 0;
            // Tokio next_remote_task: one injected task ahead of local work.
            let task = shared.inject.lock().unwrap().pop_front();
            if let Some(task) = task {
                shared.count(1);
                // A Hirsel segment need not return to this search, so the
                // private LIFO slot is never left occupied: thieves can
                // take its continuation from the FIFO.
                self.spill_lifo(shared);
                return Some((task, Source::Inject));
            }
        }
        if let Some(task) = self.lifo.pop() {
            shared.count(1);
            return Some((task, Source::Lifo));
        }
        self.spill_lifo(shared);
        if let Some(task) = self.local.pop() {
            shared.count(1);
            return Some((task, Source::Local));
        }
        if let Some(task) = self.refill(shared) {
            return Some((task, Source::Inject));
        }
        for _ in 0..shared.thieves.len() {
            let victim = self.victim % shared.thieves.len();
            self.victim = self.victim.wrapping_add(1);
            if victim == self.index {
                continue;
            }
            let mut batch = shared.thieves[victim].steal_half();
            if batch.is_empty() {
                continue;
            }
            if let Some(metrics) = &shared.metrics {
                metrics.steals.fetch_add(1, Ordering::Relaxed);
                metrics
                    .stolen
                    .fetch_add(batch.len() as u64, Ordering::Relaxed);
            }
            shared.count(batch.len());
            // As in Tokio steal_into, run the newest stolen member directly;
            // the older members remain available to other thieves.
            let next = batch.pop();
            for task in batch {
                assert!(self.local.push(task).is_empty());
            }
            return next.map(|task| (task, Source::Steal));
        }
        None
    }

    fn spill_lifo(&mut self, shared: &Ready<T>) {
        if let Some(task) = self.lifo.to_fifo() {
            let overflow = self.local.push(task);
            if !overflow.is_empty() {
                shared.inject(overflow);
            }
        }
    }

    /// Take this worker's fair share of the injection queue: one more than
    /// its width share, bounded by free local slots and half the capacity, so
    /// a refill never overflows back and other workers find the rest
    /// (Tokio `worker.rs:1110-1151`). The first task is returned directly.
    fn refill(&mut self, shared: &Ready<T>) -> Option<T> {
        let (first, rest): (T, Vec<_>) = {
            let mut inject = shared.inject.lock().unwrap();
            let first = inject.pop_front()?;
            if inject.is_empty() {
                // A sole injected task needs no batch.
                (first, Vec::new())
            } else {
                let width = shared.width().max(1) as usize;
                let cap = self.local.remaining().min(CAPACITY / 2);
                let count = ((inject.len() + 1) / width + 1).min(cap).max(1);
                let rest = (count - 1).min(inject.len());
                (first, inject.drain(..rest).collect())
            }
        };
        shared.count(1 + rest.len());
        for task in rest {
            assert!(self.local.push(task).is_empty());
        }
        Some(first)
    }
}
