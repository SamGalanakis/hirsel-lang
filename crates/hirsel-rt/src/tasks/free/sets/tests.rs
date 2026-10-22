use super::*;
use crate::tasks::free::record;
use crate::tasks::free::replay::Replay;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct CompletionOrder {
    reverse: bool,
    early: bool,
    allow_first: AtomicBool,
    second: AtomicBool,
    waiter: std::sync::atomic::AtomicU32,
    checked: AtomicBool,
}

unsafe extern "C" fn reordered_result(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let (id, order) = unsafe {
        crate::alloc::payload(env)
            .cast::<(u64, *const CompletionOrder)>()
            .read()
    };
    let order = unsafe { &*order };
    if id == 0 && order.reverse {
        while !(order.second.load(Ordering::Acquire)
            && (!order.early || order.allow_first.load(Ordering::Acquire)))
        {
            let _role = unsafe { role::Guard::new(ctx) };
            if order.second.load(Ordering::Acquire)
                && (!order.early || order.allow_first.load(Ordering::Acquire))
            {
                break;
            }
            order
                .waiter
                .store(scheduler(ctx).current, Ordering::Release);
            unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }.unwrap();
        }
    }
    if id == 1 {
        let _role = unsafe { role::Guard::new(ctx) };
        order.second.store(true, Ordering::Release);
        let waiter = order.waiter.load(Ordering::Acquire);
        if waiter != u32::MAX && !order.early {
            let s = scheduler(ctx);
            s.task(waiter).retried = true;
            s.wake(waiter);
        }
        if order.early {
            let s = scheduler(ctx);
            s.task(ROOT).retried = true;
            s.wake(ROOT);
        }
    }
    let mut image = [1u64, id];
    let none = [0u64; 2];
    unsafe {
        hirsel_rt_v2_task_finish(
            ctx,
            image.as_mut_ptr().cast(),
            none.as_ptr().cast(),
            16,
            u32::MAX,
        )
    }
}

unsafe fn reordered_set_body(ctx: *mut TaskCtx, order: &CompletionOrder) {
    unsafe {
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let mut children = [0; 2];
        for (id, child) in children.iter_mut().enumerate() {
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<(u64, *const CompletionOrder)>()
                .write((id as u64, order));
            hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                child,
                &scope,
                reordered_result,
                env,
                0,
            );
            if !order.reverse {
                loop {
                    let _role = role::Guard::new(ctx);
                    let s = scheduler(ctx);
                    if s.task(*child as u32).state == State::Holding
                        && readiness::saved(s, *child as u32)
                    {
                        break;
                    }
                    let scope = scheduler(ctx).scope_handle(scope);
                    try_block(&mut *ctx, Wait::Exit { scope }, false, 0).unwrap();
                }
            }
        }
        loop {
            let _role = role::Guard::new(ctx);
            let s = scheduler(ctx);
            if order.early {
                let second = children[1] as u32;
                if s.task(second).state == State::Holding {
                    if readiness::saved(s, second) {
                        break;
                    }
                    // The early wake precedes the child's physical save.
                    // Retry has no notification for the later save of a
                    // Holding child. Let its idle callback acquire the role;
                    // it needs no compute permit to finish this save.
                    let _segment = role::Segment::new(ctx);
                    std::thread::yield_now();
                    continue;
                }
                try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0).unwrap();
                continue;
            }
            if children.iter().all(|&child| {
                s.tasks[child as u32 as usize].as_ref().unwrap().state == State::Holding
                    && readiness::saved(s, child as u32)
            }) {
                let first = s.task(children[0] as u32).completed;
                let second = s.task(children[1] as u32).completed;
                assert_eq!(
                    first > second,
                    order.reverse,
                    "actual native completion ranks were reversed"
                );
                break;
            }
            let scope = scheduler(ctx).scope_handle(scope);
            try_block(&mut *ctx, Wait::Exit { scope }, false, 0).unwrap();
        }
        let mut set = 0;
        hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
        hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[1]);
        hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[0]);
        let mut index = IndexOption {
            tag: 99,
            padding: 99,
            index: 99,
        };
        let parks = {
            let _role = role::Guard::new(ctx);
            let s = scheduler(ctx);
            if order.early {
                assert!(matches!(
                    s.task(children[0] as u32).state,
                    State::Ready | State::Parked
                ));
                order.allow_first.store(true, Ordering::Release);
                s.task(children[0] as u32).retried = true;
                s.wake(children[0] as u32);
            }
            s.parks
        };
        hirsel_rt_v2_task_await_any(ctx, 0, 0, std::ptr::null_mut(), &mut index, &scope, &set);
        assert_eq!(
            (index.tag, index.index),
            (1, 1),
            "replay selects the recorded add-order index"
        );
        if order.early {
            let _role = role::Guard::new(ctx);
            assert!(
                scheduler(ctx).parks > parks,
                "the recorded member was pending while another member was ready"
            );
        }
        order.checked.store(true, Ordering::Release);
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, set, 0, 0), STATUS_OK);
        }
        // An empty set has no recorded winner; it still publishes its op.
        hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
        hirsel_rt_v2_task_await_any(ctx, 0, 0, std::ptr::null_mut(), &mut index, &scope, &set);
        assert_eq!((index.tag, index.index), (0, 0));
        {
            let _role = role::Guard::new(ctx);
            assert_eq!(close_endpoint(ctx, set, 0, 0), STATUS_OK);
        }
        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
    }
}

#[test]
fn native_await_any_replay_uses_recorded_members_after_reversed_late_completion() {
    const CHILD: &str = "HIRSEL_F1_REPLAY_AWAIT_ANY";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::sets::tests::native_await_any_replay_uses_recorded_members_after_reversed_late_completion", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 16,
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
        let recording = record::Recording::new(b"native await_any replay", durable);
        let install = recording.clone();
        let order = Arc::new(CompletionOrder {
            reverse: false,
            early: false,
            allow_first: AtomicBool::new(false),
            second: AtomicBool::new(false),
            waiter: std::sync::atomic::AtomicU32::new(u32::MAX),
            checked: AtomicBool::new(false),
        });
        let observed = order.clone();
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
                reordered_set_body(ctx, &observed);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert!(order.checked.load(Ordering::Acquire));
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
        for (width, early) in [
            (1, false),
            (2, false),
            (8, false),
            (1, true),
            (2, true),
            (8, true),
        ] {
            let replay = Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let order = Arc::new(CompletionOrder {
                reverse: true,
                early,
                allow_first: AtomicBool::new(false),
                second: AtomicBool::new(false),
                waiter: std::sync::atomic::AtomicU32::new(u32::MAX),
                checked: AtomicBool::new(false),
            });
            let observed = order.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    reordered_set_body(ctx, &observed);
                    panic!("replay passed its set cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert!(order.checked.load(Ordering::Acquire));
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("returned", &cut.progress, &trace)
            );
        }
    }
}

unsafe extern "C" fn result(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
    let id = unsafe { environment.add(1).cast::<u64>().read() };
    let mut image = [1u64, id];
    let none = [0u64; 2];
    let status = unsafe {
        hirsel_rt_v2_task_finish(
            ctx,
            image.as_mut_ptr().cast(),
            none.as_ptr().cast(),
            16,
            u32::MAX,
        )
    };
    assert_eq!(image, none);
    status
}

unsafe extern "C" fn cut_entered_await(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let recording = unsafe {
        crate::alloc::payload(env)
            .cast::<*const record::Recording>()
            .read()
    };
    assert!(
        !recording.is_null(),
        "a START-only replay child must never enter user code"
    );
    let cut = unsafe { &*recording }
        .recorder
        .stop("stopped", None)
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(cut.progress[&Vec::new()], Progress::new(5, Phase::Entered));
    assert_eq!(cut.progress[&vec![0]], Progress::START);
    unsafe {
        (*ctx)
            .run_ref()
            .control
            .request_inside(crate::stop::Stop::Requested);
        crate::stop::raise(ctx, crate::stop::Stop::Requested);
    }
}

unsafe fn entered_await_body(ctx: *mut TaskCtx, recording: *const record::Recording) {
    unsafe {
        let mut scope = 0;
        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
        let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
        crate::alloc::payload(env)
            .cast::<*const record::Recording>()
            .write(recording);
        let mut child = 0;
        hirsel_rt_v2_task_spawn(
            ctx,
            0,
            0,
            std::ptr::null_mut(),
            &mut child,
            &scope,
            cut_entered_await,
            env,
            0,
        );
        let mut set = 0;
        hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
        hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut child);
        let mut index = IndexOption {
            tag: 99,
            padding: 99,
            index: 99,
        };
        hirsel_rt_v2_task_await_any(ctx, 0, 0, std::ptr::null_mut(), &mut index, &scope, &set);
        panic!("an Entered-only await_any resumed");
    }
}

#[test]
fn an_entered_only_native_await_any_cut_needs_no_recorded_member() {
    const CHILD: &str = "HIRSEL_F1_REPLAY_AWAIT_ENTERED";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::sets::tests::an_entered_only_native_await_any_cut_needs_no_recorded_member", "--nocapture"])
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
        let recording = record::Recording::new(b"native entered await_any", durable);
        let install = recording.clone();
        let observer = recording.clone();
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
                entered_await_body(ctx, Arc::as_ptr(&observer));
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while Arc::strong_count(&recording) != 1 {
            assert!(
                std::time::Instant::now() < until,
                "stopped recorder frames reclaim"
            );
            std::thread::yield_now();
        }
        let trace = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let cut = hirsel_free::tape::parse(&bytes).unwrap().cut;
        for width in [1, 2, 8] {
            let replay = Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                |ctx| unsafe {
                    entered_await_body(ctx, std::ptr::null());
                    STATUS_OK
                },
            );
            assert!(
                matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new("stopped", &cut, &trace)
            );
        }
    }
}

#[test]
fn late_set_creation_and_reverse_insertion_preserve_scope_completion_ranks() {
    for durable in [false, true] {
        let recording = record::Recording::new(b"native late task set", durable);
        let install = recording.clone();
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
        let mut run = crate::host::BodyRun::new(2, false);
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
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let mut children = [0u64; 2];
                for (id, child) in children.iter_mut().enumerate() {
                    let environment = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    environment.add(1).cast::<u64>().write(id as u64);
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        child,
                        &scope,
                        result,
                        environment,
                        0,
                    );
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        let held = {
                            let _role = role::Guard::new(ctx);
                            let s = scheduler(ctx);
                            s.task(*child as u32).state == State::Holding
                                && readiness::saved(s, *child as u32)
                        };
                        if held {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "the child must complete before set creation"
                        );
                        std::thread::yield_now();
                    }
                }
                let mut set = 0;
                hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
                hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[1]);
                hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[0]);
                let mut index = IndexOption {
                    tag: 99,
                    padding: 99,
                    index: 99,
                };
                hirsel_rt_v2_task_await_any(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut index,
                    &scope,
                    &set,
                );
                assert_eq!((index.tag, index.index), (1, 1));
                let mut has = 99;
                hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, 1);
                assert_eq!(has, 1);
                let mut task = 0;
                hirsel_rt_v2_task_set_take(
                    ctx,
                    std::ptr::null_mut(),
                    &mut task,
                    &scope,
                    &mut set,
                    1,
                );
                let mut image = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        image.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(image, [1, 0]);
                hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, 1);
                assert_eq!(has, 0);
                hirsel_rt_v2_task_await_any(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut index,
                    &scope,
                    &set,
                );
                assert_eq!((index.tag, index.index), (1, 0));
                hirsel_rt_v2_task_set_take(
                    ctx,
                    std::ptr::null_mut(),
                    &mut task,
                    &scope,
                    &mut set,
                    0,
                );
                image = [0, 0];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        image.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(image, [1, 1]);
                hirsel_rt_v2_task_await_any(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut index,
                    &scope,
                    &set,
                );
                assert_eq!((index.tag, index.index), (0, 0));
                {
                    let _role = role::Guard::new(ctx);
                    assert_eq!(close_endpoint(ctx, set, 0, 0), STATUS_OK);
                }
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(cut.progress[&vec![]], Progress::new(17, Phase::Done));
        let trace = recording.trace();
        let marked = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        let mut replay = Session::replay(&marked.bytes).unwrap();
        assert!(replay.start(&[]));
        for path in [&[0][..], &[1][..]] {
            assert!(replay.start(path));
            assert_eq!(cut.progress[path], Progress::new(2, Phase::Done));
            for _ in 0..2 {
                replay.begin(path);
                replay.publish(path, Phase::Entered).unwrap();
                replay.publish(path, Phase::Done).unwrap();
            }
        }
        for op in 1..=17 {
            replay.begin(&[]);
            if let Some(index) = match op {
                7 => Some(1),
                12 => Some(0),
                _ => None,
            } {
                assert_eq!(replay.await_any(&[], None, |_| true).unwrap(), Some(index));
            }
            if [10, 14, 17].contains(&op) {
                replay.publish(&[], Phase::Entered).unwrap();
            }
            replay.publish(&[], Phase::Done).unwrap();
        }
        assert_eq!(
            replay.finish().unwrap(),
            Projection::new("returned", &cut.progress, &trace)
        );
    }
}
