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

#[test]
fn spawn_failure_finishes_every_task_once_in_input_order() {
    for successful_spawns in [0, 1] {
        let attempts = AtomicUsize::new(0);
        let initialized = AtomicUsize::new(0);
        let dropped = AtomicUsize::new(0);
        struct State<'a>(&'a AtomicUsize);
        impl Drop for State<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let calls: Vec<_> = (0..32).map(|_| AtomicUsize::new(0)).collect();
        let before = work::read(work::Counter::CacheValidationRows);
        let results = map_with_state_spawn(
            Jobs::new(4),
            (0..32).collect(),
            |&i| i as u64,
            || {
                initialized.fetch_add(1, Ordering::Relaxed);
                State(&dropped)
            },
            |_, i| {
                calls[i].fetch_add(1, Ordering::Relaxed);
                work::count_by(work::Counter::CacheValidationRows, i as u64);
                i
            },
            || {
                if attempts.fetch_add(1, Ordering::Relaxed) == successful_spawns {
                    Err(std::io::Error::from_raw_os_error(11))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(results, (0..32).collect::<Vec<_>>());
        assert!(calls.iter().all(|count| count.load(Ordering::Relaxed) == 1));
        assert_eq!(attempts.load(Ordering::Relaxed), successful_spawns + 1);
        assert_eq!(initialized.load(Ordering::Relaxed), 1);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(
            work::read(work::Counter::CacheValidationRows) - before,
            (0..32u64).sum::<u64>()
        );
    }
}

#[test]
fn spawn_failure_preserves_task_panic_and_drops_queued_tasks() {
    for successful_spawns in [0, 1] {
        let attempts = AtomicUsize::new(0);
        let started = AtomicUsize::new(0);
        let dropped = AtomicUsize::new(0);
        struct Task<'a>(&'a AtomicUsize);
        impl Drop for Task<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let payload = catch_unwind(AssertUnwindSafe(|| {
            map_with_state_spawn(
                Jobs::new(4),
                (0..32).map(|_| Task(&dropped)).collect(),
                |_| 0,
                || (),
                |_, _task| -> () {
                    started.fetch_add(1, Ordering::Relaxed);
                    std::panic::panic_any("task failure");
                },
                || {
                    if attempts.fetch_add(1, Ordering::Relaxed) == successful_spawns {
                        Err(std::io::Error::from_raw_os_error(11))
                    } else {
                        Ok(())
                    }
                },
            )
        }))
        .expect_err("the task panic still reaches the caller");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"task failure"));
        assert_eq!(started.load(Ordering::Relaxed), 1);
        assert_eq!(dropped.load(Ordering::Relaxed), 32);
    }
}

#[test]
fn spawn_failure_preserves_initializer_panic() {
    for successful_spawns in [0, 1] {
        let attempts = AtomicUsize::new(0);
        let payload = catch_unwind(AssertUnwindSafe(|| {
            map_with_state_spawn(
                Jobs::new(4),
                vec![(); 4],
                |_| 0,
                || -> () { std::panic::panic_any("initializer failure") },
                |_, ()| (),
                || {
                    if attempts.fetch_add(1, Ordering::Relaxed) == successful_spawns {
                        Err(std::io::Error::from_raw_os_error(11))
                    } else {
                        Ok(())
                    }
                },
            )
        }))
        .expect_err("the initializer panic still reaches the caller");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"initializer failure"));
    }
}

#[test]
fn jobs_one_and_empty_frontiers_never_try_to_spawn() {
    let caller = std::thread::current().id();
    for (jobs, tasks) in [(Jobs::ONE, vec![(); 4]), (Jobs::new(4), vec![])] {
        let results = map_with_state_spawn(
            jobs,
            tasks,
            |_| 0,
            || (),
            |_, ()| std::thread::current().id(),
            || panic!("sequential and empty frontiers must not spawn"),
        );
        assert!(results.iter().all(|&id| id == caller));
    }
}
