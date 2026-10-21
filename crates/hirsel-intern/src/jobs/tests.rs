use super::*;
use proptest::prelude::*;
use std::sync::atomic::AtomicU64;

#[test]
fn worker_count_respects_stack_reservations_and_empty_frontiers() {
    for requested in [1, 2, 8, 32] {
        let jobs = Jobs::new(requested);
        assert_eq!(jobs.worker_count_with_cap(0, 0), 0);
        assert_eq!(jobs.worker_count_with_cap(64, 0), 1);
        assert_eq!(jobs.worker_count_with_cap(64, 512 << 20), requested.min(2));
        assert_eq!(jobs.worker_count_with_cap(64, 4 << 30), requested.min(16));
        assert_eq!(jobs.worker_count_with_cap(1, 4 << 30), 1);
    }
}

/// A task that records the order it started in and does work proportional to its cost.
fn run(jobs: Jobs, costs: &[u64]) -> (Vec<(usize, u64)>, Vec<usize>) {
    let started = Mutex::new(Vec::new());
    let tasks: Vec<(usize, u64)> = costs.iter().copied().enumerate().collect();
    let results = map(
        jobs,
        tasks,
        |&(_, cost)| cost,
        |(index, cost)| {
            started.lock().unwrap().push(index);
            work::count_by(work::Counter::CacheValidationRows, cost);
            (index, cost.wrapping_mul(31))
        },
    );
    (results, started.into_inner().unwrap())
}

proptest! {
    #[test]
    fn results_come_back_in_input_order(costs in prop::collection::vec(0u64..50, 0..64)) {
        for jobs in [1, 2, 8] {
            let (results, _) = run(Jobs::new(jobs), &costs);
            let expected: Vec<_> = costs.iter().map(|c| c.wrapping_mul(31)).enumerate().collect();
            prop_assert_eq!(results, expected);
        }
    }

    #[test]
    fn one_worker_starts_longest_first_ties_by_index(costs in prop::collection::vec(0u64..4, 0..32)) {
        let (_, started) = run(Jobs::ONE, &costs);
        let mut expected: Vec<usize> = (0..costs.len()).collect();
        expected.sort_by_key(|&i| (std::cmp::Reverse(costs[i]), i));
        prop_assert_eq!(started, expected);
    }

    #[test]
    fn worker_counters_are_summed_into_the_caller(costs in prop::collection::vec(0u64..1000, 1..64)) {
        for jobs in [1, 3, 8] {
            let before = work::read(work::Counter::CacheValidationRows);
            run(Jobs::new(jobs), &costs);
            let added = work::read(work::Counter::CacheValidationRows) - before;
            prop_assert_eq!(added, costs.iter().sum::<u64>());
        }
    }

    #[test]
    fn a_panicking_task_resurfaces_once(len in 1usize..40, bad in any::<prop::sample::Index>(), jobs in 1usize..9) {
        let bad = bad.index(len);
        let finished = AtomicU64::new(0);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            map(Jobs::new(jobs), (0..len).collect(), |_| 1, |i| {
                if i == bad {
                    std::panic::panic_any(format!("task {i}"));
                }
                finished.fetch_add(1, Ordering::Relaxed);
                i
            })
        }));
        let payload = outcome.expect_err("the panic reaches the caller");
        prop_assert_eq!(payload.downcast_ref::<String>(), Some(&format!("task {bad}")));
        prop_assert!(finished.load(Ordering::Relaxed) < len as u64);
    }
}

#[test]
fn the_earliest_started_panic_wins_when_several_run() {
    // Both tasks panic. The costlier starts first, so its panic wins whether or
    // not the other worker started before the stop.
    for _ in 0..20 {
        let payload = catch_unwind(AssertUnwindSafe(|| {
            map(
                Jobs::new(2),
                vec![1u64, 2],
                |&cost| cost,
                |cost| -> u8 { std::panic::panic_any(cost) },
            )
        }))
        .expect_err("a panic reaches the caller");
        assert_eq!(payload.downcast_ref::<u64>(), Some(&2));
    }
}

/// A 16 MiB frame overflows a default thread stack but fits a compiler stack.
#[inline(never)]
fn deep_frame() -> u8 {
    let deep = [7u8; 16 * 1024 * 1024];
    std::hint::black_box(&deep)[7]
}

#[test]
fn one_job_runs_on_the_callers_thread_and_more_use_compiler_stacks() {
    let caller = std::thread::current().id();
    let ran = map(
        Jobs::ONE,
        vec![(); 3],
        |_| 0,
        |()| std::thread::current().id(),
    );
    assert!(ran.iter().all(|&id| id == caller));
    let names = map(
        Jobs::new(2),
        vec![(); 4],
        |_| 0,
        |()| {
            (
                std::thread::current().name().map(str::to_owned),
                deep_frame(),
            )
        },
    );
    assert!(
        names
            .iter()
            .all(|(name, _)| name.as_deref() == Some("hirsel-job"))
    );
}

#[test]
fn state_is_initialized_once_per_worker_and_reused_only_on_that_thread() {
    use std::cell::Cell;
    // Rc/Cell deliberately make this state !Send and !Sync: construction
    // happens on its worker, and state never crosses a thread boundary.
    struct State {
        owner: std::thread::ThreadId,
        calls: std::rc::Rc<Cell<usize>>,
    }
    for jobs in [1, 2, 8] {
        let initialized = AtomicUsize::new(0);
        let ran = map_with_state(
            Jobs::new(jobs),
            (0..40).collect(),
            |&i| i,
            || {
                initialized.fetch_add(1, Ordering::Relaxed);
                State {
                    owner: std::thread::current().id(),
                    calls: std::rc::Rc::new(Cell::new(0)),
                }
            },
            |state, task| {
                assert_eq!(state.owner, std::thread::current().id());
                state.calls.set(state.calls.get() + 1);
                (task, state.owner, state.calls.get())
            },
        );
        assert_eq!(
            initialized.load(Ordering::Relaxed),
            Jobs::new(jobs).worker_count(40)
        );
        let mut by_thread = std::collections::HashMap::new();
        for (index, (task, owner, ordinal)) in ran.into_iter().enumerate() {
            assert_eq!(index, task as usize);
            by_thread
                .entry(owner)
                .or_insert_with(Vec::new)
                .push(ordinal);
        }
        for ordinals in by_thread.values_mut() {
            ordinals.sort_unstable();
            assert_eq!(*ordinals, (1..=ordinals.len()).collect::<Vec<_>>());
        }
    }
    let count = AtomicUsize::new(0);
    let empty: Vec<()> = map_with_state(
        Jobs::new(8),
        Vec::<()>::new(),
        |_| 0,
        || {
            count.fetch_add(1, Ordering::Relaxed);
        },
        |_, _| (),
    );
    assert!(empty.is_empty());
    assert_eq!(count.load(Ordering::Relaxed), 0);
}
