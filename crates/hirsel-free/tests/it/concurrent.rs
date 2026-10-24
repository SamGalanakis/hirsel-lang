//! Concurrent recordings use the same F0 tape, projection and decoder as the
//! interpreter. Cuts must close over actual simultaneous phase actions.
use hirsel_free::concurrent::{Recorder, Task};
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::projection::{Projection, Stamped};
use hirsel_free::record::ObjectId;
use hirsel_free::replay::Replayer;
use hirsel_free::tape;
use std::sync::{Arc, Mutex};

fn done(op: u64) -> Progress {
    Progress::new(op, Phase::Done)
}

#[test]
fn prepared_children_join_the_cut_only_when_their_spawn_section_commits() {
    let recorder =
        Recorder::<Vec<u8>>::new(Vec::new(), b"prepared child", 1, false, true, None).unwrap();
    let mut worker = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut worker).unwrap();
    let prepared = root.prepare_child();
    let first = recorder.flush().unwrap().unwrap();
    assert_eq!(first.progress.len(), 1);
    assert!(!first.progress.contains_key(&vec![0]));
    let child = recorder
        .phase(&mut worker, &mut root, done(1), |writer| {
            writer.spawn_prepared(prepared)
        })
        .unwrap();
    let second = recorder.flush().unwrap().unwrap();
    assert_eq!(child.path(), &[0]);
    assert_eq!(second.progress[&vec![]], done(1));
    assert_eq!(second.progress[&vec![0]], Progress::START);
    let prepared = root.prepare_child();
    let stopped = recorder.stop("cut", None).unwrap().unwrap().0;
    assert!(!stopped.progress.contains_key(&vec![1]));
    let mut acted = false;
    assert!(
        recorder
            .phase(&mut worker, &mut root, done(2), |writer| {
                acted = true;
                writer.spawn_prepared(prepared)
            })
            .is_err()
    );
    assert!(!acted);
    drop(worker);
    let bytes = recorder.into_sink();
    let tape = tape::parse(&bytes).unwrap();
    assert_eq!(tape.cut, stopped.progress);
}

#[test]
fn eight_workers_record_object_actions_and_replay_at_one_permit() {
    const TASKS: usize = 8;
    const OPS: u64 = 32;
    let recorder =
        Arc::new(Recorder::<Vec<u8>>::new(Vec::new(), b"wide", TASKS, false, true, None).unwrap());
    let mut worker = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut worker).unwrap();
    let object = ObjectId::Made {
        path: vec![],
        op: 1,
    };
    let mut children = Vec::new();
    recorder
        .phase(&mut worker, &mut root, done(1), |phase| {
            phase.create(object.clone());
            for _ in 0..TASKS {
                children.push(phase.spawn());
            }
        })
        .unwrap();
    drop(worker);
    let version = Arc::new(Mutex::new(1u64));
    let threads: Vec<_> = children
        .into_iter()
        .enumerate()
        .map(|(slot, mut task)| {
            let (recorder, version, object) = (recorder.clone(), version.clone(), object.clone());
            std::thread::spawn(move || {
                let mut worker = recorder.executor(slot).unwrap();
                for op in 1..=OPS {
                    // No phase waits for the object lock: take it first. The
                    // version and queue effect then occur inside the section.
                    let mut version = version.lock().unwrap();
                    recorder
                        .phase(&mut worker, &mut task, done(op), |phase| {
                            *version += 1;
                            phase.touch(&object, *version, 0, "send");
                            phase.load(u128::from(op));
                        })
                        .unwrap();
                }
                task
            })
        })
        .collect();
    // Cut concurrently with the object actions, never after joining them all.
    let mut cuts = Vec::new();
    for _ in 0..12 {
        cuts.push(recorder.flush().unwrap().unwrap());
    }
    let tasks: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    let terminal = recorder.stop("returned", None).unwrap().unwrap().0;
    let mut trace: Vec<Stamped> = root.trace().to_vec();
    for task in &tasks {
        trace.extend_from_slice(task.trace());
    }
    let live = Projection::new("returned", &terminal.progress, &trace);
    let bytes = Arc::try_unwrap(recorder).ok().unwrap().into_sink();
    let parsed = tape::parse(&bytes).unwrap();
    let mut replayer = Replayer::new(&parsed).unwrap();
    replayer.create(&[], object.clone()).unwrap();
    replayer.publish(&[], done(1)).unwrap();
    let mut at: std::collections::BTreeMap<_, _> = [(Vec::new(), done(1))].into();
    // A single permit enables only one recorded next touch at a time; its
    // predecessor's publication enables it, independent of recording width.
    let mut next = [1u64; TASKS];
    while next.iter().any(|op| *op <= OPS) {
        let mut moved = false;
        for id in (0..TASKS).rev() {
            let op = next[id];
            let path = [id as u32];
            if op <= OPS && replayer.enabled(&path, op, 0, &object).unwrap() {
                replayer.touch(&path, op, 0, &object, "send").unwrap();
                assert_eq!(replayer.load(&path, op).unwrap(), u128::from(op));
                replayer.publish(&path, done(op)).unwrap();
                at.insert(path.to_vec(), done(op));
                next[id] += 1;
                moved = true;
            }
        }
        assert!(moved, "the recorded prefix must make progress at permit 1");
    }
    assert_eq!(replayer.finish(&at).unwrap(), live);
    for cut in cuts {
        let projection = Projection::new("truncated", &cut.progress, &trace);
        let history = &projection.objects[&object];
        assert_eq!(
            history.versions.keys().copied().collect::<Vec<_>>(),
            (1..=history.versions.len() as u64).collect::<Vec<_>>(),
            "every flush has a closed object prefix"
        );
    }
}

#[test]
fn stop_with_a_claimed_request_does_not_wait_for_host_and_quarantines_completion() {
    for durable in [false, true] {
        let recorder = Recorder::new(Vec::new(), b"in flight", 1, durable, false, None).unwrap();
        let mut worker = recorder.executor(0).unwrap();
        let mut root = recorder.root(&mut worker).unwrap();
        let request = recorder.prepare(1, vec![7; 64]);
        recorder
            .phase(
                &mut worker,
                &mut root,
                Progress::new(1, Phase::Entered),
                |phase| phase.prepare(&request),
            )
            .unwrap();
        assert!(recorder.durable_through(&[], root.progress()).unwrap());
        recorder
            .phase(
                &mut worker,
                &mut root,
                Progress::new(1, Phase::Claimed),
                |phase| assert!(recorder.claim(phase, &request)),
            )
            .unwrap();
        // The host has not returned; a stop must retain the request's buffers
        // while its completion is quarantined outside every admitted phase.
        let (cut, sweep) = recorder.stop("stopped", None).unwrap().unwrap();
        assert_eq!(sweep.in_doubt, [1]);
        assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Claimed));
        assert_eq!(request.buffers(), &vec![7; 64]);
        assert!(
            recorder
                .phase(&mut worker, &mut root, done(1), |_| panic!(
                    "late completion acted"
                ))
                .is_err()
        );
        assert!(recorder.stop("later", None).unwrap().is_none());
    }
}

#[test]
fn recording_metrics_count_only_gap_bytes_and_do_not_invent_cpu_time() {
    let metrics = Arc::new(hirsel_free::concurrent::Metrics::default());
    let recorder = Recorder::<Vec<u8>>::new(
        Vec::new(),
        b"metrics",
        1,
        false,
        false,
        Some(metrics.clone()),
    )
    .unwrap();
    let mut worker = recorder.executor(0).unwrap();
    let mut task: Task = recorder.root(&mut worker).unwrap();
    let object = ObjectId::Gate("package".into());
    recorder
        .phase(&mut worker, &mut task, done(1), |phase| {
            phase.touch(&object, 3, 0, "gate enter"); // One gapped event.
            phase.observation(&vec![7; 1000]);
            phase.load(u128::MAX);
        })
        .unwrap();
    let values: std::collections::BTreeMap<_, _> = metrics.values().into_iter().collect();
    assert!(values["compressed_bytes_per_contended_event"] <= 4.0);
    assert!(!values.contains_key("ns_per_op"));
    assert_eq!(values["cut_pause_ns"], 0.0);
}

#[test]
fn a_durable_watermark_never_admits_an_unpublished_prepare_or_a_failed_barrier() {
    use hirsel_free::recorder::TapeSink;
    struct Fails {
        barriers: usize,
    }
    impl TapeSink for Fails {
        fn append(&mut self, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn barrier(&mut self) -> std::io::Result<()> {
            self.barriers += 1;
            if self.barriers > 1 {
                Err(std::io::Error::other("injected durability failure"))
            } else {
                Ok(())
            }
        }
    }
    let recorder =
        Recorder::<Vec<u8>>::new(Vec::new(), b"not published", 1, true, false, None).unwrap();
    let mut worker = recorder.executor(0).unwrap();
    let _root = recorder.root(&mut worker).unwrap();
    assert!(
        !recorder
            .durable_through(&[], Progress::new(1, Phase::Entered))
            .unwrap()
    );

    let recorder = Recorder::new(
        Fails { barriers: 0 },
        b"failed barrier",
        1,
        true,
        false,
        None,
    )
    .unwrap();
    let mut worker = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut worker).unwrap();
    let request = recorder.prepare(1, ());
    recorder
        .phase(
            &mut worker,
            &mut root,
            Progress::new(1, Phase::Entered),
            |phase| phase.prepare(&request),
        )
        .unwrap();
    assert!(recorder.durable_through(&[], root.progress()).is_err());
}

#[test]
fn section_slot_tokens_cannot_be_held_by_two_workers_at_once() {
    let recorder = Recorder::<Vec<u8>>::new(Vec::new(), b"slots", 1, false, false, None).unwrap();
    let worker = recorder.executor(0).unwrap();
    assert!(recorder.executor(0).is_none());
    assert!(recorder.executor(1).is_none());
    drop(worker);
    assert!(recorder.executor(0).is_some());
}

#[test]
fn retiring_workers_with_one_preferred_slot_keep_exclusive_tokens_and_closed_cuts() {
    const WORKERS: usize = 8;
    let recorder = Arc::new(
        Recorder::<Vec<u8>>::new(Vec::new(), b"reused slots", WORKERS, true, true, None).unwrap(),
    );
    let mut root_worker = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut root_worker).unwrap();
    let children = recorder
        .phase(&mut root_worker, &mut root, done(1), |writer| {
            (0..WORKERS).map(|_| writer.spawn()).collect::<Vec<_>>()
        })
        .unwrap();
    drop(root_worker);
    let reserved = Arc::new(std::sync::Barrier::new(WORKERS + 1));
    let resume = Arc::new(std::sync::Barrier::new(WORKERS + 1));
    let threads: Vec<_> = children
        .into_iter()
        .enumerate()
        .map(|(index, mut task)| {
            let recorder = recorder.clone();
            let reserved = reserved.clone();
            let resume = resume.clone();
            std::thread::spawn(move || {
                // A retiring exec and its replacement prefer the same index.
                let mut worker = recorder.executor_available(0).unwrap();
                reserved.wait();
                resume.wait();
                recorder
                    .phase(&mut worker, &mut task, done(1), |writer| {
                        writer.load(index as u128)
                    })
                    .unwrap();
                drop(worker);
                let mut worker = loop {
                    if let Some(worker) = recorder.executor_available(0) {
                        break worker;
                    }
                    std::thread::yield_now();
                };
                recorder
                    .phase(&mut worker, &mut task, done(2), |writer| {
                        writer.load(100 + index as u128)
                    })
                    .unwrap();
                task
            })
        })
        .collect();
    reserved.wait();
    assert!(
        recorder.executor_available(0).is_none(),
        "every token is exclusively held"
    );
    // Merely holding tokens does not block a cut: no phase is open yet.
    let first = recorder.flush().unwrap().unwrap();
    assert_eq!(first.progress.len(), WORKERS + 1);
    assert!(
        first
            .progress
            .iter()
            .filter(|(path, _)| !path.is_empty())
            .all(|(_, progress)| *progress == Progress::START)
    );
    resume.wait();
    let _ = recorder.flush().unwrap().unwrap();
    let tasks: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    let cut = recorder.stop("returned", None).unwrap().unwrap().0;
    let mut trace = root.trace().to_vec();
    for task in tasks {
        trace.extend_from_slice(task.trace());
    }
    let bytes = Arc::try_unwrap(recorder).ok().unwrap().into_sink();
    let tape = tape::parse(&bytes).unwrap();
    let mut replay = Replayer::new(&tape).unwrap();
    replay.publish(&[], done(1)).unwrap();
    for index in 0..WORKERS {
        let path = [index as u32];
        assert_eq!(replay.load(&path, 1).unwrap(), index as u128);
        replay.publish(&path, done(1)).unwrap();
        assert_eq!(replay.load(&path, 2).unwrap(), 100 + index as u128);
        replay.publish(&path, done(2)).unwrap();
    }
    assert_eq!(
        replay.finish(&cut.progress).unwrap(),
        Projection::new("returned", &cut.progress, &trace)
    );
}

#[test]
#[should_panic(expected = "task belongs to another recording")]
fn a_task_log_cannot_be_written_under_another_runs_gate() {
    let a = Recorder::<Vec<u8>>::new(Vec::new(), b"a", 1, false, false, None).unwrap();
    let b = Recorder::<Vec<u8>>::new(Vec::new(), b"b", 1, false, false, None).unwrap();
    let mut a_worker = a.executor(0).unwrap();
    let mut b_worker = b.executor(0).unwrap();
    let mut task = a.root(&mut a_worker).unwrap();
    b.phase(&mut b_worker, &mut task, done(1), |_| ()).unwrap();
}

#[test]
fn a_rejected_entry_prefix_leaves_no_progress_record_or_open_gate_slot() {
    let recorder =
        Recorder::<Vec<u8>>::new(Vec::new(), b"entry retry", 1, false, true, None).unwrap();
    let mut worker = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut worker).unwrap();
    let mut acted = false;
    let rejected = recorder.phase_if(
        &mut worker,
        &mut root,
        done(1),
        || false,
        |_| {
            acted = true;
        },
    );
    assert_eq!(rejected.unwrap(), None);
    assert!(!acted);
    let cut = recorder.flush().unwrap().unwrap();
    assert_eq!(cut.progress[&Vec::new()], Progress::START);
    assert!(root.trace().is_empty());
    recorder
        .phase_if(
            &mut worker,
            &mut root,
            done(1),
            || true,
            |writer| writer.load(77),
        )
        .unwrap()
        .unwrap();
    let cut = recorder.stop("returned", None).unwrap().unwrap().0;
    assert_eq!(cut.progress[&Vec::new()], done(1));
    drop(worker);
    let tape = tape::parse(&recorder.into_sink()).unwrap();
    let mut replay = Replayer::new(&tape).unwrap();
    assert!(replay.starts(&[]));
    assert_eq!(replay.load(&[], 1).unwrap(), 77);
    replay.publish(&[], done(1)).unwrap();
    replay.finish(&cut.progress).unwrap();
}
