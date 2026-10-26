use super::*;
use crate::tasks::free::replay::Replay;
use crate::tasks::*;
use hirsel_free::projection::Projection;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn a_cut_admitting_entry_also_admits_its_yet_unfinished_nested_root_tail() {
    nested_entry(true);
}

fn nested_entry(cut_tail: bool) {
    for durable in [false, true] {
        for bound in [3, 100] {
            let shared = Recording::new(b"native root phase tail", durable);
            let install = shared.clone();
            let observer = shared.clone();
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
            let nested = crate::Step {
                kind: crate::STEP_RELEASE_ROOT,
                field: 0,
                variant: 0,
                span: 0,
                offset: 0,
                stride: 0,
                count: 0,
                count_offset: 0,
                layout: std::ptr::null(),
            };
            let outer = crate::Layout {
                steps: &nested,
                step_count: 1,
                ..scalar
            };
            let layouts = [&raw const scalar, &raw const outer];
            let mut run = crate::host::BodyRun::new(2, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = layouts.len() as u32;
            run.config.count_max = bound;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let inner = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(inner).cast::<u64>().write(11);
                    crate::roots::hirsel_rt_v2_root_dup(ctx, inner);
                    let outer = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        3,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(outer)
                        .cast::<*mut crate::Header>()
                        .write(inner);
                    for _ in 0..5 {
                        crate::roots::hirsel_rt_v2_root_dup(ctx, outer);
                    }
                    for _ in 0..6 {
                        crate::roots::hirsel_rt_v2_root_release(ctx, outer);
                    }
                    assert!((*ctx).roots.has_pending());
                    assert_eq!(crate::roots::count(&*ctx, inner), 2);
                    if cut_tail {
                        let _role = role::Guard::new(ctx);
                        begin(ctx);
                        let mut tail = crate::roots::PhaseRoots::prepare(ctx);
                        let mut task = (*ctx).recording.take().unwrap();
                        let mut worker = observer
                            .recorder
                            .executor_available(crate::pool::recorder_slot())
                            .unwrap();
                        observer
                            .recorder
                            .phase_if(
                                &mut worker,
                                &mut task.log,
                                Progress::new(task.ordinal, Phase::Entered),
                                hirsel_free::record::RecordBound::default(),
                                || tail.apply(ctx),
                                |_| (),
                            )
                            .unwrap()
                            .unwrap();
                        drop(worker);
                        (*ctx).recording = Some(task);
                        let cut = observer.recorder.flush().unwrap().unwrap();
                        assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Entered));
                        assert_eq!(
                            crate::roots::count(&*ctx, inner),
                            2,
                            "the cut precedes the frozen traversal tail"
                        );
                        tail.finish(ctx);
                        publish(ctx, Phase::Done, |_| ()).unwrap();
                    } else {
                        let mut atomic = 0;
                        let value = 0u64;
                        hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut atomic,
                            (&raw const value).cast(),
                            8,
                            0,
                            8,
                        );
                    }
                    assert!(!(*ctx).roots.has_pending());
                    assert_eq!(
                        crate::roots::count(&*ctx, inner),
                        1,
                        "nested phase release cannot be deferred again"
                    );
                    let mut last = 99;
                    crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, inner);
                    assert_eq!(last, 1);
                    crate::roots::hirsel_rt_v2_root_release(ctx, inner);
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let cut = shared.recorder.stop("returned", None).unwrap().unwrap().0;
            assert_eq!(cut.progress[&Vec::new()], Progress::new(2, Phase::Done));
            let trace = shared.trace();
            let marked = Arc::try_unwrap(shared).ok().unwrap().recorder.into_sink();
            for end in marked.marks {
                let bytes = &marked.bytes[..end];
                let tape = hirsel_free::tape::parse(bytes).unwrap();
                let mut replay = hirsel_free::session::Session::replay(bytes).unwrap();
                if let Some(progress) = tape.cut.get(&Vec::new()) {
                    assert!(replay.start(&[]));
                    replay.begin(&[]);
                    replay.publish(&[], Phase::Entered).unwrap();
                    if progress.op > 1 || progress.phase == Phase::Done {
                        replay.publish(&[], Phase::Done).unwrap();
                    }
                    if progress.op == 2 {
                        replay.begin(&[]);
                        assert!(replay.thaw(&[], true).unwrap());
                        replay.publish(&[], Phase::Done).unwrap();
                    }
                } else {
                    assert!(!replay.start(&[]));
                }
                assert_eq!(
                    replay.finish().unwrap(),
                    hirsel_free::projection::Projection::new(
                        if end == marked.bytes.len() {
                            "returned"
                        } else {
                            "truncated"
                        },
                        &tape.cut,
                        &trace
                    )
                );
            }
        }
    }
}

unsafe extern "C" fn root_child(ctx: *mut TaskCtx, env: *mut crate::Header, _: u64, _: u64) -> u8 {
    let root = unsafe {
        crate::alloc::payload(env)
            .cast::<*mut crate::Header>()
            .read()
    };
    for _ in 0..6 {
        unsafe { crate::roots::hirsel_rt_v2_root_release(ctx, root) };
    }
    let mut image = [1u64, 42];
    let none = [0u64; 2];
    unsafe {
        hirsel_rt_v2_task_finish(
            ctx,
            image.as_mut_ptr().cast(),
            none.as_ptr().cast(),
            16,
            u32::MAX,
        )
    };
    assert_eq!(image, none);
    crate::STATUS_OK
}

#[test]
fn recorded_child_end_finishes_its_nested_root_tail_before_exposing_the_result() {
    for width in [2, 8] {
        for durable in [false, true] {
            let shared = Recording::new(b"native root at child end", durable);
            let install = shared.clone();
            let cutter = shared.clone();
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
            let nested = crate::Step {
                kind: crate::STEP_RELEASE_ROOT,
                field: 0,
                variant: 0,
                span: 0,
                offset: 0,
                stride: 0,
                count: 0,
                count_offset: 0,
                layout: std::ptr::null(),
            };
            let outer = crate::Layout {
                steps: &nested,
                step_count: 1,
                ..scalar
            };
            let env_layout = crate::Layout { flags: 0, ..scalar };
            let layouts = [&raw const scalar, &raw const outer, &raw const env_layout];
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = layouts.len() as u32;
            run.config.count_max = 3;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let inner = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(inner).cast::<u64>().write(11);
                    crate::roots::hirsel_rt_v2_root_dup(ctx, inner);
                    let outer = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        3,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(outer)
                        .cast::<*mut crate::Header>()
                        .write(inner);
                    for _ in 0..5 {
                        crate::roots::hirsel_rt_v2_root_dup(ctx, outer);
                    }
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        4,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env)
                        .cast::<*mut crate::Header>()
                        .write(outer);
                    let mut child = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        root_child,
                        env,
                        0,
                    );
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        let held = {
                            let _role = role::Guard::new(ctx);
                            let s = scheduler(ctx);
                            s.task(child as u32).state == State::Holding
                                && readiness::saved(s, child as u32)
                        };
                        if held {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "child result reaches idle"
                        );
                        std::thread::yield_now();
                    }
                    let cut = cutter.recorder.flush().unwrap().unwrap();
                    assert_eq!(cut.progress[&vec![0]], Progress::new(1, Phase::Done));
                    assert_eq!(crate::roots::count(&*ctx, inner), 1);
                    let mut last = 99;
                    crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, inner);
                    assert_eq!(last, 1);
                    let mut result = [0u64; 2];
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child,
                        result.as_mut_ptr().cast(),
                        16,
                    );
                    assert_eq!(result, [1, 42]);
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                    crate::roots::hirsel_rt_v2_root_release(ctx, inner);
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let cut = shared.recorder.stop("returned", None).unwrap().unwrap().0;
            assert_eq!(cut.progress[&Vec::new()], Progress::new(5, Phase::Done));
            let trace = shared.trace();
            let marked = Arc::try_unwrap(shared).ok().unwrap().recorder.into_sink();
            let mut replay = hirsel_free::session::Session::replay(&marked.bytes).unwrap();
            assert!(replay.start(&[]));
            assert!(replay.start(&[0]));
            assert_eq!(cut.progress[&vec![0]], Progress::new(2, Phase::Done));
            for _ in 0..2 {
                replay.begin(&[0]);
                replay.publish(&[0], Phase::Entered).unwrap();
                replay.publish(&[0], Phase::Done).unwrap();
            }
            for op in 1..=5 {
                replay.begin(&[]);
                if op == 3 {
                    assert!(replay.thaw(&[], true).unwrap());
                }
                if op >= 4 {
                    replay.publish(&[], Phase::Entered).unwrap();
                }
                replay.publish(&[], Phase::Done).unwrap();
            }
            assert_eq!(
                replay.finish().unwrap(),
                hirsel_free::projection::Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

#[test]
fn native_replay_applies_nested_root_tails_at_an_entered_only_cut() {
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
    let nested = crate::Step {
        kind: crate::STEP_RELEASE_ROOT,
        field: 0,
        variant: 0,
        span: 0,
        offset: 0,
        stride: 0,
        count: 0,
        count_offset: 0,
        layout: std::ptr::null(),
    };
    let outer = crate::Layout {
        steps: &nested,
        step_count: 1,
        ..scalar
    };
    let layouts = [&raw const scalar, &raw const outer];
    for durable in [false, true] {
        for bound in [3, 100] {
            let recording = super::super::record::Recording::new(b"root tail replay", durable);
            let install = recording.clone();
            let cutter = recording.clone();
            let mut run = crate::host::BodyRun::new(8, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 2;
            run.config.count_max = bound;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    let inner = pending_nested_roots(ctx);
                    // Force the exact flush between the native entry's primary
                    // decrements and traversal, as the cutter may do live.
                    super::super::record::begin(ctx);
                    let mut tail = crate::roots::PhaseRoots::prepare(ctx);
                    let mut task = (*ctx).recording.take().unwrap();
                    let mut worker = cutter
                        .recorder
                        .executor_available(crate::pool::recorder_slot())
                        .unwrap();
                    cutter
                        .recorder
                        .phase_if(
                            &mut worker,
                            &mut task.log,
                            Progress::new(1, Phase::Entered),
                            hirsel_free::record::RecordBound::default(),
                            || tail.apply(ctx),
                            |_| (),
                        )
                        .unwrap()
                        .unwrap();
                    drop(worker);
                    (*ctx).recording = Some(task);
                    cutter.recorder.flush().unwrap().unwrap();
                    assert_eq!(crate::roots::count(&*ctx, inner), 2);
                    tail.finish(ctx);
                    super::super::record::publish(ctx, Phase::Done, |_| ()).unwrap();
                    assert_eq!(crate::roots::count(&*ctx, inner), 1);
                    crate::roots::hirsel_rt_v2_root_release(ctx, inner);
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
                        tape.cut.get(&Vec::new()) == Some(&Progress::new(1, Phase::Entered))
                    })
                })
                .unwrap();
            for end in [flush, flush + 1, bytes.len()] {
                let full = end == bytes.len();
                let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
                let expected = Projection::new(
                    if full { "returned" } else { "truncated" },
                    &parsed.cut,
                    &trace,
                );
                for width in [1, 2, 8] {
                    let replay = Replay::new(&bytes[..end]).unwrap();
                    let install = replay.clone();
                    let checked = Arc::new(AtomicBool::new(false));
                    let observed = checked.clone();
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 2;
                    run.config.count_max = bound;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe { install.root(ctx) },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            let inner = pending_nested_roots(ctx);
                            let mut word = 0;
                            let result = super::super::record::atomic_create(
                                ctx,
                                &mut word,
                                (&0u64 as *const u64).cast(),
                                8,
                                0,
                                8,
                            );
                            // Runtime-owner check before propagating the cut:
                            // no user continuation or object action is admitted.
                            assert!(!(*ctx).roots.has_pending(), "entry applies pending roots");
                            assert_eq!(
                                crate::roots::count(&*ctx, inner),
                                1,
                                "entry admits its nested tail"
                            );
                            assert_eq!(word == 0, !full);
                            observed.store(true, Ordering::Release);
                            crate::roots::hirsel_rt_v2_root_release(ctx, inner);
                            crate::stop::check(ctx, result);
                            super::super::try_yield(&mut *ctx).unwrap();
                            panic!("replay passed the root-tail cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert!(checked.load(Ordering::Acquire));
                    assert_eq!(replay.finish().unwrap(), expected);
                }
            }
        }
    }
}

unsafe fn pending_nested_roots(ctx: *mut TaskCtx) -> *mut crate::Header {
    unsafe {
        let inner = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(inner).cast::<u64>().write(11);
        crate::roots::hirsel_rt_v2_root_dup(ctx, inner);
        let outer = crate::alloc::hirsel_rt_v2_alloc(ctx, 3, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(outer)
            .cast::<*mut crate::Header>()
            .write(inner);
        for _ in 0..5 {
            crate::roots::hirsel_rt_v2_root_dup(ctx, outer);
        }
        for _ in 0..6 {
            crate::roots::hirsel_rt_v2_root_release(ctx, outer);
        }
        assert!((*ctx).roots.has_pending());
        assert_eq!(crate::roots::count(&*ctx, inner), 2);
        inner
    }
}
#[test]
fn entered_publication_releases_checkout_on_rejection_and_finishes_roots_once() {
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
    let nested = crate::Step {
        kind: crate::STEP_RELEASE_ROOT,
        field: 0,
        variant: 0,
        span: 0,
        offset: 0,
        stride: 0,
        count: 0,
        count_offset: 0,
        layout: std::ptr::null(),
    };
    let outer = crate::Layout {
        steps: &nested,
        step_count: 1,
        ..scalar
    };
    let layouts = [&raw const scalar, &raw const outer];
    let recording = Recording::new(b"entered root admission", false);
    let install = recording.clone();
    let run_once = |install: Box<dyn FnOnce(*mut TaskCtx)>, replay: bool| {
        let mut run = crate::host::BodyRun::new(1, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        run.config.count_max = 3;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(install, move |ctx| unsafe {
            let _role = role::Guard::new(ctx);
            let inner = pending_nested_roots(ctx);
            assert!(begin_entered(ctx));
            let mut effects = 0u32;
            let overflow = &(*(*ctx).run_ptr()).root_overflow;
            let held = overflow.test_hold();
            assert!(
                poll_publish(ctx, Phase::Done, |_| {
                    effects += 1;
                    42
                })
                .unwrap()
                .is_none()
            );
            assert_eq!(effects, 0);
            assert_eq!(crate::roots::count(&*ctx, inner), 2);
            assert!((*ctx).roots.has_pending());
            assert_eq!(
                (*ctx).recording.is_some(),
                !replay,
                "checkout returned before Pending"
            );
            if let Some(task) = &(*ctx).recording {
                assert_eq!(task.ordinal, 1);
            }
            drop(held);
            assert_eq!(
                poll_publish(ctx, Phase::Done, |_| {
                    effects += 1;
                    42
                })
                .unwrap(),
                Some(42)
            );
            assert_eq!(effects, 1);
            assert!(!(*ctx).roots.has_pending());
            assert_eq!(
                crate::roots::count(&*ctx, inner),
                1,
                "frozen nested tail finished exactly once"
            );
            if replay {
                assert!(!begin_entered(ctx), "next ordinal rejects at the saved cut");
            }
            crate::roots::hirsel_rt_v2_root_release(ctx, inner);
            STATUS_OK
        });
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    };
    run_once(
        Box::new(move |ctx| unsafe {
            super::super::enable(ctx);
            install.root(ctx)
        }),
        false,
    );
    let cut = recording
        .recorder
        .stop("returned", None)
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Done));
    let trace = recording.trace();
    let bytes = Arc::try_unwrap(recording)
        .ok()
        .unwrap()
        .recorder
        .into_sink()
        .bytes;
    let replay = Replay::new(&bytes).unwrap();
    let install = replay.clone();
    run_once(Box::new(move |ctx| unsafe { install.root(ctx) }), true);
    assert_eq!(
        replay.finish().unwrap(),
        Projection::new("returned", &cut.progress, &trace)
    );
}
