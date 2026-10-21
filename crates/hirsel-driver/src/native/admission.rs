//! Weighted admission layered over the existing deterministic Go-style job map.
//! Adapted from golang.org/x/sync/semaphore/semaphore.go:38-79,163-190:
//! weighted capacity and FIFO waiters. Tickets replace cancellable async nodes;
//! an oversized singleton differs because a compiler must finish a large body.
//! A FIFO ticket prevents a stream of small jobs starving a large body. One
//! oversized task may run alone, including at a zero budget, to ensure progress.
use hirsel_intern::jobs::{self, Jobs};
use std::sync::{Condvar, Mutex};

#[derive(Default)]
struct State {
    next: u64,
    serving: u64,
    active: u64,
    peak: u64,
}
struct Admission {
    budget: u64,
    state: Mutex<State>,
    changed: Condvar,
}
struct Permit<'a> {
    admission: &'a Admission,
    weight: u64,
}
impl Admission {
    fn acquire(&self, weight: u64) -> Permit<'_> {
        let weight = weight.max(1);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = state.next;
        state.next += 1;
        self.changed.notify_all();
        while ticket != state.serving
            || (state.active != 0 && weight > self.budget.saturating_sub(state.active))
        {
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.serving += 1;
        state.active = state.active.saturating_add(weight);
        state.peak = state.peak.max(state.active);
        self.changed.notify_all();
        Permit {
            admission: self,
            weight,
        }
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .admission
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.active -= self.weight;
        self.admission.changed.notify_all();
    }
}

/// Results remain in plan order; the returned peak is admitted estimated bytes,
/// including an oversized singleton, not process RSS or allocator capacity.
pub(super) fn map<T: Send, R: Send>(
    jobs: Jobs,
    budget: u64,
    tasks: Vec<T>,
    weight: impl Fn(&T) -> u64 + Sync,
    work: impl Fn(T) -> R + Sync,
) -> (Vec<R>, u64) {
    let tasks: Vec<_> = tasks
        .into_iter()
        .map(|task| (weight(&task).max(1), task))
        .collect();
    // Do not reserve stacks for workers that cannot simultaneously enter.
    let smallest = tasks.iter().map(|(bytes, _)| *bytes).min().unwrap_or(1);
    let workers = jobs
        .worker_count(tasks.len())
        .min((budget / smallest).max(1).min(usize::MAX as u64) as usize);
    let effective = workers.min(tasks.len());
    hirsel_intern::timings::count_peak(
        hirsel_intern::timings::Count::NativeWorkers,
        effective as u64,
    );
    hirsel_intern::timings::count_peak(
        hirsel_intern::timings::Count::NativeStackBytes,
        if effective > 1 {
            effective as u64 * jobs::COMPILER_STACK_BYTES as u64
        } else {
            0
        },
    );
    let admission = Admission {
        budget,
        state: Mutex::new(State::default()),
        changed: Condvar::new(),
    };
    let results = jobs::map(
        Jobs::new(workers),
        tasks,
        |(weight, _)| *weight,
        |(weight, task)| {
            let _permit = admission.acquire(weight);
            work(task)
        },
    );
    let peak = admission
        .state
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .peak;
    (results, peak)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_tiny_normal_and_oversized_budgets_complete_in_input_order() {
        for jobs in [1, 2, 8] {
            for budget in [0, 1, 10, 100] {
                let weights = vec![1, 2, 70, 3, 4, 5, 0];
                let (output, peak) = map(Jobs::new(jobs), budget, weights.clone(), |w| *w, |w| w);
                assert_eq!(output, weights);
                assert!(
                    peak <= budget.max(70),
                    "jobs={jobs} budget={budget} peak={peak}"
                );
            }
        }
        let (output, peak) = map(Jobs::new(8), 0, Vec::<u64>::new(), |w| *w, |w| w);
        assert!(output.is_empty());
        assert_eq!(peak, 0);
    }
    #[test]
    fn unwinding_releases_admission() {
        let admission = Admission {
            budget: 2,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        };
        let _ = std::panic::catch_unwind(|| {
            let _permit = admission.acquire(7);
            panic!("fixture");
        });
        assert_eq!(admission.state.lock().unwrap().active, 0);
        drop(admission.acquire(1));
    }

    #[test]
    fn a_small_request_cannot_bypass_the_waiting_large_request() {
        let admission = Admission {
            budget: 2,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        };
        let first = admission.acquire(1);
        std::thread::scope(|scope| {
            let (entered, receiver) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel();
            let large_entered = entered.clone();
            let shared = &admission;
            scope.spawn(move || {
                let _permit = shared.acquire(2);
                large_entered.send(2).unwrap();
                released.recv().unwrap();
            });
            let mut state = admission.state.lock().unwrap();
            while state.next < 2 {
                state = admission.changed.wait(state).unwrap();
            }
            drop(state);
            scope.spawn(move || {
                let _permit = shared.acquire(1);
                entered.send(1).unwrap();
            });
            drop(first);
            assert_eq!(receiver.recv().unwrap(), 2);
            assert!(receiver.try_recv().is_err());
            release.send(()).unwrap();
            assert_eq!(receiver.recv().unwrap(), 1);
        });
    }
}
