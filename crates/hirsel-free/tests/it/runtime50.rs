//! Reserved direct recording, registered executor slots and diagnostic-free encoding
//! (runtime r6 S4).

use hirsel_free::concurrent::{ExecutorSlot, Metrics, Recorder, Task};
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::projection::Projection;
use hirsel_free::record::{Encoder, ObjectId, Record, RecordBound, decode};
use hirsel_free::replay::Replayer;
use hirsel_free::section::SEGMENT as SEGMENT_HINT;
use hirsel_free::session::{Marked, Session};
use hirsel_free::tape;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};

/// The test binary's allocator: counts this thread's allocations.
pub struct Counting;
thread_local! { static ALLOCATIONS: Cell<u64> = const { Cell::new(0) }; }
fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(block, layout, size) }
    }
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        unsafe { System.dealloc(block, layout) }
    }
}

fn wait(flag: &AtomicBool) {
    while !flag.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
}

fn done(op: u64) -> Progress {
    Progress::new(op, Phase::Done)
}

/// One phase, counting this thread's allocations from inside the open section (the prefix
/// runs right after admission) until it published and closed.
fn counted<S: hirsel_free::recorder::TapeSink, T>(
    recorder: &Recorder<S>,
    slot: &mut ExecutorSlot,
    task: &mut Task,
    progress: Progress,
    action: impl FnOnce(&mut hirsel_free::concurrent::Writer<'_, '_>) -> T,
) -> (T, u64) {
    let opened = Cell::new(0);
    let result = recorder
        .phase_if(
            slot,
            task,
            progress,
            || {
                opened.set(allocations());
                true
            },
            action,
        )
        .unwrap()
        .unwrap();
    (result, allocations() - opened.get())
}

/// Op `op`'s payload: lengths chosen so records straddle and exhaust segments.
fn payload(op: u64) -> Vec<u8> {
    let len = (op as usize * 977) % (SEGMENT_HINT + 700);
    (0..len).map(|i| (i as u64 ^ op) as u8).collect()
}

fn records_of(op: u64) -> Vec<Record> {
    vec![
        Record::Load {
            op,
            bits: u128::from(op) << 100 | 0xfeed,
        },
        Record::Observation {
            op,
            bytes: payload(op),
        },
    ]
}

/// Every flush and the terminal cut of `marked` holds exactly the records of the phases it
/// admits: complete, decodable and none beyond.
fn admitted_records_complete(marked: &Marked, path: &[u32]) -> usize {
    let mut checked = 0;
    for &end in &marked.marks {
        let parsed = tape::parse(&marked.bytes[..end]).unwrap();
        let Some(at) = parsed.cut.get(path) else {
            continue;
        };
        let records = decode(parsed.logs.get(path).map_or(&[][..], Vec::as_slice)).unwrap();
        let expected: Vec<Record> = (1..=at.op).flat_map(records_of).collect();
        assert_eq!(
            records, expected,
            "the cut at {at} admits exactly its records"
        );
        checked += 1;
    }
    checked
}

#[test]
fn runtime50_reserved_record_write_cut_matrix() {
    const OPS: u64 = 48;
    let metrics = Arc::new(Metrics::default());
    let recorder = Arc::new(
        Recorder::<Marked>::new(
            Marked::default(),
            b"reserved cut matrix",
            2,
            false,
            false,
            Some(metrics.clone()),
        )
        .unwrap(),
    );
    let mut slot = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut slot).unwrap();
    let effects = Arc::new(AtomicU64::new(0));
    let mut linked = 0;
    for op in 1..=OPS {
        let bytes = payload(op);
        let bound = RecordBound::records(1).payload(bytes.len());
        let reservation = root.reserve(bound);
        linked += reservation.allocated;
        match op % 4 {
            // A cut between reservation and admission: reserved room is not published.
            0 => {
                let cut = recorder.flush().unwrap().unwrap();
                assert_eq!(cut.progress[&Vec::new()], done(op - 1));
            }
            // A cut that wants the gate while the phase writes waits for its publication.
            1 => {
                let open = Arc::new(AtomicBool::new(false));
                let cutter = std::thread::spawn({
                    let (recorder, effects, open) =
                        (recorder.clone(), effects.clone(), open.clone());
                    move || {
                        wait(&open);
                        let cut = recorder.flush().unwrap().unwrap();
                        (cut, effects.load(Ordering::Acquire))
                    }
                });
                let (_, allocated) = counted(&recorder, &mut slot, &mut root, done(op), |w| {
                    effects.fetch_add(1, Ordering::AcqRel);
                    w.load(u128::from(op) << 100 | 0xfeed);
                    open.store(true, Ordering::Release);
                    // The cutter is now closing the gate; it cannot pass this section.
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    w.observation(&bytes);
                });
                assert_eq!(allocated, 0, "op {op} allocated inside its section");
                let (cut, seen) = cutter.join().unwrap();
                assert_eq!(cut.progress[&Vec::new()], done(op));
                assert_eq!(seen, op, "every observed action has its admitted phase");
                continue;
            }
            _ => {}
        }
        let (_, allocated) = counted(&recorder, &mut slot, &mut root, done(op), |w| {
            effects.fetch_add(1, Ordering::AcqRel);
            w.load(u128::from(op) << 100 | 0xfeed);
            w.observation(&bytes);
        });
        assert_eq!(allocated, 0, "op {op} allocated inside its section");
        // A cut right after publication (and after a segment was exhausted and linked).
        if op % 4 == 2 || reservation.allocated != 0 {
            let cut = recorder.flush().unwrap().unwrap();
            assert_eq!(cut.progress[&Vec::new()], done(op));
        }
    }
    assert!(
        linked > 4,
        "the matrix exhausts and links reserved segments"
    );
    let values: BTreeMap<_, _> = metrics.values().into_iter().collect();
    assert_eq!(values["section_segment_allocations"], 0.0);

    // Short reservation: still complete and correct, but the section allocates (and says so).
    let big = vec![7u8; 3 * SEGMENT_HINT];
    let (_, allocated) = counted(&recorder, &mut slot, &mut root, done(OPS + 1), |w| {
        w.observation(&big)
    });
    assert!(allocated > 0);
    let values: BTreeMap<_, _> = metrics.values().into_iter().collect();
    assert!(values["section_segment_allocations"] >= 2.0);

    // A stop's cut while a section is open waits for it, then admits nothing more.
    let open = Arc::new(AtomicBool::new(false));
    let stopper = std::thread::spawn({
        let (recorder, open) = (recorder.clone(), open.clone());
        move || {
            wait(&open);
            recorder.stop("stopped", None).unwrap().unwrap().0
        }
    });
    root.reserve(RecordBound::records(1));
    counted(&recorder, &mut slot, &mut root, done(OPS + 2), |w| {
        open.store(true, Ordering::Release);
        std::thread::sleep(std::time::Duration::from_millis(2));
        w.load(1);
    });
    let terminal = stopper.join().unwrap();
    assert_eq!(terminal.progress[&Vec::new()], done(OPS + 2));
    root.reserve(RecordBound::records(1));
    assert!(
        recorder
            .phase(&mut slot, &mut root, done(OPS + 3), |w| w.load(2))
            .is_err()
    );
    drop(slot);
    let marked = Arc::try_unwrap(recorder).ok().unwrap().into_sink();
    // The last two phases' records differ from the matrix pattern: check the matrix cuts.
    let matrix = Marked {
        marks: marked
            .marks
            .iter()
            .copied()
            .filter(|&end| {
                tape::parse(&marked.bytes[..end])
                    .unwrap()
                    .cut
                    .get(&Vec::new())
                    .is_some_and(|at| at.op <= OPS)
            })
            .collect(),
        bytes: marked.bytes.clone(),
    };
    assert!(admitted_records_complete(&matrix, &[]) > OPS as usize / 4);
    let parsed = tape::parse(&marked.bytes).unwrap();
    let records = decode(&parsed.logs[&Vec::new()]).unwrap();
    assert_eq!(records.len(), 2 * OPS as usize + 2);
    assert_eq!(
        records[2 * OPS as usize],
        Record::Observation {
            op: OPS + 1,
            bytes: big
        }
    );
    assert_eq!(
        records.last(),
        Some(&Record::Load {
            op: OPS + 2,
            bits: 1
        })
    );
}

#[test]
fn runtime50_reserved_records_complete_under_concurrent_cuts() {
    const TASKS: usize = 4;
    const OPS: u64 = 64;
    let recorder = Arc::new(
        Recorder::<Marked>::new(
            Marked::default(),
            b"wide reserve",
            TASKS + 1,
            false,
            false,
            None,
        )
        .unwrap(),
    );
    let mut slot = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut slot).unwrap();
    let children = recorder
        .phase(&mut slot, &mut root, done(1), |w| {
            (0..TASKS).map(|_| w.spawn()).collect::<Vec<_>>()
        })
        .unwrap();
    drop(slot);
    let running = Arc::new(AtomicUsize::new(TASKS));
    let threads: Vec<_> = children
        .into_iter()
        .map(|mut task| {
            let (recorder, running) = (recorder.clone(), running.clone());
            std::thread::spawn(move || {
                let mut slot = recorder.executor_available(1).unwrap();
                for op in 1..=OPS {
                    let bytes = payload(op);
                    task.reserve(RecordBound::records(1).payload(bytes.len()));
                    let (_, allocated) = counted(&recorder, &mut slot, &mut task, done(op), |w| {
                        w.load(u128::from(op) << 100 | 0xfeed);
                        w.observation(&bytes);
                    });
                    assert_eq!(allocated, 0);
                }
                running.fetch_sub(1, Ordering::AcqRel);
                task
            })
        })
        .collect();
    let mut cuts = 0;
    while running.load(Ordering::Acquire) != 0 {
        recorder.flush().unwrap().unwrap();
        cuts += 1;
    }
    let _tasks: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    recorder.stop("returned", None).unwrap().unwrap();
    let marked = Arc::try_unwrap(recorder).ok().unwrap().into_sink();
    assert!(cuts > 0);
    for id in 0..TASKS as u32 {
        assert!(admitted_records_complete(&marked, &[id]) > 0);
    }
}

#[test]
fn runtime50_executor_slot_migration_reentry() {
    // Two compute executors, the home thread and one spare word for a nested callback.
    const SLOTS: usize = 4;
    let recorder = Arc::new(
        Recorder::<Marked>::new(
            Marked::default(),
            b"executor slots",
            SLOTS,
            false,
            true,
            None,
        )
        .unwrap(),
    );
    let mut home = recorder.executor(2).unwrap();
    let mut root = recorder.root(&mut home).unwrap();
    let mut children = recorder
        .phase(&mut home, &mut root, done(1), |w| [w.spawn(), w.spawn()])
        .unwrap()
        .into_iter();
    let (mut migrant, mut blocker) = (children.next().unwrap(), children.next().unwrap());
    // Registration is exclusive: an index has one owner until it retires.
    assert!(recorder.executor(2).is_none());

    let (to_c1, from_c0) = mpsc::channel::<Task>();
    let (to_home, from_c1) = mpsc::channel::<Task>();
    let c0_retired = Arc::new(Barrier::new(2));
    let c0 = std::thread::spawn({
        let (recorder, c0_retired) = (recorder.clone(), c0_retired.clone());
        move || {
            let mut slot = recorder.executor(0).unwrap();
            migrant.reserve(RecordBound::records(1));
            recorder
                .phase(&mut slot, &mut migrant, done(1), |w| w.load(10))
                .unwrap();
            // Migration moves the task's encoder; the physical slot stays with C0.
            to_c1.send(migrant).unwrap();
            c0_retired.wait();
            drop(slot); // retirement: inactive by construction
            c0_retired.wait();
        }
    });
    let c1 = std::thread::spawn({
        let (recorder, c0_retired) = (recorder.clone(), c0_retired.clone());
        move || {
            let mut slot = recorder.executor(1).unwrap();
            let mut migrant = from_c0.recv().unwrap();
            assert!(recorder.executor(0).is_none(), "C0 still owns its slot");
            migrant.reserve(RecordBound::records(1));
            recorder
                .phase(&mut slot, &mut migrant, done(2), |w| w.load(11))
                .unwrap();
            c0_retired.wait();
            c0_retired.wait();
            // A replacement executor reuses the retired index; C1 keeps its own.
            let replacement = std::thread::scope(|scope| {
                scope
                    .spawn(|| recorder.executor(0).map(|slot| slot.index()))
                    .join()
                    .unwrap()
            });
            assert_eq!(replacement, Some(0));
            to_home.send(migrant).unwrap();
        }
    });
    c0.join().unwrap();
    c1.join().unwrap();

    // A home callback runs the migrated task's next phase on the home executor's own slot.
    let mut migrant = from_c1.recv().unwrap();
    migrant.reserve(RecordBound::records(1));
    recorder
        .phase(&mut home, &mut migrant, done(3), |w| w.load(12))
        .unwrap();
    // Nested reentry while the outer frame has the home slot checked out: it cannot reuse
    // the active authority and registers a distinct exclusive slot.
    {
        let outer = &mut home;
        assert!(recorder.executor(outer.index()).is_none());
        let mut nested = recorder.executor_available(outer.index()).unwrap();
        assert_ne!(nested.index(), outer.index());
        migrant.reserve(RecordBound::records(1));
        recorder
            .phase(&mut nested, &mut migrant, done(4), |w| w.load(13))
            .unwrap();
        drop(nested);
        root.reserve(RecordBound::records(1));
        recorder
            .phase(outer, &mut root, done(2), |w| w.load(1))
            .unwrap();
    }

    // Terminal stop while another executor's section is open: the stop waits for that
    // section, admits its publication, and no slot opens a section afterwards.
    let in_section = Arc::new(AtomicBool::new(false));
    let (release, released) = mpsc::sync_channel::<()>(0);
    let busy = std::thread::spawn({
        let (recorder, in_section) = (recorder.clone(), in_section.clone());
        move || {
            let mut slot = recorder.executor_available(0).unwrap();
            blocker.reserve(RecordBound::records(1));
            recorder
                .phase(&mut slot, &mut blocker, done(1), |w| {
                    in_section.store(true, Ordering::Release);
                    released.recv().unwrap();
                    w.load(20);
                })
                .unwrap();
            blocker.reserve(RecordBound::records(1));
            let after = recorder.phase(&mut slot, &mut blocker, done(2), |w| w.load(21));
            (after.is_err(), slot.index())
        }
    });
    while !in_section.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    let stopper = std::thread::spawn({
        let recorder = recorder.clone();
        move || recorder.stop("stopped", None).unwrap().unwrap().0
    });
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(!stopper.is_finished(), "a stop waits for open sections");
    release.send(()).unwrap();
    let terminal = stopper.join().unwrap();
    let (refused, busy_index) = busy.join().unwrap();
    assert!(refused, "no section opens after the stop's cut");
    assert_ne!(busy_index, home.index());
    assert_eq!(terminal.progress[&vec![0]], done(4));
    assert_eq!(terminal.progress[&vec![1]], done(1));
    assert_eq!(terminal.progress[&Vec::new()], done(2));
    root.reserve(RecordBound::records(1));
    assert!(
        recorder
            .phase(&mut home, &mut root, done(3), |w| w.load(2))
            .is_err()
    );
    drop(home);

    // The migrated task's log is one stream across four executors.
    let marked = Arc::try_unwrap(recorder).ok().unwrap().into_sink();
    let parsed = tape::parse(&marked.bytes).unwrap();
    let loads: Vec<_> = decode(&parsed.logs[&vec![0]])
        .unwrap()
        .into_iter()
        .map(|r| match r {
            Record::Load { op, bits } => (op, bits),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(loads, [(1, 10), (2, 11), (3, 12), (4, 13)]);
}

/// A contended workload over two objects, recorded with or without diagnostics.
fn contended(trace: bool) -> (Vec<u8>, Option<Projection>, BTreeMap<&'static str, f64>) {
    const OPS: u64 = 24;
    let metrics = Arc::new(Metrics::default());
    let recorder = Recorder::<Marked>::new(
        Marked::default(),
        b"diagnostics",
        1,
        false,
        trace,
        Some(metrics.clone()),
    )
    .unwrap();
    let mut slot = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut slot).unwrap();
    let objects = [
        ObjectId::Made {
            path: vec![],
            op: 1,
        },
        ObjectId::Made {
            path: vec![],
            op: 2,
        },
    ];
    let mut tasks = recorder
        .phase(&mut slot, &mut root, done(1), |w| {
            w.create(objects[0].clone());
            [w.spawn(), w.spawn()]
        })
        .unwrap();
    recorder
        .phase(&mut slot, &mut root, done(2), |w| {
            w.create(objects[1].clone())
        })
        .unwrap();
    let mut versions = [1u64, 1];
    for op in 1..=OPS {
        for (id, task) in tasks.iter_mut().enumerate() {
            let object = &objects[(op as usize + id) % 2];
            task.track(object);
            task.reserve(RecordBound::records(3));
            let version = &mut versions[(op as usize + id) % 2];
            let (_, allocated) = counted(&recorder, &mut slot, task, done(op), |w| {
                *version += 1;
                w.touch(object, *version, 0, "send");
                if op % 5 == 0 {
                    w.load(u128::from(op));
                }
            });
            if !trace {
                assert_eq!(
                    allocated, 0,
                    "no diagnostic Event is built without an observer"
                );
            }
        }
    }
    let live = trace.then(|| {
        let cut = recorder.stop("returned", None).unwrap().unwrap().0;
        let mut events = root.trace().to_vec();
        for task in &tasks {
            events.extend_from_slice(task.trace());
        }
        Projection::new("returned", &cut.progress, &events)
    });
    if !trace {
        recorder.stop("returned", None).unwrap().unwrap();
    }
    drop(slot);
    let values = metrics.values().into_iter().collect();
    (recorder.into_sink().bytes, live, values)
}

/// Replay the contended tape under the concurrent decoder and the width-1 session.
fn replay_contended(bytes: &[u8]) -> (Projection, Projection) {
    let parsed = tape::parse(bytes).unwrap();
    let mut replayer = Replayer::new(&parsed).unwrap();
    let mut session = Session::replay(bytes).unwrap();
    assert!(session.start(&[]));
    let objects = [
        ObjectId::Made {
            path: vec![],
            op: 1,
        },
        ObjectId::Made {
            path: vec![],
            op: 2,
        },
    ];
    replayer.create(&[], objects[0].clone()).unwrap();
    replayer.publish(&[], done(1)).unwrap();
    replayer.create(&[], objects[1].clone()).unwrap();
    replayer.publish(&[], done(2)).unwrap();
    session.begin(&[]);
    session.create(&[], (0, 1)).unwrap();
    assert!(session.start(&[0]) && session.start(&[1]));
    session.publish(&[], Phase::Done).unwrap();
    session.begin(&[]);
    session.create(&[], (0, 2)).unwrap();
    session.publish(&[], Phase::Done).unwrap();
    let mut at: BTreeMap<Vec<u32>, Progress> = [(Vec::new(), done(2))].into();
    let mut next = [1u64; 2];
    // One permit: run whichever recorded next touch is enabled.
    while next.iter().any(|&op| op <= 24) {
        let mut moved = false;
        for id in 0..2usize {
            let op = next[id];
            let path = [id as u32];
            let object = &objects[(op as usize + id) % 2];
            let handle = (0, 1 + ((op + id as u64) % 2));
            if op <= 24 && replayer.enabled(&path, op, 0, object).unwrap() {
                replayer.touch(&path, op, 0, object, "send").unwrap();
                if op % 5 == 0 {
                    assert_eq!(replayer.load(&path, op).unwrap(), u128::from(op));
                }
                replayer.publish(&path, done(op)).unwrap();
                session.begin(&path);
                assert!(session.enabled(&path, 0, handle).unwrap());
                session.touch(&path, 0, handle, "send").unwrap();
                if op % 5 == 0 {
                    assert_eq!(session.load(&path, 0).unwrap(), u128::from(op));
                }
                session.publish(&path, Phase::Done).unwrap();
                at.insert(path.to_vec(), done(op));
                next[id] += 1;
                moved = true;
            }
        }
        assert!(moved, "the recorded prefix makes progress at one permit");
    }
    (replayer.finish(&at).unwrap(), session.finish().unwrap())
}

#[test]
fn runtime50_diagnostic_free_encoding_projection() {
    let (plain, none, plain_metrics) = contended(false);
    let (traced, live, traced_metrics) = contended(true);
    assert!(none.is_none());
    // Diagnostics change no tape byte: direct encoding is the compressed encoding.
    assert_eq!(plain, traced);
    assert_eq!(
        plain_metrics["compressed_bytes_per_contended_event"],
        traced_metrics["compressed_bytes_per_contended_event"]
    );
    assert!(plain_metrics["compressed_bytes_per_contended_event"] <= 4.0);
    assert_eq!(plain_metrics["section_segment_allocations"], 0.0);
    // The log bytes are the scratch encoder's bytes for the same records.
    let parsed = tape::parse(&plain).unwrap();
    for (path, bytes) in &parsed.logs {
        let mut encoder = Encoder::default();
        let mut again = Vec::new();
        for record in decode(bytes).unwrap() {
            encoder.encode(&record, &mut again);
        }
        assert_eq!(&again, bytes, "task {path:?}");
    }
    // The diagnostic-free tape replays, under both decoders, to the diagnosed live run.
    let live = live.unwrap();
    let (concurrent, session) = replay_contended(&plain);
    assert_eq!(concurrent, live);
    assert_eq!(session, live);
    assert!(
        live.objects.values().all(|h| !h.versions.is_empty()),
        "the projection compares object histories"
    );
}
