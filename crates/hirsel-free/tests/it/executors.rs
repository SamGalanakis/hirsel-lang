//! Executor registration and acknowledged retirement (runtime r6 recording slots). The
//! vectors port the intent of Tokio's `tests/rt_handle.rs` (nested entry, out-of-order drop)
//! and `tests/rt_worker_index.rs` (a worker's identity versus an external caller's). Hirsel
//! permits nested entry on distinct slots, so out-of-order retirement is not a panic here.

use hirsel_free::concurrent::{HOME, Recorder, Task};
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::record::RecordBound;
use hirsel_free::session::Marked;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

fn done(op: u64) -> Progress {
    Progress::new(op, Phase::Done)
}

fn recorder(slots: usize) -> Arc<Recorder<Marked>> {
    Arc::new(Recorder::new(Marked::default(), b"executors", slots, false, false, None).unwrap())
}

/// A root and `children` spawned tasks, registered on the home word, which then retires.
fn tasks(recorder: &Recorder<Marked>, children: usize) -> (Task, Vec<Task>) {
    let mut home = recorder.executor(HOME).unwrap();
    let mut root = recorder.root(&mut home).unwrap();
    let mut prepared = root.prepare_children(children as u32);
    let mut spawned = Vec::with_capacity(children);
    recorder
        .phase(&mut home, &mut root, done(1), RecordBound::default(), |w| {
            spawned.extend(prepared.drain(..).map(|child| w.spawn_prepared(child)));
        })
        .unwrap();
    recorder.retire_executor(home);
    (root, spawned)
}

#[test]
fn home_word_is_registered_only_by_its_preferrer() {
    let recorder = recorder(3);
    let one = recorder.executor_available(1).unwrap();
    let two = recorder.executor_available(1).unwrap();
    assert_eq!((one.index(), two.index()), (1, 2));
    // Home stays free for home: an external caller finds no word rather than taking it.
    assert!(recorder.executor_available(1).is_none());
    let home = recorder.executor_available(HOME).unwrap();
    assert_eq!(home.index(), HOME);
    assert_eq!(recorder.registered(), vec![0, 1, 2]);
}

#[test]
fn nested_entry_registers_distinct_slots_and_retires_out_of_order() {
    let recorder = recorder(4);
    let (mut root, _) = tasks(&recorder, 0);
    // rt_handle `basic_enter` / `interleave_enter_same_rt`: each nested entry on one thread
    // owns a distinct word while the enclosing one is checked out.
    let mut outer = recorder.executor_available(1).unwrap();
    let mut nested = recorder.executor_available(outer.index()).unwrap();
    let mut deeper = recorder.executor_available(outer.index()).unwrap();
    let indices = [outer.index(), nested.index(), deeper.index()];
    assert_eq!(indices, [1, 2, 3]);
    for (op, slot) in [&mut deeper, &mut nested, &mut outer]
        .into_iter()
        .enumerate()
    {
        root.reserve(RecordBound::records(1));
        recorder
            .phase(
                slot,
                &mut root,
                done(2 + op as u64),
                RecordBound::records(1),
                |w| w.load(op as u128),
            )
            .unwrap();
    }
    // Out-of-order retirement (Tokio panics for its single entered flag): each word retires
    // independently, after its own sections closed.
    recorder.retire_executor(outer);
    assert_eq!(recorder.registered(), vec![2, 3]);
    recorder.retire_executor(deeper);
    recorder.retire_executor(nested);
    assert!(recorder.registered().is_empty());
    let cut = recorder.flush().unwrap().unwrap();
    assert_eq!(cut.progress[&Vec::new()], done(4));
}

#[test]
fn an_executor_slot_is_identified_with_its_own_recording() {
    // rt_worker_index: a worker has its runtime's identity; another runtime's worker on the
    // same thread is an external caller there. Two recordings share this thread, each with
    // its own stable registration and no churn between their phases.
    let (a, b) = (recorder(2), recorder(2));
    let ((mut ra, _), (mut rb, _)) = (tasks(&a, 0), tasks(&b, 0));
    let mut sa = a.executor_available(1).unwrap();
    let mut sb = b.executor_available(1).unwrap();
    for op in 2..6 {
        ra.reserve(RecordBound::records(1));
        a.phase(&mut sa, &mut ra, done(op), RecordBound::records(1), |w| {
            w.load(1)
        })
        .unwrap();
        rb.reserve(RecordBound::records(1));
        b.phase(&mut sb, &mut rb, done(op), RecordBound::records(1), |w| {
            w.load(2)
        })
        .unwrap();
    }
    assert!(a.owns(&sa) && !a.owns(&sb) && b.owns(&sb) && !b.owns(&sa));
    assert_eq!((a.registered(), b.registered()), (vec![1], vec![1]));
    let foreign = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        a.phase(&mut sb, &mut ra, done(6), RecordBound::records(1), |_| ())
    }));
    assert!(
        foreign.is_err(),
        "a slot never enters another recording's gate"
    );
    a.retire_executor(sa);
    b.retire_executor(sb);
    assert!(a.registered().is_empty() && b.registered().is_empty());
}

#[test]
fn a_starting_executor_waits_for_a_retirement_without_spinning() {
    let recorder = recorder(3);
    let held = [
        recorder.executor_available(1).unwrap(),
        recorder.executor_available(1).unwrap(),
    ];
    let (started, waiting) = mpsc::channel();
    let starting = std::thread::spawn({
        let recorder = recorder.clone();
        move || {
            started.send(()).unwrap();
            recorder.register(1).map(|slot| slot.index())
        }
    });
    waiting.recv().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    assert!(!starting.is_finished(), "every non-home word is held");
    let [one, two] = held;
    recorder.retire_executor(two);
    assert_eq!(starting.join().unwrap(), Some(2));
    // A stop wakes a waiting executor: no section opens again, so it starts unregistered.
    let other = recorder.executor_available(1).unwrap();
    let starting = std::thread::spawn({
        let recorder = recorder.clone();
        move || recorder.register(1).map(|slot| slot.index())
    });
    std::thread::sleep(Duration::from_millis(20));
    recorder.stop("stopped", None).unwrap().unwrap();
    assert_eq!(starting.join().unwrap(), None);
    drop((one, other));
}

#[test]
fn an_open_section_word_is_not_reused_and_a_retired_word_is_not_waited_for() {
    let recorder = recorder(3);
    let (_, mut children) = tasks(&recorder, 1);
    let mut child = children.pop().unwrap();
    let opened = Arc::new(AtomicBool::new(false));
    // A flag, not a channel: a section allocates nothing, and a blocking receive may.
    let release = Arc::new(AtomicBool::new(false));
    let busy = std::thread::spawn({
        let (recorder, opened, release) = (recorder.clone(), opened.clone(), release.clone());
        move || {
            let mut slot = recorder.register(1).unwrap();
            child.reserve(RecordBound::records(1));
            recorder
                .phase(
                    &mut slot,
                    &mut child,
                    done(1),
                    RecordBound::records(1),
                    |w| {
                        opened.store(true, Ordering::Release);
                        while !release.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                        w.load(7);
                    },
                )
                .unwrap();
            recorder.retire_executor(slot);
        }
    });
    while !opened.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    // The open section's owner still holds its word: no executor registers it early.
    assert!(recorder.executor(1).is_none());
    release.store(true, Ordering::Release);
    busy.join().unwrap();
    // Retired after its acknowledged close: a cut does not wait for it, and it is free.
    assert!(recorder.registered().is_empty());
    let cut = recorder.flush().unwrap().unwrap();
    assert_eq!(cut.progress[&vec![0]], done(1));
    assert_eq!(recorder.executor(1).map(|slot| slot.index()), Some(1));
}

#[test]
fn shrink_and_retirement_race_concurrent_cuts_with_one_owner_per_word() {
    const EXECUTORS: usize = 6;
    const GENERATIONS: u64 = 40;
    // Room for the live set and one retiring generation beside home, as the runtime sizes it.
    let recorder = recorder(2 * EXECUTORS + 1);
    let (root, children) = tasks(&recorder, EXECUTORS);
    let owners: Arc<Vec<AtomicU64>> = Arc::new((0..2 * EXECUTORS + 1).map(|_| 0.into()).collect());
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(std::sync::Barrier::new(EXECUTORS + 1));
    let first_cut = Arc::new(AtomicBool::new(false));
    let cutter = std::thread::spawn({
        let (recorder, stop, ready, first_cut) = (
            recorder.clone(),
            stop.clone(),
            ready.clone(),
            first_cut.clone(),
        );
        move || {
            ready.wait();
            let mut cuts = 0;
            while !stop.load(Ordering::Acquire) {
                recorder.flush().unwrap().unwrap();
                cuts += 1;
                first_cut.store(true, Ordering::Release);
            }
            cuts
        }
    });
    let threads: Vec<_> = children
        .into_iter()
        .enumerate()
        .map(|(index, mut task)| {
            let (recorder, owners, ready, first_cut) = (
                recorder.clone(),
                owners.clone(),
                ready.clone(),
                first_cut.clone(),
            );
            std::thread::spawn(move || {
                for generation in 0..GENERATIONS {
                    // Each generation is a fresh executor preferring its compute word; the
                    // previous one retired, so it may take the same word again.
                    let mut slot = recorder.register(1 + index).unwrap();
                    let owner = (index as u64) << 32 | generation + 1;
                    assert_eq!(
                        owners[slot.index()].swap(owner, Ordering::AcqRel),
                        0,
                        "a word has one owner"
                    );
                    if generation == 0 {
                        ready.wait();
                    }
                    task.reserve(RecordBound::records(1));
                    recorder
                        .phase(
                            &mut slot,
                            &mut task,
                            done(generation + 1),
                            RecordBound::records(1),
                            |w| w.load(owner.into()),
                        )
                        .unwrap();
                    // Keep the first generation live until the cutter actually runs.
                    if generation == 0 {
                        while !first_cut.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                    }
                    assert_eq!(owners[slot.index()].swap(0, Ordering::AcqRel), owner);
                    recorder.retire_executor(slot);
                }
                task
            })
        })
        .collect();
    let tasks: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    stop.store(true, Ordering::Release);
    assert!(cutter.join().unwrap() > 0);
    assert!(recorder.registered().is_empty());
    let (cut, _) = recorder.stop("returned", None).unwrap().unwrap();
    for task in &tasks {
        assert_eq!(cut.progress[task.path()], done(GENERATIONS));
    }
    drop(root);
}
