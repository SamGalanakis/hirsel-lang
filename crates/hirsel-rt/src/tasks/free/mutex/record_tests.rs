//! A real native waiting take, entered cut, later acquisition and gap replay.
use super::*;
use crate::tasks::free::record;
use crate::tasks::free::replay::Replay;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn native_mutex_replay_creates_its_identity_and_consumes_scalar_take_put_versions() {
    let image = super::tests::layout();
    let wrapper = crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..super::tests::layout()
    };
    let layouts = [&raw const image, &raw const wrapper];
    for durable in [false, true] {
        let recording = record::Recording::new(b"native mutex replay", durable);
        let install = recording.clone();
        let cutter = recording.clone();
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
            move |ctx| unsafe {
                let mut handle = 0;
                let mut payload = [1u64, 7];
                let none = [0u64; 2];
                hirsel_rt_v2_mutex_create(
                    ctx,
                    std::ptr::null_mut(),
                    &mut handle,
                    payload.as_mut_ptr().cast(),
                    none.as_ptr().cast(),
                    2,
                    3,
                );
                assert_eq!(payload, none);
                cutter.recorder.flush().unwrap().unwrap();
                let mut status = 99;
                hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                assert_eq!(status, 0);
                assert_eq!(payload, [1, 7]);
                hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                assert_eq!(payload, none);
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
        for end in [bytes.len(), flush, flush + 1] {
            let parsed = hirsel_free::tape::parse(&bytes[..end]).unwrap();
            let full = end == bytes.len();
            let expected = Projection::new(
                if full { "returned" } else { "truncated" },
                &parsed.cut,
                &trace,
            );
            for width in [1, 2, 8] {
                let replay = Replay::new(&bytes[..end]).unwrap();
                let install = replay.clone();
                let stage = Arc::new(std::sync::atomic::AtomicU8::new(0));
                let observed = stage.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe { install.root(ctx) },
                    move |ctx| unsafe {
                        let mut handle = 0;
                        let mut payload = [1u64, 7];
                        let none = [0u64; 2];
                        hirsel_rt_v2_mutex_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut handle,
                            payload.as_mut_ptr().cast(),
                            none.as_ptr().cast(),
                            2,
                            3,
                        );
                        assert_eq!(payload, none);
                        observed.store(1, Ordering::Release);
                        let mut status = 99;
                        hirsel_rt_v2_mutex_take(
                            ctx,
                            &mut status,
                            &handle,
                            payload.as_mut_ptr().cast(),
                        );
                        assert_eq!(status, 0);
                        assert_eq!(payload, [1, 7]);
                        observed.store(2, Ordering::Release);
                        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                        panic!("mutex replay passed its cut");
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(stage.load(Ordering::Acquire), if full { 2 } else { 0 });
                assert_eq!(replay.finish().unwrap(), expected);
            }
        }
    }
}

struct Child {
    handle: u64,
    finished: AtomicBool,
}

unsafe extern "C" fn versioned_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let (handle, checked) = unsafe {
        crate::alloc::payload(env)
            .cast::<(u64, *const AtomicBool)>()
            .read()
    };
    let mut payload = [0u64; 2];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
        assert_eq!(status, 0);
        assert_eq!(payload, [1, 7]);
        payload[1] = 8;
        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
        (*checked).store(true, Ordering::Release);
    }
    STATUS_OK
}

unsafe fn versioned_mutex_body(ctx: *mut TaskCtx, checked: &AtomicBool, replay: bool) {
    unsafe {
        let mut handle = 0;
        let mut payload = [1u64, 7];
        let none = [0u64; 2];
        hirsel_rt_v2_mutex_create(
            ctx,
            std::ptr::null_mut(),
            &mut handle,
            payload.as_mut_ptr().cast(),
            none.as_ptr().cast(),
            2,
            3,
        );
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 4, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<(u64, *const AtomicBool)>()
            .write((handle, checked));
        let mut child = 0;
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut child,
            &scope,
            versioned_child,
            env,
            0,
        );
        let parks = {
            let _role = role::Guard::new(ctx);
            if !replay {
                let scope = scheduler(ctx).scope_handle(scope);
                try_block(&mut *ctx, Wait::Exit { scope }, false, 0).unwrap();
                assert!(checked.load(Ordering::Acquire));
            } else {
                assert!(!checked.load(Ordering::Acquire));
                let cell = scheduler(ctx).sync.mutex(mutex_word(&handle));
                assert!(
                    cell.lock().permit.available(),
                    "the live cell is ready before its recorded predecessors"
                );
            }
            scheduler(ctx).parks
        };
        let mut status = 99;
        hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
        assert_eq!(status, 0);
        assert_eq!(payload, [1, 8]);
        assert!(checked.load(Ordering::Acquire));
        if replay {
            let _role = role::Guard::new(ctx);
            assert!(
                scheduler(ctx).parks > parks,
                "the version wait saved its stack and released the sole permit"
            );
        }
        hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}

#[test]
fn native_mutex_replay_waits_for_versions_before_taking_an_available_cell() {
    const CHILD: &str = "HIRSEL_F1_MUTEX_REPLAY_GAPS";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::mutex::record_tests::native_mutex_replay_waits_for_versions_before_taking_an_available_cell", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    let image = super::tests::layout();
    let wrapper = crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..super::tests::layout()
    };
    let env = super::tests::layout();
    let layouts = [&raw const image, &raw const wrapper, &raw const env];
    for durable in [false, true] {
        let recording = record::Recording::new(b"native mutex version replay", durable);
        let install = recording.clone();
        let checked = Arc::new(AtomicBool::new(false));
        let observed = checked.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 3;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                versioned_mutex_body(ctx, &observed, false);
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
        for width in [1, 2, 8] {
            let replay = Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let checked = Arc::new(AtomicBool::new(false));
            let observed = checked.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 3;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    versioned_mutex_body(ctx, &observed, true);
                    panic!("mutex replay passed scope exit cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

unsafe fn grouped_mutex_body(ctx: *mut TaskCtx, checked: &[AtomicBool], replay: bool) {
    unsafe {
        let mut handles = Vec::new();
        for _ in checked {
            let mut handle = 0;
            let mut payload = [1u64, 7];
            let none = [0u64; 2];
            hirsel_rt_v2_mutex_create(
                ctx,
                std::ptr::null_mut(),
                &mut handle,
                payload.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                2,
                3,
            );
            handles.push(handle);
        }
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        for (handle, checked) in handles.iter().zip(checked) {
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 4, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<(u64, *const AtomicBool)>()
                .write((*handle, checked));
            let mut child = 0;
            hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                &mut child,
                &scope,
                versioned_child,
                env,
                0,
            );
        }
        if !replay {
            let _role = role::Guard::new(ctx);
            let scope = scheduler(ctx).scope_handle(scope);
            try_block(&mut *ctx, Wait::Exit { scope }, false, 0).unwrap();
        }
        // Each take waits for its own child's take and put versions.
        for (handle, checked) in handles.iter().zip(checked) {
            let mut payload = [0u64; 2];
            let mut status = 99;
            hirsel_rt_v2_mutex_take(ctx, &mut status, handle, payload.as_mut_ptr().cast());
            assert_eq!((status, payload), (0, [1, 8]));
            assert!(checked.load(Ordering::Acquire));
            hirsel_rt_v2_mutex_put(ctx, handle, payload.as_mut_ptr().cast());
        }
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}

#[test]
fn native_mutex_replay_wakes_only_the_group_whose_version_advanced() {
    const CHILD: &str = "HIRSEL_F1_MUTEX_REPLAY_GROUPS";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::mutex::record_tests::native_mutex_replay_wakes_only_the_group_whose_version_advanced", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    // One compute permit: every version wait saves its stack and releases it.
    crate::pool::set_compute_grant(1);
    let image = super::tests::layout();
    let wrapper = crate::Layout {
        size: 8,
        flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
        ..super::tests::layout()
    };
    let env = super::tests::layout();
    let layouts = [&raw const image, &raw const wrapper, &raw const env];
    for groups in [1usize, 2, 4] {
        let recording = record::Recording::new(b"native mutex replay groups", true);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 3;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let checked: Vec<_> = (0..groups).map(|_| AtomicBool::new(false)).collect();
                grouped_mutex_body(ctx, &checked, false);
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
        let expected = Projection::new("returned", &cut.progress, &trace);
        for width in [1, 2, 4] {
            let replay = Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 3;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    let checked: Vec<_> = (0..groups).map(|_| AtomicBool::new(false)).collect();
                    grouped_mutex_body(ctx, &checked, true);
                    panic!("mutex replay passed scope exit cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            // The root registers on one group's cell at a time, and only that
            // child's take and put examine it.
            let visits = replay.visits();
            assert!(
                visits <= 2 * groups as u64,
                "{groups} groups at width {width}: {visits} registrations visited"
            );
            assert_eq!(
                replay.finish().unwrap(),
                expected,
                "{groups} groups at width {width}"
            );
        }
    }
}

unsafe extern "C" fn body(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let shared = unsafe { Box::from_raw(data.cast::<Arc<Child>>()) };
    let mut payload = [0u64; 2];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_mutex_take(
            ctx,
            &mut status,
            &shared.handle,
            payload.as_mut_ptr().cast(),
        );
        assert_eq!(status, 0);
        assert_eq!(payload, [1, 7]);
        hirsel_rt_v2_mutex_put(ctx, &shared.handle, payload.as_mut_ptr().cast());
        assert_eq!(payload, [0, 0]);
    }
    shared.finished.store(true, Ordering::Release);
    drop(shared);
    unsafe {
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}

#[test]
fn waiting_native_mutex_keeps_one_ordinal_and_replays_both_cuts() {
    const CHILD: &str = "HIRSEL_F1_MUTEX_RECORDING";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::mutex::record_tests::waiting_native_mutex_keeps_one_ordinal_and_replays_both_cuts", "--nocapture"])
            .env(CHILD, "1")
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(2);
    for durable in [false, true] {
        let shared = record::Recording::new(b"native waiting mutex", durable);
        let install = shared.clone();
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let image = super::tests::layout();
        let wrapper = crate::Layout {
            size: 8,
            flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
            ..super::tests::layout()
        };
        let layouts = [&raw const image, &raw const wrapper];
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 2;
        let (cut_now, cut_ready) = std::sync::mpsc::sync_channel(1);
        let (cut_done, resume) = std::sync::mpsc::sync_channel(1);
        let cutter = std::thread::spawn({
            let shared = shared.clone();
            move || {
                cut_ready.recv().unwrap();
                let cut = shared.recorder.flush().unwrap().unwrap();
                assert_eq!(cut.progress[&vec![0]], Progress::new(1, Phase::Entered));
                cut_done.send(()).unwrap();
            }
        });
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| {
                let mut handle = 0;
                let mut payload = [1u64, 7];
                let none = [0u64; 2];
                unsafe {
                    hirsel_rt_v2_mutex_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut handle,
                        payload.as_mut_ptr().cast(),
                        none.as_ptr().cast(),
                        2,
                        3,
                    );
                }
                assert_eq!(payload, none);
                let mut status = 99;
                unsafe {
                    hirsel_rt_v2_mutex_take(ctx, &mut status, &handle, payload.as_mut_ptr().cast());
                }
                assert_eq!(status, 0);
                let child = Arc::new(Child {
                    handle,
                    finished: AtomicBool::new(false),
                });
                let cell = unsafe {
                    let _role = role::Guard::new(ctx);
                    let stack = Stack::new();
                    let child_ctx = stack.context();
                    child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                    let log = record::child(ctx).unwrap();
                    (*child_ctx).recording = Some(Box::new(log));
                    let s = scheduler(ctx);
                    let id = s.tasks.len() as u32;
                    (*child_ctx).task_id = id;
                    (*child_ctx).task_sp = stack.prepare(
                        body,
                        child_ctx.cast(),
                        Box::into_raw(Box::new(child.clone())).cast(),
                    );
                    let mut row = Task::new(1, child_ctx, None, vec![0], None);
                    row.stack = Some(stack);
                    s.tasks.push(Some(row));
                    crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
                    scheduler(ctx).sync.mutex(mutex_word(&handle))
                };
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while cell.lock().permit.waiting() != 1 {
                    assert!(
                        std::time::Instant::now() < until,
                        "take must publish entered"
                    );
                    std::thread::yield_now();
                }
                drop(cell);
                cut_now.send(()).unwrap();
                resume.recv().unwrap();
                unsafe {
                    hirsel_rt_v2_mutex_put(ctx, &handle, payload.as_mut_ptr().cast());
                }
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !child.finished.load(Ordering::Acquire) {
                    assert!(
                        std::time::Instant::now() < until,
                        "the same take ordinal must finish"
                    );
                    std::thread::yield_now();
                }
                STATUS_OK
            },
        );
        cutter.join().unwrap();
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        let cut = shared.recorder.stop("returned", None).unwrap().unwrap().0;
        assert_eq!(cut.progress[&vec![0]], Progress::new(2, Phase::Done));
        let trace = shared.trace();
        let marked = Arc::try_unwrap(shared).ok().unwrap().recorder.into_sink();
        for end in marked.marks {
            let bytes = &marked.bytes[..end];
            let tape = hirsel_free::tape::parse(bytes).unwrap();
            if tape.cut.is_empty() {
                continue;
            }
            let terminal = end == marked.bytes.len();
            let mut replay = Session::replay(bytes).unwrap();
            let handle = (crate::free::MUTEX, 1);
            assert!(replay.start(&[]));
            assert!(replay.start(&[0]));
            replay.begin(&[]);
            replay.create(&[], handle).unwrap();
            replay.publish(&[], Phase::Done).unwrap();
            replay.begin(&[]);
            replay.touch(&[], 0, handle, "take").unwrap();
            replay.publish(&[], Phase::Done).unwrap();
            replay.begin(&[]);
            replay.publish(&[], Phase::Done).unwrap();
            replay.begin(&[0]);
            replay.publish(&[0], Phase::Entered).unwrap();
            if terminal {
                assert!(!replay.enabled(&[0], 0, handle).unwrap());
                replay.begin(&[]);
                replay.touch(&[], 0, handle, "put").unwrap();
                replay.publish(&[], Phase::Done).unwrap();
                assert!(replay.enabled(&[0], 0, handle).unwrap());
                replay.touch(&[0], 0, handle, "take").unwrap();
                replay.publish(&[0], Phase::Done).unwrap();
                replay.begin(&[0]);
                replay.touch(&[0], 0, handle, "put").unwrap();
                replay.publish(&[0], Phase::Done).unwrap();
            } else {
                assert!(!replay.admits(&[0], Phase::Done));
            }
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new(
                    if terminal { "returned" } else { "truncated" },
                    &tape.cut,
                    &trace
                )
            );
        }
    }
}
