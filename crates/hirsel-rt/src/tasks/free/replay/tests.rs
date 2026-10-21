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

unsafe extern "C" fn child(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    let divergent = unsafe { crate::alloc::payload(environment).cast::<u64>().read() };
    if divergent != 0 {
        panic!("a queued child absent from the replay cut entered user code");
    }
    // Reaching its administrative end is sufficient for this prefix. Shared
    // object/cancellation outcome replay is admitted by later slices.
    let _ = ctx;
    STATUS_OK
}

#[test]
fn native_task_replay_never_starts_a_queued_divergent_child_and_finishes_started_children() {
    const CHILD: &str = "HIRSEL_F1_REPLAY_TASK_START";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::replay::tests::native_task_replay_never_starts_a_queued_divergent_child_and_finishes_started_children", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    };
    let layouts = [&raw const layout];
    for durable in [false, true] {
        for divergent in [false, true] {
            let recording = super::super::record::Recording::new(b"native task replay", durable);
            let install = recording.clone();
            let cutter = recording.clone();
            let mut run = crate::host::BodyRun::new(8, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let control = run.control();
            let stop = control.clone();
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let environment = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(environment)
                        .cast::<u64>()
                        .write(u64::from(divergent));
                    let mut task = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut task,
                        &scope,
                        child,
                        environment,
                        0,
                    );
                    if divergent {
                        let cut = cutter.recorder.stop("stopped", None).unwrap().unwrap().0;
                        assert_eq!(cut.progress[&vec![0]], Progress::START);
                        stop.stop();
                        let mut canceled = 0;
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                        panic!("a stopped recording resumed");
                    }
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                    STATUS_OK
                },
            );
            if divergent {
                assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
            } else {
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                recording.recorder.stop("returned", None).unwrap().unwrap();
            }
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while Arc::strong_count(&recording) != 1 {
                assert!(std::time::Instant::now() < until, "recorded frames reclaim");
                std::thread::yield_now();
            }
            let trace = recording.trace();
            let bytes = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes;
            let parsed = hirsel_free::tape::parse(&bytes).unwrap();
            let expected = Projection::new(
                if divergent { "stopped" } else { "returned" },
                &parsed.cut,
                &trace,
            );
            for width in [2, 8] {
                let replay = Replay::new(&bytes).unwrap();
                let install = replay.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let suffix = Arc::new(AtomicBool::new(false));
                let trailing = suffix.clone();
                let outcome = run.run(
                    move |ctx| unsafe { install.root(ctx) },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                        let environment = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(environment)
                            .cast::<u64>()
                            .write(u64::from(divergent));
                        let mut task = 0;
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut task,
                            &scope,
                            child,
                            environment,
                            0,
                        );
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                        trailing.store(true, Ordering::Release);
                        STATUS_OK
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert!(!suffix.load(Ordering::Acquire));
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}

struct RestProbe {
    recording: Option<Arc<super::super::record::Recording>>,
    ran: AtomicBool,
}
unsafe extern "C" fn held_child(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    let probe = unsafe {
        &*(crate::alloc::payload(environment)
            .cast::<*const RestProbe>()
            .read())
    };
    let mut result = [1u64, 7];
    let none = [0u64; 2];
    unsafe { hirsel_rt_v2_task_finish(ctx, result.as_mut_ptr().cast(), none.as_ptr().cast(), 16) };
    if let Some(recording) = &probe.recording {
        let cut = recording.recorder.flush().unwrap().unwrap();
        assert_eq!(cut.progress[&Vec::new()], Progress::new(3, Phase::Entered));
        assert_eq!(cut.progress[&vec![0]], Progress::new(1, Phase::Done));
    }
    probe.ran.store(true, Ordering::Release);
    let mut word = 0;
    let mut loaded = 0u64;
    unsafe {
        hirsel_rt_v2_atomic_create(
            ctx,
            std::ptr::null_mut(),
            &mut word,
            (&0u64 as *const u64).cast(),
            8,
        );
        hirsel_rt_v2_atomic_store(ctx, &word, (&7u64 as *const u64).cast(), 8);
        hirsel_rt_v2_atomic_load(ctx, &word, (&raw mut loaded).cast(), 8);
    }
    assert_eq!(loaded, 7);
    STATUS_OK
}

#[test]
fn native_called_rests_run_only_when_the_child_cut_admits_the_rest() {
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    };
    let layouts = [&raw const layout];
    for durable in [false, true] {
        let recording = super::super::record::Recording::new(b"called rest replay", durable);
        let install = recording.clone();
        let probe = Arc::new(RestProbe {
            recording: Some(recording.clone()),
            ran: AtomicBool::new(false),
        });
        let live = probe.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                rest_body(ctx, &live);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert!(probe.ran.load(Ordering::Acquire));
        drop(probe);
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let trace = recording.trace();
        let marked = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        let flush = (1..marked.bytes.len())
            .find(|&end| {
                hirsel_free::tape::parse(&marked.bytes[..end]).is_ok_and(|tape| {
                    tape.cut.get(&Vec::new()) == Some(&Progress::new(3, Phase::Entered))
                })
            })
            .unwrap();
        for end in [marked.bytes.len(), flush, flush + 1] {
            let parsed = hirsel_free::tape::parse(&marked.bytes[..end]).unwrap();
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
                let replay = Replay::new(&marked.bytes[..end]).unwrap();
                let install = replay.clone();
                let probe = Arc::new(RestProbe {
                    recording: None,
                    ran: AtomicBool::new(false),
                });
                let seen = probe.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe { install.root(ctx) },
                    move |ctx| unsafe {
                        rest_body(ctx, &seen);
                        panic!("scope exit resumed after its cut");
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(probe.ran.load(Ordering::Acquire), end == marked.bytes.len());
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}

unsafe fn rest_body(ctx: *mut TaskCtx, probe: &Arc<RestProbe>) {
    let mut scope = 0;
    unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0) };
    let environment =
        unsafe { crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0) };
    unsafe {
        crate::alloc::payload(environment)
            .cast::<*const RestProbe>()
            .write(Arc::as_ptr(probe))
    };
    let mut task = 0;
    unsafe {
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut task,
            &scope,
            held_child,
            environment,
            0,
        );
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}
