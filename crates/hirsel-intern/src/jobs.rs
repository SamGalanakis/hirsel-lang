//! Deterministic fan-out: one code path for every worker count. Results come
//! back in input order and per-thread work counters are summed into the caller,
//! so nothing a caller observes depends on how many workers ran or who ran what.
use crate::work;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

// Debug x86-64 GDB entry-to-entry rsp measurements on 1,024-level trees:
// parser 7,440 B, resolver 8,424 B, printer 9,496 B, authority 14,128 B,
// lowering 20,256 B per level (including its expression/inner/block helpers).
// Reserve 24 KiB per level, plus a fourfold margin for phase/helper changes.
/// The stack every compiler thread runs on, workers included.
pub const COMPILER_STACK_BYTES: usize = 128 * 1024 * 1024;

/// How many tasks may run at once; never zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Jobs(NonZeroUsize);

impl Jobs {
    /// Run every task on the caller's thread.
    pub const ONE: Jobs = Jobs(NonZeroUsize::MIN);

    /// The host's available parallelism, or one when it cannot be read.
    pub fn available() -> Jobs {
        std::thread::available_parallelism().map_or(Jobs::ONE, Jobs)
    }

    /// `n` workers; zero means one.
    pub fn new(n: usize) -> Jobs {
        Jobs(NonZeroUsize::new(n).unwrap_or(NonZeroUsize::MIN))
    }

    pub fn get(self) -> usize {
        self.0.get()
    }
}

/// Run `work` over `tasks` on up to `jobs` workers and return the results in
/// input order. Tasks start longest first by `cost`, ties by input index.
/// With one worker (or one task) everything runs on the caller's thread; other
/// workers are fresh threads with `COMPILER_STACK_BYTES` stacks whose work
/// counters are added to the caller's. Their `timings` phases need no transfer
/// (the totals are process-wide); the caller's wait for them is `timings`
/// wait, not work of its own phase. After a panic no further task starts,
/// and the panic of the earliest-started panicking task is resumed on the caller.
pub fn map<T: Send, R: Send>(
    jobs: Jobs,
    tasks: Vec<T>,
    cost: impl Fn(&T) -> u64 + Sync,
    work: impl Fn(T) -> R + Sync,
) -> Vec<R> {
    map_with_state(jobs, tasks, cost, || (), |_, task| work(task))
}

/// The same deterministic fan-out, with state constructed once per worker on
/// that worker's thread. Nested work can borrow this state without sharing it
/// with other workers. Like Go's backend workers (gc/compile.go:166-184), the
/// worker owns reusable evaluation storage while task results remain ordered.
/// Empty frontiers construct no state; jobs=1 keeps state on the caller.
pub fn map_with_state<T: Send, R: Send, S>(
    jobs: Jobs,
    tasks: Vec<T>,
    cost: impl Fn(&T) -> u64 + Sync,
    initialize: impl Fn() -> S + Sync,
    work: impl Fn(&mut S, T) -> R + Sync,
) -> Vec<R> {
    if tasks.is_empty() {
        return vec![];
    }
    let costs: Vec<u64> = tasks.iter().map(&cost).collect();
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by(|&a, &b| costs[b].cmp(&costs[a]).then(a.cmp(&b)));
    let workers = jobs.get().min(tasks.len());
    if workers <= 1 {
        let mut state = initialize();
        let mut slots: Vec<Option<T>> = tasks.into_iter().map(Some).collect();
        let mut results: Vec<Option<R>> = (0..slots.len()).map(|_| None).collect();
        for index in order {
            let task = slots[index].take().expect("each task starts once");
            results[index] = Some(work(&mut state, task));
        }
        return results
            .into_iter()
            .map(|r| r.expect("every task ran"))
            .collect();
    }

    let slots: Vec<Mutex<Option<T>>> = tasks.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let results: Vec<Mutex<Option<R>>> = (0..slots.len()).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    let worker = || {
        let mut state = initialize();
        let mut panicked = None;
        while !stopped.load(Ordering::Relaxed) {
            let start = next.fetch_add(1, Ordering::Relaxed);
            let Some(&index) = order.get(start) else {
                break;
            };
            let task = slots[index]
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .expect("each task starts once");
            match catch_unwind(AssertUnwindSafe(|| work(&mut state, task))) {
                Ok(result) => {
                    *results[index]
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(result);
                }
                Err(payload) => {
                    stopped.store(true, Ordering::Relaxed);
                    panicked = Some((start, payload));
                    break;
                }
            }
        }
        (work::Tally::read(), panicked)
    };
    let wait = crate::timings::wait();
    let outcomes: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                std::thread::Builder::new()
                    .name("hirsel-job".into())
                    .stack_size(COMPILER_STACK_BYTES)
                    .spawn_scoped(scope, worker)
                    .expect("job worker creation failed")
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|panic| resume_unwind(panic)))
            .collect()
    });
    drop(wait);
    let mut first = None;
    for (tally, panicked) in outcomes {
        tally.add();
        if let Some((start, payload)) = panicked
            && first.as_ref().is_none_or(|(earliest, _)| start < *earliest)
        {
            first = Some((start, payload));
        }
    }
    if let Some((_, payload)) = first {
        resume_unwind(payload);
    }
    results
        .into_iter()
        .map(|r| {
            r.into_inner()
                .unwrap_or_else(PoisonError::into_inner)
                .expect("every task ran")
        })
        .collect()
}

#[cfg(test)]
mod tests;
