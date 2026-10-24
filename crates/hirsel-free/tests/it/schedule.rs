use hirsel_free::queue;
use hirsel_free::schedule::{Budget, OP_BUDGET, TaskState};
use std::sync::Arc;

#[test]
fn a_quantum_is_128_completed_semantic_ops_and_parking_returns_the_rest() {
    let mut budget = Budget::default();
    for _ in 0..3 {
        for _ in 1..OP_BUDGET {
            assert!(budget.proceed());
        }
        assert!(!budget.proceed());
    }
    for _ in 0..77 {
        assert!(budget.proceed());
    }
    budget.reset();
    for _ in 1..OP_BUDGET {
        assert!(budget.proceed());
    }
    assert!(!budget.proceed());
}

#[test]
fn overflow_injects_the_newest_half_and_stealing_takes_the_oldest_half() {
    let (mut owner, thief) = queue::local::<_, 4>();
    for task in 0..4 {
        assert!(owner.push(task).is_empty());
    }
    assert_eq!(owner.push(4), [2, 3, 4]);
    assert_eq!(thief.steal_half(), [0]);
    assert_eq!(owner.pop(), Some(1));
    assert_eq!(owner.pop(), None);
    for task in 10..13 {
        assert!(owner.push(task).is_empty());
    }
    assert_eq!(thief.steal_half(), [10, 11]);
    assert_eq!(owner.pop(), Some(12));
}

#[test]
fn repeated_reuse_of_slots_drops_every_owned_identifier_once() {
    struct Token(usize, Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for Token {
        fn drop(&mut self) {
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (mut owner, thief) = queue::local::<_, 4>();
    let mut seen = Vec::new();
    for round in 0..200 {
        for task in 0..7 {
            for token in owner.push(Token(round * 7 + task, dropped.clone())) {
                seen.push(token.0);
            }
        }
        for token in thief.steal_half() {
            seen.push(token.0);
        }
        for token in owner.drain() {
            seen.push(token.0);
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, (0..1400).collect::<Vec<_>>());
    assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 1400);
}

#[test]
fn started_suspended_tasks_can_be_stolen_without_duplicate_execution() {
    const TASKS: usize = 4000;
    const ROUNDS: u32 = 4;
    struct Task {
        id: usize,
        rounds: u32,
        state: Arc<TaskState>,
    }
    let (mut owner, thief) = queue::local::<Task, 256>();
    let seen: Arc<Vec<std::sync::atomic::AtomicU32>> = Arc::new(
        (0..TASKS)
            .map(|_| std::sync::atomic::AtomicU32::new(0))
            .collect(),
    );
    let mut injected = Vec::new();
    for id in 0..TASKS {
        let state = Arc::new(TaskState::default());
        assert!(state.notify());
        injected.extend(owner.push(Task {
            id,
            rounds: ROUNDS,
            state,
        }));
    }
    let threads: Vec<_> = (0..3)
        .map(|_| {
            let (thief, seen) = (thief.clone(), seen.clone());
            std::thread::spawn(move || {
                for mut task in thief.steal_half() {
                    assert!(task.state.start());
                    seen[task.id].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    task.rounds -= 1;
                    assert!(task.state.suspend(true));
                    // The saved fiber is ready, and its identifier can migrate
                    // to a different queue without changing its scheduling state.
                    while task.rounds > 0 {
                        assert!(task.state.start());
                        seen[task.id].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        task.rounds -= 1;
                        if task.rounds == 0 {
                            task.state.complete();
                        } else {
                            assert!(task.state.suspend(true));
                        }
                    }
                }
            })
        })
        .collect();
    injected.extend(owner.drain());
    for task in injected {
        for round in 0..ROUNDS {
            assert!(task.state.start());
            seen[task.id].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if round + 1 == ROUNDS {
                task.state.complete();
            } else {
                assert!(task.state.suspend(true));
            }
        }
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(
        seen.iter()
            .all(|count| count.load(std::sync::atomic::Ordering::Relaxed) == ROUNDS)
    );
}

#[test]
fn lifo_chains_yield_to_fifo_after_three_polls_and_expose_work_on_shrink() {
    let mut lifo = hirsel_free::schedule::Lifo::default();
    assert_eq!(lifo.push(0), None);
    assert_eq!(lifo.push(1), Some(0));
    assert_eq!(lifo.pop(), Some(1));
    for id in 2..=3 {
        assert_eq!(lifo.push(id), None);
        assert_eq!(lifo.pop(), Some(id));
    }
    assert_eq!(lifo.push(4), Some(4));
    assert_eq!(lifo.pop(), None);
    assert_eq!(lifo.to_fifo(), None);
    assert_eq!(lifo.push(5), None);
    assert_eq!(lifo.to_fifo(), Some(5));
}

#[test]
fn gates_exist_at_admission_regardless_of_which_task_reaches_them_first() {
    use hirsel_free::phase::Phase;
    use hirsel_free::session::{Recording, Session};
    let gate = (3, 99);
    let a = [0];
    let b = [1];
    let mut live = Session::record(b"gates", Recording::default());
    for path in [&a[..], &b[..]] {
        assert!(live.start(path));
    }
    live.gate(gate, "serial-example");
    // A enters; while its callback runs, B enters its waiting op. A's
    // release must precede B's take even if B is replayed first.
    live.begin(&a);
    live.touch(&a, 0, gate, "gate enter").unwrap();
    live.publish(&a, Phase::Done).unwrap();
    live.begin(&b);
    live.publish(&b, Phase::Entered).unwrap();
    live.begin(&a);
    live.touch(&a, 0, gate, "gate leave").unwrap();
    live.publish(&a, Phase::Done).unwrap();
    live.touch(&b, 0, gate, "gate enter").unwrap();
    live.publish(&b, Phase::Done).unwrap();
    live.begin(&b);
    live.touch(&b, 0, gate, "gate leave").unwrap();
    live.publish(&b, Phase::Done).unwrap();
    live.stop("returned", None);
    let report = live.report();
    let expected = &report.cuts.last().unwrap().1;
    let mut replay = Session::replay(&report.tape).unwrap();
    for path in [&b[..], &a[..]] {
        assert!(replay.start(path));
    }
    replay.gate(gate, "serial-example");
    replay.begin(&b);
    replay.publish(&b, Phase::Entered).unwrap();
    assert!(!replay.enabled(&b, 0, gate).unwrap());
    replay.begin(&a);
    assert!(replay.enabled(&a, 0, gate).unwrap());
    replay.touch(&a, 0, gate, "gate enter").unwrap();
    replay.publish(&a, Phase::Done).unwrap();
    assert!(!replay.enabled(&b, 0, gate).unwrap());
    replay.begin(&a);
    replay.touch(&a, 0, gate, "gate leave").unwrap();
    replay.publish(&a, Phase::Done).unwrap();
    assert!(replay.enabled(&b, 0, gate).unwrap());
    replay.touch(&b, 0, gate, "gate enter").unwrap();
    replay.publish(&b, Phase::Done).unwrap();
    replay.begin(&b);
    replay.touch(&b, 0, gate, "gate leave").unwrap();
    replay.publish(&b, Phase::Done).unwrap();
    assert_eq!(&replay.finish().unwrap(), expected);
}

#[test]
fn work_search_steals_started_ids_and_shrink_exposes_the_retiring_lifo() {
    use hirsel_free::work;
    let metrics = Arc::new(work::Metrics::default());
    let (ready, mut workers) = work::workers(8, Some(metrics.clone()));
    for id in 0..6 {
        workers[7].push(&ready, id, true);
    }
    workers[7].push(&ready, 99, false);
    // Worker 0 skips empty queues, steals ceil(6/2), and runs the last
    // stolen identifier. A started fiber is represented by the same ID.
    assert_eq!(workers[0].next(&ready), Some((2, work::Source::Steal)));
    assert_eq!(workers[0].next(&ready), Some((0, work::Source::Local)));
    assert_eq!(workers[0].next(&ready), Some((1, work::Source::Local)));
    assert_eq!(metrics.counts().1, 1);
    ready.set_width(1);
    // The retiring owner exposes its LIFO slot and FIFO explicitly.
    workers[7].drain(&ready);
    let mut remaining = Vec::new();
    while let Some((id, _)) = workers[0].next(&ready) {
        remaining.push(id);
    }
    remaining.sort_unstable();
    assert_eq!(remaining, [3, 4, 5, 99]);
    assert!(
        metrics
            .values(10)
            .iter()
            .any(|(name, value)| *name == "queue_ops_per_op" && *value > 0.0)
    );
}

#[test]
fn eight_workers_requeue_and_steal_started_tasks_through_live_shrink() {
    use hirsel_free::work;
    use std::sync::atomic::{AtomicUsize, Ordering};
    const TASKS: usize = 512;
    const OPS: usize = 24;
    struct Fiber {
        id: usize,
        state: Arc<TaskState>,
        remaining: usize,
    }
    let (ready, workers) = work::workers(8, None);
    let seen: Arc<Vec<AtomicUsize>> = Arc::new((0..TASKS).map(|_| AtomicUsize::new(0)).collect());
    let finished = Arc::new(AtomicUsize::new(0));
    let total = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    ready.inject((0..TASKS).map(|id| {
        let state = Arc::new(TaskState::default());
        assert!(state.notify());
        Fiber {
            id,
            state,
            remaining: OPS,
        }
    }));
    let threads: Vec<_> = workers
        .into_iter()
        .enumerate()
        .map(|(index, mut worker)| {
            let (ready, seen, finished, total, barrier) = (
                ready.clone(),
                seen.clone(),
                finished.clone(),
                total.clone(),
                barrier.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                while finished.load(Ordering::Acquire) != TASKS {
                    if index >= ready.width() as usize {
                        // A retiring worker drains at its scheduler boundary.
                        break;
                    }
                    let Some((mut fiber, _)) = worker.next(&ready) else {
                        std::thread::yield_now();
                        continue;
                    };
                    assert!(fiber.state.start());
                    seen[fiber.id].fetch_add(1, Ordering::Relaxed);
                    if total.fetch_add(1, Ordering::Relaxed) == TASKS {
                        ready.set_width(1);
                    }
                    fiber.remaining -= 1;
                    if fiber.remaining == 0 {
                        fiber.state.complete();
                        finished.fetch_add(1, Ordering::Release);
                    } else {
                        // The fiber's stack is now saved; only then is it eligible
                        // for publication, including to another worker's steal.
                        assert!(fiber.state.suspend(true));
                        worker.push(&ready, fiber, true);
                    }
                }
                worker.drain(&ready);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(total.load(Ordering::Relaxed), TASKS * OPS);
    assert!(
        seen.iter()
            .all(|count| count.load(Ordering::Relaxed) == OPS)
    );
    assert_eq!(ready.width(), 1);
}

#[test]
fn exclusive_saved_images_migrate_through_eight_queues_and_drop_at_completion() {
    use hirsel_free::{frame, work};
    use std::sync::atomic::{AtomicUsize, Ordering};
    const TASKS: usize = 256;
    const SAVES: usize = 12;
    struct Image {
        saves: usize,
        freed: Arc<AtomicUsize>,
    }
    impl Drop for Image {
        fn drop(&mut self) {
            assert_eq!(self.saves, SAVES);
            self.freed.fetch_add(1, Ordering::Release);
        }
    }
    let freed = Arc::new(AtomicUsize::new(0));
    let (ready, workers) = work::workers(8, None);
    let mut notifications = Vec::new();
    for _ in 0..TASKS {
        let (notification, queued) = frame::prepared(Image {
            saves: 0,
            freed: freed.clone(),
        });
        notifications.push(notification);
        ready.inject([queued]);
    }
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|threads| {
        for mut worker in workers {
            let (freed, ready, barrier) = (freed.clone(), ready.clone(), barrier.clone());
            threads.spawn(move || {
                barrier.wait();
                while freed.load(Ordering::Acquire) != TASKS {
                    let Some((queued, _)) = worker.next(&ready) else {
                        std::thread::yield_now();
                        continue;
                    };
                    let mut running = queued.start();
                    if running.value().saves == SAVES {
                        drop(running);
                    } else {
                        running.value_mut().saves += 1;
                        worker.push(&ready, running.save(true).unwrap(), true);
                    }
                }
            });
        }
    });
    assert_eq!(freed.load(Ordering::Acquire), TASKS);
    assert!(notifications.iter().all(|wake| wake.notify().is_none()));
}

#[test]
fn deadline_exit_substitutes_sparse_expired_outcomes_after_entered_cuts() {
    use hirsel_free::phase::Phase;
    use hirsel_free::session::{Recording, Session};
    for durable in [false, true] {
        let mut live = Session::record(
            b"deadline exits",
            Recording {
                durable,
                flush_one_in: 1,
                seed: 0,
            },
        );
        assert!(live.start(&[]));
        let outcomes = [true, false, false, true];
        for expired in outcomes {
            live.begin(&[]);
            live.publish(&[], Phase::Entered).unwrap();
            assert_eq!(live.expired(&[], expired).unwrap(), expired);
            live.publish(&[], Phase::Done).unwrap();
        }
        live.stop("returned", None);
        let report = live.report();
        for (end, expected) in &report.cuts {
            for torn in [0, 5] {
                let bytes = &report.tape[..(end + torn).min(report.tape.len())];
                let mut replay = Session::replay(bytes).unwrap();
                if let Some(cut) = expected.cut.get(&Vec::new()) {
                    assert!(replay.start(&[]));
                    for op in 1..=cut.op {
                        replay.begin(&[]);
                        replay.publish(&[], Phase::Entered).unwrap();
                        if op < cut.op || cut.phase == Phase::Done {
                            let expired = outcomes[(op - 1) as usize];
                            assert_eq!(replay.expired(&[], !expired).unwrap(), expired);
                            replay.publish(&[], Phase::Done).unwrap();
                        }
                    }
                } else {
                    assert!(!replay.start(&[]));
                }
                assert_eq!(replay.finish().unwrap(), *expected);
            }
        }
    }
}
