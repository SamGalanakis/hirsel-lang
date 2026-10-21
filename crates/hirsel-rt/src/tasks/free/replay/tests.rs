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

#[test]
fn native_thaw_replay_forces_a_recorded_failure_even_when_the_root_is_sole() {
    let scalar = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 1,
        flags: crate::LAYOUT_FROZEN,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    };
    let layouts = [&raw const scalar];
    for durable in [false, true] {
        let recording = super::super::record::Recording::new(b"native thaw replay", durable);
        let install = recording.clone();
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
            |ctx| unsafe {
                let root =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(root).cast::<u64>().write(7);
                crate::roots::hirsel_rt_v2_root_dup(ctx, root);
                let mut last = 99;
                crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root);
                assert_eq!(last, 0);
                // Fixture disposal after observing the op, without inventing
                // another semantic boundary in the tape.
                crate::roots::immediate(ctx, || {
                    crate::roots::hirsel_rt_v2_root_release(ctx, root);
                    crate::roots::hirsel_rt_v2_root_release(ctx, root);
                });
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
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        for width in [2, 8] {
            let replay = Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let observed = Arc::new(std::sync::atomic::AtomicU8::new(99));
            let out = observed.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    let root = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(root).cast::<u64>().write(7);
                    assert!(crate::roots::sole(root));
                    crate::shared::hirsel_rt_v2_thaw_turn(ctx, out.as_ptr(), root);
                    panic!("a thaw replay passed its cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert_eq!(observed.load(Ordering::Acquire), 0);
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

unsafe extern "C" fn release_child(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    let root = unsafe {
        crate::alloc::payload(environment)
            .cast::<*mut crate::Header>()
            .read()
    };
    unsafe { crate::roots::hirsel_rt_v2_root_release(ctx, root) };
    STATUS_OK
}

#[test]
fn a_recorded_successful_native_thaw_saves_its_wait_until_another_task_releases() {
    const CHILD: &str = "HIRSEL_F1_REPLAY_THAW_WAIT";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::replay::tests::a_recorded_successful_native_thaw_saves_its_wait_until_another_task_releases", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    let scalar = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 1,
        flags: crate::LAYOUT_FROZEN,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    };
    let environment = crate::Layout { flags: 0, ..scalar };
    let layouts = [&raw const scalar, &raw const environment];
    for durable in [false, true] {
        let recording = super::super::record::Recording::new(b"native thaw wait", durable);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            |ctx| unsafe {
                thaw_wait_body(ctx, false, std::ptr::null());
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
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        for width in [2, 8] {
            let replay = Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let waited = Arc::new(AtomicBool::new(false));
            let checked = waited.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 2;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    thaw_wait_body(ctx, true, Arc::as_ptr(&checked));
                    panic!("replay passed the scope-exit cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert!(
                waited.load(Ordering::Acquire),
                "a saved wait released the sole permit"
            );
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

unsafe fn thaw_wait_body(ctx: *mut TaskCtx, replay: bool, waited: *const AtomicBool) {
    unsafe {
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let root = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(root).cast::<u64>().write(7);
        crate::roots::hirsel_rt_v2_root_dup(ctx, root);
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 3, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<*mut crate::Header>()
            .write(root);
        let mut task = 0;
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut task,
            &scope,
            release_child,
            env,
            0,
        );
        let _role = role::Guard::new(ctx);
        if !replay {
            // Perturb only the physical schedule, without adding tape ops.
            while !crate::roots::sole(root) {
                super::super::suspend(&mut *ctx, true);
            }
        } else {
            assert_eq!(
                crate::roots::count(&*ctx, root),
                2,
                "the sole permit keeps the releasing child queued"
            );
        }
        let parks = scheduler(ctx).parks;
        let mut last = 99;
        crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root);
        assert_eq!(last, 1);
        assert!(crate::roots::sole(root));
        if replay {
            assert!(
                scheduler(ctx).parks > parks,
                "recorded Ok waited instead of failing or spinning"
            );
            (*waited).store(true, Ordering::Release);
        }
        crate::roots::hirsel_rt_v2_root_release(ctx, root);
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}

#[test]
fn native_replay_forces_cancellation_changes_only_at_unshielded_checks() {
    for durable in [false, true] {
        let recording = super::super::record::Recording::new(b"native cancel replay", durable);
        let install = recording.clone();
        let cutter = recording.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let mut cause = 99;
                hirsel_rt_v2_task_canceled(ctx, &mut cause, &0);
                assert_eq!(cause, 0);
                cutter.recorder.flush().unwrap().unwrap();
                let _role = role::Guard::new(ctx);
                scheduler(ctx).cancel(ROOT, CANCEL_REQUESTED);
                (*ctx).shield = 1;
                hirsel_rt_v2_task_canceled(ctx, &mut cause, &0);
                assert_eq!(cause, 0);
                (*ctx).shield = 0;
                for _ in 0..2 {
                    hirsel_rt_v2_task_canceled(ctx, &mut cause, &0);
                    assert_eq!(cause, CANCEL_REQUESTED + 1);
                }
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let trace = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let flush = (1..bytes.len())
            .find(|&end| {
                hirsel_free::tape::parse(&bytes[..end]).is_ok_and(|tape| {
                    tape.cut.get(&Vec::new()) == Some(&Progress::new(1, Phase::Done))
                })
            })
            .unwrap();
        for end in [flush, flush + 1, bytes.len()] {
            let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
            let full = end == bytes.len();
            let expected = Projection::new(
                if full { "returned" } else { "truncated" },
                &parsed.cut,
                &trace,
            );
            for width in [2, 8] {
                for live_cause in [None, Some(CANCEL_PARENT)] {
                    let replay = Replay::new(&bytes[..end]).unwrap();
                    let install = replay.clone();
                    let outputs = Arc::new(std::array::from_fn::<_, 4, _>(|_| {
                        std::sync::atomic::AtomicU32::new(99)
                    }));
                    let observed = outputs.clone();
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe { install.root(ctx) },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            if let Some(cause) = live_cause {
                                scheduler(ctx).cancel(ROOT, cause);
                            }
                            for (index, shield) in [0, 1, 0, 0].into_iter().enumerate() {
                                (*ctx).shield = shield;
                                hirsel_rt_v2_task_canceled(ctx, observed[index].as_ptr(), &0);
                            }
                            panic!("cancellation replay passed its cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(outputs[0].load(Ordering::Acquire), 0);
                    assert_eq!(
                        outputs[1].load(Ordering::Acquire),
                        if full { 0 } else { 99 }
                    );
                    for index in 2..4 {
                        assert_eq!(
                            outputs[index].load(Ordering::Acquire),
                            if full { CANCEL_REQUESTED + 1 } else { 99 }
                        );
                    }
                    assert_eq!(replay.finish().unwrap(), expected);
                }
            }
        }
    }
}
