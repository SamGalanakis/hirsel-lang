use super::*;
use hirsel_free::phase::Progress;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[test]
fn native_atomic_replay_substitutes_loads_and_stops_before_the_cut_suffix() {
    for durable in [false, true] {
        let recording = super::super::record::Recording::new(b"native replay", durable);
        let install = recording.clone();
        let inspect = recording.clone();
        let mut run = crate::host::BodyRun::new(8, true);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
                install.recorder.flush().unwrap().unwrap();
            },
            move |ctx| unsafe {
                let mut word = 0;
                hirsel_rt_v2_atomic_create(
                    ctx,
                    std::ptr::null_mut(),
                    &mut word,
                    (&0u64 as *const u64).cast(),
                    8,
                );
                hirsel_rt_v2_atomic_store(ctx, &word, (&7u64 as *const u64).cast(), 8);
                let mut loaded = 0u64;
                hirsel_rt_v2_atomic_load(ctx, &word, (&raw mut loaded).cast(), 8);
                assert_eq!(loaded, 7);
                inspect.recorder.flush().unwrap().unwrap();
                hirsel_rt_v2_atomic_store(ctx, &word, (&9u64 as *const u64).cast(), 8);
                hirsel_rt_v2_atomic_load(ctx, &word, (&raw mut loaded).cast(), 8);
                assert_eq!(loaded, 9);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        let trace = recording.trace();
        assert_eq!(cut.progress[&Vec::new()], Progress::new(5, Phase::Done));
        let marked = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        let flush = if durable {
            marked.marks[2]
        } else {
            // Non-durable tapes share the exact framing; find the first cut.
            (1..marked.bytes.len())
                .find(|&end| {
                    hirsel_free::tape::parse(&marked.bytes[..end]).is_ok_and(|tape| {
                        tape.cut.get(&Vec::new()) == Some(&Progress::new(3, Phase::Done))
                    })
                })
                .unwrap()
        };
        let start = (1..flush)
            .find(|&end| {
                hirsel_free::tape::parse(&marked.bytes[..end])
                    .is_ok_and(|tape| tape.cut.get(&Vec::new()) == Some(&Progress::START))
            })
            .unwrap();
        for end in [marked.bytes.len(), flush, flush + 1, start] {
            let tape = &marked.bytes[..end];
            let parsed = hirsel_free::tape::parse(tape).unwrap();
            let expected = Projection::new(
                if end == marked.bytes.len() {
                    "returned"
                } else {
                    "truncated"
                },
                &parsed.cut,
                &trace,
            );
            for width in [2, 8] {
                let replay = Replay::new(tape).unwrap();
                let install = replay.clone();
                let outputs = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
                let observed = outputs.clone();
                let entered = Arc::new(AtomicBool::new(false));
                let initial = entered.clone();
                let suffix = Arc::new(AtomicBool::new(false));
                let trailing = suffix.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe { install.root(ctx) },
                    move |ctx| unsafe {
                        initial.store(true, Ordering::Release);
                        let mut word = 0;
                        hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut word,
                            (&0u64 as *const u64).cast(),
                            8,
                        );
                        // Different live cells prove loads use the recording.
                        hirsel_rt_v2_atomic_store(ctx, &word, (&700u64 as *const u64).cast(), 8);
                        hirsel_rt_v2_atomic_load(ctx, &word, observed[0].as_ptr().cast(), 8);
                        hirsel_rt_v2_atomic_store(ctx, &word, (&900u64 as *const u64).cast(), 8);
                        hirsel_rt_v2_atomic_load(ctx, &word, observed[1].as_ptr().cast(), 8);
                        trailing.store(true, Ordering::Release);
                        STATUS_OK
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(
                    entered.load(Ordering::Acquire),
                    end != start,
                    "a never-started root runs no initial segment"
                );
                assert_eq!(
                    outputs[0].load(Ordering::Acquire),
                    if end == start { 0 } else { 7 }
                );
                assert_eq!(
                    outputs[1].load(Ordering::Acquire),
                    if end == marked.bytes.len() { 9 } else { 0 }
                );
                assert!(!suffix.load(Ordering::Acquire));
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}
