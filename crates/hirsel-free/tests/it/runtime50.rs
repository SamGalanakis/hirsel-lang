//! Reserved direct recording, registered executor slots and diagnostic-free encoding
//! (runtime r6 S4).

use hirsel_free::concurrent::{ExecutorSlot, Metrics, Recorder, Task};
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::projection::Projection;
use hirsel_free::record::{Encoder, ObjectId, Record, RecordBound, decode};
use hirsel_free::replay::{Admission, Replayer, Shared};
use hirsel_free::section::{INLINE, SEGMENT as SEGMENT_HINT};
use hirsel_free::session::{Marked, Session};
use hirsel_free::tape;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};

/// The test binary's allocator: counts this thread's allocations, and separately those
/// (and frees) made while a recorder section is open on it: the section trap.
pub struct Counting;
thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static IN_SECTION: Cell<bool> = const { Cell::new(false) };
    static SECTIONS: Cell<u64> = const { Cell::new(0) };
    static TRAPPED: Cell<u64> = const { Cell::new(0) };
}
fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}
fn count(n: &'static std::thread::LocalKey<Cell<u64>>) {
    let _ = n.try_with(|n| n.set(n.get() + 1));
}
fn trap() {
    if IN_SECTION.try_with(Cell::get).unwrap_or(false) {
        count(&TRAPPED);
    }
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(&ALLOCATIONS);
        trap();
        unsafe { System.alloc(layout) }
    }
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(&ALLOCATIONS);
        trap();
        unsafe { System.realloc(block, layout, size) }
    }
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        trap();
        unsafe { System.dealloc(block, layout) }
    }
}

/// The recorder's section window, on this thread.
fn section_hook(open: bool) {
    IN_SECTION.set(open);
    if open {
        count(&SECTIONS);
    }
}

/// Recorder metrics that arm the section trap.
fn trapped() -> Arc<Metrics> {
    Arc::new(Metrics::with_section_hook(section_hook))
}

/// This thread's (sections opened, allocations and frees inside them).
fn trap_counts() -> (u64, u64) {
    (SECTIONS.with(Cell::get), TRAPPED.with(Cell::get))
}

fn wait(flag: &AtomicBool) {
    while !flag.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
}

fn done(op: u64) -> Progress {
    Progress::new(op, Phase::Done)
}

/// One phase on a [`trapped`] recorder: the allocations and frees made while its section
/// was open, from admission until just before it closed.
fn counted<S: hirsel_free::recorder::TapeSink, T>(
    recorder: &Recorder<S>,
    slot: &mut ExecutorSlot,
    task: &mut Task,
    progress: Progress,
    bound: RecordBound,
    action: impl FnOnce(&mut hirsel_free::concurrent::Writer<'_, '_>) -> T,
) -> (T, u64) {
    let (sections, before) = trap_counts();
    let result = recorder.phase(slot, task, progress, bound, action).unwrap();
    let (opened, after) = trap_counts();
    assert_eq!(
        opened,
        sections + 1,
        "the section trap was armed for this phase"
    );
    (result, after - before)
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
    let metrics = trapped();
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
                let (_, allocated) =
                    counted(&recorder, &mut slot, &mut root, done(op), bound, |w| {
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
        let (_, allocated) = counted(&recorder, &mut slot, &mut root, done(op), bound, |w| {
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
    assert_eq!(values["short_reservations"], 0.0);

    // Short reservation: admission finds it before the prefix or any effect, reserves
    // outside the section and admits the same progress once. The section allocates nothing.
    let big = vec![7u8; 3 * SEGMENT_HINT];
    let before = allocations();
    let acted = Cell::new(0);
    let (_, allocated) = counted(
        &recorder,
        &mut slot,
        &mut root,
        done(OPS + 1),
        RecordBound::records(0).payload(big.len()),
        |w| {
            acted.set(acted.get() + 1);
            w.observation(&big)
        },
    );
    assert_eq!(
        allocated, 0,
        "a short reservation never allocates in its section"
    );
    assert_eq!(acted.get(), 1);
    assert!(
        allocations() - before >= 3,
        "the missing segments were reserved outside"
    );
    let values: BTreeMap<_, _> = metrics.values().into_iter().collect();
    assert_eq!(values["short_reservations"], 1.0);
    assert_eq!(
        root.progress(),
        done(OPS + 1),
        "no ordinal was consumed by the shortage"
    );

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
    counted(
        &recorder,
        &mut slot,
        &mut root,
        done(OPS + 2),
        RecordBound::records(1),
        |w| {
            open.store(true, Ordering::Release);
            std::thread::sleep(std::time::Duration::from_millis(2));
            w.load(1);
        },
    );
    let terminal = stopper.join().unwrap();
    assert_eq!(terminal.progress[&Vec::new()], done(OPS + 2));
    root.reserve(RecordBound::records(1));
    assert!(
        recorder
            .phase(
                &mut slot,
                &mut root,
                done(OPS + 3),
                RecordBound::records(1),
                |w| w.load(2)
            )
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
            Some(trapped()),
        )
        .unwrap(),
    );
    let mut slot = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut slot).unwrap();
    let mut prepared = root.prepare_children(TASKS as u32);
    let mut children = Vec::with_capacity(TASKS);
    recorder
        .phase(&mut slot, &mut root, done(1), RecordBound::default(), |w| {
            children.extend(prepared.drain(..).map(|child| w.spawn_prepared(child)));
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
                    let bound = RecordBound::records(1).payload(bytes.len());
                    task.reserve(bound);
                    let (_, allocated) =
                        counted(&recorder, &mut slot, &mut task, done(op), bound, |w| {
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
    let mut prepared = root.prepare_children(2);
    let mut children = Vec::with_capacity(2);
    recorder
        .phase(&mut home, &mut root, done(1), RecordBound::default(), |w| {
            children.extend(prepared.drain(..).map(|child| w.spawn_prepared(child)));
        })
        .unwrap();
    let mut children = children.into_iter();
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
                .phase(
                    &mut slot,
                    &mut migrant,
                    done(1),
                    RecordBound::records(1),
                    |w| w.load(10),
                )
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
                .phase(
                    &mut slot,
                    &mut migrant,
                    done(2),
                    RecordBound::records(1),
                    |w| w.load(11),
                )
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
        .phase(
            &mut home,
            &mut migrant,
            done(3),
            RecordBound::records(1),
            |w| w.load(12),
        )
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
            .phase(
                &mut nested,
                &mut migrant,
                done(4),
                RecordBound::records(1),
                |w| w.load(13),
            )
            .unwrap();
        drop(nested);
        root.reserve(RecordBound::records(1));
        recorder
            .phase(outer, &mut root, done(2), RecordBound::records(1), |w| {
                w.load(1)
            })
            .unwrap();
    }

    // Terminal stop while another executor's section is open: the stop waits for that
    // section, admits its publication, and no slot opens a section afterwards.
    let in_section = Arc::new(AtomicBool::new(false));
    // A flag, not a channel: a section allocates nothing, and a blocking receive may.
    let release = Arc::new(AtomicBool::new(false));
    let busy = std::thread::spawn({
        let (recorder, in_section, release) =
            (recorder.clone(), in_section.clone(), release.clone());
        move || {
            let mut slot = recorder.executor_available(0).unwrap();
            blocker.reserve(RecordBound::records(1));
            recorder
                .phase(
                    &mut slot,
                    &mut blocker,
                    done(1),
                    RecordBound::records(1),
                    |w| {
                        in_section.store(true, Ordering::Release);
                        wait(&release);
                        w.load(20);
                    },
                )
                .unwrap();
            blocker.reserve(RecordBound::records(1));
            let after = recorder.phase(
                &mut slot,
                &mut blocker,
                done(2),
                RecordBound::records(1),
                |w| w.load(21),
            );
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
    release.store(true, Ordering::Release);
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
            .phase(
                &mut home,
                &mut root,
                done(3),
                RecordBound::records(1),
                |w| w.load(2)
            )
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
    let metrics = trapped();
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
    let mut prepared = root.prepare_children(2);
    let mut tasks = Vec::with_capacity(2);
    root.track(&objects[0]);
    root.track(&objects[1]);
    recorder
        .phase(
            &mut slot,
            &mut root,
            done(1),
            RecordBound::records(1),
            |w| {
                w.create(&objects[0]);
                tasks.extend(prepared.drain(..).map(|child| w.spawn_prepared(child)));
            },
        )
        .unwrap();
    recorder
        .phase(
            &mut slot,
            &mut root,
            done(2),
            RecordBound::records(1),
            |w| w.create(&objects[1]),
        )
        .unwrap();
    let mut versions = [1u64, 1];
    for op in 1..=OPS {
        for (id, task) in tasks.iter_mut().enumerate() {
            let object = &objects[(op as usize + id) % 2];
            task.track(object);
            let bound = RecordBound::records(2);
            task.reserve(bound);
            let version = &mut versions[(op as usize + id) % 2];
            let (_, allocated) = counted(&recorder, &mut slot, task, done(op), bound, |w| {
                *version += 1;
                w.touch(object, *version, 0, "send");
                if op % 5 == 0 {
                    w.load(u128::from(op));
                }
            });
            assert_eq!(
                allocated, 0,
                "a section builds no diagnostic Event, traced or not (trace {trace})"
            );
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
    // Only the root's first phase was admitted without a reservation ahead: its records fit
    // the log's inline room, and with diagnostics its notes are short.
    assert_eq!(plain_metrics["short_reservations"], 0.0);
    assert_eq!(traced_metrics["short_reservations"], 1.0);
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

/// The F0 dependency traces, replayed through owner-local cursors at width and permit 1.
fn dependency_traces() -> Vec<(&'static str, crate::executor::Program)> {
    use crate::executor::{Ins::*, Program};
    vec![
        (
            "mutex-root cycle",
            Program(vec![
                vec![
                    Mutex("m"),
                    Freeze("s", vec![]),
                    Dup("s"),
                    Take("m"),
                    Spawn(1, vec!["s"]),
                    Atomic("y"),
                    Store("y", 1),
                    Thaw("s"),
                    Put("m"),
                    Join(1),
                ],
                vec![Release("s"), Take("m"), Put("m")],
            ]),
        ),
        (
            "infinite trailing segment",
            Program(vec![
                vec![
                    Freeze("s", vec![]),
                    Dup("s"),
                    Atomic("x"),
                    Spawn(1, vec!["s"]),
                    Load("x"),
                    Thaw("s"),
                ],
                vec![Store("x", 1), Release("s"), Loop],
            ]),
        ),
        (
            "parked receive",
            Program(vec![
                vec![
                    Open("c", 1),
                    Freeze("s", vec![]),
                    Dup("s"),
                    Spawn(1, vec!["s"]),
                    Atomic("y"),
                    Thaw("s"),
                    Write("out", "x"),
                    Loop,
                ],
                vec![Release("s"), Recv("c")],
            ]),
        ),
        (
            "nested phase root",
            Program(vec![
                vec![
                    Freeze("r2", vec![]),
                    Dup("r2"),
                    Freeze("r1", vec!["r2"]),
                    Dup("r1"),
                    Spawn(1, vec!["r1"]),
                    Release("r1"),
                    Join(1),
                    Thaw("r2"),
                ],
                vec![Atomic("z")],
            ]),
        ),
    ]
}

/// Rebuild `tape` with each task's log rewritten by `edit`.
fn rewrite(tape: &tape::Tape, edit: impl Fn(Vec<Record>) -> Vec<Record>) -> Vec<u8> {
    let mut bytes = tape::header(&tape.binding);
    let tasks: Vec<_> = tape
        .cut
        .iter()
        .map(|(path, progress)| {
            let records = edit(decode(tape.logs.get(path).map_or(&[][..], Vec::as_slice)).unwrap());
            let mut encoder = Encoder::default();
            let mut log = Vec::new();
            for record in &records {
                encoder.encode(record, &mut log);
            }
            tape::TaskChunk {
                path: path.clone(),
                progress: *progress,
                bytes: log,
            }
        })
        .collect();
    let end = tape.end.clone().expect("a terminal tape");
    bytes.extend(tape::stop(&tape.keys, &tasks, &end));
    bytes
}

/// Serial-gate callbacks recorded by three executors, replayed by three threads each owning
/// its task's cursor, under one permit: a pending task holds nothing and waits on its
/// dependency token.
fn serial_callbacks_with_local_cursors() {
    const TASKS: usize = 3;
    const ROUNDS: u64 = 16;
    let recorder = Arc::new(
        Recorder::<Marked>::new(
            Marked::default(),
            b"serial gate",
            TASKS + 1,
            false,
            true,
            None,
        )
        .unwrap(),
    );
    let gate = ObjectId::Gate("package:7".into());
    let mut slot = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut slot).unwrap();
    let mut prepared = root.prepare_children(TASKS as u32);
    let mut children = Vec::with_capacity(TASKS);
    recorder
        .phase(&mut slot, &mut root, done(1), RecordBound::default(), |w| {
            children.extend(prepared.drain(..).map(|child| w.spawn_prepared(child)));
        })
        .unwrap();
    drop(slot);
    // The gate's version: enter and leave each advance it, under the package's serial lock.
    let version = Arc::new(std::sync::Mutex::new(1u64));
    let threads: Vec<_> = children
        .into_iter()
        .map(|mut task| {
            let (recorder, version, gate) = (recorder.clone(), version.clone(), gate.clone());
            std::thread::spawn(move || {
                let mut slot = recorder.executor_available(1).unwrap();
                for round in 0..ROUNDS {
                    let mut held = version.lock().unwrap();
                    for (i, outcome) in ["gate enter", "gate leave"].into_iter().enumerate() {
                        task.track(&gate);
                        task.reserve(RecordBound::records(1));
                        recorder
                            .phase(
                                &mut slot,
                                &mut task,
                                done(2 * round + 1 + i as u64),
                                RecordBound::records(1),
                                |w| {
                                    *held += 1;
                                    w.touch(&gate, *held, 0, outcome);
                                },
                            )
                            .unwrap();
                    }
                    drop(held);
                    std::thread::yield_now();
                }
                task
            })
        })
        .collect();
    let tasks: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let cut = recorder.stop("returned", None).unwrap().unwrap().0;
    let mut events = root.trace().to_vec();
    for task in &tasks {
        events.extend_from_slice(task.trace());
    }
    let live = Projection::new("returned", &cut.progress, &events);
    let marked = Arc::try_unwrap(recorder).ok().unwrap().into_sink();

    // Observed or not, the replay consumes every record and reaches every cut.
    for observe in [true, false] {
        let shared = Shared::open(&tape::parse(&marked.bytes).unwrap(), observe).unwrap();
        shared.gate("package:7");
        let mut root = shared.cursor(&[]);
        root.publish(done(1)).unwrap();
        let permit = Arc::new(std::sync::Mutex::new(()));
        let pending = Arc::new(AtomicUsize::new(0));
        let admitted = Arc::new(Barrier::new(TASKS));
        let replayers: Vec<_> = (0..TASKS as u32)
            .map(|id| {
                let mut cursor = shared.cursor(&[id]);
                let (permit, gate, pending, admitted) = (
                    permit.clone(),
                    gate.clone(),
                    pending.clone(),
                    admitted.clone(),
                );
                std::thread::spawn(move || {
                    assert!(cursor.starts());
                    // Check every first callback before any predecessor publishes.
                    // This forces the pending path even on a serial host schedule.
                    let mut first = Some(cursor.admission(1, 0, &gate).unwrap());
                    admitted.wait();
                    for op in 1..=2 * ROUNDS {
                        let outcome = if op % 2 == 1 {
                            "gate enter"
                        } else {
                            "gate leave"
                        };
                        loop {
                            let held = permit.lock().unwrap();
                            match first
                                .take()
                                .unwrap_or_else(|| cursor.admission(op, 0, &gate).unwrap())
                            {
                                Admission::Ready => {
                                    cursor.touch(op, 0, &gate, outcome).unwrap();
                                    cursor.publish(done(op)).unwrap();
                                    break;
                                }
                                Admission::Pending(token) => {
                                    // Pending owns no permit: release it, then wait for the token.
                                    drop(held);
                                    pending.fetch_add(1, Ordering::Relaxed);
                                    while !token.ready() {
                                        std::thread::yield_now();
                                    }
                                }
                            }
                        }
                    }
                    cursor
                })
            })
            .collect();
        let cursors: Vec<_> = replayers.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(
            pending.load(Ordering::Relaxed) > 0,
            "some callback waited for its predecessor"
        );
        let mut all = vec![(&root, done(1))];
        all.extend(cursors.iter().map(|cursor| (cursor, done(2 * ROUNDS))));
        assert_eq!(shared.finish(all).unwrap(), observe.then(|| live.clone()));
    }
}

#[test]
fn runtime50_local_replay_dependency_progress() {
    use crate::executor::{Drive, record, replay};
    let mut gapped = 0;
    for (name, program) in dependency_traces() {
        for seed in 0..16 {
            for drive in [
                Drive::new(seed),
                Drive {
                    durable: true,
                    flush_one_in: 3,
                    ..Drive::new(seed)
                },
            ] {
                let recorded = record(&program, drive);
                // Consumes every record and reaches every cut, at width and permit 1.
                let replayed = replay(&program, &recorded.tape, seed ^ 0x5a)
                    .unwrap_or_else(|e| panic!("{name} {drive:?}: {e}"));
                assert_eq!(replayed, recorded.live, "{name} {drive:?}");
                let parsed = tape::parse(&recorded.tape).unwrap();
                let has_gap = parsed.logs.values().any(|log| {
                    decode(log)
                        .unwrap()
                        .iter()
                        .any(|r| matches!(r, Record::Gap { .. }))
                });
                if !has_gap {
                    continue;
                }
                gapped += 1;
                // An impossible predecessor (a gap no version can satisfy) is refused,
                // never left blocked.
                let impossible = rewrite(&parsed, |records| {
                    records
                        .into_iter()
                        .map(|r| match r {
                            Record::Gap { op, sub, gap } => Record::Gap {
                                op,
                                sub,
                                gap: gap + 1000,
                            },
                            other => other,
                        })
                        .collect()
                });
                assert!(
                    replay(&program, &impossible, seed).is_err(),
                    "{name}: an impossible dependency diverges"
                );
            }
        }
    }
    assert!(
        gapped > 0,
        "the traces record contended object dependencies"
    );

    // Malformed streams are refused before any task runs.
    let (program, recorded) = {
        let (_, program) = dependency_traces().remove(0);
        let recorded = record(&program, Drive::new(3));
        (program, recorded)
    };
    let parsed = tape::parse(&recorded.tape).unwrap();
    let past_cut = rewrite(&parsed, |mut records| {
        let op = records.last().map_or(0, Record::op) + 100;
        records.push(Record::Load { op, bits: 1 });
        records
    });
    let error = replay(&program, &past_cut, 0).unwrap_err();
    assert!(error.0.contains("past its cut"), "{error}");
    assert!(Shared::new(&tape::parse(&past_cut).unwrap()).is_err());
    let mut torn = parsed.clone();
    for log in torn.logs.values_mut() {
        if !log.is_empty() {
            log.push(0xff);
        }
    }
    assert!(
        Shared::new(&torn).is_err(),
        "an undecodable stream is refused"
    );
    let mut orphan = parsed.clone();
    let mut log = Vec::new();
    Encoder::default().encode(&Record::Load { op: 1, bits: 1 }, &mut log);
    orphan.logs.insert(vec![9], log);
    assert!(
        Shared::new(&orphan).is_err(),
        "records of a task outside the cut"
    );

    serial_callbacks_with_local_cursors();
}

/// A root that records phases on a trapped recorder and mirrors its log with a scratch
/// encoder, so a test knows where each record ends against segment boundaries.
struct Mirrored {
    recorder: Arc<Recorder<Marked>>,
    metrics: Arc<Metrics>,
    slot: ExecutorSlot,
    root: Task,
    op: u64,
    encoder: Encoder,
    log: Vec<u8>,
    records: Vec<Record>,
}

impl Mirrored {
    fn new(trace: bool) -> Mirrored {
        let metrics = trapped();
        let recorder = Arc::new(
            Recorder::<Marked>::new(
                Marked::default(),
                b"section boundaries",
                1,
                false,
                trace,
                Some(metrics.clone()),
            )
            .unwrap(),
        );
        let mut slot = recorder.executor(0).unwrap();
        let root = recorder.root(&mut slot).unwrap();
        Mirrored {
            recorder,
            metrics,
            slot,
            root,
            op: 0,
            encoder: Encoder::default(),
            log: Vec::new(),
            records: Vec::new(),
        }
    }

    /// One phase admitted with `bound`, reserved ahead or not, that writes `records`. A cut
    /// follows each, so the cutter frees every segment the phase filled.
    fn phase(
        &mut self,
        bound: RecordBound,
        reserve: bool,
        records: impl FnOnce(u64) -> Vec<Record>,
        act: impl FnOnce(&mut hirsel_free::concurrent::Writer<'_, '_>),
    ) {
        self.op += 1;
        let op = self.op;
        if reserve {
            self.root.reserve(bound);
        }
        let acted = Cell::new(0);
        let (_, trapped) = counted(
            &self.recorder,
            &mut self.slot,
            &mut self.root,
            done(op),
            bound,
            |w| {
                acted.set(acted.get() + 1);
                act(w);
            },
        );
        assert_eq!(
            trapped, 0,
            "op {op} (reserved ahead: {reserve}) allocated or freed in its section"
        );
        assert_eq!(acted.get(), 1, "op {op} acted exactly once");
        for record in records(op) {
            self.encoder.encode(&record, &mut self.log);
            self.records.push(record);
        }
        self.recorder.flush().unwrap().unwrap();
    }

    /// The payload length whose record (tag, one-byte op delta, length varint, payload) ends
    /// `at` bytes from a storage end (the inline room's or a segment's), `segments` segments
    /// on.
    fn payload_ending(&self, at: i64, segments: usize) -> usize {
        let room = to_boundary(self.log.len());
        let mut end = (room as i64 + at) as usize + segments * SEGMENT_HINT;
        if end < 4 + 128 {
            end += SEGMENT_HINT; // a two-byte length varint
        }
        end - 4
    }

    /// Stop, then check the tape's log is the mirror byte for byte. Returns the tape, the
    /// trace and the short-reservation count.
    fn finish(self) -> (Vec<u8>, Vec<hirsel_free::projection::Stamped>, f64) {
        let terminal = self.recorder.stop("returned", None).unwrap().unwrap().0;
        assert_eq!(terminal.progress[&Vec::new()], done(self.op));
        drop(self.slot);
        let short = self
            .metrics
            .values()
            .into_iter()
            .find(|(name, _)| *name == "short_reservations")
            .unwrap()
            .1;
        let marked = Arc::try_unwrap(self.recorder).ok().unwrap().into_sink();
        let parsed = tape::parse(&marked.bytes).unwrap();
        let log = &parsed.logs[&Vec::new()];
        assert_eq!(
            log, &self.log,
            "the log is the scratch encoding, byte for byte"
        );
        assert_eq!(decode(log).unwrap(), self.records);
        (marked.bytes, self.root.trace().to_vec(), short)
    }
}

/// The bytes from log length `len` to the end of its storage: the task's inline room, then
/// each segment.
fn to_boundary(len: usize) -> usize {
    if len < INLINE {
        INLINE - len
    } else {
        SEGMENT_HINT - (len - INLINE) % SEGMENT_HINT
    }
}

/// rr declines a buffered syscall before its effect when the record would not fit
/// (src/preload/syscallbuf.c:1309-1333), and block_clone_syscallbuf_overflow.c:5-36 moves
/// blocks exactly the buffer's size, checking both ends. Here, on one segment size: a first
/// touch with the largest gap, every fixed kind at its largest fields, a first key position,
/// an output note, and payloads ending one byte under, exactly at and one byte over a
/// segment end, two segments on, and empty; each admitted reserved ahead and not.
fn boundary_vectors(trace: bool) -> (Vec<u8>, Vec<hirsel_free::projection::Event>, f64) {
    use hirsel_free::projection::{Event, Observed};
    let mut run = Mirrored::new(trace);
    let mut events = Vec::new();
    let object = ObjectId::Made {
        path: vec![u32::MAX; 3],
        op: u64::MAX,
    };
    run.root.track(&object);
    run.phase(
        RecordBound::records(1),
        false,
        |op| {
            vec![Record::Gap {
                op,
                sub: u32::MAX,
                gap: u64::MAX - 2,
            }]
        },
        |w| w.touch(&object, u64::MAX, u32::MAX, "first touch"),
    );
    events.push(Event::Versioned {
        object: object.clone(),
        version: u64::MAX,
        sub: u32::MAX,
        outcome: "first touch".into(),
    });
    run.phase(
        RecordBound::records(5),
        true,
        |op| {
            vec![
                Record::Load {
                    op,
                    bits: u128::MAX,
                },
                Record::Cancel {
                    op,
                    check: 0,
                    cause: Some(u32::MAX),
                },
                Record::AwaitAny {
                    op,
                    index: u64::MAX,
                },
                Record::Expired { op },
                Record::Thaw { op, ok: true },
            ]
        },
        |w| {
            w.load(u128::MAX);
            w.cancel(Some(u32::MAX));
            w.await_any(u64::MAX);
            w.expired(true);
            w.thaw(true);
        },
    );
    events.extend(
        [
            Observed::Load(u128::MAX),
            Observed::Cancel(Some(u32::MAX)),
            Observed::AwaitAny(u64::MAX),
            Observed::Expired(true),
            Observed::Thaw(true),
        ]
        .map(Event::Observed),
    );
    let name = "file:/a/key/name/longer/than/one/outcome/label";
    run.root.track_key(u32::MAX);
    run.phase(
        RecordBound::default().key(name.len()),
        false,
        |op| {
            vec![Record::Key {
                op,
                key: u32::MAX,
                shared: true,
                gap: u64::MAX,
            }]
        },
        |w| w.key(u32::MAX, name, u64::MAX, true, u32::MAX),
    );
    events.push(Event::Key {
        key: name.into(),
        epoch: u64::MAX,
        index: u32::MAX,
    });
    let written = payload(5);
    run.phase(
        RecordBound::default().output("stdout".len(), written.len()),
        false,
        |_| Vec::new(),
        |w| w.output("stdout", 9, &written),
    );
    events.push(Event::Output {
        sink: "stdout".into(),
        epoch: 9,
        bytes: written.clone(),
    });
    for reserve in [true, false] {
        for (at, segments) in [(-1i64, 0), (0, 0), (1, 0), (0, 2)] {
            let len = run.payload_ending(at, segments);
            let bytes: Vec<u8> = (0..len).map(|i| (i * 31 + len) as u8).collect();
            let record = |op| {
                vec![Record::Observation {
                    op,
                    bytes: bytes.clone(),
                }]
            };
            run.phase(RecordBound::default().payload(len), reserve, record, |w| {
                w.observation(&bytes)
            });
            assert_eq!(
                to_boundary(run.log.len()) as i64,
                if at < 0 {
                    -at
                } else {
                    SEGMENT_HINT as i64 - at
                },
                "the record ends {at} from a segment end"
            );
        }
        run.phase(
            RecordBound::default().payload(0),
            reserve,
            |op| vec![Record::Observation { op, bytes: vec![] }],
            |w| w.observation(&[]),
        );
    }
    let (bytes, stamped, short) = run.finish();
    let observed: Vec<_> = stamped.into_iter().map(|stamped| stamped.event).collect();
    if trace {
        assert_eq!(
            observed, events,
            "each phase's notes, in order, as its events"
        );
    } else {
        assert!(observed.is_empty());
    }
    (bytes, observed, short)
}

#[test]
fn record51_boundary_vectors_allocate_nothing_in_sections() {
    let (plain, _, plain_short) = boundary_vectors(false);
    let (traced, trace, traced_short) = boundary_vectors(true);
    assert_eq!(trace.len(), 8);
    assert_eq!(plain, traced, "diagnostics change no tape byte");
    // The unreserved first phase fits the log's inline room; only its notes are short.
    assert_eq!(plain_short + 1.0, traced_short);
    assert!(
        plain_short >= 3.0,
        "unreserved phases took the short path: {plain_short}"
    );
}

/// block_clone_syscallbuf_overflow.c:5-36 at the inline room's end: payload records ending
/// one byte under, exactly at and one byte over it, each followed by a fixed record and a
/// payload that spans a segment, reserved ahead and not. The task's first phases need no
/// segment; the tape mirrors the scratch encoding byte for byte and no section allocates.
#[test]
fn record53_inline_room_boundary_vectors() {
    for reserve in [true, false] {
        for at in [-1i64, 0, 1] {
            let mut run = Mirrored::new(false);
            let first = run.root.reserve(RecordBound::records(4));
            assert_eq!(first.allocated, 0, "a first phase fits the inline room");
            run.phase(
                RecordBound::records(1),
                reserve,
                |op| vec![Record::Thaw { op, ok: true }],
                |w| w.thaw(true),
            );
            let len = run.payload_ending(at, 0);
            assert!(len >= 128 && run.log.len() + 4 + len == (INLINE as i64 + at) as usize);
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + len) as u8).collect();
            run.phase(
                RecordBound::default().payload(len),
                reserve,
                |op| {
                    vec![Record::Observation {
                        op,
                        bytes: bytes.clone(),
                    }]
                },
                |w| w.observation(&bytes),
            );
            run.phase(
                RecordBound::records(1),
                reserve,
                |op| {
                    vec![Record::Load {
                        op,
                        bits: u128::MAX,
                    }]
                },
                |w| w.load(u128::MAX),
            );
            let spans: Vec<u8> = (0..SEGMENT_HINT).map(|i| i as u8).collect();
            run.phase(
                RecordBound::default().payload(spans.len()),
                reserve,
                |op| {
                    vec![Record::Observation {
                        op,
                        bytes: spans.clone(),
                    }]
                },
                |w| w.observation(&spans),
            );
            run.finish();
        }
    }
}

/// Nested reentry across the inline end: the root reserves a payload that crosses its inline
/// room on the outer slot, then a nested slot on the same thread runs a child's phases across
/// the child's inline end before the outer phase is admitted. The outer reservation is still
/// whole (no short path), no section allocates, the nested word retires first, and the tape
/// holds both logs exactly.
#[test]
fn record53_nested_reentry_keeps_reserved_room() {
    let metrics = trapped();
    let recorder = Recorder::<Marked>::new(
        Marked::default(),
        b"nested inline",
        3,
        false,
        false,
        Some(metrics.clone()),
    )
    .unwrap();
    let mut outer = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut outer).unwrap();
    let child = root.prepare_child();
    let (mut child, trapped) = counted(
        &recorder,
        &mut outer,
        &mut root,
        done(1),
        RecordBound::records(1),
        |w| w.spawn_prepared(child),
    );
    assert_eq!(trapped, 0);
    let big: Vec<u8> = (0..INLINE * 2).map(|i| i as u8).collect();
    let outer_bound = RecordBound::default().payload(big.len());
    assert_eq!(root.reserve(outer_bound).allocated, 1);
    let mut nested = recorder.executor_available(outer.index()).unwrap();
    let mut expected = Vec::new();
    let mut encoder = Encoder::default();
    for op in 1..=4u64 {
        let bytes = vec![op as u8; INLINE / 3];
        let bound = RecordBound::default().payload(bytes.len());
        let ((), trapped) = counted(&recorder, &mut nested, &mut child, done(op), bound, |w| {
            w.observation(&bytes)
        });
        assert_eq!(trapped, 0, "nested op {op} allocated in its section");
        encoder.encode(&Record::Observation { op, bytes }, &mut expected);
    }
    assert!(
        expected.len() > INLINE,
        "the child's log crossed its inline end"
    );
    recorder.retire_executor(nested);
    let ((), trapped) = counted(
        &recorder,
        &mut outer,
        &mut root,
        done(2),
        outer_bound,
        |w| w.observation(&big),
    );
    assert_eq!(trapped, 0);
    let values: BTreeMap<_, _> = metrics.values().into_iter().collect();
    assert_eq!(
        values["short_reservations"], 1.0,
        "only the child's unreserved crossing"
    );
    recorder.stop("returned", None).unwrap().unwrap();
    drop(outer);
    let parsed = tape::parse(&recorder.into_sink().bytes).unwrap();
    assert_eq!(parsed.logs[&vec![0]], expected);
    assert_eq!(
        decode(&parsed.logs[&Vec::new()]).unwrap(),
        [Record::Observation { op: 2, bytes: big }]
    );
}

/// A child's log, path and reserved room take one allocation: a spawn chain eight deep (two
/// past the inline path) keeps each path, and a short child's phases link no segment.
#[test]
fn record53_child_log_is_one_allocation() {
    let recorder = Recorder::<Marked>::new(
        Marked::default(),
        b"spawn chain",
        1,
        false,
        false,
        Some(trapped()),
    )
    .unwrap();
    let mut slot = recorder.executor(0).unwrap();
    let mut task = recorder.root(&mut slot).unwrap();
    let mut spawn_at = 1;
    let mut chain = Vec::new();
    for depth in 1..=8u32 {
        let before = allocations();
        let child = task.prepare_child();
        let made = allocations() - before;
        assert!(
            made <= if depth > 6 { 2 } else { 1 },
            "depth {depth}: preparing a child made {made} allocations"
        );
        let (mut child, trapped) = counted(
            &recorder,
            &mut slot,
            &mut task,
            done(spawn_at),
            RecordBound::records(1),
            |w| w.spawn_prepared(child),
        );
        assert_eq!(trapped, 0);
        assert_eq!(child.path(), vec![0; depth as usize]);
        for op in 1..=2 {
            let reservation = child.reserve(RecordBound::records(4));
            assert_eq!(reservation.allocated, 0, "a short child links no segment");
            let ((), trapped) = counted(
                &recorder,
                &mut slot,
                &mut child,
                done(op),
                RecordBound::records(4),
                |w| w.load(u128::from(op)),
            );
            assert_eq!(trapped, 0);
        }
        spawn_at = 3;
        chain.push(std::mem::replace(&mut task, child));
    }
    let (cut, _) = recorder.stop("returned", None).unwrap().unwrap();
    assert_eq!(cut.progress.len(), 9);
    assert_eq!(cut.progress[&vec![0; 8]], done(2));
    drop(slot);
    let parsed = tape::parse(&recorder.into_sink().bytes).unwrap();
    for depth in 1..=8 {
        let log = decode(&parsed.logs[&vec![0; depth]]).unwrap();
        assert_eq!(
            log,
            [1, 2].map(|op| Record::Load {
                op,
                bits: u128::from(op)
            })
        );
    }
}

/// Tasks touch one object on their own threads, first touch tracked but nothing reserved
/// ahead, while a cutter flushes and then stops mid-run. Every section stays clean, and the
/// observer's projection, which phases publish after their sections, equals both the
/// traces' projection of the terminal cut and its one-permit replay.
#[test]
fn record51_observer_projection_closes_over_racing_cuts() {
    use hirsel_free::concurrent::Observer;
    const TASKS: usize = 4;
    const OPS: u64 = 400;
    let observer = Arc::new(Observer::default());
    let mut recorder = Recorder::<Marked>::new(
        Marked::default(),
        b"observed cut",
        TASKS + 1,
        false,
        true,
        Some(trapped()),
    )
    .unwrap();
    recorder.observe(observer.clone());
    let recorder = Arc::new(recorder);
    let mut slot = recorder.executor(0).unwrap();
    let mut root = recorder.root(&mut slot).unwrap();
    let object = ObjectId::Made {
        path: vec![],
        op: 1,
    };
    // Children, their slots and the created object are all prepared outside the section.
    let mut prepared = root.prepare_children(TASKS as u32);
    let mut tasks = Vec::with_capacity(TASKS);
    root.track(&object);
    let ((), trapped) = counted(
        &recorder,
        &mut slot,
        &mut root,
        done(1),
        RecordBound::records(1),
        |w| {
            w.create(&object);
            for child in prepared.drain(..) {
                tasks.push(w.spawn_prepared(child));
            }
        },
    );
    assert_eq!(
        trapped, 0,
        "creation and spawns allocate nothing in their section"
    );
    drop(slot);
    let version = Arc::new(std::sync::Mutex::new(1u64));
    let halfway = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = tasks
        .into_iter()
        .map(|mut task| {
            let (recorder, version, object, halfway) = (
                recorder.clone(),
                version.clone(),
                object.clone(),
                halfway.clone(),
            );
            std::thread::spawn(move || {
                let mut slot = recorder.executor_available(1).unwrap();
                task.track(&object);
                let mut published = 0;
                for op in 1..=OPS {
                    let mut held = version.lock().unwrap();
                    let (sections, before) = trap_counts();
                    let phase = recorder.phase(
                        &mut slot,
                        &mut task,
                        done(op),
                        RecordBound::records(2),
                        |w| {
                            *held += 1;
                            w.touch(&object, *held, 0, "send");
                            w.load(u128::from(op));
                        },
                    );
                    drop(held);
                    if phase.is_err() {
                        break;
                    }
                    let (opened, after) = trap_counts();
                    assert_eq!(opened, sections + 1);
                    assert_eq!(after, before, "op {op} allocated or freed in its section");
                    published = op;
                    if op == OPS / 2 {
                        halfway.fetch_add(1, Ordering::AcqRel);
                    }
                }
                (task, published)
            })
        })
        .collect();
    let mut flushes = 0;
    while halfway.load(Ordering::Acquire) < TASKS {
        recorder.flush().unwrap().unwrap();
        flushes += 1;
    }
    // The terminal cut lands while phases are still being admitted and publishing notes;
    // the projection is read before any worker is joined.
    let cut = recorder.stop("stopped", None).unwrap().unwrap().0;
    let projection = recorder.projection().unwrap();
    let tasks: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(flushes > 0);
    let mut trace = root.trace().to_vec();
    for (id, (task, published)) in tasks.iter().enumerate() {
        assert_eq!(cut.progress[&vec![id as u32]], done(*published));
        trace.extend_from_slice(task.trace());
    }
    assert_eq!(
        projection,
        Projection::new("stopped", &cut.progress, &trace),
        "the observer's projection waits for every admitted phase's notes"
    );
    let history = &projection.objects[&object];
    assert_eq!(
        history.versions.keys().copied().collect::<Vec<_>>(),
        (1..=history.versions.len() as u64).collect::<Vec<_>>(),
        "the terminal cut admits a closed object prefix"
    );
    assert!(history.versions.len() > TASKS * OPS as usize / 2);
    let marked = Arc::try_unwrap(recorder).ok().unwrap().into_sink();
    let parsed = tape::parse(&marked.bytes).unwrap();
    let mut replayer = Replayer::new(&parsed).unwrap();
    replayer.create(&[], object.clone()).unwrap();
    replayer.publish(&[], done(1)).unwrap();
    let mut at: BTreeMap<Vec<u32>, Progress> = [(Vec::new(), done(1))].into();
    let mut next = [1u64; TASKS];
    let last: Vec<u64> = tasks.iter().map(|(_, published)| *published).collect();
    while (0..TASKS).any(|id| next[id] <= last[id]) {
        let mut moved = false;
        for id in 0..TASKS {
            let (op, path) = (next[id], [id as u32]);
            if op <= last[id] && replayer.enabled(&path, op, 0, &object).unwrap() {
                replayer.touch(&path, op, 0, &object, "send").unwrap();
                assert_eq!(replayer.load(&path, op).unwrap(), u128::from(op));
                replayer.publish(&path, done(op)).unwrap();
                at.insert(path.to_vec(), done(op));
                next[id] += 1;
                moved = true;
            }
        }
        assert!(moved, "the admitted prefix makes progress at one permit");
    }
    assert_eq!(replayer.finish(&at).unwrap(), projection);
}

fn untrapped(binding: &[u8]) -> (Recorder<Marked>, ExecutorSlot, Task) {
    let recorder =
        Recorder::<Marked>::new(Marked::default(), binding, 1, false, false, None).unwrap();
    let mut slot = recorder.executor(0).unwrap();
    let root = recorder.root(&mut slot).unwrap();
    (recorder, slot, root)
}

#[test]
#[should_panic(expected = "tracked before the section of its first touch")]
fn record51_an_untracked_first_touch_fails_instead_of_inserting() {
    let (recorder, mut slot, mut root) = untrapped(b"untracked");
    let object = ObjectId::Gate("package:1".into());
    let _ = recorder.phase(
        &mut slot,
        &mut root,
        done(1),
        RecordBound::records(1),
        |w| w.touch(&object, 2, 0, "gate enter"),
    );
}

#[test]
#[should_panic(expected = "tracked before the section of its first position")]
fn record51_an_untracked_key_fails_instead_of_inserting() {
    let (recorder, mut slot, mut root) = untrapped(b"untracked key");
    let _ = recorder.phase(
        &mut slot,
        &mut root,
        done(1),
        RecordBound::default().key(4),
        |w| w.key(3, "file", 1, false, 0),
    );
}

#[test]
#[should_panic(expected = "record bytes past its bound")]
fn record51_a_write_past_its_bound_fails_even_with_spare_room() {
    let (recorder, mut slot, mut root) = untrapped(b"past bound");
    // Room for many records is reserved, but this phase declared none.
    root.reserve(RecordBound::records(8));
    let _ = recorder.phase(&mut slot, &mut root, done(1), RecordBound::default(), |w| {
        w.load(1)
    });
}

#[test]
#[should_panic(expected = "noted past its bound")]
fn record51_an_unbounded_note_fails_with_diagnostics_off() {
    let (recorder, mut slot, mut root) = untrapped(b"note bound");
    let _ = recorder.phase(&mut slot, &mut root, done(1), RecordBound::default(), |w| {
        w.output("stdout", 1, b"unbounded")
    });
}
